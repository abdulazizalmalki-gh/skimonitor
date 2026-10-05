//! Synthetic probe frames for `--demo`: drives the REAL UI code path
//! (JSON frame -> Probe -> derive -> render) with fabricated numbers, so the
//! screen you see is exactly what a live session looks like — with no real
//! machine involved. Hostnames, pools, processes and commands here are invented.

use crate::model::Probe;
use crate::ssh::Sample;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// Everything needed to make one fake host look alive.
pub struct DemoSpec {
    pub target: &'static str,
    pub hostname: &'static str,
    pub kernel: &'static str,
    pub arch: &'static str,
    pub cpu_model: &'static str,
    pub ncpu: u64,
    pub base_mhz: u64,
    pub uptime_s: u64,
    pub load_base: f64,
    pub mem_total_kb: u64,
    pub mem_used_pct: f64,
    pub swap_total_kb: u64,
    pub swap_used_pct: f64,
    pub disks: &'static [(&'static str, &'static str, &'static str, u64, u64)],
    pub zpool: Option<(&'static str, u64, u64)>,
    pub datasets: &'static [(&'static str, &'static str, u64, u64)],
    pub nets: &'static [(&'static str, i64)],
    pub cpu_temp: f64,
    pub gpus: &'static [(&'static str, u64, f64, f64, f64)],
    pub procs: &'static [(u64, &'static str, &'static str, u64)],
}

/// Deterministic noise in -1..1 keyed by (seed, tick).
fn wobble(seed: u64, tick: u64) -> f64 {
    let mut x = (seed ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(0x2545_F491_4F6C_DD1D);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x as f64 / u64::MAX as f64) * 2.0 - 1.0
}

fn clampf(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Level value at tick `t`.
fn frac_at(seed: u64, t: u64, base: f64, amp: f64) -> f64 {
    clampf(base + wobble(seed, t) * amp, 0.004, 0.995)
}

/// Sum of per-tick levels up to and including `tick` — the integral, so the
/// app's frame-to-frame diff of these cumulative counters yields the intended
/// non-negative rate (exactly what /proc's cumulative counters look like).
fn integral(seed: u64, tick: u64, base: f64, amp: f64, per_tick: f64) -> u64 {
    let mut s = 0.0f64;
    for t in 0..=tick {
        s += frac_at(seed, t, base, amp) * per_tick;
    }
    s as u64
}

/// Build a flat JSON object from (key, raw-JSON-value) pairs.
fn obj(pairs: &[(&str, String)]) -> String {
    let inner: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect();
    format!("{{{}}}", inner.join(","))
}

fn s(x: &str) -> String {
    format!("\"{x}\"")
}
fn n(x: u64) -> String {
    x.to_string()
}
fn f1(x: f64) -> String {
    format!("{x:.2}")
}
/// integer-valued field printed with no decimals
fn fi(x: f64) -> String {
    format!("{x:.0}")
}

pub fn run(hosts: &'static [DemoSpec], sleep_secs: u64, tx: Sender<Sample>) {
    thread::spawn(move || {
        let mut tick: u64 = 0;
        loop {
            for (hi, h) in hosts.iter().enumerate() {
                let line = frame_json(h, (hi as u64 + 1) * 7919, tick, sleep_secs);
                match serde_json::from_str::<Probe>(&line) {
                    Ok(p) => {
                        if tx
                            .send(Sample {
                                target: h.target.to_string(),
                                result: Ok(p),
                                at: Instant::now(),
                                epoch: 0,
                            })
                            .is_err()
                        {
                            return; // app gone
                        }
                    }
                    Err(e) => eprintln!("demo frame parse: {e}"),
                }
            }
            tick += 1;
            thread::sleep(Duration::from_secs(sleep_secs.max(1)));
        }
    });
}

/// The three demo hosts: a plain web VPS, a ZFS/NAS box, a GPU inference box.
/// None of these exist anywhere.
pub const DEMO_HOSTS: &[DemoSpec] = &[
    DemoSpec {
        target: "web-01",
        hostname: "web-01.example",
        kernel: "6.8.0-41-generic",
        arch: "x86_64",
        cpu_model: "AMD EPYC 7543",
        ncpu: 8,
        base_mhz: 2894,
        uptime_s: 42 * 86400 + 7 * 3600,
        load_base: 0.16,
        mem_total_kb: 16_384 * 1024,
        mem_used_pct: 41.0,
        swap_total_kb: 2 * 1024 * 1024,
        swap_used_pct: 4.0,
        disks: &[
            ("/", "sda1", "ext4", 64 * 1024u64.pow(3), 54),
            ("/var/www", "sdb1", "ext4", 512 * 1024u64.pow(3), 31),
            ("/var/log", "sda2", "ext4", 32 * 1024u64.pow(3), 77),
            ("/home", "sda3", "ext4", 96 * 1024u64.pow(3), 22),
            ("/backup/daily", "sdc1", "xfs", 2 * 1024u64.pow(4), 58),
            ("/srv/docker", "dm-0", "ext4", 120 * 1024u64.pow(3), 68),
        ],
        zpool: None,
        datasets: &[],
        nets: &[("eth0", 1000)],
        cpu_temp: 52.0,
        gpus: &[],
        procs: &[],
    },
    DemoSpec {
        target: "nas-01",
        hostname: "nas-01.example",
        kernel: "6.5.13-1-pve",
        arch: "x86_64",
        cpu_model: "Intel Core i5-12500",
        ncpu: 8,
        base_mhz: 3600,
        uptime_s: 63 * 86400 + 11 * 3600,
        load_base: 0.10,
        mem_total_kb: 64 * 1024 * 1024,
        mem_used_pct: 33.0,
        swap_total_kb: 8 * 1024 * 1024,
        swap_used_pct: 0.5,
        disks: &[
            ("/", "nvme0n1p2", "ext4", 465 * 1024u64.pow(3), 38),
            ("/mnt/isos", "sdc1", "ext4", 1 * 1024u64.pow(4), 12),
        ],
        zpool: Some(("tank", 16 * 1024u64.pow(4), 46)),
        datasets: &[
            ("tank/data/nextcloud", "dataset", 61, 4 * 1024u64.pow(3)),
            ("tank/backup/pve", "dataset", 84, 2 * 1024u64.pow(3)),
            ("tank/vm-112-disk-0", "volume", 46, 64 * 1024u64.pow(3)),
            ("tank/vm-118-disk-0", "volume", 12, 128 * 1024u64.pow(3)),
            ("tank/sub-108-root", "subvol", 72, 96 * 1024u64.pow(3)),
            ("tank/sub-121-root", "subvol", 34, 32 * 1024u64.pow(3)),
            ("tank/vm-104-disk-0", "volume", 91, 32 * 1024u64.pow(3)),
            ("tank/data/media", "dataset", 47, 8 * 1024u64.pow(3)),
        ],
        nets: &[("eno1", 2500), ("eno2", 10000)],
        cpu_temp: 47.0,
        gpus: &[],
        procs: &[],
    },
    DemoSpec {
        target: "gpu-01",
        hostname: "gpu-01.example",
        kernel: "6.8.0-45-generic",
        arch: "x86_64",
        cpu_model: "AMD Ryzen 9 7950X",
        ncpu: 16,
        base_mhz: 4200,
        uptime_s: 6 * 86400 + 2 * 3600,
        load_base: 0.34,
        mem_total_kb: 96 * 1024 * 1024,
        mem_used_pct: 62.0,
        swap_total_kb: 8 * 1024 * 1024,
        swap_used_pct: 2.0,
        disks: &[
            ("/", "nvme0n1p2", "ext4", 931 * 1024u64.pow(3), 44),
            ("/models", "sda1", "ext4", 3860 * 1024u64.pow(3), 63),
            ("/datasets", "sdb1", "xfs", 7400 * 1024u64.pow(3), 71),
        ],
        zpool: None,
        datasets: &[],
        nets: &[("eno1", 10000)],
        cpu_temp: 61.0,
        gpus: &[
            ("NVIDIA GeForce RTX 4090", 24564, 0.885, 96.0, 356.0),
            ("NVIDIA GeForce RTX 4090", 24564, 0.812, 74.0, 288.0),
        ],
        procs: &[
            (0, "svc-llm", "/usr/local/bin/llama-server -m /srv/models/demo-9b-q4_k_m.gguf --port 9090 -ngl 99 --ctx 32768", 21810),
            (1, "ops", "/usr/bin/python3 -m vllm.entrypoints.openai.api_server --model demo-lab/embed-v2 --max-model-len 4096", 4820),
            (1, "svc-llm", "/usr/local/bin/llama-server -m /models/demo-32b-iq4.gguf --port 9091", 15104),
        ],
    },
];

/// Build one JSON frame. Cumulative counters (jiffies, sectors, rx/tx bytes)
/// are integrals over ticks, so every frame-to-frame diff is >= 0.
fn frame_json(h: &DemoSpec, seed: u64, tick: u64, dt: u64) -> String {
    let hz = 100.0f64; // jiffies/s per CPU
    let per = dt as f64; // jiffies (or bytes/sectors) contributed per tick
    let secs = (tick + 1) as f64 * dt as f64; // pretend the box streamed this long
    let base_busy = clampf(h.load_base, 0.01, 0.95);
    let ncpu_f = h.ncpu as f64;

    // ---- cpu ----
    let busy = integral(seed + 1, tick, base_busy, 0.10, hz * per * ncpu_f);
    let idle = integral(seed + 2, tick, 1.0 - base_busy, 0.10, hz * per * ncpu_f);

    let mut cores: Vec<String> = Vec::new();
    let mut lsum = 0.0f64;
    for c in 0..h.ncpu {
        let cp = clampf(
            base_busy * 100.0 + wobble(seed + c * 31 + 5, tick) * 34.0,
            0.0,
            99.0,
        );
        lsum += cp;
        let cb = integral(seed + c * 97 + 11, tick, cp / 100.0, 0.05, hz * per);
        let ci = integral(seed + c * 97 + 12, tick, 1.0 - cp / 100.0, 0.05, hz * per);
        let mhz = (h.base_mhz as f64 * clampf(0.52 + cp / 125.0, 0.5, 1.32)) as u64;
        cores.push(obj(&[
            ("id", n(c)),
            ("core", n(c / 2)), // 2 logical CPUs per physical core (HT)
            ("busy", n(cb)),
            ("idle", n(ci)),
            ("mhz", n(mhz)),
        ]));
    }
    // load1 ≈ number of CPUs busy on average (sum of per-core busy% / 100)
    let l1 = clampf(lsum / 100.0 + wobble(seed + 9, tick) * 0.2, 0.0, 64.0);
    let cpu = obj(&[
        ("model", s(h.cpu_model)),
        ("mhz", n(h.base_mhz)),
        ("hz", n(100)),
        ("ncpu", n(h.ncpu)),
        ("busy", n(busy)),
        ("idle", n(idle)),
        (
            "load",
            format!("[{},{},{}]", f1(l1), f1(l1 * 0.85), f1(l1 * 0.7)),
        ),
        ("cores", format!("[{}]", cores.join(","))),
    ]);

    // ---- memory ----
    let total = h.mem_total_kb as f64;
    let used = (total * clampf(h.mem_used_pct / 100.0 + wobble(seed + 3, tick) * 0.012, 0.05, 0.97)) as u64;
    let mem = obj(&[
        ("total_kb", n(h.mem_total_kb)),
        ("avail_kb", n((total - used as f64).max(total * 0.02) as u64)),
        ("free_kb", n(((total - used as f64) * 0.6) as u64)),
        ("buffers_kb", n((total * 0.05) as u64)),
        ("cached_kb", n((total * 0.19) as u64)),
        ("sreclaim_kb", n((total * 0.06) as u64)),
        ("swap_total_kb", n(h.swap_total_kb)),
        (
            "swap_free_kb",
            n(h.swap_total_kb - (h.swap_total_kb as f64 * h.swap_used_pct / 100.0) as u64),
        ),
    ]);

    // ---- disks ----
    let disks: Vec<String> = h
        .disks
        .iter()
        .enumerate()
        .map(|(i, (mount, dev, fs, size, pct))| {
            let dused = (*size as f64 * *pct as f64 / 100.0) as u64;
            let rsec = integral(seed + i as u64 * 101 + 21, tick, 0.30, 0.18, 3_800.0 * per);
            let wsec = integral(seed + i as u64 * 101 + 22, tick, 0.42, 0.20, 5_400.0 * per);
            obj(&[
                ("mount", s(mount)),
                ("device", s(dev)),
                ("fs", s(fs)),
                ("size", n(*size)),
                ("used", n(dused)),
                ("avail", n(*size - dused)),
                ("use_pct", n(*pct)),
                ("rsec", n(rsec)),
                ("wsec", n(wsec)),
            ])
        })
        .collect();

    // ---- zfs ----
    let zpools: Vec<String> = h
        .zpool
        .iter()
        .map(|(name, size, cap)| {
            let alloc = (*size as f64 * *cap as f64 / 100.0) as u64;
            obj(&[
                ("name", s(name)),
                ("size", n(*size)),
                ("alloc", n(alloc)),
                ("free", n(*size - alloc)),
                ("cap", n(*cap)),
                ("state", s("ONLINE")),
            ])
        })
        .collect();
    let datasets: Vec<String> = h
        .datasets
        .iter()
        .map(|(name, ty, used_pct, limit)| {
            let dused = (*limit as f64 * *used_pct as f64 / 100.0) as u64;
            let lim = *limit as i64;
            obj(&[
                ("name", s(name)),
                ("type", s(ty)),
                ("used", n(dused)),
                ("quota", (if *ty == "dataset" { lim } else { -1 }).to_string()),
                ("refquota", (if *ty == "subvol" { lim } else { -1 }).to_string()),
                ("volsize", (if *ty == "volume" { lim } else { -1 }).to_string()),
                ("avail", n(9_663_676_416)),
            ])
        })
        .collect();

    // ---- net ----
    let nets: Vec<String> = h
        .nets
        .iter()
        .enumerate()
        .map(|(i, (name, speed))| {
            let rx = integral(
                seed + i as u64 * 7 + 41,
                tick,
                0.35,
                0.28,
                1_400_000.0 * per * (i as f64 + 1.0),
            );
            let tx = integral(
                seed + i as u64 * 7 + 42,
                tick,
                0.12,
                0.08,
                1_400_000.0 * per * (i as f64 + 1.0),
            );
            obj(&[
                ("name", s(name)),
                ("rx", n(rx)),
                ("tx", n(tx)),
                ("state", s("up")),
                ("speed", speed.to_string()),
            ])
        })
        .collect();

    // ---- temp ----
    let cput = h.cpu_temp + wobble(seed + 21, tick) * 2.5;
    let nvmet = h.cpu_temp - 7.0 + wobble(seed + 22, tick) * 2.0;
    let temp = obj(&[
        ("cpu_c", fi(cput)),
        ("cpu_src", s("package")),
        ("nvme_c", fi(nvmet)),
        ("sensors", "[]".to_string()),
    ]);

    // ---- gpus ----
    let gpus: Vec<String> = h
        .gpus
        .iter()
        .enumerate()
        .map(|(i, (name, total_mb, vram_pct, util_base, power))| {
            let gi = i as u64;
            let vused = (*total_mb as f64
                * clampf(*vram_pct + wobble(seed + gi * 13 + 61, tick) * 0.02, 0.05, 0.985))
                as u64;
            let util = clampf(*util_base + wobble(seed + gi * 17 + 62, tick) * 14.0, 2.0, 100.0);
            let temp = clampf(44.0 + util * 0.30 + wobble(seed + gi * 5 + 63, tick) * 2.5, 36.0, 83.0);
            let fan = clampf(28.0 + util * 0.45, 25.0, 89.0);
            let pw = clampf(*power + wobble(seed + gi * 3 + 64, tick) * 22.0, 40.0, 450.0);
            let procs: Vec<String> = h
                .procs
                .iter()
                .filter(|(g, ..)| *g == gi)
                .map(|(_, user, cmd, mem)| {
                    obj(&[
                        ("pid", n(8100 + mem % 700)),
                        ("name", s(cmd)),
                        ("user", s(user)),
                        ("mem_mb", n(*mem)),
                    ])
                })
                .collect();
            obj(&[
                ("vendor", s("nvidia")),
                ("idx", n(gi)),
                ("name", s(name)),
                ("uuid", s(&format!("GPU-DEMO-000{gi}"))),
                ("util", fi(util)),
                ("mem_total_mb", n(*total_mb)),
                ("mem_used_mb", n(vused)),
                ("temp_c", fi(temp)),
                ("fan_pct", fi(fan)),
                ("power_w", fi(pw)),
                ("power_cap_w", n(450)),
                ("processes", format!("[{}]", procs.join(","))),
            ])
        })
        .collect();

    obj(&[
        ("v", n(1)),
        ("epoch", n(tick)),
        ("hostname", s(h.hostname)),
        ("kernel", s(h.kernel)),
        ("arch", s(h.arch)),
        ("uptime_s", n(h.uptime_s + secs as u64)),
        ("cpu", cpu),
        ("mem", mem),
        ("disks", format!("[{}]", disks.join(","))),
        ("zpools", format!("[{}]", zpools.join(","))),
        ("zdatasets", format!("[{}]", datasets.join(","))),
        ("nets", format!("[{}]", nets.join(","))),
        ("temp", temp),
        ("gpus", format!("[{}]", gpus.join(","))),
    ])
}
