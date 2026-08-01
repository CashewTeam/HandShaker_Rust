#!/usr/bin/env python3
"""Multi-frame downstream chunking validation: force a >32761-byte response
and a large multi-frame file download; log every frame."""
import base64
import hashlib
import os
import socket
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ssp_capture import (  # noqa: E402
    KEY_TABLE, AES_KEY, AES_IV, T, RNAME, Cap, Ssp,
    f_varint, f_str, f_msg, f_bool, parse_varint, pb_fields, pb_get_str,
    pb_get_varint, decode_response, sign, build_enckey,
)
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

PORT = int(os.environ.get("HSPORT", "10086"))


def fieldnames(buf, maxlen=40):
    out = []
    for n, w, v in pb_fields(buf):
        if w == 0:
            out.append(f"f{n}={v}")
        elif w == 2:
            try:
                s = v.decode("utf-8")
                if all(32 <= ord(c) < 127 for c in s):
                    out.append(f"f{n}='{s[:maxlen]}'")
                else:
                    out.append(f"f{n}=<{len(v)}B>")
            except Exception:
                out.append(f"f{n}=<{len(v)}B>")
    return " ".join(out)


def main():
    cap = Cap(os.environ.get("CAP_FILE", "/tmp/ssp_multi.log"))
    priv = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    pub_der = priv.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.PKCS1)
    ssp = Ssp(cap)

    # handshake
    sid = ssp.next_sid()
    ssp.send(sid, 0, build_enckey(pub_der, "aes_cbc"), "HS")
    frames, hlen = ssp.recv_frames("hs-reply")
    reply = decode_response(frames, hlen)
    ok = priv.decrypt(base64.b64decode(reply), __import__("cryptography").hazmat.primitives.asymmetric.padding.PKCS1v15())
    assert ok == b"ok", ok
    print("=== handshake ok ===")

    # device info (root)
    req = (f_varint(1, 2) + f_varint(2, int(time.time())) + f_str(3, "1")
           + f_bool(4, False) + f_str(9, "1.0.0") + f_varint(10, 1) + f_varint(11, 408))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "DEVINFO")
    frames, hlen = ssp.recv_frames("devinfo")
    dev = decode_response(frames, hlen)
    root = pb_get_str(dev, 17) or "/sdcard"
    print("root:", root)

    # ---- force multi-frame: GET_DIR_FILES maxdepth=2 ----
    dfile = f_str(1, root) + f_bool(6, True)
    req = f_varint(1, 7) + f_msg(2, dfile) + f_varint(3, 2)
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "DIRFILES(depth2)")
    frames, hlen = ssp.recv_frames("dirfiles-depth2")
    data = decode_response(frames, hlen)
    nfiles = sum(1 for n, w, v in pb_fields(data) if n == 5 and w == 2)
    print(f"dirfiles-depth2 response: {len(data)} bytes, {len(frames)} frames, {nfiles} files")
    print("frame sizes:", [len(c) for _, c in frames])
    print("header_total:", hlen, "actual-8+total:", 8 + hlen)

    # ---- large download: find a big file ----
    big = None
    for n, w, v in pb_fields(data):
        if n == 5 and w == 2:
            p = pb_get_str(v, 1); sz = pb_get_varint(v, 2)
            if p and sz and sz > 100_000:
                big = (p, sz)
                break
    print("big file candidate:", big)
    if big:
        p, sz = big
        req = (f_varint(1, 12) + f_msg(2, f_str(1, p) + f_varint(2, sz))
               + f_msg(3, f_varint(1, 0) + f_varint(2, 0))
               + f_bool(4, False) + f_bool(5, False) + f_bool(6, False))
        sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "DL")
        frames, hlen = ssp.recv_frames("dl-header")
        hdr = decode_response(frames, hlen)
        rlen = pb_get_varint(pb_get_msg_field(hdr, 3), 2)
        print("download header: file range.length =", rlen, "ready =", pb_get_varint(hdr, 6))
        print("download header frames:", [len(c) for _, c in frames])
        frames, _ = ssp.recv_frames("dl-body", total_expected=rlen)
        got = sum(len(c) for _, c in frames)
        print(f"download body: {len(frames)} frames, {got} bytes (expected {rlen})")
        print("frame sizes:", [len(c) for _, c in frames][:30], "..." if len(frames) > 30 else "")

    # ---- photo library (often large) ----
    req = f_varint(1, 4)
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "PHOTOLIB")
    frames, hlen = ssp.recv_frames("photolib")
    data = decode_response(frames, hlen)
    nimg = sum(1 for n, w, v in pb_fields(data) if n == 2 and w == 2)
    print(f"photolib response: {len(data)} bytes, {len(frames)} frames, {nimg} images")

    ssp.sock.close(); cap.close()
    print("=== done ===")


def pb_get_msg_field(buf, num):
    for n, w, v in pb_fields(buf):
        if n == num and w == 2:
            return v
    return None


if __name__ == "__main__":
    main()
