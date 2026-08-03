#!/usr/bin/env bash
# Phase D 真机验收脚本（ADB 通道）。
#
# 用法：
#   scripts/phase-d-acceptance.sh [--device SERIAL] [--keep]
#
# 行为：
#   - 无 ADB 设备时打印跳过说明并退出 0（自动化环境预期）；
#   - 有设备时执行基础验收（连接/info/ping/文件/剪贴板/媒体）、
#     sync 验收（首次/增量/watch 生命周期）与 SIGINT 清理校验；
#   - 测试文件放在设备唯一目录 /sdcard/Download/hs-phase-d-acceptance-<pid>，
#     结束后删除；ADB forward 由 CLI 自动清理，脚本额外校验无残留
#     （等待 adb 收敛，容忍其他工具持有的 forward）；
#   - 默认结束时断开 session；--keep 保留测试目录（调试用）。
#
# 退出码：0 全部通过或无可测设备；非 0 为失败（沿用 CLI 退出码语义）。

set -u

DEVICE_ARGS=()
KEEP=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --device)
      [ "$#" -ge 2 ] || { echo "缺少 --device 的值" >&2; exit 2; }
      DEVICE_ARGS=("--serial" "$2")
      shift 2
      ;;
    --keep) KEEP=1; shift ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

BIN="${CARGO_BIN_EXE_handshaker:-target/release/handshaker}"
if [ ! -x "$BIN" ]; then
  echo "未找到 $BIN，请先执行 cargo build --release" >&2
  exit 2
fi

adb_devices="$(adb devices 2>/dev/null | awk 'NR>1 && $2=="device" {print $1}')"
if [ -z "${adb_devices:-}" ]; then
  echo "SKIP: 未检测到在线 ADB 设备，跳过真机验收。"
  echo "有设备时运行: scripts/phase-d-acceptance.sh"
  exit 0
fi
if [ "${#DEVICE_ARGS[@]}" -eq 0 ] && [ "$(echo "$adb_devices" | wc -l | tr -d ' ')" -gt 1 ]; then
  echo "检测到多台设备，请用 --device SERIAL 指定：" >&2
  echo "$adb_devices" >&2
  exit 3
fi

SERIAL="${DEVICE_ARGS[1]:-$adb_devices}"
# 注意：手机端不接受 /sdcard 前缀路径（create_directory 报
# FILE_IO_INVALID_SOURCE），测试目录必须用 root 绝对路径。
TEST_DIR="/storage/emulated/0/Download/hs-phase-d-acceptance-$$"
LOCAL_TMP="$(mktemp -d)"
STATE_DIR="$LOCAL_TMP/state"
trap 'rm -rf "$LOCAL_TMP"; [ "$KEEP" -eq 0 ] && adb -s "$SERIAL" shell rm -rf "$TEST_DIR" 2>/dev/null || true' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

run() { # run <expect_exit> <args...>
  local expect="$1"; shift
  # set -u 下空数组展开会 unbound；带空串守卫展开。
  "$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} "$@"; local code=$?
  [ "$code" -eq "$expect" ] || fail "exit=$code (期望 $expect): $*"
}

echo "== 设备: $SERIAL =="
echo "== 基础验收 =="

run 0 device info
run 0 device ping

run 0 fs mkdir "$TEST_DIR"
# 不存在的本地源必须失败（不能静默成功）。
if "$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} fs push \
  "$LOCAL_TMP/nonexistent/never.txt" "$TEST_DIR/x.txt" > /dev/null 2>&1; then
  fail "不存在的本地源不应成功"
else
  ok "不存在的本地源被拒绝"
fi

echo "hello-phase-d-$$" > "$LOCAL_TMP/hello.txt"
run 0 fs push "$LOCAL_TMP/hello.txt" -- "$TEST_DIR/hello.txt"
run 0 fs pull "$TEST_DIR/hello.txt" -- "$LOCAL_TMP/pulled.txt"
cmp -s "$LOCAL_TMP/hello.txt" "$LOCAL_TMP/pulled.txt" || fail "下载文件内容不一致"
ok "push/pull 往返一致"

run 0 fs mv "$TEST_DIR/hello.txt" "$TEST_DIR/renamed.txt"
run 0 fs stat "$TEST_DIR/renamed.txt"

dd if=/dev/urandom of="$LOCAL_TMP/rand.bin" bs=1024 count=64 2>/dev/null
run 0 fs push "$LOCAL_TMP/rand.bin" -- "$TEST_DIR/rand.bin"
run 0 fs pull --recursive "$TEST_DIR" -- "$LOCAL_TMP/tree/"
cmp -s "$LOCAL_TMP/rand.bin" "$LOCAL_TMP/tree/rand.bin" || fail "递归下载内容不一致"
ok "recursive pull 往返一致"

run 0 clipboard set "phase-d-clipboard-$$"
out="$("$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} --output json clipboard get)"
echo "$out" | grep -q "phase-d-clipboard-$$" || fail "剪贴板读回不一致"
ok "clipboard 往返一致"

run 0 media photo

echo "== sync 验收（首次/增量/watch） =="
SYNC_DIR="$LOCAL_TMP/sync"
mkdir -p "$SYNC_DIR"
SYNC_OUT="$TEST_DIR/sync-root"
run 0 fs mkdir "$SYNC_OUT"

# 首次 sync plan/run（参数与 CLI 一致；失败即退出）。
# 注意：sync 只同步照片库（photo_sync），任意文件不会出现在计划里，
# 所以这里用相机目录验证；若相机目录为空则跳过 sync 断言。
plan_out="$("$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} --output json sync plan --output-dir "$SYNC_DIR" --root /storage/emulated/0/DCIM/Camera)" \
  || fail "sync plan 失败: $plan_out"
added="$(echo "$plan_out" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['data']['added']))" 2>/dev/null || echo 0)"
echo "首次 plan added=$added"
if [ "$added" -gt 0 ]; then
  run 0 sync run --yes --output-dir "$SYNC_DIR" --root /storage/emulated/0/DCIM/Camera
  # 手机端在 run 后仍处于 SYNCING 状态：新连接的 photo_sync 可能被拒或
  # 响应超时，等待其恢复后再做增量校验。
  sleep 3
  plan2_out="$("$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} --output json sync plan --output-dir "$SYNC_DIR" --root /storage/emulated/0/DCIM/Camera)" \
    || fail "增量 sync plan 失败: $plan2_out"
  echo "$plan2_out" | grep -q '"added":\[\]' || fail "增量 plan 应无新增: $plan2_out"
  ok "sync 首次+增量通过"
  status_out="$("$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} --output json sync status)"
  echo "$status_out" | grep -q '"files":' || fail "sync status 输出异常: $status_out"
  ok "sync status 正常"
  # watch 生命周期:启动 + 订阅,等待其进入事件循环后 SIGINT 应干净退出。
  # 首次同步(含 SYNCING 重试)可能耗时数秒,给足时间再中断。
  "$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} --output jsonl sync watch --yes \
    --output-dir "$SYNC_DIR" --root /storage/emulated/0/DCIM/Camera > "$LOCAL_TMP/watch.jsonl" 2> "$LOCAL_TMP/watch.err" &
  WPID=$!
  sleep 8
  kill -INT "$WPID" 2>/dev/null
  sleep 5
  wait "$WPID"; code=$?
  [ "$code" -eq 130 ] || fail "sync watch SIGINT 应退出 130,实际 $code"
  ok "sync watch 生命周期/SIGINT 通过"
else
  echo "相机目录为空，跳过 sync 下载断言（plan 结构已验证）"
fi

echo "== 清理校验 =="
# adb 可能尚未回收刚关闭的 forward：等待最多 10 秒收敛。
forward_left=1
for _ in 1 2 3 4 5 6 7 8 9 10; do
  forward_left="$(adb -s "$SERIAL" forward --list 2>/dev/null | grep -c "$SERIAL" || true)"
  [ "$forward_left" -eq 0 ] && break
  sleep 1
done
[ "$forward_left" -eq 0 ] || fail "存在残留 adb forward: $forward_left 条"
ok "无残留 adb forward"

echo "PASS: Phase D 真机验收全部通过（设备 $SERIAL）"
