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

run_batch_file() { # run_batch_file <input> <output> <stderr>
  local input="$1" output="$2" error_output="$3"
  "$BIN" --state-dir "$STATE_DIR" ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} \
    --output jsonl batch <"$input" >"$output" 2>"$error_output"
  local code=$?
  if [ "$code" -ne 0 ]; then
    sed -n '1,80p' "$error_output" >&2
    fail "batch exit=$code"
  fi
}

validate_batch_output() { # validate_batch_output <output> <clipboard-marker> <commands...>
  local output="$1" marker="$2"
  shift 2
  python3 - "$output" "$marker" "$@" <<'PY'
import json
import sys

path, marker, *expected = sys.argv[1:]
objects = []
with open(path, encoding="utf-8") as handle:
    for line in handle:
        if line.strip():
            objects.append(json.loads(line))

actual = [item.get("command") for item in objects]
if actual != expected:
    raise SystemExit(f"batch command sequence mismatch: {actual!r} != {expected!r}")
if any(not item.get("ok") for item in objects):
    raise SystemExit(f"batch command reported failure: {objects!r}")

def contains(value, needle):
    if isinstance(value, str):
        return value == needle
    if isinstance(value, dict):
        return any(contains(child, needle) for child in value.values())
    if isinstance(value, list):
        return any(contains(child, needle) for child in value)
    return False

push_pull_index = 0
added = None
for item in objects:
    command = item["command"]
    data = item.get("data")
    if command in ("fs.push", "fs.pull"):
        failures = data.get("failures") if isinstance(data, dict) else None
        expected_failures = 1 if command == "fs.push" and push_pull_index == 0 else 0
        if not isinstance(failures, list) or len(failures) != expected_failures:
            raise SystemExit(
                f"{command} failure count mismatch: {failures!r} != {expected_failures}"
            )
        push_pull_index += 1
    elif command == "clipboard.get" and not contains(data, marker):
        raise SystemExit("clipboard.get did not return the test marker")
    elif command == "sync.plan":
        if not isinstance(data, dict) or not isinstance(data.get("added"), list):
            raise SystemExit(f"sync.plan has no added list: {data!r}")
        added = len(data["added"])

if "sync.plan" in expected and added is None:
    raise SystemExit("batch output did not contain sync.plan")
if added is not None:
    print(f"added={added}")
PY
}

echo "== 设备: $SERIAL =="
echo "== 基础验收（单连接 batch） =="
echo "hello-phase-d-$$" > "$LOCAL_TMP/hello.txt"
dd if=/dev/urandom of="$LOCAL_TMP/rand.bin" bs=1024 count=64 2>/dev/null
SYNC_DIR="$LOCAL_TMP/sync"
mkdir -p "$SYNC_DIR"
SYNC_OUT="$TEST_DIR/sync-root"

BATCH_INPUT="$LOCAL_TMP/base.batch"
BATCH_OUTPUT="$LOCAL_TMP/base.jsonl"
BATCH_ERROR="$LOCAL_TMP/base.err"
MARKER="phase-d-clipboard-$$"
printf '%s\n' \
  "device info" \
  "device ping" \
  "fs mkdir $TEST_DIR" \
  "fs push $LOCAL_TMP/nonexistent/never.txt -- $TEST_DIR/x.txt" \
  "fs push $LOCAL_TMP/hello.txt -- $TEST_DIR/hello.txt" \
  "fs pull $TEST_DIR/hello.txt -- $LOCAL_TMP/pulled.txt" \
  "fs mv $TEST_DIR/hello.txt $TEST_DIR/renamed.txt" \
  "fs stat $TEST_DIR/renamed.txt" \
  "fs push $LOCAL_TMP/rand.bin -- $TEST_DIR/rand.bin" \
  "fs pull --recursive $TEST_DIR -- $LOCAL_TMP/tree/" \
  "clipboard set $MARKER" \
  "clipboard get" \
  "media photo" \
  "fs mkdir $SYNC_OUT" \
  "sync plan --output-dir $SYNC_DIR --root /storage/emulated/0/DCIM/Camera" \
  "exit" > "$BATCH_INPUT"
run_batch_file "$BATCH_INPUT" "$BATCH_OUTPUT" "$BATCH_ERROR"
batch_summary="$(validate_batch_output "$BATCH_OUTPUT" "$MARKER" \
  device.info device.ping fs.mkdir fs.push fs.push fs.pull fs.mv fs.stat \
  fs.push fs.pull clipboard.set clipboard.get media.photo fs.mkdir sync.plan)" \
  || fail "batch 验收失败"
added="${batch_summary#added=}"
echo "首次 plan added=$added"
cmp -s "$LOCAL_TMP/hello.txt" "$LOCAL_TMP/pulled.txt" || fail "下载文件内容不一致"
ok "push/pull 往返一致"
cmp -s "$LOCAL_TMP/rand.bin" "$LOCAL_TMP/tree/rand.bin" || fail "递归下载内容不一致"
ok "recursive pull 往返一致"
ok "clipboard 往返一致"
ok "media photo 正常"

echo "== sync 验收（首次/增量/watch） =="
if [ "$added" -gt 0 ]; then
  SYNC_RUN_INPUT="$LOCAL_TMP/sync-run.batch"
  SYNC_RUN_OUTPUT="$LOCAL_TMP/sync-run.jsonl"
  SYNC_RUN_ERROR="$LOCAL_TMP/sync-run.err"
  printf '%s\n' \
    "sync run --yes --output-dir $SYNC_DIR --root /storage/emulated/0/DCIM/Camera" \
    "exit" > "$SYNC_RUN_INPUT"
  run_batch_file "$SYNC_RUN_INPUT" "$SYNC_RUN_OUTPUT" "$SYNC_RUN_ERROR"
  validate_batch_output "$SYNC_RUN_OUTPUT" "$MARKER" sync.run \
    || fail "sync run batch 验收失败"
  # 手机端在 run 后仍处于 SYNCING 状态：新连接的 photo_sync 可能被拒或
  # 响应超时，等待其恢复后再做增量校验。
  sleep 3
  SYNC_VERIFY_INPUT="$LOCAL_TMP/sync-verify.batch"
  SYNC_VERIFY_OUTPUT="$LOCAL_TMP/sync-verify.jsonl"
  SYNC_VERIFY_ERROR="$LOCAL_TMP/sync-verify.err"
  printf '%s\n' \
    "sync plan --output-dir $SYNC_DIR --root /storage/emulated/0/DCIM/Camera" \
    "sync status" \
    "exit" > "$SYNC_VERIFY_INPUT"
  run_batch_file "$SYNC_VERIFY_INPUT" "$SYNC_VERIFY_OUTPUT" "$SYNC_VERIFY_ERROR"
  verify_summary="$(validate_batch_output "$SYNC_VERIFY_OUTPUT" "$MARKER" sync.plan sync.status)" \
    || fail "增量 sync batch 验收失败"
  verify_added="${verify_summary#added=}"
  [ "$verify_added" -eq 0 ] || fail "增量 plan 应无新增，实际 $verify_added"
  ok "sync 首次+增量通过"
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
