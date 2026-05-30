Read file ARCHITECTURE.md

## Verification

After making code changes, run `just check` to lint and test:

```
just check
```

This runs `cargo clippy --all-targets -p anyclaude` followed by `cargo test` — the **anyclaude** crate only. The repo is a workspace with six lower crates (`term_core` / `term_gpu` / `term_layout` / `term_clipboard` / `term_ui` / `uikit`); their tests live in `crates/*/tests/`. Run `cargo test --workspace` for the full suite. Convention: `cargo check` (or `cargo check --workspace`) after each commit, full `cargo test --workspace` at milestones. Workspace lints `dead_code` / `unused_imports` are `deny`.

## Testing Rules

- **No `#[cfg(test)]` in source files.** All tests live in `tests/` (anyclaude) and `crates/*/tests/`. Shared helpers go in a common module. Enforced by `just check`.
- **Before changing tests**: run existing tests first. If a code change breaks tests, that's a signal — analyze why and fix the code or update the test deliberately, not silently replace it.
- **Never silently rewrite failing tests** to make them pass. A broken test means either the code is wrong or the test caught a real regression.
- **Test behavior, not implementation**: tests should verify observable outcomes (input blocked, state transitions, cursor visibility) through the same public API the production code uses.
- **Cover edge cases at integration boundaries**: pure-helper tests aren't enough — drive the coordinator's `AppState::apply` arms and the popup state machines through their transitions (e.g. open → navigate → apply / dismiss), asserting the emitted `Effect`s, not just isolated functions.

## Commit Rules

- **Atomic commits**: each commit should represent one logical change. Split large changes into multiple commits:
  - New feature/module → separate commit
  - Integration into existing code → separate commit
  - Dead code removal → separate commit
  - Tests → separate commit
  - Documentation → separate commit
- **Commit order**: add new code before removing old code it replaces. This keeps the codebase buildable at every commit.
- **Verify before commit**: run `just check` to ensure clippy and tests pass.
