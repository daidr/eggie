#!/usr/bin/env bash
# 把 Eggie.app 打成首次安装用的 .dmg(带 /Applications 拖拽软链接)。
# 产物: target/app/Eggie-<version>.dmg
#
# 用法:
#   scripts/make-dmg.sh [path/to/Eggie.app]
#   不传参数时使用 target/app/Eggie.app(即 make-app.sh 的产物)。
#
# 注意:dmg 的公证与 staple 由 release workflow 单独完成——ticket 钉不进
# 裸 zip,但能钉进 .app 和 .dmg。本脚本只负责造盘。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$ROOT/target/app/Eggie.app}"

if [[ ! -d "$APP" ]]; then
    echo "错误: 找不到 $APP" >&2
    echo "      先运行 scripts/make-app.sh,或显式传入 .app 路径。" >&2
    exit 1
fi

VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
OUT="$ROOT/target/app/Eggie-$VERSION.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "==> 组装 dmg 内容 -> $STAGE"
# ditto 保留签名封印(相比 cp -R 更稳妥)。
ditto "$APP" "$STAGE/Eggie.app"
# 拖拽安装的 /Applications 软链接。
ln -s /Applications "$STAGE/Applications"

echo "==> 造盘 $OUT"
rm -f "$OUT"
# UDZO = zlib 压缩只读盘;-volname 决定挂载后显示的卷名。
hdiutil create \
    -volname "Eggie" \
    -srcfolder "$STAGE" \
    -fs HFS+ \
    -format UDZO \
    -ov \
    "$OUT" >/dev/null

SHA=$(shasum -a 256 "$OUT" | awk '{print $1}')
echo "==> 完成"
echo "    版本:   $VERSION"
echo "    产物:   $OUT"
echo "    sha256: $SHA"
