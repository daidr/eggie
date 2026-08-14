#!/usr/bin/env bash
# 从 GitHub Releases 全量重建两个渠道的 feed 文档,输出到 <out-dir>/
# {stable,beta}.json,供 GitHub Pages 托管。
#
# 用法:
#   scripts/build-feed.sh <out-dir>
#
# 前置:仓库里每个 release 都挂了一个 manifest.json 资产(由 release
# workflow 的 build job 生成),内容是单个 ReleaseInfo:
#   { version, protocol_version, release_notes, published_at,
#     download_url, sha256 }
#
# 为什么要 manifest 资产:feed 的 protocol_version / sha256 无法从
# GitHub /releases API 得到,只能由每个 release 自带的 manifest 携带。
# 这样 Pages-from-Actions(部署构建产物而非 git 文件)也能每次全量重建。
#
# 渠道划分:
#   stable.json —— 仅正式版(release 未标记 prerelease)
#   beta.json   —— 全部(含正式版,因为 beta 用户也应收到正式版)
set -euo pipefail

OUT_DIR="${1:?用法: build-feed.sh <out-dir>}"
REPO="${GITHUB_REPOSITORY:-daidr/eggie}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$OUT_DIR"

echo "==> 枚举 $REPO 的所有 release"
# 拉全部 release 的 tag + prerelease 标志(JSON)。
gh release list --repo "$REPO" --limit 200 \
    --json tagName,isPrerelease,isDraft > "$WORK/releases.json"

# 逐个 release 下载 manifest.json,拼装成 {stable,beta} 两个数组。
: > "$WORK/stable-items.ndjson"
: > "$WORK/beta-items.ndjson"

count=$(jq 'length' "$WORK/releases.json")
echo "==> 共 $count 个 release"

for i in $(seq 0 $((count - 1))); do
    tag=$(jq -r ".[$i].tagName" "$WORK/releases.json")
    is_pre=$(jq -r ".[$i].isPrerelease" "$WORK/releases.json")
    is_draft=$(jq -r ".[$i].isDraft" "$WORK/releases.json")

    # 跳过草稿。
    if [[ "$is_draft" == "true" ]]; then
        echo "   跳过草稿 $tag"
        continue
    fi

    # 下载该 release 的 manifest.json;缺失则跳过(老版本可能没有)。
    if ! gh release download "$tag" --repo "$REPO" \
        --pattern "manifest.json" --dir "$WORK/$tag" 2>/dev/null; then
        echo "   ⚠️  $tag 无 manifest.json,跳过"
        continue
    fi

    manifest="$WORK/$tag/manifest.json"
    # 校验是合法 JSON 且含必需字段。
    if ! jq -e '.version and .protocol_version and .download_url and .sha256' \
        "$manifest" >/dev/null 2>&1; then
        echo "   ⚠️  $tag 的 manifest.json 字段不全,跳过"
        continue
    fi

    # beta 收所有;stable 只收正式版。
    cat "$manifest" >> "$WORK/beta-items.ndjson"
    if [[ "$is_pre" != "true" ]]; then
        cat "$manifest" >> "$WORK/stable-items.ndjson"
    fi
    echo "   ✓ $tag (prerelease=$is_pre)"
done

# 把 ndjson 收拢成 { "releases": [...] }。
jq -s '{releases: .}' "$WORK/stable-items.ndjson" > "$OUT_DIR/stable.json"
jq -s '{releases: .}' "$WORK/beta-items.ndjson" > "$OUT_DIR/beta.json"

echo "==> 生成完成"
echo "    stable.json: $(jq '.releases | length' "$OUT_DIR/stable.json") 个版本"
echo "    beta.json:   $(jq '.releases | length' "$OUT_DIR/beta.json") 个版本"
