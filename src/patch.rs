//! Replaying a Codex-style patch (`*** Begin Patch` ... `*** End Patch`) in memory,
//! so the hook can compare locked regions before and after the patch the same way it
//! does for an edit. A patch that does not apply exactly returns `None`, and the hook
//! then treats every file the patch names as unknown.

/// One file the patch writes: its path as written in the patch, and its new content,
/// or `None` when the patch deletes it.
pub type Write = (String, Option<String>);

fn split(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn find(lines: &[String], wanted: &[String], from: usize) -> Option<usize> {
    if wanted.is_empty() {
        return Some(lines.len());
    }
    (from..=lines.len().checked_sub(wanted.len())?)
        .find(|&at| lines[at..at + wanted.len()] == *wanted)
}

/// Applies the patch to files read with `read`, which returns LF-normalized text.
pub fn replay(patch: &str, read: impl Fn(&str) -> Option<String>) -> Option<Vec<Write>> {
    let patch = patch.replace("\r\n", "\n");
    let rows: Vec<&str> = patch.lines().collect();
    if rows.first()?.trim() != "*** Begin Patch" {
        return None;
    }
    let mut writes = Vec::new();
    let mut i = 1;
    loop {
        let row = *rows.get(i)?;
        i += 1;
        if row.trim() == "*** End Patch" {
            return Some(writes);
        }
        if let Some(path) = row.strip_prefix("*** Delete File: ") {
            writes.push((path.trim().to_string(), None));
            continue;
        }
        if let Some(path) = row.strip_prefix("*** Add File: ") {
            let mut body = Vec::new();
            while let Some(line) = rows.get(i).filter(|line| !line.starts_with("*** ")) {
                body.push(line.strip_prefix('+')?.to_string());
                i += 1;
            }
            writes.push((
                path.trim().to_string(),
                Some(format!("{}\n", body.join("\n"))),
            ));
            continue;
        }
        let path = row.strip_prefix("*** Update File: ")?.trim().to_string();
        let destination = rows
            .get(i)
            .and_then(|line| line.strip_prefix("*** Move to: "))
            .map(|to| to.trim().to_string());
        if destination.is_some() {
            i += 1;
        }
        let mut lines = split(&read(&path)?);
        let mut cursor = 0;
        let mut hunks = 0;
        while let Some(line) = rows.get(i) {
            if *line == "*** End of File" {
                i += 1;
                continue;
            }
            if line.starts_with("*** ") {
                break;
            }
            if let Some(anchor) = line.strip_prefix("@@") {
                i += 1;
                let anchor = anchor.trim();
                if !anchor.is_empty() {
                    cursor = (cursor..lines.len()).find(|&at| lines[at].trim() == anchor)? + 1;
                }
                continue;
            }
            let (mut old, mut new) = (Vec::new(), Vec::new());
            while let Some(line) = rows
                .get(i)
                .filter(|line| !line.starts_with("@@") && !line.starts_with("*** "))
            {
                let mut chars = line.chars();
                let kind = chars.next().unwrap_or(' ');
                let text = chars.as_str();
                match kind {
                    ' ' => {
                        old.push(text.to_string());
                        new.push(text.to_string());
                    }
                    '-' => old.push(text.to_string()),
                    '+' => new.push(text.to_string()),
                    _ => return None,
                }
                i += 1;
            }
            let at = find(&lines, &old, cursor)?;
            let next_cursor = at + new.len();
            lines.splice(at..at + old.len(), new);
            cursor = next_cursor;
            hunks += 1;
        }
        if hunks == 0 {
            return None;
        }
        let content = Some(format!("{}\n", lines.join("\n")));
        match destination {
            Some(to) => {
                writes.push((path, None));
                writes.push((to, content));
            }
            None => writes.push((path, content)),
        }
    }
}
