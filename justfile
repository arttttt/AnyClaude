# AnyClaude task runner

# Run all checks (lint + test)
check:
    cargo clippy --all-targets -p anyclaude
    cargo test

# Release a new version: just release 0.3.0
release version:
    cargo set-version {{version}}
    git cliff --tag v{{version}} --output CHANGELOG.md
    git add Cargo.toml Cargo.lock CHANGELOG.md
    git commit -m "chore: release v{{version}}"
    git tag v{{version}}

# Update CHANGELOG without releasing
changelog:
    git cliff --output CHANGELOG.md
