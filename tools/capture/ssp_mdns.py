#!/usr/bin/env python3
"""
Raw mDNS/DNS-SD browser (stdlib only) for HandShaker's `_handshaker_ssp._tcp` service.

Multiple discovery strategies (macOS multicast can be broken / blocked):
  A. IPv4 multicast to 224.0.0.251:5353  (bind 0.0.0.0:5353, join group)
  B. IPv6 multicast to ff02::fb:5353      (scope = en0/iface)
  C. unicast mDNS query to --ip hosts on :5353 (Android mdnsd answers unicast)
Use at least one; pass known phone IPs with --ip to be safe:
    python3 ssp_mdns.py 6 --ip 192.168.2.47
"""
import argparse
import select
import socket
import struct
import sys
import time

MDNS4 = "224.0.0.251"
MDNS6 = "ff02::fb"
MDNS_PORT = 5353
SERVICE = "_handshaker_ssp._tcp.local."
SERVICE_SUFFIX = SERVICE.rstrip(".")  # "_handshaker_ssp._tcp.local" (names parse without trailing dot)


def encode_name(name):
    out = b""
    for part in name.rstrip(".").split("."):
        b = part.encode("utf-8")
        out += bytes([len(b)]) + b
    return out + b"\x00"


def make_query(name, qtype=12):
    header = struct.pack(">HHHHHH", 0, 0x0000, 1, 0, 0, 0)
    q = encode_name(name) + struct.pack(">HH", qtype, 1)
    return header + q


def parse_name(buf, off):
    labels = []
    while True:
        b = buf[off]
        if b == 0:
            off += 1
            break
        if b & 0xC0 == 0xC0:
            ptr = struct.unpack(">H", buf[off:off + 2])[0] & 0x3FFF
            labels.append(parse_name(buf, ptr)[0])
            off += 2
            break
        off += 1
        labels.append(buf[off:off + b].decode("utf-8", "replace"))
        off += b
    return ".".join(labels), off


def parse_packet(data):
    if len(data) < 12:
        return None
    tid, flags, qd, an, ns, ar = struct.unpack(">HHHHHH", data[:12])
    off = 12
    qs = []
    for _ in range(qd):
        name, off = parse_name(data, off)
        qtype, qclass = struct.unpack(">HH", data[off:off + 4])
        off += 4
        qs.append((name, qtype, qclass))
    rrs = []
    for _ in range(an + ns + ar):
        name, off = parse_name(data, off)
        rtype, rclass, ttl, rdlen = struct.unpack(">HHIH", data[off:off + 10])
        off += 10
        rdata_off = off
        rdata = data[off:off + rdlen]
        off += rdlen
        rrs.append((name, rtype, rclass, ttl, rdata, rdata_off))
    return {"flags": flags, "qs": qs, "rrs": rrs}


def parse_rdata(rtype, rdata, full_buf=None, rdata_off=None):
    buf = full_buf if full_buf is not None else rdata
    base = rdata_off if rdata_off is not None else 0
    if rtype == 12:  # PTR -> instance name (compression ptrs refer to full message)
        n, _ = parse_name(buf, base)
        return n
    if rtype == 33:  # SRV -> priority,weight,port,target
        prio, weight, port = struct.unpack(">HHH", rdata[:6])
        target, _ = parse_name(buf, base + 6)
        return {"priority": prio, "weight": weight, "port": port, "target": target}
    if rtype == 1:  # A
        return socket.inet_ntoa(rdata)
    if rtype == 28:  # AAAA
        return socket.inet_ntop(socket.AF_INET6, rdata)
    if rtype == 16:  # TXT
        out = {}
        i = 0
        while i < len(rdata):
            ln = rdata[i]
            kv = rdata[i + 1:i + 1 + ln].decode("utf-8", "replace")
            i += 1 + ln
            if "=" in kv:
                k, v = kv.split("=", 1)
                out[k] = v
        return out
    return rdata.hex()


def resolve_iface_ip4(name):
    import subprocess
    try:
        out = subprocess.run(["ipconfig", "getifaddr", name],
                             capture_output=True, text=True, timeout=5)
        ip = out.stdout.strip()
        return socket.inet_aton(ip) if ip else None
    except Exception:
        return None


def browse(service=SERVICE, timeout=4.0, iface=None, unicast_ips=(), verbose=False):
    q = make_query(service, 12)
    socks = []

    def setup_v4_mcast():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except OSError:
            pass
        s.bind(("", MDNS_PORT))
        mreq = struct.pack("4s4s", socket.inet_aton(MDNS4), socket.inet_aton("0.0.0.0"))
        s.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
        s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)
        if iface:
            try:
                iface_ip = socket.inet_aton(iface)
            except OSError:
                iface_ip = resolve_iface_ip4(iface)
            if iface_ip:
                s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, iface_ip)
        s.settimeout(0.4)
        socks.append(("v4-mcast", s))
        try:
            s.sendto(q, (MDNS4, MDNS_PORT))
            print("[mdns] v4 multicast sent", file=sys.stderr)
        except OSError as e:
            print(f"[mdns] v4 multicast send failed: {e}", file=sys.stderr)

    def setup_v6_mcast():
        s = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except OSError:
            pass
        iface_idx = socket.if_nametoindex(iface or "en0")
        s.bind(("::", MDNS_PORT, 0, iface_idx))
        mreq6 = struct.pack("16sI", socket.inet_pton(socket.AF_INET6, MDNS6), iface_idx)
        s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_JOIN_GROUP, mreq6)
        s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_MULTICAST_HOPS, 1)
        s.settimeout(0.4)
        socks.append(("v6-mcast", s))
        try:
            s.sendto(q, (MDNS6, MDNS_PORT, 0, iface_idx))
            print("[mdns] v6 multicast sent", file=sys.stderr)
        except OSError as e:
            print(f"[mdns] v6 multicast send failed: {e}", file=sys.stderr)

    # 1) unicast sockets FIRST (proven: mdns_debug.py receives with this order)
    for ip in unicast_ips:
        try:
            uc = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            uc.settimeout(0.4)
            uc.sendto(q, (ip, MDNS_PORT))
            socks.append((f"unicast-{ip}", uc))
            print(f"[mdns] unicast query sent to {ip} (src port {uc.getsockname()[1]})", file=sys.stderr)
        except OSError as e:
            print(f"[mdns] unicast to {ip} failed: {e}", file=sys.stderr)

    # 2) then multicast sockets
    try:
        setup_v4_mcast()
    except OSError as e:
        print(f"[mdns] v4 socket setup failed: {e}", file=sys.stderr)
    try:
        setup_v6_mcast()
    except OSError as e:
        print(f"[mdns] v6 socket setup failed: {e}", file=sys.stderr)

    ptrs, srvs, addrs, txts = {}, {}, {}, {}
    n_pkts = 0
    end = time.time() + timeout
    while time.time() < end:
        for name, sock in list(socks):
            try:
                data, addr = sock.recvfrom(8192)
            except socket.timeout:
                continue
            except OSError:
                continue
            n_pkts += 1
            if verbose:
                print(f"[mdns] {name} <- {addr} len={len(data)}", file=sys.stderr)
            pkt = parse_packet(data)
            if not pkt:
                continue
            for rname, rtype, rclass, ttl, rdata, rdata_off in pkt["rrs"]:
                try:
                    val = parse_rdata(rtype, rdata, data, rdata_off)
                except Exception:
                    continue
                if verbose:
                    print(f"[mdns]   RR type={rtype} {rname} -> {val}", file=sys.stderr)
                if rtype == 12 and val.lower().endswith(SERVICE_SUFFIX):
                    ptrs[val] = True
                elif rtype == 33 and rname.lower().endswith(SERVICE_SUFFIX):
                    srvs[rname] = val
                elif rtype == 1:
                    addrs.setdefault(rname.lower(), []).append(val)
                elif rtype == 28:
                    addrs.setdefault(rname.lower(), []).append(val)
                elif rtype == 16:
                    txts[rname] = val
        time.sleep(0.02)
    if verbose:
        print(f"[mdns] total packets received: {n_pkts}", file=sys.stderr)
    for _, s in socks:
        try:
            s.close()
        except OSError:
            pass

    results = []
    for inst in ptrs:
        svc = srvs.get(inst, {})
        ips = addrs.get(svc.get("target", "").lower(), [])
        results.append({
            "instance": inst,
            "port": svc.get("port"),
            "target": svc.get("target"),
            "ips": ips,
            "txt": txts.get(inst, {}),
        })
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("timeout", nargs="?", type=float, default=6.0)
    ap.add_argument("--iface", default="en0")
    ap.add_argument("--ip", action="append", default=[], help="send unicast mDNS queries to these IPs")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    ips = args.ip
    print(f"browsing {SERVICE} (timeout {args.timeout}s, unicast={ips or 'none'}) ...")
    res = browse(timeout=args.timeout, iface=args.iface, unicast_ips=ips, verbose=args.verbose)
    if not res:
        print("no _handshaker_ssp services found")
        print("hint: python3 ssp_mdns.py 6 --ip <phone-ip>  (unicast mDNS to the phone)")
    else:
        import json
        print(json.dumps(res, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
