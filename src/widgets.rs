//! Low-cost visual widgets: gauge bars, braille sparklines, block-arc pie charts.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::f64::consts::PI;

/// heat color for a 0..100 percentage (util-like)
pub fn heat(pct: f64) -> Color {
    if pct >= 85.0 {
        Color::Red
    } else if pct >= 60.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// heat color for temperature in °C
pub fn heat_temp(t: f64) -> Color {
    if t >= 85.0 {
        Color::Red
    } else if t >= 65.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Horizontal Unicode-block gauge: `label [██████▋      ] 73%`
pub fn gauge(
    label: Option<&str>,
    value: f64,
    max: f64,
    width: usize,
    unit: &str,
    color: Color,
    right: Option<String>,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(l) = label {
        spans.push(Span::styled(
            format!("{l:<6}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let w = width.max(3);
    let v = if max > 0.0 {
        value.clamp(0.0, max)
    } else {
        0.0
    };
    let tenths = ((v / max.max(f64::EPSILON)) * (w as f64 * 10.0)).round() as usize;
    let full = (tenths / 10).min(w);
    let frac = tenths % 10;
    const PART: [char; 10] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█', '█'];
    let mut fill: String = "\u{2588}".repeat(full);
    let mut rest = w - full;
    if rest > 0 {
        fill.push(PART[frac]);
        rest -= 1;
    }
    spans.push(Span::styled(
        "[".to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(fill, Style::default().fg(color)));
    if rest > 0 {
        spans.push(Span::styled(
            "\u{2591}".repeat(rest),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::styled(
        "]".to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    let value_text = right.unwrap_or_else(|| format!("{:.0}{unit}", value));
    spans.push(Span::styled(
        format!(" {value_text}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    spans
}

/// Compact bar without brackets/label-padding: `0 ████████░░ 100%`
pub fn mini(label: &str, pct: f64, bar_w: usize) -> Vec<Span<'static>> {
    let c = heat(pct);
    let filled = ((pct / 100.0) * bar_w as f64).round() as usize;
    let filled = filled.min(bar_w);
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("{label:>2} "),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled("█".repeat(filled), Style::default().fg(c)));
    if bar_w - filled > 0 {
        spans.push(Span::styled(
            "░".repeat(bar_w - filled),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::styled(
        format!(" {:>3.0}%", pct),
        Style::default().fg(c),
    ));
    spans
}

/// Braille sparkline: 2 samples per cell, 4 vertical levels.
/// Returns spans + the auto-scale max actually used.
pub fn sparkline(values: &[f64], width_cells: usize, color: Color) -> (Vec<Span<'static>>, f64) {
    if width_cells == 0 {
        return (vec![], 0.0);
    }
    let n = width_cells * 2;
    let mut data: Vec<f64> = values.to_vec();
    if data.len() > n {
        data = data.split_at(data.len() - n).1.to_vec();
    }
    if data.is_empty() {
        return (
            vec![Span::styled(
                " ".repeat(width_cells),
                Style::default().fg(color),
            )],
            0.0,
        );
    }
    let mut mx = data.iter().fold(0.0f64, |a, b| a.max(*b));
    if mx <= 0.0 {
        mx = 1.0;
    }
    // level 0..3 per sample
    let lv: Vec<u8> = data
        .iter()
        .map(|v| ((v.clamp(0.0, mx) / mx) * 3.999).floor() as u8)
        .collect();
    // braille dots, bottom-up: left col dots 1,2,4,7 (rows 0..3), right col 4,5,6? —
    // standard: U+2800 bit0=dot1 (top-left), bit1=dot2 (2nd row left), bit2=dot3 top-right?
    // Layout (rows top→bottom 0..3):
    //   dot1 = row0 left, dot2 = row1 left, dot3 = row2 left, dot7 = row3 left
    //   dot4 = row0 right, dot5 = row1 right, dot6 = row2 right, dot8 = row3 right
    // value 0 → baseline (bottom row, level0), value 3 → top row.
    let mut out = String::new();
    let mut i = 0;
    while i < lv.len() {
        let l = lv[i];
        let r = lv.get(i + 1).copied().unwrap_or(0);
        let mut bits = 0u16;
        // left dot at row (3 - l)  (l=0 → bottom row 3 → dot7 = bit6)
        let lrow = 3 - l;
        bits |= match lrow {
            0 => 1 << 0, // dot1
            1 => 1 << 1, // dot2
            2 => 1 << 2, // dot3
            _ => 1 << 6, // dot7
        };
        let rrow = 3 - r;
        bits |= match rrow {
            0 => 1 << 3, // dot4
            1 => 1 << 4, // dot5
            2 => 1 << 5, // dot6
            _ => 1 << 7, // dot8
        };
        out.push(char::from_u32(0x2800 + bits as u32).unwrap_or(' '));
        i += 2;
    }
    while out.chars().count() < width_cells {
        out.push(' ');
    }
    (vec![Span::styled(out, Style::default().fg(color))], mx)
}

pub struct Slice {
    pub color: Color,
    pub label: String,
    pub value: f64,
}

/// Angle (deg, 0..360) clockwise from 12 o'clock for a pixel at (dx, dy-up).
fn ang_cw_from_top(dx: f64, dy_up: f64) -> f64 {
    let mut a = 90.0 - dy_up.atan2(dx).to_degrees();
    if a < 0.0 {
        a += 360.0;
    }
    a % 360.0
}

/// Block-arc pie: rows∈{2,3}, cols = 2*rows + 1. Renders into colored Spans.
pub fn pie(slices: &[Slice], rows: usize) -> Vec<Line<'static>> {
    let cols = rows * 2 + 1;
    let total: f64 = slices.iter().map(|s| s.value.max(0.0)).sum();
    // cumulative clockwise from top: (start, end, color)
    let mut segs: Vec<(f64, f64, Color)> = Vec::new();
    if total > 0.0 {
        let mut acc = 0.0;
        for s in slices {
            let span = s.value.max(0.0) / total * 360.0;
            if span > 0.05 {
                segs.push((acc, acc + span, s.color));
            }
            acc += span;
        }
    } else {
        segs.push((0.0, 360.0, Color::DarkGray));
    }

    // pixel model: each cell = 2 half-pixels tall (char aspect ≈ 1:2), 1 wide.
    // sample 2 subrows per char row to get finer edges; pick block by coverage.
    let mut lines = Vec::new();
    let r_px = rows as f64; // radius in char rows == half-pixels/2
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run: Option<(String, Color)> = None;
        let mut flush = |spans: &mut Vec<Span<'static>>, run: &mut Option<(String, Color)>| {
            if let Some((t, c)) = run.take() {
                spans.push(Span::styled(t, Style::default().fg(c)));
            }
        };
        for col in 0..cols {
            // cell center in "unit" coords: x in [-rows, rows], y-up in [-rows, rows]
            let cx = (col as f64 + 0.5) - (cols as f64 / 2.0);
            let cy = r_px - (row as f64 + 0.5); // top row → positive
                                                // test 4 sample points (2x2) per cell for coverage
            let mut cnt = vec![0usize; segs.len()];
            let mut inside = 0usize;
            for sy in 0..2 {
                for sx in 0..2 {
                    let px = cx - 0.25 + sx as f64 * 0.5;
                    let py = cy - 0.25 + sy as f64 * 0.5;
                    let d = (px * px + py * py).sqrt();
                    if d <= r_px * (1.0 + 0.42 / (r_px + 0.5)) {
                        inside += 1;
                        let a = ang_cw_from_top(px, py);
                        if let Some(k) = segs.iter().position(|(s, e, _)| a >= *s && a < *e) {
                            cnt[k] += 1;
                        }
                    }
                }
            }
            if inside == 0 {
                flush(&mut spans, &mut run);
                run.get_or_insert((" ".to_string(), Color::Reset));
                // merge spaces into the run
                continue;
            }
            let best = (0..segs.len()).max_by_key(|k| cnt[*k]).unwrap_or(0);
            let frac = cnt[best] as f64 / 4.0;
            let (ch, c) = if frac >= 0.75 {
                ('█', segs[best].2)
            } else if frac >= 0.5 {
                ('▊', segs[best].2)
            } else if frac >= 0.25 {
                ('▍', segs[best].2)
            } else if inside == 4 {
                ('▁', segs[best].2)
            } else {
                ('▁', segs[best].2)
            };
            match &mut run {
                Some((t, cc)) if *cc == c => t.push(ch),
                _ => {
                    flush(&mut spans, &mut run);
                    run = Some((ch.to_string(), c));
                }
            }
        }
        flush(&mut spans, &mut run);
        if spans.is_empty() {
            spans.push(Span::raw(" ".repeat(cols)));
        }
        lines.push(Line::from(spans));
    }
    let _ = PI;
    lines
}
