#!/usr/bin/env node
/**
 * Behaviour tests for the Orbit auto-title extension entry.
 *
 * Run: node --test contrib/orbit-title-extension/index.test.mjs
 *
 * Drives the extension the way pi does — `activate(pi)`, then an
 * `agent_settled` event — with a fake `pi`/`ctx` and a temp HOME holding the
 * same `~/.orbit-pi/auto-title.json` Orbit writes, asserting exactly which
 * model is asked and which title reaches `pi.setSessionName`.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test, { beforeEach } from "node:test";

import activate, { pendingTitle } from "./index.js";

let home;

beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "orbit-title-entry-"));
  fs.mkdirSync(path.join(home, ".orbit-pi"), { recursive: true });
  process.env.HOME = home;
});

function setConfig(config) {
  fs.writeFileSync(path.join(home, ".orbit-pi", "auto-title.json"), JSON.stringify(config));
}

function fakePi({ name = "" } = {}) {
  const handlers = new Map();
  const commands = new Map();
  const state = { name, set: [] };
  return {
    state,
    commands,
    on(event, handler) {
      handlers.set(event, handler);
    },
    registerCommand(name, options) {
      commands.set(name, options);
    },
    fire(event, payload, ctx) {
      const handler = handlers.get(event);
      assert.ok(handler, `no handler for ${event}`);
      return handler(payload, ctx);
    },
    getSessionName() {
      return state.name;
    },
    setSessionName(title) {
      state.name = title;
      state.set.push(title);
    },
  };
}

function exchange() {
  return [
    { type: "message", message: { role: "user", content: "fix the redirect loop on login" } },
    {
      type: "message",
      message: { role: "assistant", content: [{ type: "text", text: "I traced it to the oauth callback." }] },
    },
  ];
}

function fakeCtx({ title = "Fix login redirect", model = { provider: "active", id: "session-model" } } = {}) {
  const calls = [];
  return {
    calls,
    model,
    signal: undefined,
    sessionManager: { getBranch: () => exchange() },
    modelRegistry: {
      find: () => undefined,
      hasConfiguredAuth: () => true,
      complete: async (chosen, request, options) => {
        calls.push({ chosen, request, options });
        return { content: [{ type: "text", text: `"${title}"` }] };
      },
    },
  };
}

test("titles the session from the first exchange with the active model", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
  assert.equal(ctx.calls.length, 1);
  assert.deepEqual(ctx.calls[0].chosen, { provider: "active", id: "session-model" });
  assert.match(ctx.calls[0].request.messages[0].content[0].text, /fix the redirect loop on login/);
  // The budget has to cover a reasoning model's thinking, or the answer is
  // starved and no title is ever produced.
  assert.ok(ctx.calls[0].options.maxTokens >= 256);
});

test("uses the configured model when the setting names one", async () => {
  setConfig({ enabled: true, model: { provider: "ollama", id: "glm-5.3" } });
  const pi = fakePi();
  activate(pi);
  const chosen = { provider: "ollama", id: "glm-5.3" };
  const ctx = fakeCtx();
  ctx.modelRegistry.find = (provider, id) =>
    provider === "ollama" && id === "glm-5.3" ? chosen : undefined;
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.equal(ctx.calls[0].chosen, chosen);
  assert.equal(pi.state.name, "Fix login redirect");
});

test("does nothing when the setting is disabled", async () => {
  setConfig({ enabled: false });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.equal(ctx.calls.length, 0);
  assert.deepEqual(pi.state.set, []);
});

test("never clobbers a name that is already set", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi({ name: "Manual title" });
  activate(pi);
  const ctx = fakeCtx();
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.equal(ctx.calls.length, 0, "an already-named session is left alone");
  assert.deepEqual(pi.state.set, []);
});

test("titles a first turn that called tools (several assistant messages)", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  // A real tool-using turn streams toolCall-only assistants before the final
  // text reply; with one user prompt the session is still fresh and titleable.
  ctx.sessionManager.getBranch = () => [
    { type: "message", message: { role: "user", content: "fix the redirect loop on login" } },
    { type: "message", message: { role: "assistant", content: [{ type: "toolCall", name: "read" }] } },
    { type: "message", message: { role: "toolResult", content: [{ type: "text", text: "…" }] } },
    {
      type: "message",
      message: { role: "assistant", content: [{ type: "text", text: "I traced it to the oauth callback." }] },
    },
  ];
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
  assert.equal(ctx.calls.length, 1);
  assert.match(ctx.calls[0].request.messages[0].content[0].text, /I traced it to the oauth callback/);
});

test("titles from a reply that leads with a thinking block", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  // Reasoning models emit a `thinking` block before the text answer; the
  // title comes from the text, never the thinking.
  ctx.modelRegistry.complete = async (chosen, request, options) => {
    ctx.calls.push({ chosen, request, options });
    return {
      content: [
        { type: "thinking", thinking: "The user wants a short title…" },
        { type: "text", text: '"Fix login redirect"' },
      ],
    };
  };
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
});

test("does not retry a completed ask that produced no title", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  // A clean completion with no text (the whole budget went to thinking) is
  // deterministic: asking again would only repeat the empty reply.
  ctx.modelRegistry.complete = async (chosen, request, options) => {
    ctx.calls.push({ chosen, request, options });
    return { content: [{ type: "thinking", thinking: "…" }] };
  };
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.equal(ctx.calls.length, 1);
  assert.deepEqual(pi.state.set, []);
});

test("the /generate-title command re-titles an already-named session", async () => {
  // Manual titling is an explicit request: it works even when the automatic
  // setting is off and even when the session already carries a name.
  setConfig({ enabled: false });
  const pi = fakePi({ name: "Old title" });
  activate(pi);
  const ctx = fakeCtx();
  const command = pi.commands.get("generate-title");
  assert.ok(command, "generate-title is registered");

  await command.handler("", ctx);

  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
  assert.equal(ctx.calls.length, 1);
});

test("the /generate-title command titles a resumed session", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  // More than one user prompt means the automatic pass would skip it; a
  // manual request still describes the session from its first exchange.
  ctx.sessionManager.getBranch = () => [
    ...exchange(),
    { type: "message", message: { role: "user", content: "again" } },
    { type: "message", message: { role: "assistant", content: [{ type: "text", text: "done" }] } },
  ];

  await pi.commands.get("generate-title").handler("", ctx);

  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
});

test("leaves a resumed session with more than one user prompt alone", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  ctx.sessionManager.getBranch = () => [
    ...exchange(),
    { type: "message", message: { role: "user", content: "again" } },
    { type: "message", message: { role: "assistant", content: [{ type: "text", text: "done" }] } },
  ];
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();

  assert.equal(ctx.calls.length, 0);
});

test("a failed ask is retried on the next settle", async () => {
  setConfig({ enabled: true, model: null });
  const pi = fakePi();
  activate(pi);
  const ctx = fakeCtx();
  let attempt = 0;
  ctx.modelRegistry.complete = async (chosen, request) => {
    attempt += 1;
    if (attempt === 1) {
      throw new Error("provider hiccup");
    }
    return { content: [{ type: "text", text: '"Fix login redirect"' }] };
  };
  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();
  assert.deepEqual(pi.state.set, []);

  await pi.fire("agent_settled", {}, ctx);
  await pendingTitle();
  assert.deepEqual(pi.state.set, ["Fix login redirect"]);
});
