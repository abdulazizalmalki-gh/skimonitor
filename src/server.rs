//! Web front end: a small dependency-free HTTP/1.1 server (the dashboard page,
//! its assets, an SSE stream of rendered frames, and the action endpoints the
//! browser calls) plus the render loop that feeds it.
//!
//! Deliberately thread-per-connection with `std` only: a dashboard has one or
//! two browsers attached, so an async runtime would be more moving parts than
//! the problem needs.

use crate::app::{handle_key, App};
use crate::view;
use crossterm::event::{KeyCode, KeyModifiers};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const INDEX_HTML: &str = include_str!("../web/index.html");
pub const APP_JS: &str = include_str!("../web/app.js");
pub const STYLE_CSS: &str = include_str!("../web/style.css");
pub const FONT_REGULAR: &[u8] = include_bytes!("../web/JetBrainsMono-Regular.woff2");
pub const FONT_BOLD: &[u8] = include_bytes!("../web/JetBrainsMono-Bold.woff2");

pub struct Shared {
    pub app: Mutex<App>,
    /// grid the browsers asked for (cells)
    size: Mutex<(u16, u16)>,
    clients: Mutex<Vec<(u64, Sender<String>)>>,
    /// something changed outside the stream (a key, a resize)
    dirty: AtomicBool,
    next_client: AtomicU64,
}

impl Shared {
    fn new(app: App) -> Arc<Self> {
        Arc::new(Self {
            app: Mutex::new(app),
            size: Mutex::new((150, 46)),
            clients: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(true),
            next_client: AtomicU64::new(1),
        })
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn client_count(&self) -> usize {
        self.clients.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// push one frame to every attached browser; drop the ones that went away
    fn broadcast(&self, frame: String) {
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        clients.retain(|(_, tx)| tx.send(frame.clone()).is_ok());
    }
}

fn set_size(shared: &Shared, cols: u16, rows: u16) {
    if let Ok(mut s) = shared.size.lock() {
        let cols = cols.clamp(20, 500);
        let rows = rows.clamp(8, 200);
        if *s != (cols, rows) {
            *s = (cols, rows);
            shared.mark_dirty();
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: String,
    body: String,
}

fn parse_query(q: &str, key: &str) -> Option<String> {
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let head_end;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = find_head_end(&buf) {
                    head_end = p;
                    break;
                }
                if buf.len() > 64 * 1024 {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let body_start = head_end + 4;
    let mut body = buf.get(body_start..).map(|b| b.to_vec()).unwrap_or_default();
    while body.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    body.truncate(content_length);
    Some(Request {
        method,
        path,
        query,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    }
}

fn respond(stream: &mut TcpStream, code: u16, mime: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        reason(code),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Browser key message -> the key the TUI would have received, with the two
/// policies a server needs: a keystroke cannot stop it, and Esc closes help.
fn apply_web_key(app: &mut App, code: KeyCode) {
    if app.input.is_none() {
        match code {
            KeyCode::Char('q') => return,
            KeyCode::Esc => {
                app.help = false;
                return;
            }
            _ => {}
        }
    }
    handle_key(app, code, KeyModifiers::NONE);
}

fn key_from_json(v: &serde_json::Value) -> Option<KeyCode> {
    let k = v.get("k")?.as_str()?;
    Some(match k {
        "c" => {
            let s = v.get("v")?.as_str()?;
            let mut it = s.chars();
            let c = it.next()?;
            if it.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        _ => return None,
    })
}

fn handle_connection(mut stream: TcpStream, shared: Arc<Shared>) {
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    let path = req.path.as_str();
    match (req.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => respond(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        ),
        ("GET", "/app.js") => respond(
            &mut stream,
            200,
            "application/javascript; charset=utf-8",
            APP_JS.as_bytes(),
        ),
        ("GET", "/style.css") => respond(
            &mut stream,
            200,
            "text/css; charset=utf-8",
            STYLE_CSS.as_bytes(),
        ),
        ("GET", "/font/regular.woff2") => {
            respond(&mut stream, 200, "font/woff2", FONT_REGULAR)
        }
        ("GET", "/font/bold.woff2") => respond(&mut stream, 200, "font/woff2", FONT_BOLD),
        ("GET", "/health") => respond(&mut stream, 200, "text/plain; charset=utf-8", b"ok\n"),
        ("GET", "/api/size") => {
            let (c, r) = *shared.size.lock().unwrap();
            let body = format!("{{\"cols\":{c},\"rows\":{r}}}");
            respond(
                &mut stream,
                200,
                "application/json; charset=utf-8",
                body.as_bytes(),
            );
        }
        ("GET", "/events") => stream_events(stream, shared),
        ("GET", "/api/screen") => {
            let (dc, dr) = *shared.size.lock().unwrap();
            let cols = parse_query(&req.query, "cols")
                .and_then(|v| v.parse().ok())
                .unwrap_or(dc);
            let rows = parse_query(&req.query, "rows")
                .and_then(|v| v.parse().ok())
                .unwrap_or(dr);
            let app = shared.app.lock().unwrap();
            let out = view::render_once(&app, cols, rows);
            drop(app);
            respond(
                &mut stream,
                200,
                "application/json; charset=utf-8",
                out.json.as_bytes(),
            );
        }
        ("POST", "/api/key") => {
            let v: serde_json::Value = match serde_json::from_str(&req.body) {
                Ok(v) => v,
                Err(_) => return respond(&mut stream, 400, "application/json", b"{\"ok\":false}"),
            };
            if let Some(code) = key_from_json(&v) {
                if let Ok(mut app) = shared.app.lock() {
                    apply_web_key(&mut app, code);
                }
                shared.mark_dirty();
            }
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("POST", "/api/click") => {
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            let x = v.get("x").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
            let y = v.get("y").and_then(|y| y.as_u64()).unwrap_or(0) as u16;
            let mut changed = false;
            if let Ok(mut app) = shared.app.lock() {
                if let Some(i) = app.card_at(x, y) {
                    if app.sel != i {
                        app.sel = i;
                        changed = true;
                    }
                }
            }
            if changed {
                shared.mark_dirty();
            }
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("POST", "/api/wheel") => {
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            let x = v.get("x").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
            let y = v.get("y").and_then(|y| y.as_u64()).unwrap_or(0) as u16;
            let d = v.get("d").and_then(|d| d.as_i64()).unwrap_or(0);
            if let Ok(mut app) = shared.app.lock() {
                let i = app.card_at(x, y).unwrap_or(app.sel);
                app.scroll_card(i, d as isize);
            }
            shared.mark_dirty();
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("POST", "/api/size") => {
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            let cols = v.get("cols").and_then(|x| x.as_u64()).unwrap_or(150) as u16;
            let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(46) as u16;
            set_size(&shared, cols, rows);
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("POST", "/api/add") => {
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            let targets = v.get("targets").and_then(|t| t.as_str()).unwrap_or("");
            if let Ok(mut app) = shared.app.lock() {
                for t in targets.split([',', ' ', ';', '\t']) {
                    app.add_target(t);
                }
            }
            shared.mark_dirty();
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("POST", "/api/action") => {
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
            let action = v.get("a").and_then(|a| a.as_str()).unwrap_or("");
            if let Ok(mut app) = shared.app.lock() {
                match action {
                    "restart" => app.restart_all(),
                    "pause" => {
                        let p = !app.paused;
                        app.set_paused(p);
                    }
                    "drop" => app.remove_selected(),
                    _ => {}
                }
            }
            shared.mark_dirty();
            respond(&mut stream, 204, "text/plain", b"");
        }
        ("GET", _) | ("HEAD", _) => respond(&mut stream, 404, "text/plain", b"not found\n"),
        _ => respond(&mut stream, 405, "text/plain", b"method not allowed\n"),
    }
}

fn stream_events(mut stream: TcpStream, shared: Arc<Shared>) {
    stream.set_read_timeout(None).ok();
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    let (tx, rx) = mpsc::channel::<String>();
    let id = shared.next_client.fetch_add(1, Ordering::SeqCst);
    {
        let Ok(mut clients) = shared.clients.lock() else {
            return;
        };
        clients.push((id, tx));
    }
    shared.mark_dirty();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\nretry: 1000\n\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let _ = stream.flush();
    while let Ok(frame) = rx.recv() {
        let mut payload = String::with_capacity(frame.len() + 16);
        payload.push_str("data: ");
        payload.push_str(&frame);
        payload.push_str("\n\n");
        if stream.write_all(payload.as_bytes()).is_err() {
            break;
        }
        let _ = stream.flush();
    }
    if let Ok(mut clients) = shared.clients.lock() {
        clients.retain(|(i, _)| *i != id);
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let _ = stream.set_nodelay(true);
            handle_connection(stream, shared);
        });
    }
}

/// Serve the dashboard on `bind:port` and keep it fed, until the process dies.
pub fn run(app: App, bind: &str, port: u16) -> io::Result<()> {
    let addr = format!("{bind}:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot listen on {addr}: {e} (pick another --port)"),
        )
    })?;
    println!("skimonitor --web — http://{addr}/ — {}", app.describe_targets());
    let shared = Shared::new(app);
    let serving = Arc::clone(&shared);
    thread::spawn(move || accept_loop(listener, serving));

    // ---- render / supervision loop: the TUI's loop, minus the terminal ----
    let (cols, rows) = *shared.size.lock().unwrap();
    let mut renderer = view::Renderer::new(cols, rows);
    let (mut last_cols, mut last_rows) = (0u16, 0u16);
    let mut last_render = Instant::now() - Duration::from_secs(1);
    let mut last_tick = Instant::now();
    loop {
        {
            let mut app = shared.app.lock().unwrap();
            app.drain_samples();
            if last_tick.elapsed() >= Duration::from_millis(250) {
                last_tick = Instant::now();
                app.tick();
                app.drain_samples();
            }
        }
        let wanted = shared.client_count() > 0 || shared.dirty.swap(false, Ordering::SeqCst);
        if wanted && last_render.elapsed() >= Duration::from_millis(120) {
            let (cols, rows) = *shared.size.lock().unwrap();
            let mut app = shared.app.lock().unwrap();
            app.seq += 1;
            let scrolls = app.scrolls.clone();
            let out = renderer.draw(&app, cols, rows);
            // keep stored offsets honest after resize/data shrink, exactly as
            // the TUI loop does after each draw
            let mut clamped = scrolls;
            for (i, mx) in out.maxes.iter().enumerate() {
                if let Some(cur) = clamped.get_mut(i) {
                    *cur = (*cur).min(*mx);
                }
            }
            app.scrolls = clamped;
            app.card_max = out.maxes;
            app.card_geoms = out.geoms;
            if (out.cols, out.rows) != (last_cols, last_rows) {
                println!("grid {}x{}", out.cols, out.rows);
                last_cols = out.cols;
                last_rows = out.rows;
            }
            drop(app);
            shared.broadcast(out.json);
            last_render = Instant::now();
        }
        thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_decode() {
        let v: serde_json::Value = serde_json::json!({"k":"c","v":"a"});
        assert_eq!(key_from_json(&v), Some(KeyCode::Char('a')));
        let v: serde_json::Value = serde_json::json!({"k":"pagedown"});
        assert_eq!(key_from_json(&v), Some(KeyCode::PageDown));
        // a multi-char payload is refused rather than silently truncated
        let v: serde_json::Value = serde_json::json!({"k":"c","v":"ab"});
        assert_eq!(key_from_json(&v), None);
    }

    #[test]
    fn q_cannot_stop_the_server_but_esc_closes_help() {
        let ts = vec!["a".to_string()];
        let mut app = App::new(&ts, 1, true);
        apply_web_key(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        app.help = true;
        apply_web_key(&mut app, KeyCode::Esc);
        assert!(!app.help);
        assert!(!app.quit);
        // with the add-host prompt open, Esc cancels the prompt instead
        apply_web_key(&mut app, KeyCode::Char('a'));
        assert!(app.input.is_some());
        apply_web_key(&mut app, KeyCode::Esc);
        assert!(app.input.is_none());
    }

    #[test]
    fn query_parsing() {
        assert_eq!(parse_query("cols=120&rows=40", "rows"), Some("40".into()));
        assert_eq!(parse_query("cols=120", "rows"), None);
    }

    #[test]
    fn assets_are_embedded_and_nonempty() {
        assert!(INDEX_HTML.contains("skimonitor"));
        assert!(APP_JS.contains("EventSource"));
        assert!(!FONT_REGULAR.is_empty() && !FONT_BOLD.is_empty());
    }
}
