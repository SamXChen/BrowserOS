#!/usr/bin/env bash
# 注销常驻并移除 NeoB.app
set -euo pipefail

cd "$(dirname "$0")"
LABEL="com.sam.neob"
SERVER_LABEL="${NEOB_SERVER_LABEL:-com.sam.neob.server}"
UID_NUM="$(id -u)"

launchctl bootout "gui/$UID_NUM/$SERVER_LABEL" 2>/dev/null || true
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
rm -f "$HOME/Library/LaunchAgents/$SERVER_LABEL.plist"
rm -rf "$HOME/Applications/NeoB.app"

echo "已注销 $LABEL 并移除 ~/Applications/NeoB.app"
echo "（profile 与 server 状态保留在 ~/Library/Application Support/NeoB）"
