/**
 * Orbit auto-title — pure helpers.
 *
 * Orbit bundles the companion `index.js` with `pi --extension` on every
 * session it spawns. After the first turn settles, that extension asks a
 * model for a short session title and sets it with `pi.setSessionName`; pi
 * persists a `session_info` entry and emits `session_info_changed`, which
 * Orbit renders live.
 *
 * Everything testable lives here (no pi, no filesystem): config parsing,
 * exchange extraction, prompt building, model choice, and title
 * normalization. `title.test.mjs` covers it with `node --test`.
 */

/** Config filename Orbit writes under `~/.orbit-pi/`. */
export const CONFIG_FILE = "auto-title.json";

/** Defaults when the file is missing or unreadable: on, active model. */
export const DEFAULT_CONFIG = { enabled: true, model: null };

/**
 * Parse `~/.orbit-pi/auto-title.json`. Unknown shapes fall back to the
 * defaults rather than throwing: a malformed setting must never stop a
 * session from running.
 */
export function parseConfig(raw) {
  if (typeof raw !== "string" || raw.trim() === "") {
    return { ...DEFAULT_CONFIG };
  }
  let value;
  try {
    value = JSON.parse(raw);
  } catch {
    return { ...DEFAULT_CONFIG };
  }
  if (!value || typeof value !== "object") {
    return { ...DEFAULT_CONFIG };
  }
  const enabled = value.enabled !== false; // only an explicit false disables
  const model = value.model;
  const normalizedModel =
    model &&
    typeof model.provider === "string" &&
    typeof model.id === "string" &&
    model.provider.length > 0 &&
    model.id.length > 0
      ? { provider: model.provider, id: model.id }
      : null;
  return { enabled, model: normalizedModel };
}

/** The text of one message's content (string or block array). */
export function extractText(content) {
  if (typeof content === "string") {
    return content.trim();
  }
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .filter((block) => block && block.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join("\n")
    .trim();
}

/**
 * The session's first user prompt and first assistant reply, plus how many
 * user prompts the branch holds. A turn that calls tools streams several
 * assistant messages, so assistant messages cannot tell a fresh session from
 * a resumed one — user prompts can: `userCount === 1` means the session's
 * very first exchange (titleable); more means prompts existed before this
 * extension looked (resumed) and must not be re-titled.
 */
export function firstExchange(entries) {
  let user = "";
  let assistant = "";
  let userCount = 0;
  for (const entry of entries ?? []) {
    if (!entry || entry.type !== "message" || !entry.message) {
      continue;
    }
    const role = entry.message.role;
    if (role === "user") {
      userCount += 1;
      if (!user) {
        user = extractText(entry.message.content);
      }
    } else if (role === "assistant") {
      if (!assistant) {
        assistant = extractText(entry.message.content);
      }
    }
  }
  return { user, assistant, userCount };
}

/** A title needs both sides of the first exchange to describe the task. */
export function isTitleable(user, assistant) {
  return Boolean(user) && Boolean(assistant);
}

/** The prompt asking for a short, task-naming title. */
export function buildTitlePrompt(user, assistant) {
  return [
    "Write a concise title for this coding session in 3 to 6 words.",
    "Name the actual task or subject, not the tooling.",
    "Reply with the title only — no quotes, no trailing punctuation.",
    "",
    `User: ${String(user).slice(0, 800)}`,
    `Assistant: ${String(assistant).slice(0, 800)}`,
  ].join("\n");
}

/** Trim a model reply to a one-line title (quotes/punctuation stripped). */
export function normalizeTitle(raw) {
  let title = String(raw ?? "")
    .replace(/\s+/g, " ")
    .trim();
  title = title.replace(/^["'`]+/, "").replace(/["'`]+$/, "").trim();
  title = title.replace(/[.!?]+$/, "").trim();
  if (title.length > 60) {
    title = `${title.slice(0, 59).trimEnd()}…`;
  }
  return title;
}

/** The title text of a `modelRegistry.complete` response. */
export function titleFromResponse(response) {
  const text = (response?.content ?? [])
    .filter((block) => block && block.type === "text" && typeof block.text === "string")
    .map((block) => block.text)
    .join(" ");
  return normalizeTitle(text);
}

/**
 * The model to ask: the configured override when it resolves and has auth,
 * otherwise the session's active model. Returning `null` skips titling.
 */
export function resolveModel(ctx, config) {
  const active = ctx?.model ?? null;
  if (!config?.model) {
    return active;
  }
  const { provider, id } = config.model;
  try {
    const found = ctx?.modelRegistry?.find?.(provider, id);
    if (!found) {
      return active;
    }
    const registry = ctx.modelRegistry;
    if (typeof registry.hasConfiguredAuth === "function" && !registry.hasConfiguredAuth(found)) {
      return active;
    }
    return found;
  } catch {
    return active;
  }
}

/** A stale `ctx` after a session switch is expected, not an error. */
export function isStaleContext(error) {
  return String(error?.message ?? error).includes("ctx is stale");
}
