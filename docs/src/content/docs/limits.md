---
title: Limits
---

`lock` is built so that no tool call, script, or settings change by an agent can
change locked text without your approval. An agent runs as you, on your machine, so
a few cases remain. Most are caught before the change can be committed or merged;
a few are only caught by CI after the fact. State them plainly so people know what
lock covers and what it does not.

## Caught before commit or merge

- A process that an agent starts and that keeps running after the agent's turn
  ends can still change a locked file. The change stays on disk, but it cannot be
  committed until you approve it, and `lock approve` shows it to you first.
- Such a process could also write an approval record, because a record is a plain
  file. On the main branch, code owner review of `.locks/` makes sure that a person
  accepted each record.
- Codex lets a call through when its hook fails, when `lock` is not installed, or
  when the hooks changed and you have not reviewed them yet. The commit check and
  CI still catch the change.
- An agent without hooks is not checked when it edits. Its change cannot be
  committed without your approval.
- `git commit --no-verify` run by a person, or in a clone where nobody ran
  `lock install`, skips the commit check. CI fails the pull request instead.
- Another agent host, or a worktree where nobody ran `lock install`, is not
  guarded at edit time. Once those hooks exist, the commit check and CI still
  apply.

## Only CI (or review) catches these

- Changes made between tool calls, such as by a leftover process, another hook,
  or a person, become the next baseline. The next PreToolUse snapshot sees the new
  bytes as the starting point. Only the commit check and CI catch them.
- A `git push` inside an agent command cannot be undone on the remote. CI after
  the fact is the only net.
- An agent that fakes a terminal and runs `lock approve` from a detached process
  outside any tracked tool call gets past the interactive approval check. Code
  owner review of `.locks/` on the protected branch is what stops a forged record
  from landing.

## Allowed on purpose

A command that returns locked text to its committed state, such as `git checkout`
or `git stash`, is not undone, so an agent can discard an edit to locked text that
you have not committed yet, as with any other file. While a merge or a rebase
stops on a conflict, `lock` does not undo changes, and the commit hook checks the
result.

Approval in the Claude Code chat trusts Claude Code's edit prompt. A
`PermissionRequest` hook or a permission callback could answer that prompt without
you, so `lock` refuses the edit instead of asking when it finds such a hook in your
Claude Code settings or plugins, and agents cannot add one.

## Why not only a rule in AGENTS.md?

| Approach | What it misses |
| --- | --- |
| A rule in `AGENTS.md` or `CLAUDE.md` | The agent can forget the rule, or decide that it does not apply. |
| Read-only file permissions | They protect whole files only, and an agent can change the permissions back. |
| Code owner review | It finds the change in review, after the agent has made it. |
| `lock` | It stops the change at the tool call, for any part of a file, and checks again at commit and in CI. |

Rules and skills still help for judgment. They do not replace a mechanical gate
when the text must not move.
