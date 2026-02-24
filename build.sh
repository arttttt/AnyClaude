#!/usr/bin/env bash
#
# Build script for anyclaude
#
# Usage:
#   ./build.sh              - Build release binary
#   ./build.sh debug        - Build debug binary
#   ./build.sh clean        - Clean build artifacts
#   ./build.sh test         - Run tests
#   ./build.sh install      - Build and install to ~/.cargo/bin
#   ./build.sh release-tag  - Build release without dev version stamp
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Project root directory (where this script lives)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Print colored status message
info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
    exit 1
}

# Check if Rust toolchain is installed
check_rust() {
    if ! command -v cargo &> /dev/null; then
        error "Cargo not found. Please install Rust: https://rustup.rs"
    fi
    info "Rust toolchain found: $(rustc --version)"
}

# --- Version stamping ---
# For non-release builds, stamp the version in Cargo.toml with git info:
#   0.4.0 → 0.4.0-dev.abc1234        (clean tree)
#   0.4.0 → 0.4.0-dev.abc1234.dirty  (uncommitted changes)
# The original Cargo.toml is always restored after build.

BASE_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
CARGO_TOML_PATCHED=false

stamp_version() {
    local commit dirty suffix

    commit=$(git rev-parse --short=7 HEAD 2>/dev/null || echo "unknown")
    if git diff --quiet HEAD 2>/dev/null; then
        dirty=""
    else
        dirty=".dirty"
    fi
    suffix="dev.${commit}${dirty}"

    local stamped="${BASE_VERSION}-${suffix}"
    info "Version: ${stamped}"

    # Backup and patch
    cp Cargo.toml Cargo.toml.bak
    sed -i '' "s/^version = \"${BASE_VERSION}\"/version = \"${stamped}\"/" Cargo.toml
    CARGO_TOML_PATCHED=true
}

restore_version() {
    if [ "$CARGO_TOML_PATCHED" = true ] && [ -f Cargo.toml.bak ]; then
        mv Cargo.toml.bak Cargo.toml
        CARGO_TOML_PATCHED=false
    fi
}

# Always restore Cargo.toml on exit (error, signal, etc.)
trap restore_version EXIT

# Build release binary
build_release() {
    stamp_version
    info "Building release binary..."
    cargo build --release
    info "Build complete: target/release/anyclaude"
}

# Build tagged release (no dev stamp)
build_release_tag() {
    info "Version: ${BASE_VERSION} (release)"
    info "Building release binary..."
    cargo build --release
    info "Build complete: target/release/anyclaude"
}

# Build debug binary
build_debug() {
    stamp_version
    info "Building debug binary..."
    cargo build
    info "Build complete: target/debug/anyclaude"
}

# Clean build artifacts
clean() {
    info "Cleaning build artifacts..."
    cargo clean
    info "Clean complete"
}

# Run tests
run_tests() {
    info "Running tests..."
    cargo test
    info "Tests complete"
}

# Install binary to ~/.cargo/bin
install_binary() {
    stamp_version
    info "Building and installing..."
    cargo install --path .
    info "Installed to ~/.cargo/bin/anyclaude"
}

# Main entry point
main() {
    check_rust

    case "${1:-release}" in
        release)
            build_release
            ;;
        release-tag)
            build_release_tag
            ;;
        debug)
            build_debug
            ;;
        clean)
            clean
            ;;
        test)
            run_tests
            ;;
        install)
            install_binary
            ;;
        *)
            echo "Usage: $0 {release|release-tag|debug|clean|test|install}"
            exit 1
            ;;
    esac
}

main "$@"
