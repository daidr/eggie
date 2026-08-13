#!/usr/bin/env bash
# 把 Eggie.app 打成更新包 zip,并计算 sha256。
# 产物: target/app/Eggie-<version>.zip
#
# 用法:
#   scripts/make-update-zip.sh [path/to/Eggie.app]
#   不传参数时使用 target/app/Eggie.app(即 make-app.sh 的产物)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$ROOT/target/app/Eggie.app}"

if [[ ! -d "$APP" ]]; then
    echo "错误: 找不到 $APP" >&2
    echo "      先运行 scripts/make-app.sh,或显式传入 .app 路径。" >&2
    exit 1
fi

VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
OUT="$ROOT/target/app/Eggie-$VERSION.zip"

echo "==> 打包 $APP -> $OUT"
# ditto 保留 symlink 和 unix 权限。--norsrc/--noextattr/--noqtn 抑制 AppleDouble
# `._*` 伴随文件——它们会落在 bundle 根,让 codesign 报 "unsealed contents
# present in the bundle root" 而拒签。
ditto -c -k --norsrc --noextattr --noqtn --keepParent "$APP" "$OUT"

SHA=$(shasum -a 256 "$OUT" | awk '{print $1}')
echo "==> 完成"
echo "    版本:   $VERSION"
echo "    sha256: $SHA"
echo
echo "把下面内容写入 ~/Library/Application Support/Eggie/dev/update-feed.json 即可模拟更新:"
cat <<EOF
{
  "version": "$VERSION",
  "protocol_version": 1,
  "release_notes": "- 在此填写更新内容",
  "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "download_url": "file://$OUT",
  "sha256": "$SHA"
}
EOF
