#!/usr/bin/env bash
# 默认只停 NeoB 浏览器；--all 同时停止并卸载自建 server。
# 只匹配 NeoB 自己的 profile 与 sidecar 路径，不影响其他浏览器实例。
set -uo pipefail

STATE_DIR="$HOME/Library/Application Support/NeoB/state"
PROFILE_DIR="$HOME/Library/Application Support/NeoB/profile"

echo "==> 停浏览器（仅 NeoB profile）"
pkill -f "user-data-dir=$PROFILE_DIR" 2>/dev/null && echo "已停止" || echo "未在运行"

if [ "${1:-}" = "--all" ]; then
  echo "==> 停 claw-server（仅 NeoB sidecar）"
  pkill -f "$STATE_DIR/sidecar.json" 2>/dev/null && echo "已停止" || echo "未在运行"
  echo "==> 卸载 server LaunchAgent"
  launchctl bootout "gui/$(id -u)/${NEOB_SERVER_LABEL:-com.sam.neob.server}" 2>/dev/null \
    && echo "已卸载" || echo "未加载"
fi
