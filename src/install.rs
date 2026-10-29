//! `lock install`: registers the agent hook with Claude Code and Cursor and adds the
//! staged check to the git pre-commit hook. Running it again changes nothing.

use crate::repo::git_text;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

const HOME: &str = "https://github.com/dariodaddamio/lock";

/// Exit code 2 is what makes Claude Code block the call, so a missing binary blocks
/// edits instead of letting them through unchecked.
fn claude_command() -> String {
    format!(
        "command -v lock >/dev/null 2>&1 || {{ echo 'lock is not installed, so edits are blocked. Install it: {HOME}' >&2; exit 2; }}; lock hook claude"
    )
}

fn read_json(path: &Path) -> Result<Value, String> {
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(_) => Ok(json!({})),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    fs::write(path, format!("{value:#}\n")).map_err(|error| error.to_string())
}

fn object(value: &mut Value) -> Result<&mut Map<String, Value>, String> {
    if value.is_null() {
        *value = json!({});
    }
    value
        .as_object_mut()
        .ok_or_else(|| "expected a JSON object".to_string())
}

fn mentions_lock(entries: &Value) -> bool {
    entries.to_string().contains("lock hook")
}

fn claude(root: &Path) -> Result<String, String> {
    let path = root.join(".claude/settings.json");
    let mut settings = read_json(&path)?;
    let hooks = object(object(&mut settings)?.entry("hooks").or_insert(json!({})))?;
    let pre = hooks.entry("PreToolUse").or_insert(json!([]));
    if mentions_lock(pre) {
        return Ok(".claude/settings.json already runs the lock hook.".into());
    }
    pre.as_array_mut()
        .ok_or("PreToolUse must be a list")?
        .push(json!({
            "matcher": "Edit|MultiEdit|Write|NotebookEdit|Bash|PowerShell",
            "hooks": [{ "type": "command", "command": claude_command() }],
        }));
    write_json(&path, &settings)?;
    Ok("Added the lock hook to .claude/settings.json.".into())
}

fn cursor(root: &Path) -> Result<String, String> {
    let path = root.join(".cursor/hooks.json");
    let mut config = read_json(&path)?;
    let top = object(&mut config)?;
    top.entry("version").or_insert(json!(1));
    let hooks = object(top.entry("hooks").or_insert(json!({})))?;
    let mut added = false;
    for (event, matcher) in [
        (
            "preToolUse",
            Some("Write|StrReplace|Edit|MultiEdit|Delete|ApplyPatch"),
        ),
        ("beforeShellExecution", None),
    ] {
        let list = hooks.entry(event).or_insert(json!([]));
        if mentions_lock(list) {
            continue;
        }
        let mut entry = json!({ "command": "lock hook cursor", "failClosed": true });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        list.as_array_mut()
            .ok_or_else(|| format!("{event} must be a list"))?
            .push(entry);
        added = true;
    }
    if !added {
        return Ok(".cursor/hooks.json already runs the lock hook.".into());
    }
    write_json(&path, &config)?;
    Ok("Added the lock hook to .cursor/hooks.json.".into())
}

fn git_hook(root: &Path) -> Result<String, String> {
    let dir = match git_text(root, &["config", "core.hooksPath"]) {
        Ok(custom) if !custom.is_empty() => root.join(custom),
        _ => root.join(git_text(root, &["rev-parse", "--git-path", "hooks"])?),
    };
    let hook = dir.join("pre-commit");
    let current = fs::read_to_string(&hook).unwrap_or_else(|_| "#!/bin/sh\n".into());
    if current.contains("# lock") {
        return Ok("The pre-commit hook already runs the lock check.".into());
    }
    let line = format!(
        "{{ command -v lock >/dev/null 2>&1 || {{ echo 'lock is not installed: {HOME}' >&2; exit 1; }}; lock check --staged || exit 1; }} # lock"
    );
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    fs::write(&hook, format!("{}\n{line}\n", current.trim_end()))
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hook, fs::Permissions::from_mode(0o755));
    }
    Ok(format!("Added the lock check to {}.", hook.display()))
}

pub fn run(flags: &[String]) -> Result<i32, String> {
    let Ok(root) = git_text(
        &std::env::current_dir().map_err(|e| e.to_string())?,
        &["rev-parse", "--show-toplevel"],
    ) else {
        // Package installs also run outside a clone, for example in a deploy build.
        println!("lock: not inside a git repository, so there is nothing to install.");
        return Ok(0);
    };
    let root = Path::new(&root);
    let all = flags.is_empty();
    let wants = |flag: &str| all || flags.iter().any(|f| f == flag);
    type Step = fn(&Path) -> Result<String, String>;
    let mut steps: Vec<Step> = Vec::new();
    if wants("--git") {
        steps.push(git_hook);
    }
    if wants("--claude") {
        steps.push(claude);
    }
    if wants("--cursor") {
        steps.push(cursor);
    }
    for step in steps {
        println!("lock: {}", step(root)?);
    }
    Ok(0)
}
