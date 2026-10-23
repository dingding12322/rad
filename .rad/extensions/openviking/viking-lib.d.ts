/**
 * Minimal type surface of @openviking/opencode-plugin lib modules used by index.ts.
 * The package ships untyped .mjs; these declarations only satisfy editors — runtime
 * behavior is the npm package's responsibility.
 */
declare module "*@openviking/opencode-plugin/lib/config.mjs" {
	export function loadConfig(pluginRoot: string, projectDirectory: string): any;
	export function resolveDataDir(pluginRoot: string, config: any): string;
}

declare module "*@openviking/opencode-plugin/lib/runtime.mjs" {
	export function initializeRuntime(config: any, client?: unknown): Promise<boolean>;
}

declare module "*@openviking/opencode-plugin/lib/repo-context.mjs" {
	export function createRepoContext(options: { config: any }): {
		refreshRepos(options?: { force?: boolean }): Promise<string | null>;
		getRepoSystemPrompt(): string | null;
	};
}

declare module "*@openviking/opencode-plugin/lib/memory-session.mjs" {
	export function createMemorySessionManager(options: { config: any; pluginRoot: string }): {
		init(): Promise<void>;
		handleEvent(event: { type: string; properties?: any }): Promise<void>;
		getMappedSessionId(sessionId: string): string;
		commitSession(sessionId: string, harnessSessionId: string, abortSignal?: AbortSignal): Promise<unknown>;
		flushSession(sessionId: string, options?: { commit?: boolean; reason?: string }): Promise<boolean>;
		flushAll(options?: { commit?: boolean }): Promise<void>;
	};
}

declare module "*@openviking/opencode-plugin/lib/memory-recall.mjs" {
	export function createMemoryRecall(options: { config: any; sessionManager: any }): {
		injectRelevantMemories(input: any, output: { parts: any[] }): Promise<void>;
	};
}

declare module "*@openviking/opencode-plugin/lib/session-inject.mjs" {
	export function createSessionInject(options: { config: any; sessionManager: any }): {
		injectSessionContext(input: any, output: { parts: any[] }): Promise<boolean>;
	};
}

declare module "*@openviking/opencode-plugin/lib/viking-uri-guard.mjs" {
	export function createVikingUriGuard(): (
		input: { tool?: string; name?: string; args?: any },
		output: { args?: any },
	) => Promise<void>;
	export function createVikingUriNotice(): (
		input: { tool?: string; name?: string; args?: any },
		output?: { output?: string },
	) => Promise<void>;
}

declare module "*@openviking/opencode-plugin/lib/utils.mjs" {
	export function initLogger(dataDir: string): void;
	export function log(
		level: "DEBUG" | "INFO" | "WARN" | "ERROR",
		tool: string,
		message: string,
		data?: unknown,
	): void;
}
