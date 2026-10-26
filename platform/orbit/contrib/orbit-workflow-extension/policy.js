/**
 * Orbit workflow policy — pure, offline-testable decisions.
 *
 * A session runs in one of three workflow modes. This module answers, for a
 * given mode, which tools stay active, whether a tool call must be blocked,
 * and what prompt guidance to inject.
 *
 *   plan   read-only exploration; produce a numbered plan under a `Plan:` header
 *   build  normal agent work (default)
 *   ask    read-only question answering
 *
 * Plan and Ask disable the write tools and gate bash to a read-only allowlist.
 * This is a guard, not a sandbox — pi ships none, and Orbit does not add one.
 *
 * The mode itself lives per session in `~/.orbit-pi/workflow.json` (a map of
 * pi session id → wire id) and is read fresh on every hook, so a change in the
 * app re-arms a live session without a restart.
 */

/** Fallback when nothing is stored, readable, or known. */
export const DEFAULT_MODE = "build";

/** Every mode the app can write. Kept in sync with `crate::workflow`. */
export const MODES = ["plan", "build", "ask"];

/** Custom session-entry type recording the mode (survives resume). */
export const WORKFLOW_ENTRY_TYPE = "orbit:workflow";

/** Custom message type carrying per-mode prompt guidance. */
export const CONTEXT_TYPE = "orbit-workflow-context";

/** Tools forced off per mode. Unknown names are ignored by pi. */
export const DISABLED_TOOLS = {
  plan: ["edit", "write"],
  ask: ["edit", "write"],
  build: [],
};

/** Tools that execute a process, where bash is gated by the allowlist. */
const EXEC_TOOLS = new Set(["bash", "powershell", "shell", "exec", "terminal"]);

/** Coerce an arbitrary value to a known mode, falling back to the default. */
export function normalizeMode(raw) {
  return typeof raw === "string" && MODES.includes(raw) ? raw : DEFAULT_MODE;
}

/** A readable label for a block reason: `plan` → `Plan`. */
function label(mode) {
  return normalizeMode(mode).replace(/^./, (c) => c.toUpperCase());
}

/**
 * The tools a mode leaves active, derived from the base set by *subtracting*
 * the disabled names. Never hardcode the full set — pi owns tool names.
 */
export function allowedTools(mode, baseTools = []) {
  const disabled = new Set(DISABLED_TOOLS[normalizeMode(mode)] ?? []);
  const seen = new Set();
  const out = [];
  for (const name of baseTools) {
    if (typeof name !== "string" || disabled.has(name) || seen.has(name)) continue;
    seen.add(name);
    out.push(name);
  }
  return out;
}

/**
 * The next active set for `mode`, derived read-modify-write from the tools
 * active *now* plus the names this extension previously removed.
 *
 * A session-start snapshot is not a safe base: other extensions activate
 * tools after `session_start` — the MCP extension turns on `codemode` (or
 * `tool_search`) when a server with that exposure connects — and a cached
 * snapshot would silently drop them on the next mode change.
 *
 * Returns the new tool list and the updated set of names this extension has
 * removed, so switching to a mode that allows them restores exactly what it
 * took away (and never re-adds a tool another extension removed).
 */
export function applyToolScoping(mode, current = [], removedByUs = []) {
  const disabled = new Set(DISABLED_TOOLS[normalizeMode(mode)] ?? []);
  const removed = new Set(removedByUs);
  const restore = [];
  for (const name of removed) {
    if (disabled.has(name)) continue;
    restore.push(name);
    removed.delete(name);
  }
  const base = [...new Set([...current, ...restore])];
  const tools = allowedTools(mode, base);
  for (const name of base) {
    if (!tools.includes(name)) removed.add(name);
  }
  return { tools, removed: [...removed] };
}

// Destructive shell operations. A conservative backstop to the allowlist: if
// any matches, the command is blocked even when it also looks safe.
const DESTRUCTIVE_PATTERNS = [
  /\brm\b/i,
  /\brmdir\b/i,
  /\bmv\b/i,
  /\bcp\b/i,
  /\bmkdir\b/i,
  /\btouch\b/i,
  /\bchmod\b/i,
  /\bchown\b/i,
  /\bln\b/i,
  /\btee\b/i,
  /\btruncate\b/i,
  /\bdd\b/i,
  /\bnpm\s+(install|uninstall|update|ci|link|publish)/i,
  /\byarn\s+(add|remove|install|publish)/i,
  /\bpnpm\s+(add|remove|install|publish)/i,
  /\bpip\s+(install|uninstall)/i,
  /\bbrew\s+(install|uninstall|upgrade)/i,
  /\bgit\s+(add|commit|push|pull|merge|rebase|reset|checkout|stash|cherry-pick|revert|tag|init|clone)/i,
  /\bgit\s+branch\s+-[dD]/i,
  /\bsudo\b/i,
  /\bsu\b/i,
  /\bkill\b/i,
  /\bpkill\b/i,
  /\bkillall\b/i,
  /\breboot\b/i,
  /\bshutdown\b/i,
  /\b(vim?|nano|emacs|code|subl)\b/i,
];

// Read-only commands allowed in plan/ask mode. Anchored at the start.
const SAFE_PATTERNS = [
  /^\s*cat\b/, /^\s*head\b/, /^\s*tail\b/, /^\s*less\b/, /^\s*more\b/,
  /^\s*grep\b/, /^\s*rg\b/, /^\s*find\b/, /^\s*fd\b/, /^\s*ls\b/, /^\s*pwd\b/,
  /^\s*tree\b/, /^\s*wc\b/, /^\s*file\b/, /^\s*stat\b/, /^\s*du\b/, /^\s*df\b/,
  /^\s*sort\b/, /^\s*uniq\b/, /^\s*diff\b/, /^\s*which\b/, /^\s*whereis\b/,
  /^\s*uname\b/, /^\s*whoami\b/, /^\s*id\b/, /^\s*date\b/, /^\s*uptime\b/,
  /^\s*ps\b/, /^\s*env\b/, /^\s*printenv\b/, /^\s*bat\b/, /^\s*eza\b/, /^\s*jq\b/,
  /^\s*sed\s+-n/i,
  /^\s*git\s+(status|log|diff|show|blame|rev-parse|ls-files|ls-tree|ls-remote|remote|config\s+--get)/i,
  /^\s*git\s+branch(?!\s+-[dD])/i,
  /^\s*npm\s+(list|ls|view|info|search|outdated|audit)/i,
  /^\s*yarn\s+(list|info|why|audit)/i,
  /^\s*node\s+--version/i,
  /^\s*python\d?\s+--version/i,
];

/**
 * Whether a bash command is safe under plan/ask mode.
 *
 * Rejects compound commands (`;`, `&&`, `||`, `|`), redirects (`>`, `<`),
 * command substitution (`$(`, backticks), subshells, and newlines outright —
 * a false *allow* is worse than a false block. Then it must match the
 * read-only allowlist and no destructive pattern.
 */
export function isSafeCommand(command) {
  const cmd = String(command ?? "");
  if (cmd.trim() === "") return false;
  if (/[;&|`<>\n]|\$\(|[()]/.test(cmd)) return false;
  if (DESTRUCTIVE_PATTERNS.some((re) => re.test(cmd))) return false;
  return SAFE_PATTERNS.some((re) => re.test(cmd));
}

/**
 * Why a tool call is blocked under `mode`, or `undefined` when it may run.
 * `build` never blocks; plan/ask block the disabled write tools and any bash
 * command outside the read-only allowlist.
 */
export function blockReason(mode, toolName, input) {
  const resolved = normalizeMode(mode);
  if (resolved === "build") return undefined;

  const name = String(toolName ?? "").toLowerCase();
  if ((DISABLED_TOOLS[resolved] ?? []).includes(name)) {
    return `${label(resolved)} mode: ${name} is disabled. Switch to Build to make changes.`;
  }
  if (EXEC_TOOLS.has(name)) {
    const command = input?.command ?? input?.cmd ?? "";
    if (!isSafeCommand(command)) {
      return `${label(resolved)} mode: command blocked (not on the read-only allowlist). Switch to Build to run it.`;
    }
  }
  return undefined;
}

/**
 * The system-prompt addendum for a mode, or `""` when there is nothing to add
 * (Build keeps pi's own prompt untouched).
 */
export function guidance(mode) {
  const resolved = normalizeMode(mode);
  if (resolved === "plan") {
    return [
      "[WORKFLOW: PLAN MODE]",
      "You are in plan mode — read-only exploration for safe code analysis.",
      "Do not make changes: write tools are disabled and shell commands are limited to a read-only allowlist.",
      "Investigate, then present a numbered plan under a `Plan:` header, one line per step, e.g.",
      "Plan:",
      "1. First step",
      "2. Second step",
    ].join("\n");
  }
  if (resolved === "ask") {
    return [
      "[WORKFLOW: ASK MODE]",
      "Answer the user's question from the codebase. Do not modify anything — write tools are disabled and shell commands are limited to a read-only allowlist.",
      "Do not produce an implementation plan unless the user asks for one.",
    ].join("\n");
  }
  return "";
}
