#!/usr/bin/env node
/**
 * Behaviour tests for the Orbit quota bridge extension entry.
 *
 * Run: node --test contrib/orbit-quota-extension/
 *
 * Drives the extension the way pi does — `activate(pi)`, then lifecycle
 * events — with a fake `pi` object and a stubbed `fetch`, so the whole path
 * from "session starts" to "normalized entry appended" is covered offline.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test, { beforeEach } from "node:test";

import { resetQuotaRuntime } from "./adapters.js";
import activate, { ENTRY_TYPE, pendingSnapshot } from "./index.js";

beforeEach(() => resetQuotaRuntime());

function fakePi() {
  const handlers = new Map();
  const entries = [];
  return {
    entries,
    on(event, handler) {
      handlers.set(event, handler);
    },
    appendEntry(type, data) {
      entries.push({ type, data });
    },
    fire(event, ctx) {
      const handler = handlers.get(event);
      assert.ok(handler, `no handler for ${event}`);
      return handler({ type: event }, ctx);
    },
  };
}

/** A pi-like activation context exposing one configured provider. */
function fakeCtx(providerIds) {
  return {
    modelRegistry: {
      getRegisteredProviderIds: () => providerIds,
      hasConfiguredAuth: (model) => providerIds.includes(model.provider),
      getProviderAuth: async () => ({ auth: { apiKey: "test-key" } }),
    },
  };
}

/** Point `$HOME` at a temp dir containing `auth.json`, run `fn`, restore. */
async function withHome(auth, fn) {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || "/tmp", "orbit-bridge-"));
  fs.mkdirSync(path.join(dir, ".pi", "agent"), { recursive: true });
  fs.writeFileSync(path.join(dir, ".pi", "agent", "auth.json"), JSON.stringify(auth));
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

function deepseekFetch(body) {
  return async (url) => ({
    ok: true,
    status: 200,
    redirected: false,
    url,
    text: async () => JSON.stringify(body),
  });
}

test("session start appends one normalized snapshot for configured providers", async () => {
  await withHome({ deepseek: { type: "api_key", key: "test-key" } }, async () => {
    globalThis.fetch = deepseekFetch({
      is_available: true,
      balance_infos: [{ currency: "USD", total_balance: "12.50" }],
    });
    const pi = fakePi();
    await activate(pi);
    await pi.fire("session_start", fakeCtx(["deepseek"]));
    // session_start detaches the fetch; wait for the background snapshot.
    await pendingSnapshot();

    assert.equal(pi.entries.length, 1);
    const entry = pi.entries[0];
    assert.equal(entry.type, ENTRY_TYPE);
    assert.equal(entry.data.providers.length, 1);
    const report = entry.data.providers[0];
    assert.equal(report.provider, "deepseek");
    assert.equal(report.kind, "balance");
    assert.equal(report.balances[0].amount, 12.5);
    assert.ok(Number.isFinite(report.fetchedAt), "fetch time recorded");

    await pi.fire("session_shutdown");
  });
});

test("an unconfigured account appends nothing", async () => {
  await withHome({}, async () => {
    const pi = fakePi();
    await activate(pi);
    await pi.fire("session_start", fakeCtx([]));
    await pendingSnapshot();
    assert.equal(pi.entries.length, 0);
    await pi.fire("session_shutdown");
  });
});

test("a session-only Ollama Cloud setup still snapshots usage", async () => {
  await withHome(
    {
      "ollama-cloud-session": {
        type: "ollama_cloud_session",
        session: "__Secure-session=abc123",
      },
    },
    async () => {
      globalThis.fetch = async (url) => ({
        ok: true,
        status: 200,
        redirected: false,
        url,
        text: async () =>
          '<h2>Cloud Usage</h2> (Pro) <div aria-label="Session usage 30% used"></div>',
      });
      const pi = fakePi();
      await activate(pi);
      await pi.fire("session_start", fakeCtx([]));
      await pendingSnapshot();
      assert.equal(pi.entries.length, 1);
      const report = pi.entries[0].data.providers.find(
        (provider) => provider.provider === "ollama",
      );
      assert.ok(report, "the session alone configures the ollama adapter");
      assert.equal(report.windows.find((w) => w.id === "session").usedPercent, 30);
      await pi.fire("session_shutdown");
    },
  );
});

test("a turn end inside the refresh gap does not append again", async () => {
  await withHome({ deepseek: { type: "api_key", key: "test-key" } }, async () => {
    let calls = 0;
    globalThis.fetch = async (url) => {
      calls += 1;
      return deepseekFetch({
        is_available: true,
        balance_infos: [{ currency: "USD", total_balance: "12.50" }],
      })(url);
    };
    const pi = fakePi();
    await activate(pi);
    await pi.fire("session_start", fakeCtx(["deepseek"]));
    await pendingSnapshot();
    await pi.fire("turn_end", fakeCtx(["deepseek"]));
    await pendingSnapshot();
    assert.equal(pi.entries.length, 1, "no duplicate entry");
    assert.equal(calls, 1, "no duplicate fetch inside the per-provider TTL");
    await pi.fire("session_shutdown");
  });
});

test("a stale activation context never throws out of the extension", async () => {
  await withHome({}, async () => {
    const pi = fakePi();
    await activate(pi);
    const stale = {
      get modelRegistry() {
        throw new Error("This extension ctx is stale");
      },
    };
    await pi.fire("session_start", stale);
    await pendingSnapshot();
    assert.equal(pi.entries.length, 0);
    await pi.fire("session_shutdown");
  });
});
