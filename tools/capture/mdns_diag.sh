#!/bin/bash
# mDNS 诊断：抓取 5353 端口流量 + 运行查询，一次定位问题
# 用法: sudo ./mdns_diag.sh <phone_ip>
set -e
cd "$(dirname "$0")"
if [ "$#" -ne 1 ]; then
  echo "用法: sudo ./mdns_diag.sh <phone_ip>" >&2
  exit 2
fi
PHONE="$1"
PY="$(command -v python3)"

echo "=== tcpdump 5353 (5s) + mDNS 查询 ==="
sudo tcpdump -i en0 -n -vv port 5353 -w /tmp/mdns.pcap >/dev/null 2>&1 &
TCPID=$!
sleep 1
sudo "$PY" ssp_mdns.py 8 --ip "$PHONE"
sleep 2
sudo kill "$TCPID" 2>/dev/null
wait "$TCPID" 2>/dev/null

echo
echo "=== pcap 内容 ==="
sudo tcpdump -r /tmp/mdns.pcap -n 2>/dev/null | head -50
