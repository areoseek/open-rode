#!/usr/bin/env python3
"""Click RODE Central controls (under Wine) via the uinput tablet and print the
HID exchanges each click caused, minus the known polling noise.

usage: click_and_diff.py LOG "label:x:y" ["label:x:y" ...]
"""
import re
import subprocess
import sys
import time

sys.path.insert(0, "/home/areoseek/cs2-tradeup")
from executor.vio import VirtualInput  # noqa: E402

PARSER = "/home/areoseek/rode-central-linux/tools/parse_wine_hid.py"
POLL = re.compile(r"^(09 (00|01|04|05|06|0b|0c|0f) 01 00|01 (36|77) 00)")


def count(log):
    """Number of fully parsed OUT frames so far (same counting as the parser)."""
    sys.path.insert(0, "/home/areoseek/rode-central-linux/tools")
    from parse_wine_hid import parse
    return len(parse(log)[0])


def diff(log, since):
    out = subprocess.run(
        ["python3", "-I", PARSER, log, "--all", "--since", str(since)],
        capture_output=True, text=True,
    ).stdout.splitlines()[1:]
    rows = []
    for l in out:
        m = re.match(r"\s*\d+\s+(.*?)\s+->\s+(.*)$", l)
        if not m:
            continue
        req, rep = m.group(1).strip(), m.group(2).strip()
        if POLL.match(req):
            continue
        rows.append(f"    {req:<40} -> {rep}")
    return rows


def main():
    log = sys.argv[1]
    v = VirtualInput()
    try:
        for spec in sys.argv[2:]:
            label, x, y = spec.rsplit(":", 2)
            before = count(log)
            v.click(int(x), int(y))
            time.sleep(2.0)
            rows = diff(log, before)
            print(f"== {label}  (@{x},{y})  {count(log) - before} exchanges")
            print("\n".join(rows) if rows else "    (only polling)")
            sys.stdout.flush()
        v.move(1700, 1050)
    finally:
        v.close()


if __name__ == "__main__":
    main()
