use crate::repo::git_dir;
use std::fs;
use std::path::Path;

/// Reads HEAD and resolves it without starting a git process.
pub fn head(root: &Path) -> Option<String> {
    let dir = git_dir(root);
    let contents = fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = contents.trim();
    if let Some(id) = valid_id(head) {
        return Some(id);
    }
    let reference = head.strip_prefix("ref: ")?;

    if let Some(id) = fs::read_to_string(dir.join(reference))
        .ok()
        .and_then(|text| valid_id(text.trim()))
    {
        return Some(id);
    }

    let packed = fs::read_to_string(dir.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let id = fields.next()?;
        let name = fields.next()?;
        (name == reference).then(|| valid_id(id)).flatten()
    })
}

fn valid_id(id: &str) -> Option<String> {
    (id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| id.to_string())
}
