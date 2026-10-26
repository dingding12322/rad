/**
 * Orbit's provider-quota bridge (pi extension).
 *
 * Orbit loads this file with `pi --extension <path>` on every session it
 * spawns. It fetches each configured provider's account quota through pi's
 * own credential resolution and appends one normalized snapshot as a custom
 * session entry (`orbit:quota`). Custom entries never enter the model's
 * context. Orbit reads them over RPC (`get_entries`, incremental `since`
 * cursor) and renders them in the top-bar quota pill/popover and
 * Settings → Providers — with no patching of pi and no settings writes.
 *
 * The snapshot payload matches `crates/orbit-rpc/docs/quota-rpc.md`:
 *   { providers: [ { provider, kind, plan?, windows[], balances[], … } ] }
 *
 * A snapshot is appended only when it changes (ignoring fetch timestamps), so
 * a quiet account does not grow session files. Quota endpoints rate-limit
 * aggressively, so refreshes are event-driven (session start, turn end) plus
 * a slow idle timer.
 */
import {
  configureQuotaAuth,
  OLLAMA_SESSION_KEY,
  quotaAdapters,
  quotaReports,
  quotaStoredAuth,
} from "./adapters.js";

/** Custom session-entry type Orbit reads. */
export const ENTRY_TYPE = "orbit:quota";

/** Idle refresh cadence. Quota windows move slowly; endpoints rate-limit. */
const REFRESH_INTERVAL_MS = 5 * 60_000;

/** Minimum gap between turn-driven refreshes. */
const TURN_REFRESH_GAP_MS = 60_000;

/** Provider ids the bridge knows how to query; anything else configured is
 *  reported as generically unsupported, matching the `quota.list` patch. */
const KNOWN_PROVIDERS = Object.keys(quotaAdapters);

/** A snapshot's stable identity: `fetchedAt` changes on every fetch. */
function fingerprint(data) {
  return JSON.stringify(data, (key, value) =>
    key === "fetchedAt" ? undefined : value,
  );
}

/** A stale activation context is expected during session switches. */
function isStaleContext(error) {
  return String(error?.message ?? error).includes("This extension ctx is stale");
}

/**
 * The most recent background snapshot. pi awaits async lifecycle handlers, so
 * `session_start`/`turn_end` must NOT await the network fetch — doing so makes
 * every `switch_session` wait out the provider round-trips (measured ~2.5s
 * before this). The fetch runs detached; Orbit picks the entry up on its
 * `get_entries` poll. Tests await this to stay deterministic.
 */
let inflight = Promise.resolve();

export function pendingSnapshot() {
  return inflight;
}

export default async function activate(pi) {
  let ctx = null;
  let timer = null;
  let lastFingerprint = "";
  let lastSnapshotAt = 0;
  let stopped = false;

  configureQuotaAuth(async (providerId) => {
    const current = ctx;
    if (!current) return null;
    try {
      return (await current.modelRegistry?.getProviderAuth?.(providerId)) ?? null;
    } catch {
      return null;
    }
  });

  /** Every provider with a credential: the adapter set plus whatever else pi
   *  has registered, filtered by pi's own "configured" test. */
  async function configuredProviders(current) {
    const registered = new Set();
    try {
      for (const id of current.modelRegistry?.getRegisteredProviderIds?.() ?? []) {
        registered.add(id);
      }
    } catch {
      // Older pi without registry enumeration: the adapter set still works.
    }
    const ids = new Set([...KNOWN_PROVIDERS, ...registered]);
    let stored = {};
    try {
      stored = (await quotaStoredAuth()) ?? {};
    } catch {
      stored = {};
    }
    const out = [];
    for (const id of ids) {
      let configured = false;
      try {
        configured = current.modelRegistry?.hasConfiguredAuth?.({ provider: id }) === true;
      } catch {
        configured = false;
      }
      if (configured || stored[id] != null) out.push(id);
    }
    // A cookie-only Ollama Cloud setup stores the session under its own
    // non-provider key: pi sees no provider credential, so `hasConfiguredAuth`
    // would otherwise exclude it and the usage would never load. One adapter
    // id is enough — prefer the third-party provider's `ollama-cloud` when it
    // is registered, else the local endpoint's `ollama`.
    if (stored[OLLAMA_SESSION_KEY] != null) {
      const hasOllama = out.some((id) => id === "ollama" || id === "ollama-cloud");
      if (!hasOllama) out.push(registered.has("ollama-cloud") ? "ollama-cloud" : "ollama");
    }
    return out;
  }

  async function snapshot() {
    if (stopped || !ctx) return;
    const current = ctx;
    try {
      const ids = await configuredProviders(current);
      if (ids.length === 0) return;
      const reports = await quotaReports(ids);
      if (stopped || ctx !== current) return;
      const data = { providers: reports, source: "orbit-quota-bridge" };
      lastSnapshotAt = Date.now();
      const next = fingerprint(data);
      if (next === lastFingerprint) return;
      lastFingerprint = next;
      pi.appendEntry(ENTRY_TYPE, data);
    } catch (error) {
      // Never let a quota fetch break the session. Stale activation contexts
      // are expected while switching sessions; a real failure is worth a line.
      if (!isStaleContext(error)) {
        console.error(
          "[orbit-quota] snapshot failed:",
          error instanceof Error ? error.message : String(error),
        );
      }
    }
  }

  pi.on("session_start", async (_event, eventCtx) => {
    ctx = eventCtx;
    stopped = false;
    // A new session always gets a fresh snapshot, even if the values match
    // the previous one: the new session file has no entry yet.
    lastFingerprint = "";
    if (timer) clearInterval(timer);
    timer = setInterval(() => void snapshot(), REFRESH_INTERVAL_MS);
    timer.unref?.();
    // Detached: never make pi wait on quota before it answers the RPC that
    // opened the session. The entry lands moments later for Orbit's poll.
    inflight = snapshot();
  });

  pi.on("session_shutdown", async () => {
    stopped = true;
    if (timer) clearInterval(timer);
    timer = null;
    ctx = null;
  });

  pi.on("turn_end", async (_event, eventCtx) => {
    ctx = eventCtx;
    if (Date.now() - lastSnapshotAt < TURN_REFRESH_GAP_MS) return;
    // Detached for the same reason as session_start: a turn must not wait on
    // provider quota before pi processes the next command.
    inflight = snapshot();
  });

  pi.on("model_select", async (_event, eventCtx) => {
    // A model switch can make another provider relevant; keep the context
    // fresh and let the next session-start/turn-end/timer snapshot include it.
    ctx = eventCtx;
  });
}
