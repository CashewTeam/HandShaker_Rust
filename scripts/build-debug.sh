#!/usr/bin/env bash
#
# build-debug.sh — 构建 debug profile 并自动清理旧版本编译缓存。
#
# 行为：
#   1. 直接复用上一版缓存执行 `cargo build --workspace`（增量编译，构建前不清理）；
#   2. 构建成功后，对每个 workspace crate 只保留最新 KEEP 个版本的缓存
#      （target/debug/deps 的 lib/测试二进制 + target/debug/incremental），
#      删除其余旧版本，避免 target/debug 无限膨胀。
#
# 设计约束：
#   - 只清理本 workspace crate（handshaker_core / handshaker_application /
#     handshaker_cli / handshaker_ffi 及历史遗留名 handshaker_rust），
#     绝不删除第三方依赖缓存；
#   - 构建失败时不执行清理，保留上一版缓存以便下次增量编译；
#   - 尊重 CARGO_TARGET_DIR 覆盖目标目录；
#   - 支持 --dry-run 预览将要删除的文件；
#   - 兼容 macOS 自带 bash 3.2 与 Linux bash。
#
# 环境变量：
#   HS_SKIP_BUILD=1  跳过 cargo build，仅执行缓存清理（供测试/复用清理逻辑）。
#
# 用法：
#   scripts/build-debug.sh [--dry-run] [--keep=N] [cargo 附加参数...]
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DRY_RUN=0
KEEP=1
CARGO_ARGS=()

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --keep=*) KEEP="${arg#--keep=}" ;;
    -h|--help)
      awk 'NR > 1 { if ($0 ~ /^#/) { sub(/^# ?/, ""); print } else { exit } }' "$0"
      exit 0
      ;;
    *) CARGO_ARGS+=("$arg") ;;
  esac
done

# rustup 安装的 cargo 不在默认 PATH 时，加载 ~/.cargo/env
if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null 2>&1 || { echo "build-debug: 找不到 cargo（尝试 . \$HOME/.cargo/env 后仍失败）" >&2; exit 1; }

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
DEPS_DIR="$TARGET_DIR/debug/deps"
INCR_DIR="$TARGET_DIR/debug/incremental"

# workspace crate 在 deps/ 中的文件名前缀（lib 产物 + 测试二进制）。
# handshaker_rust 为历史遗留 crate 名，一并纳入清理。
DEPS_PREFIXES=(
  libhandshaker_core handshaker_core
  libhandshaker_application handshaker_application
  libhandshaker_cli handshaker_cli
  libhandshaker_ffi handshaker_ffi
  libhandshaker_rust handshaker_rust
  libhandshaker handshaker
)

TOTAL_REMOVED=0
TOTAL_BYTES=0

# 判断元素是否在数组中（bash 3.2 兼容，线性查找）
in_array() {
  local needle="$1"; shift
  local item
  for item in "$@"; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

# 累加一组路径的磁盘占用（KB），分块避免 ARG_MAX
size_kb_of() {
  local total=0 chunk=() kb
  for f in "$@"; do
    chunk+=("$f")
    if [ "${#chunk[@]}" -ge 500 ]; then
      kb="$(du -ck "${chunk[@]}" 2>/dev/null | tail -1 | awk '{print $1}')" || kb=""
      total=$((total + ${kb:-0}))
      chunk=()
    fi
  done
  if [ "${#chunk[@]}" -gt 0 ]; then
    kb="$(du -ck "${chunk[@]}" 2>/dev/null | tail -1 | awk '{print $1}')" || kb=""
    total=$((total + ${kb:-0}))
  fi
  echo "$total"
}

# 分块删除文件；dry-run 时只打印
remove_files() {
  [ "$#" -gt 0 ] || return 0
  TOTAL_REMOVED=$((TOTAL_REMOVED + $#))
  TOTAL_BYTES=$((TOTAL_BYTES + $(size_kb_of "$@")))
  if [ "$DRY_RUN" = 1 ]; then
    local f
    for f in "$@"; do echo "  [dry-run] rm $f"; done
    return 0
  fi
  local chunk=()
  for f in "$@"; do
    chunk+=("$f")
    if [ "${#chunk[@]}" -ge 500 ]; then
      rm -f "${chunk[@]}"
      chunk=()
    fi
  done
  [ "${#chunk[@]}" -gt 0 ] && rm -f "${chunk[@]}"
}

# 分块删除目录；dry-run 时只打印
remove_dirs() {
  [ "$#" -gt 0 ] || return 0
  TOTAL_REMOVED=$((TOTAL_REMOVED + $#))
  TOTAL_BYTES=$((TOTAL_BYTES + $(size_kb_of "$@")))
  if [ "$DRY_RUN" = 1 ]; then
    local d
    for d in "$@"; do echo "  [dry-run] rm -rf $d"; done
    return 0
  fi
  local chunk=()
  for d in "$@"; do
    chunk+=("$d")
    if [ "${#chunk[@]}" -ge 500 ]; then
      rm -rf "${chunk[@]}"
      chunk=()
    fi
  done
  [ "${#chunk[@]}" -gt 0 ] && rm -rf "${chunk[@]}"
}

# 清理 deps/ 中某个前缀的旧版本：同一 hash 为一组（一个版本的 lib/测试产物），
# 按 mtime 新→旧保留 KEEP 组，其余组的全部文件删除
prune_deps_prefix() {
  local prefix="$1"
  local matches=() sorted=() del=() f name hash
  matches=("$DEPS_DIR"/${prefix}-[0-9a-f]*)
  [ "${#matches[@]}" -gt 0 ] && [ -e "${matches[0]}" ] || return 0
  sorted=($(ls -t "${matches[@]}"))

  local hashes=() drop_hashes=()
  for f in "${sorted[@]}"; do
    name="${f##*/}"
    hash="${name#${prefix}-}"
    hash="${hash%%.*}"
    if [ "${#hashes[@]}" -eq 0 ] || ! in_array "$hash" "${hashes[@]}"; then
      hashes+=("$hash")
      [ "${#hashes[@]}" -gt "$KEEP" ] && drop_hashes+=("$hash")
    fi
  done
  for f in "${matches[@]}"; do
    name="${f##*/}"
    hash="${name#${prefix}-}"
    hash="${hash%%.*}"
    if [ "${#drop_hashes[@]}" -gt 0 ] && in_array "$hash" "${drop_hashes[@]}"; then
      del+=("$f")
    fi
  done
  if [ "${#del[@]}" -gt 0 ]; then
    remove_files "${del[@]}"
  fi
}

# 清理 incremental/：同一编译单元（unit）为一组，按 mtime 新→旧保留 KEEP 个目录
prune_incremental() {
  [ -d "$INCR_DIR" ] || return 0
  local sorted=() del=() name unit
  sorted=($(ls -t "$INCR_DIR"))
  [ "${#sorted[@]}" -gt 0 ] || return 0
  local units=() counts=() idx found n
  for name in "${sorted[@]}"; do
    case "$name" in
      *-*) unit="${name%-*}" ;;
      *) continue ;;
    esac
    found=-1
    for ((idx = 0; idx < ${#units[@]}; idx++)); do
      [ "${units[$idx]}" = "$unit" ] && { found=$idx; break; }
    done
    if [ "$found" -ge 0 ]; then
      n=$((counts[$found] + 1))
      counts[$found]=$n
      [ "$n" -gt "$KEEP" ] && del+=("$INCR_DIR/$name")
    else
      units+=("$unit")
      counts+=(1)
    fi
  done
  if [ "${#del[@]}" -gt 0 ]; then
    remove_dirs "${del[@]}"
  fi
}

echo "==> cargo build --workspace（复用上一版缓存，参数: ${CARGO_ARGS[*]:-无}）"
if [ "${HS_SKIP_BUILD:-0}" = "1" ]; then
  echo "    (HS_SKIP_BUILD=1，跳过构建，仅执行缓存清理)"
elif [ "${#CARGO_ARGS[@]}" -gt 0 ]; then
  cargo build --workspace "${CARGO_ARGS[@]}"
else
  cargo build --workspace
fi

echo "==> 构建成功，清理旧版本缓存（每个 crate 保留最新 ${KEEP} 版）"
for prefix in "${DEPS_PREFIXES[@]}"; do
  prune_deps_prefix "$prefix"
done
prune_incremental

if [ "$TOTAL_REMOVED" -gt 0 ]; then
  if [ "$DRY_RUN" = 1 ]; then
    echo "==> 预览完成：将删除 ${TOTAL_REMOVED} 个文件/目录，预计释放约 $((TOTAL_BYTES / 1024)) MB"
  else
    echo "==> 完成：删除 ${TOTAL_REMOVED} 个文件/目录，释放约 $((TOTAL_BYTES / 1024)) MB"
  fi
else
  echo "==> 完成：无旧版本缓存可清理"
fi
