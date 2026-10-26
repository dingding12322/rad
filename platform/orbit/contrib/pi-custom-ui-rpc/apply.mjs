#!/usr/bin/env node
/**
 * Apply the `ctx.ui.custom()` RPC capability to an installed pi build.
 *
 * pi's RPC mode stubs `custom()` to `undefined`, so any extension whose UI is
 * a TUI component (pi-review's `/review` selector, wizards, overlays) silently
 * does nothing in an RPC host. This script patches pi's bundled RPC mode to
 * render the component to lines and stream them to the host, which answers
 * with input and resize commands.
 *
 * The matching client contract is crates/orbit-rpc/docs/pi-custom-ui-proposal.md;
 * the injected code is contrib/pi-custom-ui-rpc/custom-ui-handler.js.
 *
 * Usage:
 *   node contrib/pi-custom-ui-rpc/apply.mjs            # patch the resolved pi
 *   node contrib/pi-custom-ui-rpc/apply.mjs --revert   # restore the backup
 *   PI_BIN=/path/to/pi node contrib/pi-custom-ui-rpc/apply.mjs
 *
 * The patch is marker-delimited and idempotent, and writes a `.orbit-orig`
 * backup next to the patched file (shared with the quota/auth patches — see
 * their READMEs).
 *
 * This modifies a global npm install. Re-run after `pi update`.
 */

import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MARKER_BEGIN = "/*orbit-custom-ui-rpc:begin*/";
const MARKER_END = "/*orbit-custom-ui-rpc:end*/";

// Seams inside the bundled RPC mode. All four verified unique per chunk.
const ANCHOR_HELPERS = "let handleCommand=async command=>{";
const ANCHOR_CASES =
  "default:{let unknownCommand=command;return error(id,unknownCommand.type," +
  "`Unknown command: ${unknownCommand.type}`)}";
const ANCHOR_CUSTOM = "async custom(){},";
const ANCHOR_GET_STATE = 'return success(id,"get_state",state2)';

// `theme` is the module-level theme the UI context exposes; it resolves inside
// the `custom` method exactly as it does in the `get theme()` getter.
const CUSTOM_CASES =
  'case"extension_ui_input":orbitCustomUi.input(command.id,command.data);return;' +
  'case"extension_ui_resize":orbitCustomUi.resize(command.id,command.width,command.height);return;';
const CUSTOM_METHOD = "async custom(factory,options){return orbitCustomUi.run({factory,options,theme})},";
const GET_STATE_WITH_CAPABILITIES =
  'return success(id,"get_state",{...state2,capabilities:orbitCustomUi.capabilities()})';

function resolvePiBin() {
  if (process.env.PI_BIN) return process.env.PI_BIN;
  try {
    return execSync("command -v pi", { encoding: "utf8" }).trim();
  } catch {
    return "";
  }
}

const PI_PACKAGE = "@earendil-works/pi-coding-agent";

function packageRoot(bin) {
  // Orbit resolves the package (npm/pnpm global or pi's managed installer) and
  // passes it in, so the script works on layouts its own walk cannot reach.
  if (process.env.PI_PACKAGE_ROOT) return process.env.PI_PACKAGE_ROOT;
  const real = fs.realpathSync(bin);
  let dir = path.dirname(real);
  for (let i = 0; i < 12; i += 1) {
    const pkg = path.join(dir, "package.json");
    if (fs.existsSync(pkg)) {
      try {
        if (JSON.parse(fs.readFileSync(pkg, "utf8")).name === PI_PACKAGE) {
          return dir;
        }
      } catch {
        /* keep walking */
      }
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  // pi's managed installer: <agent>/bin/pi -> <agent>/install/releases/<v>/…
  const install = path.join(path.dirname(path.dirname(real)), "install");
  try {
    const version = fs
      .readFileSync(path.join(install, "current-version"), "utf8")
      .trim();
    const root = path.join(install, "releases", version, "node_modules", PI_PACKAGE);
    if (fs.existsSync(root)) return root;
  } catch {
    /* not a managed install */
  }
  throw new Error(`Could not locate the pi package from ${bin}`);
}

function findTargets(root) {
  const bundleDir = path.join(root, "dist", "bundle");
  const targets = [];
  const walk = (dir) => {
    if (!fs.existsSync(dir)) return;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.name.endsWith(".js")) {
        const text = fs.readFileSync(full, "utf8");
        // An already-patched file no longer has the `custom`/`get_state`
        // anchors (they were replaced), so the marker is the reliable signal
        // for revert/re-apply; the anchors identify a pristine file.
        const hasMarkers = text.includes(MARKER_BEGIN);
        const hasAnchors =
          text.includes(ANCHOR_HELPERS) &&
          text.includes(ANCHOR_CASES) &&
          text.includes(ANCHOR_CUSTOM) &&
          text.includes(ANCHOR_GET_STATE);
        if (hasMarkers || hasAnchors) {
          targets.push({ path: full, text });
        }
      }
    }
  };
  walk(bundleDir);
  return targets;
}

function handlerSource() {
  const source = fs
    .readFileSync(path.join(HERE, "custom-ui-handler.js"), "utf8")
    .trim();
  const create =
    "const orbitCustomUi=orbitCustomUiCreate({" +
    "output,pending:pendingExtensionRequests,crypto:crypto3," +
    'getKeybindings:(typeof getKeybindings==="function"?getKeybindings:undefined)});';
  return `${MARKER_BEGIN}\n${source}\n${create}\n${MARKER_END}\n`;
}

/** Re-apply: swap the injected block so edits to the handler land. */
function refreshBlock(text) {
  const begin = text.indexOf(MARKER_BEGIN);
  const end = text.indexOf(MARKER_END, begin);
  if (begin === -1 || end === -1) return null;
  return (
    text.slice(0, begin) +
    handlerSource() +
    text.slice(end + MARKER_END.length + 1)
  );
}

function patch(text) {
  return text
    .replace(ANCHOR_HELPERS, () => `${handlerSource()}${ANCHOR_HELPERS}`)
    .replace(ANCHOR_CASES, () => `${CUSTOM_CASES}${ANCHOR_CASES}`)
    .replace(ANCHOR_CUSTOM, () => CUSTOM_METHOD)
    .replace(ANCHOR_GET_STATE, () => GET_STATE_WITH_CAPABILITIES);
}

function revert(target) {
  const backup = `${target.path}.orbit-orig`;
  if (!fs.existsSync(backup)) {
    console.log(`No backup for ${target.path}; nothing to revert.`);
    return;
  }
  fs.copyFileSync(backup, target.path);
  console.log(`Reverted ${target.path} from backup.`);
}

function main() {
  const bin = resolvePiBin();
  if (!bin) {
    console.error("Could not find `pi`. Set PI_BIN or put pi on PATH.");
    process.exitCode = 1;
    return;
  }
  const root = packageRoot(bin);
  const targets = findTargets(root);
  if (targets.length === 0) {
    console.error(
      `No RPC-mode bundle found under ${root}. The pi layout may have changed.`,
    );
    process.exitCode = 1;
    return;
  }

  const reverting = process.argv.includes("--revert");
  let patchedAny = false;
  for (const target of targets) {
    if (reverting) {
      revert(target);
      continue;
    }
    if (target.text.includes(MARKER_BEGIN)) {
      const updated = refreshBlock(target.text);
      if (updated === null) {
        console.error(`Malformed patch markers in ${target.path}; not updating.`);
        process.exitCode = 1;
        continue;
      }
      fs.writeFileSync(target.path, updated);
      console.log(`Updated ${target.path}`);
      patchedAny = true;
      continue;
    }
    const missing = [ANCHOR_HELPERS, ANCHOR_CASES, ANCHOR_CUSTOM, ANCHOR_GET_STATE].filter(
      (anchor) => !target.text.includes(anchor),
    );
    if (missing.length > 0) {
      console.error(
        `Anchor(s) missing in ${target.path}: ${missing.join(", ")}; not patching.`,
      );
      process.exitCode = 1;
      continue;
    }
    const backup = `${target.path}.orbit-orig`;
    if (!fs.existsSync(backup)) {
      fs.copyFileSync(target.path, backup);
    }
    fs.writeFileSync(target.path, patch(target.text));
    console.log(`Patched ${target.path}`);
    patchedAny = true;
  }
  if (!reverting && patchedAny) {
    console.log("");
    console.log(
      "Restart pi (Settings → Runtime → Restart, or reopen Orbit) to load the patch.",
    );
  }
}

main();
