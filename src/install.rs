//! `lock install`: registers the agent hook with Claude Code, Codex, and Cursor, before
//! and after each tool call and at the end of each turn. It adds the staged check to
//! the git pre-commit hook. Running it again changes nothing.

use crate::repo::git_text;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The hook files that `lock install` writes for each agent, as the agent's name and
/// the path from the repository root.
pub(crate) const AGENT_FILES: [(&str, &str); 3] = [
    ("Claude Code", ".claude/settings.json"),
    ("Codex", ".codex/hooks.json"),
    ("Cursor", ".cursor/hooks.json"),
];

const HOME: &str = "https://github.com/dariodaddamio/lock";

/// Exit code 2 is what makes Claude Code block the call, so a missing binary blocks
/// edits instead of letting them through unchecked. At the end of a turn exit code 2
/// would keep the agent going, so that hook never uses it. Codex runs hooks in Windows
/// PowerShell on Windows and in a POSIX shell elsewhere, so its command is the one
/// form that both shells run.
fn hook_command(host: &str, event: &str) -> String {
    if host == "codex" {
        return format!("lock hook {host}");
    }
    if event == "Stop" {
        return format!("command -v lock >/dev/null 2>&1 && lock hook {host}; exit 0");
    }
    format!(
        "command -v lock >/dev/null 2>&1 || {{ echo 'lock is not installed, so edits are blocked. Install it: {HOME}' >&2; exit 2; }}; lock hook {host}"
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

pub(crate) fn mentions_lock(entries: &Value) -> bool {
    entries.to_string().contains("lock hook")
}

/// Claude Code and Codex read the same hook shape. Each entry is an event and a
/// matcher, which is `None` for events without tools.
fn agent_hooks(
    root: &Path,
    file: &str,
    entries: &[(&str, Option<&str>)],
    host: &str,
) -> Result<String, String> {
    let path = root.join(file);
    let mut settings = read_json(&path)?;
    let hooks = object(object(&mut settings)?.entry("hooks").or_insert(json!({})))?;
    let mut changed = false;
    for (event, matcher) in entries {
        let list = hooks
            .entry(*event)
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("{event} must be a list"))?;
        let mut entry =
            json!({ "hooks": [{ "type": "command", "command": hook_command(host, event) }] });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        // An entry from an older lock is brought up to date in place.
        match list.iter_mut().find(|existing| mentions_lock(existing)) {
            Some(existing) if *existing == entry => {}
            Some(existing) => {
                *existing = entry;
                changed = true;
            }
            None => {
                list.push(entry);
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(format!("{file} already runs the lock hook."));
    }
    write_json(&path, &settings)?;
    Ok(format!("Added the lock hook to {file}."))
}

/// Every tool goes through the hook: edits are checked before they run, and any other
/// tool that can write, such as a shell or an MCP tool, is checked by what it changed.
fn agent(root: &Path, host: &str) -> Result<String, String> {
    let (host, file) = [
        ("claude", ".claude/settings.json"),
        ("codex", ".codex/hooks.json"),
    ]
    .into_iter()
    .find(|(name, _)| *name == host)
    .ok_or_else(|| format!("unknown agent: {host}"))?;
    // Claude Code reports a failed tool call as PostToolUseFailure instead of
    // PostToolUse, so lock checks those too. Codex has no such event, and its hook file
    // stays unchanged, because Codex stops running hooks that changed until a person
    // trusts them again. Its failed commands are checked at Stop.
    let events: &[(&str, Option<&str>)] = if host == "claude" {
        &[
            ("PreToolUse", None),
            ("PostToolUse", None),
            ("PostToolUseFailure", None),
            ("Stop", None),
        ]
    } else {
        &[("PreToolUse", None), ("PostToolUse", None), ("Stop", None)]
    };
    agent_hooks(root, file, events, host)
}

fn cursor(root: &Path) -> Result<String, String> {
    let path = root.join(".cursor/hooks.json");
    let mut config = read_json(&path)?;
    let top = object(&mut config)?;
    top.entry("version").or_insert(json!(1));
    let hooks = object(top.entry("hooks").or_insert(json!({})))?;
    let mut added = false;
    // The checks before a call fail closed. The ones after a command and at the end of
    // a turn cannot stop anything, so they only repair and report.
    for (event, fail_closed) in [
        ("preToolUse", true),
        ("beforeShellExecution", true),
        ("postToolUse", false),
        ("postToolUseFailure", false),
        ("stop", false),
    ] {
        let list = hooks
            .entry(event)
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("{event} must be a list"))?;
        let mut entry = json!({ "command": "lock hook cursor" });
        if fail_closed {
            entry["failClosed"] = json!(true);
        }
        // An entry from an older lock, such as one limited to some tools, is replaced.
        match list.iter_mut().find(|existing| mentions_lock(existing)) {
            Some(existing) if *existing == entry => {}
            Some(existing) => {
                *existing = entry;
                added = true;
            }
            None => {
                list.push(entry);
                added = true;
            }
        }
    }
    // Older installs also ran the hook on `afterShellExecution`, which `postToolUse`
    // replaces because only it can pass a message to the agent.
    if let Some(list) = hooks
        .get_mut("afterShellExecution")
        .and_then(Value::as_array_mut)
        && list.iter().any(mentions_lock)
    {
        list.retain(|entry| !mentions_lock(entry));
        if list.is_empty() {
            hooks.remove("afterShellExecution");
        }
        added = true;
    }
    if !added {
        return Ok(".cursor/hooks.json already runs the lock hook.".into());
    }
    write_json(&path, &config)?;
    Ok("Added the lock hook to .cursor/hooks.json.".into())
}

/// A path as the person reads it: from the repository root, with forward slashes.
fn shown(root: &Path, path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let base = root.to_string_lossy().replace('\\', "/");
    text.strip_prefix(&format!("{}/", base.trim_end_matches('/')))
        .map_or(text.clone(), str::to_string)
}

/// The directory that holds this clone's git hooks.
pub(crate) fn git_hooks_dir(root: &Path) -> Result<PathBuf, String> {
    Ok(match git_text(root, &["config", "core.hooksPath"]) {
        Ok(custom) if !custom.is_empty() => root.join(custom),
        _ => root.join(git_text(root, &["rev-parse", "--git-path", "hooks"])?),
    })
}

/// The line that `lock install` adds to the pre-commit hook.
pub(crate) fn git_hook_line() -> String {
    format!(
        "{{ command -v lock >/dev/null 2>&1 || {{ echo 'lock is not installed: {HOME}' >&2; exit 1; }}; lock check --staged || exit 1; }} # lock"
    )
}

pub(crate) fn ensure_git_hook(root: &Path) -> Result<String, String> {
    let dir = git_hooks_dir(root)?;
    let hook = dir.join("pre-commit");
    let current = fs::read_to_string(&hook).unwrap_or_else(|_| "#!/bin/sh\n".into());
    let line = git_hook_line();
    if current.lines().any(|existing| existing == line) {
        return Ok("The pre-commit hook already runs the lock check.".into());
    }
    // An older lock line is replaced, so the hook always runs the current check.
    let current = current
        .lines()
        .filter(|existing| !existing.ends_with("# lock"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    fs::write(&hook, format!("{}\n{line}\n", current.trim_end()))
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hook, fs::Permissions::from_mode(0o755));
    }
    Ok(format!("Added the lock check to {}.", shown(root, &hook)))
}

/// Takes the lock entries out of one agent's hook settings. A file that held nothing
/// else is removed.
fn remove_agent_hooks(root: &Path, file: &str) -> Result<Option<String>, String> {
    let path = root.join(file);
    if !path.exists() {
        return Ok(None);
    }
    let mut settings = read_json(&path)?;
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    let before = hooks.clone();
    for list in hooks.values_mut() {
        if let Some(entries) = list.as_array_mut() {
            entries.retain(|entry| !mentions_lock(entry));
        }
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|entries| !entries.is_empty()));
    if *hooks == before {
        return Ok(None);
    }
    let empty = hooks.is_empty();
    let top = object(&mut settings)?;
    if empty {
        top.remove("hooks");
    }
    let only_version = top.keys().all(|key| key == "version");
    if only_version {
        fs::remove_file(&path).map_err(|error| error.to_string())?;
        // The folder was made for this file; a folder with other files stays.
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir);
        }
        return Ok(Some(format!(
            "Removed {file}, which held only the lock hook."
        )));
    }
    write_json(&path, &settings)?;
    Ok(Some(format!("Removed the lock hook from {file}.")))
}

fn remove_git_hook(root: &Path) -> Result<Option<String>, String> {
    let dir = match git_text(root, &["config", "core.hooksPath"]) {
        Ok(custom) if !custom.is_empty() => root.join(custom),
        _ => root.join(git_text(root, &["rev-parse", "--git-path", "hooks"])?),
    };
    let hook = dir.join("pre-commit");
    let Ok(current) = fs::read_to_string(&hook) else {
        return Ok(None);
    };
    let kept: Vec<&str> = current
        .lines()
        .filter(|line| !line.ends_with("# lock"))
        .collect();
    if kept.len() == current.lines().count() {
        return Ok(None);
    }
    if kept
        .iter()
        .all(|line| line.trim().is_empty() || line.starts_with("#!"))
    {
        fs::remove_file(&hook).map_err(|error| error.to_string())?;
        return Ok(Some(format!("Removed {}.", shown(root, &hook))));
    }
    fs::write(&hook, format!("{}\n", kept.join("\n"))).map_err(|error| error.to_string())?;
    Ok(Some(format!(
        "Removed the lock check from {}.",
        shown(root, &hook)
    )))
}

/// `lock uninstall`: takes every hook that `lock install` added out of this clone. Lock
/// markers and `.locks/` stay, so `lock install` brings the protection back. Only a
/// person at a terminal can run it, like `lock approve`.
pub fn uninstall() -> Result<i32, String> {
    crate::approve::require_person("lock uninstall")?;
    let root = git_text(
        &std::env::current_dir().map_err(|e| e.to_string())?,
        &["rev-parse", "--show-toplevel"],
    )
    .map_err(|_| "run lock uninstall inside a git repository".to_string())?;
    let root = Path::new(&root);
    let mut done = Vec::new();
    for file in [
        ".claude/settings.json",
        ".codex/hooks.json",
        ".cursor/hooks.json",
    ] {
        done.extend(remove_agent_hooks(root, file)?);
    }
    done.extend(remove_git_hook(root)?);
    if done.is_empty() {
        println!("lock: this clone has no lock hooks.");
    }
    for line in done {
        println!("lock: {line}");
    }
    println!(
        "lock: the lock markers and .locks/ stay in the repository. Run `lock install` to protect them again."
    );
    Ok(0)
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
        steps.push(ensure_git_hook);
    }
    if wants("--claude") {
        steps.push(|root| agent(root, "claude"));
    }
    if wants("--codex") {
        steps.push(|root| agent(root, "codex"));
    }
    if wants("--cursor") {
        steps.push(cursor);
    }
    for step in steps {
        println!("lock: {}", step(root)?);
    }
    let chosen: Vec<(&str, &str)> = AGENT_FILES
        .iter()
        .zip(["--claude", "--codex", "--cursor"])
        .filter(|(_, flag)| wants(flag))
        .map(|(agent, _)| *agent)
        .collect();
    let files: Vec<&str> = chosen.iter().map(|(_, file)| *file).collect();
    let ignored = crate::doctor::git_ignored(root, &files);
    if !ignored.is_empty() {
        println!("{}", ignored_warning(&ignored));
    }
    let agents: Vec<&str> = chosen.iter().map(|(name, _)| *name).collect();
    println!(
        "{}",
        next_steps(&agents, crate::doctor::lock_on_path().as_deref())
    );
    Ok(0)
}

/// Says that teammates will not get hook files that git ignores, and how to share them.
pub(crate) fn ignored_warning(files: &[String]) -> String {
    let mut text = String::new();
    for file in files {
        text.push_str(&format!(
            "lock: warning: {file} is git-ignored, so teammates and CI will not get it.\n"
        ));
    }
    text.push_str(
        "lock: to share the hooks, add these lines to .gitignore, then `git add` the files:\n",
    );
    for file in files {
        let folder = file.split('/').next().unwrap_or(file);
        text.push_str(&format!("lock:   !{folder}/\nlock:   !{file}\n"));
    }
    text.push_str("lock: `git check-ignore -v <file>` shows which rule ignores a file.");
    text
}

/// What the person does next, in the order that matters. A session that started before
/// `lock install` never ran the hooks, so only the commit check guards it.
pub(crate) fn next_steps(agents: &[&str], on_path: Option<&Path>) -> String {
    let mut steps: Vec<String> = Vec::new();
    if !agents.is_empty() {
        steps.push(
            "Restart any agent that was already running in this repository. A session that\n     started before lock install runs no hooks, so its edits are NOT guarded until you\n     restart it. Only the commit check stops them."
                .into(),
        );
        let mut trust = String::from("Trust the hooks once:");
        for agent in agents {
            trust.push_str(match *agent {
                "Claude Code" => "\n       Claude Code: accept the folder trust prompt when it starts.",
                "Codex" => {
                    "\n       Codex: open /hooks and review the new hooks. Codex runs none until you do."
                }
                _ => "\n       Cursor: reopen the project so it reads .cursor/hooks.json.",
            });
        }
        steps.push(trust);
    }
    steps.push(match on_path {
        Some(path) => format!(
            "lock is on your PATH at {}. An agent finds lock on the PATH it had when it\n     started, so start it from a shell where `lock --version` works.",
            path.display()
        ),
        None => "WARNING: lock is not on your PATH, so an agent cannot run it and the hooks block\n     every edit. Add the folder that holds lock to your PATH, then restart the agent."
            .into(),
    });
    steps.push("Check that everything works: lock doctor".into());
    let mut text = String::from("\nlock: Next steps");
    for (number, step) in steps.iter().enumerate() {
        text.push_str(&format!("\n  {}. {step}", number + 1));
    }
    text
}
