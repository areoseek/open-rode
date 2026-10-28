#!/usr/bin/env python3
"""Read-only sweep of the report-9 GET family: 09 <op> 01 -> 0a <op> <ack> <data>.

RODE Central itself uses byte[2]=0x01 as GET for a dozen opcodes, so this
only reads. Skips opcodes passed via --skip (default 0x0d, which RODE Central
drives with a different byte[2]).
usage: sweep_get.py [--lo 0] [--hi 0x3f] [--skip 0d,..] [--pid 002a]
"""
import argparse
import hid
import time

VID = 0x19F7


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lo", type=lambda x: int(x, 0), default=0)
    ap.add_argument("--hi", type=lambda x: int(x, 0), default=0x3F)
    ap.add_argument("--skip", default="0d")
    ap.add_argument("--pid", type=lambda x: int(x, 16), default=None)
    a = ap.parse_args()
    skip = {int(s, 16) for s in a.skip.split(",") if s}

    devs = [d for d in hid.enumerate(VID) if a.pid is None or d["product_id"] == a.pid]
    if len(devs) != 1:
        raise SystemExit(f"need exactly one device, found {len(devs)}: {[hex(d['product_id']) for d in devs]}")
    h = hid.device()
    h.open_path(devs[0]["path"])
    h.set_nonblocking(False)
    for op in range(a.lo, a.hi + 1):
        if op in skip:
            continue
        frame = bytes([0x09, op, 0x01]) + bytes(34)
        h.write(frame)
        r = bytes(h.read(64, 1500))
        extra = []
        while True:
            x = bytes(h.read(64, 150))
            if not x:
                break
            extra.append(x)
        status = {0x41: "ACK", 0x4E: "NAK"}.get(r[2] if len(r) > 2 else -1, "?")
        data = r[3:].hex(" ").rstrip("0 ").strip() if r else "(no reply)"
        print(f"09 {op:02x} 01 -> {status:3} {data}" + ("".join(f"\n            +{e.hex(' ')}" for e in extra)))
        time.sleep(0.05)
    h.close()


if __name__ == "__main__":
    main()
