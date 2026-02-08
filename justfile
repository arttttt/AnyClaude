# AnyClaude task runner

# Run all checks (lint + test)
check:
    @just lint-test-location
    cargo clippy --all-targets -p anyclaude
    cargo test

# Ensure no inline #[cfg(test)] in source files
lint-test-location:
    #!/usr/bin/env bash
    set -euo pipefail
    violations=$(grep -rn '#\[cfg(test)\]' src/ --include='*.rs' || true)
    if [ -n "$violations" ]; then
        echo "ERROR: #[cfg(test)] found in src/. All tests must be in tests/."
        echo "$violations"
        exit 1
    fi

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
