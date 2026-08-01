#!/usr/bin/env python3
"""
HandShaker SSP validation client.

Speaks the SmartSync Protocol to the phone over adb forward, logging EVERY byte
in both directions. Used to validate docs/ claims:
  - ADB forward port / phone listener
  - upstream framing [sid:4][flag:1][len:4]
  - downstream framing [sid:4][chunkLen:2] frames + 8-byte length prefix
  - enckey: MD5(DER) + AES-256-CBC(key=table[16:48], iv=table[0:16])(base64(DER))
  - RSA-1024/SHA256 signature (flag=1)
  - download file-stream frames, cancel, etc.
"""
import base64
import hashlib
import json
import os
import socket
import struct
import sys
import time

from cryptography.hazmat.primitives import serialization, hashes
from cryptography.hazmat.primitives.asymmetric import rsa, padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

HOST = "127.0.0.1"
PORT = int(os.environ.get("HSPORT", "10086"))
ADB_PORT = int(os.environ.get("ADB_PORT", "10086"))

# ---- embedded Smartisan key table (from SmartFinderCore 0x229f60 / libsmartfolder.so) ----
KEY_TABLE = bytes.fromhex(
    "2b 9e 34 d4 e1 d9 08 89 94 93 9e c4 e3 e9 60 c5"  # IV  (table[0:16])
    "28 e3 ee 32 b0 de 27 ef 6b c2 97 92 05 4e f9 73"  # AES-256 key[16:32]
    "9c e8 e8 7b b4 95 f2 ea 0d 72 d4 f4 f4 0b 3b de"  # key[32:48]
)
AES_KEY = KEY_TABLE[16:48]
AES_IV = KEY_TABLE[0:16]

# request type enum (SSPRequestType)
T = {
    "HEART_BEAT_REQUEST": 1, "GET_DEVICE_INFO_REQUEST": 2, "GET_THUMBNAIL_REQUEST": 3,
    "GET_PHOTO_LIB_REQUEST": 4, "GET_VIDEO_LIB_REQUEST": 5, "GET_AUDIO_LIB_REQUEST": 6,
    "GET_DIR_FILES_REQUEST": 7, "GET_FILE_COUNT_REQUEST": 8, "GET_FILE_EXIST_REQUEST": 9,
    "GET_CREATE_FOLDER_REQUEST": 10, "GET_RENAME_FILE_REQUEST": 11,
    "GET_DOWNLOAD_FILE_REQUEST": 12, "GET_DOWNLOAD_FILE_RESPONSE_HEADER": 13,
    "GET_DOWNLOAD_FILE_RESPONSE_BODY": 14, "GET_UPLOAD_FILE_REQUEST_HEADER": 15,
    "GET_UPLOAD_FILE_RESPONSE_HEADER": 16, "GET_UPLOAD_FILE_REQUEST_BODY": 17,
    "GET_UPLOAD_FILE_RESPONSE": 18, "GET_DELETE_FILE_REQUEST": 19,
    "PHOTO_LIB_CHANGE": 20, "AUDIO_LIB_CHANGE": 21, "VIDEO_LIB_CHANGE": 22,
    "MONITOR_FOLDER_REQUEST": 23, "MONITOR_FOLDER_RESPONSE_HEADER": 24,
    "MONITOR_FOLDER_RESPONSE": 25, "GET_CLIPBOARD_REQUEST": 26, "POST_CLIPBOARD_REQUEST": 27,
    "CLEAR_CLIPBOARD_REQUEST": 28, "DELETE_CLIPBOARD_REQUEST": 29, "CLIPBOARD_CHANGE": 30,
    "HANDSHAKE_REQUEST_01": 31, "HANDSHAKE_RESPONSE_01": 32, "HANDSHAKE_REQUEST_02": 33,
    "HANDSHAKE_RESPONSE_02": 34, "QUIT_REQUEST": 35, "CANCEL_REQUEST": 36,
    "PHOTO_SYNC_REQUEST": 37, "FILE_CHANGE": 38, "SYNC_MONITOR_REQUEST": 39,
    "UPDATE_FILE_INFO": 40, "UPDATE_FILE_INFO_RESPONSE": 41,
}
RNAME = {v: k for k, v in T.items()}


# ---------------- logging ----------------
class Cap:
    def __init__(self, path):
        self.f = open(path, "w")
        self.start = time.time()

    def write(self, direction, data, note=""):
        t = time.time() - self.start
        hexs = " ".join(f"{b:02x}" for b in data)
        asc = "".join(chr(b) if 32 <= b < 127 else "." for b in data)
        self.f.write(f"[{t:8.4f}] {direction} {note}\n  hex: {hexs}\n  asc: {asc}\n")
        self.f.flush()
        print(f"[cap] {direction} {note} len={len(data)}")

    def close(self):
        self.f.close()


# ---------------- minimal protobuf ----------------
def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def fld(num, wire, payload):
    return varint((num << 3) | wire) + payload


def f_varint(num, val):
    return fld(num, 0, varint(val))


def f_bytes(num, val):
    return fld(num, 2, varint(len(val)) + val)


def f_str(num, s):
    return f_bytes(num, s.encode("utf-8"))


def f_msg(num, msg):
    return f_bytes(num, msg)


def f_bool(num, val):
    return f_varint(num, 1 if val else 0)


def parse_varint(buf, i):
    val = 0
    shift = 0
    while True:
        b = buf[i]
        i += 1
        val |= (b & 0x7F) << shift
        if not (b & 0x80):
            return val, i
        shift += 7


def pb_fields(buf):
    """Generator of (field_no, wire_type, value) for a protobuf message."""
    i = 0
    while i < len(buf):
        tag, i = parse_varint(buf, i)
        num, wire = tag >> 3, tag & 7
        if wire == 0:
            val, i = parse_varint(buf, i)
        elif wire == 2:
            ln, i = parse_varint(buf, i)
            val = buf[i:i + ln]
            i += ln
        elif wire == 1:
            val = buf[i:i + 8]; i += 8
        elif wire == 5:
            val = buf[i:i + 4]; i += 4
        else:
            raise ValueError(f"unsupported wire {wire} at {i}")
        yield num, wire, val


def pb_get_str(buf, num):
    for n, w, v in pb_fields(buf):
        if n == num and w == 2:
            return v.decode("utf-8", "replace")
    return None


def pb_get_varint(buf, num):
    for n, w, v in pb_fields(buf):
        if n == num and w == 0:
            return v
    return None


# ---------------- framing ----------------
class Ssp:
    def __init__(self, cap):
        self.sock = socket.create_connection((HOST, PORT), timeout=10)
        self.cap = cap
        self.sid = 0x80000000  # increment before use

    def next_sid(self):
        self.sid += 1
        return self.sid

    def send(self, sid, flag, payload, note=""):
        hdr = struct.pack(">IBI", sid, flag, len(payload))
        self.cap.write(">>", hdr, f"{note} header(sid={sid},flag={flag},len={len(payload)})")
        self.cap.write(">>", payload, f"{note} payload")
        self.sock.sendall(hdr + payload)

    def _read_exact(self, n, note=""):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise EOFError(f"eof while reading {note}")
            buf += chunk
        return buf

    def recv_frames(self, note="", sink=None, total_expected=None):
        """Read downstream [sid:4][chunkLen:2] frame stream.

        Returns list of (sid, chunk) frames. For normal responses the first
        8 bytes of the first chunk are the big-endian total length (returned
        separately); for raw file streams there is no length prefix.
        """
        frames = []
        header_total = None
        first = True
        got = 0
        while True:
            hdr = self._read_exact(6, note + " frame header")
            sid, clen = struct.unpack(">IH", hdr)
            self.cap.write("<<", hdr, f"{note} frame-header(sid={sid},chunkLen={clen})")
            chunk = self._read_exact(clen, note + " chunk")
            self.cap.write("<<", chunk, f"{note} chunk")
            frames.append((sid, chunk))
            if first and total_expected is None:
                if len(chunk) >= 8:
                    header_total = struct.unpack(">Q", chunk[:8])[0]
                else:
                    # prefix may span frames; accumulate into a temp parse
                    acc = chunk
                    while len(acc) < 8:
                        h2 = self._read_exact(6, note + " frame header")
                        s2, c2 = struct.unpack(">IH", h2)
                        self.cap.write("<<", h2, f"{note} frame-header")
                        ch2 = self._read_exact(c2, note + " chunk")
                        self.cap.write("<<", ch2, f"{note} chunk")
                        frames.append((s2, ch2))
                        acc += ch2
                    header_total = struct.unpack(">Q", acc[:8])[0]
                first = False
            if total_expected is None:
                if header_total is not None:
                    got += len(chunk)
                    if got >= 8 + header_total:
                        break
            else:
                got += len(chunk)
                if got >= total_expected:
                    break
        return frames, header_total


def decode_response(frames, header_total):
    data = b"".join(c for _, c in frames)
    if header_total is not None:
        data = data[8:8 + header_total]
    return data


# ---------------- enckey / rsa ----------------
def build_enckey(pub_der, mode):
    b64 = base64.b64encode(pub_der)  # ascii base64 of DER PKCS#1 public key
    if mode == "identity":
        ciphertext = b64
    elif mode == "aes_cbc":
        pad = 16 - (len(b64) % 16)
        if pad == 16:
            pad = 0
        plain = b64 + bytes([pad]) * pad if pad else b64
        enc = Cipher(algorithms.AES(AES_KEY), modes.CBC(AES_IV)).encryptor()
        ciphertext = enc.update(plain) + enc.finalize()
    else:
        raise ValueError(mode)
    return hashlib.md5(pub_der).digest() + ciphertext


def sign(payload, priv):
    return priv.sign(payload, padding.PKCS1v15(), hashes.SHA256())


# ---------------- main ----------------
def main():
    mode = os.environ.get("ENCKEY_MODE", "aes_cbc")
    cap = Cap(os.environ.get("CAP_FILE", "/tmp/ssp_capture.log"))

    priv = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    pub = priv.public_key()
    pub_der = pub.public_bytes(serialization.Encoding.DER, serialization.PublicFormat.PKCS1)
    print(f"RSA-1024 key generated, DER len={len(pub_der)}")
    cap.write("META", b"", f"enckey_mode={mode} pub_der_len={len(pub_der)}")

    ssp = Ssp(cap)

    # ---- Handshake: USB-style raw key exchange (flag=0) ----
    sid = ssp.next_sid()
    payload = build_enckey(pub_der, mode)
    ssp.send(sid, 0, payload, "HANDSHAKE(key-exchange)")
    frames, hlen = ssp.recv_frames("handshake-reply")
    reply = decode_response(frames, hlen)
    print("handshake reply bytes:", reply[:64], "len", len(reply))
    cap.write("MSG", reply, "handshake-reply-text")
    if reply == b"failed" or reply == b"locked":
        print("!! handshake rejected:", reply)
        ssp.sock.close(); cap.close()
        return 1
    try:
        ok_raw = base64.b64decode(reply)
        ok = priv.decrypt(ok_raw, padding.PKCS1v15())
        print("decrypted handshake reply:", ok)
        if ok != b"ok":
            print("!! expected 'ok'"); return 1
    except Exception as e:
        print("!! could not decrypt handshake reply:", e)
        return 1
    print("=== HANDSHAKE OK ===")

    # ---- GET_DEVICE_INFO (flag=1 signed) ----
    req = (
        f_varint(1, T["GET_DEVICE_INFO_REQUEST"])
        + f_varint(2, int(time.time()))
        + f_str(3, "1")
        + f_bool(4, True)     # need_device_info_callback
        + f_bool(5, False)    # need_photo_library_callback
        + f_bool(6, False)
        + f_bool(7, False)
        + f_str(8, "2.5.6")
        + f_str(9, "1.0.0")   # host_min_client_version -> phone must be >= this
        + f_varint(10, 1)     # host_type = mac
        + f_varint(11, 408)
    )
    devinfo = run_request(ssp, priv, req, "GET_DEVICE_INFO")

    # ---- HEART_BEAT ----
    hb = f_varint(1, T["HEART_BEAT_REQUEST"]) + f_varint(2, int(time.time()))
    run_request(ssp, priv, hb, "HEART_BEAT")

    # ---- GET_DIR_FILES on root ----
    root = pb_get_str(devinfo, 17) or "/sdcard"
    print("root =", root)
    dfile = f_str(1, root) + f_bool(6, True)
    gdf = (
        f_varint(1, T["GET_DIR_FILES_REQUEST"])
        + f_msg(2, dfile)
        + f_varint(3, 1)
    )
    dirresp = run_request(ssp, priv, gdf, "GET_DIR_FILES")

    # pick a small file to download
    files = parse_dir_files(dirresp)
    print("dir files:", len(files))
    small = None
    for name, size in files:
        if not name.endswith("/") and 0 < size <= 4096:
            small = (name, size)
            break
    print("candidate small file:", small)
    if small:
        fpath, fsize = small
        dl = (
            f_varint(1, T["GET_DOWNLOAD_FILE_REQUEST"])
            + f_msg(2, f_str(1, fpath) + f_varint(2, fsize))
            + f_msg(3, f_varint(1, 0) + f_varint(2, 0))  # range: offset=0 length=0 (full)
            + f_bool(4, False)  # need_md5
            + f_bool(5, False)  # gzip
            + f_bool(6, False)  # is_sync
        )
        # download header first
        frames, hlen = run_request_frames(ssp, priv, dl, "DOWNLOAD", parse=False)
        hdr = decode_response(frames, hlen)
        print("download header proto len:", len(hdr))
        dump_msg(hdr, "download-header")
        # parse header range length
        rng = pb_get_msg(hdr, 3)
        rlen = pb_get_varint(rng, 2) if rng else None
        ready = pb_get_varint(hdr, 6)
        print("header range.length:", rlen, "ready:", ready)
        if rlen and ready is not False:
            total = rlen
            frames, _ = ssp.recv_frames("download-body", total_expected=total)
            print("download body frames:", len(frames), "bytes:", sum(len(c) for _, c in frames))
            # verify data (first bytes vs actual file if possible)
        # CANCEL test: send flag=2 with the download sid
        ssp.send(ssp.next_sid(), 2, struct.pack(">I", 0xDEADBEEF), "CANCEL(flag=2)")
        time.sleep(0.5)

    # ---- QUIT ----
    quit_req = f_varint(1, T["QUIT_REQUEST"])
    run_request(ssp, priv, quit_req, "QUIT")
    ssp.sock.close()
    cap.close()
    print("=== done ===")
    return 0


def run_request(ssp, priv, req, note):
    frames, hlen = run_request_frames(ssp, priv, req, note, parse=True)
    data = decode_response(frames, hlen)
    print(f"--- {note} response: {len(data)} bytes, type=", RNAME.get(pb_get_varint(data, 1)))
    return data


def run_request_frames(ssp, priv, req, note, parse=True):
    sid = ssp.next_sid()
    body = sign(req, priv) + req  # 128B RSA signature + protobuf
    ssp.send(sid, 1, body, note)
    frames, hlen = ssp.recv_frames(note + " response")
    return frames, hlen


def pb_get_msg(buf, num):
    for n, w, v in pb_fields(buf):
        if n == num and w == 2:
            # could be bytes/string/message; return raw
            return v
    return None


def dump_msg(buf, note):
    for n, w, v in pb_fields(buf):
        if w == 0:
            print(f"  {note}.f{n} (varint) = {v}")
        elif w == 2:
            try:
                s = v.decode("utf-8")
                if all(32 <= ord(c) < 127 for c in s):
                    print(f"  {note}.f{n} (str) = {s}")
                else:
                    print(f"  {note}.f{n} (bytes {len(v)}) = {v[:32].hex()}")
            except Exception:
                print(f"  {note}.f{n} (bytes {len(v)}) = {v[:32].hex()}")


def parse_dir_files(buf):
    files = []
    for n, w, v in pb_fields(buf):
        if n == 5 and w == 2:
            path = pb_get_str(v, 1)
            isdir = pb_get_varint(v, 6)
            size = pb_get_varint(v, 2)
            if path is not None:
                files.append((path + ("/" if isdir else ""), size or 0))
    return files


if __name__ == "__main__":
    sys.exit(main())
