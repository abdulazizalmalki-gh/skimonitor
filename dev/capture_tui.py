#!/usr/bin/env python3
"""Capture skimonitor --demo over a pty (real render path) and save screen+attrs."""
import os, pty, subprocess, time, select, struct, fcntl, termios, json, sys

W, H = int(sys.argv[1]), int(sys.argv[2])
BIN = os.environ.get("SKIMO_BIN", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "skimonitor"))
mfd, sfd = pty.openpty()
fcntl.ioctl(sfd, termios.TIOCSWINSZ, struct.pack("HHHH", H, W, 0, 0))
p = subprocess.Popen([BIN, "--demo", "-i", "1"], stdin=sfd, stdout=sfd, stderr=sfd,
                     env=dict(os.environ, TERM="xterm-256color"), close_fds=True)
os.close(sfd)

acc = b""
def drain(t):
    global acc
    e = time.time() + t
    while time.time() < e:
        r, _, _ = select.select([mfd], [], [], 0.2)
        if r:
            try:
                acc += os.read(mfd, 65536)
            except OSError:
                break

drain(int(sys.argv[3]) if len(sys.argv) > 3 else 16)   # frames: rates + sparklines populated
import pyte
sc = pyte.Screen(W, H)
pyte.ByteStream(sc).feed(acc)

rows = []
for y in range(H):
    line = sc.buffer[y]
    row = []
    for x in range(W):
        c = line.get(x)
        d = c.data if c and c.data != "" else " "
        row.append([d, c.fg if c else "default", c.bg if c else "default",
                    bool(c.bold) if c else False])
    rows.append(row)

text = "\n".join("".join(r[0] for r in row).rstrip() for row in rows)
here = os.path.dirname(os.path.abspath(__file__))
open(os.path.join(here, "capture.json"), "w").write(json.dumps(rows))
open(os.path.join(here, "capture.txt"), "w").write(text)
print(text)
assert "panic" not in acc.decode("utf-8", "replace").lower(), "panic in stream"
p.terminate(); p.wait()
