---
title: Using lock
---

You make the decisions and your agent does the writing, so in practice you do one
of four things:

| You want to | You do this | What lock does |
| --- | --- | --- |
| Lock some text | Ask your agent, for example "Lock the pricing tagline." | The agent adds the markers. A new lock needs no approval, because it only adds protection. |
| Change locked text yourself | Edit it, run `lock approve` in your terminal, and commit | The approval goes into the same commit. A commit of locked text without an approval is refused. |
| Let the agent change locked text | Ask for the change, and approve it when `lock` asks | Only the exact change that you approved goes through. |
| Remove a lock | Delete the markers and approve that, or approve the agent's edit that deletes them | The lock is gone after the commit. |
| Move or rename locked text | Move it, run `lock approve`, and commit | Moving counts as removing the locked text from the old path. Afterwards the lock lives at the new path. An agent's move is refused or undone. |

## Approve a change

### In the Claude Code chat

When an agent's edit meets a lock, `lock` refuses it and says why in the chat, so
the agent asks you whether you want the change. If you do, the agent tries the same
edit again, and Claude Code shows you the diff in its own edit prompt. Choosing
**Yes** lets the edit through, and `lock` records your approval in
`.locks/approvals/` so that you can commit it with the change.

![In Claude Code, lock refuses the first edit, the agent asks the user, and the retried edit opens Claude Code's edit prompt with the diff](/lock/approve-in-chat.svg)

This works in the Manual and Accept edits permission modes. In Auto or Bypass
permissions mode, Claude Code may answer a hook prompt without showing it to you,
so `lock` refuses the edit instead and you approve it in your terminal.

### In your terminal

Codex and Cursor cannot ask you from a hook, a shell command cannot be checked
before it runs, and your own edits need an approval too, so for these you run
`lock approve` in your own terminal. For each blocked or uncommitted change to locked
text, it shows a header with the file, the kind of lock, where the change came from,
and the number of lines that changed, followed by the diff:

![lock approve shows the change, a short header, and the diff of the locked region; you press y, and the approval is recorded](/lock/approve.svg)

After each diff, `lock approve` asks one question. Press a key and then Enter:

| You press | What happens |
| --- | --- |
| `y` | Records this exact change. `yes` and `approve` work too, in any case, and you never type the lock's id. |
| `n` or Enter | Skips this change. |
| `a` | Offers to approve this change and every change after it. `lock approve` lists them and asks `Approve all N remaining changes? [y/n]` before it records anything. |
| `q` | Stops and skips the rest. |

Any other key is not an approval. `lock approve` says which keys are valid and asks again.
At the end, it prints how many changes you approved and skipped, and which approval
files to commit.

Then tell the agent to retry, and commit the new file in `.locks/approvals/` with
the change. `lock approve` runs only when a person types into the terminal, so
neither an agent nor `! lock approve` inside Claude Code can run it. An agent could
still fake a terminal, so an approval that appears while an agent command is
running is undone when that command ends. `lock approve` warns you when that would
happen, and you approve again after the agent's turn.

### How long an approval lasts

An approval is for one exact change, not for a period of time. The record stores
a fingerprint of the locked text before the change and after it, so a retry that
differs by one character is refused again. After you commit the change, the new
text is the locked text, and the next change needs a new approval. A record that
is committed is used up: it never approves a change again, so an old record
cannot be replayed or chained with another one. An approval that nobody used stays
valid for that one change until you delete its file from `.locks/approvals/`.
