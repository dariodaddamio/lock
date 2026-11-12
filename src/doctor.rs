//! `lock doctor`: checks that the guard works in this clone and says how to fix what
//! does not. It only reads, so it is safe to run anywhere. It never touches a locked
//! file: the canary replays an edit in memory, the way the agent hook does.

use crate::hook::{self, Host, Outcome};
use crate::install::{AGENT_FILES, git_hook_line, git_hooks_dir, mentions_lock};
use crate::repo::{View, WHOLE_FILE, git_text, inventory};
use crate::{pending, repo};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub status: Status,
    pub name: String,
    /// What to do about it. Every line that is not a pass has one.
    pub fix: Option<String>,
}

impl Check {
    fn pass(name: impl Into<String>) -> Self {
        Self {
            status: Status::Pass,
            name: name.into(),
            fix: None,
        }
    }

    fn warn(name: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            status: Status::Warn,
            name: name.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(name: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            status: Status::Fail,
            name: name.into(),
            fix: Some(fix.into()),
        }
    }
}

/// The checklist as text, with a summary line at the end.
pub(crate) fn render(checks: &[Check]) -> String {
    let mut text = String::from("lock doctor\n\n");
    let mut count = [0usize; 3];
    for check in checks {
        let (label, slot) = match check.status {
            Status::Pass => ("PASS", 0),
            Status::Warn => ("WARN", 1),
            Status::Fail => ("FAIL", 2),
        };
        count[slot] += 1;
        text.push_str(&format!("{label}  {}\n", check.name));
        if let Some(fix) = &check.fix {
            text.push_str(&format!("      Fix: {fix}\n"));
        }
    }
    text.push_str(&format!(
        "\n{} passed, {} warning{}, {} failed.\n",
        count[0],
        count[1],
        if count[1] == 1 { "" } else { "s" },
        count[2]
    ));
    text
}

/// A failure makes the exit code non-zero. A warning does not.
pub(crate) fn exit_code(checks: &[Check]) -> i32 {
    i32::from(checks.iter().any(|check| check.status == Status::Fail))
}

/// The first `lock` that a shell with this PATH would run.
pub(crate) fn find_in_path(path: &OsStr) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["lock.exe", "lock.cmd", "lock.bat"]
    } else {
        &["lock"]
    };
    std::env::split_paths(path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn lock_on_path() -> Option<PathBuf> {
    find_in_path(&std::env::var_os("PATH")?)
}

/// The files, from those that exist, that git ignores. A file that git already tracks
/// is shared whatever the ignore rules say, and `git check-ignore` reports it that way.
pub(crate) fn git_ignored(root: &Path, files: &[&str]) -> Vec<String> {
    files
        .iter()
        .filter(|file| root.join(file).exists())
        .filter(|file| {
            Command::new("git")
                .args(["check-ignore", "-q", "--"])
                .arg(file)
                .current_dir(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .map(|file| (*file).to_string())
        .collect()
}

fn binary_check() -> Check {
    match lock_on_path() {
        Some(path) => Check::pass(format!("lock is on your PATH ({})", path.display())),
        None => Check::fail(
            "lock is not on your PATH",
            "Add the folder that holds lock to your PATH and restart your agent. Until then the hooks block every edit.",
        ),
    }
}

fn commit_hook_check(root: &Path) -> Check {
    let missing = || {
        Check::fail(
            "The pre-commit hook does not run the lock check",
            "Run `lock install --git`.",
        )
    };
    let Ok(dir) = git_hooks_dir(root) else {
        return missing();
    };
    let Ok(text) = fs::read_to_string(dir.join("pre-commit")) else {
        return missing();
    };
    if text.lines().any(|line| line == git_hook_line()) {
        Check::pass("The pre-commit hook runs the lock check")
    } else if text.lines().any(|line| line.ends_with("# lock")) {
        Check::warn(
            "The pre-commit hook is from an older lock",
            "Run `lock install --git` to update it.",
        )
    } else {
        missing()
    }
}

/// The events that each agent's hook file must run lock for.
fn required_events(file: &str) -> &'static [&'static str] {
    if file.starts_with(".cursor") {
        &[
            "preToolUse",
            "beforeShellExecution",
            "postToolUse",
            "postToolUseFailure",
            "stop",
        ]
    } else if file.starts_with(".claude") {
        &["PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop"]
    } else {
        &["PreToolUse", "PostToolUse", "Stop"]
    }
}

/// One check for each agent whose file exists, and a failure when no agent has one.
fn agent_checks(root: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut absent = Vec::new();
    for (agent, file) in AGENT_FILES {
        let Ok(bytes) = fs::read(root.join(file)) else {
            absent.push((agent, file));
            continue;
        };
        let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
            checks.push(Check::fail(
                format!("{file} is not valid JSON"),
                format!("Fix the file by hand, then run `lock install` to add the {agent} hook."),
            ));
            continue;
        };
        let missing: Vec<&str> = required_events(file)
            .iter()
            .copied()
            .filter(|event| {
                !settings
                    .pointer(&format!("/hooks/{event}"))
                    .is_some_and(mentions_lock)
            })
            .collect();
        checks.push(if missing.is_empty() {
            Check::pass(format!("{file} runs the lock hook for {agent}"))
        } else {
            Check::fail(
                format!("{file} lacks the lock hook for: {}", missing.join(", ")),
                "Run `lock install` to add it.",
            )
        });
    }
    if absent.len() == AGENT_FILES.len() {
        checks.push(Check::fail(
            "No agent has the lock hook: .claude/settings.json, .codex/hooks.json, and .cursor/hooks.json are all missing",
            "Run `lock install` in the root of the repository.",
        ));
    } else {
        for (agent, file) in absent {
            let name = format!("{file} is missing, so {agent} is not guarded");
            let install = format!("Run `lock install --{}`.", flag(agent));
            // Only an agent that has left no trace on this machine is excused.
            checks.push(if agent_in_use(agent) {
                Check::fail(name, install)
            } else {
                Check::warn(
                    name,
                    format!("Fine if you do not use {agent}. Otherwise: {install}"),
                )
            });
        }
    }
    checks
}

/// Whether the agent's folder exists in the home directory or its command is on the PATH.
fn agent_in_use(agent: &str) -> bool {
    let (folder, command) = match agent {
        "Claude Code" => (".claude", "claude"),
        "Codex" => (".codex", "codex"),
        _ => (".cursor", "cursor"),
    };
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    home.is_some_and(|home| Path::new(&home).join(folder).exists())
        || std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| {
                ["", ".exe", ".cmd"]
                    .iter()
                    .any(|ext| dir.join(format!("{command}{ext}")).is_file())
            })
        })
}

fn flag(agent: &str) -> &'static str {
    match agent {
        "Claude Code" => "claude",
        "Codex" => "codex",
        _ => "cursor",
    }
}

fn ignored_check(root: &Path) -> Check {
    let files: Vec<&str> = AGENT_FILES.iter().map(|(_, file)| *file).collect();
    let ignored = git_ignored(root, &files);
    if ignored.is_empty() {
        return Check::pass("The hook files are not git-ignored, so teammates get them");
    }
    let lines: Vec<String> = ignored
        .iter()
        .flat_map(|file| {
            [
                format!("!{}/", file.split('/').next().unwrap_or(file)),
                format!("!{file}"),
            ]
        })
        .collect();
    Check::warn(
        format!(
            "Git ignores {}, so teammates and CI do not get the hooks",
            ignored.join(", ")
        ),
        format!(
            "Add these lines to .gitignore, then `git add` the files: {}",
            lines.join("  ")
        ),
    )
}

fn locks_dir_check(root: &Path) -> Check {
    if root.join(".locks").is_dir() {
        Check::pass(".locks is present")
    } else {
        Check::warn(
            ".locks is missing",
            "Only whole-file locks and approvals need it. Ask your agent to lock something, or create .locks/files.json.",
        )
    }
}

/// The locks in the working tree, or the reason they cannot be read, as a check.
fn locks_check(root: &Path) -> (Check, Option<repo::Locks>) {
    let locks = match inventory(root, &View::Worktree) {
        Ok(locks) => locks,
        Err(error) => {
            return (
                Check::fail(
                    format!("The locks cannot be read: {error}"),
                    "Fix the markers or .locks/files.json by hand. Until then lock refuses edits to the affected files.",
                ),
                None,
            );
        }
    };
    let missing: Vec<&str> = locks
        .values()
        .filter(|entry| entry.id == WHOLE_FILE && entry.hash.is_none())
        .map(|entry| entry.path.as_str())
        .collect();
    let files: BTreeSet<&str> = locks.keys().map(|(path, _)| path.as_str()).collect();
    let check = if missing.is_empty() {
        Check::pass(format!(
            "{} lock{} in {} file{}, and the markers are well formed",
            locks.len(),
            if locks.len() == 1 { "" } else { "s" },
            files.len(),
            if files.len() == 1 { "" } else { "s" },
        ))
    } else {
        Check::warn(
            format!("Locked files do not exist: {}", missing.join(", ")),
            "Restore them with git, or run `lock approve` if you removed them on purpose.",
        )
    };
    (check, Some(locks))
}

/// The tool call that an agent would send to edit `text` inside `path`.
pub(crate) fn canary_payload(root: &Path, path: &str, text: &str, file_text: &str) -> Value {
    let file_text = file_text.replace("\r\n", "\n");
    let text = text.replace("\r\n", "\n");
    // The edit must match exactly once, as the agent's own edit tools require, so the
    // hook replays it instead of refusing it as unreadable.
    let old = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.contains("LOCKED:"))
        .find(|line| file_text.matches(*line).count() == 1)
        .unwrap_or(&text)
        .to_string();
    json!({
        "cwd": root.to_string_lossy(),
        "session_id": "lock-doctor",
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": root.join(path).to_string_lossy(),
            "old_string": old,
            "new_string": format!("{old} lock-doctor-canary"),
        },
    })
}

/// Replays an edit to locked text through the hook logic and reports whether the hook
/// would deny it. Nothing is written: the hook works on the text in memory, and no
/// proposal is saved for `lock approve`.
fn canary_check(root: &Path, locks: &repo::Locks) -> Check {
    let pick = locks.values().find(|entry| {
        !entry.text.trim().is_empty()
            && !entry.text.contains(['\0', '\u{fffd}'])
            && entry.hash.is_some()
    });
    let Some(entry) = pick else {
        return Check::warn(
            "No locked text to test the hook with",
            "Ask your agent to lock some text, then run `lock doctor` again.",
        );
    };
    let file_text = fs::read(root.join(&entry.path))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let payload = canary_payload(root, &entry.path, &entry.text, &file_text);
    pending::dry_run(true);
    let outcome = hook::decide(Host::Claude, &payload);
    pending::dry_run(false);
    let target = if entry.id == WHOLE_FILE {
        entry.path.clone()
    } else {
        format!("{} :: {}", entry.path, entry.id)
    };
    match outcome {
        Ok(Outcome::Deny(reason)) if !reason.contains("cannot be checked") => Check::pass(format!(
            "Canary: an agent's edit to {target} would be denied"
        )),
        Ok(Outcome::Deny(reason)) => Check::warn(
            format!(
                "Canary: the hook denied an edit to {target}, but could not check it: {reason}"
            ),
            "Run `lock check` and look at the locked file.",
        ),
        Ok(_) => Check::fail(
            format!("Canary: an agent's edit to {target} would be allowed"),
            "The hook does not guard this file. Run `lock install` and restart your agent.",
        ),
        Err(error) => Check::fail(
            format!("Canary: the hook could not check an edit to {target}: {error}"),
            "Run `lock check` and fix what it reports.",
        ),
    }
}

/// Every check, in the order that a person fixes them.
pub(crate) fn checks(start: &Path) -> Vec<Check> {
    let mut list = vec![binary_check()];
    let Ok(root) = git_text(start, &["rev-parse", "--show-toplevel"]) else {
        list.push(Check::fail(
            "No git repository found here",
            "Run lock doctor inside the repository that you want to check.",
        ));
        return list;
    };
    let root = PathBuf::from(root);
    list.push(Check::pass(format!("Git repository at {}", root.display())));
    list.push(commit_hook_check(&root));
    list.extend(agent_checks(&root));
    list.push(ignored_check(&root));
    list.push(locks_dir_check(&root));
    let (locks, found) = locks_check(&root);
    list.push(locks);
    list.push(unsupported_markers_check(&root));
    list.push(match found {
        Some(found) => canary_check(&root, &found),
        None => Check::warn(
            "Canary skipped because the locks cannot be read",
            "Fix the line above first.",
        ),
    });
    list
}

fn unsupported_markers_check(root: &Path) -> Check {
    match repo::unsupported_markers(root) {
        Ok(paths) if paths.is_empty() => {
            Check::pass("No LOCKED: markers in unsupported file types")
        }
        Ok(paths) => Check::warn(
            format!(
                "LOCKED: in unsupported file type{} (locks nothing): {}",
                if paths.len() == 1 { "" } else { "s" },
                paths.join(", ")
            ),
            "Use a supported comment syntax, or lock the whole file in .locks/files.json.",
        ),
        Err(error) => Check::warn(
            format!("Could not scan for unsupported LOCKED: markers: {error}"),
            "Fix the repository state, then run lock doctor again.",
        ),
    }
}

pub fn run() -> Result<i32, String> {
    let start = std::env::current_dir().map_err(|error| error.to_string())?;
    let list = checks(&start);
    print!("{}", render(&list));
    Ok(exit_code(&list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Check> {
        vec![
            Check::pass("a"),
            Check::warn("b", "do b"),
            Check::fail("c", "do c"),
        ]
    }

    #[test]
    fn the_checklist_shows_a_status_and_a_fix_per_line() {
        let text = render(&sample());
        assert!(text.contains("PASS  a\n"), "{text}");
        assert!(text.contains("WARN  b\n      Fix: do b\n"), "{text}");
        assert!(text.contains("FAIL  c\n      Fix: do c\n"), "{text}");
        assert!(text.contains("1 passed, 1 warning, 1 failed."), "{text}");
    }

    #[test]
    fn only_a_failure_makes_the_exit_code_non_zero() {
        assert_eq!(exit_code(&sample()), 1);
        assert_eq!(exit_code(&sample()[..2]), 0);
        assert_eq!(exit_code(&[]), 0);
    }

    #[test]
    fn the_canary_edits_one_unique_line_of_the_locked_text() {
        let file = "a\n<!-- LOCKED: x START -->\nsame\nsame\nunique\n<!-- LOCKED: x END -->\n";
        let region = "<!-- LOCKED: x START -->\nsame\nsame\nunique\n<!-- LOCKED: x END -->";
        let payload = canary_payload(Path::new("/r"), "f.md", region, file);
        let input = &payload["tool_input"];
        assert_eq!(input["old_string"], "unique");
        assert_eq!(input["new_string"], "unique lock-doctor-canary");
        assert_eq!(payload["hook_event_name"], "PreToolUse");
    }

    #[test]
    fn the_canary_falls_back_to_the_whole_text() {
        let payload = canary_payload(Path::new("/r"), "f.md", "x\nx", "x\nx");
        assert_eq!(payload["tool_input"]["old_string"], "x\nx");
    }

    #[test]
    fn lock_is_found_on_a_path() {
        let dir = std::env::temp_dir().join(format!("lock-path-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "lock.exe" } else { "lock" };
        let empty = std::env::join_paths([&dir]).unwrap();
        assert_eq!(find_in_path(&empty), None);
        fs::write(dir.join(name), "").unwrap();
        assert_eq!(find_in_path(&empty), Some(dir.join(name)));
        let _ = fs::remove_dir_all(&dir);
    }
}
