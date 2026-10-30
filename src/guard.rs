//! Settings outside the locked files that decide whether lock runs at all: each
//! agent's own settings in the home directory, and git's `core.hooksPath`, which can
//! point the commit hook somewhere else. An agent may edit these files, but it may not
//! use them to turn hooks off.

use crate::repo::git_text;
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
            count(after, "disableAllHooks") > count(before, "disableAllHooks")
                || count(after, "PermissionRequest") > count(before, "PermissionRequest")
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

/// `core.hooksPath` in the repository's own config and in the user's config.
pub fn hooks_paths(root: &Path) -> (Option<String>, Option<String>) {
    let read = |scope: &str| {
        git_text(root, &["config", scope, "--get", "core.hooksPath"])
            .ok()
            .filter(|value| !value.is_empty())
    };
    (read("--local"), read("--global"))
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
