use serde::Deserialize;
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Debug, Clone, Deserialize)]
pub struct Probe {
    #[serde(default)]
    pub v: u8,
    #[serde(default)]
    pub epoch: u64,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub uptime_s: u64,
    pub cpu: Cpu,
    pub mem: Mem,
    #[serde(default)]
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub zpools: Vec<Zpool>,
    #[serde(default)]
    pub zdatasets: Vec<Dataset>,
    #[serde(default)]
    pub nets: Vec<Net>,
    #[serde(default)]
    pub temp: Temp,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cpu {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub mhz: u64,
    #[serde(default)]
    pub hz: u64,
    #[serde(default)]
    pub ncpu: u64,
    /// aggregate busy/idle jiffies, cumulative since boot
    #[serde(default)]
    pub busy: u64,
    #[serde(default)]
    pub idle: u64,
    pub load: [f64; 3],
    #[serde(default)]
    pub cores: Vec<Core>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Core {
    pub id: u64,
    #[serde(default)]
    pub core: u64,
    pub busy: u64,
    pub idle: u64,
    #[serde(default)]
    pub mhz: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mem {
    pub total_kb: u64,
    pub avail_kb: u64,
    #[serde(default)]
    pub free_kb: u64,
    #[serde(default)]
    pub buffers_kb: u64,
    #[serde(default)]
    pub cached_kb: u64,
    #[serde(default)]
    pub sreclaim_kb: u64,
    #[serde(default)]
    pub swap_total_kb: u64,
    #[serde(default)]
    pub swap_free_kb: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Disk {
    pub mount: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub fs: String,
    pub size: u64,
    pub used: u64,
    #[serde(default)]
    pub avail: u64,
    #[serde(default)]
    pub use_pct: u64,
    /// sectors read/written, cumulative
    #[serde(default)]
    pub rsec: u64,
    #[serde(default)]
    pub wsec: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Zpool {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub alloc: u64,
    #[serde(default)]
    pub free: u64,
    #[serde(default)]
    pub cap: u64,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dataset {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub used: u64,
    /// -1 = no quota
    #[serde(default)]
    pub quota: i64,
    /// -1 = no refquota
    #[serde(default)]
    pub refquota: i64,
    /// -1 = not a volume
    #[serde(default)]
    pub volsize: i64,
    #[serde(default)]
    pub avail: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Net {
    pub name: String,
    pub rx: u64,
    pub tx: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub speed: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Temp {
    #[serde(default)]
    pub cpu_c: Option<f64>,
    #[serde(default)]
    pub cpu_src: Option<String>,
    #[serde(default)]
    pub nvme_c: Option<f64>,
    #[serde(default)]
    pub sensors: Vec<Sensor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sensor {
    #[serde(default)]
    pub label: String,
    pub c: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Gpu {
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub idx: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub util: f64,
    #[serde(default)]
    pub mem_total_mb: u64,
    #[serde(default)]
    pub mem_used_mb: u64,
    #[serde(default)]
    pub temp_c: Option<f64>,
    #[serde(default)]
    pub fan_pct: Option<f64>,
    #[serde(default)]
    pub power_w: Option<f64>,
    #[serde(default)]
    pub power_cap_w: Option<f64>,
    #[serde(default)]
    pub processes: Vec<GpuProc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GpuProc {
    pub pid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub mem_mb: u64,
}

/// derived metrics: percent values + byte rates from a probe diff
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub cpu_pct: f64,
    pub per_core: Vec<CoreStat>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_free: u64,
    pub mem_cache: u64,
    pub mem_sreclaim: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disks: Vec<DiskStat>,
    pub zpools: Vec<ZpoolStat>,
    pub datasets: Vec<DatasetStat>,
    pub nets: Vec<NetStat>,
    pub gpus: Vec<GpuStat>,
    pub cpu_temp: Option<f64>,
    pub cpu_temp_src: Option<String>,
    pub nvme_temp: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct CoreStat {
    pub id: u64,
    pub core: u64,
    pub pct: f64,
    pub mhz: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DiskStat {
    pub mount: String,
    pub fs: String,
    pub device: String,
    pub pct: f64,
    pub used: u64,
    pub size: u64,
    pub read_bps: f64,
    pub write_bps: f64,
}

#[derive(Debug, Clone, Default)]
pub struct NetStat {
    pub name: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub state: String,
    pub speed_mbps: i64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ZpoolStat {
    pub name: String,
    pub size: u64,
    pub alloc: u64,
    pub free: u64,
    pub pct: f64,
    pub state: String,
}

#[derive(Debug, Clone, Default)]
pub struct DatasetStat {
    pub name: String,
    pub vtype: String,
    pub used: u64,
    /// cap = quota, else refquota, else 0 (pool-shared, no own limit)
    pub cap: u64,
    pub pct: f64,
}

#[derive(Debug, Clone, Default)]
pub struct GpuStat {
    pub vendor: String,
    pub idx: u64,
    pub name: String,
    pub util: f64,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub temp_c: Option<f64>,
    pub fan_pct: Option<f64>,
    pub power_w: Option<f64>,
    pub power_cap_w: Option<f64>,
    pub procs: Vec<GpuProc>,
}

pub fn derive(cur: &Probe, prev: &Probe, dt: f64) -> Metrics {
    let mut m = Metrics::default();
    if dt <= 0.0 {
        return m;
    }
    let hz = cur.cpu.hz.max(1) as f64;

    // aggregate cpu% from cumulative jiffies
    let db = cur.cpu.busy.saturating_sub(prev.cpu.busy) as f64;
    let di = cur.cpu.idle.saturating_sub(prev.cpu.idle) as f64;
    m.cpu_pct = if db + di > 0.0 {
        (db / (db + di) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    // per-core: map id -> jiffies, diff by id (order may differ between hosts)
    let pcore: std::collections::HashMap<u64, (u64, u64)> = prev
        .cpu
        .cores
        .iter()
        .map(|c| (c.id, (c.busy, c.idle)))
        .collect();
    m.per_core = cur
        .cpu
        .cores
        .iter()
        .map(|c| {
            let (pb, pi) = pcore.get(&c.id).copied().unwrap_or((c.busy, c.idle));
            let cb = c.busy.saturating_sub(pb) as f64;
            let ci = c.idle.saturating_sub(pi) as f64;
            let pct = if cb + ci > 0.0 {
                (cb / (cb + ci) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            CoreStat {
                id: c.id,
                core: c.core,
                pct,
                mhz: c.mhz,
            }
        })
        .collect();
    let _ = hz;

    m.load1 = cur.cpu.load[0];
    m.load5 = cur.cpu.load[1];
    m.load15 = cur.cpu.load[2];

    // memory
    m.mem_total = cur.mem.total_kb;
    m.mem_used = cur.mem.total_kb.saturating_sub(cur.mem.avail_kb);
    m.mem_free = cur.mem.avail_kb;
    m.mem_cache = cur.mem.buffers_kb + cur.mem.cached_kb;
    m.mem_sreclaim = cur.mem.sreclaim_kb;
    m.swap_total = cur.mem.swap_total_kb;
    m.swap_used = cur.mem.swap_total_kb.saturating_sub(cur.mem.swap_free_kb);

    // disks
    let pd: std::collections::HashMap<(&str, &str), (u64, u64)> = prev
        .disks
        .iter()
        .map(|d| ((d.mount.as_str(), d.device.as_str()), (d.rsec, d.wsec)))
        .collect();
    m.disks = cur
        .disks
        .iter()
        .map(|d| {
            let (pr, pw) = pd
                .get(&(d.mount.as_str(), d.device.as_str()))
                .copied()
                .unwrap_or((d.rsec, d.wsec));
            // sectors are 512 bytes
            DiskStat {
                mount: d.mount.clone(),
                fs: d.fs.clone(),
                device: d.device.clone(),
                pct: d.use_pct as f64,
                used: d.used,
                size: d.size,
                read_bps: d.rsec.saturating_sub(pr) as f64 * 512.0 / dt,
                write_bps: d.wsec.saturating_sub(pw) as f64 * 512.0 / dt,
            }
        })
        .collect();

    // zfs pools: true pool capacity (df only sees the mount overhead);
    // datasets: quota/refquota is the real per-subvol/vm limit, not pool avail
    m.zpools = cur
        .zpools
        .iter()
        .map(|z| ZpoolStat {
            name: z.name.clone(),
            size: z.size,
            alloc: z.alloc,
            free: z.free,
            pct: if z.size > 0 {
                (z.alloc as f64 / z.size as f64 * 100.0).clamp(0.0, 100.0)
            } else {
                z.cap as f64
            },
            state: z.state.clone(),
        })
        .collect();
    m.datasets = cur
        .zdatasets
        .iter()
        .map(|d| {
            let cap = if d.quota > 0 {
                d.quota as u64
            } else if d.refquota > 0 {
                d.refquota as u64
            } else if d.volsize > 0 {
                d.volsize as u64
            } else {
                0
            };
            let pct = if cap > 0 {
                (d.used as f64 / cap as f64 * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            DatasetStat {
                name: d.name.clone(),
                vtype: d.r#type.clone(),
                used: d.used,
                cap,
                pct,
            }
        })
        .collect();

    // networks
    let pn: std::collections::HashMap<&str, (u64, u64)> = prev
        .nets
        .iter()
        .map(|n| ((n.name.as_str()), (n.rx, n.tx)))
        .collect();
    m.nets = cur
        .nets
        .iter()
        .map(|n| {
            let (pr, pt) = pn.get(n.name.as_str()).copied().unwrap_or((n.rx, n.tx));
            NetStat {
                name: n.name.clone(),
                rx_bps: n.rx.saturating_sub(pr) as f64 * 8.0 / dt, // bits/s
                tx_bps: n.tx.saturating_sub(pt) as f64 * 8.0 / dt,
                state: n.state.clone(),
                speed_mbps: n.speed,
                rx_total: n.rx,
                tx_total: n.tx,
            }
        })
        .collect();

    m.cpu_temp = cur.temp.cpu_c;
    m.cpu_temp_src = cur.temp.cpu_src.clone();
    m.nvme_temp = cur.temp.nvme_c;

    m.gpus = cur
        .gpus
        .iter()
        .map(|g| GpuStat {
            vendor: g.vendor.clone(),
            idx: g.idx,
            name: g.name.clone(),
            util: g.util,
            mem_used_mb: g.mem_used_mb,
            mem_total_mb: g.mem_total_mb,
            temp_c: g.temp_c,
            fan_pct: g.fan_pct,
            power_w: g.power_w,
            power_cap_w: g.power_cap_w,
            procs: g.processes.clone(),
        })
        .collect();

    m
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    Connecting,
    Ok,
    Error,
}

#[derive(Debug, Clone)]
pub struct Host {
    pub target: String,
    pub label: String,
    /// frame came from the built-in demo replay, not ssh
    pub as_demo: bool,
    /// the live stream is connected as root@host (auth fallback)
    pub as_root: bool,
    pub state: HostState,
    pub last_error: Option<String>,
    pub last_ok: Option<std::time::SystemTime>,
    pub probe: Option<Probe>,
    pub metrics: Option<Metrics>,
    /// rolling history: cpu% and primary-NIC rx bytes/s
    pub hist_cpu: VecDeque<f64>,
    pub hist_rx: VecDeque<f64>,
    pub hist_mem: VecDeque<f64>,
    pub last_at: Option<Instant>,
    pub consecutive_errors: u32,
    pub probing: bool,
}

impl Host {
    pub fn new(target: &str) -> Self {
        let label = target.rsplit('@').next().unwrap_or(target).to_string();
        Self {
            target: target.to_string(),
            label,
            as_demo: false,
            as_root: false,
            state: HostState::Connecting,
            last_error: None,
            last_ok: None,
            probe: None,
            metrics: None,
            hist_cpu: VecDeque::with_capacity(256),
            hist_rx: VecDeque::with_capacity(256),
            hist_mem: VecDeque::with_capacity(256),
            last_at: None,
            consecutive_errors: 0,
            probing: false,
        }
    }

    pub fn push_hist(&mut self, cpu: f64, rx_bps: f64, mem_pct: f64) {
        for dq in [&mut self.hist_cpu, &mut self.hist_rx, &mut self.hist_mem] {
            if dq.len() >= 250 {
                dq.pop_front();
            }
        }
        self.hist_cpu.push_back(cpu);
        self.hist_rx.push_back(rx_bps);
        self.hist_mem.push_back(mem_pct);
    }
}

// ---------- formatting helpers ----------

pub fn fmt_bytes_kb(kb: u64) -> String {
    fmt_bytes(kb * 1024)
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "Ki", "Mi", "Gi", "Ti", "Pi"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", b, UNITS[0])
    } else if v >= 100.0 {
        format!("{:.0}{}", v, UNITS[i])
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

pub fn fmt_bps(b: f64) -> String {
    const UNITS: [&str; 5] = ["b/s", "Kb/s", "Mb/s", "Gb/s", "Tb/s"];
    let mut v = b;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{:.0}{}", v, UNITS[0])
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

pub fn fmt_bytesps(b: f64) -> String {
    fmt_bytes(b as u64) + "/s"
}

pub fn fmt_uptime(s: u64) -> String {
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 {
        format!("{}d {:02}h {:02}m", d, h, m)
    } else if h > 0 {
        format!("{}h {:02}m", h, m)
    } else {
        format!("{}m", m)
    }
}

pub fn short_label(target: &str) -> String {
    // "user@host" -> "host"; "host.example.com" -> "host" style label keeps fqdn
    target.rsplit('@').next().unwrap_or(target).to_string()
}
