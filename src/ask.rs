//! Approval in the chat. Claude Code can ask the person to confirm a tool call in its
//! own permission prompt, but the prompt does not show the hook's reason, so the first
//! attempt at a locked change is refused with a message in the chat, and the same edit
//! tried again opens the prompt. The hook asks only when that prompt reaches a person,
//! keeps a ticket for the call, and records the approval after the edit ran. The edit
//! runs only after the person chose yes, so a ticket that meets its edit on disk is a
//! person's approval.

use crate::regions::{hash, regions};
use crate::repo::{WHOLE_FILE, git_dir};
use crate::{approve, pending, watch};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One locked change in a tool call, as fingerprints before and after.
pub struct Ask {
    pub path: String,
    pub id: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Permission modes in which Claude Code shows a hook's question to the person. In
/// `bypassPermissions` the prompt is skipped and in `dontAsk` it is denied.
const PROMPTING: [&str; 3] = ["default", "acceptEdits", "auto"];
const STALE_SECONDS: u64 = 6 * 60 * 60;

fn tickets(root: &Path) -> PathBuf {
    git_dir(root).join("lock").join("asked")
}

fn refused(root: &Path) -> PathBuf {
    git_dir(root).join("lock").join("refused")
}

fn ticket(root: &Path, call: &str) -> PathBuf {
    tickets(root).join(format!("{}.json", &hash(call.as_bytes())[..16]))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a question from the hook reaches a person. A `PermissionRequest` hook can
/// answer Claude Code's prompt by itself, so when one could see an edit, lock refuses
/// the edit instead of asking.
pub fn possible(root: &Path, payload: &Value) -> bool {
    let mode = payload.get("permission_mode").and_then(Value::as_str);
    mode.is_some_and(|mode| PROMPTING.contains(&mode)) && !answered_automatically(root)
}

fn managed_settings() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode\managed-settings.json")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    }
}

fn answered_automatically(root: &Path) -> bool {
    let mut files = vec![
        root.join(".claude/settings.json"),
        root.join(".claude/settings.local.json"),
        managed_settings(),
    ];
    if let Some(dir) = crate::guard::claude_dir() {
        files.push(dir.join("settings.json"));
        files.push(dir.join("settings.local.json"));
        for install in installed_plugins(&dir) {
            json_files(&install, 4, &mut files);
        }
    }
    files.iter().any(|file| {
        fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|value| answers_edits(&value))
    })
}

fn installed_plugins(dir: &Path) -> Vec<PathBuf> {
    let Some(list) = fs::read(dir.join("plugins/installed_plugins.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    list.get("plugins")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|plugins| plugins.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|install| install.get("installPath").and_then(Value::as_str))
        .map(PathBuf::from)
        .collect()
}

fn json_files(dir: &Path, depth: u8, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if depth > 0 {
                json_files(&path, depth - 1, found);
            }
        } else if path.extension().is_some_and(|ext| ext == "json") {
            found.push(path);
        }
    }
}

/// Whether a settings or plugin file has a `PermissionRequest` hook that could match an
/// edit tool. A matcher that only names other tools is safe; anything else is not.
fn answers_edits(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, entry)| {
            if key == "PermissionRequest" {
                return entry.as_array().is_none_or(|list| {
                    list.iter().any(|hook| {
                        let matcher = hook.get("matcher").and_then(Value::as_str).unwrap_or("");
                        let named = !matcher.is_empty()
                            && matcher
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|');
                        !named
                            || matcher
                                .split('|')
                                .any(|tool| ["Edit", "MultiEdit", "Write"].contains(&tool))
                    })
                });
            }
            answers_edits(entry)
        }),
        Value::Array(list) => list.iter().any(answers_edits),
        _ => false,
    }
}

fn names(asks: &[Ask]) -> String {
    asks.iter()
        .map(|ask| {
            if ask.id == WHOLE_FILE {
                format!("{} (locked as a whole file)", ask.path)
            } else {
                format!("{} :: {}", ask.path, ask.id)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the agent and the person read in the chat when an edit first meets a lock.
pub fn first_refusal(asks: &[Ask]) -> String {
    format!(
        "lock: {} is locked, so this edit was refused and nothing was written. Ask the user whether they want this exact change. If they do, retry the same edit, and Claude Code will show them the change to approve. Do not work around the lock.",
        names(asks)
    )
}

/// The reason that goes with the permission prompt.
pub fn question(asks: &[Ask]) -> String {
    format!(
        "lock: this edit changes locked text in {}. Yes approves this exact change and records it in .locks/approvals/. No leaves the locked text as it is.",
        names(asks)
    )
}

/// Whether this session already tried the same locked change and was refused. The
/// first attempt leaves a mark and the second one uses it up, so each prompt follows
/// a refusal that the chat shows. Marks outlive the turn, because the agent usually
/// asks the user and waits for the answer.
pub fn retried(root: &Path, session: &str, asks: &[Ask]) -> Result<bool, String> {
    let mut key = session.to_string();
    for ask in asks {
        key.push_str(&format!(
            "\n{}\n{}\n{}\n{}",
            ask.path,
            ask.id,
            ask.before.as_deref().unwrap_or("-"),
            ask.after.as_deref().unwrap_or("-")
        ));
    }
    let mark = refused(root).join(format!("{}.json", &hash(key.as_bytes())[..16]));
    if fs::remove_file(&mark).is_ok() {
        return Ok(true);
    }
    fs::create_dir_all(refused(root)).map_err(|error| error.to_string())?;
    fs::write(&mark, format!("{:#}\n", json!({ "created": now() })))
        .map_err(|error| error.to_string())?;
    Ok(false)
}

pub fn save(root: &Path, session: &str, call: &str, asks: &[Ask]) -> Result<(), String> {
    let record = json!({
        "session": session,
        "created": now(),
        "asks": asks.iter().map(|ask| json!({
            "path": ask.path,
            "id": ask.id,
            "before": ask.before,
            "after": ask.after,
        })).collect::<Vec<_>>(),
    });
    fs::create_dir_all(tickets(root)).map_err(|error| error.to_string())?;
    fs::write(ticket(root, call), format!("{record:#}\n")).map_err(|error| error.to_string())
}

/// A lock's state on disk, fingerprinted the way the hook fingerprints a proposal.
fn state(root: &Path, path: &str, id: &str) -> Option<String> {
    let bytes = fs::read(root.join(path)).ok()?;
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    if id == WHOLE_FILE {
        return Some(hash(text.as_bytes()));
    }
    regions(path, &text)
        .ok()?
        .into_iter()
        .find(|region| region.id == id)
        .map(|region| region.hash)
}

/// After an edit ran: records each change that the person approved in the prompt. A
/// change that is not on disk as asked, because the edit failed or differs, records
/// nothing.
pub fn confirm(root: &Path, call: &str) -> Result<Option<String>, String> {
    let file = ticket(root, call);
    let Ok(bytes) = fs::read(&file) else {
        return Ok(None);
    };
    let _ = fs::remove_file(&file);
    let record: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut recorded = Vec::new();
    for ask in record
        .get("asks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| ask.get(key).and_then(Value::as_str);
        let (Some(path), Some(id)) = (text("path"), text("id")) else {
            continue;
        };
        let after = text("after");
        if state(root, path, id).as_deref() != after {
            continue;
        }
        let saved = approve::record(
            root,
            path,
            id,
            text("before"),
            after,
            "Approved in Claude Code.",
        )?;
        watch::keep(root, &saved);
        pending::forget(root, path, id, after);
        recorded.push(saved);
    }
    if recorded.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "lock: the user approved this change in the permission prompt, and lock recorded it in {}. Commit the record together with the change.",
        recorded.join(", ")
    )))
}

/// Removes the tickets of a turn that ended, and stale tickets and marks. A ticket
/// whose prompt was answered with no never meets its edit.
pub fn prune(root: &Path, session: &str) {
    for (dir, by_session) in [(tickets(root), true), (refused(root), false)] {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let record = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let field = |key: &str| record.as_ref().and_then(|value| value.get(key).cloned());
            let mine =
                by_session && field("session").as_ref().and_then(Value::as_str) == Some(session);
            let created = field("created")
                .as_ref()
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if mine || now().saturating_sub(created) > STALE_SECONDS {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}
