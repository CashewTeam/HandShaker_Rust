#!/usr/bin/env python3
"""Check handshaker-ffi ABI consistency (single source of truth).

Verifies:
1. every exported `fn hs_*` in the Rust lib (crates/handshaker-ffi/src/lib.rs)
   has a prototype in the C header (crates/handshaker-ffi/include/handshaker_ffi.h);2. the header prototype matches the Rust signature: parameter count, type
   categories (pointer widths / integer widths / value structs) and return type;
3. the ABI_VERSION_* constants in lib.rs match the header top comment
   ("ABI version: X.Y.Z");
4. an ABI snapshot file stays in sync (--check-snapshot / --snapshot).

Type categories intentionally compare ABI-relevant shapes, not spelling:
HsRuntime* / HsSubscription* / void* are all `ptr`; const uint8_t* is `u8ptr`.

Usage:
  check-ffi-abi.py --lib LIB_RS --header HEADER_H [--check-snapshot FILE | --snapshot FILE]
"""
import argparse
import os
import re
import sys

RUST_FN_RE = re.compile(r"\bfn (hs_[a-z0-9_]+)\s*\(")
RUST_RET_RE = re.compile(r"->\s*([A-Za-z_][A-Za-z0-9_:<>]*)")
C_FN_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*\s*\**)\s+(hs_[a-z0-9_]+)\s*\(")

RUST_TYPE_MAP = {
    "u32": "u32", "u64": "u64", "usize": "usize", "i32": "i32", "bool": "bool",
    "HsByteBuffer": "buf", "HsCallResult": "buf",
}


def rust_category(ty: str) -> str:
    ty = ty.strip()
    if ty in ("*mut c_void", "*const c_void"):
        return "ptr"
    if ty in ("*mut *mut c_void", "*mut *const c_void"):
        return "ptrptr"
    if ty in ("*const u8", "*mut u8"):
        return "u8ptr"
    if ty in ("*mut HsByteBuffer", "*const HsByteBuffer"):
        return "bufptr"
    if ty == "()":
        return "void"
    return RUST_TYPE_MAP.get(ty, "?" + ty)


def c_category(ty: str) -> str:
    ty = ty.strip()
    if re.fullmatch(r"(const\s+)?uint8_t\s*\*", ty):
        return "u8ptr"
    if re.fullmatch(r"(const\s+)?void\s*\*", ty) or re.fullmatch(
        r"Hs(Runtime|Subscription)\s*\*", ty
    ):
        return "ptr"
    if re.fullmatch(r"(const\s+)?void\s*\*\s*\*", ty) or re.fullmatch(
        r"Hs(Runtime|Subscription)\s*\*\s*\*", ty
    ):
        return "ptrptr"
    if re.fullmatch(r"HsByteBuffer\s*\*", ty):
        return "bufptr"
    if re.fullmatch(r"uint32_t", ty):
        return "u32"
    if re.fullmatch(r"uint64_t", ty):
        return "u64"
    if re.fullmatch(r"size_t", ty):
        return "usize"
    if re.fullmatch(r"int32_t", ty):
        return "i32"
    if re.fullmatch(r"HsByteBuffer", ty):
        return "buf"
    if re.fullmatch(r"HsCallResult", ty):
        return "buf"
    if re.fullmatch(r"void", ty):
        return "void"
    return "?" + ty


def split_top(s: str, sep: str):
    parts, depth, cur = [], 0, ""
    for ch in s:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == sep and depth == 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    return parts


def rust_signatures(src: str):
    """name -> (param categories, return category) for every hs_* fn."""
    sigs = {}
    pos = 0
    while True:
        m = RUST_FN_RE.search(src, pos)
        if not m:
            break
        name = m.group(1)
        depth, j = 1, m.end()
        while depth > 0:
            if src[j] == "(":
                depth += 1
            elif src[j] == ")":
                depth -= 1
            j += 1
        params_str = src[m.end() : j - 1]
        ret = "void"
        rm = RUST_RET_RE.search(src, j)
        if rm and rm.start() < src.find("{", j):
            ret = rust_category(rm.group(1))
        params = []
        for p in split_top(params_str, ","):
            p = p.strip()
            pm = re.match(r"(?:mut\s+)?[A-Za-z_][A-Za-z0-9_]*\s*:\s*(.*)", p)
            params.append(rust_category(pm.group(1)) if pm else rust_category(p))
        sigs[name] = (params, ret)
        pos = m.end()
    return sigs


def header_signatures(hdr: str):
    """name -> (param categories, return category) for every hs_* prototype."""
    sigs = {}
    pos = 0
    while True:
        m = C_FN_RE.search(hdr, pos)
        if not m:
            break
        ret_type, name = m.group(1), m.group(2)
        depth, j = 1, m.end()
        while depth > 0:
            if hdr[j] == "(":
                depth += 1
            elif hdr[j] == ")":
                depth -= 1
            j += 1
        params_str = hdr[m.end() : j - 1]
        params = []
        for p in split_top(params_str, ","):
            p = p.strip()
            if p == "void":
                continue  # C `(void)` == no parameters
            pm = re.match(r"^(.*?)([A-Za-z_][A-Za-z0-9_]*)$", p)
            params.append(c_category(pm.group(1)) if pm else c_category(p))
        sigs[name] = (params, c_category(ret_type))
        pos = m.end()
    return sigs


def abi_constants(src: str):
    def one(name):
        m = re.search(r"ABI_VERSION_%s:\s*u32\s*=\s*(\d+)" % name, src)
        return m.group(1) if m else None

    return one("MAJOR"), one("MINOR"), one("PATCH")


def snapshot_text(version, rust):
    lines = [
        "# handshaker-ffi ABI snapshot (generated by scripts/check-ffi-abi.py;",
        "# do not edit by hand — run `scripts/generate-ffi-header.sh --update`)",
        "",
        "ABI: " + version,
        "",
    ]
    for name in sorted(rust):
        params, ret = rust[name]
        lines.append("%s(%s) -> %s" % (name, ",".join(params), ret))
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lib", required=True)
    ap.add_argument("--header", required=True)
    ap.add_argument("--snapshot", help="regenerate snapshot file (update mode)")
    ap.add_argument("--check-snapshot", help="compare snapshot file (check mode)")
    args = ap.parse_args()

    # lib.rs may declare submodules (`mod files;` etc.); exported symbols
    # live in the same src/ directory, so scan every *.rs next to lib.rs.
    lib_dir = os.path.dirname(args.lib)
    extra = sorted(
        os.path.join(lib_dir, f)
        for f in os.listdir(lib_dir)
        if f.endswith(".rs") and f != os.path.basename(args.lib)
    )
    src = "\n".join(
        open(path, encoding="utf-8").read() for path in [args.lib, *extra]
    )
    hdr = open(args.header, encoding="utf-8").read()

    rust = rust_signatures(src)
    header = header_signatures(hdr)

    errors = []
    # 1+2. symbols and signatures
    for name in sorted(rust):
        if name not in header:
            errors.append("missing header prototype: %s" % name)
            continue
        rp, rr = rust[name]
        hp, hr = header[name]
        if rp != hp:
            errors.append(
                "signature mismatch %s: rust(%s) != header(%s)"
                % (name, ",".join(rp), ",".join(hp))
            )
        if rr != hr:
            errors.append(
                "return mismatch %s: rust(%s) != header(%s)" % (name, rr, hr)
            )
    for name in sorted(header):
        if name not in rust:
            errors.append("header prototype without rust export: %s" % name)

    # 3. ABI version constants vs header comment.
    maj, minor, patch = abi_constants(src)
    version = "%s.%s.%s" % (maj, minor, patch)
    header_top = "\n".join(hdr.splitlines()[0:12])
    if "ABI version: %s" % version not in header_top:
        errors.append(
            "header comment does not state 'ABI version: %s' (constants: %s)"
            % (version, (maj, minor, patch))
        )

    if errors:
        for e in errors:
            print("ABI CHECK FAILED: %s" % e, file=sys.stderr)
        sys.exit(1)

    text = snapshot_text(version, rust)
    if args.snapshot:
        with open(args.snapshot, "w", encoding="utf-8") as f:
            f.write(text)
        print("abi ok: %d exports, snapshot written to %s" % (len(rust), args.snapshot))
    elif args.check_snapshot:
        try:
            existing = open(args.check_snapshot, encoding="utf-8").read()
        except FileNotFoundError:
            print(
                "ABI CHECK FAILED: snapshot %s missing; run "
                "`scripts/generate-ffi-header.sh --update`" % args.check_snapshot,
                file=sys.stderr,
            )
            sys.exit(1)
        if existing != text:
            print(
                "ABI CHECK FAILED: snapshot %s out of date; run "
                "`scripts/generate-ffi-header.sh --update`" % args.check_snapshot,
                file=sys.stderr,
            )
            sys.exit(1)
        print("abi ok: %d exports, header/snapshot in sync" % len(rust))
    else:
        print("abi ok: %d exports" % len(rust))


if __name__ == "__main__":
    main()
