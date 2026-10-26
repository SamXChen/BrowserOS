#!/usr/bin/env bash
# 由官方 BrowserOS neo.app 生成 NeoB.app
#   改显示名 + 换图标 + 关自动更新 + 内嵌启动脚本 + 保留 entitlements 重签
# 官方版本升级后重跑本脚本即可重建。
set -euo pipefail

cd "$(dirname "$0")"

APP_NAME="NeoB"
LAUNCHER="NeoBLauncher"
SRC="/Applications/BrowserOS neo.app"
APP="$HOME/Applications/$APP_NAME.app"
ICON="Resources/AppIcon.icns"

if [ ! -d "$SRC" ]; then
  echo "未找到官方内核：$SRC" >&2
  exit 1
fi
if [ ! -f "$ICON" ]; then
  echo "未找到图标：$ICON（先运行 make-icon.sh）" >&2
  exit 1
fi

echo "==> 复制官方内核（$(du -sh "$SRC" | cut -f1)）"
rm -rf "$APP"
cp -R "$SRC" "$APP"

PB=/usr/libexec/PlistBuddy
echo "==> 改显示名 / 关自动更新"
$PB -c "Set :CFBundleName $APP_NAME" "$APP/Contents/Info.plist" 2>/dev/null \
  || $PB -c "Add :CFBundleName string $APP_NAME" "$APP/Contents/Info.plist"
$PB -c "Set :CFBundleDisplayName $APP_NAME" "$APP/Contents/Info.plist" 2>/dev/null \
  || $PB -c "Add :CFBundleDisplayName string $APP_NAME" "$APP/Contents/Info.plist"
# 关闭 Sparkle 自动更新：否则官方更新会覆盖改名与图标
$PB -c "Set :SUEnableAutomaticChecks false" "$APP/Contents/Info.plist" 2>/dev/null \
  || $PB -c "Add :SUEnableAutomaticChecks bool false" "$APP/Contents/Info.plist"

echo "==> 换图标"
cp "$ICON" "$APP/Contents/Resources/app.icns"
# 官方 Info.plist 里有 CFBundleIconName=AppIcon，它让 macOS 优先读
# Resources/Assets.car，只换 app.icns 完全不生效。删掉这个键才会回退到
# CFBundleIconFile=app.icns（已用 NSWorkspace 解析验证）。
$PB -c "Delete :CFBundleIconName" "$APP/Contents/Info.plist" 2>/dev/null || true

# chrome:// 内置页（下载记录、版本页等）的 logo 编译在 chrome_*_percent.pak 里，
# 光换 Contents/Resources/app.icns 不会改变它们。patch-internal-logo.py 会把同一份
# AppIcon.icns 生成 32px PNG，同时覆盖散文件和 pak 内的同尺寸条目。
echo "==> 换内置页 logo（product_logo_32.png + chrome_*_percent.pak）"
python3 patch-internal-logo.py "$APP" "$ICON" || echo "    内置页 logo 未替换（不影响打包）" >&2

echo "==> 内嵌启动脚本（设为 CFBundleExecutable，双击即注入参数）"
cp "src/$APP_NAME" "$APP/Contents/MacOS/$LAUNCHER"
chmod +x "$APP/Contents/MacOS/$LAUNCHER"
cp "config.env.template" "$APP/Contents/Resources/config.env"
$PB -c "Set :CFBundleExecutable $LAUNCHER" "$APP/Contents/Info.plist"

# resource fork / Finder 信息会让 codesign 直接拒绝
xattr -cr "$APP" 2>/dev/null || true

echo "==> 重签（由内到外）"
# 关键：只 --preserve-metadata=entitlements。
# 加上 requirements 会保留官方的 designated requirement（含 Apple Team ID），
# 与 ad-hoc 签名冲突，启动时报
#   "mapping process and mapped file (non-platform) have different Team IDs"
# 导致 dlopen framework 失败、进程直接退出。
find "$APP" -depth \( -name "*.framework" -o -name "*.app" -o -name "*.xpc" \
  -o \( -type f -perm +111 \) \) -print0 \
  | xargs -0 -I{} codesign --force --sign - --preserve-metadata=entitlements --timestamp=none "{}" >/dev/null 2>&1
codesign --force --sign - --preserve-metadata=entitlements --timestamp=none "$APP"

echo "==> 校验"
codesign --verify --deep --verbose=2 "$APP" 2>&1 | tail -3

echo "==> 完成：$APP"
