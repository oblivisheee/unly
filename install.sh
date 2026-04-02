#!/usr/bin/env bash
# install.sh — Install and set up the Unly agent platform.
#
# Usage:
#   bash <(curl -fsSL https://raw.githubusercontent.com/oblivisheee/unly/main/install.sh)
#   or, from a local clone:
#   bash install.sh
#
# What this script does:
#   1. Installs Rust (via rustup) if not already present.
#   2. Clones the repository if the script is not already running inside it.
#   3. Builds the release binary.
#   4. Runs the first-run onboarding wizard (unly setup).

set -euo pipefail

REPO_URL="https://github.com/oblivisheee/unly"
INSTALL_DIR="${UNLY_DIR:-$HOME/.local/share/unly}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m    OK: %s\033[0m\n' "$*"; }
die()   { printf '\033[1;31mError: %s\033[0m\n' "$*" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found. $2"
}

# ---------------------------------------------------------------------------
# 1. Rust
# ---------------------------------------------------------------------------

install_rust() {
    info "Installing Rust via rustup..."
    require_cmd curl "Please install curl first."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
    # Source the cargo env so the rest of the script can use it.
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
}

if ! command -v cargo >/dev/null 2>&1; then
    install_rust
elif ! command -v rustup >/dev/null 2>&1; then
    ok "cargo found (not managed by rustup — skipping Rust install)"
else
    ok "Rust $(rustc --version)"
fi

# ---------------------------------------------------------------------------
# 2. Repository
# ---------------------------------------------------------------------------

# Detect whether we are already inside the unly repository.
if [ -f "Cargo.toml" ] && grep -q 'name = "unly"' Cargo.toml 2>/dev/null; then
    REPO_DIR="$(pwd)"
    info "Using existing repository at $REPO_DIR"
else
    require_cmd git "Please install git first."
    if [ -d "$INSTALL_DIR/.git" ]; then
        info "Updating existing clone at $INSTALL_DIR..."
        git -C "$INSTALL_DIR" pull --ff-only
    else
        info "Cloning $REPO_URL into $INSTALL_DIR..."
        git clone "$REPO_URL" "$INSTALL_DIR"
    fi
    REPO_DIR="$INSTALL_DIR"
    cd "$REPO_DIR"
fi

# ---------------------------------------------------------------------------
# 3. Build
# ---------------------------------------------------------------------------

info "Building release binary (this may take a few minutes)..."
cargo build --release 2>&1

BINARY="$REPO_DIR/target/release/unly"
[ -x "$BINARY" ] || die "Build succeeded but binary not found at $BINARY"
ok "Binary built: $BINARY"

# ---------------------------------------------------------------------------
# 4. Convenience symlink (optional, skip if /usr/local/bin is not writable)
# ---------------------------------------------------------------------------

LINK_TARGET="/usr/local/bin/unly"
if [ -w "/usr/local/bin" ] && [ ! -e "$LINK_TARGET" ]; then
    ln -sf "$BINARY" "$LINK_TARGET"
    ok "Symlink created: $LINK_TARGET"
elif [ ! -w "/usr/local/bin" ]; then
    printf '\nNote: Add %s to your PATH to run unly from anywhere:\n' "$REPO_DIR/target/release"
    printf '  export PATH="%s:$PATH"\n\n' "$REPO_DIR/target/release"
fi

# ---------------------------------------------------------------------------
# 5. Onboarding wizard
# ---------------------------------------------------------------------------

info "Launching onboarding wizard..."
"$BINARY" setup
