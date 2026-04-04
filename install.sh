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
#   1. Detects the platform and tries to download a prebuilt binary from the
#      latest GitHub release.
#   2. If no prebuilt binary is available for this platform, installs Rust
#      (via rustup) if needed and falls back to cargo install.
#   3. Ensures the install directory is on PATH.
#   4. Runs the first-run onboarding wizard (unly setup).

set -euo pipefail

RELEASE_REPO="${UNLY_RELEASE_REPO:-oblivisheee/unly}"
REPO_URL="https://github.com/$RELEASE_REPO"
PREBUILT_INSTALL_DIR="${UNLY_INSTALL_DIR:-$HOME/.local/bin}"
LOCAL_ONLY=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m    OK: %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m WARN: %s\033[0m\n' "$*"; }
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
# Platform detection
# ---------------------------------------------------------------------------

detect_target() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64)          echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64)   echo "aarch64-unknown-linux-gnu" ;;
                *)               echo "" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64|aarch64)   echo "aarch64-apple-darwin" ;;
                *)               echo "" ;;
            esac
            ;;
        *)
            echo ""
            ;;
    esac
}

# ---------------------------------------------------------------------------
# 1. Try prebuilt binary from GitHub Releases
# ---------------------------------------------------------------------------

INSTALL_DIR=""
BINARY=""
INSTALLED_VIA=""

try_install_prebuilt() {
    local target
    target="$(detect_target)"
    if [ -z "$target" ]; then
        warn "No prebuilt binary for this platform ($(uname -s)/$(uname -m)). Will use cargo install."
        return 1
    fi

    require_cmd curl "Please install curl first."

    local url="https://github.com/$RELEASE_REPO/releases/latest/download/unly-${target}"
    info "Downloading prebuilt binary for ${target}..."
    if ! curl -fsSL --output /tmp/unly-prebuilt "$url"; then
        warn "Download failed (URL: $url). Will use cargo install."
        return 1
    fi

    mkdir -p "$PREBUILT_INSTALL_DIR"
    mv /tmp/unly-prebuilt "$PREBUILT_INSTALL_DIR/unly"
    chmod +x "$PREBUILT_INSTALL_DIR/unly"

    INSTALL_DIR="$PREBUILT_INSTALL_DIR"
    BINARY="$INSTALL_DIR/unly"
    INSTALLED_VIA="prebuilt"
}

is_local_repo() {
    [ -f "Cargo.toml" ] && grep -q 'name = "unly"' Cargo.toml 2>/dev/null
}

if [ "$LOCAL_ONLY" = false ] && ! is_local_repo; then
    try_install_prebuilt || true
fi

# ---------------------------------------------------------------------------
# 2. Fall back to cargo install if prebuilt was not installed
# ---------------------------------------------------------------------------

if [ -z "$INSTALLED_VIA" ]; then
    install_rust() {
        info "Installing Rust via rustup..."
        require_cmd curl "Please install curl first."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
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

    CARGO_BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
    INSTALL_DIR="$CARGO_BIN_DIR"
    BINARY="$CARGO_BIN_DIR/unly"

    if [ "$LOCAL_ONLY" = true ]; then
        if [ ! -f "Cargo.toml" ]; then
            die "--local requires running inside the local unly repository root"
        fi
        info "Installing from local repository (--local)..."
        cargo install --path crates/unly-cli --force
    elif is_local_repo; then
        info "Installing from local repository..."
        cargo install --path crates/unly-cli --force
    else
        info "Installing from GitHub repository via cargo..."
        cargo install --git "$REPO_URL" --bin unly --force
    fi

    INSTALLED_VIA="cargo"
fi

[ -x "$BINARY" ] || die "Install succeeded but binary not found at $BINARY"
ok "Binary installed at $BINARY (via $INSTALLED_VIA)"

# ---------------------------------------------------------------------------
# 3. Ensure PATH includes install directory
# ---------------------------------------------------------------------------

ensure_path_entry() {
    local shell_rc="$1"
    local dir="$2"
    local line="export PATH=\"${dir}:\$PATH\""
    if [ -f "$shell_rc" ] && grep -Fq "$dir" "$shell_rc"; then
        return 0
    fi
    printf '\n# Added by unly installer\nexport PATH="%s:$PATH"\n' "$dir" >> "$shell_rc"
}

case "${SHELL:-}" in
    */zsh)  ensure_path_entry "$HOME/.zshrc"    "$INSTALL_DIR" ;;
    */bash) ensure_path_entry "$HOME/.bashrc"   "$INSTALL_DIR" ;;
esac

export PATH="$INSTALL_DIR:$PATH"
ok "PATH updated for current shell ($INSTALL_DIR)"

# ---------------------------------------------------------------------------
# 4. Onboarding wizard
# ---------------------------------------------------------------------------

info "Launching onboarding wizard..."
"$BINARY" setup
