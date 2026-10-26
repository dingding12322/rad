# AI 项目指南（rad）

给 AI agent 看的项目简报：读完这页应能正确定位代码、遵守边界、不踩已知坑。
深度细节见指针文档，不要凭猜测修改本文件未覆盖的链路。

## 项目是什么

rad = 一体化个人 agent 平台，不是 pi 的 fork：

- **内核**：魔改 pi coding-agent，唯一上游差异是
  `packages/coding-agent/package.json` 的 `bin: rad` 与 `piConfig.configDir: ".rad"`
  两行。其余与上游 `earendil-works/pi` 保持可同步。
- **扩展**：`.rad/extensions/` —— `plan-gate.ts`（大任务先出 plan、GUI 弹窗审批）
  与 openviking 记忆接入（开发中）。
- **桌面壳**：`platform/orbit/`（vendored fork，0.2.12 快照，自有维护）。
- **外围**（外部依赖，不进仓库、不魔改）：Herdr、pi-subagents、
  OpenCode 及其 OpenViking 插件。

## 运行时拓扑（谁读谁）

```
Orbit.app (自建二进制，GPUI/Rust)
 ├─ ~/.orbit-pi/binary.json      → 启动哪个 agent（bin: ~/Documents/command-line-tools/bin/rad,
 │                                  packageRoot: 仓库 packages/coding-agent）
 ├─ ~/.orbit-pi/agent-dir.json   → agent 数据目录（指向 ~/.rad/agent）
 └─ orbit-rpc::agent_dir()       → 覆盖 → piConfig.configDir 推导 → ~/.pi/agent 兜底
rad (bin 脚本)                    → Node 24 + dist/bundle/cli.js（系统 Node 18 会崩）
 ├─ ~/.rad/agent/auth.json       → 服务商密钥（与 ~/.pi/agent/auth.json 保持合并）
 ├─ ~/.rad/agent/models.json     → 用户级模型覆盖（如全模型 contextWindow=476000）
 └─ ~/.rad/agent/mcp.json        → MCP（openviking: stdio proxy → 127.0.0.1:1933/mcp）
OpenViking server (Python, uv)    → http://127.0.0.1:1933；Web 控制台 /studio/
 └─ ~/.openviking/ovcli.conf     → api_key/account/user（JSON plist；Studio 注入源）
```

## 红线（违反即事故）

1. 凭证（`~/.rad/agent/auth.json`）与会话文件绝不入库。
2. 机器绝对路径不入库——运行时推导（homedir / configDir / which）。
3. `platform/orbit` 不跟随上游同步：定期吸收新技术，上游好改动 cherry-pick。
4. 外部依赖不魔改：Herdr、pi-subagents、OpenCode 插件、官方 pi（/opt/homebrew/bin/pi）。
5. 与上游冲突面保持最小：内核只允许 `package.json` 那两行差异。

## 构建与验证

- 内核：仓库 Node 24（默认 nvm v18 会 `enableCompileCache` 语法崩）；
  改动后 `npm run check`；测试用 `./test.sh`，勿全量 vitest（含 e2e）。
- Orbit：`cd platform/orbit && cargo build --release`；测试
  `cargo test -p orbit-pi --release`（1076 项，i18n 覆盖测试要求 12 个
  locales 全部含 en 的每个 key）。
- **替换 /Applications 的 Orbit 二进制（标准流程，缺一必炸）**：
  ```bash
  killall orbit-pi                                   # 1. 先停（覆盖运行中二进制=签名失效闪退）
  cp target/release/orbit-pi "/Applications/Orbit Pi.app/Contents/MacOS/orbit-pi"
  codesign --force --deep -s - "/Applications/Orbit Pi.app"   # 2. 后签（cp 后 hash 不匹配，Gatekeeper 会 kill）
  ```
  重签后首次启动有 Gatekeeper 全量校验卡顿，属正常一次性成本。

## 已知坑（勿重蹈）

- **objc2 手写消息（platform/orbit/crates/orbit-pi/src/studio.rs）**：
  - `addUserScript:` 属于 `WKUserContentController`，不是 `WKWebViewConfiguration`；
  - `URLWithString:` / `requestWithURL:` 等便利构造器返回 autoreleased 对象，
    手动 release 即过度释放（导航中途 CFRelease 崩）；
  - struct 字面量不要内联进 `msg_send!` 参数表；
  - WebKit framework 需显式 `#[link]`，否则运行时 `class!` 找不到类。
- **写拦截器/hook 类文件**禁止“顶掉式”字符串替换；每次替换立即语法检查。
- **`printf > symlink` 会穿透**：写文件前先 realpath 检查。
- 网络需代理 `127.0.0.1:7890`（GitHub push/release 下载）。
- Studio 控制台连接存在 localStorage `ov_console_connection`
  （accountId/adminApiKey/apiKey/baseUrl/userId）。

## 文档指针

- `platform/ARCHITECTURE.md` —— 分层架构与状态清单
- `platform/orbit/RAD-NOTES.md` —— Orbit 维护策略、魔改清单、构建前提
