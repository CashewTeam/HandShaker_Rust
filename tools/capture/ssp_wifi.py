#!/usr/bin/env python3
"""
HandShaker SSP over WiFi/LAN validation client.

Validates (docs/02, docs/04, docs/14):
  - mDNS discovery of `_handshaker_ssp._tcp` (or direct --ip/--port)
  - WiFi-type handshake: REQUEST_01 -> RESPONSE_01 -> REQUEST_02 -> RESPONSE_02
    (incl. phone trust dialog, auto-tapped via adb)
  - GET_DEVICE_INFO / HEART_BEAT / GET_DIR_FILES / download / upload over LAN
  - full byte logging (same downstream framing as ADB tests)

Run this in a terminal WITHOUT proxy/sandbox restrictions (a normal Terminal works).

Usage:
  python3 ssp_wifi.py                # auto mDNS discovery
  python3 ssp_wifi.py --ip <phone_ip> --port <dynamic_port>
  python3 ssp_wifi.py --no-autotap   # you tap "信任" on the phone yourself
"""
import argparse
import base64
import hashlib
import os
import socket
import struct
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ssp_capture import (KEY_TABLE, AES_KEY, AES_IV, T, RNAME, Cap, Ssp,
                         f_varint, f_str, f_msg, f_bool, pb_fields,
                         pb_get_str, pb_get_varint, decode_response,
                         build_enckey)
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa, padding

TRUST_TAP_LABELS = ("信任", "ALWAYS", "始终信任", "Trust")
TRUST_STORE = "/tmp/ssp_wifi_trust.json"


def load_trust():
    import json
    try:
        with open(TRUST_STORE) as f:
            return json.load(f)
    except Exception:
        return None


def save_trust(host_uuid, derived_key_b64):
    import json
    with open(TRUST_STORE, "w") as f:
        json.dump({"host_uuid": host_uuid, "derived_key_b64": derived_key_b64}, f)
    print(f"[trust] saved derived_key to {TRUST_STORE}")


def parse_args():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ip", default=None)
    ap.add_argument("--port", type=int, default=None)
    ap.add_argument("--no-autotap", action="store_true", help="manually tap trust on phone")
    ap.add_argument("--reset-trust", action="store_true",
                    help="send request02 with TRUST_REMOVE first (clears phone trust record)")
    ap.add_argument("--cap", default="/tmp/ssp_wifi.log")
    return ap.parse_args()


def mdns_discover():
    import ssp_mdns
    res = ssp_mdns.browse(timeout=6)
    for r in res:
        if r["ips"]:
            return r["ips"][0], r["port"]
    return None, None


def adb_tap_trust():
    """Auto-tap the 'trust' positive button via uiautomator. Returns True on success."""
    import re
    import xml.etree.ElementTree as ET
    for attempt in range(3):
        try:
            subprocess.run(["adb", "shell", "uiautomator", "dump", "/sdcard/window_dump.xml"],
                           capture_output=True, timeout=10)
            xml = subprocess.run(["adb", "shell", "cat", "/sdcard/window_dump.xml"],
                                 capture_output=True, timeout=10).stdout.decode("utf-8", "replace")
        except Exception:
            time.sleep(1.0)
            continue
        best = None
        try:
            root = ET.fromstring(xml)
            for node in root.iter("node"):
                text = (node.get("text") or "").strip()
                desc = (node.get("content-desc") or "").strip()
                bounds = node.get("bounds") or ""
                m = re.match(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", bounds)
                if text in TRUST_TAP_LABELS or desc in TRUST_TAP_LABELS:
                    if m:
                        cx, cy = (int(m.group(1)) + int(m.group(3))) // 2, (int(m.group(2)) + int(m.group(4))) // 2
                        best = (text or desc, cx, cy)
                        break
        except Exception:
            pass
        if best:
            subprocess.run(["adb", "shell", "input", "tap", str(best[1]), str(best[2])],
                           capture_output=True, timeout=5)
            print(f"[trust] auto-tapped '{best[0]}' at {best[1]},{best[2]}")
            return True
        time.sleep(1.0)
    return False


def main():
    args = parse_args()
    ip, port = args.ip, args.port
    if not ip:
        print("[mdns] discovering _handshaker_ssp._tcp ...")
        ip, port = mdns_discover()
        if not ip:
            print("!! mDNS discovery found nothing. use --ip/--port")
            return 1
    print(f"target: {ip}:{port}")

    cap = Cap(args.cap)
    priv = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    pub_der = priv.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.PKCS1)

    class WifiSsp(Ssp):
        def __init__(self, c):
            self.sock = socket.create_connection((ip, port), timeout=15)
            self.cap = c
            self.sid = 0x80000000

    ssp = WifiSsp(cap)
    print("=== TCP connected ===")

    # ---------- 1) HANDSHAKE_REQUEST_01 ----------
    host_uuid = "python-lan-test-0001"
    sid = ssp.next_sid()
    req01 = (
        f_varint(1, T["HANDSHAKE_REQUEST_01"])
        + f_str(2, host_uuid)
        + f_str(3, "python-lan-test")
        + f_varint(4, int(time.time()))
        + f_str(5, "1")
        + f_str(6, "2.5.6")
        + f_str(7, "1.0.0")
        + f_bytes(8, hashlib.md5(pub_der).digest())
        + f_bytes(9, build_enckey(pub_der, "aes_cbc")[16:])  # enckey (without md5 prefix)
        + f_str(10, "MacBookPro-python")
        + f_varint(11, 30)
    )
    ssp.send(sid, 0, req01, "HANDSHAKE_REQUEST_01")
    frames, hlen = ssp.recv_frames("resp01")
    resp01 = decode_response(frames, hlen)
    print("--- RESPONSE_01 ---")
    for n, w, v in pb_fields(resp01):
        if w == 0:
            print(f"  f{n} = {v}")
        elif w == 2:
            try:
                s = v.decode()
                print(f"  f{n} = {s!r}" if all(32 <= ord(c) < 127 for c in s) else f"  f{n} = <{len(v)}B>")
            except Exception:
                print(f"  f{n} = <{len(v)}B>")
    if pb_get_varint(resp01, 1) != T["HANDSHAKE_RESPONSE_01"]:
        print("!! unexpected response01 type"); return 1

    # ---------- 2) HANDSHAKE_REQUEST_02 ----------
    sid = ssp.next_sid()
    trust = load_trust()
    req02 = f_varint(1, T["HANDSHAKE_REQUEST_02"]) + f_str(2, host_uuid)
    if args.reset_trust:
        req02 += f_varint(4, 6)  # TRUST_REMOVE -> phone clears its record
        print("[trust] sending TRUST_REMOVE to reset phone trust record")
    elif trust and trust.get("host_uuid") == host_uuid:
        # reconnect: send the derived_key previously received from the phone
        dk = base64.b64decode(trust["derived_key_b64"])
        req02 += f_bytes(3, dk)
        print("[trust] reconnecting with stored derived_key")
    ssp.send(sid, 0, req02, "HANDSHAKE_REQUEST_02")
    print("--- waiting for RESPONSE_02 (trust) ---")
    autotapped = False
    handshake_ok = False
    deadline = time.time() + 120
    while time.time() < deadline:
        frames, hlen = ssp.recv_frames("resp02")
        r = decode_response(frames, hlen)
        ttype = pb_get_varint(r, 2)
        result = pb_get_str(r, 6)
        dk_field = None
        for n, w, v in pb_fields(r):
            if n == 5 and w == 2:
                dk_field = v
        print(f"  RESPONSE_02: trust_type={ttype} result={result!r} derived_key=<{len(dk_field or b'')}B>")
        if dk_field:
            save_trust(host_uuid, base64.b64encode(dk_field).decode())
        if result:
            if result in ("failed", "locked", "needauth"):
                print(f"!! handshake result: {result}"); return 1
            try:
                ok = priv.decrypt(base64.b64decode(result), padding.PKCS1v15())
                print("  decrypted result:", ok)
                if ok == b"ok":
                    print("=== WIFI HANDSHAKE OK ===")
                    handshake_ok = True
                    break
            except Exception as e:
                print("  decrypt failed:", e)
        else:
            # TRUST_WAITING -> show dialog; auto-tap once
            if ttype == 1:
                if not autotapped and not args.no_autotap:
                    print("[trust] waiting for dialog, auto-tapping ...")
                    time.sleep(2.0)
                    if adb_tap_trust():
                        autotapped = True
                    else:
                        print("[trust] auto-tap failed -> TAP '信任/ALWAYS' ON THE PHONE NOW")
                else:
                    print("[trust] waiting for you to tap '信任' on the phone ...")
    else:
        print("!! trust dialog timeout"); return 1
    if not handshake_ok:
        return 1

    # ---------- 3) device info ----------
    req = (f_varint(1, T["GET_DEVICE_INFO_REQUEST"]) + f_varint(2, int(time.time()))
           + f_str(3, "1") + f_bool(4, True)
           + f_str(9, "1.0.0") + f_varint(10, 1) + f_varint(11, 408))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "GET_DEVICE_INFO")
    frames, hlen = ssp.recv_frames("devinfo")
    dev = decode_response(frames, hlen)
    root = pb_get_str(dev, 17)
    print("--- DEVICE_INFO: root =", root, "model =", pb_get_str(dev, 9),
          "device_uuid =", pb_get_str(dev, 28) or pb_get_str(dev, 7))

    # ---------- 4) heartbeat ----------
    hb = f_varint(1, T["HEART_BEAT_REQUEST"]) + f_varint(2, int(time.time()))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(hb, priv) + hb, "HEART_BEAT")
    frames, hlen = ssp.recv_frames("hb")
    print("HEART_BEAT response:", decode_response(frames, hlen).hex())

    # ---------- 5) dir files ----------
    dfile = f_str(1, root or "/sdcard") + f_bool(6, True)
    req = f_varint(1, T["GET_DIR_FILES_REQUEST"]) + f_msg(2, dfile) + f_varint(3, 1)
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "GET_DIR_FILES")
    frames, hlen = ssp.recv_frames("dir")
    data = decode_response(frames, hlen)
    nfiles = sum(1 for n, w, v in pb_fields(data) if n == 5 and w == 2)
    print(f"GET_DIR_FILES: {nfiles} entries, {len(data)} bytes, {len(frames)} frames")

    # ---------- 6) download a small file ----------
    small = None
    for n, w, v in pb_fields(data):
        if n == 5 and w == 2:
            p = pb_get_str(v, 1); sz = pb_get_varint(v, 2)
            if p and sz and 0 < sz <= 8192:
                small = (p, sz); break
    if small:
        p, sz = small
        req = (f_varint(1, T["GET_DOWNLOAD_FILE_REQUEST"])
               + f_msg(2, f_str(1, p) + f_varint(2, sz))
               + f_msg(3, f_varint(1, 0) + f_varint(2, 0))
               + f_bool(4, False) + f_bool(5, False) + f_bool(6, False))
        sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "DL")
        frames, hlen = ssp.recv_frames("dl-header")
        hdr = decode_response(frames, hlen)
        rlen = None
        for n, w, v in pb_fields(hdr):
            if n == 3 and w == 2:
                for n2, w2, v2 in pb_fields(v):
                    if n2 == 2 and w2 == 0:
                        rlen = v2
        print(f"download {p}: header range.length={rlen}")
        if rlen:
            frames, _ = ssp.recv_frames("dl-body", total_expected=rlen)
            got = sum(len(c) for _, c in frames)
            print(f"download body: {len(frames)} frames / {got} bytes")

    # ---------- 7) upload ----------
    up_path = "/storage/emulated/0/Download/ssp_wifi_test.bin"
    data_up = bytes((i * 13 + 5) % 256 for i in range(50000))
    up_md5 = hashlib.md5(data_up).hexdigest()
    req = (f_varint(1, T["GET_UPLOAD_FILE_REQUEST_HEADER"])
           + f_msg(2, f_str(1, up_path) + f_varint(2, len(data_up)))
           + f_str(3, up_md5) + f_bool(4, False) + f_bool(5, False))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "UPLOAD_HEADER")
    frames, hlen = ssp.recv_frames("uph")
    up_resp = decode_response(frames, hlen)
    ready = pb_get_varint(up_resp, 3)
    print("upload ready:", ready)
    if ready == 1:
        for o in range(0, len(data_up), 2048):
            ssp.send(sid, 3, data_up[o:o + 2048], "UDATA")
        frames, hlen = ssp.recv_frames("up-done")
        done = decode_response(frames, hlen)
        print("upload done fields:", [(n, w, v) for n, w, v in pb_fields(done)])

    # ---------- 8) quit ----------
    quit_req = f_varint(1, T["QUIT_REQUEST"])
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(quit_req, priv) + quit_req, "QUIT")
    time.sleep(0.5)
    ssp.sock.close()
    cap.close()
    print("=== WIFI LAN VALIDATION DONE (see", args.cap, ") ===")
    return 0


def sign(payload, priv):
    from cryptography.hazmat.primitives import hashes
    return priv.sign(payload, padding.PKCS1v15(), hashes.SHA256())


def f_bytes(num, val):
    from ssp_capture import fld, varint
    return fld(num, 2, varint(len(val)) + val)


if __name__ == "__main__":
    sys.exit(main())
