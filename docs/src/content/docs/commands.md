---
title: Commands
---

| Command | What it does |
| --- | --- |
| `lock install [--git] [--claude] [--codex] [--cursor]` | Adds the hooks to this clone. With no options, it adds all of them. |
| `lock approve [<id> or <path>...] [--reason <text>]` | Shows each blocked or unrecorded change and records the ones that you accept. |
| `lock list` | Shows each lock in the working tree. |
| `lock doctor` | Checks that `lock` guards this clone, and prints a PASS, WARN, or FAIL line with a fix for each check. It only reads, and it exits with a non-zero code when a check fails. |
| `lock check` | Compares the working tree with `HEAD`. Warns when `LOCKED:` appears in a file type that cannot hold markers. |
| `lock check --staged` | Checks the staged changes, and fails for a locked change without an approval. The commit hook runs this. |
| `lock check --base <ref>` | Compares `HEAD` with the point where it branched from `<ref>`. CI runs this. |
| `lock uninstall` | Removes the hooks that `lock install` added. The markers and `.locks/` stay. |
| `lock hook claude`, `lock hook codex`, `lock hook cursor` | The agent hook. The agent runs it and sends one tool call on stdin. |
