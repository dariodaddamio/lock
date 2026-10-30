# Changelog

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
