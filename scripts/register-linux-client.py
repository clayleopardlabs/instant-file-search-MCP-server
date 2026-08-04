#!/usr/bin/env python3
"""
register-linux-client.py — register the instant-file-search MCP server as a
client for OpenCode on Linux, mirroring what scripts/install.ps1 does on
Windows:

  1. Adds the `instant-file-search` server entry to the OpenCode config's
     top-level `mcp` object (~/.config/opencode/opencode.jsonc, or
     opencode.json if that is what exists).
  2. Patches oh-my-opencode-slim sub-agent `mcps` lists so explorer/fixer/
     oracle/designer/librarian can see the MCP-instantiated tools.

Comment-aware: JSONC (comments + trailing commas) is preserved for the
OpenCode config by editing the raw text, exactly like Insert-McpEntryText
in install.ps1. The OMO config is parsed and rewritten as plain JSON.

Usage:
  python3 register-linux-client.py --server-binary /usr/local/lib/instant-file-search/instant-file-search-mcp-server
  python3 register-linux-client.py --server-binary PATH --config-dir ~/.config/opencode --dry-run
"""

import argparse
import json
import os
import re
import shutil
import sys
import tempfile

SERVER_NAME = "instant-file-search"


def config_paths(config_dir: str) -> list[str]:
    """Candidate OpenCode config paths, preferred first."""
    return [
        os.path.join(config_dir, "opencode.jsonc"),
        os.path.join(config_dir, "opencode.json"),
    ]


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def write_text(path: str, text: str) -> None:
    """Write UTF-8 without BOM (Python open() never writes a BOM)."""
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)


def find_mcp_object(raw: str) -> tuple[int, int] | None:
    """
    Locate the top-level `mcp` key's value object (brace span) in JSONC text.
    Returns (brace_open_index, brace_close_index) or None.
    """
    n = len(raw)
    i = 0
    in_str = False
    esc = False
    while i < n:
        c = raw[i]
        nxt = raw[i + 1] if i + 1 < n else ""
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == '"':
                in_str = False
        elif c == '"':
            in_str = True
        elif c == "/" and nxt == "/":
            while i < n and raw[i] != "\n":
                i += 1
        elif c == "/" and nxt == "*":
            i += 2
            while i < n and not (raw[i] == "*" and i + 1 < n and raw[i + 1] == "/"):
                i += 1
            if i < n:
                i += 2
        else:
            # Match a key named "mcp" at the start of a line region.
            m = re.match(r'\s*"mcp"\s*:', raw[i:])
            if m:
                j = i + m.end()
                while j < n and raw[j] != "{":
                    j += 1
                if j >= n:
                    return None
                open_idx = j
                # find matching close brace
                k = open_idx
                d = 0
                in_str2 = False
                esc2 = False
                while k < n:
                    c2 = raw[k]
                    n2 = raw[k + 1] if k + 1 < n else ""
                    if in_str2:
                        if esc2:
                            esc2 = False
                        elif c2 == "\\":
                            esc2 = True
                        elif c2 == '"':
                            in_str2 = False
                    elif c2 == '"':
                        in_str2 = True
                    elif c2 == "/" and n2 == "/":
                        while k < n and raw[k] != "\n":
                            k += 1
                    elif c2 == "/" and n2 == "*":
                        k += 2
                        while k < n and not (raw[k] == "*" and k + 1 < n and raw[k + 1] == "/"):
                            k += 1
                        if k < n:
                            k += 2
                    else:
                        if c2 == "{":
                            d += 1
                        elif c2 == "}":
                            d -= 1
                            if d == 0:
                                return (open_idx, k)
                    k += 1
                return None
            i += 1
        i += 1
    return None


def entry_text(server_binary: str, indent: str) -> str:
    """The JSONC entry for the instant-file-search server."""
    return (
        f"{indent}\"{SERVER_NAME}\": {{\n"
        f"{indent}  \"type\": \"local\",\n"
        f"{indent}  \"command\": [\"{server_binary}\"],\n"
        f"{indent}  \"enabled\": true\n"
        f"{indent}}}"
    )


def patch_opencode_config(path: str, server_binary: str, dry_run: bool) -> bool:
    """Add the server entry to an existing OpenCode config; returns changed."""
    raw = read_text(path)
    found = find_mcp_object(raw)
    if found is None:
        # No top-level mcp object: append one.
        sep = "\n" if raw.rstrip() else ""
        entry = (
            f"{sep}{{\n"
            f"  \"mcp\": {{\n"
            f"    \"{SERVER_NAME}\": {{\n"
            f"      \"type\": \"local\",\n"
            f"      \"command\": [\"{server_binary}\"],\n"
            f"      \"enabled\": true\n"
            f"    }}\n"
            f"  }}\n"
            f"}}"
        )
        new_raw = raw.rstrip() + "\n" + entry.strip() + "\n"
    else:
        open_idx, close_idx = found
        inner = raw[open_idx + 1 : close_idx]
        # Already present?
        if re.search(rf'"{re.escape(SERVER_NAME)}"\s*:', inner):
            return False
        entry = entry_text(server_binary, "    ")
        if inner.strip():
            new_inner = inner.rstrip() + ",\n" + entry
        else:
            new_inner = entry
        new_raw = raw[: open_idx + 1] + new_inner + raw[close_idx:]
    if not dry_run:
        write_text(path, new_raw)
    return True


def patch_omo_config(path: str, dry_run: bool) -> bool:
    """
    Add 'instant-file-search' to every sub-agent `mcps` list in the
    oh-my-opencode-slim config, skipping agents with a wildcard ["*"].
    Rewrites as plain JSON (same trade-off as install.ps1 ConvertTo-Json).
    """
    try:
        with open(path, "r", encoding="utf-8") as fh:
            text = fh.read()
        # Strip // and /* */ comments (outside strings) before json.loads.
        cleaned = re.sub(
            r'//[^\n]*|/\*.*?\*/', "", text, flags=re.DOTALL
        )
        config = json.loads(cleaned)
    except Exception as exc:  # noqa: BLE001
        print(f"  WARN: could not parse '{path}': {exc}; skipping OMO config.")
        return False

    changed = False
    presets = config.get("presets") if isinstance(config, dict) else None
    if not isinstance(presets, dict):
        print("  No presets found in OMO config; skipping.")
        return False

    for preset_name, preset in presets.items():
        if not isinstance(preset, dict):
            continue
        for agent_name, agent in preset.items():
            if not isinstance(agent, dict):
                continue
            mcps = agent.get("mcps")
            if not isinstance(mcps, list):
                continue
            if "*" in mcps:
                continue  # orchestrator with wildcard
            if SERVER_NAME in mcps:
                print(f"  preset '{preset_name}' / {agent_name}: already has '{SERVER_NAME}'")
                continue
            mcps.append(SERVER_NAME)
            changed = True
            print(f"  preset '{preset_name}' / {agent_name}: added '{SERVER_NAME}' to mcps")

    if changed and not dry_run:
        write_text(path, json.dumps(config, indent=2) + "\n")
    return changed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-binary", required=True,
                        help="absolute path to instant-file-search-mcp-server")
    parser.add_argument("--config-dir",
                        default=os.path.expanduser("~/.config/opencode"),
                        help="OpenCode config directory (default ~/.config/opencode)")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if not os.path.isabs(args.server_binary):
        print(f"error: --server-binary must be absolute: {args.server_binary}", file=sys.stderr)
        return 2
    if not os.path.isfile(args.server_binary):
        print(f"error: server binary not found: {args.server_binary}", file=sys.stderr)
        return 2

    os.makedirs(args.config_dir, exist_ok=True)

    # 1. OpenCode MCP entry.
    active = next((p for p in config_paths(args.config_dir) if os.path.isfile(p)), None)
    if active:
        if patch_opencode_config(active, args.server_binary, args.dry_run):
            print(f"PASS: added '{SERVER_NAME}' MCP entry to {active}")
        else:
            print(f"  {active}: already has '{SERVER_NAME}'; no changes needed.")
    else:
        target = config_paths(args.config_dir)[0]
        if not args.dry_run:
            write_text(target, json.dumps({
                "mcp": {
                    SERVER_NAME: {
                        "type": "local",
                        "command": [args.server_binary],
                        "enabled": True,
                    }
                }
            }, indent=2) + "\n")
        print(f"PASS: created {target} with '{SERVER_NAME}' MCP entry")

    # 2. OMO sub-agent mcps.
    omo_jsonc = os.path.join(args.config_dir, "oh-my-opencode-slim.jsonc")
    omo_json = os.path.join(args.config_dir, "oh-my-opencode-slim.json")
    omo = omo_jsonc if os.path.isfile(omo_jsonc) else (omo_json if os.path.isfile(omo_json) else None)
    if omo:
        changed = patch_omo_config(omo, args.dry_run)
        if changed:
            print(f"PASS: OMO sub-agent MCP access configured ({omo})")
        else:
            print(f"  {omo}: no OMO mcps changes needed.")
    else:
        print("  oh-my-opencode-slim config not found; skipping OMO configuration.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
