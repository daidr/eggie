#!/usr/bin/env bash
# 把 eggie 二进制打包成 Eggie.app(含应用图标)。
#
# 前置:先用 Icon Composer 打开 images/EggieIcon.icon,
# Export 为 images/AppIcon.icns(⌘E,格式选 .icns)。
#
# 用法:
#   scripts/make-app.sh            # release 打包
#   scripts/make-app.sh --debug    # debug 打包
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Eggie"
PROFILE="release"

if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
fi

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BINARY="$CARGO_TARGET_DIR/$PROFILE/eggie"
BUNDLE="$CARGO_TARGET_DIR/app/$APP_NAME.app"
ICNS_SRC="$ROOT/images/AppIcon.icns"

echo "==> cargo build ($PROFILE)"
cargo build -p eggie-ui -p eggie-updater $( [[ "$PROFILE" == "release" ]] && echo "--release" )

echo "==> 组装 $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"

cp "$BINARY" "$BUNDLE/Contents/MacOS/eggie"
cp "$CARGO_TARGET_DIR/$PROFILE/eggie-updater" "$BUNDLE/Contents/MacOS/eggie-updater"
cp "$ROOT/packaging/Info.plist" "$BUNDLE/Contents/Info.plist"

if [[ -f "$ICNS_SRC" ]]; then
    cp "$ICNS_SRC" "$BUNDLE/Contents/Resources/AppIcon.icns"
else
    echo "警告: $ICNS_SRC 不存在,将使用通用图标。" >&2
    echo "      请在 Icon Composer 中导出 images/EggieIcon.icon 为 .icns。" >&2
fi

# 本地 ad-hoc 签名,避免 Gatekeeper 提示,并让 LaunchServices 正确登记图标。
codesign --force --deep --sign - "$BUNDLE" 2>/dev/null || true

# 强制 LaunchServices 刷新图标缓存。
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$BUNDLE" 2>/dev/null || true

echo "==> 完成: $BUNDLE"
echo "    运行: open \"$BUNDLE\""
