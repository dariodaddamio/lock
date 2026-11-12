---
title: Questions
---

### How do I change locked text myself?

Edit it as usual. Before you commit, run `lock approve` in your terminal: it shows
your change as a diff and records it. Then commit the approval file with your
change. Until you do, the commit hook that `lock install` adds refuses the commit.

### What happens when locked text is moved or renamed?

A move counts as removing the locked text from its old path.

![When an agent moves a locked file, lock puts it back and the commit is refused; when you move it, you approve once and the lock lives at the new path](/lock/move.svg)

| Who moves it | What happens |
| --- | --- |
| An agent, with an edit tool or a patch | The tool call is refused, and nothing changes. |
| An agent, with a shell command such as `git mv` | `lock` puts the file back at its old path, unstages the move, and tells the agent. A copy that the command made at the new path stays. |
| An agent, with a commit, even with `--no-verify` | The commit is refused or undone. |
| You | `git commit` asks you to run `lock approve` once. After that commit, the lock lives at the new path. |

Copying locked text to a new place needs no approval, because a new lock only adds
protection.

### What if the agent commits my edit?

The commit is refused until you approve your edit with `lock approve`. After that,
the agent's commit goes through, with your approval in it.

### Can I add another agent?

Yes, if the agent can run a hook before and after each tool call. [Open an issue](https://github.com/dariodaddamio/lock/issues/new?template=agent.yml)
with an example of the JSON that the agent sends to its hooks.

### Why was my agent able to edit a locked file?

Run `lock doctor`. It checks the setup and edits nothing. The usual causes are these:

- The agent was already running when you ran `lock install`. A session that started
  before the install runs no hooks, so restart the agent. Only the commit check
  stops its edits until then.
- The agent cannot find `lock` on its PATH. Start the agent from a shell where
  `lock --version` works.
- The agent does not trust the folder or the hooks yet. Accept the folder trust
  prompt in Claude Code, review the hooks with `/hooks` in Codex, and reopen the
  project in Cursor.
- The hook files are git-ignored, so this clone has them but a teammate's or a CI
  checkout does not. Add `!.claude/` and `!.claude/settings.json` to `.gitignore`,
  and the same for the other agents, then commit the files.

### How do I remove lock?

Run `lock uninstall` in the repository, then delete the binary. The lock markers
and `.locks/` stay, so `lock install` brings the protection back.
