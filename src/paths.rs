//! Turning the paths that agents send into repository-relative paths.

use std::path::Path;

fn slash(text: &str) -> String {
    text.replace('\\', "/")
}

/// `/c/Users/...` from Git Bash becomes `C:/Users/...` when the root has a drive.
fn windows_drive(path: &str, root: &str) -> String {
    let bytes = path.as_bytes();
    if root.as_bytes().get(1) == Some(&b':')
        && bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b'/'
    {
        return format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &path[2..]);
    }
    path.to_string()
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if path.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
}

fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// The path relative to `root`, or `None` when it points outside the repository.
/// Relative paths resolve against `cwd`.
pub fn relative(root: &Path, cwd: &Path, raw: &str) -> Option<String> {
    let root = normalize(&slash(&root.to_string_lossy()));
    let raw = windows_drive(&slash(raw.trim()), &root);
    let full = if is_absolute(&raw) {
        normalize(&raw)
    } else {
        normalize(&format!("{}/{raw}", slash(&cwd.to_string_lossy())))
    };
    if same(&full, &root) {
        return Some(String::new());
    }
    let prefix = format!("{root}/");
    match (full.get(..prefix.len()), full.get(prefix.len()..)) {
        (Some(head), Some(rest)) if !rest.is_empty() && same(head, &prefix) => {
            Some(rest.to_string())
        }
        _ => None,
    }
}

/// Case-insensitive on Windows, where the file system is.
pub fn eq(a: &str, b: &str) -> bool {
    same(a, b)
}
