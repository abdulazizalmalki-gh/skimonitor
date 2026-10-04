# skimonitor

Multi-host SSH monitor TUI in Rust. Enter hosts, get one dense live dashboard — CPU, memory, disk, network, GPU — streaming at 1 Hz from up to 3 machines side by side on a single screen.

## Build

```
cargo build --release            # target/release/skimonitor
./dev/build.sh                   # same, but remaps local paths out of the binary
```

Rust stable, no other toolchain deps. The remote probe is embedded in the binary — nothing needs to be installed on target hosts beyond bash + coreutils.

## Run

```
skimonitor local myalias user@192.0.2.7
skimonitor -i 2 somehost            # 2s frames
skimonitor --once local somehost    # one-shot snapshot for scripts (exit 1 if any fail)
```

Every argument is a target: `ssh` alias, hostname, IP, `user@host`, or `local` (self-scan). Max 3 hosts; drop one with `x` to add another.

## Keys

| key | action |
|-----|--------|
| `a` / `x` | add / remove host |
| `1..3`, `←/→` | select host (drives detail density) |
| `+` / `-` | stream interval 1..60s |
| `r` | restart all streams |
| `p` | pause (tears down SSH sessions) / resume |
| `?` / `q` | help / quit |

## Authentication

Uses the system `ssh` client, so it's exactly the keys residing on the machine running skimonitor: `ssh-agent`, `~/.ssh/config`, and every private key file found in `~/.ssh` — non-default filenames included — offered explicitly via `-i`. `BatchMode=yes` + publickey-only: it never hangs on a password prompt.

If a host refuses the current user (`Permission denied (publickey)`) and no user was pinned in the target, skimonitor retries once as `root@host`; the card is then titled `ssh:root`.

## Reliability

One persistent SSH connection per host streams JSON frames back (~1 Hz; ~1.35s over high-latency WAN, which is the probe's own cost on the target). A dead or wedged stream is detected and respawned with capped exponential backoff; a silence watchdog reconnects streams that stop answering. Rates (CPU %, disk R/W, net ↓↑) are computed by diffing consecutive frames, so they appear one frame after connect.

## What's collected per frame

Load avg, overall + per-core CPU % and MHz, temps + sensors, RAM (used/cache/free, swap, live trend), per-mount disk usage + IO, NIC rx/tx with sparkline, and GPUs: utilization, VRAM, temp/power/fan, and compute processes with **full command lines** (from `/proc/<pid>/cmdline`) and per-process VRAM. On hosts without GPU drivers (e.g. Proxmox with cards passed through to VMs), GPUs are still listed read-only via PCI sysfs presence.
