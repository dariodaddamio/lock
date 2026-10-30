//! Turning the paths that agents send into repository-relative paths.

use std::path::{Path, PathBuf};

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

pub(crate) fn normalize(path: &str) -> String {
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
    if cfg!(any(windows, target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

pub fn under(path: &str, dir: &str) -> bool {
    same(path, dir)
        || path.len() > dir.len() && {
            path.get(..dir.len()).is_some_and(|head| same(head, dir))
                && path.as_bytes().get(dir.len()) == Some(&b'/')
        }
}

/// The path relative to `root`, or `None` when it points outside the repository.
/// Relative paths resolve against `cwd`.
pub fn relative(root: &Path, cwd: &Path, raw: &str) -> Option<String> {
    let root_path = root.to_path_buf();
    let root = normalize(&slash(&root_path.to_string_lossy()));
    let raw = windows_drive(&slash(raw.trim()), &root);
    let full = if is_absolute(&raw) {
        normalize(&raw)
    } else {
        normalize(&format!("{}/{raw}", slash(&cwd.to_string_lossy())))
    };
    let textual = || {
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
    };

    let target = if is_absolute(&raw) {
        PathBuf::from(&raw)
    } else {
        cwd.join(&raw)
    };
    let canonical_root = std::fs::canonicalize(&root_path);
    // A new file can sit in folders that do not exist yet, so resolve the nearest
    // folder that does and add the rest back. A symlink anywhere on the way counts.
    let canonical_target = {
        let mut existing = target.as_path();
        let mut rest = Vec::new();
        loop {
            if let Ok(found) = std::fs::canonicalize(existing) {
                break Ok(rest
                    .iter()
                    .rev()
                    .fold(found, |path: PathBuf, part| path.join(part)));
            }
            match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_owned());
                    existing = parent;
                }
                _ => break Err(()),
            }
        }
    };
    let (Ok(canonical_root), Ok(canonical_target)) = (canonical_root, canonical_target) else {
        return textual();
    };
    let canonical_root = normalize(
        slash(&canonical_root.to_string_lossy())
            .strip_prefix("//?/")
            .unwrap_or(&slash(&canonical_root.to_string_lossy())),
    );
    let canonical_target = normalize(
        slash(&canonical_target.to_string_lossy())
            .strip_prefix("//?/")
            .unwrap_or(&slash(&canonical_target.to_string_lossy())),
    );
    if same(&canonical_root, &canonical_target) {
        Some(String::new())
    } else {
        let prefix = format!("{canonical_root}/");
        match (
            canonical_target.get(..prefix.len()),
            canonical_target.get(prefix.len()..),
        ) {
            (Some(head), Some(rest)) if !rest.is_empty() && same(head, &prefix) => {
                Some(rest.to_string())
            }
            _ => None,
        }
    }
}

/// Case-insensitive on Windows, where the file system is.
pub fn eq(a: &str, b: &str) -> bool {
    same(a, b)
}
