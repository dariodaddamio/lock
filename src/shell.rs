//! The shell commands the hook refuses before they run: an agent asking `lock approve`
//! to approve its own change, or `lock uninstall` to take the hooks out. Every other
//! command runs, and `watch` checks what it changed afterwards.

pub fn denial(command: &str) -> Option<String> {
    let words: Vec<String> = command
        .split(|c: char| c.is_whitespace() || "\"'`;|&(){}<>=,".contains(c))
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase().replace('\\', "/"))
        .collect();
    let mentions_lock = words.iter().any(|word| {
        let name = word.rsplit('/').next().unwrap_or(word);
        name == "lock" || name == "lock.exe"
    });
    if !mentions_lock {
        return None;
    }
    if words.iter().any(|word| word == "approve") {
        return Some("lock: only a person approves a locked change, by running `lock approve` in their own terminal. Ask the user to run it.".into());
    }
    words.iter().any(|word| word == "uninstall").then(|| {
        "lock: only a person removes the lock hooks. Ask the user to run `lock uninstall` in their own terminal.".into()
    })
}
