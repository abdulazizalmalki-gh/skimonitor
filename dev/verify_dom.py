#!/usr/bin/env python3
"""Parity check #2: what the browser actually built vs the frame it was sent.

`verify_parity.py` proves the server's cells equal the TUI's cells. This proves
the page turns those cells into the same grid: every text character, every
geometry-painted cell (blocks, eighths, shade) and every braille cell occupies
exactly one cell in the row, in the same order.

Run it against a FROZEN dashboard (POST /api/key {"k":"c","v":"p"} first), so
the frame the page renders and the frame fetched for comparison carry the same
data; digits are normalized because the "Xs ago" counters still tick.

usage: verify_dom.py <dom.html> <screen.json>
"""
import json
import sys
from html.parser import HTMLParser

BLOCKS = set("▁▂▃▄▅▆▇█░▏▎▍▌▋▊▉")
GEOM = ("ge", "br")


def is_special(ch):
    return ch in BLOCKS or 0x2800 <= ord(ch) <= 0x28FF


class Rows(HTMLParser):
    """Collects each .row's cells: one per character, one per painted span."""

    def __init__(self):
        super().__init__()
        self.rows = []
        self.cur = None
        self.depth = 0
        self.stack = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        cls = (a.get("class") or "").split()
        if tag == "div" and "row" in cls:
            self.cur = []
            self.rows.append(self.cur)
            self.depth = 1
            self.stack = []
            return
        if self.cur is None:
            return
        self.depth += 1
        painted = tag == "span" and any(c in cls for c in GEOM)
        self.stack.append(painted)
        if painted:
            self.cur.append("\x00")          # the span IS one painted cell

    def handle_endtag(self, tag):
        if self.cur is None:
            return
        if self.stack:
            self.stack.pop()
        self.depth -= 1
        if self.depth == 0:
            self.cur = None
            self.stack = []

    def handle_data(self, data):
        if self.cur is None or any(self.stack):
            return
        self.cur.extend(list(data))


def norm(s):
    out = []
    for ch in s:
        if ch == "\x00" or is_special(ch):
            out.append("\x00")
        elif ch.isdigit():
            out.append("#")
        else:
            out.append(ch)
    return "".join(out)


def main():
    dom_path, json_path = sys.argv[1], sys.argv[2]
    parser = Rows()
    parser.feed(open(dom_path, encoding="utf-8").read())
    data = json.load(open(json_path, encoding="utf-8"))
    cols, nrows = data["cols"], data["rows"]
    want = [norm("".join(t for t, _ in runs))[:cols] for runs in data["lines"]]

    got = [norm("".join(r)) for r in parser.rows]
    print(f"dom rows={len(got)} server grid={cols}x{nrows}")
    bad = 0
    for y in range(min(len(got), len(want))):
        if got[y] != want[y]:
            bad += 1
            if bad <= 6:
                print(f"row {y + 1} mismatch (dom {len(got[y])} cells, expected {len(want[y])})")
                print(f"  dom|{got[y].rstrip()}")
                print(f"  exp|{want[y].rstrip()}")
    if len(got) != len(want):
        print(f"ROW COUNT: dom {len(got)} expected {len(want)}")
        bad += abs(len(got) - len(want))
    widths = sorted({len(r) for r in got})
    print(f"dom row widths={widths} grid cols={cols}")
    ok = bad == 0
    print("DOM PARITY: " + ("identical" if ok else f"{bad} row(s) differ"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
