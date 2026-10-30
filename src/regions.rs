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
    let digest = if bytes.contains(&0) {
        Sha256::digest(bytes)
    } else {
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
        Sha256::digest(&normalized)
    };
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every region in one file, in order.
/// File bytes as text without losing any byte. Valid UTF-8 is read as UTF-8. Anything
/// else is read one byte per character, so two different byte sequences never look
/// the same, and `encode` turns the text back into the same bytes. Returns whether
/// the bytes were read one per character.
pub fn decode(bytes: &[u8]) -> (String, bool) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_string(), false),
        Err(_) => (bytes.iter().map(|&byte| char::from(byte)).collect(), true),
    }
}

/// The bytes of text that `decode` produced.
pub fn encode(text: &str, bytewise: bool) -> Vec<u8> {
    if bytewise {
        text.chars().map(|c| c as u32 as u8).collect()
    } else {
        text.as_bytes().to_vec()
    }
}

/// Each region as its id and its first and last row, with rows split on `\n`.
fn spans(path: &str, rows: &[&str]) -> Result<Vec<(String, usize, usize)>, String> {
    let openers = openers(path);
    let markup = is_markup(path);
    let mut found: Vec<(String, usize, usize)> = Vec::new();
    let mut open: Option<(String, usize)> = None;
    let mut fenced = false;
    for (index, row) in rows.iter().enumerate() {
        let row = row.strip_suffix('\r').unwrap_or(row);
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
            if found.iter().any(|(seen, _, _)| *seen == id) {
                return Err(format!("{path}: LOCKED {id} appears twice"));
            }
            open = Some((id, index));
            continue;
        }
        match open.take() {
            Some((open_id, first)) if open_id == id => found.push((id, first, index)),
            _ => return Err(format!("{path}: LOCKED {id} END has no matching START")),
        }
    }
    if let Some((id, _)) = open {
        return Err(format!("{path}: LOCKED {id} has no END"));
    }
    Ok(found)
}

pub fn regions(path: &str, source: &str) -> Result<Vec<Region>, String> {
    if openers(path).is_empty() || !source.contains("LOCKED:") {
        return Ok(Vec::new());
    }
    let source = source.replace("\r\n", "\n");
    let rows: Vec<&str> = source.split('\n').collect();
    Ok(spans(path, &rows)?
        .into_iter()
        .map(|(id, first, last)| {
            let text = rows[first..=last].join(
                "
",
            );
            let hash = hash(text.as_bytes());
            Region { id, text, hash }
        })
        .collect())
}

/// Copies the regions named in `ids` from `from` into `into`, byte for byte, and
/// leaves every other byte of `into` as it is, line endings included. Returns `None`
/// when that is not possible, such as when a marker is broken, a region is missing,
/// or the two versions are not decoded the same way; the caller then restores the
/// whole file.
pub fn splice(path: &str, from: &[u8], into: &[u8], ids: &[&str]) -> Option<Vec<u8>> {
    let (old, old_bytewise) = decode(from);
    let (new, new_bytewise) = decode(into);
    if old_bytewise != new_bytewise {
        return None;
    }
    let offsets = |text: &str| -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        starts
    };
    let old_rows: Vec<&str> = old.split('\n').collect();
    let new_rows: Vec<&str> = new.split('\n').collect();
    let (old_spans, new_spans) = (spans(path, &old_rows).ok()?, spans(path, &new_rows).ok()?);
    let (old_at, new_at) = (offsets(&old), offsets(&new));
    // The byte range of rows first..=last, without the newline after the last row.
    let range = |at: &[usize], rows: &[&str], first: usize, last: usize| {
        (at[first], at[last] + rows[last].len())
    };
    let mut edits = Vec::new();
    for id in ids {
        let (_, first, last) = old_spans.iter().find(|(seen, _, _)| seen == id)?;
        let (_, to_first, to_last) = new_spans.iter().find(|(seen, _, _)| seen == id)?;
        let (from_start, from_end) = range(&old_at, &old_rows, *first, *last);
        let (to_start, to_end) = range(&new_at, &new_rows, *to_first, *to_last);
        edits.push((to_start, to_end, &old[from_start..from_end]));
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut text = new.clone();
    for (start, end, replacement) in edits {
        text.replace_range(start..end, replacement);
    }
    Some(encode(&text, new_bytewise))
}
