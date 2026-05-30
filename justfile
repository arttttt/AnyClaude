# AnyClaude task runner

# Run all checks (lint + test) across the whole workspace
check:
    @just lint-test-location
    cargo clippy --workspace --all-targets
    cargo test --workspace

# Ensure no inline #[cfg(test)] in source files (anyclaude + all workspace crates)
lint-test-location:
    #!/usr/bin/env bash
    set -euo pipefail
    violations=$(grep -rn '#\[cfg(test)\]' src/ crates/*/src/ --include='*.rs' || true)
    if [ -n "$violations" ]; then
        echo "ERROR: #[cfg(test)] found in src/ or crates/*/src/. All tests must be in tests/."
        echo "$violations"
        exit 1
    fi

# Release a new version: just release 0.3.0
release version:
    cargo set-version {{version}}
    git cliff --tag {{version}} --output CHANGELOG.md
    git add Cargo.toml Cargo.lock CHANGELOG.md
    git commit -m "chore: release {{version}}"
    git tag {{version}}

# Update CHANGELOG without releasing
changelog:
    git cliff --output CHANGELOG.md
