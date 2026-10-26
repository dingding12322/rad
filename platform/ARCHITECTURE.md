# rad 架构：一体化个人 agent 平台

rad 不是 pi 的 fork，而是一体化仓库：内核、扩展、桌面壳、控制层、安装脚本
都在这里。clone 后按 `platform/setup/` 引导即可完整还原工作环境。

## 分层

```
rad/
├─ packages/                 # 内核：魔改 pi（bin=rad，configDir=.rad）
│  └─ coding-agent …
├─ .rad/                     # 内核的项目级资源（rad 运行时直接加载）
│  └─ extensions/            # plan-gate（大任务拦截）、openviking（记忆）
└─ platform/                 # 一体化平台层
   ├─ orbit/                 # 桌面壳（vendored fork，自有维护）
   │  └─ RAD-NOTES.md        # 维护策略与魔改清单
   └─ setup/                 # 环境还原脚本（规划中）
```

## 外部依赖（不进仓库，声明式使用）

| 件 | 角色 | 接入方式 |
|---|---|---|
| Herdr | 终端多会话控制层（Rust） | 独立安装，不魔改 |
| pi-subagents / pi-crew 等 | 编排扩展 | `rad install`，settings 声明 |
| pi-agent-dashboard | 浏览器监控 | 可选 |
| OpenViking server | 个人记忆服务（Python/uv） | 本地 `127.0.0.1:1933`，MCP 经 `~/.rad/agent/mcp.json` |

## 边界红线

- 凭证（`~/.rad/agent/auth.json`）与会话文件**绝不入库**；setup 只负责引导生成。
- 机器特定绝对路径不入库；一律运行时推导（homedir / which / configDir）。
- 第三方 vendored 件（如 orbit）保留其 LICENSE/NOTICE，魔改记录写在各自 RAD-NOTES.md。

## 当前状态

- [x] 内核改名 rad、configDir=.rad
- [x] plan-gate 扩展：GUI 弹窗审批（拦截写操作时 `ctx.ui.confirm`，
      Orbit 渲染为带「通过」按钮的对话框；headless 保持硬阻断）
- [x] orbit vendored（0.2.12 快照）
- [x] orbit 二进制覆盖 `~/.orbit-pi/binary.json`（bin=rad 启动脚本 +
      packageRoot=仓库包，双击启动不再误拉官方 pi）
- [x] orbit agent-dir 中心化（sessions/auth.json/models.json/skills/plugins
      全部跟随 `~/.rad/agent`，凭证写读同源）
- [x] OpenViking 接入：MCP 15 工具直连 + Studio 原生 WKWebView 窗口
      （Settings→About 入口，key 自动注入 `ov_console_connection`）
- [x] 模型上下文统一 476K（`~/.rad/agent/models.json` modelOverrides）
- [ ] platform/setup 一键还原脚本
- [ ] openviking 记忆 hook 移植为 rad 扩展（plan 已出：session_start/
      context/before_agent_start/agent_settled/session_before_compact/
      session_shutdown/tool_call 七事件映射，待实施）

AI agent 进入项目请先读根目录 `AI.md`（运行时拓扑、红线、构建验证、已知坑）。
- [ ] Rust 工具链引导（orbit 本地构建前提）
