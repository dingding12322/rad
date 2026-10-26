#!/usr/bin/env node
/**
 * Unit tests for the custom-UI RPC handler.
 *
 * The handler is injected into pi's bundle and closes over `runRpcMode`'s
 * scope. To test it in isolation we evaluate its source with the few globals
 * it touches stubbed, then drive the returned API directly.
 *
 * Run: node --test contrib/pi-custom-ui-rpc/custom-ui-handler.test.mjs
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));

function loadHandler() {
  const source = fs.readFileSync(path.join(HERE, "custom-ui-handler.js"), "utf8");
  const frames = [];
  const pending = new Map();
  const crypto = { randomUUID: () => "test-id" };
  const factory = new Function(
    "output",
    "pending",
    "crypto",
    "getKeybindings",
    `${source}; return { orbitCustomUiCreate, orbitCustomStripAnsi, orbitCustomWidth, orbitCustomCursorIn, orbitCustomKeybindings, orbitCustomCells, orbitCustomTruncate, orbitCustomPadRight, orbitCustomOverlayOrigin, orbitCustomComposite };`,
  );
  const exports = factory(
    (frame) => frames.push(frame),
    pending,
    crypto,
    undefined,
  );
  const ui = exports.orbitCustomUiCreate({
    output: (frame) => frames.push(frame),
    pending,
    crypto,
    getKeybindings: undefined,
  });
  return { ui, exports, frames, pending };
}

/** The `open` frame lands a microtask after `run`; wait for it like the host. */
async function openFrame(frames) {
  for (let i = 0; i < 100 && !frames.some((frame) => frame.phase === "open"); i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
  return frames.find((frame) => frame.phase === "open");
}

test("ANSI and width helpers measure the caret", () => {
  const { exports } = loadHandler();
  assert.equal(exports.orbitCustomStripAnsi("\u001B[38;5;2mgreen\u001B[39m"), "green");
  assert.equal(exports.orbitCustomWidth("\u001B[1mA\u001B[22m"), 1);
  assert.equal(exports.orbitCustomWidth("宽"), 2);
  assert.equal(exports.orbitCustomCursorIn("ab\u001B_pi:c\u0007cd"), 2);
  assert.equal(exports.orbitCustomCursorIn("no marker"), null);
});

test("cell slicing preserves color and truncates", () => {
  const { exports } = loadHandler();
  const line = "\u001B[31mABCDEF\u001B[39m";
  assert.equal(exports.orbitCustomStripAnsi(exports.orbitCustomCells(line, 1, 4)), "BCD");
  // The color seen before the slice is replayed so it keeps its styling.
  assert.ok(exports.orbitCustomCells(line, 2, 4).startsWith("\u001B[31m"));
  assert.equal(exports.orbitCustomStripAnsi(exports.orbitCustomTruncate(line, 3)), "ABC");
  assert.equal(exports.orbitCustomTruncate(line, 6), line);
});

test("run streams an open frame with lines and caret", async () => {
  const { ui, frames } = loadHandler();
  const component = {
    render: (width) => [`width=${width}`, `second\u001B_pi:c\u0007line`],
    handleInput() {},
  };
  const result = ui.run({
    factory: async () => component,
    options: {},
    theme: { fg: (_color, text) => text },
  });

  const open = await openFrame(frames);
  assert.ok(open, "an open frame was emitted");
  assert.equal(open.id, "test-id");
  assert.equal(open.method, "custom");
  assert.deepEqual(open.lines, ["width=80", "secondline"]);
  assert.deepEqual(open.cursor, { row: 1, col: 6 });
  assert.equal(open.keyboard, "legacy");
  assert.equal(open.overlay, false);

  // The promise stays pending until the component calls `done`.
  assert.equal(typeof result.then, "function");
});

test("input reaches handleInput and schedules a render", async () => {
  const { ui, frames } = loadHandler();
  const received = [];
  const component = {
    render: () => ["row"],
    handleInput: (data) => received.push(data),
  };
  ui.run({ factory: async () => component, options: {}, theme: {} });
  await openFrame(frames);

  ui.input("test-id", "\u001B[B");
  assert.deepEqual(received, ["\u001B[B"]);
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.ok(frames.some((frame) => frame.phase === "render"));
});

test("resize re-renders at the requested width", async () => {
  const { ui, frames } = loadHandler();
  const component = { render: (width) => [`w=${width}`], handleInput() {} };
  ui.run({ factory: async () => component, options: {}, theme: {} });
  await openFrame(frames);

  ui.resize("test-id", 100, 30);
  await new Promise((resolve) => setTimeout(resolve, 5));
  const render = frames.find((frame) => frame.phase === "render" && frame.width === 100);
  assert.ok(render, "a 100-column render frame was emitted");
  assert.deepEqual(render.lines, ["w=100"]);
});

test("done resolves the promise and emits close", async () => {
  const { ui, frames } = loadHandler();
  let done;
  const component = { render: () => ["pick"], handleInput() {} };
  const result = ui.run({
    factory: async (_tui, _theme, _kb, resolve) => {
      done = resolve;
      return component;
    },
    options: {},
    theme: {},
  });

  await openFrame(frames);
  done("uncommitted");
  assert.equal(await result, "uncommitted");
  const close = frames.find((frame) => frame.phase === "close");
  assert.ok(close);
  assert.equal(close.result, "uncommitted");
});

test("extension_ui_response cancel resolves with undefined and no close", async () => {
  const { ui, frames, pending } = loadHandler();
  const component = { render: () => ["x"], handleInput() {} };
  const result = ui.run({ factory: async () => component, options: {}, theme: {} });
  await openFrame(frames);

  pending.get("test-id").resolve({ cancelled: true });
  assert.equal(await result, undefined);
  assert.ok(!frames.some((frame) => frame.phase === "close"));
});

test("input listeners can transform or consume data", async () => {
  const { ui, frames } = loadHandler();
  const received = [];
  const component = { render: () => ["x"], handleInput: (data) => received.push(data) };
  ui.run({
    factory: async (tui) => {
      tui.addInputListener((data) => (data === "a" ? { data: "z" } : undefined));
      tui.addInputListener((data) => (data === "z" ? { consume: true } : undefined));
      return component;
    },
    options: {},
    theme: {},
  });

  await openFrame(frames);
  ui.input("test-id", "a"); // transformed to z, then consumed
  assert.deepEqual(received, []);
  ui.input("test-id", "b");
  assert.deepEqual(received, ["b"]);
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.ok(frames.length > 0);
});

test("showOverlay composites a centered overlay onto the base", async () => {
  const { ui, exports, frames } = loadHandler();
  const base = {
    render: (width) =>
      Array.from({ length: 5 }, (_, i) => `base-${i}`.padEnd(width, ".")),
    handleInput() {},
  };
  const overlay = { render: () => ["OVERLAY"], handleInput() {} };
  ui.run({
    factory: async (tui) => {
      tui.showOverlay(overlay, { width: 9 });
      return base;
    },
    options: {},
    theme: {},
  });

  const open = await openFrame(frames);
  const mid = open.lines[2];
  assert.ok(mid.includes("OVERLAY"), `overlay lands in the middle row: ${JSON.stringify(mid)}`);
  const col = exports.orbitCustomWidth(mid.slice(0, mid.indexOf("OVERLAY")));
  assert.equal(col, Math.floor((open.width - 9) / 2));
  // Base content survives to the right of the overlay.
  assert.ok(exports.orbitCustomStripAnsi(mid).endsWith(".."));
});

test("overlay anchors to a corner with a margin", async () => {
  const { ui, exports, frames } = loadHandler();
  const base = {
    render: (width) => Array.from({ length: 5 }, () => "b".repeat(width)),
    handleInput() {},
  };
  const overlay = { render: () => ["TOP"], handleInput() {} };
  ui.run({
    factory: async (tui) => {
      tui.showOverlay(overlay, { width: 3, anchor: "top-left", margin: 1 });
      return base;
    },
    options: {},
    theme: {},
  });

  const open = await openFrame(frames);
  const col = exports.orbitCustomWidth(
    open.lines[1].slice(0, open.lines[1].indexOf("TOP")),
  );
  assert.equal(col, 1);
});

test("unknown ids are ignored and capabilities advertise custom", () => {
  const { ui } = loadHandler();
  assert.doesNotThrow(() => {
    ui.input("nope", "x");
    ui.resize("nope", 100, 10);
  });
  assert.ok(ui.capabilities().extension_ui.includes("custom"));
});
