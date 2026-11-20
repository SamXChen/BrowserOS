#!/bin/zsh
# Builds the claw-server artifact, verifies it, and moves it out of `dist/`.
#
# Why the shuffle: the build tool `rm -rf`s `dist/prod/claw-server` and rebuilds it, and
# older launchers start the server straight from that directory. Building in place would
# swap the binary under a running NeoB. So: back up, build, verify, move the artifact to a
# release directory, restore dist.
#
# Usage: build-artifact.sh [--out <dir>] [--keep-dist-backup]
# Output: last line is ARTIFACT_DIR=<path>

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
AGENT="$REPO/packages/browseros-agent"
DIST="$AGENT/dist/prod/claw-server"
OUT="${NEOB_STATE_ROOT:-$HOME/Library/Application Support/NeoB}/server/staging"
KEEP_BACKUP=0

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --keep-dist-backup) KEEP_BACKUP=1; shift ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

TS="$(date +%Y%m%d-%H%M%S)"
BACKUP="${TMPDIR:-/tmp}/neob-dist-backup-$TS"
mkdir -p "$OUT"

restore() {
  local code=$?
  if [ -d "$BACKUP" ]; then
    rm -rf "$DIST"
    cp -Rp "$BACKUP" "$DIST"
    echo "dist 已恢复：$DIST" >&2
    if [ "$KEEP_BACKUP" -eq 0 ]; then rm -rf "$BACKUP"; fi
  fi
  exit $code
}
trap restore EXIT

if [ -d "$DIST" ]; then
  cp -Rp "$DIST" "$BACKUP"
  echo "备份 dist → $BACKUP"
else
  echo "dist 不存在，跳过备份"
fi

echo "== 构建"
(cd "$AGENT" && bun run build:claw-server:test) || { echo "构建失败" >&2; exit 1; }

BUILT="$DIST/darwin-arm64"
[ -f "$BUILT/artifact-metadata.json" ] || { echo "缺少 artifact-metadata.json：$BUILT" >&2; exit 1; }

echo "== 校验清单"
bun "$HERE/checksum.ts" "$BUILT"

SHA12="$(shasum -a 256 "$BUILT/resources/bin/browseros-claw-server" | cut -c1-12)"
ARTIFACT="$OUT/$SHA12"
rm -rf "$ARTIFACT"
mv "$BUILT" "$ARTIFACT"

echo "== 校验签名"
codesign -v "$ARTIFACT/resources/bin/browseros-claw-server" && echo "codesign ok"

echo "ARTIFACT_DIR=$ARTIFACT"
