//! The one shell command the hook refuses before it runs: an agent asking `lock approve`
//! to approve its own change. Every other command runs, and `watch` checks what it
//! changed afterwards.

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
    (mentions_lock && words.iter().any(|word| word == "approve")).then(|| {
        "lock: only a person approves a locked change, by running `lock approve` in their own terminal. Ask the user to run it."
            .to_string()
    })
}
