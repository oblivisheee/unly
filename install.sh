#!/usr/bin/env bash
# install.sh — Install and set up the Unly agent platform.
#
# Usage:
#   bash <(curl -fsSL https://raw.githubusercontent.com/oblivisheee/unly/main/install.sh)
#   or, from a local clone:
#   bash install.sh
#   force local install mode:
#   bash install.sh --local
#
# What this script does:
#   1. Installs Rust (via rustup) if not already present.
#   2. Installs `unly` via cargo install.
#   3. Ensures Cargo bin path is on PATH.
#   4. Runs the first-run onboarding wizard (unly setup).

set -euo pipefail

REPO_URL="https://github.com/oblivisheee/unly"
CARGO_BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
LOCAL_ONLY=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m    OK: %s\033[0m\n' "$*"; }
die()   { printf '\033[1;31mError: %s\033[0m\n' "$*" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found. $2"
}

print_usage() {
    cat <<EOF
Usage: bash install.sh [--local]

Options:
  --local   Install only from the current local repository (no remote fetch).
  -h, --help  Show this help message.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --local)
            LOCAL_ONLY=true
            shift
            ;;
        -h|--help)
            print_usage
            exit 0
            ;;
        *)
            die "Unknown argument: $1"
            ;;
    esac
done

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
# 2. Install via cargo
# ---------------------------------------------------------------------------

if [ "$LOCAL_ONLY" = true ]; then
    if [ ! -f "Cargo.toml" ]; then
        die "--local requires running inside the local unly repository root"
    fi
    REPO_DIR="$(pwd)"
    info "Installing from local repository at $REPO_DIR (--local)..."
    cargo install --path crates/unly-cli --force
elif [ -f "Cargo.toml" ] && grep -q 'name = "unly"' Cargo.toml 2>/dev/null; then
    REPO_DIR="$(pwd)"
    info "Installing from local repository at $REPO_DIR..."
    cargo install --path crates/unly-cli --force
else
    info "Installing from GitHub repository..."
    cargo install --git "$REPO_URL" --bin unly --force
fi

BINARY="$CARGO_BIN_DIR/unly"
[ -x "$BINARY" ] || die "Install succeeded but binary not found at $BINARY"
ok "Binary installed: $BINARY"

# ---------------------------------------------------------------------------
# 3. Ensure PATH includes cargo bin
# ---------------------------------------------------------------------------

ensure_path_entry() {
    local shell_rc="$1"
    local line='export PATH="$HOME/.cargo/bin:$PATH"'
    if [ -f "$shell_rc" ] && grep -Fq "$line" "$shell_rc"; then
        return 0
    fi
    printf '\n# Added by unly installer\n%s\n' "$line" >> "$shell_rc"
}

case "${SHELL:-}" in
    */zsh) ensure_path_entry "$HOME/.zshrc" ;;
    */bash) ensure_path_entry "$HOME/.bashrc" ;;
esac

export PATH="$CARGO_BIN_DIR:$PATH"
ok "PATH updated for current shell ($CARGO_BIN_DIR)"

# ---------------------------------------------------------------------------
# 4. Onboarding wizard
# ---------------------------------------------------------------------------

info "Launching onboarding wizard..."
"$BINARY" setup
