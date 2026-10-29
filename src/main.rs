//! lock: lock regions of a repository so coding agents cannot change them without a
//! person's approval.
//!
//!   lock hook claude|cursor                   the agent hook; reads one tool call on stdin
//!   lock check [--staged | --base <ref>]      verify locks against git
//!   lock approve [<id>|<path>...] [--reason <text>]
//!                                             review and approve changes (terminal only)
//!   lock list                                 print every lock in the working tree
//!   lock install [--git] [--claude] [--cursor]
//!                                             register the hooks in this clone

mod approve;
mod hook;
mod install;
mod paths;
mod pending;
mod regions;
mod repo;
mod shell;

use repo::{View, WHOLE_FILE, git_text, inventory, violations};
use std::path::PathBuf;

const USAGE: &str = "Usage:
  lock hook claude|cursor
  lock check [--staged | --base <ref>]
  lock approve [<id>|<path>...] [--reason <text>]
  lock list
  lock install [--git] [--claude] [--cursor]";

fn root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    git_text(&cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| "run lock inside a git repository".to_string())
}

fn check(args: &[String]) -> Result<i32, String> {
    let root = root()?;
    let base_ref = args
        .iter()
        .position(|arg| arg == "--base")
        .map(|at| args.get(at + 1).cloned());
    let (view, base, label) = match base_ref {
        Some(None) => return Err("--base needs a ref".into()),
        Some(Some(reference)) => {
            let base = git_text(&root, &["merge-base", &reference, "HEAD"])?;
            let label = format!("merge base {}", &base[..base.len().min(12)]);
            (View::Commit("HEAD".into()), base, label)
        }
        None if args.iter().any(|arg| arg == "--staged") => {
            (View::Index, "HEAD".into(), "HEAD".into())
        }
        None => (View::Worktree, "HEAD".into(), "HEAD".into()),
    };
    let problems = violations(&root, &view, &base)?;
    for problem in &problems {
        let (path, id) = (&problem.before.path, &problem.before.id);
        let what = if problem.after.is_some() {
            "changed"
        } else {
            "was removed"
        };
        let target = if id == WHOLE_FILE { path } else { id };
        eprintln!(
            "LOCKED {path} :: {id} {what} without approval. A person runs `lock approve {target}` and commits the record."
        );
    }
    if !problems.is_empty() {
        return Ok(1);
    }
    let count = inventory(&root, &view)?.len();
    println!("lock: {count} locks match {label} or carry an approval.");
    Ok(0)
}

fn run(args: &[String]) -> Result<i32, String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("hook") => match rest.first().map(String::as_str) {
            Some("claude") => Ok(hook::run(hook::Host::Claude)),
            Some("cursor") => Ok(hook::run(hook::Host::Cursor)),
            _ => Err("lock hook needs a host: claude or cursor".into()),
        },
        Some("check") => check(rest),
        Some("approve") => {
            let at = rest.iter().position(|arg| arg == "--reason");
            let reason = at.and_then(|at| rest.get(at + 1)).map(String::as_str);
            let filters: Vec<String> = rest
                .iter()
                .enumerate()
                .filter(|(index, arg)| {
                    !arg.starts_with("--") && at.is_none_or(|at| *index != at + 1)
                })
                .map(|(_, arg)| arg.clone())
                .collect();
            approve::run(&root()?, &filters, reason)
        }
        Some("list") => {
            for (path, id) in inventory(&root()?, &View::Worktree)?.into_keys() {
                println!("{path} :: {id}");
            }
            Ok(0)
        }
        Some("install") => install::run(rest),
        Some("--version" | "-V") => {
            println!("lock {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(0)
        }
        Some(other) => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args).unwrap_or_else(|error| {
        eprintln!("lock: {error}");
        1
    });
    std::process::exit(code);
}
