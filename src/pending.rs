//! Changes an agent proposed and the hook blocked, kept in the git directory until a
//! person reviews them with `lock approve`. A pending change grants nothing by itself.

use crate::regions::hash;
use crate::repo::git_dir;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Pending {
    pub file: PathBuf,
    pub path: String,
    pub id: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub before_text: String,
    pub after_text: String,
}

fn dir(root: &Path) -> PathBuf {
    git_dir(root).join("lock").join("pending")
}

pub fn save(
    root: &Path,
    path: &str,
    id: &str,
    before: (Option<String>, &str),
    after: (Option<String>, &str),
) {
    let key = hash(format!("{path}\n{id}\n{}", after.0.as_deref().unwrap_or("removed")).as_bytes());
    let dir = dir(root);
    let record = json!({
        "path": path,
        "id": id,
        "before": before.0,
        "after": after.0,
        "beforeText": before.1,
        "afterText": after.1,
    });
    // A failure to save only means the person sees no proposal to review; the block stands.
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(
            dir.join(format!("{}.json", &key[..16])),
            format!("{record:#}\n"),
        );
    }
}

pub fn list(root: &Path) -> Vec<Pending> {
    let Ok(entries) = fs::read_dir(dir(root)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let value: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
            Some(Pending {
                file: entry.path(),
                path: text("path")?,
                id: text("id")?,
                before: text("before"),
                after: text("after"),
                before_text: text("beforeText").unwrap_or_default(),
                after_text: text("afterText").unwrap_or_default(),
            })
        })
        .collect()
}
