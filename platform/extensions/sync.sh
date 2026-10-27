#!/bin/zsh
# 同步 rad 平台扩展（仓库源 → rad 用户级扩展目录，对所有 cwd 生效）。
# 修改源文件后运行本脚本；rad 新会话加载生效。
set -euo pipefail

SRC="$(cd "$(dirname "$0")" && pwd)"
DEST="${HOME}/.rad/agent/extensions"

mkdir -p "$DEST"
for file in ov.ts; do
  if [[ -f "$SRC/$file" ]]; then
    cp "$SRC/$file" "$DEST/$file"
    echo "synced $file -> $DEST/$file"
  fi
done
