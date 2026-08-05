#!/usr/bin/env bash
#
# install-macos.sh — Instant File Search: build, deploy, and register the
# native indexer + MCP server on macOS (launchd LaunchDaemon).
#
# Mirrors scripts/install-linux.sh for macOS. What it does:
#   1. Builds release binaries (indexer + MCP server)
#   2. Deploys them to /usr/local/lib/instant-file-search/
#   3. Installs + bootstraps the launchd daemon
#      (com.clayleopardlabs.instant-file-search.plist)
#   4. Registers the MCP server client for OpenCode (~/.config/opencode/)
#      and patches oh-my-opencode-slim subagent `mcps` lists
#   5. Prints Full Disk Access (TCC) guidance — a required manual step
#
# Requirements:
#   - macOS 13+ (FSEvents / getattrlistbulk; any recent macOS works)
#   - rustup with the `aarch64-apple-darwin` target (cargo build)
#   - python3 (for comment-aware JSONC config patching)
#   - run as root (sudo), or with sudo available
#
# Usage:
#   sudo ./install-macos.sh [--no-register] [--dry-run]
#
# After install: grant Full Disk Access to
# /usr/local/lib/instant-file-search/instant-file-search-indexer in
# System Settings > Privacy & Security > Full Disk Access, then restart the
# daemon (launchctl kickstart -k system/com.clayleopardlabs.instant-file-search).

set -euo pipefail

INSTALL_DIR="/usr/local/lib/instant-file-search"
PLIST_NAME="com.clayleopardlabs.instant-file-search.plist"
PLIST_SRC="$(cd "$(dirname "$0")" && pwd)/${PLIST_NAME}"
REGISTER_SCRIPT="$(cd "$(dirname "$0")" && pwd)/register-linux-client.py"

DRY_RUN=0
DO_REGISTER=1
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        --no-register) DO_REGISTER=0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

step() { printf '\n==> %s\n' "$*"; }
action() { if [ "$DRY_RUN" -eq 1 ]; then printf 'DRY RUN: %s\n' "$*"; fi }

# ---- 0. Prerequisites -------------------------------------------------------
if [ "$DRY_RUN" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
    echo "error: run as root (sudo ./install-macos.sh)" >&2
    exit 1
fi

command -v cargo >/dev/null 2>&1 || { echo "error: cargo not found in PATH" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "error: python3 required for client registration" >&2; exit 1; }
command -v launchctl >/dev/null 2>&1 || { echo "error: launchctl not found; this installer must run on macOS" >&2; exit 1; }
command -v plutil >/dev/null 2>&1 || { echo "error: plutil not found; cannot validate launchd plist" >&2; exit 1; }

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ---- 1. Build ---------------------------------------------------------------
step "Building release binaries (workspace)"
action "cargo build --release --workspace"
if [ "$DRY_RUN" -eq 0 ]; then
    (cd "$REPO_ROOT" && cargo build --release --workspace)
fi

INDEXER_BIN="$REPO_ROOT/target/release/instant-file-search-indexer"
SERVER_BIN="$REPO_ROOT/target/release/instant-file-search-mcp-server"
if [ "$DRY_RUN" -eq 0 ]; then
    [ -x "$INDEXER_BIN" ] || { echo "error: indexer binary missing: $INDEXER_BIN" >&2; exit 1; }
    [ -x "$SERVER_BIN" ] || { echo "error: MCP server binary missing: $SERVER_BIN" >&2; exit 1; }
fi

# ---- 2. Deploy ---------------------------------------------------------------
step "Deploying binaries to $INSTALL_DIR"
action "mkdir -p $INSTALL_DIR && install -m 0755 $INDEXER_BIN $SERVER_BIN $INSTALL_DIR/"
if [ "$DRY_RUN" -eq 0 ]; then
    mkdir -p "$INSTALL_DIR"
    install -m 0755 "$INDEXER_BIN" "$INSTALL_DIR/"
    install -m 0755 "$SERVER_BIN" "$INSTALL_DIR/"
    # Ad-hoc code signature: bare binaries are refused FDA grants on some macOS
    # versions. A signed bundle with a real CFBundleIdentifier is the robust
    # path; this gives dev installs the least overhead.
    codesign --force --sign - "$INSTALL_DIR/instant-file-search-indexer" 2>/dev/null \
        || echo "warning: ad-hoc codesign failed (optional for dev; see docs/macos-support.md)" >&2
fi

# ---- 3. launchd daemon -------------------------------------------------------
step "Installing launchd daemon $PLIST_NAME"
action "plutil -lint $PLIST_SRC && install -o root -g wheel -m 0644 $PLIST_SRC /Library/LaunchDaemons/$PLIST_NAME && launchctl bootstrap system /Library/LaunchDaemons/$PLIST_NAME"
if [ "$DRY_RUN" -eq 0 ]; then
    [ -f "$PLIST_SRC" ] || { echo "error: plist missing: $PLIST_SRC" >&2; exit 1; }
    plutil -lint "$PLIST_SRC" >/dev/null
    install -o root -g wheel -m 0644 "$PLIST_SRC" "/Library/LaunchDaemons/$PLIST_NAME"
    # Remove any previous instance so bootstrap is idempotent.
    launchctl bootout "system/com.clayleopardlabs.instant-file-search" 2>/dev/null || true
    launchctl bootstrap system "/Library/LaunchDaemons/$PLIST_NAME"
    launchctl kickstart -k "system/com.clayleopardlabs.instant-file-search"
    echo "Daemon status:"
    launchctl print "system/com.clayleopardlabs.instant-file-search" 2>/dev/null | head -20 || true
fi

# ---- 4. OpenCode client registration -----------------------------------------
if [ "$DO_REGISTER" -eq 1 ]; then
    step "Registering MCP client for OpenCode"
    action "python3 $REGISTER_SCRIPT --server-binary $INSTALL_DIR/instant-file-search-mcp-server"
    if [ "$DRY_RUN" -eq 0 ]; then
        if [ -f "$REGISTER_SCRIPT" ]; then
            python3 "$REGISTER_SCRIPT" --server-binary "$INSTALL_DIR/instant-file-search-mcp-server"
        else
            echo "warning: $REGISTER_SCRIPT not found; skipping client registration" >&2
        fi
    fi
else
    echo "skipping client registration (--no-register)"
fi

# ---- 5. TCC / Full Disk Access guidance --------------------------------------
step "Full Disk Access (required manual step)"
cat <<'EOF'
The indexer must be granted Full Disk Access to scan protected locations
(Desktop, Documents, Downloads, iCloud, Photos, Mail, ...). root does NOT
imply FDA.

1. Open System Settings > Privacy & Security > Full Disk Access
2. Click "+" and add:
     /usr/local/lib/instant-file-search/instant-file-search-indexer
   (If the bare binary is refused, build a signed app bundle around it —
   see docs/macos-support.md.)
3. Restart the daemon:
     sudo launchctl kickstart -k system/com.clayleopardlabs.instant-file-search

Grants are silently revocable by OS upgrades. If searches silently miss files
after an upgrade, re-check this setting.
EOF

step "Done. Verify with: launchctl print system/com.clayleopardlabs.instant-file-search  and  search_status via the MCP server"
