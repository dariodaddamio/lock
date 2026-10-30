//! The agent hook. Claude Code, Codex, and Cursor run it before and after every tool
//! call, and when the agent's turn ends. Before an edit, it replays the proposed tool call against the file in memory and denies the
//! call when a locked region would change without a person's approval, so the file on
//! disk is never touched. In Claude Code, the same edit tried again asks the person in
//! the permission prompt, see `ask`. Shell commands and other tools that can write
//! are checked by `watch`, and agent settings by `guard`.

use crate::ask::{self, Ask};
use crate::paths::{eq, relative, under};
use crate::regions::{hash, regions, supports_markers};
use crate::repo::{
    APPROVALS, FILE_LOCKS, WHOLE_FILE, approved, disk_approvals_for, find_root, parse_file_locks,
};
use crate::{guard, patch, pending, shell, watch};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Claude Code and Codex share the `PreToolUse` output contract. Only Claude Code
/// shows a hook's question to the person.
#[derive(Clone, Copy, PartialEq)]
pub enum Host {
    Claude,
    Codex,
    Cursor,
}

/// Agent configuration that keeps this hook running. An agent may edit these files
/// but may not take the hook out or switch hooks off.
pub const HOOK_CONFIGS: [&str; 4] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".cursor/hooks.json",
    ".codex/hooks.json",
];
const SHELL_TOOLS: [&str; 9] = [
    "exec",
    "bash",
    "powershell",
    "shell",
    "terminal",
    "run_terminal_cmd",
    "exec_command",
    "local_shell",
    "monitor",
];
/// Tools that only read, plan, or talk, so they cannot change a file. Any tool that is
/// not on this list and is not a known edit or shell tool, such as an MCP tool, is
/// checked like a shell command: lock copies the locked files before it runs and
/// compares them after.
const SAFE_TOOLS: [&str; 34] = [
    "read",
    "read_file",
    "grep",
    "glob",
    "ls",
    "list_dir",
    "listdir",
    "file_search",
    "filesearch",
    "codebase_search",
    "semanticsearch",
    "webfetch",
    "web_fetch",
    "websearch",
    "web_search",
    "todowrite",
    "todo_write",
    "update_plan",
    "task",
    "agent",
    "askuserquestion",
    "exitplanmode",
    "enterplanmode",
    "skill",
    "toolsearch",
    "view_image",
    "bashoutput",
    "taskoutput",
    "notebookread",
    "killshell",
    "taskstop",
    "sendmessage",
    "listagents",
    "read_lints",
];

/// What the hook answers.
pub enum Outcome {
    Allow,
    /// After a command, or at the end of a turn, with nothing to report.
    Quiet,
    Deny(String),
    /// Before an edit in Claude Code: the person decides in the permission prompt.
    Ask(String),
    /// After a command ran: lock put locked text back and tells the agent.
    Report(String),
    /// After an edit ran: lock tells the agent what it recorded.
    Note(String),
    /// At the end of a turn: lock tells the person what it put back.
    Notice(String),
}

pub fn run(host: Host) -> i32 {
    let mut input = String::new();
    let decision = std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("could not read the hook input: {error}"))
        .and_then(|_| {
            serde_json::from_str::<Value>(&input)
                .map_err(|_| "the hook input is not JSON".to_string())
        })
        .and_then(|payload| decide(host, &payload));
    match decision {
        Ok(Outcome::Allow) => {
            if let Host::Cursor = host {
                println!("{}", json!({ "permission": "allow" }));
            }
            0
        }
        Ok(Outcome::Quiet) => {
            if let Host::Cursor = host {
                println!("{{}}");
            }
            0
        }
        Ok(Outcome::Deny(reason)) => {
            println!("{}", deny(host, &reason));
            0
        }
        Ok(Outcome::Ask(question)) => {
            println!(
                "{}",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": question,
                    }
                })
            );
            0
        }
        Ok(Outcome::Report(message)) => {
            println!("{}", report(host, &message));
            0
        }
        Ok(Outcome::Note(message)) => {
            println!("{}", note(host, &message));
            0
        }
        Ok(Outcome::Notice(message)) => {
            // Cursor's stop hook can only send the agent on, so it stays quiet there.
            match host {
                Host::Claude | Host::Codex => println!("{}", json!({ "systemMessage": message })),
                Host::Cursor => println!("{{}}"),
            }
            eprintln!("{message}");
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

fn report(host: Host, message: &str) -> Value {
    match host {
        Host::Claude | Host::Codex => json!({
            "decision": "block",
            "reason": message,
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": message,
            }
        }),
        Host::Cursor => json!({ "additional_context": message }),
    }
}

fn note(host: Host, message: &str) -> Value {
    match host {
        Host::Claude | Host::Codex => json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": message,
            }
        }),
        Host::Cursor => json!({ "additional_context": message }),
    }
}

pub(crate) fn deny(host: Host, reason: &str) -> Value {
    match host {
        Host::Claude | Host::Codex => json!({
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

fn decide(host: Host, payload: &Value) -> Result<Outcome, String> {
    // Cursor sends an empty `cwd` with shell commands, so an empty one means none.
    let cwd = text(payload, &["cwd"])
        .filter(|cwd| !cwd.is_empty())
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
        return Ok(Outcome::Allow);
    };
    let event = text(payload, &["hook_event_name", "hookEventName"])
        .unwrap_or("PreToolUse")
        .to_ascii_lowercase();
    let session = text(payload, &["session_id", "conversation_id"]).unwrap_or_default();
    let call = text(payload, &["tool_use_id"]);
    let tool = text(payload, &["tool_name", "toolName"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let tool = tool.strip_prefix("functions.").unwrap_or(&tool).to_string();
    let input = payload
        .get("tool_input")
        .or_else(|| payload.get("arguments"))
        .unwrap_or(&Value::Null);
    let command = command_text(input).or_else(|| command_text(payload));
    let is_shell = SHELL_TOOLS.contains(&tool.as_str()) || (input.is_null() && command.is_some());
    let background =
        tool == "monitor" || input.get("run_in_background") == Some(&Value::Bool(true));

    match event.as_str() {
        "stop" | "sessionend" => {
            // A command that runs in the background may still change files after it
            // reported back, so lock checks it when the turn ends.
            let message = watch::settle_background(&root, session);
            watch::prune(&root, Some(session));
            ask::prune(&root, session);
            return Ok(match message {
                Ok(Some(message)) => Outcome::Notice(message),
                Ok(None) => Outcome::Quiet,
                Err(error) => Outcome::Notice(format!(
                    "lock could not check what a background command changed: {error}. Run `lock check`."
                )),
            });
        }
        // Cursor sends both events after a command. Only `postToolUse` can pass a
        // message to the agent, so it does the check, and older installs that still
        // run this hook after a command change nothing here.
        "aftershellexecution" => return Ok(Outcome::Quiet),
        "posttooluse" => {
            if background {
                return Ok(Outcome::Quiet);
            }
            // An edit ran. If lock asked about it, the person said yes.
            if let Some(call) = call.filter(|_| host == Host::Claude && !is_shell) {
                match ask::confirm(&root, call) {
                    Ok(Some(message)) => return Ok(Outcome::Note(message)),
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(Outcome::Report(format!(
                            "lock could not record the approval for this edit: {error}. Ask the user to run `lock approve`."
                        )));
                    }
                }
            }
            let key = match command.filter(|_| is_shell) {
                Some(command) => shell_key(&command, call),
                None => other_key(&tool, call, input),
            };
            // The call already ran, so an error here cannot block it; it is reported.
            return Ok(match watch::after(&root, session, &key) {
                Ok(None) => Outcome::Quiet,
                Ok(Some(message)) => Outcome::Report(message),
                Err(error) => Outcome::Report(format!(
                    "lock could not check what this command changed: {error}. Ask the user to run `lock check`."
                )),
            });
        }
        _ => {}
    }

    let changes = if is_shell {
        let Some(command) = command else {
            return Ok(Outcome::Allow);
        };
        if let Some(reason) = shell::denial(&command) {
            return Ok(Outcome::Deny(reason));
        }
        // Codex can also run `apply_patch` through the shell with the patch inline.
        let changes = match command.find("*** Begin Patch") {
            Some(start) => {
                let body = &command[start..];
                let end = body.find("*** End Patch").map_or(body.len(), |at| at + 13);
                patch_changes(&root, &cwd, &body[..end])
            }
            None => Vec::new(),
        };
        let refusals = judge_all(&root, &changes)?;
        if !refusals.is_empty() {
            return Ok(Outcome::Deny(reasons(&refusals)));
        }
        let key = shell_key(&command, call);
        watch::before(&root, session, &key, background)?;
        return Ok(Outcome::Allow);
    } else if SAFE_TOOLS.contains(&tool.as_str()) {
        return Ok(Outcome::Allow);
    } else {
        let changes = proposals(&root, &cwd, &tool, input, payload)?;
        if changes.is_empty() {
            // A tool that lock cannot read, such as an MCP tool, is checked by what it
            // changed.
            watch::before(&root, session, &other_key(&tool, call, input), false)?;
            return Ok(Outcome::Allow);
        }
        changes
    };
    let refusals = judge_all(&root, &changes)?;
    if refusals.is_empty() {
        return Ok(Outcome::Allow);
    }
    // A person may approve a locked change in Claude Code's own prompt. The first
    // attempt is refused, so the chat shows that the text is locked, and the same edit
    // tried again opens the prompt. A refusal for any other reason, such as a broken
    // marker, stays a refusal.
    if host == Host::Claude
        && let Some(call) = call
        && refusals.iter().all(|refusal| !refusal.asks.is_empty())
        && ask::possible(&root, payload)
    {
        let asks: Vec<Ask> = refusals
            .into_iter()
            .flat_map(|refusal| refusal.asks)
            .collect();
        if !ask::retried(&root, session, &asks)? {
            return Ok(Outcome::Deny(ask::first_refusal(&asks)));
        }
        ask::save(&root, session, call, &asks)?;
        return Ok(Outcome::Ask(ask::question(&asks)));
    }
    Ok(Outcome::Deny(reasons(&refusals)))
}

/// Why lock refuses a change in one file, and the locked changes in it that a person
/// may approve. A refusal that a person cannot approve has no asks.
struct Refusal {
    reason: String,
    asks: Vec<Ask>,
}

impl Refusal {
    fn hard(reason: String) -> Option<Self> {
        Some(Self {
            reason,
            asks: Vec::new(),
        })
    }
}

fn reasons(refusals: &[Refusal]) -> String {
    refusals
        .iter()
        .map(|refusal| refusal.reason.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The key that ties the check before a tool call to the check after it.
fn other_key(tool: &str, call: Option<&str>, input: &Value) -> String {
    match call {
        Some(call) => format!("tool {tool} {call}"),
        None => format!("tool {tool} {input}"),
    }
}

fn shell_key(command: &str, call: Option<&str>) -> String {
    call.map_or_else(|| command.to_string(), |id| format!("{command}\n{id}"))
}

fn judge_all(root: &Path, changes: &[Change]) -> Result<Vec<Refusal>, String> {
    let mut refusals = Vec::new();
    for change in changes {
        refusals.extend(judge(root, change)?);
    }
    Ok(refusals)
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
        .map(|bytes| normalized(&crate::regions::decode(&bytes).0))
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
    // A file outside the repository matters only if it is an agent's own settings.
    let path = match relative(root, cwd, raw) {
        Some(path) => path,
        None => {
            let full = cwd.join(raw);
            if guard::external_kind(&full).is_none() {
                return Ok(Vec::new());
            }
            full.to_string_lossy().into_owned()
        }
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

fn judge(root: &Path, change: &Change) -> Result<Option<Refusal>, String> {
    let path = change.path.as_str();
    let before = change.before.as_deref();
    let after = match &change.after {
        After::Content(text) => Some(text.as_str()),
        _ => None,
    };

    // An agent's settings can sit inside the repository too, such as in a repository
    // of dotfiles, so the full path decides.
    let settings = guard::external_kind(&root.join(path))
        .or_else(|| eq(path, ".git/config").then_some(guard::Kind::Git));
    if let Some(kind) = settings {
        let unknown = matches!(change.after, After::Unknown(_));
        if unknown || guard::weakens(kind, before, after) {
            return Ok(Refusal::hard(format!(
                "lock: this edit to {path} would turn hooks off, trust fewer hooks, or move the git hooks. Agents cannot do that. Ask the user."
            )));
        }
        return Ok(None);
    }

    if under(path, ".locks") {
        if eq(path, FILE_LOCKS)
            && let Some(after) = after
        {
            let old = parse_file_locks(before.map(str::as_bytes))?;
            let new = parse_file_locks(Some(after.as_bytes()))?;
            let dropped: Vec<&String> = old.iter().filter(|item| !new.contains(item)).collect();
            if dropped.is_empty() {
                return Ok(None);
            }
            return Ok(Refusal::hard(format!(
                "lock: this edit removes {} from {FILE_LOCKS}. Agents may add whole-file locks but not remove them. Ask the user to make this change.",
                dropped
                    .iter()
                    .map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if under(path, APPROVALS) {
            return Ok(Refusal::hard(format!(
                "lock: {path} is an approval record. Only a person writes approvals, with `lock approve` or in the permission prompt."
            )));
        }
        return Ok(Refusal::hard(format!(
            "lock: {path} is lock configuration. Ask the user to change it."
        )));
    }

    if HOOK_CONFIGS.iter().any(|config| eq(path, config)) {
        if weakens_hooks(before.map(str::as_bytes), after.map(str::as_bytes)) {
            return Ok(Refusal::hard(format!(
                "lock: this edit to {path} would remove the lock hook, turn hooks off, or add a hook that answers permission prompts. Agents cannot do that. Ask the user."
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
        return Ok(Refusal::hard(format!(
            "lock: {path} holds a lock, and this tool call cannot be checked against it ({why}). Use an exact Edit or Write instead. {}",
            approve_hint(if whole_file { WHOLE_FILE } else { "<id>" }, path)
        )));
    }

    let records = disk_approvals_for(root, path)?;
    let mut blocked: Vec<Ask> = Vec::new();
    if whole_file {
        let from = before.map(|text| hash(text.as_bytes()));
        let to = after.map(|text| hash(text.as_bytes()));
        if from != to && !approved(&records, path, WHOLE_FILE, from.as_deref(), to.as_deref()) {
            pending::save(
                root,
                path,
                WHOLE_FILE,
                (from.clone(), before.unwrap_or_default()),
                (to.clone(), after.unwrap_or_default()),
            );
            blocked.push(Ask {
                path: path.to_string(),
                id: WHOLE_FILE.to_string(),
                before: from,
                after: to,
            });
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
                    return Ok(Refusal::hard(format!(
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
                    to.clone(),
                    next.map(|candidate| candidate.text.as_str())
                        .unwrap_or_default(),
                ),
            );
            blocked.push(Ask {
                path: path.to_string(),
                id: region.id,
                before: Some(region.hash),
                after: to,
            });
        }
    }
    if blocked.is_empty() {
        return Ok(None);
    }
    let what = if blocked.len() == 1 && blocked[0].id == WHOLE_FILE {
        format!("{path} is locked as a whole file, and this tool call would change it.")
    } else {
        format!(
            "{path} has locked regions that this tool call would change or remove: {}.",
            blocked
                .iter()
                .map(|ask| format!("`{}`", ask.id))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(Some(Refusal {
        reason: format!("lock: {what} {}", approve_hint(&blocked[0].id, path)),
        asks: blocked,
    }))
}

/// The entries in an agent's hook settings that run lock, each as its JSON text, or
/// `None` when the settings are not valid JSON.
fn lock_entries(bytes: Option<&[u8]>) -> Option<Vec<String>> {
    fn walk(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    if item.is_object() && item.to_string().contains("lock hook") {
                        found.push(item.to_string());
                    }
                    walk(item, found);
                }
            }
            Value::Object(map) => map.values().for_each(|value| walk(value, found)),
            _ => {}
        }
    }
    let Some(bytes) = bytes else {
        return Some(Vec::new());
    };
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let mut found = Vec::new();
    walk(&value, &mut found);
    found.sort();
    Some(found)
}

/// Whether a change to an agent's hook settings touches the lock hook, turns hooks off,
/// or adds a hook that could answer lock's question in the permission prompt. Any
/// change to the lock hook counts, because Codex stops running a hook that changed
/// until a person reviews it again.
pub fn weakens_hooks(before: Option<&[u8]>, after: Option<&[u8]>) -> bool {
    let count = |text: Option<&[u8]>, needle: &str| {
        text.map_or(0, |bytes| {
            String::from_utf8_lossy(bytes).matches(needle).count()
        })
    };
    let entries_changed = match (lock_entries(before), lock_entries(after)) {
        (Some(old), Some(new)) => {
            let mut left = new;
            !old.iter()
                .all(|entry| match left.iter().position(|item| item == entry) {
                    Some(at) => {
                        left.remove(at);
                        true
                    }
                    None => false,
                })
        }
        // Settings that no longer parse turn every hook in them off.
        (Some(old), None) => !old.is_empty(),
        (None, _) => count(after, "lock hook") < count(before, "lock hook"),
    };
    entries_changed
        || count(after, "disableAllHooks") > count(before, "disableAllHooks")
        || count(after, "PermissionRequest") > count(before, "PermissionRequest")
}
