//! The agent hook. Claude Code, Codex, and Cursor run it before every file edit and
//! shell command. It replays the proposed tool call against the file in memory and denies
//! the call when a locked region would change without a person's approval, so the
//! file on disk is never touched.

use crate::paths::{eq, relative};
use crate::regions::{hash, regions, supports_markers};
use crate::repo::{
    APPROVALS, FILE_LOCKS, WHOLE_FILE, approved, disk_approvals, find_root, parse_file_locks,
};
use crate::{patch, pending, shell};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub enum Host {
    /// Claude Code and Codex share the `PreToolUse` output contract.
    Claude,
    Cursor,
}

/// Agent configuration that keeps this hook running. An agent may edit these files
/// but may not take the hook out or switch hooks off.
const HOOK_CONFIGS: [&str; 4] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".cursor/hooks.json",
    ".codex/hooks.json",
];
const SHELL_TOOLS: [&str; 8] = [
    "exec",
    "bash",
    "powershell",
    "shell",
    "terminal",
    "run_terminal_cmd",
    "exec_command",
    "local_shell",
];

pub fn run(host: Host) -> i32 {
    let mut input = String::new();
    let decision = std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("could not read the hook input: {error}"))
        .and_then(|_| {
            serde_json::from_str::<Value>(&input)
                .map_err(|_| "the hook input is not JSON".to_string())
        })
        .and_then(|payload| decide(&payload));
    match decision {
        Ok(None) => {
            if let Host::Cursor = host {
                println!("{}", json!({ "permission": "allow" }));
            }
            0
        }
        Ok(Some(reason)) => {
            println!("{}", deny(host, &reason));
            0
        }
        Err(error) => {
            // Fail closed: when the hook cannot tell what a call does, the call does not run.
            let reason = format!("lock could not check this tool call, so it was blocked: {error}");
            println!("{}", deny(host, &reason));
            eprintln!("{reason}");
            2
        }
    }
}

fn deny(host: Host, reason: &str) -> Value {
    match host {
        Host::Claude => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        Host::Cursor => {
            json!({ "permission": "deny", "user_message": reason, "agent_message": reason })
        }
    }
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

/// A shell command as text. Codex sends some commands as an argument list.
fn command_text(value: &Value) -> Option<String> {
    let command = value.get("command").or_else(|| value.get("cmd"))?;
    match command {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// `None` allows the call; `Some(reason)` denies it.
pub fn decide(payload: &Value) -> Result<Option<String>, String> {
    let cwd = text(payload, &["cwd"])
        .map(PathBuf::from)
        .or_else(|| {
            payload
                .get("workspace_roots")
                .and_then(Value::as_array)
                .and_then(|roots| roots.first())
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or("no working directory")?;
    let Some(root) = find_root(&cwd) else {
        return Ok(None);
    };
    let tool = text(payload, &["tool_name", "toolName"])
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let input = payload
        .get("tool_input")
        .or_else(|| payload.get("arguments"))
        .unwrap_or(&Value::Null);
    let command = command_text(input).or_else(|| command_text(payload));
    let changes = if SHELL_TOOLS.contains(&tool.as_str()) || (input.is_null() && command.is_some())
    {
        let Some(command) = command else {
            return Ok(None);
        };
        // Codex can also run `apply_patch` through the shell with the patch inline.
        let Some(start) = command.find("*** Begin Patch") else {
            return Ok(shell::denial(&root, &cwd, &command));
        };
        let body = &command[start..];
        let end = body.find("*** End Patch").map_or(body.len(), |at| at + 13);
        patch_changes(&root, &cwd, &body[..end])
    } else {
        proposals(&root, &cwd, &tool, input, payload)?
    };
    let mut reasons = Vec::new();
    for change in changes {
        if let Some(reason) = judge(&root, &change)? {
            reasons.push(reason);
        }
    }
    Ok((!reasons.is_empty()).then(|| reasons.join("\n\n")))
}

/// What a tool call would leave in one file.
enum After {
    Content(String),
    Deleted,
    /// The hook cannot reconstruct the result, such as a notebook cell edit or a patch.
    Unknown(&'static str),
}

struct Change {
    path: String,
    before: Option<String>,
    after: After,
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn read(root: &Path, path: &str) -> Option<String> {
    fs::read(root.join(path))
        .ok()
        .map(|bytes| normalized(&String::from_utf8_lossy(&bytes)))
}

/// Applies one exact replacement the way the edit tools do. An edit that does not
/// match exactly once is reported as unknown, because a tool that matches loosely
/// could still apply it.
fn replace(before: &str, edit: &Value) -> Result<String, &'static str> {
    let old = text(edit, &["old_string", "oldString"])
        .map(normalized)
        .ok_or("the edit has no old text")?;
    let new = text(edit, &["new_string", "newString"])
        .map(normalized)
        .ok_or("the edit has no new text")?;
    let all = edit
        .get("replace_all")
        .or_else(|| edit.get("replaceAll"))
        .and_then(Value::as_bool)
        == Some(true);
    let count = if old.is_empty() {
        0
    } else {
        before.matches(old.as_str()).count()
    };
    if count == 0 {
        return Err(
            "the old text is not in the file exactly; re-read the file and retry with exact text",
        );
    }
    if count > 1 && !all {
        return Err("the old text appears more than once; include more context");
    }
    Ok(if all {
        before.replace(old.as_str(), &new)
    } else {
        before.replacen(old.as_str(), &new, 1)
    })
}

fn patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
                "--- ",
                "+++ ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|path| {
            path.trim()
                .trim_start_matches("a/")
                .trim_start_matches("b/")
                .to_string()
        })
        .filter(|path| path != "/dev/null")
        .collect()
}

fn looks_like_patch(text: &str) -> bool {
    text.starts_with("*** Begin Patch")
        || text.starts_with("diff --git ")
        || (text.contains("\n+++ ") && text.starts_with("--- "))
}

/// Every file a patch touches. A Codex patch that applies exactly is replayed; any
/// other patch leaves its files unknown, which blocks it only on locked files.
fn patch_changes(root: &Path, cwd: &Path, patch: &str) -> Vec<Change> {
    let replayed = patch::replay(patch, |raw| {
        relative(root, cwd, raw).and_then(|path| read(root, &path))
    });
    match replayed {
        Some(writes) => writes
            .into_iter()
            .filter_map(|(raw, after)| {
                let path = relative(root, cwd, &raw)?;
                Some(Change {
                    before: read(root, &path),
                    path,
                    after: after.map_or(After::Deleted, After::Content),
                })
            })
            .collect(),
        None => patch_paths(patch)
            .into_iter()
            .filter_map(|raw| relative(root, cwd, &raw))
            .map(|path| Change {
                before: read(root, &path),
                path,
                after: After::Unknown("the patch does not apply exactly"),
            })
            .collect(),
    }
}

fn proposals(
    root: &Path,
    cwd: &Path,
    tool: &str,
    input: &Value,
    payload: &Value,
) -> Result<Vec<Change>, String> {
    let patch = input
        .as_str()
        .or_else(|| {
            text(
                input,
                &["patch", "patchText", "diff", "input", "raw", "command"],
            )
        })
        .filter(|body| looks_like_patch(body));
    if let Some(patch) = patch {
        return Ok(patch_changes(root, cwd, patch));
    }
    let keys = [
        "file_path",
        "filePath",
        "path",
        "target_file",
        "targetFile",
        "notebook_path",
    ];
    let raw = text(input, &keys).or_else(|| text(payload, &keys));
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Some(path) = relative(root, cwd, raw) else {
        return Ok(Vec::new());
    };
    let before = read(root, &path);
    let after = if tool.contains("delete") || tool.contains("remove") {
        After::Deleted
    } else if tool.contains("notebook") || tool.contains("move") || tool.contains("rename") {
        After::Unknown("this tool is not replayed")
    } else if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        let mut next = before.clone().unwrap_or_default();
        let mut outcome = None;
        for edit in edits {
            match replace(&next, edit) {
                Ok(text) => next = text,
                Err(reason) => {
                    outcome = Some(After::Unknown(reason));
                    break;
                }
            }
        }
        outcome.unwrap_or(After::Content(next))
    } else if input.get("old_string").is_some() || input.get("oldString").is_some() {
        match replace(before.as_deref().unwrap_or_default(), input) {
            Ok(text) => After::Content(text),
            Err(reason) => After::Unknown(reason),
        }
    } else if let Some(content) = text(input, &["content", "contents"]) {
        After::Content(normalized(content))
    } else {
        After::Unknown("the tool input does not say what the file will contain")
    };
    Ok(vec![Change {
        path,
        before,
        after,
    }])
}

fn approve_hint(id: &str, path: &str) -> String {
    let target = if id == WHOLE_FILE { path } else { id };
    format!(
        "Nothing was written. If this change is wanted, ask the user to run `lock approve {target}` in their own terminal, then retry the same edit. Do not work around the lock."
    )
}

fn judge(root: &Path, change: &Change) -> Result<Option<String>, String> {
    let path = change.path.as_str();
    let before = change.before.as_deref();
    let after = match &change.after {
        After::Content(text) => Some(text.as_str()),
        _ => None,
    };

    if path == ".locks" || path.starts_with(".locks/") {
        if eq(path, FILE_LOCKS)
            && let Some(after) = after
        {
            let old = parse_file_locks(before.map(str::as_bytes))?;
            let new = parse_file_locks(Some(after.as_bytes()))?;
            let dropped: Vec<&String> = old.iter().filter(|item| !new.contains(item)).collect();
            if dropped.is_empty() {
                return Ok(None);
            }
            return Ok(Some(format!(
                "lock: this edit removes {} from {FILE_LOCKS}. Agents may add whole-file locks but not remove them. Ask the user to make this change.",
                dropped
                    .iter()
                    .map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if path.starts_with(APPROVALS) {
            return Ok(Some(format!(
                "lock: {path} is an approval record. Only `lock approve`, run by a person at a terminal, writes approvals."
            )));
        }
        return Ok(Some(format!(
            "lock: {path} is lock configuration. Ask the user to change it."
        )));
    }

    if HOOK_CONFIGS.iter().any(|config| eq(path, config)) {
        let had_hook = before.is_some_and(|text| text.contains("lock hook"));
        let keeps_hook = after.is_some_and(|text| text.contains("lock hook"));
        let disables = after.is_some_and(|text| text.contains("disableAllHooks"))
            && !before.is_some_and(|text| text.contains("disableAllHooks"));
        if (had_hook && !keeps_hook) || disables {
            return Ok(Some(format!(
                "lock: this edit to {path} would remove the lock hook or turn hooks off. Agents cannot do that. Ask the user."
            )));
        }
        return Ok(None);
    }

    let whole_file = parse_file_locks(fs::read(root.join(FILE_LOCKS)).ok().as_deref())?
        .iter()
        .any(|locked| eq(locked, path));
    let marked = supports_markers(path) && before.is_some_and(|text| text.contains("LOCKED:"));
    if !whole_file && !marked {
        return Ok(None);
    }
    if let After::Unknown(why) = &change.after {
        return Ok(Some(format!(
            "lock: {path} holds a lock, and this tool call cannot be checked against it ({why}). Use an exact Edit or Write instead. {}",
            approve_hint(if whole_file { WHOLE_FILE } else { "<id>" }, path)
        )));
    }

    let records = disk_approvals(root)?;
    let mut blocked = Vec::new();
    if whole_file {
        let from = before.map(|text| hash(text.as_bytes()));
        let to = after.map(|text| hash(text.as_bytes()));
        if from != to && !approved(&records, path, WHOLE_FILE, from.as_deref(), to.as_deref()) {
            pending::save(
                root,
                path,
                WHOLE_FILE,
                (from, before.unwrap_or_default()),
                (to, after.unwrap_or_default()),
            );
            blocked.push(WHOLE_FILE.to_string());
        }
    }
    if marked {
        let old = regions(path, before.unwrap_or_default()).map_err(|error| {
            format!("{error}. Fix the markers by hand before editing this file")
        })?;
        let new = match after {
            Some(text) => match regions(path, text) {
                Ok(found) => found,
                Err(error) => {
                    return Ok(Some(format!(
                        "lock: this edit would break a lock marker ({error}). Nothing was written."
                    )));
                }
            },
            None => Vec::new(),
        };
        for region in old {
            let next = new.iter().find(|candidate| candidate.id == region.id);
            let to = next.map(|candidate| candidate.hash.clone());
            if to.as_deref() == Some(region.hash.as_str())
                || approved(
                    &records,
                    path,
                    &region.id,
                    Some(&region.hash),
                    to.as_deref(),
                )
            {
                continue;
            }
            pending::save(
                root,
                path,
                &region.id,
                (Some(region.hash.clone()), &region.text),
                (
                    to,
                    next.map(|candidate| candidate.text.as_str())
                        .unwrap_or_default(),
                ),
            );
            blocked.push(region.id);
        }
    }
    if blocked.is_empty() {
        return Ok(None);
    }
    let what = if blocked == [WHOLE_FILE] {
        format!("{path} is locked as a whole file, and this tool call would change it.")
    } else {
        format!(
            "{path} has locked regions that this tool call would change or remove: {}.",
            blocked
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(Some(format!(
        "lock: {what} {}",
        approve_hint(&blocked[0], path)
    )))
}
