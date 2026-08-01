#!/usr/bin/env python3
"""Test MD5-mismatch rejection on upload + cancel (flag=2) during download."""
import base64
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ssp_capture import (Cap, Ssp, f_varint, f_str, f_msg, f_bool, pb_fields,
                         pb_get_varint, decode_response, sign, build_enckey)
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa


def handshake(ssp, priv, pub_der):
    sid = ssp.next_sid()
    ssp.send(sid, 0, build_enckey(pub_der, "aes_cbc"), "HS")
    frames, hlen = ssp.recv_frames("hs-reply")
    reply = decode_response(frames, hlen)
    ok = priv.decrypt(base64.b64decode(reply), __import__("cryptography").hazmat.primitives.asymmetric.padding.PKCS1v15())
    assert ok == b"ok", ok


def main():
    cap = Cap(os.environ.get("CAP_FILE", "/tmp/ssp_err.log"))
    priv = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    pub_der = priv.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.PKCS1)
    ssp = Ssp(cap)
    handshake(ssp, priv, pub_der)
    print("=== handshake ok ===")

    # ---- MD5 mismatch upload ----
    data = b"X" * 2048
    req = (f_varint(1, 15)
           + f_msg(2, f_str(1, "/storage/emulated/0/Download/ssp_md5bad.bin") + f_varint(2, len(data)))
           + f_str(3, "0" * 32)  # WRONG md5
           + f_bool(4, False) + f_bool(5, False))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "UPLOAD_HEADER(badmd5)")
    frames, hlen = ssp.recv_frames("uph")
    resp = decode_response(frames, hlen)
    ready = pb_get_varint(resp, 3)
    print("upload header ready:", ready, "fields:", [(n, w, v) for n, w, v in pb_fields(resp)])
    if ready == 1:
        for o in range(0, len(data), 1024):
            ssp.send(sid, 3, data[o:o + 1024], "UDATA")
        frames, hlen = ssp.recv_frames("up-done")
        resp = decode_response(frames, hlen)
        print("upload done (bad md5) fields:", [(n, w, v) for n, w, v in pb_fields(resp)])
        print("succeed:", pb_get_varint(resp, 4), "error_code:", pb_get_varint(resp, 5))

    # ---- cancel during download (flag=2) ----
    req = (f_varint(1, 12)
           + f_msg(2, f_str(1, "/storage/emulated/0/Download/ssp_upload_test.bin") + f_varint(2, 150000))
           + f_msg(3, f_varint(1, 0) + f_varint(2, 0))
           + f_bool(4, False) + f_bool(5, False) + f_bool(6, False))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "DL(big)")
    frames, hlen = ssp.recv_frames("dl-header")
    hdr = decode_response(frames, hlen)
    rlen = pb_get_varint(pb_get_msg(hdr, 3), 2)
    print("download header range.length:", rlen, "ready:", pb_get_varint(hdr, 6))
    # read one body frame then cancel
    ssp.recv_frames("dl-partial", total_expected=32761)
    print("read 32761 bytes; sending cancel(flag=2) for sid", sid)
    ssp.send(ssp.next_sid(), 2, struct.pack(">I", sid), "CANCEL flag=2")
    try:
        frames, hlen = ssp.recv_frames("after-cancel", total_expected=200000)
        resp = decode_response(frames, hlen)
        print("after-cancel bytes:", resp[:80])
    except Exception as e:
        print("after-cancel read ended:", type(e).__name__, e)

    ssp.sock.close(); cap.close()
    print("=== done ===")


def pb_get_msg(buf, num):
    for n, w, v in pb_fields(buf):
        if n == num and w == 2:
            return v
    return None


if __name__ == "__main__":
    main()
