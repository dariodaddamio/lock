# Changelog

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
