#!/usr/bin/env node
/**
 * Tests for the pure workflow policy.
 *
 * Run: node --test contrib/orbit-workflow-extension/policy.test.mjs
 */

import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_MODE,
  allowedTools,
  applyToolScoping,
  blockReason,
  guidance,
  isSafeCommand,
  normalizeMode,
} from "./policy.js";

test("normalizeMode falls back to build", () => {
  assert.equal(normalizeMode("plan"), "plan");
  assert.equal(normalizeMode("ask"), "ask");
  assert.equal(normalizeMode("yolo"), DEFAULT_MODE);
  assert.equal(normalizeMode(undefined), DEFAULT_MODE);
});

test("allowedTools subtracts the disabled write tools", () => {
  const base = ["read", "bash", "edit", "write", "grep"];
  assert.deepEqual(allowedTools("plan", base), ["read", "bash", "grep"]);
  assert.deepEqual(allowedTools("ask", base), ["read", "bash", "grep"]);
  assert.deepEqual(allowedTools("build", base), base);
  // Unknown names are ignored, duplicates collapse.
  assert.deepEqual(allowedTools("plan", ["read", "read", "nope", "edit"]), ["read", "nope"]);
});

test("applyToolScoping keeps tools activated after session start", () => {
  // Session start: no codemode yet.
  let state = applyToolScoping("build", ["read", "edit", "write", "bash"], []);
  assert.deepEqual(state.tools, ["read", "edit", "write", "bash"]);
  // Pi's MCP extension activates `codemode` once a codemode server connects.
  const active = [...state.tools, "codemode"];
  // A mode change must not drop a tool this extension does not own.
  state = applyToolScoping("plan", active, state.removed);
  assert.deepEqual(state.tools, ["read", "bash", "codemode"]);
  // Switching back restores the writes and still keeps codemode.
  state = applyToolScoping("build", state.tools, state.removed);
  assert.deepEqual(state.tools, ["read", "bash", "codemode", "edit", "write"]);
  // Nothing is left to restore once the mode allows everything.
  assert.deepEqual(state.removed, []);
});

test("applyToolScoping restores only what it removed", () => {
  const state = applyToolScoping("plan", ["read", "edit"], []);
  assert.deepEqual(state.tools, ["read"]);
  assert.deepEqual(state.removed.sort(), ["edit"]);
  // `grep` was never active (another extension removed it); this extension
  // must not resurrect it when the mode changes.
  const next = applyToolScoping("build", ["read", "codemode"], state.removed);
  assert.deepEqual(next.tools, ["read", "codemode", "edit"]);
  assert.deepEqual(next.removed, []);
});

test("build never blocks", () => {
  assert.equal(blockReason("build", "edit", {}), undefined);
  assert.equal(blockReason("build", "bash", { command: "rm -rf /" }), undefined);
});

test("plan and ask block the write tools", () => {
  assert.match(blockReason("plan", "edit", {}), /edit is disabled/);
  assert.match(blockReason("ask", "write", {}), /write is disabled/);
});

test("plan and ask gate bash by the allowlist", () => {
  assert.equal(blockReason("plan", "bash", { command: "git log --oneline" }), undefined);
  assert.equal(blockReason("ask", "bash", { command: "rg TODO src" }), undefined);
  // The AI reviewer's own commands: a read-only diff runs, a commit is blocked.
  assert.equal(blockReason("ask", "bash", { command: "git diff HEAD" }), undefined);
  assert.match(blockReason("plan", "bash", { command: "rm -rf build" }), /blocked/);
  assert.match(blockReason("ask", "bash", { command: "git commit -m x" }), /blocked/);
  // Compound/redirect/substitution are rejected outright.
  for (const command of ["cat a; rm b", "cat a && rm b", "cat a | sh", "echo x > f", "echo $(rm x)", "echo `rm x`", "ls\nrm x"]) {
    assert.equal(isSafeCommand(command), false, `${command} must be rejected`);
  }
  assert.equal(isSafeCommand(""), false);
});

test("non-bash tools are not gated", () => {
  assert.equal(blockReason("plan", "grep", { pattern: "x" }), undefined);
  assert.equal(blockReason("plan", "read", { path: "x" }), undefined);
});

test("guidance differs per mode", () => {
  assert.match(guidance("plan"), /PLAN MODE/);
  assert.match(guidance("ask"), /ASK MODE/);
  assert.equal(guidance("build"), "");
});
