/**
 * ov — OpenViking 全职责扩展（rad）：记忆召回 + 守卫 + 增量保全 + 归档提炼
 *
 * 写路径（记忆沉淀）：
 *   - turn 级：会话消息在压缩前与结束时全量推给 OV（messages/batch + commit，
 *     服务端提炼）；`distilled` 标志保证同一会话不重复提炼
 *   - 压缩前（session_before_compact）必须先提炼——压缩会截断 transcript
 *   - 结束（session_shutdown）：未提炼则提炼；然后工件归档 + manifest
 *   - scan（session_start / node ov.ts --scan）：闲置 >48h 的会话补跑同一
 *     流程；归档 >30d 且已提炼的退役（删除归档目录，OV 记忆保留）
 *
 * 读路径（记忆召回）：
 *   - session-start 块：用户偏好记忆索引（preferences 目录清单）+ 相关记忆
 *     检索，每个会话注入一次
 *   - 每轮 recall：最新用户消息语义检索 OV，注入 `<openviking-recall>` 块
 *     （同一 query 不重复注入）
 *   - repo 段：已索引资源清单 + viking:// 工具指引，附加到系统提示
 *
 * 守卫：
 *   - 非 OV 通道（read/glob/bash 等）访问 `viking://` URI 时拦截并提示改用
 *     openviking MCP 工具
 *
 * 双形态：rad 扩展（jiti 加载）与 `node ov.ts --scan`（Node 24 原生跑 TS）。
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
/** 验证脚本（/tmp 下或 verify/check/probe 前缀）。 */
const SCRIPT_RE = /(?:^|[\s"'])(\/tmp\/[\w./-]+\.(?:mjs|js|ts|sh|py))|(?:^|[\s"'])((?:verify|check|probe)[\w.-]*\.(?:mjs|js|ts|sh|py))/gi;
/** OV 会话 id 前缀：区分 rad 来源。 */
const OV_SESSION_PREFIX = "oc-rad-";
/** OV 本机服务地址。 */
const OV_ORIGIN = "http://127.0.0.1:1933";
/** repo 段与召回块的缓存时长。 */
const REPO_CACHE_MS = 60_000;

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

function homeDir(): string {
	return process.env.HOME || process.env.USERPROFILE || "";
}

function agentDir(): string {
	return process.env.RAD_AGENT_DIR || join(homeDir(), ".rad", "agent");
}

function sessionsDir(): string {
	return join(agentDir(), "sessions");
}

function archiveRoot(): string {
	return join(agentDir(), "archive");
}

function readOvConf(): OvConf | null {
	try {
		const value = JSON.parse(readFileSync(join(homeDir(), ".openviking", "ovcli.conf"), "utf8")) as Record<string, unknown>;
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
		const response = await fetch(`${OV_ORIGIN}${path}`, {
			method: "POST",
			headers: { "Content-Type": "application/json", ...ovHeaders(conf) },
			body: JSON.stringify(body),
			signal: AbortSignal.timeout(30_000),
		});
		if (!response.ok) {
			console.error(`[ov] ${path} -> ${response.status}: ${(await response.text()).slice(0, 200)}`);
		}
		return response.ok;
	} catch (error) {
		console.error(`[ov] ${path} failed:`, error instanceof Error ? error.message : error);
		return false;
	}
}

async function ovGetJson(conf: OvConf, path: string): Promise<Record<string, unknown> | null> {
	try {
		const response = await fetch(`${OV_ORIGIN}${path}`, {
			headers: ovHeaders(conf),
			signal: AbortSignal.timeout(15_000),
		});
		if (!response.ok) return null;
		return (await response.json()) as Record<string, unknown>;
	} catch {
		return null;
	}
}

/** 会话文件名 `${timestamp}_${sessionId}.jsonl` → sessionId（时间戳无下划线）。 */
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
		for (const line of readFileSync(file, "utf8").split("\n").filter(Boolean)) {
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
		// 读不了的会话按空处理。
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
	const batched = await ovPost(conf, `/api/v1/sessions/${ovSessionId}/messages/batch`, {
		messages: payload.map((message) => ({ role: message.role, content: message.text })),
	});
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
				const manifest = JSON.parse(readFileSync(join(archiveRoot(), dir, "manifest.json"), "utf8")) as Manifest;
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

/** 会话目录名（`--<cwd slug>--`，`/` 有损映射为 `-`）——仅作参考，不可逆。 */
function cwdFromSessionDir(sessionFile: string): string {
	return basename(join(sessionFile, ".."));
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

// ── 读路径：召回注入 ─────────────────────────────────────────────────

/** OV 检索：语义召回 top 条目（uri + 截断文本）。 */
async function ovRecall(conf: OvConf, query: string, limit: number): Promise<{ uri: string; text: string }[]> {
	try {
		const response = await fetch(`${OV_ORIGIN}/api/v1/search/search`, {
			method: "POST",
			headers: { "Content-Type": "application/json", ...ovHeaders(conf) },
			body: JSON.stringify({ mode: "context", query }),
			signal: AbortSignal.timeout(15_000),
		});
		if (!response.ok) return [];
		const data = (await response.json()) as { result?: { entries?: { uri: string; text?: string }[] } };
		return (data.result?.entries ?? []).slice(0, limit).map((entry) => ({
			uri: entry.uri,
			text: (entry.text ?? "").slice(0, 500),
		}));
	} catch {
		return [];
	}
}

/** viking://resources/ 下的已索引资源名（60s 缓存）。 */
let repoCache: { at: number; names: string[] } | null = null;
async function ovResourceNames(conf: OvConf): Promise<string[]> {
	if (repoCache && Date.now() - repoCache.at < REPO_CACHE_MS) return repoCache.names;
	const data = await ovGetJson(conf, `/api/v1/fs/ls?uri=${encodeURIComponent("viking://resources/")}&recursive=false&simple=false`);
	const items = (data?.result as { uri?: string }[] | undefined) ?? [];
	const names = items
		.map((item) => item.uri ?? "")
		.filter((uri) => uri.startsWith("viking://resources/") && uri !== "viking://resources/")
		.map((uri) => uri.replace("viking://resources/", "").replace(/\/$/, ""));
	repoCache = { at: Date.now(), names };
	return names;
}

/** 用户偏好记忆索引（preferences 目录清单，v1 只列路径不做摘要）。 */
async function ovPreferenceIndex(conf: OvConf): Promise<string[]> {
	const user = conf.user || "default";
	const data = await ovGetJson(
		conf,
		`/api/v1/fs/ls?uri=${encodeURIComponent(`viking://user/default/memories/preferences/${user}`)}&output=agent&recursive=true&abs_limit=512&node_limit=512`,
	);
	const items = (data?.result as { rel_path?: string }[] | undefined) ?? [];
	return items.map((item) => item.rel_path ?? "").filter(Boolean);
}

function renderRecallBlock(entries: { uri: string; text: string }[]): string {
	if (!entries.length) return "";
	const body = entries
		.map((entry) => `- ${entry.uri}\n  ${entry.text.replace(/\n+/g, " ")}`)
		.join("\n");
	return `<openviking-recall>\n${body}\n</openviking-recall>\n用 openviking MCP 工具（search/read）按需展开上述记忆。`;
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
		console.log(`ov scan: ${result.archived} archived, ${result.retired} retired`);
		return;
	}
	console.log("usage: node ov.ts --scan");
}

// CLI 形态：node ov.ts --scan（不用 top-level await，兼容无 package.json
// 的目录被 Node 按启发式选模块系统）。
if (isMainModule()) {
	void cli();
}

/**
 * 扩展形态入口：rad/jiti 加载本文件时注册钩子。
 */
export default function ov(pi: ExtensionAPI): void {
	const artifacts = new Set<string>();
	/** 本会话是否已完成 OV 提炼（压缩前或结束时，二者取一）。 */
	let distilled = false;
	/** session-start 块只注入一次。 */
	let injectedSessionStart = false;
	/** 上次召回用的 query：同一 query 不重复注入。 */
	let lastRecallQuery = "";

	pi.on("tool_call", (event) => {
		const input = (event.input ?? {}) as Record<string, unknown>;
		collectArtifacts(event.toolName, input, artifacts);
		// viking:// 守卫：OV 的数据只走 OV 工具，普通文件工具够不到它。
		if (event.toolName.startsWith("mcp__openviking")) return;
		const hits = JSON.stringify(input).includes("viking://");
		if (hits) {
			return {
				block: true,
				reason: "[ov] `viking://` 是 OpenViking 的 URI，不能当本地文件读。请改用 openviking MCP 工具（openviking_search / openviking_read / openviking_list）。",
			};
		}
	});

	pi.on("session_before_compact", async () => {
		// 压缩会截断 transcript：先提炼全量，成功后标记，结束时不再重复。
		const conf = readOvConf();
		const sessionFile = newestSessionFile();
		if (!conf || !sessionFile) return;
		const sessionId = sessionIdOf(sessionFile);
		if (await ovCommit(conf, `${OV_SESSION_PREFIX}${sessionId}`, readTranscript(sessionFile), basename(sessionFile, ".jsonl"))) {
			distilled = true;
		}
	});

	pi.on("session_shutdown", () => {
		const sessionFile = newestSessionFile();
		if (sessionFile) {
			void (async () => {
				const conf = readOvConf();
				if (!conf) return;
				const sessionId = sessionIdOf(sessionFile);
				if (!distilled) {
					distilled = await ovCommit(conf, `${OV_SESSION_PREFIX}${sessionId}`, readTranscript(sessionFile), basename(sessionFile, ".jsonl"));
				}
				if (distilled) await archiveSessionFiles(sessionId, sessionFile, artifacts);
			})();
		}
		artifacts.clear();
	});

	pi.on("session_start", () => {
		void scan();
	});

	// 每轮 LLM 调用前：session-start 块（一次）+ 最新用户消息的 recall。
	pi.on("context", async (event) => {
		const messages = event.messages;
		const conf = readOvConf();
		const additions: { role: "user"; content: string }[] = [];
		if (conf) {
			if (!injectedSessionStart) {
				injectedSessionStart = true;
				const [index, entries] = await Promise.all([
					ovPreferenceIndex(conf),
					ovRecall(conf, `工作区 ${basename(process.cwd())} 的偏好与工作流`, 5),
				]);
				if (index.length || entries.length) {
					const indexLines = index.slice(0, 40).map((path) => `- memories/preferences/${path}`).join("\n");
					const recallBody = entries
						.map((entry) => `- ${entry.uri}\n  ${entry.text.replace(/\n+/g, " ")}`)
						.join("\n");
					additions.push({
						role: "user",
						content: `<openviking-context source="session-start">\n用户记忆索引（openviking_read 可读全文）:\n${indexLines}\n\n相关记忆:\n${recallBody}\n</openviking-context>`,
					});
				}
			}
			const lastUser = [...messages].reverse().find((message) => message.role === "user");
			const query = lastUser ? messageText(lastUser.content).slice(0, 500) : "";
			if (query && query !== lastRecallQuery) {
				lastRecallQuery = query;
				const entries = await ovRecall(conf, query, 5);
				const block = renderRecallBlock(entries);
				if (block) additions.push({ role: "user", content: block });
			}
		}
		if (!additions.length) return;
		const injected = additions.map((addition) => ({ role: "user", content: addition.content })) as unknown as typeof messages;
		return { messages: [...injected, ...messages] };
	});

	// 系统提示：附加已索引资源清单 + viking:// 指引。
	pi.on("context_with_system", async (event) => {
		const conf = readOvConf();
		if (!conf) return;
		const names = await ovResourceNames(conf);
		if (!names.length) return;
		const section = `\n\n# OpenViking\n个人知识库（OpenViking）已索引以下资源：${names.join("、")}。用 viking://<uri> 标识它们，并通过 openviking MCP 工具（search/read/list）访问；不要用本地文件工具读 viking:// URI。`;
		const messages = event.messages.map((message) => {
			if (message.role !== "system") return message;
			const content = typeof message.content === "string" ? message.content + section : message.content;
			return { ...message, content };
		});
		return { messages: messages as unknown as typeof event.messages };
	});
}

/** shutdown 归档：工件移动 + manifest（提炼已由调用方确认）。 */
async function archiveSessionFiles(sessionId: string, sessionFile: string, artifacts: Set<string>): Promise<void> {
	const ovSessionId = `${OV_SESSION_PREFIX}${sessionId}`;
	const moved = moveArtifacts(ovSessionId, artifacts);
	const manifest: Manifest = {
		session: sessionId,
		ovSession: ovSessionId,
		title: basename(sessionFile, ".jsonl"),
		cwd: cwdFromSessionDir(sessionFile),
		endedAt: statSync(sessionFile).mtimeMs,
		ovCommitted: true,
		files: moved,
	};
	mkdirSync(join(archiveRoot(), ovSessionId), { recursive: true });
	writeFileSync(manifestPath(ovSessionId), JSON.stringify(manifest, null, 1));
}

/** 最新修改的会话文件（shutdown 时当前会话仍持有最新 mtime）。 */
function newestSessionFile(): string | null {
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
