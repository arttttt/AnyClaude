Read file ARCHITECTURE.md

## Verification

After making code changes, run `just check` to lint and test:

```
just check
```

This runs `cargo clippy --all-targets -p anyclaude` followed by `cargo test`. Both must pass before committing.

## Testing Rules

- **No `#[cfg(test)]` in source files.** All tests go in `tests/`. Shared helpers go in a common module. Enforced by `just check`.
- **Before changing tests**: run existing tests first. If a code change breaks tests, that's a signal — analyze why and fix the code or update the test deliberately, not silently replace it.
- **Never silently rewrite failing tests** to make them pass. A broken test means either the code is wrong or the test caught a real regression.
- **Test behavior, not implementation**: tests should verify observable outcomes (input blocked, state transitions, cursor visibility) through the same public API the production code uses.
- **Cover edge cases at integration boundaries**: unit tests on reducers are not enough — test how App methods behave across lifecycle states (Pending → Attached → Ready) including the transitions themselves.
