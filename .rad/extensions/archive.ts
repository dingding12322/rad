/**
 * archive — 会话自动归档 + OpenViking 提炼（rad 扩展）
 *
 * 生命周期（先提炼，后归档）：
 *   1. 会话内：记录工件（write/edit 目标、bash 产生的验证脚本/截图）
 *   2. session_shutdown：提炼先行——transcript 推给 OV（messages/batch + commit，
 *      服务端提炼）；commit 成功才移动工件并写 manifest
 *   3. scan（session_start / node 直跑 --scan）：
 *      - 会话文件闲置 >48h 且未归档 → 同上流程（覆盖未正常结束的会话）
 *      - manifest.endedAt >30d 且 ovCommitted → 删除归档目录（OV 记忆保留）
 *
 * 双形态：
 *   - rad 作为扩展加载：注册事件钩子
 *   - `node archive.ts --scan`：直接执行扫描（Node 24 原生跑 TS）
 *
 * 映射（manifest.json）是误归档恢复的唯一事实来源；30 天后提炼已成功，
 * 可恢复性让位给"已提炼"，归档目录整体退役。
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

/** 会话闲置多久后自动归档。 */
const ARCHIVE_AFTER_MS = 48 * 60 * 60 * 1000;
/** 归档保留多久后退役（前提：提炼已成功）。 */
const RETIRE_AFTER_MS = 30 * 24 * 60 * 60 * 1000;
/** 单条消息推给 OV 的最大字符数。 */
const MAX_MESSAGE_CHARS = 4000;
/** 单会话最多推送的消息条数。 */
const MAX_MESSAGES = 200;
/** 工件后缀（截图快照）。 */
const SNAPSHOT_RE = /\.(png|jpe?g|webp)$/i;
/** 验证脚本（/tmp 下或 verify/check/probe 前缀）。 */
const SCRIPT_RE = /(?:^|[\s"'])(\/tmp\/[\w./-]+\.(?:mjs|js|ts|sh|py))|(?:^|[\s"'])((?:verify|check|probe)[\w.-]*\.(?:mjs|js|ts|sh|py))/gi;
/** OV 会话 id 前缀：区分 rad 来源。 */
const OV_SESSION_PREFIX = "oc-rad-";

interface OvConf {
	apiKey: string;
	account: string;
	user: string;
}

interface Manifest {
	session: string;
	ovSession: string;
	title: string;
	cwd: string;
	endedAt: number;
	ovCommitted: boolean;
	files: { from: string; to: string }[];
}

function agentDir(): string {
	return process.env.RAD_AGENT_DIR || join(homeDir(), ".rad", "agent");
}

function homeDir(): string {
	return process.env.HOME || process.env.USERPROFILE || "";
}

function sessionsDir(): string {
	return join(agentDir(), "sessions");
}

function archiveRoot(): string {
	return join(agentDir(), "archive");
}

function readOvConf(): OvConf | null {
	try {
		const raw = readFileSync(join(homeDir(), ".openviking", "ovcli.conf"), "utf8");
		const value = JSON.parse(raw) as Record<string, unknown>;
		const apiKey = typeof value.api_key === "string" ? value.api_key : "";
		if (!apiKey) return null;
		return {
			apiKey,
			account: typeof value.account === "string" ? value.account : "",
			user: typeof value.user === "string" ? value.user : "",
		};
	} catch {
		return null;
	}
}

function ovHeaders(conf: OvConf): Record<string, string> {
	const headers: Record<string, string> = { "X-API-Key": conf.apiKey };
	if (conf.account) headers["X-OpenViking-Account"] = conf.account;
	if (conf.user) headers["X-OpenViking-User"] = conf.user;
	return headers;
}

async function ovPost(conf: OvConf, path: string, body: unknown): Promise<boolean> {
	try {
		const response = await fetch(`http://127.0.0.1:1933${path}`, {
			method: "POST",
			headers: { "Content-Type": "application/json", ...ovHeaders(conf) },
			body: JSON.stringify(body),
			signal: AbortSignal.timeout(30_000),
		});
		if (!response.ok) {
			console.error(`[archive] OV ${path} -> ${response.status}: ${(await response.text()).slice(0, 200)}`);
		}
		return response.ok;
	} catch (error) {
		console.error(`[archive] OV ${path} failed:`, error instanceof Error ? error.message : error);
		return false;
	}
}

/** 会话文件名 `${timestamp}_${sessionId}.jsonl` → sessionId（时间戳不含下划线）。 */
function sessionIdOf(file: string): string {
	return basename(file, ".jsonl").replace(/^[^_]*_/, "");
}

interface TranscriptMessage {
	role: string;
	text: string;
}

/** 解析 pi 会话 JSONL：type:"message" 条目 → role + 纯文本（截断）。 */
function readTranscript(file: string): TranscriptMessage[] {
	const messages: TranscriptMessage[] = [];
	try {
		const lines = readFileSync(file, "utf8").split("\n").filter(Boolean);
		for (const line of lines) {
			if (messages.length >= MAX_MESSAGES) break;
			let entry: Record<string, unknown>;
			try {
				entry = JSON.parse(line) as Record<string, unknown>;
			} catch {
				continue;
			}
			if (entry.type !== "message") continue;
			const message = entry.message as Record<string, unknown> | undefined;
			if (!message) continue;
			const role = String(message.role ?? "unknown");
			if (role !== "user" && role !== "assistant") continue;
			const text = messageText(message.content);
			if (!text.trim()) continue;
			messages.push({ role, text: text.slice(0, MAX_MESSAGE_CHARS) });
		}
	} catch {
		// 读不了的会话按空处理（batch 空列表会被跳过）。
	}
	return messages;
}

function messageText(content: unknown): string {
	if (typeof content === "string") return content;
	if (Array.isArray(content)) {
		return content
		.map((block) => {
			const typed = block as Record<string, unknown>;
			return typeof typed.text === "string" ? typed.text : "";
		})
		.join("\n");
	}
	return "";
}

/** OV 提炼：批量消息 + commit。任一步失败返回 false（不归档，等重试）。 */
async function ovCommit(conf: OvConf, ovSessionId: string, messages: TranscriptMessage[], title: string): Promise<boolean> {
	const payload = messages.length
		? messages
		: [{ role: "user", text: title || "(empty session)" }];
	const batchBody = {
		messages: payload.map((message) => ({ role: message.role, content: message.text })),
	};
	const batched = await ovPost(conf, `/api/v1/sessions/${ovSessionId}/messages/batch`, batchBody);
	if (!batched) return false;
	return ovPost(conf, `/api/v1/sessions/${ovSessionId}/commit`, { keep_recent_count: 0 });
}

function manifestPath(ovSessionId: string): string {
	return join(archiveRoot(), ovSessionId, "manifest.json");
}

function loadManifests(): Map<string, Manifest> {
	const manifests = new Map<string, Manifest>();
	try {
		for (const dir of readdirSync(archiveRoot())) {
			try {
				const raw = readFileSync(join(archiveRoot(), dir, "manifest.json"), "utf8");
				const manifest = JSON.parse(raw) as Manifest;
				manifests.set(manifest.session, manifest);
			} catch {
				continue;
			}
		}
	} catch {
		// 无归档目录。
	}
	return manifests;
}

/** 把仍存在的工件移入归档目录；返回移动映射。 */
function moveArtifacts(ovSessionId: string, artifacts: Set<string>): { from: string; to: string }[] {
	const moved: { from: string; to: string }[] = [];
	const filesDir = join(archiveRoot(), ovSessionId, "files");
	for (const from of artifacts) {
		try {
			if (!existsSync(from) || !statSync(from).isFile()) continue;
			mkdirSync(filesDir, { recursive: true });
			const to = join(filesDir, basename(from));
			copyFileSync(from, to);
			rmSync(from);
			moved.push({ from, to });
		} catch {
			continue;
		}
	}
	return moved;
}

/** 先提炼后归档：OV commit 成功才移动工件并落盘 manifest。 */
async function archiveSession(sessionFile: string, artifacts: Set<string>): Promise<boolean> {
	const conf = readOvConf();
	if (!conf) return false;
	const sessionId = sessionIdOf(sessionFile);
	const ovSessionId = `${OV_SESSION_PREFIX}${sessionId}`;
	const stat = statSync(sessionFile);
	const title = basename(sessionFile, ".jsonl");
	const committed = await ovCommit(conf, ovSessionId, readTranscript(sessionFile), title);
	if (!committed) return false;
	const moved = moveArtifacts(ovSessionId, artifacts);
	const manifest: Manifest = {
		session: sessionId,
		ovSession: ovSessionId,
		title,
		cwd: cwdFromSessionDir(sessionFile),
		endedAt: stat.mtimeMs,
		ovCommitted: true,
		files: moved,
	};
	mkdirSync(join(archiveRoot(), ovSessionId), { recursive: true });
	writeFileSync(manifestPath(ovSessionId), JSON.stringify(manifest, null, 1));
	return true;
}

/** 会话目录名（`--<cwd slug>--`，`/` 有损映射为 `-`）——仅作参考，不可逆。 */
function cwdFromSessionDir(sessionFile: string): string {
	return basename(join(sessionFile, ".."));
}

/** 扫描：48h 未归档会话走提炼+归档；30d 前的已提炼归档退役。 */
async function scan(): Promise<{ archived: number; retired: number }> {
	let archived = 0;
	let retired = 0;
	const manifests = loadManifests();
	const now = Date.now();
	try {
		for (const dir of readdirSync(sessionsDir())) {
			const dirPath = join(sessionsDir(), dir);
			let files: string[];
			try {
				files = readdirSync(dirPath).filter((name) => name.endsWith(".jsonl"));
			} catch {
				continue;
			}
			for (const name of files) {
				const file = join(dirPath, name);
				const sessionId = sessionIdOf(file);
				const known = manifests.get(sessionId);
				// 已提炼成功的归档：到期退役。
				if (known?.ovCommitted && now - known.endedAt > RETIRE_AFTER_MS) {
					try {
						rmSync(join(archiveRoot(), known.ovSession), { recursive: true, force: true });
						retired += 1;
					} catch {
						continue;
					}
					continue;
				}
				if (known) continue;
				// 未归档且闲置超 48h：提炼 + 归档（无已知工件，仅 transcript）。
				const idle = now - statSync(file).mtimeMs;
				if (idle > ARCHIVE_AFTER_MS) {
					if (await archiveSession(file, new Set())) archived += 1;
				}
			}
		}
	} catch {
		// sessions 目录缺失：无事可做。
	}
	return { archived, retired };
}

/** 从工具调用里收集工件路径。 */
function collectArtifacts(toolName: string, input: Record<string, unknown>, prior: Set<string>): void {
	const push = (value: unknown) => {
		if (typeof value === "string" && value && existsSync(value)) prior.add(value);
	};
	if (toolName === "write" || toolName === "edit") {
		push(input.path);
		push(input.file_path);
		return;
	}
	if (toolName === "bash" && typeof input.command === "string") {
		for (const match of input.command.matchAll(SCRIPT_RE)) {
			push(match[1]);
			push(match[2]);
		}
		for (const match of input.command.matchAll(/[\w./-]+\.(?:png|jpe?g|webp)/gi)) {
			const candidate = match[0];
			push(candidate.startsWith("/") ? candidate : join(process.cwd(), candidate));
		}
	}
}

/** 是否被 node 直接执行（CLI 形态）。 */
function isMainModule(): boolean {
	const entry = process.argv[1];
	if (!entry) return false;
	try {
		return pathToFileURL(entry).href === import.meta.url;
	} catch {
		return false;
	}
}

async function cli(): Promise<void> {
	if (process.argv.includes("--scan")) {
		const result = await scan();
		console.log(`archive scan: ${result.archived} archived, ${result.retired} retired`);
		return;
	}
	console.log("usage: node archive.ts --scan");
}

// CLI 形态：node archive.ts --scan（不用 top-level await，兼容无 package.json
// 的目录被 Node 按启发式选模块系统）。
if (isMainModule()) {
	void cli();
}

/**
 * 扩展形态入口：rad/jiti 加载本文件时注册钩子。
 * 工件只在 tool_call 收集（write/edit/bash 的 input 均在调用侧）。
 */
export default function archive(pi: ExtensionAPI): void {
	const artifacts = new Set<string>();
	pi.on("tool_call", (event) => {
		collectArtifacts(event.toolName, (event.input ?? {}) as Record<string, unknown>, artifacts);
	});
	pi.on("session_shutdown", () => {
		const sessionFile = currentSessionFile();
		if (sessionFile) void archiveSession(sessionFile, artifacts);
		artifacts.clear();
	});
	pi.on("session_start", () => {
		void scan();
	});
}

/** 当前会话文件：shutdown 时 pi 仍未关闭它，按 mtime 最新的 jsonl 近似。 */
function currentSessionFile(): string | null {
	let newest: string | null = null;
	let newestMs = 0;
	try {
		for (const dir of readdirSync(sessionsDir())) {
			const dirPath = join(sessionsDir(), dir);
			for (const name of readdirSync(dirPath)) {
				if (!name.endsWith(".jsonl")) continue;
				const file = join(dirPath, name);
				const ms = statSync(file).mtimeMs;
				if (ms > newestMs) {
					newestMs = ms;
					newest = file;
				}
			}
		}
	} catch {
		return null;
	}
	return newest;
}
