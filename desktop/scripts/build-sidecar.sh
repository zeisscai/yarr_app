#!/usr/bin/env bash
#
# 构建 yarr 上游二进制，并按 Tauri externalBin 的要求重命名。
#
# 设计约定：
#   - 不复制上游的 go build 参数，一律调用上游 makefile 的目标，
#     这样上游改构建标签 / ldflags 时这里会自动跟上（见 docs/DESKTOP-PLAN.md §5.6）。
#   - 产物落到 desktop/shell/src-tauri/binaries/yarr-<target-triple>[.exe]。
#
# 用法：
#   ./desktop/scripts/build-sidecar.sh                      # 本机构建（make host）
#   YARR_REBUILD_ASSETS=1 ./desktop/scripts/build-sidecar.sh
#   YARR_MAKE_TARGET=linux_amd64 ./desktop/scripts/build-sidecar.sh
#   YARR_TRIPLE=x86_64-pc-windows-msvc YARR_MAKE_TARGET=windows_amd64 ./desktop/scripts/build-sidecar.sh
#
# 变量：
#   YARR_MAKE_TARGET  上游 makefile 目标，默认 host；其他常用：linux_amd64 / windows_amd64
#   YARR_BUILD_OUT    产物路径，默认按目标推导：host -> out/yarr，其他 -> out/<target>/yarr[.exe]
#   YARR_TRIPLE       Tauri 的目标三元组，默认 rustc --print host-tuple
#   YARR_REBUILD_ASSETS=1  强制重建前端资源
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

TRIPLE="${YARR_TRIPLE:-$(rustc --print host-tuple 2>/dev/null || echo aarch64-apple-darwin)}"
MAKE_TARGET="${YARR_MAKE_TARGET:-host}"
OUT_DIR="desktop/shell/src-tauri/binaries"

# Windows 的 externalBin 必须是 .exe，且文件名带目标三元组
EXT=""
case "$TRIPLE" in
  *windows*) EXT=".exe" ;;
esac

if [[ -n "${YARR_BUILD_OUT:-}" ]]; then
  BUILD_OUT="$YARR_BUILD_OUT"
elif [[ "$MAKE_TARGET" == "host" ]]; then
  BUILD_OUT="out/yarr$EXT"
else
  BUILD_OUT="out/${MAKE_TARGET}/yarr$EXT"
fi

OUT_BIN="$OUT_DIR/yarr-${TRIPLE}${EXT}"

echo "==> 仓库: $ROOT"
echo "==> make 目标: $MAKE_TARGET | 三元组: $TRIPLE | 产物: $OUT_BIN"

# 1. 前端资源（bundle.js / bundle.css 是 go:embed 的输入，缺失会导致编译失败）
if [[ ! -f src/assets/static/bundle.js || "${YARR_REBUILD_ASSETS:-0}" == "1" ]]; then
  echo "==> 构建前端资源"
  [[ -d node_modules ]] || npm ci --no-audit --no-fund
  npm run build -- --minify
fi

# 2. 调用上游 makefile（含 assets 目标，幂等）
#    GOTOOLCHAIN/GOSUMDB 是为了让 go.mod 里声明的更高版本 Go 能自动下载。
echo "==> 构建 yarr 二进制（make ${MAKE_TARGET}）"
GOTOOLCHAIN="${GOTOOLCHAIN:-auto}" GOSUMDB="${GOSUMDB:-sum.golang.org}" make "$MAKE_TARGET"

# 3. 复制并按 target triple 命名
[[ -f "$BUILD_OUT" ]] || { echo "!! 未找到构建产物: $BUILD_OUT" >&2; exit 1; }
mkdir -p "$OUT_DIR"
cp "$BUILD_OUT" "$OUT_BIN"
chmod +x "$OUT_BIN" 2>/dev/null || true

UPSTREAM_SHA="$(git log --format=%h -n1 --abbrev=8 -- . ':!desktop' 2>/dev/null || echo unknown)"
echo "==> 完成: $OUT_BIN ($(du -h "$OUT_BIN" | cut -f1), upstream ${UPSTREAM_SHA})"
"$OUT_BIN" -version || true
