#!/bin/bash
# 局域网验证一键脚本（需 sudo：绕过 macOS 本地网络权限门控）
# 用法: sudo ./run_lan.sh [phone_ip]
set -e
cd "$(dirname "$0")"

PHONE="${1:-192.168.2.47}"
PY="$(command -v python3)"

echo "=== [1/3] mDNS 发现 _handshaker_ssp._tcp (unicast=$PHONE) ==="
sudo -n true 2>/dev/null || echo "（下面会要求输入密码）"
sudo "$PY" ssp_mdns.py 6 --ip "$PHONE"

echo
echo "=== [2/3] 读取手机当前 WiFi 服务器端口 ==="
PORT=$(adb shell "cat /proc/net/tcp6" 2>/dev/null | awk '$4=="0A" && $8==10062 {split($2,a,":"); printf "%d\n", "0x"a[2]}' | head -1)
if [ -z "$PORT" ]; then
  echo "!! 未找到手机 WiFi 服务器端口（uid 10062 的 LISTEN 端口），请从上面 mDNS 输出获取端口"
  exit 1
fi
echo "使用端口: $PORT"

echo
echo "=== [3/3] WiFi 握手 + 传输验证 ==="
sudo "$PY" ssp_wifi.py --ip "$PHONE" --port "$PORT"
