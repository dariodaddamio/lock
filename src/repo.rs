//! Locks and approvals as git sees them: in a commit, in the index, or in the
//! working tree.

use crate::paths::eq;
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
    pub fn marked(&self, root: &Path) -> Vec<String> {
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
        self.read_all(root, &[path.to_string()]).pop().flatten()
    }

    /// Reads many files in one go. Outside the working tree, one `git cat-file`
    /// process serves every file, because starting git once per file is slow on large
    /// repositories, and slowest on Windows.
    pub fn read_all(&self, root: &Path, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        let prefix = match self {
            View::Worktree => {
                return paths
                    .iter()
                    .map(|path| fs::read(root.join(path)).ok())
                    .collect();
            }
            View::Index => String::new(),
            View::Commit(rev) => rev.clone(),
        };
        if paths.is_empty() {
            return Vec::new();
        }
        let mut input = String::new();
        for path in paths {
            input.push_str(&format!("{prefix}:{path}\n"));
        }
        let output = Command::new("git")
            .args(["cat-file", "--batch"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    // A failed write shows up as missing objects below.
                    let _ = stdin.write_all(input.as_bytes());
                }
                child.wait_with_output()
            });
        let Ok(output) = output else {
            return vec![None; paths.len()];
        };
        // Each answer is `<oid> <type> <size>\n<content>\n`, or `<name> missing\n`.
        let bytes = output.stdout;
        let mut at = 0;
        let mut found = Vec::with_capacity(paths.len());
        for _ in paths {
            let Some(end) = bytes[at..].iter().position(|b| *b == b'\n') else {
                found.push(None);
                continue;
            };
            let header = String::from_utf8_lossy(&bytes[at..at + end]).into_owned();
            at += end + 1;
            let size = header
                .rsplit(' ')
                .next()
                .and_then(|size| size.parse::<usize>().ok())
                .filter(|_| header.split(' ').nth(1) == Some("blob"));
            match size {
                Some(size) if at + size <= bytes.len() => {
                    found.push(Some(bytes[at..at + size].to_vec()));
                    at += size + 1;
                }
                _ => {
                    // A tree or commit has content too, which is skipped.
                    if let Some(size) = header
                        .rsplit(' ')
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                        && !header.ends_with(" missing")
                    {
                        at = (at + size + 1).min(bytes.len());
                    }
                    found.push(None);
                }
            }
        }
        found
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
                .map(|path| {
                    let original = path.to_string();
                    let mut parts = Vec::new();
                    let normalized = path.replace('\\', "/");
                    for part in normalized.split('/') {
                        match part {
                            "" | "." => {}
                            ".." if parts.pop().is_none() => {
                                return Err(format!(
                                    "{FILE_LOCKS} lists escaping path entry {original}"
                                ));
                            }
                            ".." => {}
                            part => parts.push(part),
                        }
                    }
                    Ok(parts.join("/"))
                })
                .ok_or_else(|| format!("{FILE_LOCKS} lists something that is not a path"))
                .and_then(|path| path)
        })
        .collect()
}

type Locks = BTreeMap<(String, String), Entry>;

/// Every lock in a view, keyed by path and id.
pub fn inventory(root: &Path, view: &View) -> Result<Locks, String> {
    let whole = parse_file_locks(view.read(root, FILE_LOCKS).as_deref())?;
    let mut paths = view.marked(root);
    paths.extend(whole.iter().cloned());
    paths.sort();
    paths.dedup();
    locks_in(root, view, &paths, &whole)
}

/// The locks in some files of a view. `whole` lists the whole-file locks of the view.
fn locks_in(root: &Path, view: &View, paths: &[String], whole: &[String]) -> Result<Locks, String> {
    let mut map = BTreeMap::new();
    for (path, bytes) in paths.iter().zip(view.read_all(root, paths)) {
        if whole.iter().any(|locked| eq(locked, path)) {
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
        // Binary files hold no markers, as with `git grep -I`.
        let Some(bytes) = bytes.filter(|bytes| !bytes.contains(&0)) else {
            continue;
        };
        let (text, _) = regions::decode(&bytes);
        for region in regions::regions(path, &text)? {
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

fn approvals_since(root: &Path, view: &View, base: Option<&str>) -> Result<Vec<Approval>, String> {
    // A record that the base already has was used by an earlier commit, so it cannot
    // approve anything again. An error here must not let old records count.
    let base_paths = match base {
        Some(base) => Some(
            git(
                root,
                &["ls-tree", "-r", "--name-only", base, "--", APPROVALS],
            )
            .map(|bytes| lines(&bytes).into_iter().collect::<HashSet<_>>())?,
        ),
        None => None,
    };
    let paths: Vec<String> = view
        .files(root, APPROVALS)
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .filter(|path| {
            base_paths
                .as_ref()
                .is_none_or(|paths| !paths.contains(path))
        })
        .collect();
    paths
        .iter()
        .zip(view.read_all(root, &paths))
        .filter_map(|(path, bytes)| bytes.map(|bytes| (path, bytes)))
        .map(|(path, bytes)| parse_approval(path, &bytes))
        .collect()
}

/// Approvals in the working tree that `HEAD` does not have yet. A committed record was
/// used by that commit, so it cannot approve a change again. Before the first commit
/// every record counts.
pub fn disk_approvals(root: &Path) -> Result<Vec<Approval>, String> {
    let tracked = if git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok() {
        git(
            root,
            &["ls-tree", "-r", "--name-only", "HEAD", "--", APPROVALS],
        )
        .map(|bytes| lines(&bytes).into_iter().collect::<HashSet<_>>())?
    } else {
        HashSet::new()
    };
    let Ok(dir) = fs::read_dir(root.join(APPROVALS)) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for item in dir.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let path = format!("{APPROVALS}/{name}");
        if tracked.contains(&path) {
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
    // Only a file that differs from the base can hold a changed lock, so the check
    // reads those files and stays fast in a large repository.
    let mut args = vec!["diff", "--name-only", "--no-renames"];
    if let View::Index = target {
        args.push("--cached");
    }
    args.push(base);
    if let View::Commit(rev) = target {
        args.push(rev);
    }
    let mut changed: Vec<String> = lines(&git(root, &args)?);
    let base_view = View::Commit(base.to_string());
    let base_whole = parse_file_locks(base_view.read(root, FILE_LOCKS).as_deref())?;
    if changed.iter().any(|path| path == FILE_LOCKS) {
        // A whole-file lock can disappear from the list while its file stays the same.
        changed.extend(base_whole.iter().cloned());
    }
    changed.sort();
    changed.dedup();
    let baseline = locks_in(root, &base_view, &changed, &base_whole)?;
    let target_whole = parse_file_locks(target.read(root, FILE_LOCKS).as_deref())?;
    let current = locks_in(root, target, &changed, &target_whole)?;
    let records = approvals_since(root, target, Some(base))?;
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
