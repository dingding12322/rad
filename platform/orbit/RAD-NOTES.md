# Orbit — rad 的桌面壳（vendored fork）

来源：https://github.com/imrj05/orbit （v0.2.12 快照，Apache-2.0）

## 维护策略

- **不跟随上游同步**。Orbit 自导入之日起是 rad 的自有组件，维护方式为
  **定期吸收新技术完善它**：关注 GPUI/Zed 生态、Rust GUI 新实践、pi RPC 协议
  演进，择优合入；上游若有个别好改动，cherry-pick 即可，不做整体 rebase。
- 源头仓库仅作参考：https://github.com/imrj05/orbit

## 已知魔改目标（按优先级）

1. **configDir 跟随 rad**：✅ 已完成（2026-10-26）。`sessions_dir()` 读取
   `~/.orbit-pi/agent-dir.json` 的 `{ "agentDir": "/absolute/path" }` 覆盖
   （沿用本仓 `~/.orbit-pi/*.json` 每功能一文件的持久化惯例），缺省回退官方
   `~/.pi/agent`。rad 环境已配置指向 `~/.rad/agent`，symlink 桥已拆除。
   注意：仅会话存储走此覆盖；二进制解析（`resolve_pi_bin`）仍按
   设置页 binary / `PI_BIN` / `~/.local/bin/pi` 垫片的顺序。
2. **二进制探测跟随 rad**：`crates/orbit-rpc/src/client.rs` 的
   `resolve_pi_bin()` 默认找 `pi`（含 `~/.local/bin/pi` 探测），应支持
   `PI_BIN` 之外的持久化配置（设置页已有 `settings.binary` 字段，确认其
   持久化与默认值逻辑）。
3. **UI 品味**：外观遵循 `产品界面风格`——安静、层级清楚，反渐变/玻璃拟态/SaaS 卡片堆叠。

## 构建前提

- Rust 1.94+（`rustup` 未装时 `platform/setup/` 脚本负责引导）
- `cargo build --release`；或直接用官方 release 的 DMG（但魔改必须自建）
