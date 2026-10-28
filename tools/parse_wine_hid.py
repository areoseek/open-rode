#!/usr/bin/env python3
"""Pair OUT/IN HID reports from a WINEDEBUG=+hid trace of RODE Central.

usage: parse_wine_hid.py rc_hid.log [--all] [--since N] [--nopoll]
  default: unique (request -> reply) pairs with counts, first-seen order
  --all:    every exchange in order
  --since N: skip the first N exchanges (to isolate what a UI click did)
  --nopoll: hide the idle polling requests

Wine's trace is written by several threads at once, so hex-dump lines of one
report can interleave with another thread's lines: state is kept per thread id
(the 4 hex digits at the start of every line).
"""
import re
import sys

LINE = re.compile(r"^([0-9a-f]{4}):trace:hid:(\w+) (.*)$")
HEX = re.compile(r"^([0-9a-f]{8})  ((?:[0-9a-f]{2} ?)+)$")
OUT = re.compile(r"write output report id (\d+) length (\d+):")
IN = re.compile(r"input report length (\d+):")
POLL = re.compile(r"^(09 (00|01|04|05|06|0b|0c|0f) 01 00|01 (36|77) 00)")


def parse(path):
    """Return (outs, ins) as lists of bytes, in file order."""
    outs, ins = [], []
    cur = {}  # thread id -> [kind, expected length, bytearray]
    for line in open(path, errors="replace"):
        m = LINE.match(line.rstrip())
        if not m:
            continue
        tid, func, rest = m.groups()
        mo = OUT.search(rest)
        if mo:
            cur[tid] = ["out", int(mo.group(2)), bytearray()]
            continue
        mi = IN.search(rest)
        if mi:
            cur[tid] = ["in", int(mi.group(1)), bytearray()]
            continue
        c = cur.get(tid)
        if c is None:
            continue
        mh = HEX.match(rest)
        if not mh:
            cur.pop(tid, None)
            continue
        c[2] += bytes.fromhex(mh.group(2).replace(" ", ""))
        if len(c[2]) >= c[1]:
            (outs if c[0] == "out" else ins).append(bytes(c[2][: c[1]]))
            cur.pop(tid, None)


    return outs, ins


def pair(outs, ins):
    """Reply report id == request id + 1 and reply[1] == request opcode."""
    pairs, j = [], 0
    for o in outs:
        reply = None
        for k in range(j, min(j + 16, len(ins))):
            r = ins[k]
            if r[0] == o[0] + 1 and r[1] == o[1]:
                reply, j = r, k + 1
                break
        pairs.append((o, reply))
    return pairs


def trim(b):
    """First 6 bytes verbatim, then zero padding stripped."""
    head, tail = b[:6].hex(" "), b[6:].hex(" ")
    tail = re.sub(r"( ?00)+$", "", tail)
    return (head + " " + tail).strip()


def main():
    args = sys.argv[1:]
    show_all = "--all" in args
    nopoll = "--nopoll" in args
    since = int(args[args.index("--since") + 1]) if "--since" in args else 0
    path = [a for a in args if not a.startswith("--") and not a.isdigit()][0]
    outs, ins = parse(path)
    pairs = pair(outs, ins)[since:]
    print(f"# {len(outs)} out, {len(ins)} in, showing from exchange {since}")
    if show_all:
        for i, (o, r) in enumerate(pairs, since):
            if nopoll and POLL.match(trim(o)):
                continue
            print(f"{i:5d}  {trim(o):<44} -> {trim(r) if r else '(no reply)'}")
        return
    seen = {}
    for o, r in pairs:
        key = (trim(o), trim(r) if r else None)
        seen[key] = seen.get(key, 0) + 1
    for (o, r), n in seen.items():
        if nopoll and POLL.match(o):
            continue
        print(f"{n:4d}x  {o:<44} -> {r if r else '(no reply)'}")


if __name__ == "__main__":
    main()
