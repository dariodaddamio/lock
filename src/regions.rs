//! Finding locked regions in one file.
//!
//! A region is a pair of marker lines, `LOCKED: <id> START` and `LOCKED: <id> END`,
//! each written as a comment in the syntax of the file. The marker must be the first
//! thing on its line. Markers inside fenced code blocks in Markdown are examples and
//! do not create a lock. A malformed pair is an error, so a broken lock fails closed.

use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Region {
    pub id: String,
    pub text: String,
    pub hash: String,
}

/// The comment openers a file type accepts for markers.
fn openers(path: &str) -> &'static [&'static str] {
    let ext = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "css" | "scss" | "less"
        | "rs" | "go" | "java" | "kt" | "kts" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp"
        | "cs" | "php" | "scala" | "dart" | "groovy" | "zig" => &["{/*", "/*", "//"],
        "vue" | "svelte" | "astro" => &["<!--", "{/*", "/*", "//"],
        "py" | "rb" | "sh" | "bash" | "zsh" | "ps1" | "psm1" | "yaml" | "yml" | "toml" | "r"
        | "pl" | "ex" | "exs" | "nix" | "tf" | "hcl" => &["#"],
        "sql" | "lua" | "hs" | "elm" => &["--"],
        "md" | "mdx" | "markdown" | "html" | "htm" | "svg" | "xml" => &["<!--"],
        _ => &[],
    }
}

/// Whether a path can hold LOCKED markers at all.
pub fn supports_markers(path: &str) -> bool {
    !openers(path).is_empty()
}

fn is_markup(path: &str) -> bool {
    openers(path) == ["<!--"]
}

/// Parses one marker line into `(id, is_start)`.
fn marker(line: &str, openers: &[&str]) -> Option<(String, bool)> {
    let line = line.trim_start();
    let rest = openers
        .iter()
        .find_map(|opener| line.strip_prefix(opener))?;
    let rest = rest.trim_start().strip_prefix("LOCKED:")?.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let (id, rest) = rest.split_at(end);
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let edge = |word: &str| {
        rest.strip_prefix(word)
            .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    };
    if edge("START") {
        Some((id.to_string(), true))
    } else if edge("END") {
        Some((id.to_string(), false))
    } else {
        None
    }
}

/// SHA-256 of the text with line endings normalized, so a checkout with CRLF and one
/// with LF agree.
pub fn hash(bytes: &[u8]) -> String {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
            continue;
        }
        normalized.push(bytes[index]);
        index += 1;
    }
    let digest = Sha256::digest(&normalized);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every region in one file, in order.
pub fn regions(path: &str, source: &str) -> Result<Vec<Region>, String> {
    let openers = openers(path);
    if openers.is_empty() || !source.contains("LOCKED:") {
        return Ok(Vec::new());
    }
    let markup = is_markup(path);
    let source = source.replace("\r\n", "\n");
    let rows: Vec<&str> = source.split('\n').collect();
    let mut found: Vec<Region> = Vec::new();
    let mut open: Option<(String, usize)> = None;
    let mut fenced = false;
    for (index, row) in rows.iter().enumerate() {
        if markup {
            let trimmed = row.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
            }
            if fenced {
                continue;
            }
        }
        let Some((id, start)) = marker(row, openers) else {
            continue;
        };
        if start {
            if let Some((outer, _)) = &open {
                return Err(format!("{path}: LOCKED {id} starts inside {outer}"));
            }
            if found.iter().any(|region| region.id == id) {
                return Err(format!("{path}: LOCKED {id} appears twice"));
            }
            open = Some((id, index));
            continue;
        }
        match open.take() {
            Some((open_id, first)) if open_id == id => {
                let text = rows[first..=index].join("\n");
                let hash = hash(text.as_bytes());
                found.push(Region { id, text, hash });
            }
            _ => return Err(format!("{path}: LOCKED {id} END has no matching START")),
        }
    }
    if let Some((id, _)) = open {
        return Err(format!("{path}: LOCKED {id} has no END"));
    }
    Ok(found)
}
