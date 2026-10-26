#!/usr/bin/env node
/**
 * Apply the `quota.*` RPC capability to an installed pi build.
 *
 * pi's RPC server exposes only session-local `get_session_stats`; it has no
 * account-level provider quota. This script injects one command, `quota.list`,
 * into pi's bundled RPC mode. The handler resolves each connected provider's
 * credential through pi's own `session.modelRuntime` (never exposing it),
 * queries that provider's usage endpoint, and returns a normalized report.
 *
 * Adapter coverage mirrors `@narumitw/pi-usage`'s verified provider reference.
 * Providers with no usage surface report `unsupported`.
 *
 * The matching client contract is crates/orbit-rpc/docs/quota-rpc.md.
 *
 * Usage:
 *   node contrib/pi-quota-rpc/apply.mjs            # patch the resolved pi
 *   node contrib/pi-quota-rpc/apply.mjs --revert   # restore the backup
 *   PI_BIN=/path/to/pi node contrib/pi-quota-rpc/apply.mjs
 *
 * The patch is marker-delimited and idempotent, and writes a `.orbit-orig`
 * backup next to the patched file (shared with the auth patch).
 *
 * This modifies a global npm install. Re-run after `pi update`.
 */

import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MARKER_BEGIN = "/*orbit-quota-rpc:begin*/";
const MARKER_END = "/*orbit-quota-rpc:end*/";

const ANCHOR_HELPERS = "let handleCommand=async command=>{";
const ANCHOR_CASES =
  "default:{let unknownCommand=command;return error(id,unknownCommand.type," +
  "`Unknown command: ${unknownCommand.type}`)}";

const CASES = 'case"quota.list":return await orbitQuotaHandle(command);';

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
        if (text.includes(ANCHOR_HELPERS) && text.includes(ANCHOR_CASES)) {
          targets.push({ path: full, text });
        }
      }
    }
  };
  walk(bundleDir);
  return targets;
}

function handlerSource() {
  const source = fs.readFileSync(path.join(HERE, "quota-handler.js"), "utf8").trim();
  return `${MARKER_BEGIN}\n${source}\n${MARKER_END}\n`;
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
      // Reverting restores the pre-Orbit original, which drops the auth patch
      // too; that is the honest "unpatch" state and matches the backup.
      revert(target);
      continue;
    }
    if (target.text.includes(MARKER_BEGIN)) {
      // Re-apply: swap the injected block for the current handler source so
      // edits to quota-handler.js land without a revert.
      const begin = target.text.indexOf(MARKER_BEGIN);
      const end = target.text.indexOf(MARKER_END, begin);
      if (end === -1) {
        console.error(`Malformed patch markers in ${target.path}; not updating.`);
        process.exitCode = 1;
        continue;
      }
      const updated =
        target.text.slice(0, begin) +
        handlerSource() +
        target.text.slice(end + MARKER_END.length + 1);
      fs.writeFileSync(target.path, updated);
      console.log(`Updated ${target.path}`);
      patchedAny = true;
      continue;
    }
    if (!target.text.includes(ANCHOR_HELPERS) || !target.text.includes(ANCHOR_CASES)) {
      console.error(`Anchors not found in ${target.path}; not patching.`);
      process.exitCode = 1;
      continue;
    }
    const backup = `${target.path}.orbit-orig`;
    if (!fs.existsSync(backup)) {
      fs.copyFileSync(target.path, backup);
    }
    const patched = target.text
      .replace(ANCHOR_HELPERS, `${handlerSource()}${ANCHOR_HELPERS}`)
      .replace(ANCHOR_CASES, `${CASES}${ANCHOR_CASES}`);
    fs.writeFileSync(target.path, patched);
    console.log(`Patched ${target.path}`);
    patchedAny = true;
  }
  if (!reverting && patchedAny) {
    console.log("");
    console.log("Restart pi (Settings → Runtime → Restart, or reopen Orbit) to load the patch.");
  }
}

main();
