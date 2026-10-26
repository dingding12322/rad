/**
 * Orbit's access-mode guard (pi extension).
 *
 * Orbit loads this file with `pi --extension <path>` on every session it
 * spawns. It hooks pi's `tool_call` event — which runs after a tool call is
 * proposed and can block it — and, for any mutating call the active access
 * mode does not auto-approve, asks the user through `ctx.ui.select()`. In RPC
 * mode that becomes an `extension_ui_request`; Orbit renders it as an inline
 * bar above the composer and answers it over stdin.
 *
 * The offered options are Allow once / Always allow this tool / Deny.
 * "Always allow" is recorded per mode in `~/.orbit-pi/access-allow.json`. The
 * mode lives in `~/.orbit-pi/access.json` and is read fresh on every tool
 * call, so a change in Orbit re-arms live sessions with no restart. See
 * `policy.js` for the decision table.
 *
 * Fail-safe: if no UI is available to confirm (print/json mode) or the prompt
 * throws/returns nothing, the call is blocked rather than allowed.
 */
import {
  CONFIRM_OPTIONS,
  OPTION_ALLOW_ONCE,
  OPTION_ALWAYS_ALLOW,
  addToAllowlist,
  decide,
  formatTitle,
  readAllowlist,
  readMode,
  summarize,
} from "./policy.js";

/**
 * Prompts are serialized: pi can preflight several sibling tool calls from one
 * assistant message, and Orbit renders one approval at a time. Chaining the
 * prompts guarantees each is answered before the next is shown, instead of a
 * second request arriving while the first is open and being auto-cancelled.
 */
let promptChain = Promise.resolve();

function serializePrompt(run) {
  const result = promptChain.then(run, run);
  // Keep the chain alive regardless of how the individual prompt settles.
  promptChain = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

export default function activate(pi) {
  pi.on("tool_call", async (event, ctx) => {
    const mode = readMode();
    const tool = String(event.toolName ?? "");
    if (decide(mode, tool, readAllowlist(mode)) === "allow") return;

    // No UI (print / json mode): there is no way to ask, so deny rather than
    // silently run a mutating tool the current mode wants confirmed.
    if (!ctx?.hasUI) {
      return { block: true, reason: "Blocked: Orbit has no UI to confirm this action" };
    }

    let choice;
    try {
      const title = formatTitle(tool, summarize(tool, event.input));
      choice = await serializePrompt(() => ctx.ui.select(title, CONFIRM_OPTIONS));
    } catch {
      choice = undefined;
    }

    if (choice === OPTION_ALWAYS_ALLOW) {
      addToAllowlist(mode, tool);
      return;
    }
    if (choice === OPTION_ALLOW_ONCE) return;
    return { block: true, reason: "Denied in Orbit" };
  });
}
