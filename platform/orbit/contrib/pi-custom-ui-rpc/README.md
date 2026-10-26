# pi `ctx.ui.custom()` RPC patch

Orbit renders an extension's custom TUI component (`/review`'s preset selector,
wizards, overlays) in a native modal. pi's RPC server has no such capability —
its `createExtensionUIContext().custom()` is a stub returning `undefined` — so
this patch teaches the installed pi's bundled RPC mode to render the component
to lines and stream them to the host, which answers with input and resize
commands.

The client contract is
[`../../crates/orbit-rpc/docs/pi-custom-ui-proposal.md`](../../crates/orbit-rpc/docs/pi-custom-ui-proposal.md),
and the injected code is
[`custom-ui-handler.js`](./custom-ui-handler.js). The Orbit host side lives in
`crates/orbit-pi/src/custom_ui.rs`.

## What it adds to pi

- **`ctx.ui.custom()` over RPC** — the component is rendered with
  `component.render(width)`; each repaint streams an
  `extension_ui_request` `method:"custom"` frame (`open` / `render` / `close`).
  Input arrives as `extension_ui_input {id,data}` and drives
  `component.handleInput(data)`; `extension_ui_resize {id,width,height}`
  re-renders at the host's viewport. `done(result)` emits the `close` frame and
  resolves the extension's promise.
- **Caret** — pi's `CURSOR_MARKER` is stripped and reported structurally as
  `cursor {row,col}` so the host can place its own caret.
- **Overlays** — `tui.showOverlay` is composited into the frame (anchors,
  margins, widths, offsets) with ANSI-aware cell slicing, so nested overlays
  render instead of replacing the base.
- **Capability** — `get_state` now advertises
  `capabilities.extension_ui`, including `"custom"`, so hosts can feature-detect
  instead of guessing.

## Apply

```sh
node contrib/pi-custom-ui-rpc/apply.mjs
```

Then restart the agent: **Settings → Runtime → Restart**, or reopen Orbit.

The script resolves `pi` on `PATH` (override with `PI_BIN`), locates the pi
package — an npm/pnpm/bun global or pi's managed installer (override with
`PI_PACKAGE_ROOT`) — finds the bundled RPC-mode chunk, and splices the handler
in at four verified-unique seams:
before the command switch, at the command `default` case, at the `custom` stub,
and at the `get_state` response. It is idempotent (re-running updates the
injected block in place), and writes a `<file>.orbit-orig` backup next to the
patched file — **shared with the quota and auth patches**, so reverting restores
the state from before *any* of them.

## Revert

```sh
node contrib/pi-custom-ui-rpc/apply.mjs --revert
```

## Tests

```sh
node --test contrib/pi-custom-ui-rpc/custom-ui-handler.test.mjs
```

The tests evaluate the handler in a stub `runRpcMode` scope and cover the
ANSI/width/caret helpers, `open`/`input`/`resize`/`close`, promise resolution
via `done`, cancellation through the existing `extension_ui_response` path, and
input-listener transform/consume.

## Limitations

- **Overlays** are composited server-side (anchor, margin, `width`/`minWidth`/
  `maxWidth`, offsets, dynamic option functions) with ANSI-aware cell slicing, so
  the base keeps its colors around the overlay. A wide glyph straddling the
  overlay edge is dropped rather than split.
- **Keyboard** is the legacy byte subset only; Kitty keyboard mode and mouse are
  not advertised.
- The first frame renders at a default 80 columns; the host's `resize` follows
  immediately, so the initial frame is a one-tick flash.

This modifies a global npm install. Re-run after `pi update`.
