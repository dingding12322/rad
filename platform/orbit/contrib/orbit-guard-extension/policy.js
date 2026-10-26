/**
 * Orbit guard policy — pure, offline-testable access-mode decisions.
 *
 * Orbit runs with one of three access modes. For every tool call pi emits on
 * its `tool_call` hook, this module answers whether the call is allowed
 * outright or must be confirmed by the user:
 *
 *   supervised        read → allow · edit/exec/other → ask
 *   auto-accept-edits read/edit → allow · exec/other → ask
 *   full-access       allow everything (no prompts)
 *
 * A user who answers "Always allow this tool" is recorded in
 * `~/.orbit-pi/access-allow.json` under the active mode, so the same tool in
 * that mode no longer asks. The mode itself lives in `~/.orbit-pi/access.json`
 * and is read fresh on every tool call, so changing it in the app re-arms live
 * sessions without a restart.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/** Fallback when the file is missing, unreadable, or names an unknown mode. */
export const DEFAULT_MODE = "full-access";

/** Every mode Orbit can write. Kept in sync with `crate::access::AccessMode`. */
export const MODES = ["supervised", "auto-accept-edits", "full-access"];

/**
 * Marker Orbit's dialog router looks for: a `select` whose title carries it is
 * rendered as the inline access-guard bar instead of the blocking modal. The
 * title is `PREFIX + tool + "\t" + detail`. Kept in sync with
 * `crate::dialog::GUARD_TITLE_PREFIX`.
 */
export const GUARD_TITLE_PREFIX = "[orbit-guard] ";

/** Options offered for a confirmation-required call. */
export const OPTION_ALLOW_ONCE = "Allow once";
export const OPTION_ALWAYS_ALLOW = "Always allow this tool";
export const OPTION_DENY = "Deny";

/** The option list, in display order. */
export const CONFIRM_OPTIONS = [OPTION_ALLOW_ONCE, OPTION_ALWAYS_ALLOW, OPTION_DENY];

/** Tools that only observe: they never mutate the workspace. */
const READ_TOOLS = new Set(["read", "grep", "find", "ls", "glob", "list", "webfetch"]);

/** Tools that write files. */
const EDIT_TOOLS = new Set(["edit", "write", "multiedit", "apply_patch", "str_replace_editor"]);

/** Tools that run a process / shell command. */
const EXEC_TOOLS = new Set(["bash", "powershell", "shell", "exec", "terminal"]);

/**
 * Pi names every MCP tool `mcp__<server>__<tool>`. They are treated as their
 * own kind so the confirmation can name the server, and they are always asked
 * about in the confined modes: an MCP call can run a local process or change
 * an external service, so nothing about it is auto-approved except through
 * the user's explicit "Always allow this tool" entry.
 */
const MCP_PREFIX = "mcp__";

/**
 * Coerce an arbitrary value to a known mode, falling back to
 * [`DEFAULT_MODE`]. Unknown values never allow more than intended.
 */
export function normalizeMode(raw) {
  return typeof raw === "string" && MODES.includes(raw) ? raw : DEFAULT_MODE;
}

/**
 * Classify a pi tool name into a risk kind: `"read"`, `"edit"`, `"exec"`,
 * `"mcp"`, or `"other"` (extension / custom tools, which are treated
 * conservatively).
 */
export function classify(toolName) {
  const name = String(toolName ?? "").toLowerCase();
  if (READ_TOOLS.has(name)) return "read";
  if (EDIT_TOOLS.has(name)) return "edit";
  if (EXEC_TOOLS.has(name)) return "exec";
  if (name.startsWith(MCP_PREFIX)) return "mcp";
  return "other";
}

/**
 * The decision for one tool call under `mode`: `"allow"` or `"ask"`. A tool in
 * the mode's allowlist is always allowed. There is intentionally no `"block"` —
 * the guard prompts rather than silently denying, and a failed prompt is
 * treated as a denial by the caller (fail-safe).
 */
export function decide(mode, toolName, allowlist = []) {
  const name = String(toolName ?? "");
  if (allowlist.includes(name)) return "allow";
  const kind = classify(name);
  const resolved = normalizeMode(mode);
  if (resolved === "full-access") return "allow";
  // Reads are non-mutating, so every confined mode allows them.
  if (kind === "read") return "allow";
  if (resolved === "auto-accept-edits" && kind === "edit") return "allow";
  return "ask";
}

/** The path Orbit writes the active mode to. */
export function modeFilePath() {
  const home = process.env.HOME || process.env.USERPROFILE || os.homedir() || ".";
  return path.join(home, ".orbit-pi", "access.json");
}

/** The path the "always allow" decisions live in. */
export function allowlistPath() {
  const home = process.env.HOME || process.env.USERPROFILE || os.homedir() || ".";
  return path.join(home, ".orbit-pi", "access-allow.json");
}

/**
 * Read the active mode from disk. Read fresh on every call so a change in the
 * app applies to a running session; any failure degrades to
 * [`DEFAULT_MODE`].
 */
export function readMode(filePath = modeFilePath()) {
  try {
    const parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
    return normalizeMode(parsed?.mode);
  } catch {
    return DEFAULT_MODE;
  }
}

/** Tool names the user chose "Always allow" for under `mode`. */
export function readAllowlist(mode, filePath = allowlistPath()) {
  try {
    const parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
    const list = parsed?.[normalizeMode(mode)];
    return Array.isArray(list) ? list.filter((name) => typeof name === "string") : [];
  } catch {
    return [];
  }
}

/** Record `toolName` as always-allowed under `mode`. Best-effort. */
export function addToAllowlist(mode, toolName, filePath = allowlistPath()) {
  const resolved = normalizeMode(mode);
  let all = {};
  try {
    all = JSON.parse(fs.readFileSync(filePath, "utf8")) ?? {};
  } catch {
    all = {};
  }
  const list = new Set(Array.isArray(all[resolved]) ? all[resolved] : []);
  list.add(String(toolName));
  all[resolved] = [...list];
  try {
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, JSON.stringify(all));
  } catch {
    // Best-effort: a failed write just means the prompt appears again.
  }
}

/** Build the guard title Orbit recognizes: `PREFIX + tool + "\t" + detail`. */
export function formatTitle(toolName, detail) {
  return `${GUARD_TITLE_PREFIX}${toolName}\t${detail}`;
}

/** A short, single-line summary of a tool call's salient argument. */
export function summarize(toolName, input) {
  const value = input ?? {};
  const first = (...keys) => {
    for (const key of keys) {
      if (typeof value[key] === "string" && value[key].trim() !== "") return value[key];
    }
    return null;
  };
  const detail =
    first("command", "path", "file_path", "filename", "pattern", "url", "query") ??
    JSON.stringify(value);
  const text = String(detail ?? "").replace(/\s+/g, " ").trim();
  return text.length > 200 ? `${text.slice(0, 200)}…` : text;
}
