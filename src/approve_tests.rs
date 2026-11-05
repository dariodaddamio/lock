use super::*;
use std::io::Cursor;
use std::path::PathBuf;

fn item(path: &str, id: &str, pending: bool, after: Option<&str>) -> Candidate {
    Candidate {
        path: path.into(),
        id: id.into(),
        before: Some("b".into()),
        after: after.map(str::to_string),
        before_text: "old".into(),
        after_text: "new".into(),
        pending: pending.then(|| Pending {
            file: PathBuf::from("p.json"),
            path: path.into(),
            id: id.into(),
            before: None,
            after: None,
            before_text: String::new(),
            after_text: String::new(),
        }),
    }
}

struct Fake {
    saved: std::cell::RefCell<Vec<String>>,
}

impl Review for Fake {
    fn diff(&self, _: &Candidate) -> Diff {
        Diff {
            added: 2,
            removed: 1,
            shown: "@@ diff @@\n".into(),
        }
    }
    fn approve(&self, change: &Candidate) -> Result<String, String> {
        self.saved.borrow_mut().push(change.path.clone());
        Ok(format!(".locks/approvals/{}.json", change.path))
    }
}

fn go(items: &[Candidate], typed: &str) -> (Summary, String, Vec<String>) {
    let fake = Fake {
        saved: Default::default(),
    };
    let mut out = Vec::new();
    let summary = review(items, &mut Cursor::new(typed.to_string()), &mut out, &fake).unwrap();
    let saved = fake.saved.borrow().clone();
    (summary, String::from_utf8(out).unwrap(), saved)
}

fn three() -> Vec<Candidate> {
    vec![
        item("a.ts", "x", false, Some("h")),
        item("b.ts", "y", false, Some("h")),
        item("c.ts", "z", false, Some("h")),
    ]
}

#[test]
fn answers_parse_without_regard_to_case_or_spaces() {
    for yes in [
        "y",
        "Y",
        "yes",
        "YES",
        "approve",
        "  APPROVE 
",
        "Approve",
    ] {
        assert_eq!(parse_answer(yes), Answer::Yes, "{yes:?}");
    }
    for skip in [
        "n", "N", "no", "", "
",
    ] {
        assert_eq!(parse_answer(skip), Answer::Skip, "{skip:?}");
    }
    assert_eq!(parse_answer("a"), Answer::All);
    assert_eq!(parse_answer("A"), Answer::All);
    assert_eq!(parse_answer("q"), Answer::Quit);
    assert_eq!(parse_answer("Q"), Answer::Quit);
}

#[test]
fn the_id_is_never_part_of_the_answer() {
    assert_eq!(parse_answer("APPROVE file"), Answer::Unknown);
    assert_eq!(parse_answer("approve pricing-copy"), Answer::Unknown);
    assert_eq!(parse_answer("maybe"), Answer::Unknown);
    assert_eq!(parse_answer("1"), Answer::Unknown);
}

#[test]
fn approve_all_needs_a_yes() {
    assert!(confirms_all("y"));
    assert!(confirms_all(" YES "));
    assert!(!confirms_all(""));
    assert!(!confirms_all("n"));
    assert!(!confirms_all("a"));
}

#[test]
fn a_wrong_answer_says_what_to_type_and_asks_again() {
    let items = [item("a.ts", "x", false, Some("h"))];
    let (summary, out, saved) = go(
        &items,
        "APPROVE file
maybe
APPROVE
",
    );
    assert_eq!(out.matches("Not valid.").count(), 2);
    assert_eq!(saved, ["a.ts"]);
    assert_eq!((summary.approved, summary.skipped), (1, 0));
}

#[test]
fn the_last_change_does_not_offer_all() {
    let items = [item("a.ts", "x", false, Some("h"))];
    let (_, out, _) = go(&items, "wat\n\n");
    assert!(
        out.contains("Not valid. Press y, n, or q, then Enter."),
        "{out}"
    );
    assert!(!out.contains("[a]"), "{out}");
}

#[test]
fn a_wrong_answer_mentions_all_while_changes_remain() {
    let (_, out, _) = go(&three(), "wat\n\nq\n");
    assert!(
        out.contains("Not valid. Press y, n, a, or q, then Enter."),
        "{out}"
    );
    assert!(
        out.contains(
            "Approve this change?  [y] yes   [n] skip   [a] approve all remaining   [q] quit"
        ),
        "{out}"
    );
}

#[test]
fn enter_skips_and_the_summary_counts() {
    let items = [
        item("a.ts", "x", false, Some("h")),
        item("b.ts", "y", false, Some("h")),
        item("c.ts", "z", true, None),
    ];
    let (summary, out, saved) = go(&items, "approve\n\nApprove\n");
    assert_eq!(saved, ["a.ts", "c.ts"]);
    assert!(out.contains("Approved 2, skipped 1."), "{out}");
    assert!(out.contains(".locks/approvals/a.ts.json"), "{out}");
    assert!(out.contains("The agent can now retry"), "{out}");
    assert_eq!(summary.files.len(), 2);
}

#[test]
fn quit_skips_the_rest() {
    let (summary, out, saved) = go(&three(), "approve\nq\n");
    assert_eq!(saved, ["a.ts"]);
    assert_eq!((summary.approved, summary.skipped), (1, 2));
    assert!(out.contains("Approved 1, skipped 2."), "{out}");
}

#[test]
fn closed_input_stops_instead_of_looping() {
    let items = [item("a.ts", "x", false, Some("h"))];
    let (summary, _, saved) = go(&items, "");
    assert!(saved.is_empty());
    assert_eq!((summary.approved, summary.skipped), (0, 1));
}

#[test]
fn all_lists_the_count_and_needs_a_second_confirmation() {
    let (summary, out, saved) = go(
        &three(),
        "a
y
",
    );
    assert!(
        out.contains("Approve all 3 remaining changes? [y/n]"),
        "{out}"
    );
    for file in ["a.ts", "b.ts", "c.ts"] {
        assert!(out.contains(&format!("  {file} (")), "{out}");
    }
    assert_eq!(saved, ["a.ts", "b.ts", "c.ts"]);
    assert!(out.contains("Approved 3, skipped 0."), "{out}");
    assert_eq!(summary.approved, 3);
}

#[test]
fn declining_the_confirmation_goes_back_to_this_change() {
    let items = [
        item("a.ts", "x", false, Some("h")),
        item("b.ts", "y", false, Some("h")),
    ];
    let (summary, out, saved) = go(&items, "all\n\n\nq\n");
    assert!(saved.is_empty());
    assert!(out.contains("Cancelled."), "{out}");
    assert_eq!((summary.approved, summary.skipped), (0, 2));
}

#[test]
fn all_counts_only_the_changes_from_here_on() {
    let (summary, _, saved) = go(
        &three(),
        "
a
y
",
    );
    assert_eq!(saved, ["b.ts", "c.ts"]);
    assert_eq!((summary.approved, summary.skipped), (2, 1));
}

#[test]
fn the_header_says_what_where_and_how_much() {
    let whole = header(2, 5, &item("src/p.ts", WHOLE_FILE, true, Some("h")), 3, 1);
    assert!(whole.contains("Change 2 of 5"), "{whole}");
    assert!(whole.contains("src/p.ts"), "{whole}");
    assert!(whole.contains("whole file"), "{whole}");
    assert!(whole.contains("an agent's blocked edit"), "{whole}");
    assert!(whole.contains("+3 -1"), "{whole}");
    assert!(!whole.contains("removed"), "{whole}");
    let region = header(
        1,
        1,
        &item("src/p.ts", "pricing-copy", false, Some("h")),
        0,
        0,
    );
    assert!(region.contains("region pricing-copy"), "{region}");
    assert!(region.contains("a change in your working tree"), "{region}");
    let gone = header(1, 1, &item("src/p.ts", "pricing-copy", false, None), 0, 4);
    assert!(gone.contains("removed"), "{gone}");
}

#[test]
fn line_stats_ignore_file_headers() {
    let diff = "--- a/x\n+++ b/y\n@@ -1,2 +1,2 @@\n-old\n+new\n+more\n same\n";
    assert_eq!(count_lines(diff), (2, 1));
    assert_eq!(count_lines(""), (0, 0));
}

#[test]
fn color_follows_no_color_and_dumb_terminals() {
    use std::ffi::OsStr;
    assert!(wants_color(None, Some("xterm")));
    assert!(wants_color(None, None));
    assert!(!wants_color(Some(OsStr::new("1")), Some("xterm")));
    assert!(!wants_color(Some(OsStr::new("")), Some("dumb")));
    assert!(!wants_color(None, Some("dumb")));
}

#[test]
fn the_screen_is_plain_ascii() {
    let items = [
        item("a.ts", "x", true, Some("h")),
        item("b.ts", "file", false, None),
    ];
    let (_, out, _) = go(&items, "approve\nq\n");
    assert!(out.is_ascii(), "{out}");
}
