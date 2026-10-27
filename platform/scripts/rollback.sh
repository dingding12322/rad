#!/bin/zsh
# Orbit 回滚：把生产 App 恢复到指定（或最新）的备份版本。
# 用法：platform/scripts/rollback.sh [备份文件路径]

set -euo pipefail

APP="/Applications/Orbit Pi.app"
DEST="$APP/Contents/MacOS/orbit-pi"
BACKUP_DIR="$HOME/.orbit-pi/backups"

TARGET="${1:-$(ls -t "$BACKUP_DIR"/orbit-pi.* 2>/dev/null | head -1)}"
[ -n "$TARGET" ] && [ -f "$TARGET" ] || { echo "无可用备份（$BACKUP_DIR）"; exit 1; }

killall orbit-pi 2>/dev/null && sleep 1 || true
cp "$TARGET" "$DEST"
codesign --force --deep -s - "$APP" >/dev/null 2>&1

codesign -vv "$APP" >/dev/null 2>&1 && echo "已回滚到 $TARGET" || { echo "验签失败"; exit 1; }
