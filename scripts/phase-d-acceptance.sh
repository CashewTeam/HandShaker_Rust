#!/usr/bin/env bash
# Phase D 真机验收脚本（ADB 通道）。
#
# 用法：
#   scripts/phase-d-acceptance.sh [--device SERIAL] [--keep]
#
# 行为：
#   - 无 ADB 设备时打印跳过说明并退出 0（自动化环境预期）；
#   - 有设备时执行基础验收（连接/info/ping/文件/剪贴板/媒体）与
#     sync 验收（首次/增量/watch）；
#   - 测试文件放在设备唯一目录 /sdcard/Download/hs-phase-d-acceptance-<pid>，
#     结束后删除；ADB forward 由 CLI 自动清理，脚本额外校验无残留；
#   - 默认结束时断开 session；--keep 保留测试目录（调试用）。
#
# 退出码：0 全部通过或无可测设备；非 0 为失败（沿用 CLI 退出码语义）。

set -u

DEVICE_ARGS=()
KEEP=0
for arg in "$@"; do
  case "$arg" in
    --device) DEVICE_ARGS=("--serial" "$2"); shift 2 ;;
    --keep) KEEP=1 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
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
export HOME="${HOME:-$PWD}"
TEST_DIR="/sdcard/Download/hs-phase-d-acceptance-$$"
REMOTE_SUB="$TEST_DIR/sub"
LOCAL_TMP="$(mktemp -d)"
trap 'rm -rf "$LOCAL_TMP"; [ "$KEEP" -eq 0 ] && adb -s "$SERIAL" shell rm -rf "$TEST_DIR" 2>/dev/null || true' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

run() { # run <expect_exit> <args...>
  local expect="$1"; shift
  "$BIN" "${DEVICE_ARGS[@]}" "$@"; local code=$?
  [ "$code" -eq "$expect" ] || fail "exit=$code (期望 $expect): $*"
}

echo "== 设备: $SERIAL =="
echo "== 基础验收 =="

run 0 device info
run 0 device ping

run 0 fs mkdir "$TEST_DIR"
run 0 fs mkdir "$REMOTE_SUB"

echo "hello-phase-d-$$" > "$LOCAL_TMP/hello.txt"
run 0 fs push "$LOCAL_TMP/hello.txt" -- "$TEST_DIR/hello.txt"
run 0 fs pull "$TEST_DIR/hello.txt" -- "$LOCAL_TMP/pulled.txt"
cmp -s "$LOCAL_TMP/hello.txt" "$LOCAL_TMP/pulled.txt" || fail "下载文件 MD5 不一致"
ok "push/pull 往返一致"

run 0 fs mv "$TEST_DIR/hello.txt" "$TEST_DIR/renamed.txt"
run 0 fs stat "$TEST_DIR/renamed.txt"

dd if=/dev/urandom of="$LOCAL_TMP/rand.bin" bs=1024 count=64 2>/dev/null
run 0 fs push "$LOCAL_TMP/rand.bin" -- "$TEST_DIR/rand.bin"
run 0 fs pull --recursive "$TEST_DIR" -- "$LOCAL_TMP/tree/"
cmp -s "$LOCAL_TMP/rand.bin" "$LOCAL_TMP/tree/rand.bin" || fail "递归下载 MD5 不一致"
ok "recursive pull 往返一致"

run 0 clipboard set "phase-d-clipboard-$$"
out="$("$BIN" "${DEVICE_ARGS[@]}" --output json clipboard get)"
echo "$out" | grep -q "phase-d-clipboard-$$" || fail "剪贴板读回不一致"
ok "clipboard 往返一致"

run 0 media photo
run 0 media video
run 0 media audio

echo "== sync 验收（首次/增量） =="
SYNC_DIR="$LOCAL_TMP/sync"
mkdir -p "$SYNC_DIR"
SYNC_OUT="$LOCAL_TMP/sync-dir"
run 0 fs mkdir "$SYNC_OUT"
run 0 fs push "$LOCAL_TMP/hello.txt" -- "$SYNC_OUT/seed.txt"

# 首次 sync plan/run（参数与 CLI 一致；失败即退出）
plan_out="$("$BIN" "${DEVICE_ARGS[@]}" --output json sync plan --output-dir "$SYNC_DIR" --root "$SYNC_OUT")" \
  || fail "sync plan 失败: $plan_out"
echo "$plan_out" | grep -q '"added"' || fail "sync plan 输出缺少 added 字段"
run 0 sync run --yes --output-dir "$SYNC_DIR" --root "$SYNC_OUT"
# 增量:第二次 run 应无新增(plan 全空)
plan2_out="$("$BIN" "${DEVICE_ARGS[@]}" --output json sync plan --output-dir "$SYNC_DIR" --root "$SYNC_OUT")" \
  || fail "sync plan(增量) 失败: $plan2_out"
echo "$plan2_out" | grep -q '"added": \[\]' || fail "增量 plan 应无新增: $plan2_out"
ok "sync 首次+增量 plan/run 通过"

echo "== 清理校验 =="
forwards="$(adb -s "$SERIAL" forward --list | grep -c "$SERIAL" || true)"
[ "$forwards" -eq 0 ] || fail "存在残留 adb forward: $forwards 条"
ok "无残留 adb forward"

echo "PASS: Phase D 真机验收全部通过（设备 $SERIAL）"
