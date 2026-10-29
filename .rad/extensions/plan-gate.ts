/**
 * plan-gate — 大任务 plan 确认强拦截（rad 扩展，移植自 OpenCode 插件）
 * 规则: 大任务必须先输出 plan，用户通过后才允许改代码。
 * 判定: input 命中关键词 + 长任务启发式；同意关键词或审批弹窗「通过」解除
 *       （否定语境与长消息不算批准）。
 * 阻断: tool_call 拦截非只读工具——有 UI 时弹审批对话框（Orbit 渲染为带
 *       「通过」按钮的对话框，TUI 为原生确认），headless 保持硬阻断。
 * 状态: pending 期间在状态栏显示提示，通过或会话结束即清除。
 */
import type { ExtensionAPI, ToolCallEvent } from "@earendil-works/pi-coding-agent";

const LARGE_RE = /大任务|重构|新功能|新模块|多文件|全量|批量|架构|走plan|plan先行|ultrawork|^\/plan|创建方案|设计方案/i;
const APPROVE_RE = /同意|确认执行|批准|可以执行|开始执行|就按这个来|没问题.*执行|修复|approve|approved|lgtm|go\s*ahead|ship\s*it/i;
// 否定语境不算批准: "我不同意" / "先不要执行" / "暂不批准" / "无需执行" / "not approved"
const APPROVAL_NEGATION_RE = /(不|别|未|无需|先不|暂不|暂缓|不能)[^，。；！？\n]{0,4}(同意|批准|执行)|\bnot\b[^.,;!?]{0,10}(approve|execute)/i;
// 批准只认短消息: 长消息里出现"同意"多为讨论语境而非指令
const APPROVAL_MAX_LEN = 200;
const PLAN_READ_ONLY_TOOLS = new Set([
	"read",
	"grep",
	"find",
	"ls",
	"skill_search",
	"tool_search",
]);
const READ_ONLY_BASH_COMMAND = /^(?:pwd|ls|rg|grep|find|fd|sed|head|tail|cat|wc|file|stat|git\s+(?:status|diff|log|show|ls-files|rev-parse))(?:\s|$)/;
const BASH_SIDE_EFFECT_SYNTAX = /[;&|<>`$(){}\n]|\s(?:-i|--in-place|-delete|-exec(?:dir)?|--output(?:=|\s))/;
const STATUS_KEY = "plan-gate";

const PLAN_GATE_RULES = [
	"[plan-gate 规则]",
	"1. 用户需求命中大任务(重构/新功能/多文件/架构/创建方案/消息过长)时,只做只读探索(read/grep/find/ls/bash查询类),或使用 skill_search/tool_search 做按需发现;必须先输出结构化 plan:目标/步骤/涉及文件/风险/验证方式,然后停下等待用户确认。",
	"2. 未收到用户明确同意(同意/确认执行/批准/approve/go ahead)前,严禁调用 edit/write 或其他非只读工具,扩展层会直接阻断。",
	"3. 也不要用 bash 重定向、脚本或其他间接方式写文件;等待确认期间只做只读调研。",
	"4. 用户同意后,按 plan 分步执行,保持改动最小。",
].join("\n");

const PLAN_GATE_NOTICE =
	"[plan-gate] 本次命中大任务流程:请先只读调研并输出 plan,等待用户回复同意后再改代码。此时写入或其他副作用工具会被阻断。";

const BLOCK_REASON =
	"[plan-gate] 大任务未确认:请先输出 plan 并等待用户回复 同意/确认执行 后再改代码。本次非只读工具调用已阻断(只读调研和按需发现不受影响)。";

const PENDING_STATUS = "plan-gate: 大任务已拦截,等待用户同意,非只读操作已阻断";

function isApproval(text: string): boolean {
	if (text.length > APPROVAL_MAX_LEN) return false;
	if (APPROVAL_NEGATION_RE.test(text)) return false;
	return APPROVE_RE.test(text);
}

function isPlanReadOnlyTool(event: ToolCallEvent): boolean {
	if (event.toolName === "bash") {
		const command = event.input.command.trim();
		return !BASH_SIDE_EFFECT_SYNTAX.test(command) && READ_ONLY_BASH_COMMAND.test(command);
	}
	return PLAN_READ_ONLY_TOOLS.has(event.toolName);
}

export default function (pi: ExtensionAPI) {
	let pending = false;

	pi.on("before_agent_start", (event) => {
		event.systemPromptOptions.sections.plan_gate = PLAN_GATE_RULES;
	});

	pi.on("input", (event, ctx) => {
		const raw = event.text;
		// 剥离其他扩展前置注入的内容(openviking-context 块、本扩展 notice),
		// 只对用户原始输入判定,避免 openviking 的 input transform 先注入长文本导致误触发
		const text = raw
			.replace(/<openviking-context[\s\S]*?<\/openviking-context>/g, "")
			.replace(/^\[plan-gate\][^\n]*\n?/gm, "")
			.trim();
		if (!text) return;

		if (isApproval(text)) {
			pending = false;
			ctx.ui.setStatus(STATUS_KEY, undefined);
			return;
		}
		if (LARGE_RE.test(text) || text.length > 300) {
			const wasPending = pending;
			pending = true;
			ctx.ui.setStatus(STATUS_KEY, PENDING_STATUS);
			// 已拦截状态下不重复注入 notice,避免每条消息都污染转录
			if (wasPending) return;
			return { action: "transform", text: `${PLAN_GATE_NOTICE}\n\n${raw}` };
		}
	});

	// 拦截非只读工具:有 UI 时弹确认框(Orbit 渲染为带「通过」按钮的对话框,TUI 为原生确认),
	// 用户点通过即放行本次任务;无 UI(headless)保持硬阻断
	pi.on("tool_call", async (event, ctx) => {
		if (!pending) return;
		if (isPlanReadOnlyTool(event)) return;
		if (ctx.hasUI) {
			const approved = await ctx.ui.confirm(
				"plan-gate：等待 Plan 审批",
				"Agent 已输出 Plan 并请求执行非只读操作。通过后本次任务不再拦截。",
			);
			if (approved) {
				pending = false;
				ctx.ui.setStatus(STATUS_KEY, undefined);
				return;
			}
			return { block: true, reason: "[plan-gate] 用户未通过：继续只读调研或按需发现，或回复同意后再试。" };
		}
		return { block: true, reason: BLOCK_REASON };
	});
}
