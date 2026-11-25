# skimonitor

**One terminal. Up to six servers. Everything at a glance.**

skimonitor is a multi-host SSH monitoring TUI written in Rust. Point it at your machines and it streams a single dense dashboard — CPU, memory, disks (including ZFS truth), network, temperatures, and GPUs with per-process VRAM — side by side, refreshed every second. No agents, no daemon, no database: just SSH keys and one static binary. Prefer a browser tab to a terminal? `--web` serves the exact same screen over HTTP.

![skimonitor dashboard](docs/screenshot.png)

*(The screenshot above is `--demo` mode: three invented hosts replayed through the exact same render pipeline as live data — nothing here comes from a real machine.)*

## Why

Most fleet monitors want you to install a collector on every box, run a server, and open ports. skimonitor wants one thing: that you can already `ssh` to the host. It runs a small self-contained probe over a persistent SSH connection, streams one JSON frame per second back, and renders up to six hosts on one screen at terminal size. It is the tool you start when `htop` on five tabs and `watch -n1 df` on three terminals stop being manageable — but a full observability stack is overkill.

## Features

- **Zero install on targets** — the probe is embedded in the binary and shipped over SSH; the target only needs `bash` + coreutils (no jq, no python, no root except for ZFS pool stats).
- **True rates, honestly computed** — CPU %, disk R/W, and net ↓↑ are derived by diffing cumulative counters between consecutive frames, so nothing is sampled-and-hoped.
- **GPU visibility that actually helps** — three symmetric bars per card, stacked one on top of the other: compute %, VRAM, and power (watts as % of cap) — each with its own live trend sparkline on the same 0–100 scale, plus temp/fan, and every compute process with its **full command line** (read from `/proc/<pid>/cmdline`, not nvidia-smi's truncated name) and per-process VRAM. Driver-less hosts (e.g. Proxmox with cards passed through to VMs) still list GPUs via PCI sysfs presence.
- **Inference readout per GPU process** — under each compute process's command line, a compact line found by probing the ports that process owns (fd socket inodes; container ports translated via `docker ps`; argv `--port` hints otherwise): engine kind + endpoint, model, decode and prefill tok/s (live from the engine's own rolling gauge; `~` prefixed lifetime averages when idle), draft accept %, prefix-cache hit %, KV usage, context, running/queued. llama.cpp (prometheus and legacy JSON), vLLM, SGLang, TGI, Strata and Ollama-style health shapes are recognized; a server started without `--metrics` says so instead of showing zeros. Probing is read-only GETs to localhost on a ~6s cached cycle with a hard time budget — it never slows the 1s frames.
- **ZFS truth, not `df` lies** — pool rows use real device usage (`zpool list -p`), and each dataset with its own limit (quota / refquota / volsize — what Proxmox actually enforces on subvols and VM disks) gets its own row against that limit.
- **Per-core load with topology** — one block per logical CPU, hyperthread siblings glued together, physical cores spaced, frequency range on the right.
- **Six hosts on one screen** — the first three sit side by side; add a fourth and the highlighted card splits in two, the new host taking the lower half (three columns, two rows max). Every card stays visible and scrollable — nothing is squeezed out.
- **Nothing silently truncated** — cards scroll (keys or mouse wheel); a GPU process with a 300-character command line is wrapped and shown in full. The title bar tells you exactly which lines you're looking at (`↕7-42/58`).
- **Self-healing streams** — dead SSH connections are detected, respawned with capped exponential backoff, and a silence watchdog reconnects streams that stop answering. Auth failures as your user automatically retry as `root@host`.
- **`--demo` mode** — three invented hosts replayed through the real code path, for screenshots, testing, and showing your team what the tool does. No SSH, no data, no risk.
- **The same dashboard in a browser** — `--web` serves it over HTTP instead of drawing a TUI. Not a re-design: the server paints the *identical* ratatui buffer and ships the cells to the page, so the browser shows the terminal screen glyph for glyph (see [Web dashboard](#web-dashboard---web)).

## Install

Grab the latest release binary for your platform:

```
https://github.com/abdulazizalmalki-gh/skimonitor/releases
```

Or build from source (Rust stable, no other toolchain dependencies):

```
cargo build --release        # -> target/release/skimonitor
```

## Quick start

```
skimonitor web-01 db-01 cache-02        # three hosts, 1 Hz
skimonitor web-01 db-01 cache-02 db-02  # fourth splits the highlighted card
skimonitor local                        # watch this machine
skimonitor -i 2 user@203.0.113.9        # 2-second frames over WAN
```

Every argument is a target: an `~/.ssh/config` alias, a hostname, an IP, or `user@host`. `local` self-scans without SSH. Up to six hosts: the first three side by side, then the highlighted card splits and the new host stacks under it. Press `x` to drop one, `a` to add another.

Want to see it before wiring it to anything?

```
skimonitor --demo
```

## Authentication

skimonitor shells out to your system `ssh` client, so it uses exactly the keys that already live on the machine running it: `ssh-agent`, `~/.ssh/config` (aliases, `IdentityFile`), and every private key found in `~/.ssh` — including non-default filenames, which plain `ssh` would never offer. `BatchMode=yes` + publickey-only means it can never hang waiting for a password nobody typed.

Nothing is stored by skimonitor or sent anywhere except the hosts you target. One honest exception, inherited from standard ssh: first-seen host keys are accepted and written to your `~/.ssh/known_hosts` (`accept-new`). skimonitor is also not an ssh sandbox — your own `~/.ssh/config` behavior (proxies, forwarding) applies. If a host refuses your user and you didn't pin one, skimonitor retries once as `root@host` and labels the card `ssh:root`.

## Keys

| key | action |
|-----|--------|
| `a` / `x` | add / remove host |
| `1..3`, `←`/`→`, `Tab` | select host |
| `↑`/`↓`, `PgUp`/`PgDn`, `Home`/`End` | scroll the focused card when it overflows (`↕` shown in its title) |
| mouse wheel | scroll the card under the cursor |
| `+` / `-` | stream interval, 1..60 s |
| `r` | restart all streams |
| `p` | pause (tears down the SSH sessions) / resume |
| `?` / `q` | help / quit (`q` and `Ctrl-C` are terminal-only; a `--web` server is stopped by its operator) |

## Web dashboard (`--web`)

```
skimonitor --web web-01 db-01 cache-02        # http://<this machine>:8383/
skimonitor --web -p 9000 --bind 127.0.0.1 db-01   # loopback only, for a reverse proxy
```

![skimonitor in a browser](docs/web.png)

The page is the terminal screen, not a reinterpretation of it: skimonitor draws
the same `ui::draw` into an off-screen ratatui buffer at the browser's grid
size and ships those cells (glyph, foreground, background, bold) over
Server-Sent Events. The page is a monospace grid that paints them; braille
sparklines and block glyphs are drawn as cell geometry because no common
monospace font has them. Keys, clicks and the wheel post back to the same state
machine the TUI uses, so scrolling, focus, add/drop and pause behave
identically.

Consequences worth knowing before exposing it: the HTTP port has **no login of its own** — anyone who can reach it sees what you would see, including process command lines. Bind it to `127.0.0.1` behind a proxy, or keep it on a network you trust. `q`/`Ctrl-C` cannot stop the server; Esc closes the help overlay.

Endpoints, if you want to wire it into something else: `/` (page), `/events`
(SSE frames), `/api/screen?cols=&rows=` (one JSON frame), `/api/size`
(grid), `/api/{key,click,wheel,add,action}`, `/health`.

Two scripts keep the fidelity claim honest (the demo instance is enough):
`dev/verify_parity.py` compares the served cells against a TUI capture at the
same size, and `dev/verify_dom.py` compares what the browser built against the
frame it was handed. `dev/capture_web.sh` re-shoots `docs/web.png`.

## `--once`: scripted snapshots

```
skimonitor --once local web-01 db-01
```

Probes each target once, prints a one-line summary per host, exits 1 if any failed — handy in cron alerts and deploy scripts.

## What's collected per frame

Load average, overall + per-core CPU % and MHz, CPU/NVME temperatures and sensors, RAM (used / cache / free, swap, live trend sparkline), per-mount disk usage with read/write throughput, NIC rx/tx with sparkline and link speed, and GPUs: symmetric compute/VRAM/power bars each with its own trend sparkline, temp, fan, plus compute processes with full argv and per-process VRAM.

On ZFS hosts the disk section switches to pool + dataset truth (see Features). On Proxmox, VM disks (zvols) and LXC roots (subvols) are shown against the quotas Proxmox actually enforces — not the pool-wide mount numbers `df` reports for every dataset identically.

## How it works

```
┌ your laptop ──────┐        ┌ target host ────────────┐
│ skimonitor (TUI)  │  ssh   │ bash -s << embedded     │
│ rates = diff of   │◄───────│ probe.sh                │
│ two frames @1 Hz  │ stdout │ → 1 JSON line / tick    │
└───────────────────┘        └─────────────────────────┘
```

One persistent `ssh -T` per host runs the embedded probe (`probe.sh`, bash + coreutils only) in a loop; each tick prints exactly one JSON object. The Rust side parses frames, diffs consecutive counters into rates, and renders with ratatui. There is no state between frames beyond rolling sparkline history, so a dropped connection costs one tick — and supervision reconnects automatically.

## Repository layout

```
probe.sh        the remote probe (embedded via include_str!)
src/main.rs     CLI + front-end dispatch
src/app.rs      app state, stream supervision, key handling
src/tui.rs      terminal front end (crossterm)
src/server.rs   --web front end: HTTP/SSE server + render loop
src/view.rs     renders the screen to cells and serializes them
src/model.rs    probe JSON schema + frame-diff metrics
src/ui.rs       card rendering, scrolling, layout
src/widgets.rs  gauges, heat colors, braille sparklines
src/demo.rs     --demo synthetic hosts (same pipeline as live)
src/ssh.rs      ssh invocation, stream reader
web/            the browser page (grid renderer, geometry glyphs, font)
dev/build.sh    release build with local paths remapped
dev/capture_tui.py + dev/render_shot.py   screenshot pipeline for docs/
dev/verify_parity.py + dev/verify_dom.py  web-vs-TUI fidelity checks
```

## Privacy

skimonitor reads what a shell session could already read on a host you can SSH to, keeps it in RAM for the lifetime of the terminal, and never writes metrics anywhere. No telemetry, no updates, no network calls except the SSH connections you asked for (host-key acceptance to `known_hosts` is standard ssh behavior and the only local write). Note: GPU process command lines are shown verbatim — if you put secrets in argv, they'll be visible on the screen, same as `ps`. Release binaries are built with `--remap-path-prefix`, so they contain no build-machine paths.

## License

MIT
