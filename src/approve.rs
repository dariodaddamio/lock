//! `lock approve`: a person reviews each blocked or changed lock and records the
//! changes they accept. It refuses to run without an interactive terminal, so an
//! agent cannot approve its own change.
//!
//! The screen logic in `review` reads and writes through plain traits, so it runs in
//! tests without a terminal. Only `run`, which `require_person` guards, connects it to
//! the real keyboard.

use crate::pending::{self, Pending};
use crate::regions::hash;
use crate::repo::{APPROVALS, View, WHOLE_FILE, git_text, lock_state, utc_time, violations};
use serde_json::json;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

/// How wide the thin rules around a diff are. Plain ASCII works in every terminal.
const RULE: &str = "------------------------------------------------------------";
/// How many changes `all` lists before it says how many more there are.
const LISTED: usize = 15;

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

/// Colors only for a terminal that shows them: not when `NO_COLOR` is set to a value
/// (see no-color.org) and not with `TERM=dumb`.
fn wants_color(no_color: Option<&OsStr>, term: Option<&str>) -> bool {
    no_color.is_none_or(OsStr::is_empty) && term != Some("dumb")
}

/// Added and removed lines in a diff. The file headers before the first hunk are not
/// counted.
fn count_lines(diff: &str) -> (usize, usize) {
    diff.lines()
        .skip_while(|line| !line.starts_with("@@"))
        .fold((0, 0), |(added, removed), line| {
            if line.starts_with('+') {
                (added + 1, removed)
            } else if line.starts_with('-') {
                (added, removed + 1)
            } else {
                (added, removed)
            }
        })
}

/// What a change looks like on the screen: line counts and the diff to print.
struct Diff {
    added: usize,
    removed: usize,
    shown: String,
}

/// What `review` needs from the outside world, so tests can stand in for git and the
/// approval records.
trait Review {
    fn diff(&self, change: &Candidate) -> Diff;
    /// Records the approval and returns the record's path.
    fn approve(&self, change: &Candidate) -> Result<String, String>;
}

struct Real<'a> {
    root: &'a Path,
    reason: &'a str,
    color: bool,
}

/// The hunks of `git diff` between two files. The header names the temporary files, so
/// only the hunks are kept. git exits with 1 whenever the files differ.
fn hunks(a: &Path, b: &Path, color: bool) -> String {
    let flag = if color {
        "--color=always"
    } else {
        "--color=never"
    };
    let Ok(output) = Command::new("git")
        .args(["diff", "--no-index", flag, "--"])
        .arg(a)
        .arg(b)
        .output()
    else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut kept = String::new();
    for line in text.lines().skip_while(|line| !line.contains("@@")) {
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

impl Review for Real<'_> {
    fn diff(&self, change: &Candidate) -> Diff {
        let dir = std::env::temp_dir().join(format!("lock-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let (a, b) = (dir.join("before"), dir.join("after"));
        let _ = fs::write(&a, format!("{}\n", change.before_text));
        let _ = fs::write(
            &b,
            if change.after.is_some() {
                format!("{}\n", change.after_text)
            } else {
                String::new()
            },
        );
        let plain = hunks(&a, &b, false);
        let (added, removed) = count_lines(&plain);
        let shown = if self.color {
            hunks(&a, &b, true)
        } else {
            plain
        };
        let _ = fs::remove_dir_all(&dir);
        Diff {
            added,
            removed,
            shown,
        }
    }

    fn approve(&self, change: &Candidate) -> Result<String, String> {
        let file = record(
            self.root,
            &change.path,
            &change.id,
            change.before.as_deref(),
            change.after.as_deref(),
            self.reason,
        )?;
        if let Some(done) = &change.pending {
            let _ = fs::remove_file(&done.file);
        }
        Ok(file)
    }
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

/// What a person typed at the prompt. The lock's id is never part of an answer.
#[derive(Debug, PartialEq)]
enum Answer {
    Approve,
    All,
    Skip,
    Quit,
    Unknown,
}

fn parse_answer(typed: &str) -> Answer {
    match typed.trim().to_ascii_lowercase().as_str() {
        "approve" => Answer::Approve,
        "all" => Answer::All,
        "" => Answer::Skip,
        "q" | "quit" => Answer::Quit,
        _ => Answer::Unknown,
    }
}

/// The second confirmation for `all` names how many changes it covers, so a stray
/// `all` cannot approve anything.
fn confirms_all(typed: &str, count: usize) -> bool {
    typed.trim().to_ascii_lowercase() == format!("approve {count}")
}

/// The header above one change: which change, where, what kind of lock, where it came
/// from, and how many lines it touches.
fn header(number: usize, total: usize, change: &Candidate, added: usize, removed: usize) -> String {
    let kind = if change.id == WHOLE_FILE {
        "whole file".to_string()
    } else {
        format!("region {}", change.id)
    };
    let source = if change.pending.is_some() {
        "an agent's blocked edit"
    } else {
        "a change in your working tree"
    };
    let mut text = format!(
        "Change {number} of {total}\n  File:   {}\n  Lock:   {kind}\n  From:   {source}\n",
        change.path
    );
    if change.after.is_none() {
        text.push_str("  Effect: removed, so this lock goes away if you approve\n");
    }
    text.push_str(&format!("  Lines:  +{added} -{removed}\n"));
    text
}

/// What a review did, for the closing summary.
#[derive(Default)]
struct Summary {
    approved: usize,
    skipped: usize,
    files: Vec<String>,
    /// An agent's blocked change was approved, so the agent can retry it.
    retry: bool,
}

impl Summary {
    fn text(&self) -> String {
        let mut text = format!("Approved {}, skipped {}.", self.approved, self.skipped);
        if !self.files.is_empty() {
            text.push_str("\nCommit these approval records with your change:");
            for file in &self.files {
                text.push_str(&format!("\n  {file}"));
            }
        }
        if self.retry {
            text.push_str("\nThe agent can now retry the same edit or command.");
        }
        text
    }
}

fn ask(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &str,
) -> Result<Option<String>, String> {
    write!(out, "{prompt}").map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())?;
    let mut typed = String::new();
    let read = input
        .read_line(&mut typed)
        .map_err(|error| error.to_string())?;
    // Closed input must end the review, or the prompt would ask forever.
    Ok((read > 0).then_some(typed))
}

/// Shows each change and asks what to do with it. Every answer is read from `input`,
/// so the caller decides who is typing.
fn review(
    items: &[Candidate],
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    ui: &dyn Review,
) -> Result<Summary, String> {
    let mut summary = Summary::default();
    let total = items.len();
    let mut at = 0;
    while at < total {
        let change = &items[at];
        let remaining = total - at - 1;
        let diff = ui.diff(change);
        let shown = header(at + 1, total, change, diff.added, diff.removed);
        writeln!(out, "\n{shown}{RULE}").map_err(|error| error.to_string())?;
        write!(out, "{}", diff.shown).map_err(|error| error.to_string())?;
        writeln!(out, "{RULE}").map_err(|error| error.to_string())?;
        let prompt = if remaining > 0 {
            "Type approve to approve, Enter to skip, all to approve the rest, or q to quit: "
                .to_string()
        } else {
            "Type approve to approve, Enter to skip, or q to quit: ".to_string()
        };
        let mut prompt = prompt.as_str();
        let again = if remaining > 0 {
            "Not approved. Type approve, all, or press Enter to skip. Type q to quit: "
        } else {
            "Not approved. Type approve or press Enter to skip. Type q to quit: "
        };
        let answer = loop {
            let Some(typed) = ask(input, out, prompt)? else {
                writeln!(out, "\nInput ended.").map_err(|error| error.to_string())?;
                break Answer::Quit;
            };
            match parse_answer(&typed) {
                Answer::All if remaining == 0 => prompt = again,
                Answer::Unknown => prompt = again,
                known => break known,
            }
        };
        match answer {
            Answer::Approve => {
                let file = ui.approve(change)?;
                writeln!(out, "Approved. Recorded {file}.").map_err(|error| error.to_string())?;
                summary.approved += 1;
                summary.retry |= change.pending.is_some();
                summary.files.push(file);
                at += 1;
            }
            Answer::All => {
                let count = total - at;
                writeln!(out, "\nThis approves {count} changes, this one included:")
                    .map_err(|error| error.to_string())?;
                for change in items[at..].iter().take(LISTED) {
                    let kind = if change.id == WHOLE_FILE {
                        "whole file".to_string()
                    } else {
                        format!("region {}", change.id)
                    };
                    writeln!(out, "  {} ({kind})", change.path)
                        .map_err(|error| error.to_string())?;
                }
                if count > LISTED {
                    writeln!(out, "  and {} more", count - LISTED)
                        .map_err(|error| error.to_string())?;
                }
                let confirm =
                    format!("Type approve {count} to confirm, or press Enter to go back: ");
                let typed = ask(input, out, &confirm)?;
                if !typed.is_some_and(|typed| confirms_all(&typed, count)) {
                    writeln!(out, "Cancelled. Nothing was approved by all.")
                        .map_err(|error| error.to_string())?;
                    continue;
                }
                for change in &items[at..] {
                    let file = ui.approve(change)?;
                    writeln!(out, "Approved {}. Recorded {file}.", change.path)
                        .map_err(|error| error.to_string())?;
                    summary.approved += 1;
                    summary.retry |= change.pending.is_some();
                    summary.files.push(file);
                }
                at = total;
            }
            Answer::Skip => {
                writeln!(out, "Skipped.").map_err(|error| error.to_string())?;
                summary.skipped += 1;
                at += 1;
            }
            Answer::Quit | Answer::Unknown => {
                summary.skipped += total - at;
                at = total;
            }
        }
    }
    writeln!(out, "\n{}", summary.text()).map_err(|error| error.to_string())?;
    Ok(summary)
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
    let ui = Real {
        root,
        reason: reason.unwrap_or_default(),
        color: wants_color(
            std::env::var_os("NO_COLOR").as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ),
    };
    let stdin = std::io::stdin();
    review(&list, &mut stdin.lock(), &mut std::io::stdout(), &ui)?;
    Ok(0)
}

#[cfg(test)]
#[path = "approve_tests.rs"]
mod tests;
