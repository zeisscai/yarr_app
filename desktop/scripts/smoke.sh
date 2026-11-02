#!/usr/bin/env bash
#
# M2 验收冒烟：启动 → 页面/API → 崩溃自愈 → 退出回收。
# 只操作本仓库构建出来的 .app，不会碰你系统里其他 yarr 进程。
#
# 前置：先打包
#   cd desktop/shell && npx @tauri-apps/cli build --debug --bundles app
# 运行：
#   ./desktop/scripts/smoke.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP="$ROOT/desktop/shell/src-tauri/target/debug/bundle/macos/yarr.app"
LOG="$HOME/Library/Application Support/app.yarr.desktop/yarr.log"
PATTERN="$APP/Contents/MacOS"

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*" >&2; exit 1; }

[[ -d "$APP" ]] || fail "未找到 $APP，先执行：cd desktop/shell && npx @tauri-apps/cli build --debug --bundles app"

cleanup() {
  pkill -TERM -f "$PATTERN/yarr-desktop" 2>/dev/null || true
  pkill -TERM -f "$PATTERN/yarr -addr" 2>/dev/null || true
}
trap cleanup EXIT
cleanup
sleep 1

BEFORE=$(wc -l < "$LOG" 2>/dev/null || echo 0)
open -a "$APP" || fail "open 失败"

PORT=""
for _ in $(seq 1 40); do
  sleep 0.5
  NEW=$(tail -n "+$((BEFORE + 1))" "$LOG" 2>/dev/null || true)
  CAND=$(printf '%s\n' "$NEW" | grep -o '127\.0\.0\.1:[0-9]*' | tail -1 | cut -d: -f2 || true)
  if [[ -n "$CAND" ]] && curl -sf --max-time 1 "http://127.0.0.1:$CAND/api/status" >/dev/null; then
    PORT="$CAND"
    break
  fi
done
[[ -n "$PORT" ]] || fail "20s 内未就绪。日志尾部：
$(tail -6 "$LOG" 2>/dev/null)"
pass "启动就绪，端口 ${PORT}"

curl -sf --max-time 5 "http://127.0.0.1:$PORT/" | grep -q '<title>yarr' || fail "首页内容异常"
pass "首页由内嵌前端正常返回"

curl -sf --max-time 5 "http://127.0.0.1:$PORT/api/status" | grep -q '"refresh"' || fail "API 响应异常"
pass "API /api/status 可用"

SIDE_PID=$(pgrep -f "$PATTERN/yarr -addr" | head -1)
[[ -n "$SIDE_PID" ]] || fail "未找到 sidecar 进程"
kill -9 "$SIDE_PID"
RECOVERED=""
for _ in $(seq 1 30); do
  sleep 0.5
  if curl -sf --max-time 1 "http://127.0.0.1:$PORT/api/status" >/dev/null; then
    RECOVERED=1
    break
  fi
done
[[ -n "$RECOVERED" ]] || fail "sidecar 被 kill -9 后未在 15s 内恢复（日志：$(tail -4 "$LOG")）"
NEW_SIDE=$(pgrep -f "$PATTERN/yarr -addr" | head -1)
[[ "$NEW_SIDE" != "$SIDE_PID" ]] || fail "sidecar 未重启为新进程"
pass "崩溃自愈：新 pid ${NEW_SIDE}，端口不变"

APP_PID=$(pgrep -f "$PATTERN/yarr-desktop" | head -1)
kill -TERM "$APP_PID"
for _ in $(seq 1 24); do
  sleep 0.25
  pgrep -f "$PATTERN" >/dev/null || break
done
if pgrep -f "$PATTERN" >/dev/null; then
  fail "退出后仍有残留进程：
$(pgrep -fl "$PATTERN")"
fi
pass "退出回收（无残留进程）"

echo
echo "全部通过。数据目录：$HOME/Library/Application Support/app.yarr.desktop"
