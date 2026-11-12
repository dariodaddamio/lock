---
title: Team setup
nav_order: 7
---

# Set up a team repository

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
      - uses: dariodaddamio/lock@v0.3.2
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
