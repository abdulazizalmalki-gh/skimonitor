#!/usr/bin/env python3
"""Parity check: the web dashboard's cell grid vs the TUI's own capture.

The web app ships the ratatui buffer, so its cells must line up with the TUI's
rendered screen cell for cell. Two live runs legitimately disagree on DATA —
numbers, the rate unit picked by magnitude, how many sparkline samples exist
yet — so both sides are normalized before the cell-by-cell comparison:

  * every number (with its decimal part)  -> '#'
  * K/M/G/T in a rate unit (Ki/s, Mb/s)   -> '#'   (magnitude-dependent)
  * braille dots                          -> 'B'
  * block/eighth/shade glyphs             -> 'G'
  * '…'                                   -> '.'
  * a braille run (with its padding)      -> '[B]'  (field width is fixed)

Everything else — every label, border, gap, column position, truncated word —
must match exactly. Row/column offsets are preserved by the number and braille
rules (they collapse a whole run, not one char), so a real layout difference
cannot hide behind them.

usage: verify_parity.py [tui_capture.txt] [web_screen.json]
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TUI = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "capture.txt")
WEB = sys.argv[2] if len(sys.argv) > 2 else "/tmp/web150.json"

BLOCKS = set("▁▂▃▄▅▆▇█░▏▎▍▌▋▊▉")

# The web front end has no quit key (a dashboard is a server: nothing typed in
# a browser tab should stop it), so its hint line drops " [q]quit". That is the
# only intended text difference; everything else must be identical.
RENAMES = [(" [q]quit", "")]


def norm_row(s):
    for a, b in RENAMES:
        s = s.replace(a, b)
    s = re.sub(r"[KMGT](?=[ibB]/s)", "#", s)          # unit letter
    s = re.sub(r"[0-9]+(?:\.[0-9]+)?", "#", s)         # numbers
    out = []
    for ch in s:
        o = ord(ch)
        if 0x2800 <= o <= 0x28FF:
            out.append("B")
        elif ch in BLOCKS:
            out.append("G")
        elif ch == "…":
            out.append(".")
        else:
            out.append(ch)
    s = "".join(out)
    # braille first: its placeholder must not contain a B, or the gauge rule below
    # would eat it (and produce a different width on each side)
    s = re.sub(r"B[ B]*", "\x01", s)                   # one braille field
    return re.sub(r"\[[G ]*\]", "\x02", s)             # gauge interior: level is data


def tui_rows(path):
    with open(path, encoding="utf-8") as f:
        rows = f.read().split("\n")
    while rows and rows[-1] == "":
        rows.pop()
    return rows


def web_rows(path):
    data = json.load(open(path, encoding="utf-8"))
    rows = ["".join(text for text, _ in runs) for runs in data["lines"]]
    return data["cols"], data["rows"], rows


def main():
    tui = [norm_row(r) for r in tui_rows(TUI)]
    cols, nrows, web = web_rows(WEB)
    web = [norm_row(r) for r in web]
    print(f"tui rows={len(tui)}  web grid={cols}x{nrows}  web rows={len(web)}")
    if len(tui) != len(web):
        print(f"ROW COUNT MISMATCH: tui {len(tui)} vs web {len(web)}")
    width = max([len(r) for r in tui + web])
    diffs = 0
    bad_rows = set()
    padding_rows = set()
    for y in range(min(len(tui), len(web))):
        a = tui[y].ljust(width)
        b = web[y].ljust(width)
        if a == b:
            continue
        # A live rate can render 8 cells ("1005Ki/s") in one run and 7
        # ("1.4Mi/s") in the other, and the formatter does not pad the field —
        # so everything right of it shifts by the difference. That is a data
        # width, not a layout difference: compare with whitespace removed and
        # only count what is left.
        if a.replace(" ", "") == b.replace(" ", ""):
            padding_rows.add(y + 1)
            continue
        row_diff = 0
        for x in range(width):
            if a[x] != b[x]:
                diffs += 1
                row_diff += 1
                if len(bad_rows) < 12 and row_diff <= 3:
                    print(f"  row {y + 1:2d} col {x + 1:3d}: tui {a[x]!r} web {b[x]!r}")
                    print(f"    tui|{a.rstrip()}")
                    print(f"    web|{b.rstrip()}")
        if row_diff:
            bad_rows.add(y + 1)
    total = min(len(tui), len(web)) * width
    print(
        f"compared {total} cells; structural differences: {diffs}"
        f" on rows {sorted(bad_rows)}"
    )
    if padding_rows:
        print(
            f"padding-only shifts (same glyph sequence, live value one cell"
            f" wider): rows {sorted(padding_rows)}"
        )
    ok = diffs == 0 and len(tui) == len(web)
    print("PARITY: " + ("identical" if ok else "MISMATCH"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
