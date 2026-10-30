//! `lock approve`: a person reviews each blocked or changed lock and records the
//! changes they accept. It refuses to run without an interactive terminal, so an
//! agent cannot approve its own change.

use crate::pending::{self, Pending};
use crate::regions::hash;
use crate::repo::{APPROVALS, View, WHOLE_FILE, git_text, lock_state, utc_time, violations};
use serde_json::json;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

struct Candidate {
    path: String,
    id: String,
    before: Option<String>,
    after: Option<String>,
    before_text: String,
    after_text: String,
    pending: Option<Pending>,
}

/// The lock's current state on disk, to tell whether a pending proposal still applies.
fn current(root: &Path, path: &str, id: &str) -> Option<String> {
    lock_state(path, id, &fs::read(root.join(path)).ok()?)
}

fn candidates(root: &Path) -> Result<Vec<Candidate>, String> {
    let mut found = Vec::new();
    for item in pending::list(root) {
        if current(root, &item.path, &item.id) != item.before {
            // The lock moved on since the agent proposed this; the proposal is stale.
            let _ = fs::remove_file(&item.file);
            continue;
        }
        found.push(Candidate {
            path: item.path.clone(),
            id: item.id.clone(),
            before: item.before.clone(),
            after: item.after.clone(),
            before_text: item.before_text.clone(),
            after_text: item.after_text.clone(),
            pending: Some(item),
        });
    }
    for problem in violations(root, &View::Worktree, "HEAD")? {
        found.push(Candidate {
            path: problem.before.path.clone(),
            id: problem.before.id.clone(),
            before: problem.before.hash.clone(),
            after: problem.after.as_ref().and_then(|entry| entry.hash.clone()),
            before_text: problem.before.text.clone(),
            after_text: problem.after.map(|entry| entry.text).unwrap_or_default(),
            pending: None,
        });
    }
    Ok(found)
}

fn show_diff(candidate: &Candidate) {
    let dir = std::env::temp_dir().join(format!("lock-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let (a, b) = (dir.join("before"), dir.join("after"));
    let _ = fs::write(&a, format!("{}\n", candidate.before_text));
    let _ = fs::write(
        &b,
        if candidate.after.is_some() {
            format!("{}\n", candidate.after_text)
        } else {
            String::new()
        },
    );
    // git diff exits with 1 whenever the files differ, which is always the case here.
    // Its header names the temporary files, so only the hunks are shown.
    let output = Command::new("git")
        .args(["diff", "--no-index", "--color=always", "--"])
        .arg(&a)
        .arg(&b)
        .output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(&output.stdout);
        let hunks = text.lines().skip_while(|line| !line.contains("@@"));
        for line in hunks {
            println!("{line}");
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Writes one approval record and returns its path from the repository root.
pub fn record(
    root: &Path,
    path: &str,
    id: &str,
    before: Option<&str>,
    after: Option<&str>,
    reason: &str,
) -> Result<String, String> {
    let name = git_text(root, &["config", "user.name"]).unwrap_or_default();
    let email = git_text(root, &["config", "user.email"]).unwrap_or_default();
    let record = json!({
        "version": 1,
        "path": path,
        "id": id,
        "before": before,
        "after": after,
        "approvedBy": format!("{name} <{email}>"),
        "approvedAt": utc_time(),
        "reason": reason,
    });
    let slug = if id == WHOLE_FILE {
        path.replace(['/', '\\'], "-")
    } else {
        id.to_string()
    };
    let to = after.unwrap_or("removed");
    let file = format!(
        "{APPROVALS}/{slug}.{}.{}.json",
        &hash(path.as_bytes())[..6],
        &to[..to.len().min(12)]
    );
    fs::create_dir_all(root.join(APPROVALS)).map_err(|error| error.to_string())?;
    fs::write(root.join(&file), format!("{record:#}\n")).map_err(|error| error.to_string())?;
    Ok(file)
}

/// Refuses to go on unless a person types into this terminal. Agents and CI run
/// commands without a keyboard.
pub fn require_person(command: &str) -> Result<(), String> {
    if !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || std::env::var_os("CI").is_some()
    {
        return Err(format!(
            "{command} needs a person at an interactive terminal; it never runs in CI or from piped input"
        ));
    }
    Ok(())
}

pub fn run(root: &Path, filters: &[String], reason: Option<&str>) -> Result<i32, String> {
    require_person("lock approve")?;
    if crate::watch::agent_running(root) {
        // The check after that command undoes any approval made while it ran, because
        // an agent could fake a terminal. lock cannot tell the two apart.
        println!(
            "An agent command is running in this repository. An approval that you make now is undone when that command ends, so approve again after the agent's turn."
        );
    }
    // Only records that HEAD does not have yet count, as in the hook and the commit check.
    let records = crate::repo::disk_approvals(root, None)?;
    let mut list: Vec<Candidate> = candidates(root)?
        .into_iter()
        .filter(|item| {
            filters.is_empty() || filters.iter().any(|f| *f == item.id || *f == item.path)
        })
        .filter(|item| {
            !crate::repo::approved(
                &records,
                &item.path,
                &item.id,
                item.before.as_deref(),
                item.after.as_deref(),
            )
        })
        .collect();
    list.dedup_by(|a, b| a.path == b.path && a.id == b.id && a.after == b.after);
    if list.is_empty() {
        println!("Nothing to approve: every lock matches HEAD or already has an approval.");
        return Ok(0);
    }
    let stdin = std::io::stdin();
    for item in list {
        let source = if item.pending.is_some() {
            "proposed by an agent and blocked"
        } else {
            "changed in the working tree"
        };
        let removed = if item.after.is_none() {
            ", removed"
        } else {
            ""
        };
        println!("\n{} :: {} ({source}{removed})", item.path, item.id);
        show_diff(&item);
        print!(
            "Type APPROVE {} to approve this exact change, or press Enter to skip: ",
            item.id
        );
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        stdin
            .lock()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if answer.trim() != format!("APPROVE {}", item.id) {
            println!("Skipped.");
            continue;
        }
        let file = record(
            root,
            &item.path,
            &item.id,
            item.before.as_deref(),
            item.after.as_deref(),
            reason.unwrap_or_default(),
        )?;
        if let Some(done) = &item.pending {
            let _ = fs::remove_file(&done.file);
        }
        let next = if item.pending.is_some() {
            " The agent can now retry the same edit or command."
        } else {
            ""
        };
        println!("Recorded {file}. Commit it with the change.{next}");
    }
    Ok(0)
}
