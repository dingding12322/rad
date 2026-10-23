/**
 * plan-gate — 大任务 plan 确认强拦截（Pi 扩展，移植自 OpenCode 插件）
 * 规则: 大任务必须先输出 plan，用户明确同意后才允许改代码。
 * 判定: input 命中关键词 + 长任务启发式；同意关键词解除。
 * 阻断: tool_call 对 edit/write 返回 { block: true }，Pi 官方 gate 机制。
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const LARGE_RE = /大任务|重构|新功能|新模块|多文件|全量|批量|架构|走plan|plan先行|ultrawork|^\/plan|创建方案|设计方案/i;
const APPROVE_RE = /同意|确认执行|批准|可以执行|开始执行|就按这个来|没问题.*执行|修复|approve|approved|lgtm|go\s*ahead|ship\s*it/i;
const WRITE_TOOLS = new Set(["edit", "write"]);

const PLAN_GATE_RULES = [
	"[plan-gate 规则]",
	"1. 用户需求命中大任务(重构/新功能/多文件/架构/创建方案/消息过长)时,只做只读探索(read/grep/glob/bash查询类),必须先输出结构化 plan:目标/步骤/涉及文件/风险/验证方式,然后停下等待用户确认。",
	"2. 未收到用户明确同意(同意/确认执行/批准/approve/go ahead)前,严禁调用 edit/write,扩展层会直接阻断。",
	"3. 用户同意后,按 plan 分步执行,保持改动最小。",
].join("\n");

const PLAN_GATE_NOTICE =
	"[plan-gate] 本次命中大任务流程:请先只读调研并输出 plan,等待用户回复同意后再改代码。此时调用 edit/write 会被阻断。";

const BLOCK_REASON =
	"[plan-gate] 大任务未确认:请先输出 plan 并等待用户回复 同意/确认执行 后再改代码。本次写操作已阻断(只读调研不受影响)。";

export default function (pi: ExtensionAPI) {
	let pending = false;

	pi.on("before_agent_start", (event) => {
		event.systemPromptOptions.sections.plan_gate = PLAN_GATE_RULES;
	});

	pi.on("input", (event) => {
		const raw = event.text;
		// 剥离其他扩展前置注入的内容(openviking-context 块、本扩展 notice),
		// 只对用户原始输入判定,避免 openviking 的 input transform 先注入长文本导致误触发
		const text = raw
			.replace(/<openviking-context[\s\S]*?<\/openviking-context>/g, "")
			.replace(/^\[plan-gate\][^\n]*\n?/gm, "")
			.trim();
		if (!text) return;
		if (APPROVE_RE.test(text)) {
			pending = false;
			return;
		}
		if (LARGE_RE.test(text) || text.length > 300) {
			pending = true;
			return { action: "transform", text: `${PLAN_GATE_NOTICE}\n\n${raw}` };
		}
	});

	pi.on("tool_call", (event) => {
		if (!WRITE_TOOLS.has(event.toolName)) return;
		if (pending) return { block: true, reason: BLOCK_REASON };
	});
}
