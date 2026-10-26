/**
 * Provider quota adapters for Orbit's pi extension bridge.
 *
 * This is the extension-side twin of `contrib/pi-quota-rpc/quota-handler.js`
 * (the bundle patch that adds `quota.list` to pi's RPC mode). The two files
 * share the same adapter set by design — the patch cannot import this file
 * (its text is spliced into a non-module bundle), so fixes to a provider
 * adapter must land in both.
 *
 * Each adapter resolves the provider credential through a hook the extension
 * configures (`configureQuotaAuth`, wired to pi's `ctx.modelRegistry`), and
 * falls back to the stored `~/.pi/agent/auth.json` entry. A credential is
 * never exposed to the caller: reports carry only percentages, counts, reset
 * times, and monetary balances. The report shape is
 * `crates/orbit-rpc/docs/quota-rpc.md`.
 *
 * Adapter coverage follows `@narumitw/pi-usage`'s verified provider reference.
 * Providers without a usage surface are reported `unsupported` — never
 * fabricated.
 */

const ORBIT_QUOTA_TTL_MS = 60_000;
const orbitQuotaCache = new Map();

/** pi resolves the credential; the extension swaps this in at activation. */
let orbitQuotaGetAuth = async () => null;

/** Configure credential resolution (pi's model registry). */
export function configureQuotaAuth(getAuth) {
  orbitQuotaGetAuth = typeof getAuth === "function" ? getAuth : async () => null;
}

/** Drop cached credentials and reports — used by tests between cases. */
export function resetQuotaRuntime() {
  orbitQuotaCache.clear();
  orbitQuotaAuthCache = null;
  orbitQuotaAuthAt = 0;
}

// ── normalized report helpers ──────────────────────────────────────────

const orbitQuotaSanitize = (err) => {
  const message = err instanceof Error ? err.message : String(err);
  return message
    .replace(/(sk-|gho_|ghu_|ghp_|xoxb-)[A-Za-z0-9._-]+/gi, "$1…")
    .replace(/(Bearer|token)\s+[A-Za-z0-9._-]{8,}/gi, "$1 …")
    .slice(0, 300);
};

const orbitQuotaEpoch = (value) => {
  const n = Number(value);
  if (!isFinite(n) || n <= 0) return undefined;
  return n < 1e12 ? Math.round(n * 1000) : Math.round(n);
};

const orbitQuotaReset = (value) => {
  if (value == null) return undefined;
  if (typeof value === "string") {
    const parsed = Date.parse(value);
    return isFinite(parsed) ? parsed : undefined;
  }
  return orbitQuotaEpoch(value);
};

const orbitQuotaWindow = (id, label, opts) => {
  const spec = opts || {};
  const window = { id, label };
  if (spec.usedPercent != null && isFinite(spec.usedPercent)) {
    // Fractions converted to percentages pick up float noise (0.28 → 28.000…4).
    // Round to 4 dp so the UI shows a clean number without losing precision.
    window.usedPercent = Math.round(spec.usedPercent * 1e4) / 1e4;
  }
  if (spec.used != null && isFinite(spec.used)) window.used = spec.used;
  if (spec.limit != null && isFinite(spec.limit)) window.limit = spec.limit;
  if (spec.unit) window.unit = spec.unit;
  if (spec.resetsAt != null) window.resetsAt = spec.resetsAt;
  return window;
};

const orbitQuotaBalance = (label, amount, currency) => ({
  label,
  amount: Number(amount),
  currency: currency || "",
});

const orbitQuotaNumber = (value) => {
  const n = Number(value);
  return isFinite(n) ? n : undefined;
};

// ── credential resolution (pi owns the secret) ─────────────────────────

const orbitQuotaHome = () =>
  process.env.HOME || process.env.USERPROFILE || "";

let orbitQuotaAuthCache = null;
let orbitQuotaAuthAt = 0;

async function orbitQuotaReadAuth() {
  const now = Date.now();
  if (orbitQuotaAuthCache && now - orbitQuotaAuthAt < 5000) return orbitQuotaAuthCache;
  try {
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const file = path.join(orbitQuotaHome(), ".pi", "agent", "auth.json");
    orbitQuotaAuthCache = JSON.parse(await fs.readFile(file, "utf8"));
  } catch {
    orbitQuotaAuthCache = {};
  }
  orbitQuotaAuthAt = now;
  return orbitQuotaAuthCache;
}

/** Resolve the request credential plus the raw stored entry for special cases. */
async function orbitQuotaResolved(providerId) {
  let resolved = null;
  try {
    resolved = await orbitQuotaGetAuth(providerId);
  } catch {
    resolved = null;
  }
  let key;
  let origin;
  if (resolved && resolved.auth) {
    if (resolved.auth.apiKey) key = resolved.auth.apiKey;
    if (resolved.auth.baseUrl) origin = resolved.auth.baseUrl;
    const headers = resolved.auth.headers || {};
    const authHeader = headers.Authorization || headers.authorization;
    if (authHeader) {
      key = /^bearer\s+/i.test(authHeader) ? authHeader.replace(/^bearer\s+/i, "") : authHeader;
    }
  }
  const stored = (await orbitQuotaReadAuth())[providerId];
  return { key, origin, stored, resolved };
}

const orbitQuotaOAuthToken = (entry) => {
  if (!entry) return undefined;
  if (entry.type === "oauth" || entry.access || entry.refresh) return entry.access;
  return undefined;
};

async function orbitQuotaJson(url, headers) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12_000);
  try {
    const response = await fetch(url, {
      method: "GET",
      headers,
      redirect: "error",
      signal: controller.signal,
    });
    const text = await response.text();
    let body = null;
    try {
      body = text ? JSON.parse(text) : null;
    } catch {
      body = null;
    }
    return { ok: response.ok, status: response.status, body, text: text.slice(0, 300) };
  } finally {
    clearTimeout(timer);
  }
}

const orbitQuotaHttpError = (status) => "usage request failed (HTTP " + status + ")";

/** Raw text GET, for endpoints that return HTML rather than JSON. */
async function orbitQuotaText(url, headers) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 12_000);
  try {
    const response = await fetch(url, {
      method: "GET",
      headers,
      redirect: "error",
      signal: controller.signal,
    });
    // Do not follow a redirect to a sign-in host: a redirect here means the
    // session is expired, not that the page moved.
    const finalUrl = response.url || url;
    return {
      ok: response.ok,
      status: response.status,
      redirected: finalUrl !== url,
      text: await response.text(),
    };
  } finally {
    clearTimeout(timer);
  }
}

// ── per-provider adapters ──────────────────────────────────────────────

async function orbitQuotaAnthropic(id) {
  const { key, stored } = await orbitQuotaResolved(id);
  const isOAuth = stored && (stored.type === "oauth" || stored.access || stored.refresh);
  if (!isOAuth) {
    return {
      kind: "unsupported",
      note: "Anthropic usage needs a Claude Pro/Max sign-in; API keys expose no usage endpoint.",
    };
  }
  const token = orbitQuotaOAuthToken(stored) || key;
  if (!token) return { kind: "unsupported", note: "No Anthropic OAuth credential stored." };
  const r = await orbitQuotaJson("https://api.anthropic.com/api/oauth/usage", {
    Authorization: "Bearer " + token,
    "anthropic-beta": "oauth-2025-04-20",
    "anthropic-version": "2023-06-01",
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const windows = [];
  const push = (id2, label, node) => {
    if (!node || typeof node !== "object") return;
    windows.push(
      orbitQuotaWindow(id2, label, {
        usedPercent: orbitQuotaNumber(node.utilization),
        resetsAt: orbitQuotaReset(node.resets_at),
      }),
    );
  };
  push("five_hour", "5-hour", d.five_hour);
  push("seven_day", "Weekly", d.seven_day);
  push("seven_day_opus", "Weekly (Opus)", d.seven_day_opus);
  push("seven_day_sonnet", "Weekly (Sonnet)", d.seven_day_sonnet);
  if (Array.isArray(d.limits)) {
    for (const limit of d.limits) {
      if (!limit || !limit.kind || limit.kind === "session" || limit.kind === "weekly_all") continue;
      const label =
        (limit.scope && limit.scope.model && limit.scope.model.display_name) || limit.kind;
      windows.push(
        orbitQuotaWindow("limit:" + label, label, {
          usedPercent: orbitQuotaNumber(limit.percent),
          resetsAt: orbitQuotaReset(limit.resets_at),
        }),
      );
    }
  }
  return { kind: "subscription", windows };
}

async function orbitQuotaCodex(id) {
  const { key, stored } = await orbitQuotaResolved(id);
  const token = orbitQuotaOAuthToken(stored) || key;
  if (!token) return { kind: "unsupported", note: "No ChatGPT OAuth credential stored." };
  const headers = {
    Authorization: "Bearer " + token,
    Accept: "application/json",
    Origin: "https://chatgpt.com",
    Referer: "https://chatgpt.com/",
  };
  const account =
    stored && (stored.accountId || stored.account_id || stored.chatgptAccountId);
  if (account) headers["ChatGPT-Account-Id"] = String(account);
  const r = await orbitQuotaJson("https://chatgpt.com/backend-api/wham/usage", headers);
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const rl = d.rate_limit || d.rate_limits || {};
  const windows = [];
  const add = (id2, label, node) => {
    if (!node || typeof node !== "object") return;
    let resetsAt = orbitQuotaReset(node.reset_at);
    if (resetsAt == null && node.reset_after_seconds != null) {
      resetsAt = Date.now() + Number(node.reset_after_seconds) * 1000;
    }
    windows.push(
      orbitQuotaWindow(id2, label, { usedPercent: orbitQuotaNumber(node.used_percent), resetsAt }),
    );
  };
  add("primary", "5-hour", rl.primary_window);
  add("secondary", "Weekly", rl.secondary_window);
  if (Array.isArray(d.additional_rate_limits)) {
    for (const extra of d.additional_rate_limits) {
      const label = extra && (extra.limit_name || extra.metered_feature);
      if (label) add("limit:" + label, label, extra.rate_limit && extra.rate_limit.primary_window);
    }
  }
  const balances = [];
  const credits = d.credits;
  if (credits && credits.has_credits && credits.balance != null) {
    balances.push(orbitQuotaBalance("Credits", credits.balance, credits.currency || "USD"));
  }
  return { kind: "subscription", plan: d.plan_type, windows, balances };
}

async function orbitQuotaCopilot(id) {
  const { key, stored } = await orbitQuotaResolved(id);
  const token = (stored && (stored.access || stored.key)) || key;
  if (!token) return { kind: "unsupported", note: "No GitHub credential stored." };
  const r = await orbitQuotaJson("https://api.github.com/copilot_internal/user", {
    Authorization: "token " + token,
    Accept: "application/json",
    "Editor-Version": "vscode/1.90.0",
    "Editor-Plugin-Version": "copilot/1.200.0",
    "User-Agent": "GitHubCopilotChat/0.20.0",
    "X-GitHub-Api-Version": "2025-04-01",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const snap = d.quota_snapshots || {};
  const bucket =
    snap.premium_interactions ||
    snap.premium_requests ||
    snap.code ||
    (snap.chat && snap.chat.unlimited === false ? snap.chat : undefined) ||
    snap.completions;
  const windows = [];
  if (bucket) {
    const entitlement = Number(bucket.entitlement) || 0;
    const remaining = orbitQuotaNumber(bucket.remaining);
    const percentRemaining = orbitQuotaNumber(bucket.percent_remaining);
    const label = d.token_based_billing ? "AI Credits" : "Premium requests";
    windows.push(
      orbitQuotaWindow("allowance", label, {
        usedPercent: percentRemaining != null ? 100 - percentRemaining : undefined,
        used: remaining != null && entitlement > 0 ? entitlement - remaining : undefined,
        limit: entitlement > 0 ? entitlement : undefined,
        unit: d.token_based_billing ? "credits" : "requests",
        resetsAt: orbitQuotaReset(d.quota_reset_date),
      }),
    );
  }
  return { kind: "subscription", plan: d.copilot_plan, windows };
}

async function orbitQuotaOpenRouter(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No OpenRouter key stored." };
  const r = await orbitQuotaJson("https://openrouter.ai/api/v1/key", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "credits", error: orbitQuotaHttpError(r.status) };
  const d = (r.body && (r.body.data || r.body)) || {};
  const balances = [];
  const limitRemaining = orbitQuotaNumber(d.limit_remaining);
  const limit = orbitQuotaNumber(d.limit);
  const usage = orbitQuotaNumber(d.usage);
  if (limitRemaining != null) {
    balances.push(orbitQuotaBalance(d.limit == null ? "Spent" : "Remaining", limitRemaining, "USD"));
  } else if (usage != null) {
    balances.push(orbitQuotaBalance("Spent", usage, "USD"));
  }
  const windows = [];
  for (const [field, id2, label] of [
    ["usage_daily", "daily", "Today"],
    ["usage_weekly", "weekly", "This week"],
    ["usage_monthly", "monthly", "This month"],
  ]) {
    const amount = orbitQuotaNumber(d[field]);
    if (amount != null) {
      windows.push(orbitQuotaWindow(id2, label, { used: amount, unit: "USD" }));
    }
  }
  return { kind: "credits", windows, balances, note: d.label ? String(d.label) : undefined };
}

async function orbitQuotaVercel(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No AI Gateway key stored." };
  const r = await orbitQuotaJson("https://ai-gateway.vercel.sh/v1/credits", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "credits", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const balances = [];
  if (d.balance != null) balances.push(orbitQuotaBalance("Remaining", Number(d.balance), "USD"));
  if (d.total_used != null) balances.push(orbitQuotaBalance("Lifetime spend", Number(d.total_used), "USD"));
  return { kind: "credits", balances };
}

async function orbitQuotaDeepSeek(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No DeepSeek key stored." };
  const r = await orbitQuotaJson("https://api.deepseek.com/user/balance", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "balance", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const balances = [];
  const infos = Array.isArray(d.balance_infos) ? d.balance_infos : [];
  for (const info of infos) {
    const currency = info.currency || "";
    if (info.total_balance != null) {
      balances.push(orbitQuotaBalance("Total", info.total_balance, currency));
    }
    if (info.granted_balance != null) {
      balances.push(orbitQuotaBalance("Granted", info.granted_balance, currency));
    }
    if (info.topped_up_balance != null) {
      balances.push(orbitQuotaBalance("Topped up", info.topped_up_balance, currency));
    }
  }
  const note = d.is_available === false ? "API calls are unavailable: balance exhausted." : undefined;
  return { kind: "balance", balances, note };
}

async function orbitQuotaMoonshot(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No Moonshot key stored." };
  const base = id === "moonshotai-cn" ? "https://api.moonshot.cn" : "https://api.moonshot.ai";
  const r = await orbitQuotaJson(base + "/v1/users/me/balance", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "balance", error: orbitQuotaHttpError(r.status) };
  const d = (r.body && (r.body.data || r.body)) || {};
  const currency = id === "moonshotai-cn" ? "CNY" : "USD";
  const balances = [];
  if (d.available_balance != null) balances.push(orbitQuotaBalance("Available", d.available_balance, currency));
  if (d.voucher_balance != null) balances.push(orbitQuotaBalance("Voucher", d.voucher_balance, currency));
  if (d.cash_balance != null) balances.push(orbitQuotaBalance("Cash", d.cash_balance, currency));
  return { kind: "balance", balances };
}

async function orbitQuotaMiniMax(id) {
  const { key, origin } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No MiniMax key stored." };
  const root = (origin && origin.replace(/\/anthropic\/?$/, "").replace(/\/v1\/?$/, "")) ||
    (id === "minimax-cn" ? "https://api.minimaxi.com" : "https://api.minimax.io");
  const isPayg = /^sk-api-/.test(key);
  if (isPayg) {
    const r = await orbitQuotaJson(root + "/account/query_balance", {
      Authorization: "Bearer " + key,
      Accept: "application/json",
    });
    if (!r.ok) return { kind: "balance", error: orbitQuotaHttpError(r.status) };
    const d = (r.body && (r.body.data || r.body)) || {};
    const currency = id === "minimax-cn" ? "CNY" : "USD";
    const balances = [];
    if (d.available_balance != null) balances.push(orbitQuotaBalance("Available", d.available_balance, currency));
    if (d.cash_balance != null) balances.push(orbitQuotaBalance("Cash", d.cash_balance, currency));
    if (d.voucher_balance != null) balances.push(orbitQuotaBalance("Voucher", d.voucher_balance, currency));
    return { kind: "balance", balances };
  }
  const r = await orbitQuotaJson(root + "/v1/token_plan/remains", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const windows = [];
  const scan = (node) => {
    if (!node || typeof node !== "object") return;
    for (const [k, v] of Object.entries(node)) {
      if (v && typeof v === "object" && !Array.isArray(v)) {
        const used = orbitQuotaNumber(v.used_count != null ? v.used_count : v.usage_count);
        const limit = orbitQuotaNumber(v.total_count != null ? v.total_count : v.limit);
        if (used != null || limit != null) {
          windows.push(
            orbitQuotaWindow(k, k.replace(/[_-]+/g, " "), {
              used,
              limit,
              unit: "requests",
              resetsAt: orbitQuotaReset(v.reset_at || v.reset_time),
            }),
          );
        } else {
          scan(v);
        }
      }
    }
  };
  const d = (r.body && (r.body.data || r.body)) || {};
  if (Array.isArray(d.model_remains)) {
    for (const model of d.model_remains) {
      const label = model.model_name || model.model || "Token plan";
      windows.push(
        orbitQuotaWindow("plan:" + label, label, {
          used: orbitQuotaNumber(model.used_count != null ? model.used_count : model.current_interval_usage_count),
          limit: orbitQuotaNumber(model.total_count != null ? model.total_count : model.current_interval_total_count),
          unit: "requests",
          resetsAt: orbitQuotaReset(model.end_time || model.remains_time),
        }),
      );
    }
  } else {
    scan(d);
  }
  return { kind: "subscription", windows };
}

async function orbitQuotaZai(id) {
  const { key, origin } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No Z.AI key stored." };
  const root =
    origin && /^https:\/\/(api\.z\.ai|open\.bigmodel\.cn)/.test(origin)
      ? origin.replace(/(\/api\/.*)$/, "")
      : id === "zai-coding-cn"
        ? "https://open.bigmodel.cn"
        : "https://api.z.ai";
  const rawKey = key.replace(/^Bearer\s+/i, "");
  const r = await orbitQuotaJson(root + "/api/monitor/usage/quota/limit", {
    Authorization: rawKey,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = (r.body && (r.body.data || r.body)) || {};
  const windows = [];
  if (Array.isArray(d.limits)) {
    for (const limit of d.limits) {
      if (!limit || !limit.type) continue;
      const id2 = String(limit.type).toLowerCase();
      const label =
        limit.type === "TIME_LIMIT" ? "Monthly tools" : limit.type === "CREDIT_LIMIT" ? "Credits" : "5-hour";
      windows.push(
        orbitQuotaWindow(id2, label, {
          usedPercent: orbitQuotaNumber(limit.percentage),
          used: orbitQuotaNumber(limit.currentValue),
          limit: orbitQuotaNumber(limit.usage),
          resetsAt: orbitQuotaReset(limit.nextResetTime),
        }),
      );
    }
  }
  return { kind: "subscription", windows };
}

async function orbitQuotaOpenCode(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No OpenCode key stored." };
  const path = id === "opencode-go" ? "/zen/go/v1/usage" : "/zen/v1/usage";
  const r = await orbitQuotaJson("https://opencode.ai" + path, {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = (r.body && (r.body.data || r.body)) || {};
  const labels = { rolling: "Rolling 5h", weekly: "Weekly", monthly: "Monthly" };
  const windows = [];
  const scan = (node) => {
    if (!node || typeof node !== "object") return;
    for (const [k, v] of Object.entries(node)) {
      if (!v || typeof v !== "object") continue;
      const percent = orbitQuotaNumber(
        v.percent != null ? v.percent : v.used_percent != null ? v.used_percent : v.usedPercent,
      );
      const resetsAt = orbitQuotaReset(v.resetsAt || v.reset_at || v.reset);
      if (percent != null || resetsAt != null) {
        windows.push(
          orbitQuotaWindow(k, labels[k] || k.replace(/[_-]+/g, " "), {
            usedPercent: percent,
            resetsAt,
          }),
        );
      } else {
        scan(v);
      }
    }
  };
  scan(d);
  return { kind: "subscription", windows };
}

async function orbitQuotaXai(id) {
  const { key, stored } = await orbitQuotaResolved(id);
  const token = orbitQuotaOAuthToken(stored) || key;
  if (!token) return { kind: "unsupported", note: "No xAI credential stored." };
  const headers = {
    Authorization: "Bearer " + token,
    Accept: "application/json",
    "x-grok-client-mode": "cli",
    "x-grok-client-version": "1.0.4",
  };
  const identity = await orbitQuotaJson(
    "https://cli-chat-proxy.grok.com/v1/user?include=subscription",
    headers,
  );
  if (!identity.ok) return { kind: "credits", error: orbitQuotaHttpError(identity.status) };
  const billingHeaders = Object.assign({}, headers);
  const userId = identity.body && identity.body.userId;
  if (userId) billingHeaders["x-userid"] = String(userId);
  const billing = await orbitQuotaJson(
    "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
    billingHeaders,
  );
  if (!billing.ok) return { kind: "credits", error: orbitQuotaHttpError(billing.status) };
  const d = billing.body || {};
  const config = d.config || {};
  const windows = [];
  const balances = [];
  const period = orbitQuotaNumber(config.usedPercent != null ? config.usedPercent : config.used_percent);
  if (period != null) {
    windows.push(
      orbitQuotaWindow("period", config.period || "Current period", {
        usedPercent: period,
        resetsAt: orbitQuotaReset(config.resetAt || config.reset_at),
      }),
    );
  }
  const prepaid = config.prepaid != null ? config.prepaid : config.balance;
  if (prepaid != null && typeof prepaid !== "object") {
    balances.push(orbitQuotaBalance("Prepaid", Number(prepaid), "USD"));
  }
  const plan = identity.body && identity.body.subscriptionTier;
  return { kind: "credits", plan, windows, balances };
}

async function orbitQuotaFireworks(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No Fireworks key stored." };
  const accounts = await orbitQuotaJson("https://api.fireworks.ai/v1/accounts", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!accounts.ok) return { kind: "spend", error: orbitQuotaHttpError(accounts.status) };
  const list =
    (accounts.body && (accounts.body.accounts || accounts.body.data)) || [];
  const account = Array.isArray(list) ? list.find((a) => a && (a.name || a.id)) : null;
  const accountId = account && (account.name || account.id);
  if (!accountId) return { kind: "spend", note: "No Fireworks account visible to this key." };
  const slug = String(accountId).replace(/^accounts\//, "");
  const summary = await orbitQuotaJson(
    "https://api.fireworks.ai/v1/accounts/" + encodeURIComponent(slug) + "/billing/summary",
    { Authorization: "Bearer " + key, Accept: "application/json" },
  );
  if (!summary.ok) return { kind: "spend", error: orbitQuotaHttpError(summary.status) };
  const d = summary.body || {};
  const balances = [];
  const currency = d.currency || "USD";
  const total = orbitQuotaNumber(d.total != null ? d.total : d.amount);
  if (total != null) balances.push(orbitQuotaBalance("30-day spend", total, currency));
  return { kind: "spend", balances };
}

async function orbitQuotaBaseten(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No Baseten key stored." };
  // The billing summary requires an explicit ISO 8601 UTC range (max 31 days)
  // and rejects a bare `window` parameter with HTTP 400.
  const end = new Date();
  const start = new Date(end.getTime() - 30 * 24 * 60 * 60 * 1000);
  const iso = (date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");
  const r = await orbitQuotaJson(
    "https://api.baseten.co/v1/billing/usage_summary" +
      "?start_date=" + encodeURIComponent(iso(start)) +
      "&end_date=" + encodeURIComponent(iso(end)),
    { Authorization: "Bearer " + key, Accept: "application/json" },
  );
  if (!r.ok) return { kind: "spend", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  // Each category reports `subtotal` (after credits) and `total` (gross cost).
  const categories = [
    ["dedicated_usage", "Dedicated"],
    ["training_usage", "Training"],
    ["model_apis_usage", "Model APIs"],
  ];
  const balances = [];
  let spend = 0;
  for (const [field, label] of categories) {
    const usage = d[field] || {};
    const amount = orbitQuotaNumber(
      usage.total != null ? usage.total : usage.subtotal,
    );
    if (amount == null) continue;
    spend += amount;
    if (amount !== 0) balances.push(orbitQuotaBalance(label, amount, "USD"));
  }
  balances.unshift(orbitQuotaBalance("30-day spend", spend, "USD"));
  return { kind: "spend", balances };
}

// ── Google/Gemini: no authoritative account-usage API ──────────────────
//
// Registered explicitly so a connected account gets a provider-specific
// explanation rather than a generic "unsupported". Gemini rate limits are
// applied per Google Cloud project and are visible only in AI Studio
// (Dashboard → Usage); the API returns 429 RESOURCE_EXHAUSTED on exhaustion
// but never reports remaining quota. We do not scrape the console.

async function orbitQuotaGoogle() {
  return {
    kind: "unsupported",
    note: "Gemini rate limits are per Google Cloud project and visible only in AI Studio; the API reports no remaining quota.",
  };
}

// ── Ollama Cloud ───────────────────────────────────────────────────────
//
// Ollama Cloud has gone through two billing generations, and accounts on
// different terms expose different things:
//
//   current (usage-credit)  a single monthly dollar-credit pool, metered in
//                           tokens. `GET https://ollama.com/api/usage` with a
//                           real cloud API key returns a 0..1 `limits.monthly`
//                           object plus per-model request counts. The reset is
//                           on the plan's subscription anniversary, which the
//                           payload does NOT expose — we omit the countdown
//                           rather than guess.
//   legacy (GPU-time)       a 5-hour session window, a weekly window, and the
//                           plan's included allowance. These have no API: they
//                           render server-side on the authenticated
//                           `https://ollama.com/settings` page, so the only
//                           honest source is that page's HTML with the user's
//                           own session.
//
// Credentials are user-supplied and explicit. We NEVER read browser cookies
// and never touch pi's local models.json placeholder (the literal API key
// `ollama` pointing at 127.0.0.1). Accepted auth.json shapes:
//
//   "ollama"                {"type":"api_key","key":"<real key>"}          → current model
//   "ollama-cloud"          {"type":"api_key","key":"<real key>"}          → current model
//                           (the third-party pi-ollama-cloud-provider id)
//   "ollama-cloud-session"  {"type":"ollama_cloud_session","session":"<cookie>"} → legacy page
//
// The session lives under its own key, never the `ollama` provider id: pi's
// auth resolver treats any stored credential under a provider id as
// authoritative, so an unknown type there shadows the local endpoint's
// placeholder key and yields "Provider is not configured: ollama".
//
// The session value is a `Cookie:` header (e.g. `__Secure-session=…`). It is
// only ever sent to https://ollama.com and never logged.

const OLLAMA_USAGE_URL = "https://ollama.com/api/usage";
const OLLAMA_SETTINGS_URL = "https://ollama.com/settings";

/** auth.json key holding the session cookie; never a provider id. */
const OLLAMA_SESSION_KEY = "ollama-cloud-session";

/** The local-server placeholder pi writes into models.json; never a cloud key. */
const OLLAMA_LOCAL_PLACEHOLDER = "ollama";

/** The cookie header from a stored session entry, if any. */
function orbitOllamaSession(entry) {
  return entry && typeof entry.session === "string" ? entry.session.trim() : "";
}

/**
 * Is this stored api_key entry a real Ollama Cloud key? The local server uses
 * the literal placeholder `ollama` (or a loopback URL), which must never be
 * sent to ollama.com.
 */
function orbitOllamaCloudKey(entry) {
  const key = entry && typeof entry.key === "string" ? entry.key.trim() : "";
  if (!key) return undefined;
  if (key === OLLAMA_LOCAL_PLACEHOLDER) return undefined;
  if (/^https?:\/\//i.test(key)) return undefined;
  if (/^127\.0\.0\.1|^localhost/i.test(key)) return undefined;
  return key;
}

/**
 * Parse the authenticated ollama.com/settings page.
 *
 * This is an UNSTABLE integration: Ollama renders the numbers server-side and
 * may change the markup at any time. Everything HTML-specific lives in this
 * object so a break is contained and fixable in one place. It never throws on
 * a malformed page — it returns `{ error }` so the report degrades honestly.
 *
 * Recognized shapes (observed across generations):
 *   - plan badge text near "Cloud Usage" (Free / Pro / Max)
 *   - two usage meters labelled "Session" and "Weekly", each with an
 *     aria-label or a visible "<n>% used" value in a sibling element
 *   - a `data-time` (or `datetime`) ISO attribute on the "Resets in …"
 *     element that follows each meter
 *   - optional premium-interaction / extra-usage figures
 *
 * The page has rendered both an accessible shape (`aria-label="Session usage
 * 42.5% used"` wrapping the reset) and a plain one
 * (`<span>Session usage</span><span>5.6% used</span> … <span data-time="…">`).
 * Both are handled: each read first tries the aria-label block, then falls back
 * to scanning from the visible label to the next meter, so a value or reset is
 * never borrowed from the neighbouring window.
 */
const OllamaCloudParser = {
  /** Plan tier, if the header exposes one. */
  plan(html) {
    const match = html.match(
      /Cloud\s+Usage[\s\S]{0,400}?\b(Free|Pro|Max|Team|Enterprise)\b/i,
    );
    return match ? match[1] : undefined;
  },

  /** Collect `{id,label,percent,resetsAt}` rows for the legacy windows. */
  windows(html) {
    const out = [];
    const meters = [
      { id: "session", label: "5-hour session", pattern: /session/i },
      { id: "weekly", label: "Weekly", pattern: /weekly/i },
    ];
    for (const meter of meters) {
      const percent = this.percentFor(html, meter.pattern);
      const resetsAt = this.resetFor(html, meter.pattern);
      if (percent == null && resetsAt == null) continue;
      out.push({
        id: meter.id,
        label: meter.label,
        usedPercent: percent,
        resetsAt,
      });
    }
    return out;
  },

  /**
   * Percent-used for a labelled meter. Prefers an explicit
   * `aria-label="Session usage 42.5%…"` (the accessible value), then the visible
   * markup, where the label and its value are sibling elements
   * (`<span>Session usage</span><span>5.6% used</span>`). Returns a number or
   * undefined.
   *
   * The label word is anchored at the START of the aria-label value, because
   * the word also appears inside the value itself and an unanchored search
   * would match the label's own text and then run past the next meter.
   */
  percentFor(html, labelPattern) {
    const label = labelPattern.source;
    const aria = new RegExp(
      `aria-label="(?:${label})[^"]*?(\\d+(?:\\.\\d+)?)\\s*%`,
      "i",
    ).exec(html);
    if (aria) return Number(aria[1]);
    const block = this.meterSlice(html, labelPattern);
    if (block == null) return undefined;
    // Prefer "<n>% used" over a usage-track segment width (`style="width: 3.4%"`).
    const used = /(\d+(?:\.\d+)?)\s*%\s*used/i.exec(block);
    if (used) return Number(used[1]);
    const any = /(\d+(?:\.\d+)?)\s*%/.exec(block);
    return any ? Number(any[1]) : undefined;
  },

  /**
   * Reset time for a labelled meter. Reads a `data-time="<ISO>"` (or
   * `datetime="<ISO>"`) attribute from the meter block — the element carrying
   * the "Resets in …" text. Returns epoch millis or undefined; never invents a
   * time.
   */
  resetFor(html, labelPattern) {
    const label = labelPattern.source;
    const aria = new RegExp(
      `aria-label="(?:${label})[^"]*"([\\s\\S]*?)(?=aria-label="|$)`,
      "i",
    ).exec(html);
    if (aria) {
      const parsed = this.resetIn(aria[1]);
      if (parsed != null) return parsed;
    }
    const block = this.meterSlice(html, labelPattern);
    return block == null ? undefined : this.resetIn(block);
  },

  /**
   * The HTML belonging to one labelled meter: everything after the visible
   * label up to the next meter label (session/weekly), or a bounded tail when
   * the meter is the last one. Never spans two meters, so a value or reset is
   * never borrowed from a neighbour.
   */
  meterSlice(html, labelPattern) {
    const label = labelPattern.source;
    const start = new RegExp(`>\\s*(?:${label})[^<]*<`, "i").exec(html);
    if (!start) return null;
    const rest = html.slice(start.index + start[0].length);
    // Stop at the next *visible* meter label (`>Weekly usage<`). The same
    // meter's accessible value also spells the label
    // (`aria-label="Session usage 3.9% used"`) and the meter's tooltip sits
    // between the label and its "Resets in …" element, so the boundary must be
    // a `>`-anchored label or the slice would end before the `data-time` and
    // lose the reset. The tail cap covers the final meter, where a large
    // model-breakdown tooltip (≈3 KB) still precedes the reset element.
    const next = />\s*(?:session|weekly)\s+usage\s*</i.exec(rest);
    return next ? rest.slice(0, next.index) : rest.slice(0, 8000);
  },

  /** First `data-time`/`datetime` ISO value in a block, as epoch millis. */
  resetIn(block) {
    const match = /(?:data-time|datetime)="([^"]+)"/.exec(block);
    if (!match) return undefined;
    const parsed = Date.parse(match[1]);
    return isFinite(parsed) ? parsed : undefined;
  },
};

/** Current model: fetch the authoritative JSON usage endpoint. */
async function orbitOllamaCloudApi(key) {
  const r = await orbitQuotaJson(OLLAMA_USAGE_URL, {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = r.body || {};
  const limits = d.limits || {};
  const monthly = limits.monthly || {};
  const windows = [];

  // `limits.monthly.usage` is a 0..1 fraction of the monthly credit pool.
  const fraction = orbitQuotaNumber(monthly.usage);
  if (fraction != null) {
    windows.push(
      orbitQuotaWindow("monthly", "Monthly credits", {
        usedPercent: fraction <= 1 ? fraction * 100 : fraction,
        unit: "credits",
        // The reset is the subscription anniversary, absent from the payload.
        resetsAt: undefined,
      }),
    );
  }
  // Some responses also carry the legacy windows; surface them when present.
  for (const [key2, id2, label] of [
    ["session", "session", "5-hour session"],
    ["weekly", "weekly", "Weekly"],
  ]) {
    const node = limits[key2];
    if (node && typeof node === "object") {
      const pct = orbitQuotaNumber(
        node.usage != null ? node.usage : node.used_percent,
      );
      const resetsAt = orbitQuotaReset(node.resets_at || node.reset_at);
      if (pct != null || resetsAt != null) {
        windows.push(
          orbitQuotaWindow(id2, label, {
            usedPercent: pct != null && pct <= 1 ? pct * 100 : pct,
            resetsAt,
          }),
        );
      }
    }
  }

  const balances = [];
  const credits = d.credits || d.balance;
  if (credits && typeof credits === "object") {
    const remaining = orbitQuotaNumber(
      credits.remaining != null ? credits.remaining : credits.available,
    );
    if (remaining != null) {
      balances.push(orbitQuotaBalance("Credits", remaining, credits.currency || "USD"));
    }
  }

  return {
    kind: "subscription",
    plan: typeof d.plan === "string" ? d.plan : undefined,
    windows,
    balances,
    note:
      windows.some((w) => w.id === "monthly")
        ? "Monthly usage credits reset on your plan's anniversary, which the API does not expose."
        : "The API does not expose reset times; add a session cookie in Settings → Providers → Ollama to see when the 5-hour and weekly windows reset.",
  };
}

/**
 * Merge an API report with a settings-page report. The API carries the
 * authoritative percentages for the current account; the settings page carries
 * the only reset timestamps (Ollama's `/api/usage` omits them). Reset times are
 * copied onto the matching API windows by id, and any window only the page
 * knows about is appended. When either side failed, the usable one is returned
 * unchanged rather than guessing.
 */
function orbitOllamaMergeReports(api, page) {
  const apiOk = api && Array.isArray(api.windows) && api.error == null;
  const pageOk = page && Array.isArray(page.windows) && page.error == null;
  if (!apiOk) return pageOk ? page : api;
  if (!pageOk || page.windows.length === 0) return api;

  const pageById = new Map(page.windows.map((w) => [w.id, w]));
  const merged = api.windows.map((w) => {
    if (w.resetsAt != null) return w;
    const match = pageById.get(w.id);
    return match && match.resetsAt != null ? { ...w, resetsAt: match.resetsAt } : w;
  });
  for (const w of page.windows) {
    if (!merged.some((m) => m.id === w.id)) merged.push(w);
  }
  return {
    ...api,
    plan: api.plan || page.plan,
    windows: merged,
  };
}

/** Legacy model: fetch and parse the authenticated settings page. */
async function orbitOllamaCloudSettings(session) {
  const r = await orbitQuotaText(OLLAMA_SETTINGS_URL, {
    Accept: "text/html",
    Cookie: session,
    "User-Agent": "Orbit/0.1 (+https://github.com/)",
  });
  if (r.redirected || r.status === 302 || r.status === 303) {
    return { kind: "unsupported", error: "Ollama session expired — sign in again." };
  }
  if (r.status === 401 || r.status === 403) {
    return { kind: "unsupported", error: "Ollama session rejected (HTTP " + r.status + ")." };
  }
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  if (/\/signin|authkit/i.test(r.text) && !/Cloud\s+Usage/i.test(r.text)) {
    return { kind: "unsupported", error: "Ollama session expired — sign in again." };
  }

  const windows = OllamaCloudParser.windows(r.text);
  if (windows.length === 0) {
    // The page loaded but the markup no longer matches: say so plainly rather
    // than show a fabricated 0%.
    return {
      kind: "unsupported",
      plan: OllamaCloudParser.plan(r.text),
      error: "Could not read Ollama usage from the settings page (layout changed).",
    };
  }
  return {
    kind: "subscription",
    plan: OllamaCloudParser.plan(r.text),
    windows,
  };
}

/**
 * Ollama Cloud adapter. The API with a real cloud key is authoritative for the
 * percentages; the authenticated settings page is the only source of reset
 * timestamps. When the account has both a key and a session, both are queried
 * and merged, so a stored key never hides the countdown. With only one
 * credential, that source is used alone.
 */
async function orbitQuotaOllama(id) {
  const { stored } = await orbitQuotaResolved(id);
  const auth = await orbitQuotaReadAuth();
  const cloudKey = orbitOllamaCloudKey(stored);
  // New key first; fall back to a legacy entry stored under the provider id.
  const session =
    orbitOllamaSession(auth[OLLAMA_SESSION_KEY]) || orbitOllamaSession(stored);

  if (cloudKey && session) {
    const [api, page] = await Promise.all([
      orbitOllamaCloudApi(cloudKey),
      orbitOllamaCloudSettings(session),
    ]);
    const merged = orbitOllamaMergeReports(api, page);
    // A configured-but-unreadable settings page must not be ignored silently:
    // without it there are no resets, so carry the reason (expired cookie,
    // layout change) so the popover shows why the countdown is missing.
    if (merged && merged.error == null && page && page.error) {
      return { ...merged, error: page.error };
    }
    return merged;
  }
  if (cloudKey) return orbitOllamaCloudApi(cloudKey);
  if (session) return orbitOllamaCloudSettings(session);

  return {
    kind: "unsupported",
    note:
      "Ollama Cloud needs an API key (current monthly usage) or a session cookie (legacy window usage) — add one in Settings → Providers → Ollama. Local-only Ollama reports nothing.",
  };
}

const orbitQuotaAdapters = {
  anthropic: orbitQuotaAnthropic,
  "openai-codex": orbitQuotaCodex,
  "github-copilot": orbitQuotaCopilot,
  "kimi-coding": orbitQuotaKimi,
  google: orbitQuotaGoogle,
  "google-vertex": orbitQuotaGoogle,
  ollama: orbitQuotaOllama,
  "ollama-cloud": orbitQuotaOllama,
  "moonshotai": orbitQuotaMoonshot,
  "moonshotai-cn": orbitQuotaMoonshot,
  "minimax": orbitQuotaMiniMax,
  "minimax-cn": orbitQuotaMiniMax,
  "zai": orbitQuotaZai,
  "zai-coding-cn": orbitQuotaZai,
  "opencode-go": orbitQuotaOpenCode,
  openrouter: orbitQuotaOpenRouter,
  "vercel-ai-gateway": orbitQuotaVercel,
  deepseek: orbitQuotaDeepSeek,
  xai: orbitQuotaXai,
  fireworks: orbitQuotaFireworks,
  baseten: orbitQuotaBaseten,
};

// Kimi's usage payload is provider-shaped; parse it tolerantly.
async function orbitQuotaKimi(id) {
  const { key } = await orbitQuotaResolved(id);
  if (!key) return { kind: "unsupported", note: "No Kimi credential stored." };
  const r = await orbitQuotaJson("https://api.kimi.com/coding/v1/usages", {
    Authorization: "Bearer " + key,
    Accept: "application/json",
  });
  if (!r.ok) return { kind: "subscription", error: orbitQuotaHttpError(r.status) };
  const d = (r.body && (r.body.data || r.body)) || {};
  const windows = [];
  const balances = [];
  const scan = (node) => {
    if (!node || typeof node !== "object") return;
    for (const [k, v] of Object.entries(node)) {
      if (!v || typeof v !== "object" || Array.isArray(v)) continue;
      const used = orbitQuotaNumber(
        v.used != null ? v.used : v.usage != null ? v.usage : v.used_count,
      );
      const limit = orbitQuotaNumber(v.limit != null ? v.limit : v.total != null ? v.total : v.limit_count);
      const percent = orbitQuotaNumber(v.used_percent != null ? v.used_percent : v.usedPercent);
      const resetsAt = orbitQuotaReset(v.resetAt || v.reset_at || v.resetsAt || v.reset_time);
      if (used != null || limit != null || percent != null) {
        windows.push(
          orbitQuotaWindow(k, k.replace(/[_-]+/g, " "), {
            usedPercent: percent,
            used,
            limit,
            unit: "requests",
            resetsAt,
          }),
        );
      } else {
        scan(v);
      }
    }
  };
  scan(d);
  const wallet = d.wallet || d.booster;
  if (wallet && typeof wallet === "object") {
    const currency = wallet.currency || "USD";
    if (wallet.amount != null) balances.push(orbitQuotaBalance("Wallet", wallet.amount, currency));
    if (wallet.amountLeft != null) balances.push(orbitQuotaBalance("Wallet left", wallet.amountLeft, currency));
  }
  return { kind: "subscription", windows, balances };
}

async function orbitQuotaReport(providerId) {
  const cached = orbitQuotaCache.get(providerId);
  if (cached && Date.now() - cached.at < ORBIT_QUOTA_TTL_MS) return cached.report;
  let report;
  try {
    const adapter = orbitQuotaAdapters[providerId];
    if (!adapter) {
      report = {
        provider: providerId,
        kind: "unsupported",
        windows: [],
        balances: [],
        note: "This provider exposes no account usage API.",
      };
    } else {
      const partial = await adapter(providerId);
      report = Object.assign(
        { provider: providerId, kind: "unsupported", windows: [], balances: [] },
        partial,
      );
    }
  } catch (err) {
    report = {
      provider: providerId,
      kind: "unsupported",
      windows: [],
      balances: [],
      error: orbitQuotaSanitize(err),
    };
  }
  report.fetchedAt = Date.now();
  orbitQuotaCache.set(providerId, { at: Date.now(), report });
  return report;
}

/** Fetch normalized reports for the given provider ids, in order. */
async function quotaReports(providerIds) {
  const reports = [];
  for (const providerId of providerIds) {
    reports.push(await orbitQuotaReport(providerId));
  }
  return reports;
}

export {
  orbitQuotaReport as quotaReport,
  orbitQuotaSanitize as quotaSanitize,
  orbitQuotaAdapters as quotaAdapters,
  orbitQuotaReadAuth as quotaStoredAuth,
  orbitOllamaCloudKey as ollamaCloudKey,
  OllamaCloudParser,
  quotaReports,
  ORBIT_QUOTA_TTL_MS,
  OLLAMA_SESSION_KEY,
};
