# Contributing

Bug reports, support for more agents, and fixes are welcome. For anything larger
than a small fix, open an issue first, so that we agree on the approach before you
write the code.

## Build and check

You need Rust 1.88 or later, git, and bash. On Windows, Git Bash works.

```sh
cargo build --release
cargo fmt --check
cargo clippy --release -- -D warnings
LOCK="$PWD/target/release/lock" bash tests/scenarios.sh
```

The scenarios run `lock` against real hook payloads and git states in a throwaway
repository. CI runs them on Linux, macOS, and Windows for each pull request.

To time the hooks, run `python3 tests/bench.py target/release/lock`.

## What a pull request needs

- A change in behaviour comes with a scenario that fails without the change and
  passes with it.
- A change must never let an agent change locked text without a person's
  approval. If a change touches the hook, `watch`, or approvals, explain in the
  pull request why that still holds.
- New dependencies need a reason, because each one adds to the binary and to what
  users have to trust.
- Describe the change in the pull request. The maintainer writes the changelog
  entry when the release goes out.

To add an agent, open an issue with the agent template first, with examples of the
JSON that the agent sends to its hooks.

## How a change reaches users

A merged pull request changes nothing for users until the maintainer pushes a
version tag. The tag builds the release binaries in CI, and the maintainer
publishes the crate to crates.io. Only the maintainer can push tags or publish.

To report a way around `lock`, see [SECURITY.md](SECURITY.md) instead of opening
an issue.
