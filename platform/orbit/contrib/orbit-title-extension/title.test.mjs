import test from "node:test";
import assert from "node:assert/strict";

import {
  buildTitlePrompt,
  extractText,
  firstExchange,
  isStaleContext,
  isTitleable,
  normalizeTitle,
  parseConfig,
  resolveModel,
  titleFromResponse,
} from "./title.js";

test("parseConfig defaults to enabled on the active model", () => {
  assert.deepEqual(parseConfig(null), { enabled: true, model: null });
  assert.deepEqual(parseConfig("not json"), { enabled: true, model: null });
  assert.deepEqual(parseConfig(""), { enabled: true, model: null });
  assert.deepEqual(parseConfig("{}"), { enabled: true, model: null });
});

test("parseConfig honors disabled and a model override", () => {
  assert.deepEqual(parseConfig('{"enabled":false}'), { enabled: false, model: null });
  assert.deepEqual(parseConfig('{"enabled":true,"model":{"provider":"ollama","id":"glm-5.3"}}'), {
    enabled: true,
    model: { provider: "ollama", id: "glm-5.3" },
  });
});

test("parseConfig rejects a malformed model without failing", () => {
  assert.deepEqual(parseConfig('{"model":{"provider":"","id":"x"}}').model, null);
  assert.deepEqual(parseConfig('{"model":{"provider":"x"}}').model, null);
  assert.deepEqual(parseConfig('{"model":"ollama/glm"}').model, null);
});

test("extractText reads string and block-array content", () => {
  assert.equal(extractText("  hi  "), "hi");
  assert.equal(
    extractText([
      { type: "text", text: "one" },
      { type: "thinking", thinking: "ignored" },
      { type: "text", text: "two" },
    ]),
    "one\ntwo",
  );
  assert.equal(extractText(undefined), "");
});

test("firstExchange finds the first user and assistant text and counts prompts", () => {
  const entries = [
    { type: "model_change" },
    { type: "message", message: { role: "user", content: "first prompt" } },
    { type: "message", message: { role: "assistant", content: [{ type: "toolCall" }] } },
    { type: "message", message: { role: "assistant", content: [{ type: "text", text: "answer" }] } },
    { type: "message", message: { role: "user", content: "second prompt" } },
    { type: "message", message: { role: "assistant", content: [{ type: "text", text: "second answer" }] } },
  ];
  // The tool-call reply has no text, so the first textual assistant wins;
  // user prompts (not replies) separate a fresh session from a resumed one.
  assert.deepEqual(firstExchange(entries), {
    user: "first prompt",
    assistant: "answer",
    userCount: 2,
  });
});

test("isTitleable requires both sides of the exchange", () => {
  assert.equal(isTitleable("prompt", "answer"), true);
  assert.equal(isTitleable("prompt", ""), false);
  assert.equal(isTitleable("", "answer"), false);
});

test("buildTitlePrompt carries the first exchange", () => {
  const prompt = buildTitlePrompt("fix the login bug", "I found the redirect loop");
  assert.match(prompt, /3 to 6 words/);
  assert.match(prompt, /fix the login bug/);
  assert.match(prompt, /I found the redirect loop/);
});

test("normalizeTitle strips quotes, punctuation, and caps length", () => {
  assert.equal(normalizeTitle('  "Fix login bug."  '), "Fix login bug");
  assert.equal(normalizeTitle("`Refactor auth module`"), "Refactor auth module");
  assert.equal(normalizeTitle("one\n\ntwo"), "one two");
  const long = normalizeTitle("x".repeat(80));
  assert.equal(long.length, 60);
  assert.ok(long.endsWith("…"));
});

test("titleFromResponse joins text blocks and normalizes", () => {
  assert.equal(
    titleFromResponse({ content: [{ type: "text", text: '"Fix' }, { type: "text", text: 'login bug"' }] }),
    "Fix login bug",
  );
  assert.equal(titleFromResponse({ content: [] }), "");
  assert.equal(titleFromResponse(undefined), "");
});

test("resolveModel prefers the configured model, else the active one", () => {
  const model = { provider: "ollama", id: "glm-5.3" };
  const ctx = {
    model,
    modelRegistry: {
      find: (provider, id) => (provider === "ollama" && id === "glm-5.3" ? { id } : undefined),
      hasConfiguredAuth: () => true,
    },
  };
  assert.equal(resolveModel(ctx, { enabled: true, model: null }), model);
  assert.deepEqual(resolveModel(ctx, { enabled: true, model: { provider: "ollama", id: "glm-5.3" } }), {
    id: "glm-5.3",
  });
  // Unknown or unauthenticated override falls back to the active model.
  assert.equal(resolveModel(ctx, { enabled: true, model: { provider: "openai", id: "missing" } }), model);
  const noAuth = { ...ctx, modelRegistry: { ...ctx.modelRegistry, hasConfiguredAuth: () => false } };
  assert.equal(resolveModel(noAuth, { enabled: true, model: { provider: "ollama", id: "glm-5.3" } }), model);
});

test("isStaleContext recognizes a switched session", () => {
  assert.equal(isStaleContext(new Error("This extension ctx is stale")), true);
  assert.equal(isStaleContext(new Error("boom")), false);
});
