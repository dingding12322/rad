# Orbit — rad 的桌面壳（vendored fork）

来源：https://github.com/imrj05/orbit （v0.2.12 快照，Apache-2.0）

## 维护策略

- **不跟随上游同步**。Orbit 自导入之日起是 rad 的自有组件，维护方式为
  **定期吸收新技术完善它**：关注 GPUI/Zed 生态、Rust GUI 新实践、pi RPC 协议
  演进，择优合入；上游若有个别好改动，cherry-pick 即可，不做整体 rebase。
- 源头仓库仅作参考：https://github.com/imrj05/orbit

## 已知魔改目标（按优先级）

1. **configDir 跟随 rad**：✅ 已完成并中心化（2026-10-26）。权威解析在
   `orbit-rpc::client.rs::agent_dir()`：`~/.orbit-pi/agent-dir.json`
   （`{ "agentDir": "/absolute/path" }`）→ 由 pi 包 `package.json` 的
   `piConfig.configDir` 推导（rad=`.rad`）→ 官方 `~/.pi/agent` 兜底。
   sessions、`auth.json`、`models.json`（providers.rs）、skills、plugins
   全部委托该函数，Orbit 的凭证写入与 harness 读取同源。
2. **二进制/包根覆盖**：✅ 已完成（2026-10-26）。
   `~/.orbit-pi/binary.json`（`{ "bin": "...", "packageRoot": "..." }`，
   进程内读一次缓存）：bin 作为首选候选直接拉起（先于 `PI_BIN`/PATH），
   packageRoot 经 `is_pi_package` 校验后供 RPC patcher 使用。双击启动的
   最小 PATH 不再误解析到官方 homebrew pi + 系统 Node 18。
3. **OpenViking Studio 原生窗口**：✅ 已完成（2026-10-26）。
   `crates/orbit-pi/src/studio.rs`：Settings→About 入口 → 后台探测
   `/health` → macOS 弹原生 WKWebView 窗口（复用 GPUI 的 NSApplication，
   显式链接 WebKit），启动脚本注入 `ov_console_connection`
   （key/account/user 取自 `~/.openviking/ovcli.conf`，仅在 Studio 尚未
   配置过 key 时写入）；失败/非 macOS 回退系统浏览器。
4. **UI 品味**：外观遵循 `产品界面风格`——安静、层级清楚，反渐变/玻璃拟态/SaaS 卡片堆叠。

## 构建前提

- Rust 1.94+（`rustup` 未装时 `platform/setup/` 脚本负责引导）
- `cargo build --release`；或直接用官方 release 的 DMG（但魔改必须自建）
- 替换 `/Applications/Orbit Pi.app` 内二进制的标准流程（顺序不能错）：
  1. `killall orbit-pi`（覆盖运行中的二进制 = 签名失效，App 直接闪退）
  2. `cp target/release/orbit-pi "/Applications/Orbit Pi.app/Contents/MacOS/orbit-pi"`
  3. `codesign --force --deep -s - "/Applications/Orbit Pi.app"`
     （cp 后 CodeResources hash 不匹配，不重签 Gatekeeper 会 kill）
  重签后首次启动有一次性的 Gatekeeper 全量校验卡顿，属正常。

## objc2 手写消息纪律（studio.rs 血泪）

- `addUserScript:` 在 `WKUserContentController` 上，不在 configuration 上。
- `URLWithString:` / `requestWithURL:` 等便利构造器返回 autoreleased 对象，
  **不要手动 release**（过度释放 = 导航中途 CFRelease 崩）；`alloc`/`init`
  的 +1 才需要配对 release。
- struct 字面量（NSRect 等）先绑定局部变量，不内联进 `msg_send!` 参数表。
- 用到的 framework 显式 `#[link(name = "WebKit", kind = "framework")]`，
  否则运行时 `class!` 找不到类。
- 诊断路径：`~/Library/Logs/DiagnosticReports/*.ips` 看崩溃栈；
  `lldb -b -o "breakpoint set -n objc_exception_throw" -o run ...` 抓
  NSException 的 reason（unrecognized selector 会直接打印在异常对象里）。
