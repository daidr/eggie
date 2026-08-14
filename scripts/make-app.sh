#!/usr/bin/env bash
# 把 eggie 二进制打包成 Eggie.app(含应用图标)。
#
# 前置:先用 Icon Composer 打开 images/EggieIcon.icon,
# Export 为 images/AppIcon.icns(⌘E,格式选 .icns)。
#
# 用法:
#   scripts/make-app.sh            # release 打包
#   scripts/make-app.sh --debug    # debug 打包
#
# 环境变量(CI 注入,本地开发全部可省略):
#   EGGIE_VERSION       写入 CFBundleShortVersionString(默认沿用 Info.plist 里的值)
#   EGGIE_BUILD_NUMBER  写入 CFBundleVersion,须单调递增(默认沿用 Info.plist 里的值)
#   EGGIE_SIGN_IDENTITY 代码签名身份(默认 "-" ad-hoc;发布传 Developer ID)
#                       —— 非 ad-hoc 时自动启用 hardened runtime + secure timestamp
#                          (公证前提;经验证 Eggie 不需要任何 entitlements)
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
SIGN_IDENTITY="${EGGIE_SIGN_IDENTITY:--}"

echo "==> cargo build ($PROFILE)"
cargo build -p eggie-ui -p eggie-updater $( [[ "$PROFILE" == "release" ]] && echo "--release" )

echo "==> 组装 $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"

cp "$BINARY" "$BUNDLE/Contents/MacOS/eggie"
cp "$CARGO_TARGET_DIR/$PROFILE/eggie-updater" "$BUNDLE/Contents/MacOS/eggie-updater"
cp "$ROOT/packaging/Info.plist" "$BUNDLE/Contents/Info.plist"

PLIST="$BUNDLE/Contents/Info.plist"
if [[ -n "${EGGIE_VERSION:-}" ]]; then
    echo "==> 注入版本号 CFBundleShortVersionString=$EGGIE_VERSION"
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $EGGIE_VERSION" "$PLIST"
fi
if [[ -n "${EGGIE_BUILD_NUMBER:-}" ]]; then
    echo "==> 注入构建号 CFBundleVersion=$EGGIE_BUILD_NUMBER"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $EGGIE_BUILD_NUMBER" "$PLIST"
fi

if [[ -f "$ICNS_SRC" ]]; then
    cp "$ICNS_SRC" "$BUNDLE/Contents/Resources/AppIcon.icns"
else
    echo "警告: $ICNS_SRC 不存在,将使用通用图标。" >&2
    echo "      请在 Icon Composer 中导出 images/EggieIcon.icon 为 .icns。" >&2
fi

echo "==> 代码签名 (identity: $SIGN_IDENTITY)"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
    # 本地 ad-hoc 签名:避免 Gatekeeper 提示,并让 LaunchServices 登记图标。
    codesign --force --deep --sign - "$BUNDLE" 2>/dev/null || true
else
    # 发布签名:Developer ID + hardened runtime + secure timestamp(公证前提)。
    # 先签内嵌的辅助二进制,再签外层 bundle——--deep 会处理,但显式先签更稳妥。
    codesign --force --options runtime --timestamp \
        --sign "$SIGN_IDENTITY" \
        "$BUNDLE/Contents/MacOS/eggie-updater"
    codesign --force --deep --options runtime --timestamp \
        --sign "$SIGN_IDENTITY" \
        "$BUNDLE"
    echo "==> 验证签名"
    codesign --verify --strict --deep --verbose=2 "$BUNDLE"
fi

# 强制 LaunchServices 刷新图标缓存(本地开发用;CI 无 GUI 会静默失败,无害)。
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$BUNDLE" 2>/dev/null || true

echo "==> 完成: $BUNDLE"
echo "    运行: open \"$BUNDLE\""
