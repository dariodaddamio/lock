# Changelog

## Unreleased

- An edit tool can no longer replace the commit hook or write lock's own state.
  Before, an agent could use Write to turn `.git/hooks/pre-commit` into `exit 0`, and
  only shell commands were undone, so the commit check silently stopped running.
  Edit tools are now refused for `.git/hooks`, the resolved `core.hooksPath`, and
  `.git/lock`, including through worktrees, symlinks, and `..`
  (GHSA-c27m-6526-rg4f).
- `lock approve` now shows exactly what it approves. It used to display the text
  stored in a pending record but record a separate hash from the same file, so a
  forged record could show a harmless diff while approving a different change. Each
  record's text is now hashed again on load, and a record whose text and hash
  disagree is dropped (GHSA-52p5-gr35-gwcg).
- An edit sent from inside a nested repository or submodule is now checked against
  the outer repository too. lock used to judge an edit only by the repository of
  the agent's working directory, so an edit to a locked file in the outer
  repository was allowed. It now checks the target file's own repository and every
  repository that encloses it, with the full flow (refusal, in-chat approval, and
  snapshots) in each (GHSA-4wh2-7r58-rg32).
- A hook that hangs now blocks the tool call instead of letting it run. Hosts treat
  a hook that times out as a non-blocking error, so a hung `git` call failed open.
  A 50-second watchdog now answers with a refusal before the host gives up. It is
  armed only before a tool runs, where refusing is safe and nothing has been put
  back yet (GHSA-8fjv-27gv-5754).
- A `.gitattributes` change can no longer hide locked files from the shell check.
  lock found marked files with `git grep -I`, which skips files that git treats as
  binary, so `*.ts -diff` dropped every locked `.ts` file from the snapshot. The
  snapshot now searches without `-I` and takes the files from the worktree, the
  index, and HEAD together (GHSA-vjh5-qxfr-f5mr).
- In-chat approval is turned off in Claude Code's Auto permission mode, because the
  auto-mode classifier may answer the prompt without showing it to you, and lock
  would have recorded that as your approval. In Auto mode lock refuses the edit
  and you approve it with `lock approve` in your terminal (GHSA-g8jm-gmmr-hqcr).
- Shell commands that fail, time out, or are interrupted are now checked. Claude
  Code and Cursor report a failed call as `PostToolUseFailure`, which lock did not
  listen to, and the Stop hook deleted the leftover snapshots without checking
  them. lock now checks failure events, checks every snapshot of the session at
  Stop, and checks an earlier snapshot of the same command before it is replaced.
  Codex has no failure event, so its hook file is unchanged and its failed commands
  are checked at Stop. Run `lock install` again to add the new Claude Code and
  Cursor hook entries (GHSA-m72r-gjm9-c2jc).
- Agent edits to host settings and hook files are now refused by default. lock used
  to block a list of known-bad keys, which could never be complete, so a setting
  such as `env.PATH` could put a fake `lock` in front of the real one. Now only
  changes to `permissions` entries in `.claude/settings.json` are allowed, and
  everything else needs you (GHSA-xcqv-xx5x-95p5).

- Known read-only tools (Read, Grep, Glob, and the other exact names on the safe
  list) pass without a snapshot again. That restores the speed the README describes,
  and stops a parallel Read from putting an in-chat approved edit back before the
  edit's own PostToolUse can record it (#7).
- While an in-chat approval ticket is still open, settle does not put that region
  back, so an unknown or MCP tool that did take a snapshot cannot undo the yes (#7).
- In-chat approval records the disk approval before it drops the open ticket, so a
  parallel settle cannot hit a gap with neither protection (#7).
- Snapshots copy only locked files that differ from the commit recorded at
  PreToolUse; files that still match that commit are read from git instead (#7).
- A UTF-8 byte-order mark before a line-1 marker no longer hides `START` (#8).
- `lock check`, `lock doctor`, and the agent hook warn when `LOCKED:` appears in a
  file type that cannot hold markers, because those markers lock nothing (#8).
- The Limits section documents the cases lock does not stop, including changes
  between tool calls, `git push` from an agent command, a faked terminal for
  `lock approve`, and a host or worktree without hooks (#9).
- Documentation is published as a GitHub Pages site from `docs/`.

- `lock approve` has a clearer screen. Each change starts with a header that names
  the file, the kind of lock, where the change came from, and how many lines it
  touches. You answer one question with `y` (or `yes` or `approve`, in any case), `n` or
  Enter to skip, `a` to approve this change and the ones after it after a second
  question that lists them and names the count, or `q` to quit. You never type the
  lock's id. Any other answer is explained and asked again instead of being skipped. The screen ends with
  how many changes you approved and skipped, and which approval files to commit.
  `lock approve` still runs only for a person at a terminal, and each approval
  still names the exact change.
- `lock approve` respects `NO_COLOR` and `TERM=dumb`.
- `lock install` ends with next steps: restart any agent that was already running,
  because its edits are not guarded until then, trust the hooks in each agent, check
  that `lock` is on the PATH, and run `lock doctor`.
- `lock install` warns when git ignores a hook file, because teammates and CI do not
  get ignored files, and says which `.gitignore` lines share them.
- New command `lock doctor` checks the setup without changing anything: `lock` on the
  PATH, the commit hook, each agent's hook entries, ignored hook files, `.locks`, the
  lock markers, and a canary that replays an edit to a locked file and reports
  whether the hook would deny it. It exits with a non-zero code when a check fails. A missing hook file for an
  agent that is installed on this machine counts as a failure.

## 0.3.2 - October 30, 2026

- An edit to a locked file is as fast as in 0.2 again. Since 0.3.0 it started two
  git processes on every edit; now git runs only when an approval record names the
  file.
- A failure to list the locks stops the check instead of reading as a repository
  without locks.
- File names that git quotes, such as names with quotes or backslashes, are read
  exactly, so locks in those files are enforced.
- Two identical shell commands that run at the same time each get their own copy
  of the locked files.
- A whole-file lock on a binary file compares its bytes exactly, line endings
  included.
- The GitHub Action checks the downloaded binary against `SHA256SUMS`.
- The check after a command reads HEAD from the git directory and runs git only
  when HEAD moved.

## 0.3.1 - October 30, 2026

- When an agent command moves or deletes a locked file, such as with `git mv`,
  `lock` puts the file back and also unstages the move, so `git status` shows no
  staged deletion. The agent is told that the move was undone and that any copy
  it made at a new path is still there.

## 0.3.0 - October 30, 2026

This release closes the gaps that an outside audit found in 0.2.1. Each one has a
scenario in `tests/scenarios.sh`.

- A committed approval record is used up. It cannot approve a change again, so
  records cannot be replayed or chained.
- Paths compare without case on Windows and macOS, and symbolic links are resolved,
  so `.LOCKS/approvals/` or a link to a locked file does not lead around a lock.
- Whole-file lock entries are normalized, so `./catalog.json` locks `catalog.json`.
- Claude Code settings are compared as JSON, so changing `disableAllHooks` from
  `false` to `true` is refused.
- A tool is matched by its full name, so a tool named `x.read` is not taken for a
  read.
- A command that deletes `lock`'s copies, fakes a merge, changes the commit hook,
  or commits locked text while skipping the hook is undone.
- Locked text in a file that is not valid UTF-8 is compared and restored byte for
  byte, and a put-back keeps the rest of the file's bytes and line endings.
- A crash in the hook refuses the tool call instead of letting it through.

## 0.2.1 - October 30, 2026

- On Windows, `lock` now recognizes an agent's settings file when its path is
  written in the short form, such as `RUNNER~1`. Before, an agent could turn hooks
  off in a settings file whose folder was given in that form.

## 0.2.0 - October 30, 2026

This release closes the ways around `lock` that were known after 0.1.1.

- A commit of locked text needs an approval, including for your own edits. The
  commit hook no longer records an edit as yours, because a change that no hook saw
  can also come from an agent without hooks or from a process that an agent left
  running. Run `lock approve`, and then commit.
- The hooks now run for every tool call. A tool that `lock` cannot read, such as an
  MCP tool, is checked by what it changed, like a shell command.
- A command that runs in the background is checked again when the turn ends.
- An agent cannot turn hooks off in its own settings in your home folder, distrust
  the Codex hooks, or point git's `core.hooksPath` away from the commit check.
- An agent cannot change the lock hook entries in any way, because Codex stops
  running a changed hook until a person reviews it.
- `lock approve` warns you when an agent command is running, because an approval
  made then is undone when that command ends.
- Run `lock install` again to update the hooks. In Codex, open `/hooks` and trust
  them again.

## 0.1.1 - October 29, 2026

- In Cursor, `lock` now checks each shell command. Cursor sends an empty working
  directory with shell commands, and `lock` let those commands through without a
  check.
- In Cursor, the agent now gets the message about locked text that `lock` put
  back. `lock install` removes the `afterShellExecution` hook, because only
  `postToolUse` can send a message to the agent.
- In Codex on Windows, the hooks now run. Codex runs hooks in Windows PowerShell,
  so `lock install` now writes the command `lock hook codex`, which PowerShell and
  POSIX shells both run. Run `lock install` again to update the hooks.
- `lock` is on crates.io as `lock-hook`. `cargo install lock-hook` installs the
  `lock` command.

## 0.1.0 - October 29, 2026

This is the first release.

- The agent hook refuses an edit from Claude Code, Codex, or Cursor that changes
  locked text without an approval. Nothing is written.
- In Claude Code, you can approve a locked change in the chat. The agent tries the
  same edit again, Claude Code shows you the change, and your yes is recorded.
- After each agent shell command, `lock` puts back the locked text that the command
  changed. The other changes of the command stay.
- The commit hook records your own edits to locked text, and it stops a commit of an
  unapproved change that an agent made.
- `lock approve` shows each blocked change as a diff and records the changes that
  you accept.
- `lock check --base <ref>` and the GitHub Action check each pull request.
- `lock install` adds the hooks to a clone, and `lock uninstall` removes them.
- Prebuilt binaries are available for macOS, Linux, and Windows, with SHA-256
  checksums.
