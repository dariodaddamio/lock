//! Settings outside the locked files that decide whether lock runs at all: each
//! agent's own settings in the home directory, and git's `core.hooksPath`, which can
//! point the commit hook somewhere else. An agent may edit these files, but it may not
//! use them to turn hooks off.

use crate::repo::git_text;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What kind of settings a guarded file holds.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Claude Code settings JSON.
    Claude,
    /// Codex `config.toml`.
    Codex,
    /// A git config file.
    Git,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Claude Code's user configuration directory.
pub fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".claude")))
}

fn codex_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".codex")))
}

/// The guarded files outside the repository, with their kind.
pub fn external() -> Vec<(PathBuf, Kind)> {
    let mut files = Vec::new();
    if let Some(dir) = claude_dir() {
        files.push((dir.join("settings.json"), Kind::Claude));
        files.push((dir.join("settings.local.json"), Kind::Claude));
    }
    if let Some(dir) = codex_dir() {
        files.push((dir.join("config.toml"), Kind::Codex));
    }
    if let Some(home) = home() {
        files.push((home.join(".gitconfig"), Kind::Git));
        files.push((home.join(".config/git/config"), Kind::Git));
    }
    files
}

/// A path in one comparable form. Resolving it turns a Windows short name, such as
/// `RUNNER~1`, into the long one; a file that does not exist yet is resolved through
/// its folder.
fn normal(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).ok().or_else(|| {
        let dir = std::fs::canonicalize(path.parent()?).ok()?;
        Some(dir.join(path.file_name()?))
    });
    let path = resolved.as_deref().unwrap_or(path);
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text).to_string();
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}

/// The kind of a guarded file outside the repository, if `path` is one.
pub fn external_kind(path: &Path) -> Option<Kind> {
    let wanted = normal(path);
    external()
        .into_iter()
        .find(|(file, _)| normal(file) == wanted)
        .map(|(_, kind)| kind)
}

fn count(text: Option<&str>, needle: &str) -> usize {
    text.map_or(0, |text| text.matches(needle).count())
}

/// How many lines of a TOML file set one of `keys` to `value`.
fn settings(text: Option<&str>, keys: &[&str], value: &str) -> usize {
    text.unwrap_or_default()
        .lines()
        .filter(|line| {
            let line = line.trim().to_ascii_lowercase();
            let Some((key, rest)) = line.split_once('=') else {
                return false;
            };
            keys.contains(&key.trim()) && rest.trim().trim_matches('"') == value
        })
        .count()
}

/// Whether a change to a guarded file turns lock off or routes around it.
pub fn weakens(kind: Kind, before: Option<&str>, after: Option<&str>) -> bool {
    match kind {
        Kind::Claude => {
            let old = before.and_then(|text| serde_json::from_str::<Value>(text).ok());
            let new = after.and_then(|text| serde_json::from_str::<Value>(text).ok());
            match (old, new) {
                (Some(old), Some(new)) => {
                    let old_disabled = old.get("disableAllHooks") == Some(&Value::Bool(true));
                    let new_disabled = new.get("disableAllHooks") == Some(&Value::Bool(true));
                    new_disabled && !old_disabled || permission_hooks(&new) > permission_hooks(&old)
                }
                (Some(_), None) => true,
                (None, _) => {
                    count(after, "disableAllHooks") > count(before, "disableAllHooks")
                        || count(after, "PermissionRequest") > count(before, "PermissionRequest")
                }
            }
        }
        Kind::Codex => {
            let off = |text| settings(text, &["hooks", "codex_hooks"], "false");
            let trusted = |text| settings(text, &["trust_level"], "trusted");
            // Codex runs a project hook only while its reviewed hash is recorded here.
            off(after) > off(before)
                || trusted(after) < trusted(before)
                || count(after, "trusted_hash") < count(before, "trusted_hash")
        }
        Kind::Git => {
            let lower = |text: Option<&str>| text.map(str::to_ascii_lowercase);
            count(lower(after).as_deref(), "hookspath")
                != count(lower(before).as_deref(), "hookspath")
        }
    }
}

fn permission_hooks(value: &Value) -> usize {
    match value {
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| {
                if key == "PermissionRequest" {
                    hooks(value)
                } else {
                    permission_hooks(value)
                }
            })
            .sum(),
        Value::Array(values) => values.iter().map(permission_hooks).sum(),
        _ => 0,
    }
}

fn hooks(value: &Value) -> usize {
    match value {
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| {
                if key == "hooks" {
                    value.as_array().map_or(0, Vec::len)
                } else {
                    hooks(value)
                }
            })
            .sum(),
        Value::Array(values) => values.iter().map(hooks).sum(),
        _ => 0,
    }
}

/// `core.hooksPath` in the repository's own config and in the user's config.
pub fn hooks_paths(root: &Path) -> (Option<String>, Option<String>) {
    let mut local = None;
    let mut global = None;
    if let Ok(output) = git_text(
        root,
        &["config", "--show-scope", "--get-all", "core.hooksPath"],
    ) {
        for line in output.lines() {
            let Some((scope, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match scope {
                "local" => local = Some(value.to_string()),
                "global" => global = Some(value.to_string()),
                _ => {}
            }
        }
    }
    (local, global)
}

/// Puts `core.hooksPath` back the way it was. Returns whether anything changed.
pub fn restore_hooks_paths(root: &Path, before: &(Option<String>, Option<String>)) -> bool {
    let now = hooks_paths(root);
    let mut changed = false;
    for (scope, old, new) in [
        ("--local", &before.0, &now.0),
        ("--global", &before.1, &now.1),
    ] {
        if old == new {
            continue;
        }
        changed = true;
        let _ = match old {
            Some(value) => git_text(root, &["config", scope, "core.hooksPath", value]),
            None => git_text(root, &["config", scope, "--unset-all", "core.hooksPath"]),
        };
    }
    changed
}
