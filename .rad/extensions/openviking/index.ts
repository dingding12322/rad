/**
 * OpenViking 记忆扩展（rad 版，完整对齐 OpenCode 插件 ~/.config/opencode/plugins/openviking.js）
 *
 * 复用 @openviking/opencode-plugin 的 lib 模块。升级 npm 包
 * (`npm i -g @openviking/opencode-plugin`) 后需同步 PKG_ROOT 的 node 版本目录
 * （或用 OPENVIKING_PKG_ROOT 环境变量覆盖）。
 *
 * 与 OpenCode 版的差异：
 * - 事件适配：rad(pi) 的 session_start / message_end / session_before_compact / agent_settled /
 *   session_shutdown 映射为 lib 期望的 OpenCode 事件形状（session.created / message.updated /
 *   message.part.updated / session.compacted / session.idle）。
 * - 召回与会话注入通过 input 事件的 transform 前置到用户文本，等价于 OpenCode 的 prompt.text 改写。
 * - 数据目录隔离到 ~/.rad/openviking，避免与 OpenCode 插件并发写同一份会话状态文件。
 * - viking:// URI 守卫通过 tool_call 返回 { block } 实现，提示通过 tool_result 追加文本。
 * - lib 路径运行时推导（homedir），公开仓库不硬编码用户目录；扩展工厂为 async，jiti/pi 原生支持。
 */
import { homedir } from "os";
import path from "path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const PKG_ROOT =
	process.env.OPENVIKING_PKG_ROOT ??
	path.join(homedir(), ".nvm", "versions", "node", "v18.20.8", "lib", "node_modules", "@openviking", "opencode-plugin");
const DATA_DIR = path.join(homedir(), ".rad", "openviking");

interface TextPart {
	type: string;
	text?: string;
	synthetic?: boolean;
}

interface VikingRuntime {
	enabled: boolean;
	sessionManager: any;
	recall: { injectRelevantMemories(input: any, output: { parts: TextPart[] }): Promise<void> };
	sessionInject: { injectSessionContext(input: any, output: { parts: TextPart[] }): Promise<boolean> };
	repoContext: { refreshRepos(options?: { force?: boolean }): Promise<string | null>; getRepoSystemPrompt(): string | null };
	uriGuard: (input: { tool?: string; args?: any }, output: { args?: any }) => Promise<void>;
	uriNotice: (input: { tool?: string; args?: any }, output?: { output?: string }) => Promise<void>;
}

function textOfContent(content: Array<{ type: string; text?: string }>): string {
	return content
		.filter((part) => part.type === "text" && typeof part.text === "string")
		.map((part) => part.text)
		.join("\n");
}

export default async function (pi: ExtensionAPI) {
	// lib 模块在工厂加载期解析一次；动态导入是为了让 PKG_ROOT 可由环境推导（公开仓库不硬编码用户路径）
	const { loadConfig, resolveDataDir } = await import(path.join(PKG_ROOT, "lib", "config.mjs"));
	const { initLogger, log } = await import(path.join(PKG_ROOT, "lib", "utils.mjs"));
	const { initializeRuntime } = await import(path.join(PKG_ROOT, "lib", "runtime.mjs"));
	const { createRepoContext } = await import(path.join(PKG_ROOT, "lib", "repo-context.mjs"));
	const { createMemorySessionManager } = await import(path.join(PKG_ROOT, "lib", "memory-session.mjs"));
	const { createMemoryRecall } = await import(path.join(PKG_ROOT, "lib", "memory-recall.mjs"));
	const { createSessionInject } = await import(path.join(PKG_ROOT, "lib", "session-inject.mjs"));
	const { createVikingUriGuard, createVikingUriNotice } = await import(path.join(PKG_ROOT, "lib", "viking-uri-guard.mjs"));

	let runtime: VikingRuntime | undefined;
	let messageSeq = 0;

	function createRuntime(directory: string): VikingRuntime {
		const config = loadConfig(PKG_ROOT, directory);
		config.runtime.dataDir = DATA_DIR;
		const dataDir = resolveDataDir(PKG_ROOT, config);
		initLogger(dataDir);
		if (!config.enabled) {
			log("INFO", "plugin", "OpenViking plugin is disabled in configuration");
			return {
				enabled: false,
				sessionManager: undefined as any,
				recall: undefined as any,
				sessionInject: undefined as any,
				repoContext: undefined as any,
				uriGuard: undefined as any,
				uriNotice: undefined as any,
			};
		}
		const sessionManager = createMemorySessionManager({ config, pluginRoot: dataDir });
		const created: VikingRuntime = {
			enabled: true,
			sessionManager,
			recall: createMemoryRecall({ config, sessionManager }),
			sessionInject: createSessionInject({ config, sessionManager }),
			repoContext: createRepoContext({ config }),
			uriGuard: createVikingUriGuard(),
			uriNotice: createVikingUriNotice(),
		};
		// 后台健康检查 + 索引仓库刷新，失败不阻塞会话启动
		void initializeRuntime(config)
			.then((healthy: boolean) => (healthy ? created.repoContext.refreshRepos({ force: true }) : null))
			.catch((error: any) => log("WARN", "runtime", "OpenViking background init failed", { error: error?.message ?? String(error) }));
		return created;
	}

	// 与 opencode.json 中 openviking MCP 配置对齐；direct 让 15 个 viking 工具直接暴露给模型
	pi.registerMcpServer("openviking", {
		command: "node",
		args: [path.join(PKG_ROOT, "servers", "mcp-proxy.mjs")],
		timeout: 15,
		exposure: "direct",
		description: "OpenViking memory: semantic search and read/write of viking:// resources",
	});

	pi.on("session_start", async (_event, ctx) => {
		if (!runtime) runtime = createRuntime(ctx.cwd);
		if (!runtime.enabled) return;
		await runtime.sessionManager.init();
		await runtime.sessionManager.handleEvent({
			type: "session.created",
			properties: { info: { id: ctx.sessionManager.getSessionId() } },
		});
	});

	// repo 上下文走 prompt section，不替换整段 system prompt
	pi.on("before_agent_start", (event) => {
		const prompt = runtime?.enabled ? runtime.repoContext.getRepoSystemPrompt() : null;
		if (prompt) event.systemPromptOptions.sections.openviking_repos = prompt;
	});

	// 会话档案注入（每会话一次）+ 记忆召回（每次输入），等价于 OpenCode prompt hook
	pi.on("input", async (event, ctx) => {
		if (!runtime?.enabled) return;
		const sessionId = ctx.sessionManager.getSessionId();
		const seed: TextPart = { type: "text", text: event.text };
		const parts: TextPart[] = [seed];
		const messageID = `pi-${Date.now()}-${messageSeq++}`;
		const input = { sessionID: sessionId, messageID, directory: ctx.cwd };
		const output = { message: { sessionID: sessionId, id: messageID }, parts };
		try {
			await runtime.sessionInject.injectSessionContext(input, output);
			await runtime.recall.injectRelevantMemories(input, output);
		} catch (error) {
			log("WARN", "recall", "Auto recall failed", { error: error instanceof Error ? error.message : String(error) });
			return;
		}
		const injected = parts.filter((part) => part !== seed && part?.type === "text" && typeof part.text === "string");
		if (injected.length > 0) {
			return { action: "transform", text: `${injected.map((part) => part.text).join("\n")}\n\n${event.text}` };
		}
	});

	// 会话捕获：user/assistant 消息结束时报给 lib，由它在压缩/空闲/关闭时统一提交
	pi.on("message_end", async (event, ctx) => {
		const message = event.message;
		if ((message.role !== "user" && message.role !== "assistant") || !runtime?.enabled) return;
		const sessionId = ctx.sessionManager.getSessionId();
		const messageID = `pi-${message.role}-${message.timestamp}-${messageSeq++}`;
		const text =
			message.role === "user"
				? typeof message.content === "string"
					? message.content
					: textOfContent(message.content)
				: textOfContent(message.content);
		if (!text.trim()) return;
		await runtime.sessionManager.handleEvent({
			type: "message.updated",
			properties: { info: { sessionID: sessionId, id: messageID, role: message.role } },
		});
		await runtime.sessionManager.handleEvent({
			type: "message.part.updated",
			properties: { part: { sessionID: sessionId, messageID, id: `${messageID}:text`, type: "text", text } },
		});
	});

	// 压缩前提交边界：先落库再丢上下文，与 OpenCode compaction hook 时机一致
	pi.on("session_before_compact", async (_event, ctx) => {
		if (!runtime?.enabled) return;
		await runtime.sessionManager.flushSession(ctx.sessionManager.getSessionId(), { commit: true, reason: "session_before_compact" });
	});

	// 每轮结束后冲刷未提交消息，与 OpenCode session.idle 一致
	pi.on("agent_settled", async (_event, ctx) => {
		if (!runtime?.enabled) return;
		await runtime.sessionManager.handleEvent({
			type: "session.idle",
			properties: { info: { id: ctx.sessionManager.getSessionId() } },
		});
	});

	pi.on("session_shutdown", async () => {
		const current = runtime;
		runtime = undefined;
		if (!current?.enabled) return;
		await current.sessionManager.flushAll({ commit: true });
		log("INFO", "plugin", "OpenViking plugin disposed");
	});

	// viking:// 资源必须走 openviking_* 工具，拦截 read/grep/glob/bash 直接访问
	pi.on("tool_call", async (event) => {
		if (!runtime?.enabled) return;
		try {
			await runtime.uriGuard({ tool: event.toolName, args: event.input }, { args: event.input });
		} catch (error) {
			return { block: true, reason: error instanceof Error ? error.message : String(error) };
		}
	});

	// bash 等工具结果里出现 viking:// 路径时追加提示
	pi.on("tool_result", async (event) => {
		if (!runtime?.enabled) return;
		const original = textOfContent(event.content);
		const holder = { output: original };
		await runtime.uriNotice({ tool: event.toolName, args: event.input }, holder);
		if (holder.output !== original) {
			const notice = holder.output.slice(original.length).trim();
			if (notice) event.content.push({ type: "text", text: notice });
		}
	});
}
