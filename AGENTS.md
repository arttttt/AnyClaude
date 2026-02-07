Read file ARCHITECTURE.md

## Verification

After making code changes, run `just check` to lint and test:

```
just check
```

This runs `cargo clippy --all-targets -p anyclaude` followed by `cargo test`. Both must pass before committing.
