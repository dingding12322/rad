/**
 * Orbit workflow mode (pi extension).
 *
 * Orbit loads this file with `pi --extension <path>` on every session it
 * spawns. It scopes the session to Plan, Build, or Ask:
 *
 *   - `pi.setActiveTools()` drops the write tools in Plan/Ask.
 *   - a `tool_call` hook blocks write tools and non-allowlisted bash as a
 *     fail-safe (a tool pi preflighted before the tool set changed).
 *   - `before_agent_start` injects hidden guidance (produce a plan; or answer
 *     only).
 *
 * The mode lives per session in `~/.orbit-pi/workflow.json` (a map of pi
 * session id → wire id) and is read **fresh on every hook**, so changing it in
 * Orbit re-arms a live session with no restart. An `ORBIT_WORKFLOW_MODE` env
 * var covers the moment before the store has the id. The mode is also written
 * into the session as an `orbit:workflow` entry so it survives resume even if
 * the store is lost; the pure pieces live in `policy.js`.
 *
 * Fail-safe: any resolve/read failure degrades to Build, never to a silently
 * weaker read-only state.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  CONTEXT_TYPE,
  DEFAULT_MODE,
  WORKFLOW_ENTRY_TYPE,
  applyToolScoping,
  blockReason,
  guidance,
  normalizeMode,
} from "./policy.js";

/** `~/.orbit-pi/workflow.json`, resolved per call so a test `HOME` is honored. */
function storePath() {
  return path.join(os.homedir(), ".orbit-pi", "workflow.json");
}

/** The mode stored for `sessionId`, or `undefined` when absent/unreadable. */
export function readStoredMode(sessionId, filePath = storePath()) {
  if (!sessionId) return undefined;
  try {
    const map = JSON.parse(fs.readFileSync(filePath, "utf8"));
    const wire = map?.[sessionId];
    return typeof wire === "string" ? normalizeMode(wire) : undefined;
  } catch {
    return undefined;
  }
}

function branchEntries(ctx) {
  try {
    return ctx?.sessionManager?.getBranch?.() ?? [];
  } catch {
    return [];
  }
}

/** The newest custom entry of `customType` in `entries`, or `undefined`. */
export function lastCustom(entries, customType) {
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const entry = entries[i];
    if (entry?.type === "custom" && entry.customType === customType) return entry.data;
  }
  return undefined;
}

/** Resolve the mode: store → env → session entry → Build. */
export function resolveMode(ctx, entries) {
  const sessionId = ctx?.sessionManager?.getSessionId?.();
  const stored = readStoredMode(sessionId);
  if (stored) return stored;
  if (typeof process.env.ORBIT_WORKFLOW_MODE === "string") {
    return normalizeMode(process.env.ORBIT_WORKFLOW_MODE);
  }
  const entry = lastCustom(entries ?? [], WORKFLOW_ENTRY_TYPE);
  if (typeof entry?.mode === "string") return normalizeMode(entry.mode);
  return DEFAULT_MODE;
}

export default function activate(pi) {
  let appliedMode = null;
  /**
   * Tool names this extension removed from the active set. Tracked across
   * mode changes so Build restores exactly what Plan/Ask took away, while
   * tools another extension owns (MCP's `codemode` / `tool_search`) are
   * never touched.
   */
  let removedByUs = [];

  /**
   * Apply a mode by read-modify-write on the *current* active set. The
   * active set grows after session start (MCP activates `codemode` when a
   * server connects), so the base is always what is live now — never a
   * cached session-start snapshot.
   */
  function applyMode(mode) {
    let current;
    try {
      current = pi.getActiveTools();
    } catch {
      current = [];
    }
    const next = applyToolScoping(mode, current, removedByUs);
    removedByUs = next.removed;
    try {
      pi.setActiveTools(next.tools);
    } catch {
      // A tool-set change is best-effort; the tool_call hook still blocks.
    }
    if (mode !== appliedMode) {
      appliedMode = mode;
      try {
        pi.appendEntry(WORKFLOW_ENTRY_TYPE, { mode });
      } catch {
        // Persistence is best-effort; the store remains authoritative.
      }
    }
  }

  pi.on("session_start", (_event, ctx) => {
    applyMode(resolveMode(ctx, branchEntries(ctx)));
  });

  pi.on("before_agent_start", (_event, ctx) => {
    const mode = resolveMode(ctx, branchEntries(ctx));
    applyMode(mode);
    const content = guidance(mode);
    if (!content) return undefined;
    return { message: { customType: CONTEXT_TYPE, content, display: false } };
  });

  // Fail-safe: block write tools and unsafe bash in read-only modes even if
  // the tool set still lists them.
  pi.on("tool_call", (event, ctx) => {
    const mode = resolveMode(ctx, branchEntries(ctx));
    const reason = blockReason(mode, event?.toolName, event?.input);
    if (reason) return { block: true, reason };
    return undefined;
  });

  // Drop stale injected guidance once the mode no longer produces any — a
  // Build session leaves nothing to say, so old instructions must not keep
  // steering the run. While guidance is active, leave it alone: `context` can
  // run after `before_agent_start` injected this turn's copy.
  pi.on("context", (event, ctx) => {
    const mode = resolveMode(ctx, branchEntries(ctx));
    if (guidance(mode) !== "") return undefined;
    const messages = Array.isArray(event?.messages) ? event.messages : [];
    const filtered = messages.filter((message) => message?.customType !== CONTEXT_TYPE);
    if (filtered.length !== messages.length) return { messages: filtered };
    return undefined;
  });
}
