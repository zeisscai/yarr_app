#!/usr/bin/env bash
#
# 构建 yarr 上游二进制，并按 Tauri externalBin 的要求重命名。
# 约定：
#   - 不复制上游的 go build 参数，直接调用上游 makefile 的 host 目标，
#     这样上游改构建标签 / ldflags 时这里会自动跟上（见 docs/DESKTOP-PLAN.md §5.6）。
#   - 产物落到 desktop/shell/binaries/yarr-<target-triple>，由 Tauri 打进 .app。
#
# 用法：
#   ./desktop/scripts/build-sidecar.sh            # 增量构建
#   YARR_REBUILD_ASSETS=1 ./desktop/scripts/build-sidecar.sh   # 强制重建前端资源
#   YARR_TRIPLE=x86_64-apple-darwin ./desktop/scripts/build-sidecar.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

TRIPLE="${YARR_TRIPLE:-$(rustc --print host-tuple 2>/dev/null || echo aarch64-apple-darwin)}"
OUT_DIR="desktop/shell/src-tauri/binaries"
OUT_BIN="$OUT_DIR/yarr-$TRIPLE"

echo "==> 仓库: $ROOT"
echo "==> 目标架构: $TRIPLE"

# 1. 前端资源（bundle.js / bundle.css 是 go:embed 的输入，缺失会导致编译失败）
if [[ ! -f src/assets/static/bundle.js || "${YARR_REBUILD_ASSETS:-0}" == "1" ]]; then
  echo "==> 构建前端资源"
  [[ -d node_modules ]] || npm ci --no-audit --no-fund
  npm run build -- --minify
fi

# 2. 调用上游 makefile 构建（含 assets 目标，幂等）
#    GOTOOLCHAIN/GOSUMDB 是为了让 go.mod 里声明的更高版本 Go 能自动下载。
echo "==> 构建 yarr 二进制（make host）"
GOTOOLCHAIN="${GOTOOLCHAIN:-auto}" GOSUMDB="${GOSUMDB:-sum.golang.org}" make host

# 3. 复制并按 target triple 命名
mkdir -p "$OUT_DIR"
cp out/yarr "$OUT_BIN"
chmod +x "$OUT_BIN"

UPSTREAM_SHA="$(git rev-parse --short=8 HEAD)"
echo "==> 完成: $OUT_BIN ($(du -h "$OUT_BIN" | cut -f1), upstream ${UPSTREAM_SHA})"
"$OUT_BIN" -version || true
