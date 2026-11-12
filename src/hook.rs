//! The agent hook. Claude Code, Codex, and Cursor run it before and after every tool
//! call, and when the agent's turn ends. Before an edit, it replays the proposed tool call against the file in memory and denies the
//! call when a locked region would change without a person's approval, so the file on
//! disk is never touched. In Claude Code, the same edit tried again asks the person in
//! the permission prompt, see `ask`. Shell commands and other tools that can write
//! are checked by `watch`, and agent settings by `guard`.

use crate::ask::{self, Ask};
use crate::paths::{eq, relative, under};
use crate::regions::{hash, regions, supports_markers};
use crate::repo::{
    APPROVALS, FILE_LOCKS, WHOLE_FILE, approved, disk_approvals, enclosing_roots, find_root,
    git_dir, parse_file_locks,
};
use crate::{guard, patch, pending, shell, watch};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Claude Code and Codex share the `PreToolUse` output contract. Only Claude Code
/// shows a hook's question to the person.
#[derive(Clone, Copy, PartialEq)]
pub enum Host {
    Claude,
    Codex,
    Cursor,
}

/// Agent configuration that keeps this hook running. An agent may edit these files
/// but may not take the hook out or switch hooks off.
pub const HOOK_CONFIGS: [&str; 4] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".cursor/hooks.json",
    ".codex/hooks.json",
];
const SHELL_TOOLS: [&str; 9] = [
    "exec",
    "bash",
    "powershell",
    "shell",
    "terminal",
    "run_terminal_cmd",
    "exec_command",
    "local_shell",
    "monitor",
];
/// Tools that edit files and are replayed before they run. Every other tool that can
/// write is checked by what it changed. Substring match, so `delete`, `move`, and
/// `rename` tools are refused up front too.
const EDIT_WORDS: [&str; 9] = [
    "edit", "write", "patch", "replace", "delete", "remove", "move", "rename", "create",
];
/// Tools that only read, plan, or talk. Exact names only, so `x.read` is not treated as
/// a read. These pass without a snapshot, because a snapshot of every locked file on
/// each Read costs time and can race with an approved edit.
const SAFE_TOOLS: [&str; 34] = [
    "read",
    "read_file",
    "grep",
    "glob",
    "ls",
    "list_dir",
    "listdir",
    "file_search",
    "filesearch",
    "codebase_search",
    "semanticsearch",
    "webfetch",
    "web_fetch",
    "websearch",
    "web_search",
    "todowrite",
    "todo_write",
    "update_plan",
    "task",
    "agent",
    "askuserquestion",
    "exitplanmode",
    "enterplanmode",
    "skill",
    "toolsearch",
    "view_image",
    "bashoutput",
    "taskoutput",
    "notebookread",
    "killshell",
    "taskstop",
    "sendmessage",
    "listagents",
    "read_lints",
];
const WATCHDOG_SECONDS: u64 = 50;

/// What the hook answers.
pub enum Outcome {
    Allow,
    /// After a command, or at the end of a turn, with nothing to report.
    Quiet,
    Deny(String),
    /// Before an edit in Claude Code: the person decides in the permission prompt.
    Ask(String),
    /// After a command ran: lock put locked text back and tells the agent.
    Report(String),
    /// After an edit ran: lock tells the agent what it recorded.
    Note(String),
    /// At the end of a turn: lock tells the person what it put back.
    Notice(String),
}

pub fn run(host: Host) -> i32 {
    let mut input = String::new();
    let decision = std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("could not read the hook input: {error}"))
        .and_then(|_| {
            let payload = serde_json::from_str::<Value>(&input)
                .map_err(|_| "the hook input is not JSON".to_string())?;
            if is_pre_event(&payload) {
                watchdog(host);
            }
            decide(host, &payload)
        });
    if !claim_answer() {
        // The watchdog is answering and will end the process.
        loop {
            std::thread::park();
        }
    }
    match decision {
        Ok(Outcome::Allow) => {
            if let Host::Cursor = host {
                println!("{}", json!({ "permission": "allow" }));
            }
            0
        }
        Ok(Outcome::Quiet) => {
            if let Host::Cursor = host {
                println!("{{}}");
            }
            0
        }
        Ok(Outcome::Deny(reason)) => {
            println!("{}", format_message(host, "deny", &reason));
            0
        }
        Ok(Outcome::Ask(question)) => {
            println!(
                "{}",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": question,
                    }
                })
            );
            0
        }
        Ok(Outcome::Report(message)) => {
            println!("{}", format_message(host, "report", &message));
            0
        }
        Ok(Outcome::Note(message)) => {
            println!("{}", format_message(host, "note", &message));
            0
        }
        Ok(Outcome::Notice(message)) => {
            // Cursor's stop hook can only send the agent on, so it stays quiet there.
            match host {
                Host::Claude | Host::Codex => println!("{}", json!({ "systemMessage": message })),
                Host::Cursor => println!("{{}}"),
            }
            eprintln!("{message}");
            0
        }
        Err(error) => {
            // Fail closed: when the hook cannot tell what a call does, the call does not run.
            let reason = format!("lock could not check this tool call, so it was blocked: {error}");
            println!("{}", format_message(host, "deny", &reason));
            eprintln!("{reason}");
            2
        }
    }
}

fn is_pre_event(payload: &Value) -> bool {
    matches!(
        text(payload, &["hook_event_name", "hookEventName"])
            .unwrap_or("PreToolUse")
            .to_ascii_lowercase()
            .as_str(),
        "pretooluse" | "beforeshellexecution" | "beforemcpexecution"
    )
}

/// Set by whichever answers first, the hook or its watchdog, so the host never gets two
/// answers for one tool call.
static ANSWERED: AtomicBool = AtomicBool::new(false);

/// Claims the right to answer. Returns false when the other side already answered.
fn claim_answer() -> bool {
    !ANSWERED.swap(true, Ordering::SeqCst)
}

/// A hang must fail closed like any other error: the hosts treat a hook that times out
/// as a non-blocking error and run the tool call. The watchdog is armed only before a
/// tool runs, where denying is safe and nothing has been put back yet.
fn watchdog(host: Host) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(WATCHDOG_SECONDS));
        if let Some(answer) = watchdog_answer(host) {
            println!("{answer}");
            eprintln!("{WATCHDOG_REASON}");
            std::process::exit(2);
        }
    });
}

const WATCHDOG_REASON: &str = "lock took too long to check this tool call, so it was blocked";

fn watchdog_answer(host: Host) -> Option<Value> {
    claim_answer().then(|| format_message(host, "deny", WATCHDOG_REASON))
}

pub(crate) fn format_message(host: Host, kind: &str, message: &str) -> Value {
    match host {
        Host::Claude | Host::Codex if kind == "deny" => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": message,
            }
        }),
        Host::Claude | Host::Codex if kind == "report" => json!({
            "decision": "block", "reason": message,
            "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": message }
        }),
        Host::Claude | Host::Codex => json!({
            "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": message }
        }),
        Host::Cursor if kind == "deny" => {
            json!({ "permission": "deny", "user_message": message, "agent_message": message })
        }
        Host::Cursor => json!({ "additional_context": message }),
    }
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

/// A shell command as text. Codex sends some commands as an argument list.
fn command_text(value: &Value) -> Option<String> {
    let command = value.get("command").or_else(|| value.get("cmd"))?;
    match command {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

pub(crate) fn decide(host: Host, payload: &Value) -> Result<Outcome, String> {
    // Cursor sends an empty `cwd` with shell commands, so an empty one means none.
    let cwd = text(payload, &["cwd"])
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            payload
                .get("workspace_roots")
                .and_then(Value::as_array)
                .and_then(|roots| roots.first())
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or("no working directory")?;
    let mut roots = enclosing_roots(&cwd);
    let target = payload
        .get("tool_input")
        .or_else(|| payload.get("arguments"))
        .and_then(|input| {
            text(
                input,
                &[
                    "file_path",
                    "filePath",
                    "path",
                    "target_file",
                    "targetFile",
                    "notebook_path",
                ],
            )
            .or_else(|| {
                text(
                    payload,
                    &[
                        "file_path",
                        "filePath",
                        "path",
                        "target_file",
                        "targetFile",
                        "notebook_path",
                    ],
                )
            })
        });
    if let Some(target) = target {
        let full = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            cwd.join(target)
        };
        roots.extend(enclosing_roots(full.parent().unwrap_or(&full)));
    }
    let mut unique = Vec::new();
    for root in roots {
        let canonical = fs::canonicalize(&root).unwrap_or(root.clone());
        if !unique.iter().any(|seen: &PathBuf| seen == &canonical) {
            unique.push(canonical);
        }
    }
    roots = unique;
    let Some(root) = roots.first().cloned().or_else(|| find_root(&cwd)) else {
        return Ok(Outcome::Allow);
    };
    let protected = protected_dirs(&root);
    let event = text(payload, &["hook_event_name", "hookEventName"])
        .unwrap_or("PreToolUse")
        .to_ascii_lowercase();
    let session = text(payload, &["session_id", "conversation_id"]).unwrap_or_default();
    let call = text(payload, &["tool_use_id"]);
    let tool = text(payload, &["tool_name", "toolName"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let tool = tool.strip_prefix("functions.").unwrap_or(&tool).to_string();
    let input = payload
        .get("tool_input")
        .or_else(|| payload.get("arguments"))
        .unwrap_or(&Value::Null);
    let command = command_text(input).or_else(|| command_text(payload));
    let is_shell = SHELL_TOOLS.contains(&tool.as_str()) || (input.is_null() && command.is_some());
    let background =
        tool == "monitor" || input.get("run_in_background") == Some(&Value::Bool(true));

    match event.as_str() {
        "stop" | "sessionend" => {
            // A command that runs in the background may still change files after it
            // reported back, so lock checks it when the turn ends.
            let mut messages = Vec::new();
            for candidate in &roots {
                match watch::settle_background(candidate, session) {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => {}
                    Err(error) => messages.push(format!(
                        "lock could not check what a background command changed: {error}. Run `lock check`."
                    )),
                }
                watch::prune(candidate, Some(session));
                ask::prune(candidate, session);
            }
            return Ok(if messages.is_empty() {
                Outcome::Quiet
            } else {
                Outcome::Notice(messages.join("\n\n"))
            });
        }
        // Cursor sends both events after a command. Only `postToolUse` can pass a
        // message to the agent, so it does the check, and older installs that still
        // run this hook after a command change nothing here.
        "aftershellexecution" => return Ok(Outcome::Quiet),
        "posttooluse" => {
            if background {
                return Ok(Outcome::Quiet);
            }
            // An edit ran. If lock asked about it, the person said yes.
            if let Some(call) = call.filter(|_| host == Host::Claude && !is_shell) {
                match ask::confirm(&root, call) {
                    Ok(Some(message)) => return Ok(Outcome::Note(message)),
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(Outcome::Report(format!(
                            "lock could not record the approval for this edit: {error}. Ask the user to run `lock approve`."
                        )));
                    }
                }
            }
            let key = match command.filter(|_| is_shell) {
                Some(command) => shell_key(&command, call),
                None => other_key(&tool, call, input),
            };
            // The call already ran, so an error here cannot block it; it is reported.
            let mut messages = Vec::new();
            for candidate in &roots {
                match watch::after(candidate, session, &key) {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => {}
                    Err(error) => messages.push(format!(
                        "lock could not check what this command changed: {error}. Ask the user to run `lock check`."
                    )),
                }
            }
            return Ok(messages
                .first()
                .map_or(Outcome::Quiet, |_| Outcome::Report(messages.join("\n\n"))));
        }
        "posttoolusefailure" | "aftershellexecutionfailure" => {
            let key = match command.filter(|_| is_shell) {
                Some(command) => shell_key(&command, call),
                None => other_key(&tool, call, input),
            };
            let mut messages = Vec::new();
            for candidate in &roots {
                match watch::after(candidate, session, &key) {
                    Ok(Some(message)) => messages.push(message),
                    Ok(None) => {}
                    Err(error) => messages.push(format!(
                        "lock could not check what this command changed: {error}. Ask the user to run `lock approve`."
                    )),
                }
            }
            return Ok(messages
                .first()
                .map_or(Outcome::Quiet, |_| Outcome::Report(messages.join("\n\n"))));
        }
        _ => {}
    }

    let changes = if is_shell {
        let Some(command) = command else {
            return Ok(Outcome::Allow);
        };
        if let Some(reason) = shell::denial(&command) {
            return Ok(Outcome::Deny(reason));
        }
        let mut refusals = Vec::new();
        for candidate in &roots {
            let protected = protected_dirs(candidate);
            let candidate_changes = if command.find("*** Begin Patch").is_some() {
                let start = command.find("*** Begin Patch").unwrap();
                let body = &command[start..];
                let end = body.find("*** End Patch").map_or(body.len(), |at| at + 13);
                patch_changes(candidate, &cwd, &body[..end])
            } else {
                Vec::new()
            };
            refusals.extend(judge_all(candidate, &protected, &candidate_changes)?);
        }
        if !refusals.is_empty() {
            return Ok(Outcome::Deny(reasons(&refusals)));
        }
        let key = shell_key(&command, call);
        for candidate in &roots {
            warn_unsupported(candidate);
            watch::before(candidate, session, &key, background)?;
        }
        return Ok(Outcome::Allow);
    } else if SAFE_TOOLS.contains(&tool.as_str()) {
        return Ok(Outcome::Allow);
    } else if EDIT_WORDS.iter().any(|word| tool.contains(word)) {
        let changes = proposals(&root, &protected, &cwd, &tool, input, payload)?;
        if roots.len() > 1 {
            return decide_nested_edit(host, payload, &cwd, &tool, input, &roots);
        }
        if changes.is_empty() {
            // A tool that lock cannot read, such as an MCP tool, is checked by what it
            // changed.
            warn_unsupported(&root);
            watch::before(&root, session, &other_key(&tool, call, input), false)?;
            return Ok(Outcome::Allow);
        }
        changes
    } else {
        for candidate in &roots {
            warn_unsupported(candidate);
            watch::before(candidate, session, &other_key(&tool, call, input), false)?;
        }
        return Ok(Outcome::Allow);
    };
    let refusals = judge_all(&root, &protected, &changes)?;
    if refusals.is_empty() {
        return Ok(Outcome::Allow);
    }
    // A person may approve a locked change in Claude Code's own prompt. The first
    // attempt is refused, so the chat shows that the text is locked, and the same edit
    // tried again opens the prompt. A refusal for any other reason, such as a broken
    // marker, stays a refusal.
    if host == Host::Claude
        && let Some(call) = call
        && refusals.iter().all(|refusal| !refusal.asks.is_empty())
        && ask::possible(&root, payload)
    {
        let asks: Vec<Ask> = refusals
            .into_iter()
            .flat_map(|refusal| refusal.asks)
            .collect();
        if !ask::retried(&root, session, &asks)? {
            return Ok(Outcome::Deny(ask::first_refusal(&asks)));
        }
        ask::save(&root, session, call, &asks)?;
        return Ok(Outcome::Ask(ask::question(&asks)));
    }
    Ok(Outcome::Deny(reasons(&refusals)))
}

fn decide_nested_edit(
    host: Host,
    payload: &Value,
    cwd: &Path,
    tool: &str,
    input: &Value,
    roots: &[PathBuf],
) -> Result<Outcome, String> {
    let mut refusals = Vec::new();
    let mut snapshots = Vec::new();
    for root in roots {
        let protected = protected_dirs(root);
        let changes = proposals(root, &protected, cwd, tool, input, payload)?;
        if changes.is_empty() {
            snapshots.push(root);
        } else {
            refusals.extend(
                judge_all(root, &protected, &changes)?
                    .into_iter()
                    .map(|refusal| (root, refusal)),
            );
        }
    }
    if refusals.is_empty() {
        if snapshots.is_empty() {
            return Ok(Outcome::Allow);
        }
        let key = other_key(tool, text(payload, &["tool_use_id"]), input);
        for root in snapshots {
            warn_unsupported(root);
            watch::before(
                root,
                text(payload, &["session_id", "conversation_id"]).unwrap_or_default(),
                &key,
                false,
            )?;
        }
        return Ok(Outcome::Allow);
    }

    let askable = refusals.iter().all(|(_, refusal)| !refusal.asks.is_empty());
    let all_reasons = refusals
        .iter()
        .map(|(_, refusal)| refusal.reason.clone())
        .collect::<Vec<_>>()
        .join("\n\n");
    let ask_root = refusals[0].0.clone();
    let asks: Vec<Ask> = refusals
        .into_iter()
        .flat_map(|(_, refusal)| refusal.asks)
        .collect();
    if host == Host::Claude
        && askable
        && text(payload, &["tool_use_id"]).is_some()
        && ask::possible(&ask_root, payload)
    {
        let session = text(payload, &["session_id", "conversation_id"]).unwrap_or_default();
        if !ask::retried(&ask_root, session, &asks)? {
            return Ok(Outcome::Deny(ask::first_refusal(&asks)));
        }
        ask::save(
            &ask_root,
            session,
            text(payload, &["tool_use_id"]).unwrap(),
            &asks,
        )?;
        return Ok(Outcome::Ask(ask::question(&asks)));
    }
    Ok(Outcome::Deny(all_reasons))
}

/// Why lock refuses a change in one file, and the locked changes in it that a person
/// may approve. A refusal that a person cannot approve has no asks.
struct Refusal {
    reason: String,
    asks: Vec<Ask>,
}

impl Refusal {
    fn hard(reason: String) -> Option<Self> {
        Some(Self {
            reason,
            asks: Vec::new(),
        })
    }
}

fn reasons(refusals: &[Refusal]) -> String {
    refusals
        .iter()
        .map(|refusal| refusal.reason.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Warns when `LOCKED:` appears in a file type that cannot hold markers, so a person
/// does not believe a region is locked when it is not.
fn warn_unsupported(root: &Path) {
    match crate::repo::unsupported_markers(root) {
        Ok(paths) if !paths.is_empty() => eprintln!(
            "lock: warning: LOCKED: in unsupported file type{} (locks nothing): {}. Use a supported comment syntax, or lock the whole file in .locks/files.json.",
            if paths.len() == 1 { "" } else { "s" },
            paths.join(", ")
        ),
        _ => {}
    }
}

/// The key that ties the check before a tool call to the check after it.
fn other_key(tool: &str, call: Option<&str>, input: &Value) -> String {
    match call {
        Some(call) => format!("tool {tool} {call}"),
        None => format!("tool {tool} {input}"),
    }
}

fn shell_key(command: &str, call: Option<&str>) -> String {
    call.map_or_else(|| command.to_string(), |id| format!("{command}\n{id}"))
}

fn judge_all(
    root: &Path,
    protected: &[PathBuf],
    changes: &[Change],
) -> Result<Vec<Refusal>, String> {
    let mut refusals = Vec::new();
    for change in changes {
        refusals.extend(judge(root, protected, change)?);
    }
    Ok(refusals)
}

/// What a tool call would leave in one file.
enum After {
    Content(String),
    Deleted,
    /// The hook cannot reconstruct the result, such as a notebook cell edit or a patch.
    Unknown(&'static str),
}

struct Change {
    path: String,
    before: Option<String>,
    after: After,
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn read(root: &Path, path: &str) -> Option<String> {
    fs::read(root.join(path))
        .ok()
        .map(|bytes| normalized(&crate::regions::decode(&bytes).0))
}

fn canonical_existing(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(found) = fs::canonicalize(current) {
            return Some(
                rest.iter()
                    .rev()
                    .fold(found, |path: PathBuf, part| path.join(part)),
            );
        }
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            return None;
        };
        rest.push(name.to_owned());
        current = parent;
    }
}

/// lock's own state and the directory git runs hooks from. No agent edit to these is
/// legitimate.
fn protected_dirs(root: &Path) -> Vec<PathBuf> {
    let git = git_dir(root);
    let common = common_dir(&git);
    let mut dirs = vec![git.join("lock"), git.join("hooks"), common.join("hooks")];
    // Asking git costs a process on every edit, so it is only asked when some config
    // could move the hooks directory.
    if config_may_set_hooks_path(&git, &common)
        && let Ok(output) = crate::repo::git(root, &["rev-parse", "--git-path", "hooks"])
        && let Ok(path) = String::from_utf8(output)
    {
        let path = PathBuf::from(path.trim());
        dirs.push(if path.is_absolute() {
            path
        } else {
            root.join(path)
        });
    }
    dirs.into_iter()
        .filter_map(|path| canonical_existing(&path))
        .collect()
}

/// The shared git directory of a linked worktree, where its hooks live, or the git
/// directory itself.
fn common_dir(git: &Path) -> PathBuf {
    match fs::read_to_string(git.join("commondir")) {
        Ok(text) => {
            let dir = PathBuf::from(text.trim());
            if dir.is_absolute() {
                dir
            } else {
                git.join(dir)
            }
        }
        Err(_) => git.to_path_buf(),
    }
}

/// Whether any config git reads could set `core.hooksPath`. It errs towards yes: a
/// mention of `hookspath` or any include, an unreadable home, or config passed through
/// the environment all count, and only then is git asked.
fn config_may_set_hooks_path(git: &Path, common: &Path) -> bool {
    if [
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
    {
        return true;
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return true;
    };
    let home = PathBuf::from(home);
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let files = [
        common.join("config"),
        git.join("config.worktree"),
        home.join(".gitconfig"),
        xdg.join("git").join("config"),
        PathBuf::from("/etc/gitconfig"),
        PathBuf::from(r"C:\Program Files\Git\etc\gitconfig"),
    ];
    files.iter().any(|file| {
        fs::read(file).is_ok_and(|bytes| {
            let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            text.contains("hookspath") || text.contains("[include")
        })
    })
}

fn protected_target(root: &Path, protected: &[PathBuf], path: &str) -> bool {
    let full = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    canonical_existing(&full).is_some_and(|full| protected.iter().any(|dir| full.starts_with(dir)))
}

/// Applies one exact replacement the way the edit tools do. An edit that does not
/// match exactly once is reported as unknown, because a tool that matches loosely
/// could still apply it.
fn replace(before: &str, edit: &Value) -> Result<String, &'static str> {
    let old = text(edit, &["old_string", "oldString"])
        .map(normalized)
        .ok_or("the edit has no old text")?;
    let new = text(edit, &["new_string", "newString"])
        .map(normalized)
        .ok_or("the edit has no new text")?;
    let all = edit
        .get("replace_all")
        .or_else(|| edit.get("replaceAll"))
        .and_then(Value::as_bool)
        == Some(true);
    let count = if old.is_empty() {
        0
    } else {
        before.matches(old.as_str()).count()
    };
    if count == 0 {
        return Err(
            "the old text is not in the file exactly; re-read the file and retry with exact text",
        );
    }
    if count > 1 && !all {
        return Err("the old text appears more than once; include more context");
    }
    Ok(if all {
        before.replace(old.as_str(), &new)
    } else {
        before.replacen(old.as_str(), &new, 1)
    })
}

fn patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
                "--- ",
                "+++ ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|path| {
            path.trim()
                .trim_start_matches("a/")
                .trim_start_matches("b/")
                .to_string()
        })
        .filter(|path| path != "/dev/null")
        .collect()
}

fn looks_like_patch(text: &str) -> bool {
    text.starts_with("*** Begin Patch")
        || text.starts_with("diff --git ")
        || (text.contains("\n+++ ") && text.starts_with("--- "))
}

/// Every file a patch touches. A Codex patch that applies exactly is replayed; any
/// other patch leaves its files unknown, which blocks it only on locked files.
fn patch_changes(root: &Path, cwd: &Path, patch: &str) -> Vec<Change> {
    let replayed = patch::replay(patch, |raw| {
        relative(root, cwd, raw).and_then(|path| read(root, &path))
    });
    match replayed {
        Some(writes) => writes
            .into_iter()
            .filter_map(|(raw, after)| {
                let path = relative(root, cwd, &raw)?;
                Some(Change {
                    before: read(root, &path),
                    path,
                    after: after.map_or(After::Deleted, After::Content),
                })
            })
            .collect(),
        None => patch_paths(patch)
            .into_iter()
            .filter_map(|raw| relative(root, cwd, &raw))
            .map(|path| Change {
                before: read(root, &path),
                path,
                after: After::Unknown("the patch does not apply exactly"),
            })
            .collect(),
    }
}

fn proposals(
    root: &Path,
    protected: &[PathBuf],
    cwd: &Path,
    tool: &str,
    input: &Value,
    payload: &Value,
) -> Result<Vec<Change>, String> {
    let patch = input
        .as_str()
        .or_else(|| {
            text(
                input,
                &["patch", "patchText", "diff", "input", "raw", "command"],
            )
        })
        .filter(|body| looks_like_patch(body));
    if let Some(patch) = patch {
        return Ok(patch_changes(root, cwd, patch));
    }
    let keys = [
        "file_path",
        "filePath",
        "path",
        "target_file",
        "targetFile",
        "notebook_path",
    ];
    let raw = text(input, &keys).or_else(|| text(payload, &keys));
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    // A file outside the repository matters only if it is an agent's own settings.
    let path = match relative(root, cwd, raw) {
        Some(path) => path,
        None => {
            let full = cwd.join(raw);
            if guard::external_kind(&full).is_none()
                && !protected_target(root, protected, &full.to_string_lossy())
            {
                return Ok(Vec::new());
            }
            full.to_string_lossy().into_owned()
        }
    };
    let before = read(root, &path);
    let after = if tool.contains("delete") || tool.contains("remove") {
        After::Deleted
    } else if tool.contains("notebook") || tool.contains("move") || tool.contains("rename") {
        After::Unknown("this tool is not replayed")
    } else if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        let mut next = before.clone().unwrap_or_default();
        let mut outcome = None;
        for edit in edits {
            match replace(&next, edit) {
                Ok(text) => next = text,
                Err(reason) => {
                    outcome = Some(After::Unknown(reason));
                    break;
                }
            }
        }
        outcome.unwrap_or(After::Content(next))
    } else if input.get("old_string").is_some() || input.get("oldString").is_some() {
        match replace(before.as_deref().unwrap_or_default(), input) {
            Ok(text) => After::Content(text),
            Err(reason) => After::Unknown(reason),
        }
    } else if let Some(content) = text(input, &["content", "contents"]) {
        After::Content(normalized(content))
    } else {
        After::Unknown("the tool input does not say what the file will contain")
    };
    Ok(vec![Change {
        path,
        before,
        after,
    }])
}

#[cfg(test)]
mod tests {
    #[test]
    fn watchdog_denies_once_and_never_after_the_hook_answered() {
        // The deadline must stay below the hosts' default hook timeouts, so the deny
        // arrives before a host gives up and runs the tool call.
        assert!(super::WATCHDOG_SECONDS < 60);
        super::ANSWERED.store(false, std::sync::atomic::Ordering::SeqCst);
        let answer = super::watchdog_answer(super::Host::Claude).expect("first answer");
        assert_eq!(answer["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(super::watchdog_answer(super::Host::Claude).is_none());
        assert!(!super::claim_answer());
        super::ANSWERED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn approve_hint(id: &str, path: &str) -> String {
    let target = if id == WHOLE_FILE { path } else { id };
    format!(
        "Nothing was written. If this change is wanted, ask the user to run `lock approve {target}` in their own terminal, then retry the same edit. Do not work around the lock."
    )
}

fn judge(root: &Path, protected: &[PathBuf], change: &Change) -> Result<Option<Refusal>, String> {
    let path = change.path.as_str();
    let before = change.before.as_deref();
    let after = match &change.after {
        After::Content(text) => Some(text.as_str()),
        _ => None,
    };

    if protected_target(root, protected, path) {
        return Ok(Refusal::hard(format!(
            "lock: {path} is part of lock's enforcement surface. Agents cannot edit it. Ask the user."
        )));
    }

    // An agent's settings can sit inside the repository too, such as in a repository
    // of dotfiles, so the full path decides.
    let settings = guard::external_kind(&root.join(path))
        .or_else(|| eq(path, ".git/config").then_some(guard::Kind::Git));
    if let Some(kind) = settings {
        let unknown = matches!(change.after, After::Unknown(_));
        if unknown || guard::weakens(kind, before, after) {
            return Ok(Refusal::hard(format!(
                "lock: this edit to {path} would turn hooks off, trust fewer hooks, or move the git hooks. Agents cannot do that. Ask the user."
            )));
        }
        return Ok(None);
    }

    if under(path, ".locks") {
        if eq(path, FILE_LOCKS)
            && let Some(after) = after
        {
            let old = parse_file_locks(before.map(str::as_bytes))?;
            let new = parse_file_locks(Some(after.as_bytes()))?;
            let dropped: Vec<&String> = old.iter().filter(|item| !new.contains(item)).collect();
            if dropped.is_empty() {
                return Ok(None);
            }
            return Ok(Refusal::hard(format!(
                "lock: this edit removes {} from {FILE_LOCKS}. Agents may add whole-file locks but not remove them. Ask the user to make this change.",
                dropped
                    .iter()
                    .map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if under(path, APPROVALS) {
            return Ok(Refusal::hard(format!(
                "lock: {path} is an approval record. Only a person writes approvals, with `lock approve` or in the permission prompt."
            )));
        }
        return Ok(Refusal::hard(format!(
            "lock: {path} is lock configuration. Ask the user to change it."
        )));
    }

    if HOOK_CONFIGS.iter().any(|config| eq(path, config)) {
        let safe = eq(path, ".claude/settings.json")
            && after
                .is_some_and(|after| guard::safe_change(guard::Kind::Claude, before, Some(after)));
        if !safe {
            return Ok(Refusal::hard(format!(
                "lock: this edit to {path} would remove the lock hook, turn hooks off, or add a hook that answers permission prompts. Agents cannot do that. Ask the user."
            )));
        }
        return Ok(None);
    }

    let whole_file = parse_file_locks(fs::read(root.join(FILE_LOCKS)).ok().as_deref())?
        .iter()
        .any(|locked| eq(locked, path));
    let marked = supports_markers(path) && before.is_some_and(|text| text.contains("LOCKED:"));
    if !whole_file && !marked {
        return Ok(None);
    }
    if let After::Unknown(why) = &change.after {
        return Ok(Refusal::hard(format!(
            "lock: {path} holds a lock, and this tool call cannot be checked against it ({why}). Use an exact Edit or Write instead. {}",
            approve_hint(if whole_file { WHOLE_FILE } else { "<id>" }, path)
        )));
    }

    let records = disk_approvals(root, Some(path))?;
    let mut blocked: Vec<Ask> = Vec::new();
    if whole_file {
        let from = before.map(|text| hash(text.as_bytes()));
        let to = after.map(|text| hash(text.as_bytes()));
        if from != to && !approved(&records, path, WHOLE_FILE, from.as_deref(), to.as_deref()) {
            pending::save(
                root,
                path,
                WHOLE_FILE,
                (from.clone(), before.unwrap_or_default()),
                (to.clone(), after.unwrap_or_default()),
            );
            blocked.push(Ask {
                path: path.to_string(),
                id: WHOLE_FILE.to_string(),
                before: from,
                after: to,
            });
        }
    }
    if marked {
        let old = regions(path, before.unwrap_or_default()).map_err(|error| {
            format!("{error}. Fix the markers by hand before editing this file")
        })?;
        let new = match after {
            Some(text) => match regions(path, text) {
                Ok(found) => found,
                Err(error) => {
                    return Ok(Refusal::hard(format!(
                        "lock: this edit would break a lock marker ({error}). Nothing was written."
                    )));
                }
            },
            None => Vec::new(),
        };
        for region in old {
            let next = new.iter().find(|candidate| candidate.id == region.id);
            let to = next.map(|candidate| candidate.hash.clone());
            if to.as_deref() == Some(region.hash.as_str())
                || approved(
                    &records,
                    path,
                    &region.id,
                    Some(&region.hash),
                    to.as_deref(),
                )
            {
                continue;
            }
            pending::save(
                root,
                path,
                &region.id,
                (Some(region.hash.clone()), &region.text),
                (
                    to.clone(),
                    next.map(|candidate| candidate.text.as_str())
                        .unwrap_or_default(),
                ),
            );
            blocked.push(Ask {
                path: path.to_string(),
                id: region.id,
                before: Some(region.hash),
                after: to,
            });
        }
    }
    if blocked.is_empty() {
        return Ok(None);
    }
    let what = if blocked.len() == 1 && blocked[0].id == WHOLE_FILE {
        format!("{path} is locked as a whole file, and this tool call would change it.")
    } else {
        format!(
            "{path} has locked regions that this tool call would change or remove: {}.",
            blocked
                .iter()
                .map(|ask| format!("`{}`", ask.id))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(Some(Refusal {
        reason: format!("lock: {what} {}", approve_hint(&blocked[0].id, path)),
        asks: blocked,
    }))
}
