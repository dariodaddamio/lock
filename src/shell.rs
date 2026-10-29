//! Shell commands. A command cannot be replayed the way an edit can, so the hook
//! denies any command that could write to a locked file, and any command that tries
//! to approve a lock. This check reads the command text, so it is a best effort:
//! a command that builds a path at run time can get past it. The commit hook and
//! the CI check catch what gets past.

use crate::paths::relative;
use crate::repo::{FILE_LOCKS, View, git, inventory, parse_file_locks};
use std::path::Path;

const WRITERS: [&str; 40] = [
    "mv",
    "cp",
    "rm",
    "rmdir",
    "del",
    "erase",
    "move",
    "copy",
    "ren",
    "rename",
    "truncate",
    "dd",
    "tee",
    "touch",
    "ln",
    "unlink",
    "shred",
    "rsync",
    "robocopy",
    "xcopy",
    "patch",
    "set-content",
    "add-content",
    "out-file",
    "remove-item",
    "move-item",
    "copy-item",
    "rename-item",
    "new-item",
    "clear-content",
    "sc",
    "ac",
    "ri",
    "mi",
    "cpi",
    "ni",
    "rni",
    "writealltext",
    "writeallbytes",
    "writealllines",
];
const GIT_WRITERS: [&str; 15] = [
    "checkout",
    "restore",
    "reset",
    "stash",
    "apply",
    "am",
    "mv",
    "rm",
    "merge",
    "rebase",
    "cherry-pick",
    "revert",
    "pull",
    "switch",
    "clean",
];
const INTERPRETERS: [&str; 14] = [
    "python",
    "python3",
    "py",
    "node",
    "deno",
    "bun",
    "ruby",
    "php",
    "pwsh",
    "powershell",
    "bash",
    "sh",
    "zsh",
    "cmd",
];
const INLINE_CODE: [&str; 7] = ["-c", "-e", "--eval", "-command", "/c", "-p", "-r"];

/// Words of a command, lower-cased, with quotes and shell punctuation removed.
fn words(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || "\"'`;|&(){}<>=,".contains(c))
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase().replace('\\', "/"))
        .collect()
}

/// Output redirection other than to a null device or another stream.
fn redirects(command: &str) -> bool {
    let mut rest = command;
    while let Some(at) = rest.find('>') {
        let after = rest[at + 1..].trim_start_matches('>').trim_start();
        let harmless = after.starts_with('&')
            || ["/dev/null", "$null", "nul"]
                .iter()
                .any(|sink| after.to_ascii_lowercase().starts_with(sink));
        if !harmless {
            return true;
        }
        rest = &rest[at + 1..];
    }
    false
}

fn writes(command: &str, words: &[String]) -> bool {
    let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
    let program = |word: &str| {
        word.rsplit('/')
            .next()
            .unwrap_or(word)
            .trim_end_matches(".exe")
            .to_string()
    };
    let programs: Vec<String> = words.iter().map(|word| program(word)).collect();
    let in_place = programs.iter().any(|word| word == "sed" || word == "perl")
        && words.iter().any(|word| {
            word == "--in-place"
                || (word.starts_with('-') && !word.starts_with("--") && word.contains('i'))
        });
    let git_write = programs
        .windows(2)
        .any(|pair| pair[0] == "git" && GIT_WRITERS.contains(&pair[1].as_str()));
    let inline = programs
        .iter()
        .any(|word| INTERPRETERS.contains(&word.as_str()))
        && has(&INLINE_CODE);
    redirects(command)
        || in_place
        || git_write
        || inline
        || programs.iter().any(|word| WRITERS.contains(&word.as_str()))
        || words
            .iter()
            .any(|word| word.starts_with("[io.file]::") || word == "--write" || word == "--fix")
}

/// A simple glob for `*` and `?`.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(at) = star {
            pi = at + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn names(word: &str, guarded: &str) -> bool {
    let word = word.trim_start_matches("./").trim_end_matches('/');
    let base = guarded.rsplit('/').next().unwrap_or(guarded);
    word == guarded
        || word == "."
        || word == "*"
        || guarded.starts_with(&format!("{word}/"))
        || word.starts_with(&format!("{guarded}/"))
        || word == base
        || word.ends_with(&format!("/{base}"))
        || ((word.contains('*') || word.contains('?')) && (glob(word, guarded) || glob(word, base)))
}

/// Every path that holds a lock, plus the lock configuration and the hook files.
fn guarded(root: &Path) -> Vec<String> {
    let mut paths: Vec<String> = inventory(root, &View::Worktree)
        .map(|map| map.into_keys().map(|(path, _)| path).collect())
        .unwrap_or_default();
    if let Ok(bytes) = std::fs::read(root.join(FILE_LOCKS))
        && let Ok(files) = parse_file_locks(Some(&bytes))
    {
        paths.extend(files);
    }
    paths.extend(
        [
            ".locks",
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".cursor/hooks.json",
        ]
        .map(String::from),
    );
    if let Ok(bytes) = git(root, &["rev-parse", "--git-path", "hooks"]) {
        let hooks = String::from_utf8_lossy(&bytes).trim().replace('\\', "/");
        paths.push(format!("{hooks}/pre-commit"));
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| path.to_ascii_lowercase())
        .collect()
}

pub fn denial(root: &Path, cwd: &Path, command: &str) -> Option<String> {
    let words = words(command);
    let mentions_lock = words.iter().any(|word| {
        let name = word.rsplit('/').next().unwrap_or(word);
        name == "lock" || name == "lock.exe"
    });
    if mentions_lock && words.iter().any(|word| word == "approve") {
        return Some(
            "lock: only a person approves a locked change, by running `lock approve` in their own terminal. Ask the user to run it."
                .to_string(),
        );
    }
    if !writes(command, &words) {
        return None;
    }
    let guarded = guarded(root);
    let hit = words.iter().find_map(|word| {
        let from_root = relative(root, cwd, word).map(|path| path.to_ascii_lowercase());
        guarded.iter().find(|path| {
            names(word, path)
                || from_root
                    .as_deref()
                    .is_some_and(|rel| !rel.is_empty() && names(rel, path))
        })
    })?;
    Some(format!(
        "lock: this command could write to {hit}, which holds a lock. A shell command cannot be checked against a lock, so it was blocked. Use the Edit or Write tool for changes outside locked regions, and narrow the command to other paths. A change inside a locked region needs `lock approve`, run by the user."
    ))
}
