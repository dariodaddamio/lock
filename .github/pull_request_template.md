## What this changes

<!-- What the change does, and why. Link the issue if there is one. -->

## Checklist

- [ ] `cargo fmt --check` and `cargo clippy --release -- -D warnings` pass.
- [ ] `LOCK="$PWD/target/release/lock" bash tests/scenarios.sh` passes.
- [ ] A change in behaviour comes with a scenario in `tests/scenarios.sh`.
- [ ] No new dependency, or the pull request explains why it is needed.
