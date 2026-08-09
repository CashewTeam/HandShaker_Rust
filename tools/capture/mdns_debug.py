#!/usr/bin/env python3
"""mDNS receive debug: send unicast query, print EVERY received packet raw.
Run with sudo: sudo python3 mdns_debug.py <phone_ip>"""
import socket
import struct
import sys
import time

import ssp_mdns as M

if len(sys.argv) != 2:
    raise SystemExit("usage: sudo python3 mdns_debug.py <phone_ip>")
ip = sys.argv[1]
q = M.make_query(M.SERVICE, 12)

print("== unicast socket ==")
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(0.3)
s.sendto(q, (ip, 5353))
print(f"query sent to {ip}:5353, local port =", s.getsockname()[1])

print("== v4 multicast socket (bind 5353 + join) ==")
m4 = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
m4.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    m4.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
except OSError:
    pass
m4.bind(("", 5353))
mreq = struct.pack("4s4s", socket.inet_aton("224.0.0.251"), socket.inet_aton("0.0.0.0"))
m4.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
m4.settimeout(0.3)

socks = [("unicast", s), ("mcast-v4", m4)]
end = time.time() + 6
got = 0
while time.time() < end:
    for name, sock in socks:
        try:
            data, addr = sock.recvfrom(4096)
        except socket.timeout:
            continue
        got += 1
        print(f"[{time.time():.3f}] {name} <- {addr} len={len(data)}")
        print("   hex:", data[:64].hex(" "))
        pkt = M.parse_packet(data)
        if pkt:
            for rname, rtype, rclass, ttl, rdata, rdata_off in pkt["rrs"]:
                print(f"   RR: {rname} type={rtype} class={rclass} ttl={ttl} rdlen={len(rdata)}")
                try:
                    v = M.parse_rdata(rtype, rdata, data, rdata_off)
                    print(f"      -> {v}")
                except Exception as e:
                    print(f"      parse err {e}")
print("== total packets received:", got, "==")
