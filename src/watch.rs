//! Shell commands and other tools that cannot be replayed. A command cannot be checked
//! before it runs the way an edit can, because a script decides at run time what it
//! writes. So lock checks what the call did instead. Before an agent runs one, lock
//! copies every file that holds a lock, the lock configuration, the hook files, the
//! agent settings in the home folder, and git's `core.hooksPath` into the git
//! directory. After the call, it compares them. A locked region that changed without
//! an approval is put back, an approval record the call wrote is removed, a setting
//! that turns hooks off is restored, and the agent is told why. A command that runs in
//! the background keeps its copy until the turn ends and is checked then. The copies
//! are removed when the call ends, or when the agent's turn ends.

use crate::guard;
use crate::hook::{HOOK_CONFIGS, weakens_hooks};
use crate::install::ensure_git_hook;
use crate::pending;
use crate::regions::{hash, regions, supports_markers};
use crate::repo::{
    FILE_LOCKS, View, WHOLE_FILE, approved, disk_approvals, git, git_dir, git_text,
    parse_file_locks, violations,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A copy older than this belongs to a command that never reported back.
const STALE_SECONDS: u64 = 6 * 60 * 60;
const MANIFEST: &str = "manifest.json";

fn runs(root: &Path) -> PathBuf {
    git_dir(root).join("lock").join("runs")
}

fn marker_dir(root: &Path) -> PathBuf {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    std::env::temp_dir()
        .join("lock-runs")
        .join(&hash(canonical.to_string_lossy().as_bytes())[..12])
}

fn marker(root: &Path, snapshot: &str) -> PathBuf {
    marker_dir(root).join(snapshot)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn key(session: &str, command: &str) -> String {
    format!(
        "{}-{}",
        &hash(session.as_bytes())[..12],
        &hash(command.trim().as_bytes())[..16]
    )
}

fn walk(root: &Path, dir: &str, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root.join(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = format!("{dir}/{}", entry.file_name().to_string_lossy());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            walk(root, &path, found);
        } else {
            found.push(path);
        }
    }
}

/// The pre-commit hook path and HEAD, from one git call.
fn hooks_and_head(root: &Path) -> (String, Option<String>) {
    // Before the first commit HEAD does not resolve, and git fails the whole call.
    let out = git_text(root, &["rev-parse", "--git-path", "hooks", "HEAD"])
        .or_else(|_| git_text(root, &["rev-parse", "--git-path", "hooks"]))
        .unwrap_or_default();
    let mut lines = out.lines();
    let hooks = lines.next().unwrap_or(".git/hooks").replace('\\', "/");
    (
        format!("{hooks}/pre-commit"),
        lines.next().map(str::to_string),
    )
}

fn is_pre_commit(path: &str) -> bool {
    path.ends_with("/pre-commit")
}

/// Every file whose change lock must check after a command.
fn watched(root: &Path, pre_commit: &str) -> Vec<String> {
    let mut paths = View::Worktree.marked(root);
    paths.extend(
        parse_file_locks(fs::read(root.join(FILE_LOCKS)).ok().as_deref()).unwrap_or_default(),
    );
    walk(root, ".locks", &mut paths);
    paths.extend(HOOK_CONFIGS.map(String::from));
    paths.push(pre_commit.to_string());
    paths.sort();
    paths.dedup();
    paths
}

/// Copies the watched files before an agent command runs.
/// A command that runs in the background reports back before it is done, so its copy
/// stays until the turn ends and is checked then.
pub fn before(root: &Path, session: &str, command: &str, background: bool) -> Result<(), String> {
    prune(root, None);
    let dir = runs(root).join(key(session, command));
    fs::create_dir_all(&dir).map_err(|error| format!("could not copy locked files: {error}"))?;
    let (pre_commit, head) = hooks_and_head(root);
    let mut files = Map::new();
    for (index, path) in watched(root, &pre_commit).into_iter().enumerate() {
        let entry = match fs::read(root.join(&path)) {
            Ok(bytes) => {
                let name = index.to_string();
                fs::write(dir.join(&name), bytes)
                    .map_err(|error| format!("could not copy {path}: {error}"))?;
                json!(name)
            }
            Err(_) => Value::Null,
        };
        files.insert(path, entry);
    }
    // Agent settings outside the repository, and core.hooksPath, which decide
    // whether lock runs at all.
    let mut external = Map::new();
    for (index, (path, _)) in guard::external().into_iter().enumerate() {
        let entry = match fs::read(&path) {
            Ok(bytes) => {
                let name = format!("external-{index}");
                fs::write(dir.join(&name), bytes)
                    .map_err(|error| format!("could not copy {}: {error}", path.display()))?;
                json!(name)
            }
            Err(_) => Value::Null,
        };
        external.insert(path.to_string_lossy().into_owned(), entry);
    }
    let (local, global) = guard::hooks_paths(root);
    let manifest = json!({
        "session": session,
        "started": now(),
        "head": head,
        "preCommit": pre_commit,
        "files": files,
        "external": external,
        "hooksPath": [local, global],
        "background": background,
        "inProgress": in_progress(root),
        "preCommitMode": file_mode(&root.join(&pre_commit)),
    });
    fs::write(dir.join(MANIFEST), format!("{manifest:#}\n"))
        .map_err(|error| format!("could not copy locked files: {error}"))?;
    fs::create_dir_all(marker_dir(root)).map_err(|error| error.to_string())?;
    fs::write(
        marker(root, &key(session, command)),
        dir.to_string_lossy().as_bytes(),
    )
    .map_err(|error| format!("could not write command marker: {error}"))
}

struct Snapshot {
    dir: PathBuf,
    manifest: Value,
}

impl Snapshot {
    fn load(dir: PathBuf) -> Option<Snapshot> {
        let manifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST)).ok()?).ok()?;
        Some(Snapshot { dir, manifest })
    }

    fn field(&self, name: &str) -> Option<&str> {
        self.manifest.get(name).and_then(Value::as_str)
    }

    fn files(&self) -> Vec<String> {
        self.manifest
            .get("files")
            .and_then(Value::as_object)
            .map(|files| files.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `None` when the file was not copied; `Some(None)` when it did not exist.
    fn read(&self, path: &str) -> Result<Option<Option<Vec<u8>>>, String> {
        let Some(entry) = self.manifest.get("files").and_then(|files| files.get(path)) else {
            return Ok(None);
        };
        let Some(name) = entry.as_str() else {
            return Ok(Some(None));
        };
        fs::read(self.dir.join(name))
            .map(|bytes| Some(Some(bytes)))
            .map_err(|error| format!("could not read snapshot copy of {path}: {error}"))
    }

    fn fresh(&self) -> bool {
        let started = self.manifest.get("started").and_then(Value::as_u64);
        started.is_some_and(|at| now().saturating_sub(at) < STALE_SECONDS)
    }
}

fn snapshots(root: &Path) -> Vec<Snapshot> {
    let Ok(entries) = fs::read_dir(runs(root)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| Snapshot::load(entry.path()))
        .collect()
}

/// Removes copies that are stale, and with a session, every copy of that session. An
/// agent's turn has ended, so none of its commands is still running.
pub fn prune(root: &Path, session: Option<&str>) {
    for snapshot in snapshots(root) {
        if !snapshot.fresh() || session.is_some_and(|id| snapshot.field("session") == Some(id)) {
            if let Some(name) = snapshot.dir.file_name() {
                let _ = fs::remove_file(marker(root, &name.to_string_lossy()));
            }
            let _ = fs::remove_dir_all(&snapshot.dir);
        }
    }
}

/// Copies of agent commands that are running now.
fn running(root: &Path) -> Vec<Snapshot> {
    snapshots(root)
        .into_iter()
        .filter(Snapshot::fresh)
        .collect()
}

/// Whether an agent command may be running in this repository now.
pub fn agent_running(root: &Path) -> bool {
    !running(root).is_empty()
}

/// Adds a record that a person approved to every running copy, so the check after an
/// agent command does not take it for the command's own work.
pub fn keep(root: &Path, file: &str) {
    let Ok(bytes) = fs::read(root.join(file)) else {
        return;
    };
    for mut snapshot in running(root) {
        let name = format!("kept-{}", &hash(file.as_bytes())[..12]);
        if fs::write(snapshot.dir.join(&name), &bytes).is_err() {
            continue;
        }
        if let Some(files) = snapshot
            .manifest
            .get_mut("files")
            .and_then(Value::as_object_mut)
        {
            files.insert(file.to_string(), json!(name));
        }
        let _ = fs::write(
            snapshot.dir.join(MANIFEST),
            format!("{:#}\n", snapshot.manifest),
        );
    }
}

fn in_progress(root: &Path) -> bool {
    let dir = git_dir(root);
    [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ]
    .iter()
    .any(|name| dir.join(name).exists())
}

fn put(root: &Path, path: &str, bytes: Option<&[u8]>) -> Result<(), String> {
    let full = root.join(path);
    match bytes {
        Some(bytes) => {
            if let Some(dir) = full.parent() {
                let _ = fs::create_dir_all(dir);
            }
            fs::write(&full, bytes).map_err(|error| format!("could not restore {path}: {error}"))
        }
        None if full.exists() => {
            fs::remove_file(&full).map_err(|error| format!("could not remove {path}: {error}"))
        }
        None => Ok(()),
    }
}

fn count(bytes: Option<&[u8]>, needle: &str) -> usize {
    bytes.map_or(0, |bytes| {
        String::from_utf8_lossy(bytes).matches(needle).count()
    })
}

/// Whether `to` is a state the command may leave a lock in: unchanged, approved, or
/// the state it has in HEAD after a clean move of HEAD, such as a checkout or a pull.
struct Judge<'a> {
    root: &'a Path,
    records: Vec<crate::repo::Approval>,
    head_ok: Option<bool>,
    old_head: Option<String>,
    head_files: HashMap<String, Option<Vec<u8>>>,
}

impl Judge<'_> {
    fn head_moved_cleanly(&mut self) -> bool {
        if let Some(ok) = self.head_ok {
            return ok;
        }
        let new_head = git_text(self.root, &["rev-parse", "--verify", "-q", "HEAD"]).ok();
        let ok = match (&self.old_head, &new_head) {
            (Some(old), Some(new)) if old != new => git_text(self.root, &["merge-base", old, new])
                .ok()
                .and_then(|base| violations(self.root, &View::Commit("HEAD".into()), &base).ok())
                .is_some_and(|problems| problems.is_empty()),
            _ => true,
        };
        self.head_ok = Some(ok);
        ok
    }

    fn head_state(&mut self, path: &str, id: &str) -> Option<String> {
        let root = self.root;
        let bytes = self
            .head_files
            .entry(path.to_string())
            .or_insert_with(|| git(root, &["show", &format!("HEAD:{path}")]).ok())
            .clone()?;
        if id == WHOLE_FILE {
            return Some(hash(&bytes));
        }
        regions(path, &String::from_utf8_lossy(&bytes))
            .ok()?
            .into_iter()
            .find(|region| region.id == id)
            .map(|region| region.hash)
    }

    fn allows(&mut self, path: &str, id: &str, from: Option<&str>, to: Option<&str>) -> bool {
        if from == to || approved(&self.records, path, id, from, to) {
            return true;
        }
        to.is_some() && self.head_state(path, id).as_deref() == to && self.head_moved_cleanly()
    }
}

/// Checks and repairs what an agent command changed. Returns a message for the agent
/// when lock put something back.
/// At the end of a turn: checks each background command of the session.
pub fn settle_background(root: &Path, session: &str) -> Result<Option<String>, String> {
    let mut messages = Vec::new();
    for snapshot in snapshots(root) {
        let background = snapshot.manifest.get("background") == Some(&Value::Bool(true));
        if !background || snapshot.field("session") != Some(session) {
            continue;
        }
        let result = settle(root, &snapshot);
        let _ = fs::remove_file(marker(
            root,
            &snapshot
                .dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        ));
        let _ = fs::remove_dir_all(&snapshot.dir);
        messages.extend(result?);
    }
    Ok((!messages.is_empty()).then(|| {
        messages.join(
            "

",
        )
    }))
}

pub fn after(root: &Path, session: &str, command: &str) -> Result<Option<String>, String> {
    let snapshot_key = key(session, command);
    let path = marker(root, &snapshot_key);
    let Some(snapshot) = Snapshot::load(runs(root).join(&snapshot_key)) else {
        let result = if path.exists() {
            recover_deleted(root)
        } else {
            Ok(None)
        };
        let _ = fs::remove_file(path);
        return result;
    };
    let result = settle(root, &snapshot);
    let result = match result {
        Err(error) => recover_deleted(root).map(|message| {
            Some(format!(
                "{}\n\nlock could not read its snapshot: {error}",
                message
                    .unwrap_or_else(|| "lock restored the locked text to the last commit".into())
            ))
        }),
        other => other,
    };
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(&snapshot.dir);
    result
}

fn recover_deleted(root: &Path) -> Result<Option<String>, String> {
    let problems = violations(root, &View::Worktree, "HEAD")?;
    let mut restored = Vec::new();
    for problem in problems {
        let head = View::Commit("HEAD".into()).read(root, &problem.before.path);
        let full = root.join(&problem.before.path);
        let bytes = match (
            problem.before.id.as_str(),
            head.as_deref(),
            fs::read(&full).ok(),
        ) {
            (WHOLE_FILE, Some(head), _) => head.to_vec(),
            (_, Some(head), Some(current)) => {
                let old = regions(&problem.before.path, &String::from_utf8_lossy(head));
                let new = regions(&problem.before.path, &String::from_utf8_lossy(&current));
                match (old, new) {
                    (Ok(old), Ok(new)) => {
                        let Some(wanted) = old
                            .into_iter()
                            .find(|r| r.id == problem.before.id)
                            .map(|region| region.text)
                        else {
                            put(root, &problem.before.path, Some(head))?;
                            restored
                                .push(format!("{} :: {}", problem.before.path, problem.before.id));
                            continue;
                        };
                        match new.into_iter().find(|r| r.id == problem.before.id) {
                            Some(found) => {
                                let current_text = String::from_utf8_lossy(&current);
                                let normalized = current_text.replace("\r\n", "\n");
                                let restored = normalized.replacen(&found.text, &wanted, 1);
                                if current_text.contains("\r\n") {
                                    restored.replace('\n', "\r\n").into_bytes()
                                } else {
                                    restored.into_bytes()
                                }
                            }
                            None => head.to_vec(),
                        }
                    }
                    _ => head.to_vec(),
                }
            }
            (.., Some(head), None) => head.to_vec(),
            _ => continue,
        };
        put(root, &problem.before.path, Some(&bytes))?;
        restored.push(format!("{} :: {}", problem.before.path, problem.before.id));
    }
    ensure_git_hook(root)?;
    if restored.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "lock: its own copies were deleted during the command, so lock put the locked text back to the last commit: {}.",
        restored.join(", ")
    )))
}

fn settle(root: &Path, snapshot: &Snapshot) -> Result<Option<String>, String> {
    if snapshot.manifest.get("inProgress") == Some(&Value::Bool(true)) && in_progress(root) {
        // A merge or rebase stopped on a conflict; the commit hook judges the result.
        return Ok(None);
    }
    let mut undone: Vec<String> = Vec::new();
    let mut put_back: Vec<(String, String)> = Vec::new();
    let copied = snapshot.files();

    // The lock configuration: an agent may add whole-file locks and nothing else.
    let mut config: Vec<String> = copied
        .iter()
        .filter(|path| path.starts_with(".locks/"))
        .cloned()
        .collect();
    let mut present = Vec::new();
    walk(root, ".locks", &mut present);
    for path in present {
        if !config.contains(&path) {
            config.push(path);
        }
    }
    for path in config {
        let before = snapshot.read(&path)?.unwrap_or(None);
        let now = fs::read(root.join(&path)).ok();
        if now == before {
            continue;
        }
        if path == FILE_LOCKS
            && let (Ok(old), Ok(new)) = (
                parse_file_locks(before.as_deref()),
                parse_file_locks(now.as_deref()),
            )
            && old.iter().all(|item| new.contains(item))
        {
            continue;
        }
        put(root, &path, before.as_deref())?;
        undone.push(path);
    }

    // The hook files must keep running lock.
    for path in &copied {
        let config = HOOK_CONFIGS.contains(&path.as_str());
        if !config && !is_pre_commit(path) {
            continue;
        }
        let before = snapshot.read(path)?.unwrap_or(None);
        let now = fs::read(root.join(path)).ok();
        let weakened = if config {
            weakens_hooks(before.as_deref(), now.as_deref())
        } else {
            count(now.as_deref(), "# lock") < count(before.as_deref(), "# lock")
        };
        let hook_changed = !config && before != now;
        if weakened || hook_changed {
            put(root, path, before.as_deref())?;
            #[cfg(unix)]
            if hook_changed
                && let Some(mode) = snapshot
                    .manifest
                    .get("preCommitMode")
                    .and_then(Value::as_u64)
                    .filter(|mode| mode & 0o111 != 0)
                && let Ok(metadata) = fs::metadata(root.join(path))
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = metadata.permissions();
                permissions.set_mode(mode as u32);
                let _ = fs::set_permissions(root.join(path), permissions);
            }
            undone.push(path.clone());
        }
    }

    // Agent settings outside the repository may not turn hooks off.
    let external = snapshot
        .manifest
        .get("external")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (path, copy) in external {
        let full = PathBuf::from(&path);
        let Some(kind) = guard::external_kind(&full) else {
            continue;
        };
        let before = copy
            .as_str()
            .map(|name| fs::read(snapshot.dir.join(name)))
            .transpose()
            .map_err(|error| format!("could not read snapshot copy of {path}: {error}"))?;
        let now = fs::read(&full).ok();
        let text = |bytes: &Option<Vec<u8>>| {
            bytes
                .as_deref()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        };
        if guard::weakens(kind, text(&before).as_deref(), text(&now).as_deref()) {
            match &before {
                Some(bytes) => fs::write(&full, bytes),
                None => fs::remove_file(&full),
            }
            .map_err(|error| format!("could not restore {path}: {error}"))?;
            undone.push(path);
        }
    }
    if let Some([local, global]) = snapshot
        .manifest
        .get("hooksPath")
        .and_then(Value::as_array)
        .and_then(|pair| <&[Value; 2]>::try_from(pair.as_slice()).ok())
    {
        let text = |value: &Value| value.as_str().map(str::to_string);
        if guard::restore_hooks_paths(root, &(text(local), text(global))) {
            undone.push("core.hooksPath".into());
        }
    }

    let mut judge = Judge {
        root,
        records: disk_approvals(root)?,
        head_ok: None,
        old_head: snapshot.field("head").map(str::to_string),
        head_files: HashMap::new(),
    };
    let mut head_note = None;
    if let Some(old) = judge.old_head.clone()
        && let Ok(new) = git_text(root, &["rev-parse", "--verify", "-q", "HEAD"])
        && old != new
        && !judge.head_moved_cleanly()
    {
        let ancestor = git(root, &["merge-base", "--is-ancestor", &old, &new]).is_ok();
        if ancestor {
            let commits = git_text(root, &["log", "--format=%h", &format!("{old}..{new}")])
                .unwrap_or_default();
            git(root, &["reset", "--soft", &old]).map_err(|error| {
                format!("could not undo commits that skipped the lock hook: {error}")
            })?;
            judge.head_ok = None;
            head_note = Some(format!(
                "lock: HEAD moved through commits that skipped the hook, so lock undid them ({}) and kept their changes staged.\n",
                commits.lines().collect::<Vec<_>>().join(", ")
            ));
        } else {
            head_note = Some(format!(
                "lock: HEAD moved with locked changes; `lock check --base {old}` fails, so lock could not undo the commits.\n"
            ));
        }
    }

    // Whole-file locks, as the configuration listed them before the command.
    let whole: Vec<String> =
        parse_file_locks(snapshot.read(FILE_LOCKS)?.flatten().as_deref()).unwrap_or_default();
    for path in &whole {
        let Some(before) = snapshot.read(path)? else {
            continue;
        };
        let now = fs::read(root.join(path)).ok();
        let (from, to) = (before.as_deref().map(hash), now.as_deref().map(hash));
        if judge.allows(path, WHOLE_FILE, from.as_deref(), to.as_deref()) {
            continue;
        }
        put(root, path, before.as_deref())?;
        pending::save(
            root,
            path,
            WHOLE_FILE,
            (
                from,
                &String::from_utf8_lossy(before.as_deref().unwrap_or_default()),
            ),
            (
                to,
                &String::from_utf8_lossy(now.as_deref().unwrap_or_default()),
            ),
        );
        put_back.push((path.clone(), WHOLE_FILE.to_string()));
    }

    // Locked regions inside files.
    for path in &copied {
        if path.starts_with(".locks/") || !supports_markers(path) || whole.contains(path) {
            continue;
        }
        let Some(Some(before)) = snapshot.read(path)? else {
            continue;
        };
        let now = fs::read(root.join(path)).ok();
        if now.as_deref() == Some(before.as_slice()) {
            continue;
        }
        let before_text = String::from_utf8_lossy(&before).replace("\r\n", "\n");
        let Ok(old) = regions(path, &before_text) else {
            continue;
        };
        if old.is_empty() {
            continue;
        }
        let now_text = now
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let new = now_text
            .as_deref()
            .and_then(|text| regions(path, text).ok());
        let mut text = now_text
            .as_deref()
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let mut whole_file = new.is_none();
        let mut changed = false;
        for region in &old {
            let next = new
                .as_ref()
                .and_then(|found| found.iter().find(|candidate| candidate.id == region.id));
            let to = next.map(|candidate| candidate.hash.as_str());
            if judge.allows(path, &region.id, Some(&region.hash), to) {
                continue;
            }
            pending::save(
                root,
                path,
                &region.id,
                (Some(region.hash.clone()), &region.text),
                (
                    to.map(str::to_string),
                    next.map(|candidate| candidate.text.as_str())
                        .unwrap_or_default(),
                ),
            );
            put_back.push((path.clone(), region.id.clone()));
            match next {
                Some(candidate) => {
                    text = text.replacen(&candidate.text, &region.text, 1);
                    changed = true;
                }
                None => whole_file = true,
            }
        }
        if whole_file && put_back.iter().any(|(done, _)| done == path) {
            put(root, path, Some(&before))?;
        } else if changed {
            let crlf = now_text
                .as_deref()
                .is_some_and(|text| text.contains("\r\n"));
            let text = if crlf {
                text.replace('\n', "\r\n")
            } else {
                text
            };
            put(root, path, Some(text.as_bytes()))?;
        }
    }

    if undone.is_empty() && put_back.is_empty() {
        return Ok(head_note);
    }
    let mut lines = Vec::new();
    if !put_back.is_empty() {
        let list: Vec<String> = put_back
            .iter()
            .map(|(path, id)| format!("{path} :: {id}"))
            .collect();
        let target = if put_back[0].1 == WHOLE_FILE {
            &put_back[0].0
        } else {
            &put_back[0].1
        };
        lines.push(format!(
            "lock: this command changed locked text, so lock put it back: {}. The command's other changes were kept. If this change is wanted, ask the user to run `lock approve {target}` in their own terminal, then run the same command again. Do not work around the lock.",
            list.join(", ")
        ));
    }
    if !undone.is_empty() {
        lines.push(format!(
            "lock: this command changed {}, which only a person may change, so lock put it back.",
            undone.join(", ")
        ));
    }
    if let Some(note) = head_note {
        lines.insert(0, note);
    }
    Ok(Some(lines.join("\n\n")))
}
