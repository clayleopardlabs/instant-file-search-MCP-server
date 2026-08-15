#!/usr/bin/env bash
#
# install-linux.sh — Instant File Search: build, deploy, and register the
# native indexer + MCP server on Linux (Ubuntu 24.04 LTS / systemd).
#
# Mirrors scripts/install.ps1 for Windows. What it does:
#   1. Builds release binaries (indexer + MCP server)
#   2. Deploys them to /usr/local/lib/instant-file-search/
#   3. Installs + enables the systemd unit (instant-file-search-indexer.service)
#   4. Registers the MCP server client for OpenCode (~/.config/opencode/)
#      and patches oh-my-opencode-slim subagent `mcps` lists
#
# Requirements:
#   - Ubuntu 24.04 (or any systemd distro with fanotify in the kernel, 5.13+)
#   - rustup with the `x86_64-unknown-linux-gnu` target (cargo build)
#   - python3 (for comment-aware JSONC config patching)
#   - run as root (sudo), or with sudo available
#
# Usage:
#   sudo ./install-linux.sh [--index-mode memory|disk] [--content-mode auto|off|memory|disk] [--no-register] [--dry-run]

set -euo pipefail

INSTALL_DIR="/usr/local/lib/instant-file-search"
UNIT_NAME="instant-file-search-indexer.service"
UNIT_SRC="$(cd "$(dirname "$0")" && pwd)/${UNIT_NAME}"
REGISTER_SCRIPT="$(cd "$(dirname "$0")" && pwd)/register-linux-client.py"

DRY_RUN=0
DO_REGISTER=1
INDEX_MODE=""
CONTENT_MODE=""
ENV_FILE="/etc/instant-file-search/indexer.env"
if [ -f "$ENV_FILE" ]; then
    INDEX_MODE="$(sed -n 's/^INSTANT_FS_INDEX_MODE=//p' "$ENV_FILE" | tail -1)"
    CONTENT_MODE="$(sed -n 's/^INSTANT_FS_CONTENT_INDEX=//p' "$ENV_FILE" | tail -1)"
fi
i=1
while [ "$i" -le "$#" ]; do
    arg="${!i}"
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        --no-register) DO_REGISTER=0 ;;
        --index-mode=*) INDEX_MODE="${arg#*=}" ;;
        --content-mode=*) CONTENT_MODE="${arg#*=}" ;;
        --index-mode)
            i=$((i + 1)); INDEX_MODE="${!i}" ;;
        --content-mode)
            i=$((i + 1)); CONTENT_MODE="${!i}" ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
    i=$((i + 1))
done
: "${INDEX_MODE:=memory}"
: "${CONTENT_MODE:=auto}"
case "$INDEX_MODE" in memory|disk) ;; *) echo "error: --index-mode must be memory or disk" >&2; exit 2 ;; esac
case "$CONTENT_MODE" in auto|off|memory|disk) ;; *) echo "error: --content-mode must be auto, off, memory, or disk" >&2; exit 2 ;; esac

step() { printf '\n==> %s\n' "$*"; }
action() { if [ "$DRY_RUN" -eq 1 ]; then printf 'DRY RUN: %s\n' "$*"; fi }

# ---- 0. Prerequisites -------------------------------------------------------
if [ "$DRY_RUN" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
    echo "error: run as root (sudo ./install-linux.sh)" >&2
    exit 1
fi

command -v cargo >/dev/null 2>&1 || { echo "error: cargo not found in PATH" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "error: python3 required for client registration" >&2; exit 1; }

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
fi

# ---- 3. systemd unit ---------------------------------------------------------
step "Installing systemd unit $UNIT_NAME"
action "write /etc/instant-file-search/indexer.env (index=$INDEX_MODE content=$CONTENT_MODE); cp $UNIT_SRC /etc/systemd/system/$UNIT_NAME && systemctl daemon-reload && systemctl enable --now $UNIT_NAME"
if [ "$DRY_RUN" -eq 0 ]; then
    [ -f "$UNIT_SRC" ] || { echo "error: unit file missing: $UNIT_SRC" >&2; exit 1; }
    install -d -m 0755 "$(dirname "$ENV_FILE")"
    printf 'INSTANT_FS_INDEX_MODE=%s\nINSTANT_FS_CONTENT_INDEX=%s\n' "$INDEX_MODE" "$CONTENT_MODE" > "$ENV_FILE"
    chmod 0644 "$ENV_FILE"
    cp "$UNIT_SRC" "/etc/systemd/system/$UNIT_NAME"
    systemctl daemon-reload
    systemctl enable --now "$UNIT_NAME"
    echo "Service status:"
    systemctl --no-pager --full status "$UNIT_NAME" || true
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

step "Done. Verify with: systemctl status $UNIT_NAME  and  search_status via the MCP server"
