# Security

`lock` exists to stop coding agents from changing locked text without a person's
approval. Any way around that is a security issue, so we want to hear about it.

## What to report

- A tool call, shell command, or sequence of steps that lets an agent change locked
  text, remove a lock, or write an approval record without a person.
- A way for an agent to remove or turn off the hooks without `lock` undoing it.
- A way to make Claude Code's approval prompt approve a change without a person.

The cases that the README lists under Limits are known, so they are not reports,
but ideas to close them are welcome as issues.

## How to report

Report a vulnerability privately through
[GitHub security advisories](https://github.com/dariodaddamio/lock/security/advisories/new).
Include the agent and its version, your operating system, and the steps or the hook
input that reproduce the problem. You will get an answer within a week.

Only the latest release gets security fixes.
