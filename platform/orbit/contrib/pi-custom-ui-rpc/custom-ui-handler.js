/**
 * `ctx.ui.custom()` handler injected into pi's bundled RPC mode.
 *
 * This file is NOT executed on its own. `apply.mjs` splices its text into the
 * installed `@earendil-works/pi-coding-agent` bundle, immediately before pi's
 * command switch. It closes over `runRpcMode`'s scope through the companion
 * `orbitCustomUiCreate({ output, pending, crypto, getKeybindings })` call that
 * `apply.mjs` appends, so it can emit `extension_ui_request` frames and resolve
 * on the existing `extension_ui_response` path.
 *
 * It restores `ctx.ui.custom()` over RPC by rendering the extension's
 * `Component` to lines and streaming them to the host, which owns only a
 * renderer and an input translator. The wire contract is
 * `crates/orbit-rpc/docs/pi-custom-ui-proposal.md`.
 *
 * The server owns layout: it renders `component.render(width)`, strips pi's
 * `CURSOR_MARKER` and reports the caret position structurally, and routes host
 * input to `component.handleInput(data)`. Overlays are rendered in place of
 * the base component (a full compositor is future work).
 */

// pi-tui's zero-width caret marker (APC: ESC _ pi:c BEL).
const ORBIT_CUSTOM_CURSOR_MARKER = "\u001B_pi:c\u0007";
// ANSI strippers used only to measure the caret column; emitted lines keep
// their SGR so the host can color them.
const ORBIT_CUSTOM_CSI = /\u001B\[[0-9;?]*[ -/]*[@-~]/g;
const ORBIT_CUSTOM_OSC = /\u001B\][^\u0007\u001B]*(?:\u0007|\u001B\\)/g;
const ORBIT_CUSTOM_APC = /\u001B[_^P][^\u0007\u001B]*(?:\u0007|\u001B\\)/g;

/** Strip every escape sequence — used for measuring, never for output. */
function orbitCustomStripAnsi(text) {
  return String(text)
    .replace(ORBIT_CUSTOM_CSI, "")
    .replace(ORBIT_CUSTOM_OSC, "")
    .replace(ORBIT_CUSTOM_APC, "");
}

/** Rough East-Asian/emoji width so the caret lands on the right cell. */
function orbitCustomIsWide(cp) {
  return (
    (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  );
}

/** Display cells of a line, ignoring ANSI and zero-width marks. */
function orbitCustomWidth(text) {
  let width = 0;
  for (const ch of orbitCustomStripAnsi(text)) {
    const cp = ch.codePointAt(0);
    // Combining marks and ZWJ take no cell of their own.
    if (cp === 0x200d || (cp >= 0x0300 && cp <= 0x036f)) continue;
    width += orbitCustomIsWide(cp) ? 2 : 1;
  }
  return width;
}

/** The caret column on a line carrying `CURSOR_MARKER`, or null. */
function orbitCustomCursorIn(line) {
  const at = line.indexOf(ORBIT_CUSTOM_CURSOR_MARKER);
  if (at === -1) return null;
  return orbitCustomWidth(line.slice(0, at));
}

/** The escape sequence starting at `index`, or null. */
function orbitCustomEscapeAt(text, index) {
  if (text[index] !== "\u001B") return null;
  const rest = text.slice(index);
  return (
    rest.match(/^\u001B\[[0-9;?]*[ -/]*[@-~]/)?.[0] ||
    rest.match(/^\u001B\][^\u0007\u001B]*(?:\u0007|\u001B\\)/)?.[0] ||
    rest.match(/^\u001B[_^P][^\u0007\u001B]*(?:\u0007|\u001B\\)/)?.[0] ||
    rest.match(/^\u001B[0-9;?]*[@-~]/)?.[0] ||
    "\u001B"
  );
}

/**
 * Copy the cells in `[start, end)` of a line, replaying the escape sequences
 * seen before `start` so the slice keeps its color, and stopping at `end`
 * (the caller appends a reset). Used to composite an overlay into a base row.
 */
function orbitCustomCells(line, start, end) {
  const text = String(line);
  let out = "";
  let cells = 0;
  let i = 0;
  while (i < text.length && cells < end) {
    const esc = orbitCustomEscapeAt(text, i);
    if (esc) {
      out += esc;
      i += esc.length;
      continue;
    }
    const cp = text.codePointAt(i);
    const ch = String.fromCodePoint(cp);
    const wide = orbitCustomIsWide(cp);
    const width = cp === 0x200d || (cp >= 0x0300 && cp <= 0x036f) ? 0 : wide ? 2 : 1;
    if (cells >= start) out += ch;
    cells += width;
    i += ch.length;
  }
  return out;
}

/** Truncate a line to `cells` visible columns. */
function orbitCustomTruncate(line, cells) {
  if (orbitCustomWidth(line) <= cells) return String(line);
  return `${orbitCustomCells(line, 0, cells)}\u001B[0m`;
}

/** Pad a line on the right with spaces to `cells` visible columns. */
function orbitCustomPadRight(line, cells) {
  const pad = cells - orbitCustomWidth(line);
  return pad > 0 ? line + " ".repeat(pad) : line;
}

/** The overlay's width in columns from `width`/`minWidth`/`maxWidth`, or its
 * widest line, clamped to the viewport. */
function orbitCustomOverlayWidth(lines, opts, viewport) {
  let width;
  if (opts && opts.width != null) {
    if (typeof opts.width === "number") width = opts.width;
    else if (typeof opts.width === "string" && opts.width.endsWith("%")) {
      width = Math.round((viewport * parseFloat(opts.width)) / 100);
    }
  }
  if (width == null) {
    width = lines.reduce((max, line) => Math.max(max, orbitCustomWidth(line)), 0);
  }
  if (opts?.minWidth) width = Math.max(width, opts.minWidth);
  if (opts?.maxWidth) width = Math.min(width, opts.maxWidth);
  return Math.max(1, Math.min(viewport, width));
}

/** Top/left for an overlay from its anchor, margin, and offsets. */
function orbitCustomOverlayOrigin(opts, viewport, overlayWidth, baseHeight, overlayHeight) {
  const anchor = typeof opts?.anchor === "string" ? opts.anchor : "center";
  const margin = typeof opts?.margin === "number" ? opts.margin : 0;
  const offsetX = typeof opts?.offsetX === "number" ? opts.offsetX : 0;
  const offsetY = typeof opts?.offsetY === "number" ? opts.offsetY : 0;
  let top = Math.floor((baseHeight - overlayHeight) / 2);
  let left = Math.floor((viewport - overlayWidth) / 2);
  if (anchor.includes("top")) top = margin;
  else if (anchor.includes("bottom")) top = baseHeight - overlayHeight - margin;
  if (anchor.includes("left")) left = margin;
  else if (anchor.includes("right")) left = viewport - overlayWidth - margin;
  top = Math.max(0, Math.min(Math.max(0, baseHeight - 1), top + offsetY));
  left = Math.max(0, Math.min(viewport - overlayWidth, left + offsetX));
  return { top, left };
}

/** Composite an overlay's lines onto the base rows at the resolved origin. */
function orbitCustomComposite(base, overlay, opts, viewport) {
  const overlayWidth = orbitCustomOverlayWidth(overlay, opts, viewport);
  const { top, left } = orbitCustomOverlayOrigin(
    opts,
    viewport,
    overlayWidth,
    base.length,
    overlay.length,
  );
  const rows = base.slice();
  while (rows.length < top + overlay.length) rows.push("");
  const RESET = "\u001B[0m";
  for (let r = 0; r < overlay.length; r += 1) {
    const row = top + r;
    const baseLine = rows[row] ?? "";
    const leftPart = orbitCustomPadRight(orbitCustomCells(baseLine, 0, left), left);
    const rightPart = orbitCustomCells(baseLine, left + overlayWidth, viewport);
    const overlayLine = orbitCustomPadRight(
      orbitCustomTruncate(overlay[r], overlayWidth),
      overlayWidth,
    );
    rows[row] = `${leftPart}${RESET}${overlayLine}${RESET}${rightPart}`;
  }
  return rows;
}

/** Real keybindings when reachable in the bundle scope, else a safe stub. */
function orbitCustomKeybindings(getKeybindings) {
  if (typeof getKeybindings === "function") {
    try {
      const real = getKeybindings();
      if (real) return real;
    } catch {
      /* fall through to the stub */
    }
  }
  return {
    matches: () => false,
    getKeybinding: () => undefined,
    getKeybindings: () => ({}),
    formatKeybinding: (value) => String(value ?? ""),
    formatKey: (value) => String(value ?? ""),
  };
}

/** Render a component defensively; a throwing component must not break the
 * surface. */
function orbitCustomRenderLines(component, width) {
  let lines = [];
  try {
    lines = component?.render(width) || [];
  } catch (err) {
    lines = [`[custom UI render error] ${err?.message || err}`];
  }
  if (!Array.isArray(lines)) lines = [String(lines)];
  return lines.map((line) => String(line));
}

/**
 * Build the RPC `custom` driver over `runRpcMode`'s closure. Returns
 * `{ run, input, resize, capabilities }`; `run` is invoked by the patched
 * `ctx.ui.custom`, the other two by the two injected commands.
 */
function orbitCustomUiCreate({ output, pending, crypto, getKeybindings }) {
  const active = new Map();

  const randomId = () => {
    if (crypto && typeof crypto.randomUUID === "function") return crypto.randomUUID();
    return `orbit-custom-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  };

  const keybindings = orbitCustomKeybindings(getKeybindings);

  /** Render the live component, compositing any overlay, into lines + caret. */
  function renderFrame(state) {
    let lines = orbitCustomRenderLines(state.component, state.width);
    if (state.overlay) {
      const options =
        typeof state.overlay.options === "function"
          ? state.overlay.options()
          : state.overlay.options || {};
      // Measure at the viewport, then re-render at the overlay's own width so
      // a no-explicit-width overlay stays as narrow as its content.
      let overlay = orbitCustomRenderLines(state.overlay.component, state.width);
      const overlayWidth = orbitCustomOverlayWidth(overlay, options, state.width);
      if (overlayWidth !== state.width) {
        overlay = orbitCustomRenderLines(state.overlay.component, overlayWidth);
      }
      lines = orbitCustomComposite(lines, overlay, options, state.width);
    }
    let cursor = null;
    const clean = lines.map((line, row) => {
      const text = String(line);
      const col = orbitCustomCursorIn(text);
      if (col !== null && cursor === null) cursor = { row, col };
      return text.split(ORBIT_CUSTOM_CURSOR_MARKER).join("");
    });
    return { lines: clean, cursor };
  }

  function emit(state, phase, extra) {
    const { lines, cursor } = renderFrame(state);
    output({
      type: "extension_ui_request",
      id: state.id,
      method: "custom",
      phase,
      width: state.width,
      lines,
      cursor,
      ...extra,
    });
  }

  /** Coalesce repaints to one frame per tick so a burst is a single render. */
  function schedule(state) {
    if (state.closed || state.pendingRender) return;
    state.pendingRender = true;
    setTimeout(() => {
      state.pendingRender = false;
      if (!state.closed) emit(state, "render");
    }, 0);
  }

  function finish(state, result, cancelled) {
    if (state.closed) return;
    state.closed = true;
    active.delete(state.id);
    pending.delete(state.id);
    if (!cancelled) {
      output({
        type: "extension_ui_request",
        id: state.id,
        method: "custom",
        phase: "close",
        result: result === undefined ? null : result,
      });
    }
    try {
      state.component?.dispose?.();
    } catch {
      /* dispose is best-effort */
    }
    state.resolve(result);
  }

  async function run({ factory, options, theme }) {
    const id = randomId();
    const state = {
      id,
      component: null,
      overlay: null,
      width: 80,
      height: 24,
      closed: false,
      pendingRender: false,
      listeners: new Set(),
      resolve: null,
    };
    const result = new Promise((resolve) => {
      state.resolve = resolve;
    });
    // The host cancels through `extension_ui_response {id,cancelled:true}`;
    // riding the existing pending map means no new response path is needed.
    pending.set(id, {
      resolve: (response) => {
        if (response && "cancelled" in response && response.cancelled) {
          finish(state, undefined, true);
        }
      },
      reject: () => finish(state, undefined, true),
    });

    const tui = {
      mode: "regular",
      terminal: {
        get columns() {
          return state.width;
        },
        get rows() {
          return state.height;
        },
      },
      requestRender() {
        schedule(state);
      },
      setFocus() {
        /* the host owns focus while the surface is open */
      },
      showOverlay(component, overlayOptions) {
        state.overlay = { component, options: overlayOptions };
        schedule(state);
        return {
          focus() {},
          unfocus() {},
          setHidden() {},
          hide() {
            state.overlay = null;
            schedule(state);
          },
          isVisible: () => state.overlay !== null,
          getBounds: () => undefined,
        };
      },
      hideOverlay() {
        state.overlay = null;
        schedule(state);
      },
      hasOverlay() {
        return state.overlay !== null;
      },
      addInputListener(listener) {
        state.listeners.add(listener);
        return () => state.listeners.delete(listener);
      },
      removeInputListener(listener) {
        state.listeners.delete(listener);
      },
    };

    try {
      state.component = await factory(tui, theme, keybindings, (value) =>
        finish(state, value, false),
      );
    } catch (err) {
      finish(state, undefined, true);
      throw err;
    }
    active.set(id, state);
    emit(state, "open", {
      height: state.height,
      keyboard: "legacy",
      mouse: false,
      overlay: options?.overlay === true,
    });
    return result;
  }

  function input(id, data) {
    const state = active.get(id);
    if (!state || state.closed) return;
    let payload = data;
    for (const listener of state.listeners) {
      try {
        const result = listener(payload);
        if (result && result.consume) return;
        if (result && typeof result.data === "string") payload = result.data;
      } catch {
        /* one listener must not break the others */
      }
    }
    try {
      state.component?.handleInput?.(payload);
    } catch {
      /* a component input error must not break the run */
    }
    schedule(state);
  }

  function resize(id, width, height) {
    const state = active.get(id);
    if (!state || state.closed) return;
    const next = Number(width);
    if (Number.isFinite(next)) state.width = Math.min(240, Math.max(20, Math.trunc(next)));
    const nextHeight = Number(height);
    if (Number.isFinite(nextHeight)) state.height = Math.max(1, Math.trunc(nextHeight));
    schedule(state);
  }

  function capabilities() {
    return {
      extension_ui: [
        "select",
        "confirm",
        "input",
        "editor",
        "notify",
        "setStatus",
        "setWidget",
        "setTitle",
        "set_editor_text",
        "custom",
      ],
    };
  }

  return { run, input, resize, capabilities };
}
