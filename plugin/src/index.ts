import type { Plugin } from "@opencode-ai/plugin";
import { tool } from "@opencode-ai/plugin";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "url";
import path from "path";

const z = tool.schema;

/**
 * Path to the Everything MCP server binary.
 * Adjust for your environment or use the EVERYTHING_MCP_BINARY env var.
 */
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const BINARY_PATH: string =
  process.env.EVERYTHING_MCP_BINARY ??
  path.join(__dirname, "..", "..", "target", "release", "everything-mcp-server.exe");

const CALL_TIMEOUT_MS = 30_000;

// ── MCP stdio transport (NDJSON — newline-delimited JSON) ────────
// The rmcp crate's transport-io uses NDJSON, NOT Content-Length framing.
// Each message is a single JSON line terminated by \n.

function sendFrame(child: ChildProcess, msg: object): void {
  child.stdin!.write(JSON.stringify(msg) + "\n");
}

function readNextMessage(
  child: ChildProcess,
  timeoutMs: number,
): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    let buf = "";
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error("MCP response timeout"));
    }, timeoutMs);

    const onData = (chunk: Buffer) => {
      buf += chunk.toString();
      const idx = buf.indexOf("\n");
      if (idx === -1) return;
      clearTimeout(timer);
      cleanup();
      resolve(buf.slice(0, idx));
    };

    const onError = (err: Error) => {
      clearTimeout(timer);
      cleanup();
      reject(err);
    };

    const cleanup = () => {
      child.stdout?.removeListener("data", onData);
      child.removeListener("error", onError);
    };

    child.stdout!.on("data", onData);
    child.on("error", onError);
  });
}

async function callMCP(
  toolName: string,
  args: Record<string, unknown>,
): Promise<string> {
  const child = spawn(BINARY_PATH, [], {
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });

  try {
    // 1. Initialize
    sendFrame(child, {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2024-11-05",
        capabilities: {},
        clientInfo: {
          name: "everything-mcp-plugin",
          version: "1.0.0",
        },
      },
    });
    const initRaw = await readNextMessage(child, CALL_TIMEOUT_MS);
    const initResp = JSON.parse(initRaw);
    if (initResp.error) {
      throw new Error(`MCP init error: ${initResp.error.message}`);
    }

    // 2. Send initialized notification (no response expected)
    sendFrame(child, {
      jsonrpc: "2.0",
      method: "notifications/initialized",
    });

    // 3. Call the tool
    sendFrame(child, {
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: { name: toolName, arguments: args },
    });
    const toolRaw = await readNextMessage(child, CALL_TIMEOUT_MS);
    const toolResp = JSON.parse(toolRaw);
    if (toolResp.error) {
      throw new Error(`MCP ${toolName} error: ${toolResp.error.message}`);
    }

    const content: Array<{ type: string; text: string }> =
      toolResp.result?.content ?? [];
    return content.map((c) => c.text).join("\n");
  } finally {
    child.stdin!.end();
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        child.kill();
        resolve();
      }, 2_000);
      child.on("exit", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
}

// ── OpenCode plugin tools ────────────────────────────────────────

const plugin: Plugin = async (_input, _options) => {
  return {
    tool: {
      find_files: tool({
        description:
          "INSTANT file/directory search using Everything engine (NTFS index). " +
          "Fastest way to find files on this PC. Default scope: ALL indexed drives. " +
          "Supports regex, sort, path filter, exclude_path, selective fields, " +
          "match_case/match_whole_word/match_path, and pagination via offset. " +
          "Response includes: results, total, returned, offset, note. " +
          "IMPORTANT: for broad patterns, call count_files FIRST to gauge result size. " +
          "Use exclude_path to skip node_modules, WinSxS, etc. " +
          "Pass include_all=true to search without auto-exclusions.",
        args: {
          query: z
            .string()
            .min(1)
            .describe(
              "Search query. Supports Everything modifiers: file:*.ts, dm:today, " +
                "size:>1mb, folder:src, dupe:filename, and more.",
            ),
          path: z
            .string()
            .optional()
            .describe("Scope search to a directory path. Pass as double-backslash path."),
          exclude_path: z
            .string()
            .optional()
            .describe('Exclude directory paths (e.g. "node_modules;WinSxS;.git").'),
          include_all: z
            .boolean()
            .optional()
            .describe(
              "When true, search without auto-excluding noise folders " +
                "(node_modules, WinSxS, .git). Default: false.",
            ),
          regex: z
            .boolean()
            .optional()
            .describe("Enable regex mode. Default: false (plain text / wildcard)."),
          match_case: z
            .boolean()
            .optional()
            .describe("Case-sensitive search. Default: false (case-insensitive)."),
          match_whole_word: z
            .boolean()
            .optional()
            .describe("Match whole words only. Default: false (match substrings)."),
          match_path: z
            .boolean()
            .optional()
            .describe(
              "Match against the full path, not just the filename. Default: false.",
            ),
          max_results: z
            .number()
            .max(100)
            .optional()
            .describe("Maximum number of results to return. Default and max: 100."),
          offset: z
            .number()
            .optional()
            .describe(
              "Pagination offset. Use with max_results: offset=0 (page 1), " +
                "offset=100 (page 2), etc. Default: 0.",
            ),
          sort: z
            .string()
            .optional()
            .describe(
              "Sort order: name, name_desc, path, path_desc, size, size_asc, " +
                "date_modified, date_modified_asc, date_created, date_created_asc, " +
                "date_accessed, date_accessed_asc, extension, extension_desc, " +
                "run_count, run_count_asc, date_run, date_run_asc, " +
                "type_name, type_name_desc, date_recently_changed, " +
                "date_recently_changed_asc. Default: name.",
            ),
          fields: z
            .string()
            .optional()
            .describe(
              "Comma-separated fields: filename, path, size, date_modified, " +
                "date_created, date_accessed, attributes, extension, run_count, date_run, " +
                "date_recently_changed, file_list_filename. Default: all common fields.",
            ),
        },
        execute: async (args, _context) => {
          const text = await callMCP("find_files", args as Record<string, unknown>);
          return text;
        },
      }),

      count_files: tool({
        description:
          "INSTANT count of matching files using Everything engine (NTFS index). " +
          "Returns total count without transferring file data. " +
          "Default scope: ALL indexed drives. Pass path to narrow. " +
          "Supports regex, match_case, match_whole_word, exclude_path, include_all. " +
          "Use this FIRST for broad patterns (e.g. *.tmp, *.json) " +
          "to gauge total before calling find_files.",
        args: {
          query: z
            .string()
            .min(1)
            .describe("Search query. Supports Everything modifiers same as find_files."),
          path: z
            .string()
            .optional()
            .describe("Scope to a directory path. Speeds up results."),
          exclude_path: z
            .string()
            .optional()
            .describe('Exclude paths (e.g. "node_modules;WinSxS;.git").'),
          include_all: z
            .boolean()
            .optional()
            .describe(
              "When true, count without auto-excluding noise folders. Default: false.",
            ),
          regex: z
            .boolean()
            .optional()
            .describe("Enable regex mode. Default: false."),
          match_case: z
            .boolean()
            .optional()
            .describe("Case-sensitive count. Default: false."),
          match_whole_word: z
            .boolean()
            .optional()
            .describe("Match whole words only. Default: false."),
        },
        execute: async (args, _context) => {
          const text = await callMCP("count_files", args as Record<string, unknown>);
          return text;
        },
      }),

      search_status: tool({
        description:
          "Check if Everything search engine is running and IPC is connected. " +
          "Returns detailed diagnostics including window status, IPC availability, " +
          "and DB load state. Call this before using find_files or count_files " +
          "to verify the engine is available.",
        args: {},
        execute: async (_args, _context) => {
          const text = await callMCP("search_status", {});
          return text;
        },
      }),
    },
  };
};

export default plugin;
