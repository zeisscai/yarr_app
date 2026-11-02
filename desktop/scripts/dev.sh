#!/usr/bin/env bash
#
# 开发循环：先重建 sidecar（Go 侧改动只需重跑这一步），再起 Tauri dev。
# 注意：tauri dev 会把 binaries/yarr-<triple> 复制到 target/debug/yarr 供壳加载。
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

"$ROOT/desktop/scripts/build-sidecar.sh"

cd "$ROOT/desktop/shell"
exec npx --yes @tauri-apps/cli@latest dev
