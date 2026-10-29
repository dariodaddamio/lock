//! Locks and approvals as git sees them: in a commit, in the index, or in the
//! working tree.

use crate::regions::{self, hash};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const APPROVALS: &str = ".locks/approvals";
pub const FILE_LOCKS: &str = ".locks/files.json";
/// The region id that stands for a whole-file lock.
pub const WHOLE_FILE: &str = "file";

pub fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-c")
        .arg("core.quotepath=off")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    git(root, args).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Where the locks are read from.
#[derive(Clone, Debug)]
pub enum View {
    Worktree,
    Index,
    Commit(String),
}

impl View {
    /// Paths that mention `LOCKED:`, found with `git grep` so large repositories stay fast.
    fn marked(&self, root: &Path) -> Vec<String> {
        let mut args = vec!["grep", "-l", "-I", "-F", "-e", "LOCKED:"];
        match self {
            View::Worktree => args.push("--untracked"),
            View::Index => args.push("--cached"),
            View::Commit(rev) => {
                args.push(rev);
                args.push("--");
            }
        }
        let found = git(root, &args)
            .map(|bytes| lines(&bytes))
            .unwrap_or_default();
        match self {
            View::Commit(rev) => found
                .into_iter()
                .map(|line| {
                    line.strip_prefix(&format!("{rev}:"))
                        .unwrap_or(&line)
                        .to_string()
                })
                .collect(),
            _ => found,
        }
    }

    fn files(&self, root: &Path, dir: &str) -> Vec<String> {
        let output = match self {
            View::Worktree => git(
                root,
                &[
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "--",
                    dir,
                ],
            ),
            View::Index => git(root, &["ls-files", "--cached", "--", dir]),
            View::Commit(rev) => git(root, &["ls-tree", "-r", "--name-only", rev, "--", dir]),
        };
        output.map(|bytes| lines(&bytes)).unwrap_or_default()
    }

    pub fn read(&self, root: &Path, path: &str) -> Option<Vec<u8>> {
        match self {
            View::Worktree => fs::read(root.join(path)).ok(),
            View::Index => git(root, &["show", &format!(":{path}")]).ok(),
            View::Commit(rev) => git(root, &["show", &format!("{rev}:{path}")]).ok(),
        }
    }
}

/// One locked thing: a region inside a file, or a whole file. A whole file that does
/// not exist has no hash.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub id: String,
    pub text: String,
    pub hash: Option<String>,
}

/// Whole-file locks listed in `.locks/files.json`.
pub fn parse_file_locks(bytes: Option<&[u8]>) -> Result<Vec<String>, String> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    let parsed: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{FILE_LOCKS} is not valid JSON: {error}"))?;
    let files = parsed
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{FILE_LOCKS} must look like {{\"files\": [\"path\", ...]}}"))?;
    files
        .iter()
        .map(|item| {
            item.as_str()
                .map(|path| path.replace('\\', "/"))
                .ok_or_else(|| format!("{FILE_LOCKS} lists something that is not a path"))
        })
        .collect()
}

/// Every lock in a view, keyed by path and id.
pub fn inventory(root: &Path, view: &View) -> Result<BTreeMap<(String, String), Entry>, String> {
    let mut map = BTreeMap::new();
    for path in view.marked(root) {
        let Some(bytes) = view.read(root, &path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for region in regions::regions(&path, &text)? {
            map.insert(
                (path.clone(), region.id.clone()),
                Entry {
                    path: path.clone(),
                    id: region.id,
                    text: region.text,
                    hash: Some(region.hash),
                },
            );
        }
    }
    for path in parse_file_locks(view.read(root, FILE_LOCKS).as_deref())? {
        let bytes = view.read(root, &path);
        map.insert(
            (path.clone(), WHOLE_FILE.to_string()),
            Entry {
                path: path.clone(),
                id: WHOLE_FILE.to_string(),
                text: bytes
                    .as_deref()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default(),
                hash: bytes.as_deref().map(hash),
            },
        );
    }
    Ok(map)
}

/// One approved change: the lock moved from `before` to `after`. `None` means the
/// file did not exist, or the lock was removed.
#[derive(Clone, Debug)]
pub struct Approval {
    pub path: String,
    pub id: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

pub fn parse_approval(name: &str, bytes: &[u8]) -> Result<Approval, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| format!("{name} is not valid JSON"))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(Approval {
        path: text("path").ok_or_else(|| format!("{name} has no path"))?,
        id: text("id").ok_or_else(|| format!("{name} has no id"))?,
        before: text("before"),
        after: text("after"),
    })
}

pub fn approvals(root: &Path, view: &View) -> Result<Vec<Approval>, String> {
    view.files(root, APPROVALS)
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .filter_map(|path| view.read(root, &path).map(|bytes| (path, bytes)))
        .map(|(path, bytes)| parse_approval(&path, &bytes))
        .collect()
}

/// Approvals in the working tree, read straight from disk so the agent hook needs no git.
pub fn disk_approvals(root: &Path) -> Result<Vec<Approval>, String> {
    let Ok(dir) = fs::read_dir(root.join(APPROVALS)) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for item in dir.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = fs::read(item.path()).map_err(|error| format!("{name}: {error}"))?;
        found.push(parse_approval(&name, &bytes)?);
    }
    Ok(found)
}

/// Whether approvals lead from one state of a lock to another. Several approved
/// steps in a row count, so a region approved twice since the last commit passes.
pub fn approved(
    records: &[Approval],
    path: &str,
    id: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> bool {
    if from == to {
        return true;
    }
    let mut seen: HashSet<Option<String>> = HashSet::new();
    let mut queue = VecDeque::from([from.map(str::to_string)]);
    while let Some(state) = queue.pop_front() {
        if !seen.insert(state.clone()) {
            continue;
        }
        for record in records
            .iter()
            .filter(|r| r.path == path && r.id == id && r.before == state)
        {
            if record.after.as_deref() == to {
                return true;
            }
            if record.after.is_some() {
                queue.push_back(record.after.clone());
            }
        }
    }
    false
}

#[derive(Debug)]
pub struct Violation {
    pub before: Entry,
    pub after: Option<Entry>,
}

/// Locks that existed at the baseline and changed or disappeared without approval.
/// A new lock needs no approval, because adding one only adds protection.
pub fn violations(root: &Path, target: &View, base: &str) -> Result<Vec<Violation>, String> {
    if git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base}^{{commit}}"),
        ],
    )
    .is_err()
    {
        return Ok(Vec::new());
    }
    let baseline = inventory(root, &View::Commit(base.to_string()))?;
    let current = inventory(root, target)?;
    let records = approvals(root, target)?;
    let mut problems = Vec::new();
    for (key, before) in baseline {
        let after = current.get(&key).cloned();
        let after_hash = after.as_ref().and_then(|entry| entry.hash.clone());
        if after.is_some() && after_hash == before.hash {
            continue;
        }
        if !approved(
            &records,
            &before.path,
            &before.id,
            before.hash.as_deref(),
            after_hash.as_deref(),
        ) {
            problems.push(Violation { before, after });
        }
    }
    Ok(problems)
}

/// The repository root above `start`, found without running git so the agent hook
/// stays fast.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The git directory for a root, following the `gitdir:` file that worktrees use.
pub fn git_dir(root: &Path) -> PathBuf {
    let dot_git = root.join(".git");
    if dot_git.is_file()
        && let Ok(text) = fs::read_to_string(&dot_git)
        && let Some(dir) = text.trim().strip_prefix("gitdir:")
    {
        let dir = PathBuf::from(dir.trim());
        return if dir.is_absolute() {
            dir
        } else {
            root.join(dir)
        };
    }
    dot_git
}
