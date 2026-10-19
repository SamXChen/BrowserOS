#!/usr/bin/env bash
# 注册 NeoB 为登录项常驻（launchd LaunchAgent）
set -euo pipefail

cd "$(dirname "$0")"
LABEL="com.sam.neob"
UID_NUM="$(id -u)"

if [ ! -d "$HOME/Applications/NeoB.app" ]; then
  echo "未找到 ~/Applications/NeoB.app —— 先运行 ./build-app.sh" >&2
  exit 1
fi

mkdir -p "$HOME/Library/LaunchAgents"
cp "LaunchAgents/$LABEL.plist" "$HOME/Library/LaunchAgents/$LABEL.plist"

launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID_NUM" "$HOME/Library/LaunchAgents/$LABEL.plist"
launchctl kickstart -p "gui/$UID_NUM/$LABEL"

sleep 2
launchctl print "gui/$UID_NUM/$LABEL" | head -6
