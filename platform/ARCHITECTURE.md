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

## 边界红线

- 凭证（`~/.rad/agent/auth.json`）与会话文件**绝不入库**；setup 只负责引导生成。
- 机器特定绝对路径不入库；一律运行时推导（homedir / which / configDir）。
- 第三方 vendored 件（如 orbit）保留其 LICENSE/NOTICE，魔改记录写在各自 RAD-NOTES.md。

## 当前状态

- [x] 内核改名 rad、configDir=.rad
- [x] plan-gate / openviking 扩展
- [x] orbit vendored（0.2.12 快照）
- [ ] platform/setup 一键还原脚本
- [ ] orbit configDir 跟随 rad（去掉 ~/.pi symlink 桥接）
- [ ] Rust 工具链引导（orbit 本地构建前提）
