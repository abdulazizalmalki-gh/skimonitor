//! Renders the dashboard to a cell grid and serializes it for the browser.
//!
//! The screen is produced by the SAME `ui::draw` the terminal front end uses,
//! into a ratatui `TestBackend` buffer instead of a terminal. Nothing about the
//! layout is reimplemented in HTML/CSS: the browser receives the exact cells
//! (glyph, foreground, background, bold) that ratatui painted, which is what
//! makes the web UI identical to the TUI — down to the cell.

use crate::app::App;
use crate::ui::CardGeom;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use std::collections::HashMap;

/// ANSI palette of the canonical screenshot (docs/screenshot.png). The TUI
/// leaves these to the terminal theme; the browser needs concrete values, so
/// the project's own screenshot palette is the reference.
fn css_color(c: Color) -> Option<&'static str> {
    Some(match c {
        Color::Reset => return None,
        Color::Black => "#000000",
        Color::Red => "#cd0000",
        Color::Green => "#00cd00",
        Color::Yellow => "#cdcd00",
        Color::Blue => "#0000ee",
        Color::Magenta => "#cd00cd",
        Color::Cyan => "#00cdcd",
        Color::Gray => "#e5e5e5",
        Color::DarkGray => "#7f7f7f",
        Color::LightRed => "#ff0000",
        Color::LightGreen => "#90ee90",
        Color::LightYellow => "#ffff00",
        Color::LightBlue => "#0000ff",
        Color::LightMagenta => "#ff00ff",
        Color::LightCyan => "#00ffff",
        Color::White => "#ffffff",
        Color::Rgb(..) | Color::Indexed(_) => return None,
    })
}

fn rgb_css(c: Color) -> Option<String> {
    match c {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        other => css_color(other).map(|s| s.to_string()),
    }
}

/// compact key for a (fg, bg, bold) triple
fn style_key(fg: Color, bg: Color, bold: bool) -> (String, String, bool) {
    (
        rgb_css(fg).unwrap_or_default(),
        rgb_css(bg).unwrap_or_default(),
        bold,
    )
}

pub struct RenderOut {
    pub json: String,
    /// per-card max scroll (line overflow), indexed by host
    pub maxes: Vec<usize>,
    /// per-card rects, indexed by host
    pub geoms: Vec<CardGeom>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Renderer {
    term: Terminal<TestBackend>,
    cols: u16,
    rows: u16,
}

impl Renderer {
    pub fn new(cols: u16, rows: u16) -> Self {
        let (cols, rows) = (cols.max(20), rows.max(8));
        Self {
            term: Terminal::new(TestBackend::new(cols, rows)).expect("test backend"),
            cols,
            rows,
        }
    }

    fn ensure(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(20), rows.max(8));
        if cols != self.cols || rows != self.rows {
            self.term = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
            self.cols = cols;
            self.rows = rows;
        }
    }

    /// Draw the app at `cols` x `rows` and serialize the resulting cells.
    pub fn draw(&mut self, app: &App, cols: u16, rows: u16) -> RenderOut {
        self.ensure(cols, rows);
        let scrolls = app.scrolls.clone();
        let mut result = (Vec::new(), Vec::new());
        self.term
            .draw(|f| {
                result = crate::ui::draw(
                    f,
                    &app.hosts,
                    &app.rows,
                    app.sel,
                    app.interval,
                    &app.input,
                    app.help,
                    app.pending(),
                    app.paused,
                    &app.note,
                    &scrolls,
                    true, // web front end: no quit keys, help closes with Esc
                );
            })
            .expect("draw");
        let (maxes, geoms) = result;
        let buf = self.term.backend().buffer().clone();
        RenderOut {
            json: serialize(&buf, app.seq, self.cols, self.rows),
            maxes,
            geoms,
            cols: self.cols,
            rows: self.rows,
        }
    }
}

/// One-off render for the snapshot endpoint and the tests: same code path, no
/// state kept.
pub fn render_once(app: &App, cols: u16, rows: u16) -> RenderOut {
    Renderer::new(cols, rows).draw(app, cols, rows)
}

fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn style_index(
    styles: &mut Vec<(String, String, bool)>,
    index: &mut HashMap<(String, String, bool), usize>,
    fg: Color,
    bg: Color,
    bold: bool,
) -> usize {
    let key = style_key(fg, bg, bold);
    if let Some(i) = index.get(&key) {
        return *i;
    }
    styles.push(key.clone());
    index.insert(key, styles.len() - 1);
    styles.len() - 1
}

fn serialize(buf: &Buffer, seq: u64, cols: u16, rows: u16) -> String {
    let mut styles: Vec<(String, String, bool)> = Vec::new();
    let mut index: HashMap<(String, String, bool), usize> = HashMap::new();

    // runs: (text, style) per row
    let mut lines: Vec<Vec<(String, usize)>> = Vec::with_capacity(rows as usize);
    for y in 0..rows {
        let mut runs: Vec<(String, usize)> = Vec::new();
        let mut cur = String::new();
        let mut cur_style: Option<usize> = None;
        for x in 0..cols {
            let cell = &buf[(x, y)];
            let bold = cell.modifier.contains(Modifier::BOLD);
            let idx = style_index(&mut styles, &mut index, cell.fg, cell.bg, bold);
            if cur_style == Some(idx) {
                cur.push_str(cell.symbol());
            } else {
                if let Some(s) = cur_style {
                    runs.push((std::mem::take(&mut cur), s));
                }
                cur.push_str(cell.symbol());
                cur_style = Some(idx);
            }
        }
        if let Some(s) = cur_style {
            runs.push((cur, s));
        }
        // drop trailing blank cells that carry no styling at all: the page
        // background shows through them anyway
        while let Some((text, idx)) = runs.last() {
            let (fg, bg, _) = &styles[*idx];
            if !fg.is_empty() || !bg.is_empty() {
                break;
            }
            let trimmed = text.trim_end().to_string();
            if trimmed.is_empty() {
                runs.pop();
            } else {
                let last = runs.len() - 1;
                runs[last].0 = trimmed;
                break;
            }
        }
        lines.push(runs);
    }

    let mut out = String::with_capacity(8 * 1024);
    out.push_str(&format!(
        "{{\"seq\":{seq},\"cols\":{cols},\"rows\":{rows},\"pal\":["
    ));
    for (i, (fg, bg, bold)) in styles.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        push_json_str(&mut out, fg);
        out.push(',');
        push_json_str(&mut out, bg);
        out.push_str(if *bold { ",1]" } else { ",0]" });
    }
    out.push_str("],\"lines\":[");
    for (y, runs) in lines.iter().enumerate() {
        if y > 0 {
            out.push(',');
        }
        out.push('[');
        for (x, (text, idx)) in runs.iter().enumerate() {
            if x > 0 {
                out.push(',');
            }
            out.push('[');
            push_json_str(&mut out, text);
            out.push_str(&format!(",{idx}]"));
        }
        out.push(']');
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{handle_key, App};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn demo_app() -> App {
        let ts: Vec<String> = crate::demo::DEMO_HOSTS
            .iter()
            .map(|h| h.target.to_string())
            .collect();
        App::new(&ts, 1, true)
    }

    #[test]
    fn header_carries_the_version_chip() {
        let app = demo_app();
        let out = render_once(&app, 150, 46);
        assert!(out.json.contains("skimonitor"));
        assert!(out.json.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn web_frames_drop_the_quit_hint() {
        let app = demo_app();
        let out = render_once(&app, 150, 46);
        assert!(!out.json.contains("quit"));
    }

    #[test]
    fn screen_is_exactly_the_requested_grid() {
        let app = demo_app();
        let out = render_once(&app, 120, 40);
        assert_eq!(out.cols, 120);
        assert_eq!(out.rows, 40);
        assert!(out.json.matches("],[").count() + 1 >= 40);
    }

    #[test]
    fn help_overlay_renders() {
        let mut app = demo_app();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        let out = render_once(&app, 150, 46);
        assert!(out.json.contains("up to 6 hosts"));
    }

    #[test]
    fn geometry_reaches_every_card() {
        let app = demo_app();
        let out = render_once(&app, 150, 46);
        assert_eq!(out.geoms.len(), 3);
        assert!(out.geoms.iter().all(|g| g.x1 > g.x0 && g.y1 > g.y0));
    }
}
