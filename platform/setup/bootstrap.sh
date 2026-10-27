#!/bin/zsh
# rad 平台环境检查与引导（幂等：已就绪的跳过，缺失的给出一行安装指引）。
# 完整文档见仓库 AI.md。用法：platform/setup/bootstrap.sh

set -uo pipefail

OK=() MISSING=()

check() { # check <名称> <检测命令> <安装指引>
  if eval "$2" >/dev/null 2>&1; then
    OK+=("$1")
  else
    MISSING+=("$1")
    echo "MISS  $1"
    echo "      → $3"
  fi
}

check "Node 24" "[ -x \"$HOME/.nvm/versions/node/v24\"*/bin/node ] || [ -d \"$HOME/.nvm/versions/node\"/v24* ]" "nvm install 24 && nvm use 24"
check "rad 内核 dist 构建" "[ -f \"$HOME/Desktop/work/rad/packages/coding-agent/dist/bundle/cli.js\" ]" "cd ~/Desktop/work/rad && npm install --ignore-scripts && npm run build"
check "rad bin 脚本" "[ -x \"$HOME/Documents/command-line-tools/bin/rad\" ]" "见 platform/ARCHITECTURE.md（包装脚本：Node 24 + dist/bundle/cli.js + --archive-scan 分支）"
check "Rust 工具链" "cargo --version" "curl https://sh.rustup.rs | sh"
check "Orbit 自建二进制" "[ -x \"$HOME/Desktop/work/rad/platform/orbit/target/release/orbit-pi\" ]" "cd ~/Desktop/work/rad/platform/orbit && cargo build --release"
check "Orbit App 二进制为自建版（ad-hoc 签名）" "(codesign -dv \"/Applications/Orbit Pi.app/Contents/MacOS/orbit-pi\" 2>&1 || true) | grep -q 'Signature=adhoc' && codesign -vv \"/Applications/Orbit Pi.app\" >/dev/null 2>&1" "killall orbit-pi; cp platform/orbit/target/release/orbit-pi '/Applications/Orbit Pi.app/Contents/MacOS/orbit-pi'; codesign --force --deep -s - '/Applications/Orbit Pi.app'"
check "Orbit 覆盖 binary.json" "[ -f \"$HOME/.orbit-pi/binary.json\" ]" '{"bin":"<rad bin 路径>","packageRoot":"<仓库 packages/coding-agent>"}'
check "Orbit 覆盖 agent-dir.json" "[ -f \"$HOME/.orbit-pi/agent-dir.json\" ]" '{"agentDir":"<HOME>/.rad/agent"}'
check "rad 凭证 auth.json" "[ -s \"$HOME/.rad/agent/auth.json\" ]" "从 ~/.pi/agent/auth.json 合并服务商密钥（绝不入库）"
check "rad MCP: openviking" "grep -q openviking \"$HOME/.rad/agent/mcp.json\"" "rad mcp add openviking --exposure direct -- <node24> <ov插件>/servers/mcp-proxy.mjs"
check "rad MCP: rea" "grep -q '\"rea\"' \"$HOME/.rad/agent/mcp.json\"" "rad mcp add rea --exposure direct -- npx -y rea-agents@6.4.0 mcp"
check "模型目录 models.json（476K）" "grep -q 476000 \"$HOME/.rad/agent/models.json\"" "见 AI.md（modelOverrides contextWindow）"
check "OpenViking server" "curl -sf --max-time 3 http://127.0.0.1:1933/health >/dev/null" "uv tool install openviking && openviking-server --config ~/.openviking/ov.conf"
check "OV 凭证 ovcli.conf" "[ -s \"$HOME/.openviking/ovcli.conf\" ]" "向 OV 服务端获取 api_key"
check "Herdr" "\"$HOME/.local/bin/herdr\" --version >/dev/null 2>&1 || herdr --version >/dev/null 2>&1" "独立安装（外部依赖，不魔改）"
check "归档/记忆扩展已同步" "cmp -s \"$HOME/Desktop/work/rad/platform/extensions/ov.ts\" \"$HOME/.rad/agent/extensions/ov.ts\"" "platform/extensions/sync.sh"
check "GitHub 代理可达" "git ls-remote https://github.com/dingding12322/rad >/dev/null 2>&1 || git -c http.proxy=http://127.0.0.1:7890 ls-remote https://github.com/dingding12322/rad >/dev/null 2>&1" "网络需代理 127.0.0.1:7890"

echo
echo "就绪 ${#OK[@]} 项；缺失 ${#MISSING[@]} 项。"
(( ${#MISSING[@]} == 0 )) && echo "环境完整。" || exit 1
