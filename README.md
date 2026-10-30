# lock

[![CI](https://github.com/dariodaddamio/lock/actions/workflows/ci.yml/badge.svg)](https://github.com/dariodaddamio/lock/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dariodaddamio/lock)](https://github.com/dariodaddamio/lock/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`lock` prevents coding agents from being able to change parts of your repo that
must not be changed.

![An agent's edit to a locked tagline is refused, and the file on disk stays unchanged](docs/edit-before-after.svg)

Some parts of a project are settled, such as the copy on a landing page, a
released changelog entry, or the timing of an animation. A coding agent cannot
tell which parts those are, so it improves whatever looks improvable, and a rule in
`AGENTS.md` does not stop it, because an agent can forget a rule in a long session.
With `lock`, you mark the settled parts once, and from then on an agent cannot
change them unless you approve the exact change.

## Quick start

Install `lock` on macOS, on Linux, or in Git Bash on Windows:

```sh
curl -fsSL https://raw.githubusercontent.com/dariodaddamio/lock/main/install.sh | sh
```

Or in PowerShell on Windows:

```powershell
irm https://raw.githubusercontent.com/dariodaddamio/lock/main/install.ps1 | iex
```

Then run this in the root of your repository, and commit the files that it adds:

```sh
lock install
```

That is all the setup. Now tell your agent what to lock, for example "Lock the
pricing tagline." The agent puts two comment lines around the text, and from then
on `lock` refuses every change to that text that you have not approved.

## At a glance

| | |
| --- | --- |
| Agents | Claude Code, Codex, and Cursor |
| Platforms | macOS (Apple silicon and Intel), Linux (x64 and ARM64), and Windows (x64 and ARM64) |
| File types | Most code, style, markup, and config files, as [listed below](#lock-markers). Any other file can be locked as a whole. |
| Requirements | git. There is no runtime, server, account, or background process. |
| Size | One binary of 0.7 to 0.8 MB, which uses about 5 MB of memory while it runs |
| Speed | 1 to 8 ms before an edit and 12 to 112 ms for the commit check, whatever the size of the repository. See [Speed and size](#speed-and-size). |

## What lock is

`lock` is one small command-line program that works through three things in your
repository:

| Part | What it is | Who writes it |
| --- | --- | --- |
| Lock markers | Two comment lines, `LOCKED: <id> START` and `LOCKED: <id> END`, around the text that must not change | Your agent, when you ask it to lock something |
| Hooks | Settings that make each agent run `lock` before each edit, around each shell command, and at the end of each turn, and a git hook that runs `lock` before each commit | `lock install` |
| Approval records | Small JSON files in `.locks/approvals/`, each of which allows one exact change to a locked region | You, when you approve a change or commit your own edit |

`lock install` writes these files, and running it again changes nothing:

| File | What `lock install` adds | Commit it? |
| --- | --- | --- |
| `.claude/settings.json` | The hooks for Claude Code | Yes |
| `.codex/hooks.json` | The hooks for Codex | Yes |
| `.cursor/hooks.json` | The hooks for Cursor | Yes |
| `.git/hooks/pre-commit` | The commit check | No, because git does not share hooks. Each clone runs `lock install` once. |

A teammate who clones the repository gets the agent hooks with it. Until they
install `lock`, those hooks refuse every agent edit and tell the agent where to get
`lock`, so nothing gets through unchecked. In a JavaScript project, a
`"prepare": "lock install"` script in `package.json` sets up each clone on
`npm install`.

## Using lock

You make the decisions and your agent does the writing, so in practice you do one
of four things:

| You want to | You do this | What lock does |
| --- | --- | --- |
| Lock some text | Ask your agent, for example "Lock the pricing tagline." | The agent adds the markers. A new lock needs no approval, because it only adds protection. |
| Change locked text yourself | Edit it in your editor and commit as usual | The commit hook records your edit as an approval. |
| Let the agent change locked text | Ask for the change, and approve it when `lock` asks | Only the exact change that you approved goes through. |
| Remove a lock | Delete the markers yourself, or approve the agent's edit that deletes them | The lock is gone after the commit. |

## Approve a change

### In the Claude Code chat

When an agent's edit meets a lock, `lock` refuses it and says why in the chat, so
the agent asks you whether you want the change. If you do, the agent tries the same
edit again, and Claude Code shows you the diff in its own edit prompt. Choosing
**Yes** lets the edit through, and `lock` records your approval in
`.locks/approvals/` so that you can commit it with the change.

![In Claude Code, lock refuses the first edit, the agent asks the user, and the retried edit opens Claude Code's edit prompt with the diff](docs/approve-in-chat.svg)

This works in the Manual, Accept edits, and Auto permission modes. In Bypass
permissions mode, Claude Code shows no prompt, so `lock` refuses the edit instead
and you approve it in your terminal.

### In your terminal

Codex and Cursor cannot ask you from a hook, and a shell command cannot be checked
before it runs, so for these you run `lock approve` in your own terminal. It shows
each blocked change as a diff and records the ones that you accept:

![lock approve shows the diff of the locked region, the person types APPROVE, and the approval is recorded](docs/approve.svg)

Then tell the agent to retry, and commit the new file in `.locks/approvals/` with
the change. `lock approve` runs only when a person types into the terminal, so
neither an agent nor `! lock approve` inside Claude Code can run it.

### How long an approval lasts

An approval is for one exact change, not for a period of time. The record stores
a fingerprint of the locked text before the change and after it, so a retry that
differs by one character is refused again. After you commit the change, the new
text is the locked text, and the next change needs a new approval. An approval
that nobody used stays valid for that one change until you delete its file from
`.locks/approvals/`.

## How it works

`lock` checks each change to locked text at four points:

| When | What lock checks | What happens to an unapproved change |
| --- | --- | --- |
| Before an agent edit | The edit, applied to the file in memory | The edit is refused, and nothing is written. |
| Around an agent shell command | Each locked file, before and after the command | The locked text is put back, and the agent is told why. |
| Before a commit | Each locked change in the commit | Your own edit is recorded as an approval. A change made by an agent command stops the commit. |
| In CI | Each locked change since the base branch | The check fails. |

An agent's edit tool sends the hook the exact text that it wants to replace, so
`lock` can apply the edit to a copy of the file in memory and compare each locked
region before and after. An edit to the same file outside the locked region goes
through as usual.

<details>
<summary>The tool call and the answer from <code>lock</code></summary>

The agent sends:

```json
{
  "tool_name": "Edit",
  "tool_input": {
    "file_path": "src/pricing.ts",
    "old_string": "export const tagline = \"Pay for what you ship.\";",
    "new_string": "export const tagline = \"Ship faster.\";"
  }
}
```

`lock` answers:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "lock: src/pricing.ts :: pricing-copy is locked, so this edit was refused and nothing was written. ..."
  }
}
```

</details>

A script can write anything, so `lock` checks its result instead. Before a shell
command runs, `lock` copies each file that holds a lock, and after the command it
puts back any locked text that changed without approval. All other changes stay.
The same check undoes a command that writes an approval record, edits the lock
configuration, or takes the hook out of the agent settings.

![A script changes an unlocked line and a locked line, and lock keeps the first change and puts back the second](docs/script-before-after.svg)

The commit hook tells your edits from an agent's edits by when they happen. While
an agent command runs, `lock` holds a copy of each locked file, so a change that is
already in that copy was made before the command started and is yours. When the
agent's turn ends, `lock` removes the copies. If `lock` is not installed or cannot
check an edit, the hook refuses the edit instead of letting it through.

## Speed and size

Each hook is a short process that starts, checks, and exits. These are the medians
of 40 runs on GitHub's hosted runners, in a generated repository with 10,000 files
and 300 locks. The [CI benchmark](.github/workflows/ci.yml) measures them again on
each push to the main branch, and it also measures repositories with 10 and 1,000
files.

| Step | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Before an edit, inside or outside a locked region | 1 ms | 5 to 8 ms | 4 ms |
| Before a shell command | 80 ms | 222 ms | 177 ms |
| After a shell command | 10 ms | 39 ms | 64 ms |
| The commit check | 12 ms | 77 ms | 112 ms |

An edit is checked in memory, so its time does not grow with the repository. The
check before a shell command copies each file that holds a lock, so its time grows
with the number of locked files: with 30 locks it takes 12 ms on Linux, 39 ms on
macOS, and 57 ms on Windows. The commit check reads only the files that the commit
changes.

The binary is 0.7 to 0.8 MB, and a process uses about 5 MB of memory. An approval
record is about 350 bytes. While an agent command runs, `lock` keeps a copy of
each locked file in `.git/lock/`, and it removes the copies when the agent's turn
ends. To measure your own machine, run `python3 tests/bench.py <path to lock>`.

## Lock markers

Your agent writes the markers, so you do not need to learn them. This section is
for reference.

```ts
/* LOCKED: pricing-copy START */
export const tagline = "Pay for what you ship.";
/* LOCKED: pricing-copy END */
```

```md
<!-- LOCKED: release-history START -->
## 1.0.0 - October 30, 2026
<!-- LOCKED: release-history END -->
```

| File types | The markers are written as |
| --- | --- |
| JavaScript, TypeScript, JSX, TSX, CSS, SCSS, Less | `/* */`, `//`, or `{/* */}` in JSX |
| Rust, Go, Java, Kotlin, Swift, C, C++, C#, PHP, Scala, Dart, Zig | `/* */` or `//` |
| Markdown, MDX, HTML, SVG, XML | `<!-- -->` |
| Vue, Svelte, Astro | Any of the above |
| Python, Ruby, shell, PowerShell, YAML, TOML, Terraform, Nix | `#` |
| SQL, Lua, Haskell, Elm | `--` |
| Any other file, such as JSON or an image | A whole-file lock in `.locks/files.json` |

- Each marker is a comment at the start of its own line, and the id uses lowercase
  letters, digits, and hyphens and is unique in its file.
- Text after `START` or `END` is a note for people, and `lock` ignores it.
- A region cannot be inside another region.
- In Markdown, a marker inside a fenced code block is an example, so it does not
  lock anything.
- A broken marker, such as a `START` without an `END`, blocks every agent edit to
  that file until someone repairs it.

To lock a file that cannot hold a comment, add its path to `.locks/files.json`:

```json
{ "files": ["public/catalog.json"] }
```

## Set up a team repository

For a repository that other people or a CI system use, add two things.

First, run the check on each pull request and on each push to the main branch:

```yaml
name: Locks
on:
  pull_request:
  push:
    branches: [main]
jobs:
  locks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: dariodaddamio/lock@v0.1.0
```

Second, make sure that a person reviews each approval before it reaches the main
branch. In the branch protection rule for the main branch, turn on **Require
review from Code Owners**, and add these lines to `.github/CODEOWNERS`:

```text
/.locks/                 @your-handle
/.claude/settings.json   @your-handle
/.codex/hooks.json       @your-handle
/.cursor/hooks.json      @your-handle
/.github/workflows/      @your-handle
```

## Commands

| Command | What it does |
| --- | --- |
| `lock install [--git] [--claude] [--codex] [--cursor]` | Adds the hooks to this clone. With no options, it adds all of them. |
| `lock approve [<id> or <path>...] [--reason <text>]` | Shows each blocked or unrecorded change and records the ones that you accept. |
| `lock list` | Shows each lock in the working tree. |
| `lock check` | Compares the working tree with `HEAD`. |
| `lock check --staged --record` | Checks the staged changes and records your own edits. The commit hook runs this. |
| `lock check --base <ref>` | Compares `HEAD` with the point where it branched from `<ref>`. CI runs this. |
| `lock uninstall` | Removes the hooks that `lock install` added. The markers and `.locks/` stay. |
| `lock hook claude`, `lock hook codex`, `lock hook cursor` | The agent hook. The agent runs it and sends one tool call on stdin. |

## Limits

`lock` knows which changes an agent made because the agent's hooks report each
edit and command. A change that no hook reports looks like your own edit, so the
commit hook records it. That happens when an agent without hooks edits a file, or
when a process that an agent started in the background writes to a locked file
after the command has ended.

Two more cases are allowed on purpose. A command that returns locked text to its
committed state, such as `git checkout` or `git stash`, is not undone, so an agent
can discard an edit to locked text that you have not committed yet, as with any
other file. While a merge or a rebase stops on a conflict, `lock` does not undo
changes, and the commit hook checks the result.

Approval in the Claude Code chat trusts Claude Code's edit prompt. A
`PermissionRequest` hook or a permission callback could answer that prompt without
you, so `lock` refuses the edit instead of asking when it finds such a hook in your
Claude Code settings or plugins, and agents cannot add one.

An approval record is a plain file. The hooks stop agents from writing one, and a
person can still write one by hand, so on the main branch code owner review of
`.locks/` makes sure that a person accepted each record.

## Questions

### Why not a rule in `AGENTS.md`?

| Approach | What it misses |
| --- | --- |
| A rule in `AGENTS.md` or `CLAUDE.md` | The agent can forget the rule, or decide that it does not apply. |
| Read-only file permissions | They protect whole files only, and an agent can change the permissions back. |
| Code owner review | It finds the change in review, after the agent has made it. |
| `lock` | It stops the change at the tool call, for any part of a file, and checks again at commit and in CI. |

### What if the agent commits my edit?

That works. When the agent runs `git commit`, the commit hook compares each locked
change with the copy taken when the command started. Your edit was already there,
so `lock` records it as yours, and a change that the same command made is refused.

### Can I add another agent?

Yes, if the agent can run a hook before each tool call and after each shell
command. [Open an issue](https://github.com/dariodaddamio/lock/issues/new?template=agent.yml)
with an example of the JSON that the agent sends to its hooks.

### How do I remove lock?

Run `lock uninstall` in the repository, then delete the binary. The lock markers
and `.locks/` stay, so `lock install` brings the protection back.

## Contributing

Bug reports, ideas, and pull requests are welcome. To build `lock`, run the
scenario checks, and time the hooks:

```sh
cargo build --release
LOCK="$PWD/target/release/lock" bash tests/scenarios.sh
python3 tests/bench.py target/release/lock
```

To report a way around `lock`, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
