#!/usr/bin/env node
/**
 * Unit tests for the Orbit quota bridge adapters.
 *
 * Run: node --test contrib/orbit-quota-extension/
 *
 * These tests are network-free: they only cover the providers whose adapters
 * return without making a request (the honest `unsupported` set) plus
 * dispatch, credential resolution, and error sanitization. Authoritative
 * provider payloads are covered by `crates/orbit-rpc/docs/quota-rpc.md`
 * fixtures and the Rust round-trip test.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test, { beforeEach } from "node:test";

import {
  OllamaCloudParser,
  configureQuotaAuth,
  ollamaCloudKey,
  quotaAdapters,
  quotaReport,
  quotaReports,
  quotaSanitize,
  resetQuotaRuntime,
} from "./adapters.js";

beforeEach(() => resetQuotaRuntime());

/** A fetch stub returning one JSON body for any URL. */
function jsonFetch(body, { status = 200 } = {}) {
  return async (url) => ({
    ok: status >= 200 && status < 300,
    status,
    redirected: false,
    url,
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
  });
}

/** A fetch stub returning one HTML body for any URL. */
function htmlFetch(html, { status = 200, finalUrl } = {}) {
  return async (url) => ({
    ok: status >= 200 && status < 300,
    status,
    url: finalUrl || url,
    text: async () => html,
  });
}

/** Point `$HOME` at a temp dir containing `auth.json`, run `fn`, then restore. */
async function withAuth(auth, fn) {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || "/tmp", "orbit-quota-"));
  fs.mkdirSync(path.join(dir, ".pi", "agent"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, ".pi", "agent", "auth.json"),
    JSON.stringify(auth),
  );
  const prev = process.env.HOME;
  process.env.HOME = dir;
  resetQuotaRuntime();
  try {
    return await fn();
  } finally {
    process.env.HOME = prev;
    resetQuotaRuntime();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("google reports unsupported without a network request", async () => {
  const report = await quotaReport("google");
  assert.equal(report.provider, "google");
  assert.equal(report.kind, "unsupported");
  assert.deepEqual(report.windows, []);
  assert.deepEqual(report.balances, []);
  assert.match(report.note, /AI Studio/);
  assert.equal(report.error, undefined);
});

test("google-vertex shares the Gemini explanation", async () => {
  const report = await quotaReport("google-vertex");
  assert.equal(report.kind, "unsupported");
  assert.match(report.note, /AI Studio/);
});

test("ollama without a cloud credential explains how to add one", async () => {
  await withAuth({}, async () => {
    const report = await quotaReport("ollama");
    assert.equal(report.kind, "unsupported");
    assert.match(report.note, /API key|session cookie/);
    assert.match(report.note, /Settings → Providers → Ollama/);
    assert.equal(report.error, undefined);
  });
});

test("ollama local placeholder is never treated as a cloud key", () => {
  assert.equal(ollamaCloudKey({ type: "api_key", key: "ollama" }), undefined);
  assert.equal(
    ollamaCloudKey({ type: "api_key", key: "http://127.0.0.1:11434" }),
    undefined,
  );
  assert.equal(ollamaCloudKey({ type: "api_key", key: "  " }), undefined);
  assert.equal(ollamaCloudKey(null), undefined);
  assert.equal(
    ollamaCloudKey({ type: "api_key", key: "abc123realkey" }),
    "abc123realkey",
  );
});

test("OllamaCloudParser reads legacy session and weekly meters", () => {
  const html = `
    <h2>Cloud Usage</h2><span>Pro</span>
    <div class="meter" aria-label="Session usage 42.5% used">
      <span>Resets in 1h 20m</span>
      <time data-time="2026-09-12T16:30:00Z"></time>
    </div>
    <div class="meter" aria-label="Weekly usage 12% used"></div>
  `;
  assert.equal(OllamaCloudParser.plan(html), "Pro");
  const windows = OllamaCloudParser.windows(html);
  assert.equal(windows.length, 2);
  assert.equal(windows[0].id, "session");
  assert.equal(windows[0].usedPercent, 42.5);
  assert.equal(windows[0].resetsAt, Date.parse("2026-09-12T16:30:00Z"));
  assert.equal(windows[1].id, "weekly");
  assert.equal(windows[1].usedPercent, 12);
  assert.equal(windows[1].resetsAt, undefined);
});

test("OllamaCloudParser returns no windows for unrelated markup", () => {
  const windows = OllamaCloudParser.windows("<html><body>Nothing here</body></html>");
  assert.deepEqual(windows, []);
  assert.equal(OllamaCloudParser.plan("<html></html>"), undefined);
});

test("OllamaCloudParser handles zero usage as a real value, not missing", () => {
  const html = `<h2>Cloud Usage</h2> (Free)
    <div aria-label="Session usage 0% used"></div>
    <div aria-label="Weekly usage 0% used"></div>`;
  const windows = OllamaCloudParser.windows(html);
  assert.equal(windows.length, 2);
  assert.equal(windows[0].usedPercent, 0);
  assert.equal(windows[1].usedPercent, 0);
});

test("google/ollama are registered; unknown ids report unsupported", async () => {
  for (const id of ["google", "google-vertex", "ollama", "ollama-cloud"]) {
    assert.equal(typeof quotaAdapters[id], "function", `${id} registered`);
  }
  const report = await quotaReport("acme-llm");
  assert.equal(report.kind, "unsupported");
  assert.match(report.note, /no account usage API/);
});

test("dispatch returns every requested provider report", async () => {
  await withAuth({}, async () => {
    const reports = await quotaReports(["google", "google-vertex", "ollama"]);
    const providers = reports.map((report) => report.provider).sort();
    assert.deepEqual(providers, ["google", "google-vertex", "ollama"]);
  });
});

test("sanitize redacts credential-shaped substrings", () => {
  const cleaned = quotaSanitize(
    new Error("request to https://x failed with Bearer sk-abc123DEFghi and ghp_00aa11bb22cc"),
  );
  assert.ok(!cleaned.includes("sk-abc123DEFghi"), "openai-style key redacted");
  assert.ok(!cleaned.includes("ghp_00aa11bb22cc"), "github token redacted");
});

test("baseten sends an ISO range and sums the billing usage categories", async () => {
  configureQuotaAuth(async (id) =>
    id === "baseten" ? { auth: { apiKey: "bt-key" } } : null,
  );
  try {
    let requested = "";
    globalThis.fetch = async (url) => {
      requested = String(url);
      return {
        ok: true,
        status: 200,
        url,
        text: async () =>
          JSON.stringify({
            dedicated_usage: { subtotal: 10, total: 12, credits_used: 2, minutes: 60 },
            training_usage: { subtotal: 0, total: 0, credits_used: 0, minutes: 0 },
            model_apis_usage: { subtotal: 3, total: 3, credits_used: 0 },
          }),
      };
    };
    const report = await quotaReport("baseten");
    assert.equal(report.kind, "spend");
    // The endpoint requires an explicit ISO 8601 range; a bare `window` 400s.
    const params = new URL(requested).searchParams;
    assert.ok(params.get("start_date"), "start_date sent");
    assert.ok(params.get("end_date"), "end_date sent");
    const days =
      (Date.parse(params.get("end_date")) - Date.parse(params.get("start_date"))) /
      86400000;
    assert.ok(days > 0 && days <= 31, "range within 31 days");
    assert.deepEqual(report.balances, [
      { label: "30-day spend", amount: 15, currency: "USD" },
      { label: "Dedicated", amount: 12, currency: "USD" },
      { label: "Model APIs", amount: 3, currency: "USD" },
    ]);
  } finally {
    configureQuotaAuth(null);
  }
});

test("ollama with a real cloud key maps the monthly credit fraction", async () => {
  await withAuth({ ollama: { type: "api_key", key: "real-cloud-key" } }, async () => {
    globalThis.fetch = jsonFetch({
      plan: "pro",
      limits: { monthly: { usage: 0.28 } },
    });
    const report = await quotaReport("ollama");
    assert.equal(report.kind, "subscription");
    assert.equal(report.plan, "pro");
    const window = report.windows.find((w) => w.id === "monthly");
    assert.ok(window, "monthly window present");
    assert.equal(window.usedPercent, 28, "0.28 fraction becomes 28% cleanly");
    // The anniversary reset is not in the payload: never fabricated.
    assert.equal(window.resetsAt, undefined);
  });
});

test("ollama with a session parses the legacy settings page", async () => {
  await withAuth(
    { "ollama-cloud-session": { type: "ollama_cloud_session", session: "__Secure-session=abc123" } },
    async () => {
      const html = `<h2>Cloud Usage</h2> (Pro)
        <div aria-label="Session usage 42.5% used">
          <time data-time="2026-09-12T16:30:00Z"></time>
        </div>
        <div aria-label="Weekly usage 12% used"></div>`;
      globalThis.fetch = htmlFetch(html);
      const report = await quotaReport("ollama");
      assert.equal(report.kind, "subscription");
      assert.equal(report.plan, "Pro");
      const session = report.windows.find((w) => w.id === "session");
      const weekly = report.windows.find((w) => w.id === "weekly");
      assert.equal(session.usedPercent, 42.5);
      assert.equal(session.resetsAt, Date.parse("2026-09-12T16:30:00Z"));
      assert.equal(weekly.usedPercent, 12);
    },
  );
});

test("ollama-cloud (third-party provider id) uses the stored cloud key", async () => {
  await withAuth(
    { "ollama-cloud": { type: "api_key", key: "real-cloud-key" } },
    async () => {
      globalThis.fetch = jsonFetch({ limits: { monthly: { usage: 0.4 } } });
      const report = await quotaReport("ollama-cloud");
      assert.equal(report.kind, "subscription");
      assert.equal(report.windows.find((w) => w.id === "monthly").usedPercent, 40);
    },
  );
});

test("ollama-cloud falls back to the shared session cookie", async () => {
  await withAuth(
    { "ollama-cloud-session": { type: "ollama_cloud_session", session: "__Secure-session=abc123" } },
    async () => {
      globalThis.fetch = htmlFetch(
        `<h2>Cloud Usage</h2> (Pro)
        <div aria-label="Session usage 7% used"></div>`,
      );
      const report = await quotaReport("ollama-cloud");
      assert.equal(report.kind, "subscription");
      assert.equal(report.windows.find((w) => w.id === "session").usedPercent, 7);
    },
  );
});

test("ollama expired session degrades to a sign-in error, not a crash", async () => {
  await withAuth(
    { "ollama-cloud-session": { type: "ollama_cloud_session", session: "__Secure-session=expired" } },
    async () => {
      globalThis.fetch = htmlFetch("", {
        status: 200,
        finalUrl: "https://ollama.com/signin",
      });
      const report = await quotaReport("ollama");
      assert.equal(report.kind, "unsupported");
      assert.match(report.error, /expired/i);
    },
  );
});

test("ollama changed markup reports an error instead of a fake zero", async () => {
  await withAuth(
    { "ollama-cloud-session": { type: "ollama_cloud_session", session: "__Secure-session=abc123" } },
    async () => {
      globalThis.fetch = htmlFetch("<html><body>Redesigned dashboard</body></html>");
      const report = await quotaReport("ollama");
      assert.equal(report.kind, "unsupported");
      assert.match(report.error, /layout changed/i);
      assert.deepEqual(report.windows, []);
    },
  );
});

test("ollama reads a legacy session stored under the provider id", async () => {
  await withAuth(
    { ollama: { type: "ollama_cloud_session", session: "__Secure-session=legacy" } },
    async () => {
      globalThis.fetch = htmlFetch(
        `<h2>Cloud Usage</h2> (Pro)
        <div aria-label="Session usage 5% used"></div>`,
      );
      const report = await quotaReport("ollama");
      assert.equal(report.kind, "subscription");
      assert.equal(report.windows.find((w) => w.id === "session").usedPercent, 5);
    },
  );
});

test("ollama real cloud key wins when a session is also stored", async () => {
  await withAuth(
    {
      ollama: { type: "api_key", key: "real-cloud-key" },
      "ollama-cloud-session": {
        type: "ollama_cloud_session",
        session: "__Secure-session=abc123",
      },
    },
    async () => {
      globalThis.fetch = jsonFetch({ limits: { monthly: { usage: 0.5 } } });
      const report = await quotaReport("ollama");
      assert.equal(report.kind, "subscription");
      assert.ok(report.windows.find((w) => w.id === "monthly"));
    },
  );
});

// The real settings page renders the label and its value as sibling elements
// and puts the reset on a `data-time` span afterwards — no aria-label. The
// label text is then repeated in an `aria-label` on the value, and a large
// model-breakdown tooltip sits between the value and the "Resets in …" line.
const TOOLTIP = `<div class="tooltip">${'padding '.repeat(400)}</div>`;
const REAL_SETTINGS_HTML = `
  <h2><span>Cloud usage</span><span>max</span></h2>
  <div>
    <span class="text-sm">Session usage</span>
    <span class="text-sm" aria-label="Session usage 5.6% used"> 5.6% used </span>
    ${TOOLTIP}
    <div class="text-xs local-time" data-time="2026-07-23T03:00:00Z">Resets in 2 hours.</div>
  </div>
  <div>
    <span class="text-sm">Weekly usage</span>
    <span class="text-sm" aria-label="Weekly usage 14.2% used">14.2% used</span>
    ${TOOLTIP}
    <div class="text-xs local-time" data-time="2026-07-29T00:00:00Z">Resets in 6 days.</div>
  </div>
`;

test("OllamaCloudParser reads the live markup: duplicate labels, tooltip, data-time", () => {
  assert.equal(OllamaCloudParser.plan(REAL_SETTINGS_HTML), "max");
  const windows = OllamaCloudParser.windows(REAL_SETTINGS_HTML);
  assert.equal(windows.length, 2);
  assert.equal(windows[0].id, "session");
  assert.equal(windows[0].usedPercent, 5.6, "reads '<n>% used', not a segment width");
  // The reset sits after the tooltip, past the aria-label that repeats the
  // label text — the slice must not stop at that repeat.
  assert.equal(windows[0].resetsAt, Date.parse("2026-07-23T03:00:00Z"));
  assert.equal(windows[1].id, "weekly");
  assert.equal(windows[1].usedPercent, 14.2);
  assert.equal(windows[1].resetsAt, Date.parse("2026-07-29T00:00:00Z"));
});

test("ollama merges settings-page resets into the API report", async () => {
  await withAuth(
    {
      "ollama-cloud": { type: "api_key", key: "real-cloud-key" },
      "ollama-cloud-session": {
        type: "ollama_cloud_session",
        session: "__Secure-session=abc123",
      },
    },
    async () => {
      globalThis.fetch = async (url) => {
        const body = String(url).includes("/api/usage")
          ? JSON.stringify({
              limits: {
                session: { usage: 0.1, models: [] },
                weekly: { usage: 0.4, models: [] },
              },
            })
          : REAL_SETTINGS_HTML;
        return { ok: true, status: 200, url, text: async () => body };
      };
      const report = await quotaReport("ollama-cloud");
      assert.equal(report.kind, "subscription");
      assert.equal(report.plan, "max");
      const session = report.windows.find((w) => w.id === "session");
      const weekly = report.windows.find((w) => w.id === "weekly");
      // Percentages come from the API, resets from the settings page.
      assert.equal(session.usedPercent, 10);
      assert.equal(session.resetsAt, Date.parse("2026-07-23T03:00:00Z"));
      assert.equal(weekly.usedPercent, 40);
      assert.equal(weekly.resetsAt, Date.parse("2026-07-29T00:00:00Z"));
    },
  );
});

test("ollama keeps the API report when the session page is expired", async () => {
  await withAuth(
    {
      "ollama-cloud": { type: "api_key", key: "real-cloud-key" },
      "ollama-cloud-session": {
        type: "ollama_cloud_session",
        session: "__Secure-session=expired",
      },
    },
    async () => {
      globalThis.fetch = async (url) => {
        if (String(url).includes("/api/usage")) {
          return {
            ok: true,
            status: 200,
            url,
            text: async () => JSON.stringify({ limits: { session: { usage: 0.2 } } }),
          };
        }
        // A redirect to sign-in: the session is dead, but the key still works.
        return {
          ok: true,
          status: 200,
          url: "https://ollama.com/signin",
          text: async () => "",
        };
      };
      const report = await quotaReport("ollama-cloud");
      // The API meters still render, and the dead cookie is surfaced rather
      // than silently dropping the reset countdown.
      assert.match(report.error, /expired/i);
      assert.equal(report.windows.find((w) => w.id === "session").usedPercent, 20);
      assert.equal(report.windows.find((w) => w.id === "session").resetsAt, undefined);
    },
  );
});

test("the local placeholder key is never sent to ollama.com", async () => {
  // The fetch sentinel throws if a request is attempted, so reaching the
  // network fails the test.
  await withAuth({ ollama: { type: "api_key", key: "ollama" } }, async () => {
    globalThis.fetch = () => {
      throw new Error("unsupported adapters must not make a network request");
    };
    const report = await quotaReport("ollama");
    assert.equal(report.kind, "unsupported");
    assert.match(report.note, /API key|session cookie/);
  });
});
