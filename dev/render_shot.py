#!/usr/bin/env python3
"""Render capture.json (a pyte screen dump with per-cell fg/bg/bold) to a
beautiful PNG: JetBrains Mono glyphs, programmatic braille dots, GitHub-dark
palette, rounded terminal window with traffic lights + title bar.

Usage: python3 dev/render_shot.py [capture.json] [out.png]
Needs a monospace font with a Bold weight; point SKIMO_FONT_JETBRAINSMONO_REGULAR
(or drop the TTFs into assets/) if the system search paths come up empty.
"""
import json, sys, os
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
CAP = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "capture.json")
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "..", "docs", "screenshot.png")

FS       = 17            # font px size
SCALE    = 2             # supersample for crisp glow/AA
LINE_H   = int(FS * 1.46)
CELL_W   = int(FS * 0.605)
PAD_X    = 26
PAD_TOP  = 58            # title bar
PAD_BOT  = 22
MARGIN   = 34            # outer bg margin (for the drop shadow composition)

BG       = (13, 17, 23)      # window bg  (github dark dimmed)
BG_DOT   = (10, 13, 18)
TITLE_BG = (24, 29, 37)
FG_DEF   = (201, 209, 217)   # default text
BORDER   = (48, 54, 61)

# ANSI 16 / 256 palette for the names pyte emits
NAMED = {
    "black": (0, 0, 0), "maroon": (128, 0, 0), "green": (0, 128, 0),
    "brown": (128, 128, 0), "olive": (128, 128, 0), "blue": (0, 0, 238),
    "purple": (128, 0, 128), "cyan": (0, 255, 255), "gray": (229, 229, 229),
    "grey": (229, 229, 229), "darkgray": (127, 127, 127), "darkgrey": (127, 127, 127),
    "lightgreen": (144, 238, 144), "yellow": (238, 238, 0), "lightblue": (200, 200, 255),
    "lightcyan": (224, 255, 255), "lightgray": (211, 211, 211), "lightgrey": (211, 211, 211),
    "pink": (255, 165, 0), "red": (205, 0, 0), "magenta": (205, 0, 205),
    "white": (255, 255, 255),
}

def hexcol(x):
    if x == "default":
        return None
    if not isinstance(x, str):
        return None
    if len(x) == 6:
        try:
            return (int(x[0:2], 16), int(x[2:4], 16), int(x[4:6], 16))
        except ValueError:
            pass
    return NAMED.get(x.lower())

def _find_font(name):
    """Locate a mono TTF: env override, then common font dirs + repo assets/."""
    env = os.environ.get("SKIMO_FONT_" + name.replace("-", "_").upper())  # e.g. SKIMO_FONT_JETBRAINSMONO_REGULAR
    if env and os.path.exists(os.path.expanduser(env)):
        return os.path.expanduser(env)
    cands = []
    for root in [os.path.join(HERE, "..", "assets"), "/usr/share/fonts",
                 "/usr/local/share/fonts", os.path.expanduser("~/.fonts"),
                 os.path.expanduser("~/.local/share/fonts")]:
        for dirpath, _, files in os.walk(os.path.expanduser(root)):
            for f in files:
                if name.lower() in f.lower() and f.lower().endswith((".ttf", ".otf")):
                    cands.append(os.path.join(dirpath, f))
    if not cands:
        sys.exit(f"font containing '{name}' not found; set SKIMO_FONT_{name.upper()}=path")
    return cands[0]

FONT_PATH = _find_font("JetBrainsMono-Regular")
FONT_BOLD = _find_font("JetBrainsMono-Bold")

def is_braille(ch):
    return 0x2800 <= ord(ch) <= 0x28FF

def draw_braille(d, ch, x, y, col, bold):
    """dot-matrix braille: bits map 2 cols x 4 rows inside the cell"""
    b = ord(ch) - 0x2800
    # dot positions (col,row) -> bit
    layout = [
        ((0, 0), 0x01), ((0, 1), 0x02), ((0, 2), 0x04), ((0, 3), 0x08),
        ((1, 0), 0x10), ((1, 1), 0x20), ((1, 2), 0x40), ((1, 3), 0x80),
        ((0, 4), 0x400), ((1, 4), 0x800),
    ]
    cw, chh = CELL_W / 2.0, LINE_H / 5.0
    r = FS * (0.088 if not bold else 0.10)
    for (ci, ri), bit in layout:
        if b & bit:
            cx = x + cw * (ci + 0.5)
            cy = y + chh * (ri + 0.75)
            d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=col)

def rounded_rect(d, box, rad, fill=None, outline=None, width=1):
    d.rounded_rectangle(box, radius=rad, fill=fill, outline=outline, width=width)

def main():
    rows = json.load(open(CAP))
    H, W = len(rows), len(rows[0])
    # crop trailing empty rows (keep the status line: find last row w/ text)
    last = 0
    for y in range(H - 1, -1, -1):
        if any(c[0].strip() for c in rows[y]):
            last = y
            break
    rows = rows[: last + 1]
    H = len(rows)

    ss = SCALE
    img_w = (W * CELL_W + PAD_X * 2 + MARGIN * 2) * ss
    img_h = (H * LINE_H + PAD_TOP + PAD_BOT + MARGIN * 2) * ss
    img = Image.new("RGB", (img_w, img_h), (10, 13, 17))
    d = ImageDraw.Draw(img)

    R = lambda v: v * ss
    font  = ImageFont.truetype(FONT_PATH, FS * ss)
    fontb = ImageFont.truetype(FONT_BOLD, FS * ss)

    win = (R(MARGIN), R(MARGIN), img_w - R(MARGIN), img_h - R(MARGIN))
    rad = R(14)
    # drop shadow
    sh = Image.new("RGBA", img.size, (0, 0, 0, 0))
    sd = ImageDraw.Draw(sh)
    for i, a in [(8, 40), (5, 60), (2, 80)]:
        sd.rounded_rectangle([win[0] + R(i), win[1] + R(i + 4), win[2] + R(i), win[3] + R(i + 4)],
                             radius=rad, fill=(0, 0, 0, a))
    img = Image.alpha_composite(img.convert("RGBA"), sh).convert("RGB")
    d = ImageDraw.Draw(img)

    # window + title bar
    rounded_rect(d, win, rad, fill=BG, outline=BORDER, width=R(1))
    d.rounded_rectangle([win[0], win[1], win[2], win[1] + R(PAD_TOP)], radius=rad, fill=TITLE_BG)
    d.rectangle([win[0], win[1] + R(PAD_TOP // 2), win[2], win[1] + R(PAD_TOP)], fill=TITLE_BG)
    d.line([win[0], win[1] + R(PAD_TOP), win[2], win[1] + R(PAD_TOP)], fill=BORDER, width=1)
    for i, c in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        cx = win[0] + R(PAD_X + 10 + i * 26)
        cy = win[1] + R(PAD_TOP // 2)
        d.ellipse([cx - R(7), cy - R(7), cx + R(7), cy + R(7)], fill=c)
    tl = "skimonitor — ssh server stats at a glance"
    tb = d.textbbox((0, 0), tl, font=ImageFont.truetype(FONT_PATH, 13 * ss))
    d.text(((img_w - (tb[2] - tb[0])) // 2, win[1] + R(PAD_TOP // 2 - 13)), tl,
           fill=(139, 148, 158), font=ImageFont.truetype(FONT_PATH, 13 * ss))

    ox = win[0] + R(PAD_X)
    oy = win[1] + R(PAD_TOP)

    # pass 1: backgrounds (so bar glow sits under text)
    for y, row in enumerate(rows):
        for x, (ch, fg, bg, bold) in enumerate(row):
            bc = hexcol(bg)
            if bc:
                d.rectangle([ox + R(x) * CELL_W, oy + R(y) * LINE_H,
                             ox + R(x + 1) * CELL_W, oy + R(y + 1) * LINE_H], fill=bc)

    # pass 2: foregrounds
    for y, row in enumerate(rows):
        for x, (ch, fg, bg, bold) in enumerate(row):
            if ch in (" ", ""):
                continue
            fc = hexcol(fg) or FG_DEF
            if ch == "█":
                d.rectangle([ox + R(x) * CELL_W, oy + R(y) * LINE_H,
                             ox + R(x + 1) * CELL_W, oy + R(y + 1) * LINE_H], fill=fc)
            elif ch == "░":
                # light shade: fine dot grid, reads as an empty bar segment
                bx0, by0 = ox + R(x) * CELL_W + 1, oy + R(y) * LINE_H + 1
                bx1, by1 = ox + R(x + 1) * CELL_W - 1, oy + R(y + 1) * LINE_H - 1
                st = max(4, R(4))
                dim = tuple(int(c * 0.55) for c in fc)
                for gy in range(int(by0), int(by1), st):
                    for gx in range(int(bx0), int(bx1), st):
                        d.rectangle([gx, gy, min(gx + st // 2, bx1), min(gy + st // 2, by1)], fill=dim)
            elif ch in "▏▎▍▌▋▊▉":
                fr = (0x2590 - ord(ch)) / 8.0  # ▏=1/8 .. ▉=7/8
                bx0 = ox + R(x) * CELL_W
                frac_box = [bx0, oy + R(y) * LINE_H,
                            bx0 + max(1, (R(CELL_W)) * fr), oy + R(y + 1) * LINE_H]
                d.rectangle(frac_box, fill=fc)
            elif ch == "▁":
                d.rectangle([ox + R(x) * CELL_W + 1, oy + R(y + 1) * LINE_H - R(LINE_H * 0.16),
                             ox + R(x + 1) * CELL_W - 1, oy + R(y + 1) * LINE_H - 1], fill=fc)
            elif ch == "▂":
                d.rectangle([ox + R(x) * CELL_W + 1, oy + R(y + 1) * LINE_H - R(LINE_H * 0.32),
                             ox + R(x + 1) * CELL_W - 1, oy + R(y + 1) * LINE_H - 1], fill=fc)
            elif ch in "▃▄▅▆▇":
                lvl = {"▃": .44, "▄": .56, "▅": .68, "▆": .8, "▇": .92}[ch]
                d.rectangle([ox + R(x) * CELL_W + 1, oy + R(y + 1) * LINE_H - R(LINE_H * lvl),
                             ox + R(x + 1) * CELL_W - 1, oy + R(y + 1) * LINE_H - 1], fill=fc)
            elif ch == "■":
                d.rectangle([ox + R(x) * CELL_W + R(2), oy + R(y) * LINE_H + R(3),
                             ox + R(x + 1) * CELL_W - R(2), oy + R(y + 1) * LINE_H - R(3)], fill=fc)
            elif is_braille(ch):
                draw_braille(d, ch, ox + R(x) * CELL_W, oy + R(y) * LINE_H, fc, bold)
            else:
                d.text((ox + R(x) * CELL_W, oy + R(y) * LINE_H + R(1)), ch,
                       fill=fc, font=fontb if bold else font)

    img.resize((img_w // ss, img_h // ss), Image.Resampling.LANCZOS).save(OUT)
    print("wrote", OUT, img_w // ss, "x", img_h // ss)

if __name__ == "__main__":
    main()
