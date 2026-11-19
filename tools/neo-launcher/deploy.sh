#!/bin/zsh
# Deploys one claw-server release and restarts it under launchd.
#
# The rule this script enforces: the running binary is always a complete, verified
# artifact, swapped atomically, and a failed verification leaves the previous release
# serving.
#
# Usage:
#   deploy.sh                                  build, verify, deploy
#   deploy.sh --artifact <dir>                 deploy an artifact that already exists
#   deploy.sh --rollback                       switch back to the previous release
#
# Env overrides for rehearsals: NEOB_STATE_ROOT, NEOB_SERVER_LABEL, NEOB_CONFIG_ENV.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
STATE_ROOT="${NEOB_STATE_ROOT:-$HOME/Library/Application Support/NeoB}"
STATE="$STATE_ROOT/state"
LABEL="${NEOB_SERVER_LABEL:-com.sam.neob.server}"
CONFIG_ENV="${NEOB_CONFIG_ENV:-$HOME/Applications/NeoB.app/Contents/Resources/config.env}"
ARTIFACT=""
ROLLBACK=0
PREVIOUS_CURRENT=""
SWITCHED=0
DEPLOY_OK=0

while [ $# -gt 0 ]; do
  case "$1" in
    --artifact) ARTIFACT="$2"; shift 2 ;;
    --rollback) ROLLBACK=1; shift ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

if [ -f "$CONFIG_ENV" ]; then
  # shellcheck disable=SC1090
  source "$CONFIG_ENV"
else
  echo "警告：未找到 $CONFIG_ENV，使用默认端口" >&2
fi
: "${SERVER_PORT:=9310}"
: "${CDP_PORT:=49350}"
: "${NATIVE_VERSION:=0.51.1.2}"

# Health polling: 30 attempts with a half-second gap keeps the wait under 15 s while the
# server's own integrity check runs in the background.
HEALTH_POLL_ATTEMPTS=30
HEALTH_POLL_INTERVAL=0.5

mkdir -p "$STATE" "$STATE_ROOT/server/releases"
LOCK="$STATE/deploy.lock"
if [ -f "$LOCK" ]; then
  echo "已有部署在进行（$LOCK）。确认没有其他 deploy 在跑后删除它。" >&2
  exit 1
fi
echo "$$" > "$LOCK"
trap cleanup EXIT

health() {
  curl -s -m 2 "http://127.0.0.1:$SERVER_PORT/system/health" 2>/dev/null || true
}

stop_server() {
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  pkill -f "$STATE/sidecar.json" 2>/dev/null || true
  for _ in {1..20}; do
    if ! curl -s -m 1 "http://127.0.0.1:$SERVER_PORT/system/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "端口 $SERVER_PORT 仍被占用，中止" >&2
  exit 1
}

switch_current() {
  local release="$1"
  ln -sfn "$release" "$STATE_ROOT/server/current.tmp"
  mv -h "$STATE_ROOT/server/current.tmp" "$STATE_ROOT/server/current"
}

write_sidecar() {
  cat > "$STATE/sidecar.json" <<EOF
{
  "ports": { "server": $SERVER_PORT, "cdp": $CDP_PORT, "proxy": $SERVER_PORT },
  "directories": {
    "resources": "$STATE_ROOT/server/current/resources",
    "execution": "$STATE"
  },
  "flags": { "allow_remote_in_mcp": false },
  "instance": { "browseros_version": "$NATIVE_VERSION" }
}
EOF
}

start_server() {
  local source_plist="$HERE/LaunchAgents/$LABEL.plist"
  local target_plist="$HOME/Library/LaunchAgents/$LABEL.plist"
  if [ ! -f "$source_plist" ]; then
    echo "缺少 $source_plist；先运行 install-agent.sh 生成它" >&2
    return 1
  fi
  sed -e "s|__STATE__|$STATE|g" \
      -e "s|__STATE_ROOT__|$STATE_ROOT|g" \
      -e "s|__LABEL__|$LABEL|g" \
      "$source_plist" > "$target_plist"
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  # launchd needs the job to be gone before it accepts a new one; bootstrapping while it
  # is still tearing down fails with a bare I/O error and leaves the server down.
  for _ in {1..10}; do
    launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || break
    sleep 0.5
  done
  for _ in {1..3}; do
    launchctl bootstrap "gui/$(id -u)" "$target_plist" 2>/dev/null && break
    sleep 1
  done
  launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || {
    echo "无法加载 $LABEL；手动运行：launchctl bootstrap gui/$(id -u) $target_plist" >&2
    return 1
  }
  launchctl kickstart -k "gui/$(id -u)/$LABEL"
}

wait_for_integrity() {
  local sha12="$1"
  for _ in {1..$HEALTH_POLL_ATTEMPTS}; do
    local body=""
    body="$(health)"
    if [ -n "$body" ] && printf '%s' "$body" | python3 -c '
import json, sys
sha = sys.argv[1]
data = json.load(sys.stdin)
build = data.get("build") or {}
ok = build.get("integrity") == "ok" and (build.get("binarySha256") or "").startswith(sha)
sys.exit(0 if ok else 1)
' "$sha12" 2>/dev/null; then
      return 0
    fi
    sleep "$HEALTH_POLL_INTERVAL"
  done
  return 1
}

cleanup() {
  local rc=$?
  trap - EXIT
  set +e
  if [ "$rc" -ne 0 ] && [ "$SWITCHED" -eq 1 ] && [ "$DEPLOY_OK" -eq 0 ]; then
    echo "部署失败，恢复部署前的 release" >&2
    if [ -n "$PREVIOUS_CURRENT" ] && [ -d "$PREVIOUS_CURRENT" ]; then
      if switch_current "$PREVIOUS_CURRENT" && write_sidecar && start_server; then
        echo "current 已恢复：$PREVIOUS_CURRENT" >&2
      else
        echo "旧 release 已恢复链接，但 server 重启失败；请检查 $LABEL" >&2
      fi
    else
      launchctl bootout "gui/$(id -u)/$LABEL" >/dev/null 2>&1
      pkill -f "$STATE/sidecar.json" >/dev/null 2>&1
      rm -f "$STATE_ROOT/server/current"
      echo "部署前没有 current，已移除失败版本的链接" >&2
    fi
    echo "需要手动回滚时：NEOB_STATE_ROOT='$STATE_ROOT' NEOB_SERVER_LABEL='$LABEL' $HERE/deploy.sh --rollback" >&2
  fi
  rm -f "$LOCK"
  exit "$rc"
}

previous_release() {
  local current
  current="$(readlink "$STATE_ROOT/server/current" 2>/dev/null || true)"
  ls -1t "$STATE_ROOT/server/releases" 2>/dev/null | while read -r name; do
    local candidate="$STATE_ROOT/server/releases/$name"
    if [ "$candidate" != "$current" ]; then echo "$candidate"; return; fi
  done
}

if [ "$ROLLBACK" -eq 1 ]; then
  target="$(previous_release)"
  [ -n "$target" ] || { echo "没有可回滚的 release" >&2; exit 1; }
  stop_server
  switch_current "$target"
  write_sidecar
  start_server
  echo "已回滚到 $target"
  exit 0
fi

# ---- 1) get an artifact -----------------------------------------------------
if [ -z "$ARTIFACT" ]; then
  echo "== 构建产物"
  OUT="$(zsh "$HERE/build-artifact.sh" | tee /dev/stderr | tail -1)"
  ARTIFACT="${OUT#ARTIFACT_DIR=}"
fi
[ -d "$ARTIFACT" ] || { echo "产物目录不存在：$ARTIFACT" >&2; exit 1; }
[ -f "$ARTIFACT/artifact-metadata.json" ] || { echo "产物缺少清单：$ARTIFACT" >&2; exit 1; }

echo "== 校验产物"
bun "$HERE/checksum.ts" "$ARTIFACT"
codesign -v "$ARTIFACT/resources/bin/browseros-claw-server"

SHA12="$(shasum -a 256 "$ARTIFACT/resources/bin/browseros-claw-server" | cut -c1-12)"
RELEASE="$STATE_ROOT/server/releases/$SHA12"
if [ -d "$RELEASE" ]; then
  echo "release 已存在，复用：$RELEASE"
else
  cp -Rp "$ARTIFACT" "$RELEASE"
  bun "$HERE/checksum.ts" "$RELEASE" >/dev/null
fi

# ---- 2) stop, switch, start ------------------------------------------------
PREVIOUS_CURRENT="$(readlink "$STATE_ROOT/server/current" 2>/dev/null || true)"
stop_server

switch_current "$RELEASE"
SWITCHED=1
write_sidecar
start_server

if ! wait_for_integrity "$SHA12"; then
  echo "启动后 integrity 未达到 ok；退出时将自动恢复旧 release" >&2
  health
  exit 1
fi

DEPLOY_OK=1
echo "部署完成：$SHA12"
echo "本次 release：$RELEASE"
echo "上一次 release：${PREVIOUS_CURRENT:-无}"
echo "回滚命令：$HERE/deploy.sh --rollback"
health
