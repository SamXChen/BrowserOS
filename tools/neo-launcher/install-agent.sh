#!/usr/bin/env bash
# 注册 NeoB 为登录项常驻（launchd LaunchAgent）
set -euo pipefail

cd "$(dirname "$0")"
LABEL="com.sam.neob"
SERVER_LABEL="${NEOB_SERVER_LABEL:-com.sam.neob.server}"
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

# The server has its own agent. Install it from the template so the paths
# follow the current state root instead of a hardcoded home directory.
STATE_ROOT="${NEOB_STATE_ROOT:-$HOME/Library/Application Support/NeoB}"
sed -e "s|__STATE__|$STATE_ROOT/state|g" \
    -e "s|__STATE_ROOT__|$STATE_ROOT|g" \
    -e "s|__LABEL__|$SERVER_LABEL|g" \
    "LaunchAgents/com.sam.neob.server.plist" \
    > "$HOME/Library/LaunchAgents/$SERVER_LABEL.plist"
launchctl bootout "gui/$UID_NUM/$SERVER_LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID_NUM" "$HOME/Library/LaunchAgents/$SERVER_LABEL.plist"

sleep 2
launchctl print "gui/$UID_NUM/$LABEL" | head -4
launchctl print "gui/$UID_NUM/$SERVER_LABEL" | grep -E "state|pid" | head -3
