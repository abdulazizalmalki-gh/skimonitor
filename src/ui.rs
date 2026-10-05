use crate::model::{fmt_bps, fmt_bytes, fmt_bytes_kb, fmt_bytesps, fmt_uptime, Host, HostState};
use crate::widgets::{gauge, heat, heat_temp, sparkline};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub const C_ACCENT: Color = Color::Cyan;
pub const C_MUTED: Color = Color::DarkGray;
pub const C_TEXT: Color = Color::Gray;

pub const MAX_HOSTS: usize = 3;

const CORE_LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Geometry of one host card after layout: used for mouse-scroll hit testing.
#[derive(Clone, Copy)]
pub struct CardGeom {
    pub x0: u16,
    pub x1: u16,
    pub y0: u16,
    pub y1: u16,
}

/// Draws the full screen. `scrolls[i]` = desired top line of card i;
/// returns (per-card max scroll, per-card geometry) so the app can clamp
/// stored offsets and hit-test wheel events.
pub fn draw(
    f: &mut Frame,
    hosts: &[Host],
    sel: usize,
    interval_s: u64,
    input: &Option<String>,
    help: bool,
    pending: usize,
    paused: bool,
    note: &Option<String>,
    scrolls: &[usize],
) -> (Vec<usize>, Vec<CardGeom>) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),                                   // header
        Constraint::Min(14),                                     // host cards
        Constraint::Length(if input.is_some() { 2 } else { 1 }), // status/input
    ])
    .split(area);

    // ---- header ----
    let mut head: Vec<Span> = vec![
        Span::styled(
            " skimonitor ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(C_MUTED),
        ),
        Span::styled(
            format!("stream {}s · {} host(s) ", interval_s, hosts.len()),
            Style::default().fg(C_TEXT),
        ),
    ];
    for (i, h) in hosts.iter().enumerate().take(MAX_HOSTS) {
        let dot = match h.state {
            HostState::Ok => "●",
            HostState::Connecting => "◐",
            HostState::Error => "▲",
        };
        let dotc = match h.state {
            HostState::Ok => Color::Green,
            HostState::Connecting => Color::Yellow,
            HostState::Error => Color::Red,
        };
        let active = i == sel;
        let st = if active {
            Style::default()
                .fg(Color::Black)
                .bg(C_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };
        head.push(Span::styled(" ", st));
        head.push(Span::styled(
            dot.to_string(),
            if active {
                st
            } else {
                Style::default().fg(dotc)
            },
        ));
        let lbl: String = h.label.chars().take(14).collect();
        head.push(Span::styled(format!("{lbl} "), st));
    }
    let hint = if hosts.len() < MAX_HOSTS {
        "  [a]add [x]drop [r]estart [p]pause [?]help [q]quit"
    } else {
        "  [x]drop [r]estart [p]pause [?]help [q]quit (max 3 hosts)"
    };
    head.push(Span::styled(hint, Style::default().fg(C_MUTED)));
    f.render_widget(Paragraph::new(Line::from(head)), chunks[0]);

    // ---- host cards, side by side, always visible ----
    let ncols = hosts.len().clamp(1, MAX_HOSTS);
    let cols = Layout::horizontal(vec![Constraint::Fill(1); ncols]).split(chunks[1]);
    let mut maxscrolls = vec![0usize; ncols];
    let geoms: Vec<CardGeom> = cols
        .iter()
        .map(|c| CardGeom {
            x0: c.x,
            x1: c.x + c.width.saturating_sub(1),
            y0: c.y,
            y1: c.y + c.height.saturating_sub(1),
        })
        .collect();
    if hosts.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                " no hosts — press [a] to add one (max 3)",
                Style::default().fg(C_MUTED),
            )),
            cols[0],
        );
    }
    for (i, h) in hosts.iter().enumerate().take(MAX_HOSTS) {
        let sc = scrolls.get(i).copied().unwrap_or(0);
        maxscrolls[i] = draw_host_card(f, cols[i], h, i == sel, sc);
    }

    // ---- status / input line ----
    let demo_mode = hosts.iter().any(|h| h.as_demo);
    if let Some(buf) = input {
        let line = Line::from(vec![
            Span::styled(
                " add host(s): ",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(buf.clone(), Style::default().fg(Color::White)),
            Span::styled("█", Style::default().fg(C_ACCENT)),
            Span::styled(
                "   Enter=commit · Esc=cancel   (user@host, alias, or ip; space/comma separated)",
                Style::default().fg(C_MUTED),
            ),
        ]);
        f.render_widget(Paragraph::new(line), chunks[2]);
    } else {
        let mut spans = vec![
            Span::styled(
                format!(" pending:{} ", pending),
                Style::default().fg(C_MUTED),
            ),
            Span::styled(
                if demo_mode {
                    "DEMO REPLAY — invented hosts, fabricated numbers, no ssh"
                } else {
                    "auth: keys on this machine (ssh BatchMode, never prompts)"
                },
                Style::default().fg(if demo_mode {
                    Color::Yellow
                } else {
                    C_MUTED
                }),
            ),
        ];
        if paused {
            spans.push(Span::styled(
                "  ⏸ PAUSED [p]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if let Some(n) = note {
            spans.push(Span::styled(
                format!("  ⚠ {n}"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    }

    if help {
        draw_help(f, area);
    }
    (maxscrolls, geoms)
}

/// Render one host card with per-card vertical scrolling; returns the max
/// valid scroll (line overflow) for this card at the current height.
fn draw_host_card(f: &mut Frame, area: Rect, h: &Host, active: bool, scroll: usize) -> usize {
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    let name = h
        .probe
        .as_ref()
        .map(|p| p.hostname.clone())
        .unwrap_or_else(|| h.label.clone());
    let src_tag = if crate::ssh::is_local(&h.target) {
        "local"
    } else if h.as_demo {
        "demo"
    } else if h.as_root {
        "ssh:root"
    } else {
        "ssh"
    };

    let mut lines: Vec<Line> = Vec::new();

    match (&h.metrics, &h.probe) {
        (Some(m), Some(p)) => {
            let fresh = h
                .last_ok
                .map(|t| format!("{}s ago", t.elapsed().map(|d| d.as_secs()).unwrap_or(0)))
                .unwrap_or_else(|| "-".into());
            lines.push(Line::from(fit(
                vec![
                    Span::styled(format!("{} ", h.target), Style::default().fg(C_TEXT)),
                    Span::styled(
                        format!(
                            "up {} · {} · load {:.1}/{:.1} ",
                            fmt_uptime(p.uptime_s),
                            fresh,
                            m.load1,
                            m.load5
                        ),
                        Style::default().fg(C_MUTED),
                    ),
                ],
                inner_w,
            )));

            // ---- CPU: one gauge + live sparkline ----
            let bar_w = inner_w.saturating_sub(18).clamp(6, 18);
            let (sp, _) = sparkline(
                &h.hist_cpu.iter().cloned().collect::<Vec<f64>>(),
                inner_w.saturating_sub(bar_w + 12).max(4),
                C_ACCENT,
            );
            let mut l = gauge(
                Some("cpu"),
                m.cpu_pct,
                100.0,
                bar_w,
                "%",
                heat(m.cpu_pct),
                None,
            );
            l.push(Span::raw(" "));
            l.extend(sp);
            lines.push(Line::from(fit(l, inner_w)));

            // per-core: one block char per logical CPU; HT siblings (same
            // physical core_id) stay glued, 2-col gap between cores. GHz tail
            // on the right; what no longer fits is marked "…".
            if !m.per_core.is_empty() {
                let mut spans = vec![Span::styled("core ", Style::default().fg(C_MUTED))];
                let tail = if m.per_core.iter().any(|c| c.mhz > 0) { 14 } else { 2 };
                let budget = inner_w.saturating_sub(6 + tail);
                let mut used = 0usize;
                let mut prev_core: Option<u64> = None;
                let mut dropped = false;
                for c in &m.per_core {
                    let gap = match prev_core {
                        None => 0usize,
                        Some(pc) => {
                            if pc == c.core && c.core != u64::MAX {
                                0 // HT sibling: glued to its twin
                            } else {
                                2 // new physical core
                            }
                        }
                    };
                    if used + gap + 1 > budget {
                        dropped = true;
                        break;
                    }
                    if gap > 0 {
                        spans.push(Span::styled(" ".repeat(gap), Style::default().fg(C_MUTED)));
                        used += gap;
                    }
                    let lvl = (c.pct / 100.0 * 7.0).round().clamp(0.0, 7.0) as usize;
                    spans.push(Span::styled(
                        CORE_LEVELS[lvl].to_string(),
                        Style::default().fg(heat(c.pct)),
                    ));
                    used += 1;
                    prev_core = Some(c.core);
                }
                if dropped {
                    spans.push(Span::styled("…", Style::default().fg(C_MUTED)));
                }
                let mhzs: Vec<f64> = m
                    .per_core
                    .iter()
                    .map(|c| c.mhz as f64)
                    .filter(|x| *x > 0.0)
                    .collect();
                if !mhzs.is_empty() {
                    let lo = mhzs.iter().fold(f64::MAX, |a, b| a.min(*b));
                    let hi = mhzs.iter().fold(0.0f64, |a, b| a.max(*b));
                    spans.push(Span::styled(
                        format!(" {:.1}–{:.1}GHz", lo / 1000.0, hi / 1000.0),
                        Style::default().fg(C_MUTED),
                    ));
                }
                lines.push(Line::from(fit(spans, inner_w)));
            }

            // temps: one compact line
            let mut tline = vec![Span::styled("temp ", Style::default().fg(C_MUTED))];
            if let Some(t) = m.cpu_temp {
                tline.push(Span::styled(
                    format!("{:.0}°C ", t),
                    Style::default().fg(heat_temp(t)),
                ));
            } else {
                tline.push(Span::styled("cpu n/a ", Style::default().fg(C_MUTED)));
            }
            if let Some(nv) = p.temp.nvme_c {
                tline.push(Span::styled(
                    format!("nvme {:.0}° ", nv),
                    Style::default().fg(heat_temp(nv)),
                ));
            }
            for s in p
                .temp
                .sensors
                .iter()
                .filter(|s| s.label != "Package id 0")
                .take(8)
            {
                tline.push(Span::styled(
                    format!("{}:{:.0}° ", s.label.replace("Core ", "c"), s.c),
                    Style::default().fg(heat_temp(s.c)),
                ));
            }
            lines.push(Line::from(fit(tline, inner_w)));

            // ---- MEMORY: full-width segmented bar (htop-like, not a pie) ----
            let total = m.mem_total.max(1) as f64;
            let used = m.mem_used as f64;
            let cache = (m.mem_cache as f64 - m.mem_sreclaim as f64).clamp(0.0, total);
            let free = (total - used - cache).max(0.0);
            lines.push(Line::from(""));
            let mut hdr = vec![Span::styled(
                "mem ",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            )];
            hdr.push(Span::styled(
                format!(
                    "{:.0}% used · {} total",
                    used / total * 100.0,
                    fmt_bytes_kb(m.mem_total)
                ),
                Style::default().fg(C_TEXT),
            ));
            lines.push(Line::from(fit(hdr, inner_w)));
            lines.push(Line::from(fit(
                stacked_bar(
                    inner_w,
                    &[
                        (used, Color::Red, "used"),
                        (cache, Color::Rgb(220, 180, 60), "cache"),
                        (free, Color::Rgb(60, 60, 60), "free"),
                    ],
                    total,
                ),
                inner_w,
            )));
            lines.push(Line::from(fit(
                vec![
                    seg_label(Color::Red, "used", fmt_bytes_kb(m.mem_used)),
                    seg_label(
                        Color::Rgb(220, 180, 60),
                        "cache",
                        fmt_bytes_kb(cache as u64),
                    ),
                    seg_label(Color::Rgb(120, 120, 120), "free", fmt_bytes_kb(free as u64)),
                ],
                inner_w,
            )));
            if m.swap_total > 0 {
                let sw_total = m.swap_total.max(1) as f64;
                let sw_used = m.swap_used as f64;
                let mut l = stacked_bar(
                    inner_w.saturating_sub(30).clamp(6, 24),
                    &[
                        (sw_used, Color::Blue, "swap"),
                        (sw_total - sw_used, Color::Rgb(60, 60, 60), ""),
                    ],
                    sw_total,
                );
                l.push(Span::styled(
                    format!(
                        " swap {} / {} ({:.0}%)",
                        fmt_bytes_kb(m.swap_used),
                        fmt_bytes_kb(m.swap_total),
                        sw_used / sw_total * 100.0
                    ),
                    Style::default().fg(C_MUTED),
                ));
                lines.push(Line::from(fit(l, inner_w)));
            }
            // live memory-used % trend
            {
                let mut l = vec![Span::styled("trend ", Style::default().fg(C_MUTED))];
                let (sp, _) = sparkline(
                    &h.hist_mem.iter().cloned().collect::<Vec<f64>>(),
                    inner_w.saturating_sub(6).max(4),
                    heat(used / total * 100.0),
                );
                l.extend(sp);
                lines.push(Line::from(fit(l, inner_w)));
            }

            // ---- DISK: real mounts, then ZFS truth ----
            // df -T on ZFS lies: mounts report pool avail with near-zero used.
            // Non-zfs mounts keep the 2-row treatment; zfs gets a pool row
            // (real device %) + rows for datasets with a quota/refquota/volsize
            // — the limits Proxmox actually enforces (subvols, VM disks).
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "disk",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            )));
            let plain: Vec<&crate::model::DiskStat> =
                m.disks.iter().filter(|d| d.fs != "zfs").collect();
            if m.disks.is_empty() && m.zpools.is_empty() {
                lines.push(Line::from(Span::styled(
                    " none",
                    Style::default().fg(C_MUTED),
                )));
            }
            for d in plain.iter().take(6) {
                let bar_w = inner_w.saturating_sub(34).clamp(6, 16);
                let dev: String = short_dev(&d.device).chars().take(10).collect();
                let mut l = vec![Span::styled(
                    format!("{dev:<10} "),
                    Style::default().fg(C_TEXT),
                )];
                l.extend(gauge(
                    None,
                    d.pct,
                    100.0,
                    bar_w,
                    "%",
                    heat(d.pct),
                    Some(format!("{:.0}%", d.pct)),
                ));
                let mp: String = d.mount.chars().take(14).collect();
                l.push(Span::styled(
                    format!(" {mp}"),
                    Style::default().fg(C_ACCENT),
                ));
                // size right-aligned to the panel edge: numbers line up, no huge gap
                let size_s = format!("{}/{}", fmt_bytes(d.used), fmt_bytes(d.size));
                let pre: usize = l.iter().map(|s| s.content.chars().count()).sum();
                let pad = inner_w
                    .saturating_sub(pre + size_s.chars().count())
                    .clamp(1, 60);
                l.push(Span::styled(" ".repeat(pad), Style::default()));
                l.push(Span::styled(size_s, Style::default().fg(heat(d.pct))));
                lines.push(Line::from(fit(l, inner_w)));
                lines.push(Line::from(fit(
                    vec![Span::styled(
                        format!(
                            "  {} · R {} · W {}",
                            d.fs,
                            fmt_bytesps(d.read_bps),
                            fmt_bytesps(d.write_bps)
                        ),
                        Style::default().fg(C_MUTED),
                    )],
                    inner_w,
                )));
            }
            if !m.zpools.is_empty() {
                for z in m.zpools.iter().take(2) {
                    let bar_w = inner_w.saturating_sub(30).clamp(6, 16);
                    let label: String =
                        format!("zpool {}", z.name).chars().take(13).collect();
                    let mut l = vec![Span::styled(
                        format!("{:<14}", label),
                        Style::default()
                            .fg(C_TEXT)
                            .add_modifier(Modifier::BOLD),
                    )];
                    l.extend(gauge(
                        None,
                        z.pct,
                        100.0,
                        bar_w,
                        "%",
                        heat(z.pct),
                        Some(format!("{:.0}%", z.pct)),
                    ));
                    let size_s = format!(
                        "{}/{} · {} free",
                        fmt_bytes(z.alloc),
                        fmt_bytes(z.size),
                        fmt_bytes(z.free)
                    );
                    let pre: usize = l.iter().map(|s| s.content.chars().count()).sum();
                    let pad = inner_w
                        .saturating_sub(pre + size_s.chars().count())
                        .clamp(1, 60);
                    l.push(Span::styled(" ".repeat(pad), Style::default()));
                    l.push(Span::styled(
                        size_s,
                        Style::default().fg(if z.state == "ONLINE" {
                            heat(z.pct)
                        } else {
                            Color::Red
                        }),
                    ));
                    lines.push(Line::from(fit(l, inner_w)));
                    if z.state != "ONLINE" {
                        lines.push(Line::from(fit(
                            vec![Span::styled(
                                format!("  ⚠ pool state: {}", z.state),
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            )],
                            inner_w,
                        )));
                    }
                }
                // capped datasets: the limits Proxmox actually enforces
                let capped: Vec<&crate::model::DatasetStat> =
                    m.datasets.iter().filter(|d| d.cap > 0).take(8).collect();
                for d in capped {
                    let short: String = d
                        .name
                        .rsplit('/')
                        .next()
                        .unwrap_or(&d.name)
                        .to_string()
                        .replace("-disk-0", "")
                        .replace("-disk-", ":")
                        .chars()
                        .take(15)
                        .collect();
                    let tag = if d.vtype == "volume" { "vol" } else { "sub" };
                    let size_s = format!("{}/{}", fmt_bytes(d.used), fmt_bytes(d.cap));
                    let lbl_w = 21usize;
                    let bar_w = inner_w
                        .saturating_sub(lbl_w + size_s.chars().count() + 6)
                        .clamp(4, 12);
                    let mut l = vec![Span::styled(
                        format!("{:<1$}", format!("[{tag}] {short}"), lbl_w),
                        Style::default().fg(C_TEXT),
                    )];
                    l.extend(gauge(
                        None,
                        d.pct,
                        100.0,
                        bar_w,
                        "%",
                        heat(d.pct),
                        Some(format!("{:.0}%", d.pct)),
                    ));
                    l.push(Span::styled(
                        format!(" {size_s}"),
                        Style::default().fg(heat(d.pct)),
                    ));
                    lines.push(Line::from(fit(l, inner_w)));
                }
                let uncapped = m.datasets.iter().filter(|d| d.cap == 0).count();
                if uncapped > 0 {
                    lines.push(Line::from(fit(
                        vec![Span::styled(
                            format!("  +{} datasets w/o own limit (pool-shared)", uncapped),
                            Style::default().fg(C_MUTED),
                        )],
                        inner_w,
                    )));
                }
            } else if m.disks.iter().any(|d| d.fs == "zfs") {
                lines.push(Line::from(fit(
                    vec![Span::styled(
                        "  zfs: pool stats n/a (zpool list needs root)",
                        Style::default().fg(C_MUTED),
                    )],
                    inner_w,
                )));
            }

            // ---- NET ----
            lines.push(Line::from(Span::styled(
                "net ",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            )));
            if m.nets.is_empty() {
                lines.push(Line::from(Span::styled(
                    " none",
                    Style::default().fg(C_MUTED),
                )));
            }
            for n in m.nets.iter().take(2) {
                let mut spans = Vec::new();
                let label = if n.speed_mbps > 0 {
                    format!("{}@{} ", n.name, n.speed_mbps)
                } else {
                    format!("{} ", n.name)
                };
                spans.push(Span::styled(
                    label,
                    Style::default().fg(if n.state == "up" { C_TEXT } else { Color::Red }),
                ));
                spans.push(Span::styled(
                    format!("↓{} ↑{} ", fmt_bps(n.rx_bps), fmt_bps(n.tx_bps)),
                    Style::default().fg(Color::Blue),
                ));
                let used_cols = spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>();
                let (sp, _) = sparkline(
                    &h.hist_rx.iter().cloned().collect::<Vec<f64>>(),
                    inner_w.saturating_sub(used_cols).max(4),
                    Color::Blue,
                );
                spans.extend(sp);
                lines.push(Line::from(fit(spans, inner_w)));
            }

            // ---- GPU: every card, every compute proc. Nothing is dropped for
            // lack of vertical room anymore — the card scrolls (↕ in its title).
            if !m.gpus.is_empty() {
                lines.push(Line::from(Span::styled(
                    "gpu ",
                    Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
                )));
                for g in m.gpus.iter().take(4) {
                    let short_name: String = g
                        .name
                        .replace("NVIDIA GeForce ", "")
                        .replace("NVIDIA ", "")
                        .chars()
                        .take(16)
                        .collect();
                    // util bar full width, labeled
                    let mut ul = vec![Span::styled(
                        format!("G{} ", g.idx),
                        Style::default().fg(C_TEXT),
                    )];
                    ul.push(Span::styled(
                        format!("{:.0}% ", g.util),
                        Style::default().fg(heat(g.util)),
                    ));
                    ul.extend(stacked_bar(
                        inner_w.saturating_sub(7),
                        &[(g.util, heat(g.util), "")],
                        100.0,
                    ));
                    lines.push(Line::from(fit(ul, inner_w)));
                    // vram segmented bar (skipped when unknown, e.g. PCI-presence-only)
                    let vt = g.mem_total_mb.max(1) as f64;
                    let vu = g.mem_used_mb as f64;
                    let vf = (vt - vu).max(0.0);
                    if g.mem_total_mb > 0 {
                        let mut vl = vec![Span::styled(
                            format!("{short_name} "),
                            Style::default().fg(C_MUTED),
                        )];
                        let name_w = short_name.chars().count() + 1;
                        let vram_txt = format!(" {:.1}/{:.1}Gi", vu / 1024.0, vt / 1024.0);
                        // fill every cell left between name and readout — no cap:
                        // a short fixed bar made the VRAM row look truncated on wide cards
                        let bar_w = inner_w
                            .saturating_sub(name_w + vram_txt.chars().count())
                            .max(6);
                        vl.extend(stacked_bar(
                            bar_w,
                            &[(vu, Color::Magenta, ""), (vf, Color::Rgb(60, 60, 60), "")],
                            vt,
                        ));
                        vl.push(Span::styled(vram_txt, Style::default().fg(C_TEXT)));
                        lines.push(Line::from(fit(vl, inner_w)));
                    } else {
                        lines.push(Line::from(fit(
                            vec![Span::styled(
                                format!("{short_name} · PCI presence only, no metrics"),
                                Style::default().fg(C_MUTED),
                            )],
                            inner_w,
                        )));
                    }
                    // env stats get their own row so wattage never truncates
                    let mut es = vec![Span::styled("  ", Style::default())];
                    if let Some(t) = g.temp_c {
                        es.push(Span::styled(
                            format!("{:.0}°C · ", t),
                            Style::default().fg(heat_temp(t)),
                        ));
                    }
                    if let Some(pw) = g.power_w {
                        es.push(Span::styled(
                            match g.power_cap_w {
                                Some(c) if c > 0.0 => format!("{:.0}/{}W", pw, c as u64),
                                _ => format!("{:.0}W", pw),
                            },
                            Style::default().fg(C_TEXT),
                        ));
                    }
                    if let Some(f) = g.fan_pct {
                        es.push(Span::styled(
                            format!(" · fan {:.0}%", f),
                            Style::default().fg(C_MUTED),
                        ));
                    }
                    if es.len() > 1 {
                        lines.push(Line::from(fit(es, inner_w)));
                    }
                    // compute processes: full cmd, wrapped to width
                    if g.procs.is_empty() {
                        lines.push(Line::from(Span::styled(
                            "  no compute procs",
                            Style::default().fg(C_MUTED),
                        )));
                    } else {
                        let head_w = 19usize; // "{:>5}MB {:<10} "
                        let cmd_w = inner_w.saturating_sub(head_w).max(8);
                        let cap_show = 16usize;
                        for pr in g.procs.iter().take(cap_show) {
                            let ws = wrap_cmd(&pr.name, cmd_w);
                            let usr: String = pr.user.chars().take(10).collect();
                            let head = format!("{:>5}MB {:<10} ", pr.mem_mb, usr);
                            for (li, chunk) in ws.iter().enumerate() {
                                emit_proc(&mut lines, &head, li == 0, chunk, inner_w, head_w);
                            }
                        }
                        if g.procs.len() > cap_show {
                            lines.push(Line::from(Span::styled(
                                format!("  … {} more", g.procs.len() - cap_show),
                                Style::default().fg(C_MUTED),
                            )));
                        }
                    }
                }
            }
        }
        (None, Some(p)) => {
            lines.push(Line::from(Span::styled(
                format!("{} · {}", p.arch, p.kernel),
                Style::default().fg(C_TEXT),
            )));
            lines.push(Line::from(Span::styled(
                "streaming… first rates need two frames",
                Style::default().fg(C_MUTED),
            )));
        }
        (None, None) | (Some(_), None) => {
            let msg = match h.state {
                HostState::Error => format!(
                    "⚠ {} ({} fails, reconnecting…)",
                    h.last_error.clone().unwrap_or_default(),
                    h.consecutive_errors
                ),
                _ => "connecting / streaming…".into(),
            };
            lines.push(Line::from(Span::styled(
                format!(" {}", msg),
                Style::default().fg(if h.state == HostState::Error {
                    Color::Red
                } else {
                    Color::Yellow
                }),
            )));
        }
    }

    // ---- scrollable render ----
    let overflow = lines.len().saturating_sub(inner_h);
    let eff_scroll = scroll.min(overflow);
    let title_str = if inner_w > 30 {
        format!(" {name} · {src_tag} ")
    } else {
        format!(" {name} ")
    };
    let title_str = if overflow > 0 {
        let lo = eff_scroll + 1;
        let hi = (eff_scroll + inner_h).min(lines.len());
        format!("{title_str}↕{lo}-{hi}/{} ", lines.len())
    } else {
        title_str
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if active { C_ACCENT } else { C_MUTED }))
        .title(Span::styled(
            title_str,
            Style::default()
                .fg(if active { C_ACCENT } else { Color::White })
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let para = Paragraph::new(lines).scroll((eff_scroll as u16, 0));
    f.render_widget(para, inner);
    overflow
}

/// One segmented full-width bar: proportional colored blocks, no gaps, no
/// rainbow legend — each segment length *is* the share of `total`.
fn stacked_bar(w: usize, parts: &[(f64, Color, &str)], total: f64) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut assigned = 0usize;
    let n = parts.len();
    for (i, (v, col, _label)) in parts.iter().enumerate() {
        let seg = if i == n - 1 {
            w.saturating_sub(assigned)
        } else {
            let raw = (v / total.max(f64::EPSILON)) * w as f64;
            let c = (raw.round() as usize).min(w.saturating_sub(assigned));
            assigned += c;
            c
        };
        if seg > 0 {
            spans.push(Span::styled("█".repeat(seg), Style::default().fg(*col)));
        }
    }
    spans
}

fn seg_label(col: Color, name: &str, value: String) -> Span<'static> {
    Span::styled(format!("■ {name} {value}   "), Style::default().fg(col))
}

/// one wrapped line of a GPU compute process: header (mem/user) on first line,
/// indent on continuation lines, command text as given
fn emit_proc(
    lines: &mut Vec<Line>,
    head: &str,
    first: bool,
    text: &str,
    inner_w: usize,
    head_w: usize,
) {
    let mut spans = if first {
        let mb_end = head.find("MB").map(|i| i + 2).unwrap_or(0);
        vec![
            Span::styled(
                head[..mb_end].to_string(),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(head[mb_end..].to_string(), Style::default().fg(C_MUTED)),
        ]
    } else {
        vec![Span::styled(
            " ".repeat(head_w),
            Style::default().fg(C_MUTED),
        )]
    };
    spans.push(Span::styled(text.to_string(), Style::default().fg(C_TEXT)));
    lines.push(Line::from(fit(spans, inner_w)));
}

/// word-wrap a full command line to `w` cells; long tokens break at '/' first,
/// then hard-split. Always returns >=1 line.
fn wrap_cmd(name: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut flush = |out: &mut Vec<String>, s: &mut String| {
        if !s.is_empty() {
            out.push(std::mem::take(s));
        }
    };
    for word in name.split_whitespace() {
        if word.chars().count() > w {
            flush(&mut out, &mut line);
            let mut cur = String::new();
            for seg in word.split_inclusive('/') {
                if seg.chars().count() > w {
                    flush(&mut out, &mut cur);
                    for part in seg.chars().collect::<Vec<char>>().chunks(w) {
                        out.push(part.iter().collect::<String>());
                    }
                    continue;
                }
                if cur.chars().count() + seg.chars().count() > w && !cur.is_empty() {
                    flush(&mut out, &mut cur);
                }
                cur.push_str(seg);
            }
            flush(&mut out, &mut cur);
        } else if line.is_empty() {
            line = word.to_string();
        } else if line.chars().count() + 1 + word.chars().count() <= w {
            line.push(' ');
            line.push_str(word);
        } else {
            flush(&mut out, &mut line);
            line = word.to_string();
        }
    }
    flush(&mut out, &mut line);
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 74u16.min(area.width.saturating_sub(4));
    let h = 20u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    let text = vec![
        Line::from(Span::styled(
            "skimonitor — up to 3 hosts, all live on one screen",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" a        add host (user@host, ssh alias, IP; space/comma)"),
        Line::from(" ←→/Tab   cycle focus (highlight)  1/2/3  select card"),
        Line::from(" x/Del    drop focused card        r    restart all streams"),
        Line::from(" +/-      stream interval 1..60s   p    pause / resume"),
        Line::from(" ↑↓/PgUp/PgDn   scroll focused card (↕ lines in its title)"),
        Line::from(" Home/End jump to top / bottom of the focused card"),
        Line::from(" wheel    scroll the card under the mouse (if reported)"),
        Line::from(" ? this help        q / Ctrl-C quit"),
        Line::from(""),
        Line::from(Span::styled(
            " mem bar:  red = used, amber = cache/reclaimable, dark = free.",
            Style::default().fg(C_TEXT),
        )),
        Line::from(Span::styled(
            " gpu bar:  purple = vram in use.        widths are true fractions",
            Style::default().fg(C_TEXT),
        )),
        Line::from(Span::styled(
            " zfs:  pool row = real device use (df lies on ZFS); sub/vol rows",
            Style::default().fg(C_TEXT),
        )),
        Line::from(Span::styled(
            "       show quota/refquota/volsize — the limits Proxmox enforces.",
            Style::default().fg(C_TEXT),
        )),
        Line::from(Span::styled(
            " Auth: system `ssh` BatchMode — keys/agent/config of this box.",
            Style::default().fg(C_TEXT),
        )),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(C_ACCENT))
                .title(" help "),
        ),
        rect,
    );
}

/// Truncate a span list to `w` display columns (overflow would clobber borders).
fn fit(spans: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let mut used = 0usize;
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        let len = s.content.chars().count();
        if used + len <= w {
            used += len;
            out.push(s);
        } else {
            let take = w.saturating_sub(used);
            if take > 0 {
                let cut: String = s.content.chars().take(take).collect();
                out.push(Span::styled(cut, s.style));
            }
            break;
        }
    }
    out
}

fn short_dev(d: &str) -> String {
    d.rsplit('/').next().unwrap_or(d).to_string()
}
