#!/usr/bin/env node
/**
 * Behaviour tests for the Orbit workflow extension entry.
 *
 * Run: node --test contrib/orbit-workflow-extension/index.test.mjs
 *
 * Drives the extension the way pi does — `activate(pi)`, then lifecycle hooks
 * — with a fake `pi` and a fake `ctx`, asserting tool scoping, guidance
 * injection.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test, { beforeEach } from "node:test";

import activate, { resolveMode } from "./index.js";

let home;

beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "orbit-workflow-entry-"));
  fs.mkdirSync(path.join(home, ".orbit-pi"), { recursive: true });
  process.env.HOME = home;
  delete process.env.ORBIT_WORKFLOW_MODE;
});

function writeStore(map) {
  fs.writeFileSync(path.join(home, ".orbit-pi", "workflow.json"), JSON.stringify(map));
}

function fakePi() {
  const handlers = new Map();
  const state = { tools: ["read", "bash", "edit", "write", "grep"], entries: [] };
  return {
    state,
    on(event, handler) {
      handlers.set(event, handler);
    },
    fire(event, payload, ctx) {
      const handler = handlers.get(event);
      assert.ok(handler, `no handler for ${event}`);
      return handler(payload, ctx);
    },
    getActiveTools() {
      return [...state.tools];
    },
    setActiveTools(names) {
      state.tools = [...names];
    },
    appendEntry(customType, data) {
      state.entries.push({ type: "custom", customType, data });
    },
  };
}

function fakeCtx(sessionId = "s1", branch = [], entries = branch) {
  return {
    hasUI: true,
    sessionManager: {
      getSessionId: () => sessionId,
      getBranch: () => branch,
      getEntries: () => entries,
    },
  };
}

test("plan mode disables write tools on session start", async () => {
  writeStore({ s1: "plan" });
  const pi = fakePi();
  activate(pi);
  await pi.fire("session_start", {}, fakeCtx());
  assert.deepEqual(pi.state.tools, ["read", "bash", "grep"]);
  const modeEntry = pi.state.entries.find((e) => e.customType === "orbit:workflow");
  assert.equal(modeEntry.data.mode, "plan");
});

test("build keeps every tool", async () => {
  writeStore({ s1: "build" });
  const pi = fakePi();
  activate(pi);
  await pi.fire("session_start", {}, fakeCtx());
  assert.deepEqual(pi.state.tools, ["read", "bash", "edit", "write", "grep"]);
});

test("before_agent_start injects guidance and re-arms the mode live", async () => {
  writeStore({ s1: "plan" });
  const pi = fakePi();
  activate(pi);
  await pi.fire("session_start", {}, fakeCtx());
  const result = await pi.fire("before_agent_start", {}, fakeCtx());
  assert.equal(result.message.customType, "orbit-workflow-context");
  assert.match(result.message.content, /PLAN MODE/);

  // The store changed underneath: the next hook re-reads it (live re-arm).
  writeStore({ s1: "build" });
  const next = await pi.fire("before_agent_start", {}, fakeCtx());
  assert.equal(next, undefined); // build with no plan adds nothing
  assert.deepEqual(pi.state.tools, ["read", "bash", "grep", "edit", "write"]);
});

test("a tool activated after session start survives a mode change", async () => {
  // The MCP extension activates `codemode` after `session_start`, once a
  // server with that exposure connects. A later workflow-mode change used to
  // rebuild from a session-start snapshot and drop it silently.
  writeStore({ s1: "build" });
  const pi = fakePi();
  activate(pi);
  await pi.fire("session_start", {}, fakeCtx());
  pi.state.tools.push("codemode");

  writeStore({ s1: "plan" });
  await pi.fire("before_agent_start", {}, fakeCtx());
  assert.deepEqual(pi.state.tools, ["read", "bash", "grep", "codemode"]);

  writeStore({ s1: "build" });
  await pi.fire("before_agent_start", {}, fakeCtx());
  assert.deepEqual(pi.state.tools, [
    "read",
    "bash",
    "grep",
    "codemode",
    "edit",
    "write",
  ]);
});

test("tool_call blocks writes in read-only modes and passes in build", async () => {
  writeStore({ s1: "ask" });
  const pi = fakePi();
  activate(pi);
  await pi.fire("session_start", {}, fakeCtx());
  const blocked = await pi.fire("tool_call", { toolName: "edit", input: {} }, fakeCtx());
  assert.equal(blocked.block, true);
  const unsafe = await pi.fire(
    "tool_call",
    { toolName: "bash", input: { command: "rm -rf build" } },
    fakeCtx(),
  );
  assert.equal(unsafe.block, true);
  const safe = await pi.fire(
    "tool_call",
    { toolName: "bash", input: { command: "git status" } },
    fakeCtx(),
  );
  assert.equal(safe, undefined);

  writeStore({ s1: "build" });
  assert.equal(await pi.fire("tool_call", { toolName: "edit", input: {} }, fakeCtx()), undefined);
});

test("the store for the session id wins over the env fallback", () => {
  writeStore({ s1: "ask" });
  process.env.ORBIT_WORKFLOW_MODE = "plan";
  assert.equal(resolveMode(fakeCtx("s1"), []), "ask");
  delete process.env.ORBIT_WORKFLOW_MODE;
  // No store entry: env applies.
  process.env.ORBIT_WORKFLOW_MODE = "plan";
  assert.equal(resolveMode(fakeCtx("missing"), []), "plan");
  delete process.env.ORBIT_WORKFLOW_MODE;
  // No store, no env: the session entry, then Build.
  const branch = [{ type: "custom", customType: "orbit:workflow", data: { mode: "ask" } }];
  assert.equal(resolveMode(fakeCtx("missing"), branch), "ask");
  assert.equal(resolveMode(fakeCtx("missing"), []), "build");
});

test("context drops stale guidance only when the mode adds none", async () => {
  writeStore({ s1: "build" });
  const pi = fakePi();
  activate(pi);
  const result = await pi.fire(
    "context",
    { messages: [{ customType: "orbit-workflow-context" }, { role: "user" }] },
    fakeCtx(),
  );
  assert.equal(result.messages.length, 1);
  // Nothing to filter → no transform.
  assert.equal(await pi.fire("context", { messages: [{ role: "user" }] }, fakeCtx()), undefined);
});

test("context keeps active guidance in Plan/Ask", async () => {
  writeStore({ s1: "plan" });
  const pi = fakePi();
  activate(pi);
  const kept = await pi.fire(
    "context",
    { messages: [{ customType: "orbit-workflow-context" }, { role: "user" }] },
    fakeCtx(),
  );
  assert.equal(kept, undefined);
});
