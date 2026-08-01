#!/usr/bin/env python3
"""Upload flow validation: header(15) -> ready(16) -> flag=3 chunks -> done(18).
Verify uploaded bytes on the phone."""
import base64
import hashlib
import os
import socket
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ssp_capture import (Cap, Ssp, f_varint, f_str, f_msg, f_bool, pb_fields,
                         pb_get_varint, decode_response, sign, build_enckey)
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

UP_PATH = os.environ.get("UP_PATH", "/storage/emulated/0/Download/ssp_upload_test.bin")
DATA_SIZE = int(os.environ.get("UP_SIZE", "150000"))


def main():
    cap = Cap(os.environ.get("CAP_FILE", "/tmp/ssp_upload.log"))
    priv = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    pub_der = priv.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.PKCS1)
    ssp = Ssp(cap)

    sid = ssp.next_sid()
    ssp.send(sid, 0, build_enckey(pub_der, "aes_cbc"), "HS")
    frames, hlen = ssp.recv_frames("hs-reply")
    reply = decode_response(frames, hlen)
    ok = priv.decrypt(base64.b64decode(reply), __import__("cryptography").hazmat.primitives.asymmetric.padding.PKCS1v15())
    assert ok == b"ok", ok
    print("=== handshake ok ===")

    data = bytes((i * 7 + 3) % 256 for i in range(DATA_SIZE))
    md5 = hashlib.md5(data).hexdigest()
    print(f"uploading {DATA_SIZE} bytes to {UP_PATH}")

    # header: type=15, file{path,size}, data_md5, is_sync
    req = (f_varint(1, 15)
           + f_msg(2, f_str(1, UP_PATH) + f_varint(2, DATA_SIZE))
           + f_str(3, md5)
           + f_bool(4, False)
           + f_bool(5, False))
    sid = ssp.next_sid(); ssp.send(sid, 1, sign(req, priv) + req, "UPLOAD_HEADER")
    frames, hlen = ssp.recv_frames("upload-header-response")
    resp = decode_response(frames, hlen)
    print("upload header response fields:", [(n, w, v) for n, w, v in pb_fields(resp)])
    ready = pb_get_varint(resp, 3)
    err = pb_get_varint(resp, 4)
    print("ready:", ready, "error_code:", err)
    if ready != 1:
        print("!! upload rejected"); return 1

    # send flag=3 data chunks (1024-byte chunks to exercise multiple frames)
    offset = 0
    CH = 1024
    while offset < len(data):
        chunk = data[offset:offset + CH]
        ssp.send(sid, 3, chunk, f"UPLOAD_DATA({offset}-{offset+len(chunk)})")
        offset += len(chunk)
    # final response type 18
    frames, hlen = ssp.recv_frames("upload-done")
    resp = decode_response(frames, hlen)
    print("upload done response fields:", [(n, w, v) for n, w, v in pb_fields(resp)])
    success = pb_get_varint(resp, 4)
    canceled = pb_get_varint(resp, 3)
    print("succeed:", success, "canceled:", canceled)

    ssp.sock.close(); cap.close()
    print("=== done ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
