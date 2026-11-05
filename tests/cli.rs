//! End-to-end checks for `lock install` and `lock doctor` against throwaway repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_lock");
static COUNT: AtomicUsize = AtomicUsize::new(0);

fn bin_dir() -> PathBuf {
    Path::new(BIN).parent().unwrap().to_path_buf()
}

fn with_path(extra: &Path) -> std::ffi::OsString {
    let mut paths = vec![extra.to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    std::env::join_paths(paths).unwrap()
}

/// The directory that holds git, so a test can run with a PATH that has no `lock`.
fn git_dir() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .find(|dir| dir.join("git").exists() || dir.join("git.exe").exists())
        .expect("git on PATH")
}

fn repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lock-cli-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "tester"],
        &["config", "user.email", "t@example.com"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .status()
                .unwrap()
                .success()
        );
    }
    dir
}

fn lock(dir: &Path, args: &[&str]) -> Output {
    lock_with_path(dir, args, &with_path(&bin_dir()))
}

fn lock_with_path(dir: &Path, args: &[&str], path: &std::ffi::OsStr) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        .env_remove("CI")
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn marked_repo() -> PathBuf {
    let dir = repo();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/app.ts"),
        "export const free = 1;\n/* LOCKED: tagline START */\nexport const tagline = \"Pay for what you ship.\";\n/* LOCKED: tagline END */\n",
    )
    .unwrap();
    dir
}

#[test]
fn install_ends_with_next_steps() {
    let dir = marked_repo();
    let out = text(&lock(&dir, &["install"]));
    assert!(out.contains("Next steps"), "{out}");
    assert!(out.contains("Restart any agent"), "{out}");
    assert!(out.contains("NOT guarded"), "{out}");
    assert!(out.contains("/hooks"), "{out}");
    assert!(out.contains("lock doctor"), "{out}");
}

#[test]
fn install_warns_when_lock_is_not_on_the_path() {
    let dir = marked_repo();
    let out = text(&lock_with_path(&dir, &["install"], git_dir().as_os_str()));
    assert!(out.contains("not on your PATH"), "{out}");
}

#[test]
fn install_warns_when_hook_files_are_git_ignored() {
    let dir = marked_repo();
    fs::write(dir.join(".gitignore"), ".claude/\n").unwrap();
    let out = text(&lock(&dir, &["install"]));
    assert!(out.contains("git-ignored"), "{out}");
    assert!(out.contains(".claude/settings.json"), "{out}");
    assert!(out.contains("teammates"), "{out}");
    assert!(out.contains("!.claude/settings.json"), "{out}");
    assert!(!out.contains(".codex/hooks.json is git-ignored"), "{out}");
}

#[test]
fn install_does_not_warn_when_hook_files_are_shared() {
    let dir = marked_repo();
    let out = text(&lock(&dir, &["install"]));
    assert!(!out.contains("git-ignored"), "{out}");
}

#[test]
fn doctor_passes_in_an_installed_repository() {
    let dir = marked_repo();
    lock(&dir, &["install"]);
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(0), "{out}");
    assert!(out.contains("PASS"), "{out}");
    assert!(!out.contains("FAIL"), "{out}");
    assert!(out.contains("would be denied"), "{out}");
}

#[test]
fn doctor_canary_covers_whole_file_locks() {
    let dir = repo();
    fs::write(dir.join("catalog.json"), "{\n  \"name\": \"catalog\"\n}\n").unwrap();
    fs::create_dir_all(dir.join(".locks")).unwrap();
    fs::write(
        dir.join(".locks/files.json"),
        "{ \"files\": [\"catalog.json\"] }",
    )
    .unwrap();
    lock(&dir, &["install"]);
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(0), "{out}");
    assert!(out.contains("catalog.json"), "{out}");
    assert!(out.contains("would be denied"), "{out}");
}

#[test]
fn doctor_leaves_the_repository_untouched() {
    let dir = marked_repo();
    lock(&dir, &["install"]);
    let status = |dir: &Path| {
        let out = Command::new("git")
            .args(["status", "--porcelain", "-uall"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let before_status = status(&dir);
    let before_file = fs::read(dir.join("src/app.ts")).unwrap();
    lock(&dir, &["doctor"]);
    assert_eq!(status(&dir), before_status);
    assert_eq!(fs::read(dir.join("src/app.ts")).unwrap(), before_file);
    assert!(!dir.join(".git/lock/pending").exists());
}

#[test]
fn doctor_fails_when_nothing_is_installed() {
    let dir = marked_repo();
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL"), "{out}");
    assert!(out.contains("lock install"), "{out}");
}

#[test]
fn doctor_fails_outside_a_repository() {
    let dir = std::env::temp_dir().join(format!("lock-cli-plain-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL"), "{out}");
    assert!(out.contains("git repository"), "{out}");
}

#[test]
fn doctor_fails_when_lock_is_not_on_the_path() {
    let dir = marked_repo();
    lock(&dir, &["install"]);
    let run = lock_with_path(&dir, &["doctor"], git_dir().as_os_str());
    let out = text(&run);
    assert_eq!(run.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL"), "{out}");
}

#[test]
fn doctor_fails_on_a_broken_marker() {
    let dir = marked_repo();
    lock(&dir, &["install"]);
    fs::write(
        dir.join("src/app.ts"),
        "/* LOCKED: tagline START */\nexport const tagline = 1;\n",
    )
    .unwrap();
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL"), "{out}");
}

#[test]
fn doctor_fails_when_an_agent_config_lacks_the_hook() {
    let dir = marked_repo();
    lock(&dir, &["install"]);
    fs::write(dir.join(".claude/settings.json"), "{ \"hooks\": {} }\n").unwrap();
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(1), "{out}");
    assert!(out.contains("FAIL"), "{out}");
    assert!(out.contains(".claude/settings.json"), "{out}");
}

#[test]
fn doctor_warns_about_git_ignored_hook_files_without_failing() {
    let dir = marked_repo();
    fs::write(dir.join(".gitignore"), ".cursor/\n").unwrap();
    lock(&dir, &["install"]);
    let run = lock(&dir, &["doctor"]);
    let out = text(&run);
    assert_eq!(run.status.code(), Some(0), "{out}");
    assert!(out.contains("WARN"), "{out}");
    assert!(out.contains(".cursor/hooks.json"), "{out}");
}
