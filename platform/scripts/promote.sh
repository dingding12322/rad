#!/bin/zsh
# Orbit 生产版晋升：验证构建 → 备份现版本 → 停止运行实例 → 替换 → 重签 → 验签。
# 替换生产 App 前必须先在 dev 形态（直接运行 target/release/orbit-pi）冒烟通过。
# 用法：platform/scripts/promote.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/platform/orbit/target/release/orbit-pi"
APP="/Applications/Orbit Pi.app"
DEST="$APP/Contents/MacOS/orbit-pi"
BACKUP_DIR="$HOME/.orbit-pi/backups"

[ -x "$BIN" ] || { echo "构建产物不存在：先 cargo build --release"; exit 1; }

# 1. 备份当前生产二进制（最近一份已知可用的回滚点；只留 5 份）
mkdir -p "$BACKUP_DIR"
if [ -f "$DEST" ]; then
  cp "$DEST" "$BACKUP_DIR/orbit-pi.$(date +%Y%m%d-%H%M%S)"
  ls -t "$BACKUP_DIR"/orbit-pi.* 2>/dev/null | tail -n +6 | xargs rm -f 2>/dev/null || true
fi

rollback() {
  local latest
  latest=$(ls -t "$BACKUP_DIR"/orbit-pi.* 2>/dev/null | head -1)
  if [ -n "$latest" ]; then
    cp "$latest" "$DEST"
    codesign --force --deep -s - "$APP" >/dev/null 2>&1
    echo "已回滚到 $latest"
  fi
}

# 2. 停止运行中的 App（覆盖运行中二进制 = 签名失效，直接闪退）
killall orbit-pi 2>/dev/null && sleep 1 || true

# 3. 替换 + 重签
cp "$BIN" "$DEST"
codesign --force --deep -s - "$APP" >/dev/null 2>&1

# 4. 验签；失败即回滚
if codesign -vv "$APP" >/dev/null 2>&1; then
  echo "promoted：$(basename "$DEST") 已更新并重签（旧版本备份于 $BACKUP_DIR）"
  echo "首次启动会有一次性的 Gatekeeper 校验卡顿。"
else
  echo "验签失败！"
  rollback
  exit 1
fi
