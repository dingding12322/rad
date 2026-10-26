#!/usr/bin/env node
/**
 * Apply the `auth.*` RPC capability to an installed pi build.
 *
 * pi's RPC server has no provider-auth commands, so Orbit would otherwise fall
 * back to launching `pi /login` in a terminal. This script injects five command
 * handlers (`auth.list`, `auth.status`, `auth.login`, `auth.logout`,
 * `auth.cancel`) into pi's bundled RPC mode. The handlers reuse pi's own
 * `session.modelRuntime` (ModelRuntime + auth.json) — Orbit never sees a token.
 *
 * The matching client contract is crates/orbit-rpc/docs/auth-rpc.md.
 *
 * Usage:
 *   node contrib/pi-auth-rpc/apply.mjs            # patch the resolved pi
 *   node contrib/pi-auth-rpc/apply.mjs --revert   # restore the backup
 *   PI_BIN=/path/to/pi node contrib/pi-auth-rpc/apply.mjs
 *
 * A `.orbit-orig` backup is written next to the patched file. The patch is
 * idempotent and is safe to re-run after a pi update (it will not apply twice
 * to the same file).
 *
 * This modifies a global npm install. It is intentionally a local,
 * reproducible stop-gap until pi ships auth.* upstream.
 */

import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const MARKER_BEGIN = "/*orbit-auth-rpc:begin*/";
const MARKER_END = "/*orbit-auth-rpc:end*/";

const ANCHOR_HELPERS = "let handleCommand=async command=>{";
const ANCHOR_CASES =
  "default:{let unknownCommand=command;return error(id,unknownCommand.type," +
  "`Unknown command: ${unknownCommand.type}`)}";

/**
 * Injected immediately before pi's command switch. It only depends on names
 * already in `runRpcMode`'s scope: `session`, `output`, `success`, `error`,
 * and `crypto3`.
 */
const HELPERS = `${MARKER_BEGIN}
const orbitAuthSessions=new Map();
const orbitAuthClassify=(err)=>{const message=err instanceof Error?err.message:String(err);const lower=message.toLowerCase();if(lower.includes("cancel"))return "cancelled";if(lower.includes("timed out")||lower.includes("timeout"))return "timeout";if(lower.includes("unknown provider"))return "invalid_provider";if(lower.includes("unsupported")||lower.includes("not available")||lower.includes("only available"))return "unsupported_method";if(lower.includes("auth.json")||lower.includes("persist")||lower.includes("storage")||lower.includes("locked"))return "storage_error";if(lower.includes("fetch")||lower.includes("network")||lower.includes("http request failed")||lower.includes("econn")||lower.includes("enotfound"))return "network_error";return "login_failed"};
const orbitAuthCredentialKinds=async()=>{const list=await session.modelRuntime.listCredentials();return new Map(list.map((info)=>[info.providerId,info.type]))};
const orbitAuthProviderPayload=async(provider,kinds)=>{const id=provider.id;const oauth=provider.auth&&provider.auth.oauth;const apiKey=provider.auth&&provider.auth.apiKey;const methods=[];if(oauth){const label=oauth.loginLabel||oauth.name||"Sign in";methods.push({id:"browser",label});methods.push({id:"device_code",label:"Use a device code"})}if(apiKey&&apiKey.login){methods.push({id:"api_key",label:apiKey.name||"API key"})}return{id,name:provider.name||id,credential:kinds.get(id)||"none",authenticated:session.modelRuntime.hasConfiguredAuth(id),methods}};
const orbitAuthHandle=async(command)=>{const id=command.id;
if(command.type==="auth.list"){const kinds=await orbitAuthCredentialKinds();const providers=[];for(const provider of session.modelRuntime.getProviders()){providers.push(await orbitAuthProviderPayload(provider,kinds))}return success(id,"auth.list",{providers})}
if(command.type==="auth.status"){const providerId=command.provider;const provider=session.modelRuntime.getProvider(providerId);if(!provider)return error(id,"auth.status","invalid_provider: unknown provider");const kinds=await orbitAuthCredentialKinds();return success(id,"auth.status",{id:providerId,authenticated:session.modelRuntime.hasConfiguredAuth(providerId),credential:kinds.get(providerId)||"none"})}
if(command.type==="auth.login"){const providerId=command.provider;const method=command.method||"oauth";const authType=method==="api_key"?"api_key":"oauth";const sessionId=command.sessionId||crypto3.randomUUID();const provider=session.modelRuntime.getProvider(providerId);if(!provider)return error(id,"auth.login","invalid_provider: unknown provider");if(authType==="oauth"&&!(provider.auth&&provider.auth.oauth))return error(id,"auth.login","unsupported_method: provider does not support OAuth");if(authType==="api_key"&&!(provider.auth&&provider.auth.apiKey&&provider.auth.apiKey.login))return error(id,"auth.login","unsupported_method: provider has no interactive API-key login");if(orbitAuthSessions.has(sessionId))return error(id,"auth.login","already_in_progress");const controller=new AbortController();orbitAuthSessions.set(sessionId,{controller,providerId});output({type:"auth_login_started",sessionId,provider:providerId,method,expiresAt:Date.now()+5*60*1000});const interaction={signal:controller.signal,notify(event){if(event.type==="auth_url"){output({type:"auth_login_url",sessionId,provider:providerId,url:event.url})}else if(event.type==="device_code"){const expiresAt=typeof event.expiresInSeconds==="number"?Date.now()+event.expiresInSeconds*1000:undefined;output({type:"auth_device_code",sessionId,provider:providerId,userCode:event.userCode,verificationUri:event.verificationUri,expiresAt})}else if(event.type==="progress"||event.type==="info"){output({type:"auth_login_waiting",sessionId,provider:providerId,message:event.message})}},prompt(prompt){if(prompt.type==="select"){const options=prompt.options||[];const preferred=method==="device_code"?"device_code":"browser";const chosen=options.find((option)=>option.id===preferred)||options[0];if(!chosen)return Promise.reject(new Error("Login cancelled: no options available"));return Promise.resolve(chosen.id)}if(prompt.type==="text"){return Promise.resolve("")}if(prompt.type==="secret"){return Promise.reject(new Error("unsupported_method: enter API keys in Orbit's provider editor"))}return new Promise((_resolve,reject)=>{const onAbort=()=>reject(new Error("Login cancelled"));if(controller.signal.aborted){onAbort();return}controller.signal.addEventListener("abort",onAbort,{once:true})})}};void (async()=>{try{await session.modelRuntime.login(providerId,authType,interaction);output({type:"auth_login_succeeded",sessionId,provider:providerId,method,credential:authType});output({type:"auth_credentials_changed",providers:[providerId]})}catch(loginError){if(controller.signal.aborted){output({type:"auth_login_cancelled",sessionId,provider:providerId})}else{output({type:"auth_login_failed",sessionId,provider:providerId,code:orbitAuthClassify(loginError),message:loginError instanceof Error?loginError.message:String(loginError)})}}finally{orbitAuthSessions.delete(sessionId)}})();return success(id,"auth.login",{sessionId,status:"pending"})}
if(command.type==="auth.cancel"){const entry=orbitAuthSessions.get(command.sessionId);if(entry)entry.controller.abort();return success(id,"auth.cancel")}
if(command.type==="auth.logout"){const providerId=command.provider;const provider=session.modelRuntime.getProvider(providerId);if(!provider)return error(id,"auth.logout","invalid_provider: unknown provider");for(const[sid,entry]of orbitAuthSessions){if(entry.providerId===providerId){entry.controller.abort();orbitAuthSessions.delete(sid)}}await session.modelRuntime.logout(providerId);output({type:"auth_credentials_changed",providers:[providerId]});return success(id,"auth.logout",{provider:providerId,status:"signed_out"})}
return undefined};
${MARKER_END}
`;

const CASES =
  'case"auth.list":case"auth.status":case"auth.login":case"auth.logout":' +
  'case"auth.cancel":return await orbitAuthHandle(command);';

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
  for (const target of targets) {
    if (reverting) {
      revert(target);
      continue;
    }
    if (target.text.includes(MARKER_BEGIN)) {
      console.log(`Already patched: ${target.path}`);
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
      .replace(ANCHOR_HELPERS, `${HELPERS}${ANCHOR_HELPERS}`)
      .replace(ANCHOR_CASES, `${CASES}${ANCHOR_CASES}`);
    fs.writeFileSync(target.path, patched);
    console.log(`Patched ${target.path}`);
  }
  if (!reverting) {
    console.log("");
    console.log("Restart pi (Settings → Runtime → Restart, or reopen Orbit) to load the patch.");
  }
}

main();
