//! Application state and key handling, independent of where the screen goes
//! and where the keys come from: the terminal front end (`tui.rs`) and the web
//! front end (`server.rs`) drive this same state machine.

use crate::model::{derive, Host, HostState, Probe};
use crate::ssh::{spawn_stream, Sample};
use crate::ui::{layout_rows, CardGeom, MAX_COLS, MAX_HOSTS};
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

/// One stream's supervision state
pub struct Stream {
    epoch: u64,
    pid: Option<u32>,
    /// last time a good frame arrived
    last_frame: Instant,
    /// Some(t) = respawn attempt at t
    restart_at: Option<Instant>,
    /// seconds to wait before next reconnect (doubles, capped)
    backoff: u64,
    /// attempt the connection as root@host (auth-denied fallback)
    use_root: bool,
}

impl Stream {
    fn new() -> Self {
        Self {
            epoch: 0,
            pid: None,
            last_frame: Instant::now(),
            restart_at: None,
            backoff: 3,
            use_root: false,
        }
    }
}

pub struct App {
    pub hosts: Vec<Host>,
    pub sel: usize,
    pub interval: u64,
    /// true = hosts are fed by the built-in demo replay, no ssh at all
    pub demo: bool,
    pub paused: bool,
    pub help: bool,
    pub input: Option<String>,
    pub note: Option<String>,
    pub rx: mpsc::Receiver<Sample>,
    pub tx: mpsc::Sender<Sample>,
    streams: HashMap<String, Stream>,
    /// (target, epoch) of streams we still accept samples from
    active: HashSet<(String, u64)>,
    /// pid receivers not yet drained into streams
    pidrx: HashMap<String, mpsc::Receiver<u32>>,
    pub quit: bool,
    /// per-card vertical scroll offset (lines)
    pub scrolls: Vec<usize>,
    /// stacked cards per column (sum == hosts.len()); a column holds 1 or 2
    pub rows: Vec<usize>,
    /// per-card max scroll from last draw (line overflow)
    pub card_max: Vec<usize>,
    /// per-card rect from last draw (wheel hit-test)
    pub card_geoms: Vec<CardGeom>,
    /// screens rendered so far (the web front end stamps frames with it)
    pub seq: u64,
}

fn ssh_dest(target: &str, use_root: bool) -> String {
    if crate::ssh::is_local(target) || target.contains('@') {
        return target.to_string();
    }
    if use_root {
        format!("root@{target}")
    } else {
        target.to_string()
    }
}

fn kill_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

impl App {
    pub fn new(targets: &[String], interval: u64, demo: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            hosts: Vec::new(),
            sel: 0,
            interval,
            demo,
            paused: false,
            help: false,
            input: None,
            note: None,
            rx,
            tx,
            streams: HashMap::new(),
            active: HashSet::new(),
            pidrx: HashMap::new(),
            quit: false,
            scrolls: Vec::new(),
            rows: Vec::new(),
            card_max: Vec::new(),
            card_geoms: Vec::new(),
            seq: 0,
        };
        for t in targets {
            app.add_target(t);
        }
        if demo {
            // fake hosts feed the same channel real streams would use; mark
            // their (target, epoch 0) active so samples apply without spawning ssh
            for h in app.hosts.iter_mut() {
                h.as_demo = true;
            }
            let ts: Vec<String> = app.hosts.iter().map(|h| h.target.clone()).collect();
            for t in ts {
                if let Some(st) = app.streams.get_mut(&t) {
                    st.last_frame = Instant::now();
                    app.active.insert((t.clone(), st.epoch));
                }
            }
        }
        app.sel = 0;
        app
    }

    pub fn add_target(&mut self, target: &str) {
        let target = target.trim().to_string();
        if target.is_empty() || self.hosts.iter().any(|h| h.target == target) {
            return;
        }
        if self.hosts.len() >= MAX_HOSTS {
            self.note = Some(format!("max {MAX_HOSTS} hosts — drop one with [x] first"));
            return;
        }
        self.note = None;
        self.rows = layout_rows(self.hosts.len(), &self.rows);
        // Where the new card lands: the first MAX_COLS hosts each open a new
        // column; from the 4th on, the highlighted card's column splits in
        // two and the new host takes the lower half (if that column is
        // already split, the next single-card column splits instead).
        let insert_at = if self.hosts.len() < MAX_COLS {
            self.rows.push(1);
            self.hosts.len()
        } else {
            let sel_col = self.col_of(self.sel);
            let col = if self.rows.get(sel_col).copied() == Some(1) {
                sel_col
            } else {
                self.rows
                    .iter()
                    .position(|r| *r == 1)
                    .unwrap_or_else(|| self.rows.len().saturating_sub(1))
            };
            let end = self.rows[..=col]
                .iter()
                .sum::<usize>()
                .min(self.hosts.len());
            self.rows[col] += 1;
            end
        };
        self.scrolls.resize(self.hosts.len(), 0);
        self.hosts.insert(insert_at, Host::new(&target));
        self.scrolls.insert(insert_at, 0);
        self.streams.insert(target.clone(), Stream::new());
        if !self.demo {
            self.spawn(target);
        }
        self.sel = insert_at;
    }

    /// index of the column that holds card `idx`
    pub fn col_of(&self, idx: usize) -> usize {
        let plan = layout_rows(self.hosts.len(), &self.rows);
        let mut acc = 0usize;
        for (c, r) in plan.iter().enumerate() {
            acc += r;
            if idx < acc {
                return c;
            }
        }
        plan.len().saturating_sub(1)
    }

    /// (re)spawn the stream for a target
    fn spawn(&mut self, target: String) {
        let st = match self.streams.get_mut(&target) {
            Some(s) => s,
            None => return,
        };
        st.epoch += 1;
        st.restart_at = None;
        let dest = ssh_dest(&target, st.use_root);
        self.active.insert((target.clone(), st.epoch));
        let (h, prx) = spawn_stream(
            target.clone(),
            dest,
            self.interval,
            self.tx.clone(),
            st.epoch,
        );
        drop(h); // detached; feeds the channel until death
        self.pidrx.insert(target.clone(), prx);
        st.last_frame = Instant::now();
        if let Some(h) = self.hosts.iter_mut().find(|h| h.target == target) {
            h.as_root = st.use_root && !crate::ssh::is_local(&target);
        }
    }

    /// kill the current stream and mark for restart (backoff reset)
    fn restart(&mut self, target: &str, immediately: bool) {
        if let Some(st) = self.streams.get_mut(target) {
            self.active.remove(&(target.to_string(), st.epoch));
            if let Some(pid) = st.pid.take() {
                kill_pid(pid);
            }
            st.backoff = 3;
            st.restart_at = Some(if immediately {
                Instant::now()
            } else {
                Instant::now() + Duration::from_millis(300)
            });
        }
    }

    pub fn restart_all(&mut self) {
        let ts: Vec<String> = self.hosts.iter().map(|h| h.target.clone()).collect();
        for t in ts {
            self.restart(&t, true);
        }
    }

    pub fn remove_selected(&mut self) {
        if self.hosts.is_empty() {
            return;
        }
        let t = self.hosts[self.sel].target.clone();
        if let Some(st) = self.streams.remove(&t) {
            self.active.remove(&(t.clone(), st.epoch));
            if let Some(pid) = st.pid {
                kill_pid(pid);
            }
        }
        self.pidrx.remove(&t);
        let idx = self.sel;
        let col = self.col_of(idx);
        self.rows = layout_rows(self.hosts.len(), &self.rows);
        self.hosts.remove(idx);
        self.scrolls.remove(idx);
        // give the slot back: a split column loses a row, an unsplit column
        // disappears and the columns to its right slide left
        match self.rows.get_mut(col) {
            Some(r) if *r > 1 => *r -= 1,
            _ => {
                if col < self.rows.len() {
                    self.rows.remove(col);
                }
            }
        }
        self.rows = layout_rows(self.hosts.len(), &self.rows);
        self.scrolls.truncate(self.hosts.len());
        self.card_max.truncate(self.hosts.len());
        if self.sel >= self.hosts.len() {
            self.sel = self.hosts.len().saturating_sub(1);
        }
    }

    /// scroll card `i` by `delta` lines (clamped; max from last draw)
    pub fn scroll_card(&mut self, i: usize, delta: isize) {
        if i >= self.hosts.len() {
            return;
        }
        let max = self.card_max.get(i).copied().unwrap_or(0);
        if self.scrolls.len() <= i {
            self.scrolls.resize(i + 1, 0);
        }
        let cur = self.scrolls[i] as isize;
        self.scrolls[i] = (cur + delta).clamp(0, max as isize) as usize;
    }

    /// jump card `i` to top (false) / bottom (true)
    pub fn scroll_edge(&mut self, i: usize, bottom: bool) {
        if i >= self.hosts.len() {
            return;
        }
        let max = self.card_max.get(i).copied().unwrap_or(0);
        if self.scrolls.len() <= i {
            self.scrolls.resize(i + 1, 0);
        }
        self.scrolls[i] = if bottom { max } else { 0 };
    }

    /// which card contains viewport (x, y)?
    pub fn card_at(&self, x: u16, y: u16) -> Option<usize> {
        self.card_geoms
            .iter()
            .position(|g| x >= g.x0 && x <= g.x1 && y >= g.y0 && y <= g.y1)
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        let targets: Vec<String> = self.hosts.iter().map(|h| h.target.clone()).collect();
        for t in targets {
            if let Some(st) = self.streams.get_mut(&t) {
                if paused {
                    self.active.remove(&(t.clone(), st.epoch));
                    if let Some(pid) = st.pid.take() {
                        kill_pid(pid);
                    }
                    st.restart_at = None;
                } else {
                    st.backoff = 3;
                    st.restart_at = Some(Instant::now());
                }
            }
        }
    }

    pub fn set_interval(&mut self, secs: u64) {
        let ni = secs.clamp(1, 60);
        if ni != self.interval {
            self.interval = ni;
            self.restart_all();
        }
    }

    pub fn drain_samples(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(sample) => self.apply_sample(sample),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        // collect worker pids
        let mut ready = Vec::new();
        for (t, prx) in self.pidrx.iter_mut() {
            if let Ok(pid) = prx.try_recv() {
                ready.push((t.clone(), pid));
            }
        }
        for (t, pid) in ready {
            match self.streams.get_mut(&t) {
                Some(st) => st.pid = Some(pid),
                None => kill_pid(pid), // stream gone already
            }
            self.pidrx.remove(&t);
        }
    }

    fn apply_sample(&mut self, s: Sample) {
        let idx = match self.hosts.iter().position(|h| h.target == s.target) {
            Some(i) => i,
            None => return,
        };
        let key = (s.target.clone(), s.epoch);
        if !self.active.contains(&key) {
            return; // stale stream incarnation
        }
        let h = &mut self.hosts[idx];
        match s.result {
            Ok(probe) => {
                if let Some(st) = self.streams.get_mut(&s.target) {
                    st.last_frame = Instant::now();
                    st.backoff = 3;
                }
                apply_probe(h, probe);
                h.state = HostState::Ok;
                h.last_error = None;
                h.consecutive_errors = 0;
                h.probing = false;
                h.last_ok = Some(SystemTime::now());
            }
            Err(err) => {
                h.state = HostState::Error;
                h.last_error = Some(err.clone());
                h.consecutive_errors += 1;
                // stream died — schedule reconnect unless host was removed
                self.active.remove(&key);
                if let Some(st) = self.streams.get_mut(&s.target) {
                    st.pid = None;
                    let auth_denied = err.to_lowercase().contains("permission denied")
                        || err.to_lowercase().contains("auth failed");
                    let user_pinned = s.target.contains('@') || crate::ssh::is_local(&s.target);
                    if auth_denied && !user_pinned && !st.use_root {
                        // current user refused — retry immediately as root
                        st.use_root = true;
                        st.backoff = 3;
                        st.restart_at = Some(Instant::now());
                    } else {
                        st.restart_at = Some(Instant::now() + Duration::from_secs(st.backoff));
                        st.backoff = (st.backoff * 2).min(30);
                    }
                }
            }
        }
    }

    /// supervision: restart dead streams + watchdog stale ones
    pub fn tick(&mut self) {
        if self.paused || self.demo {
            return;
        }
        let now = Instant::now();
        let stale_after = Duration::from_secs(self.interval * 4 + 6);
        let mut to_restart: Vec<String> = Vec::new();
        let mut to_watchdog: Vec<String> = Vec::new();
        for (t, st) in self.streams.iter() {
            if !self.hosts.iter().any(|h| h.target == *t) {
                continue;
            }
            if self.active.contains(&(t.clone(), st.epoch)) {
                if st.restart_at.is_none() && now.duration_since(st.last_frame) > stale_after {
                    to_watchdog.push(t.clone());
                }
            } else if let Some(at) = st.restart_at {
                if now >= at {
                    to_restart.push(t.clone());
                }
            }
        }
        for t in to_watchdog {
            // kill the wedged ssh, then respawn
            if let Some(st) = self.streams.get_mut(&t) {
                self.active.remove(&(t.clone(), st.epoch));
                if let Some(pid) = st.pid.take() {
                    kill_pid(pid);
                }
                st.restart_at = Some(Instant::now());
                st.backoff = 3;
            }
        }
        for t in to_restart {
            self.spawn(t);
        }
    }

    pub fn pending(&self) -> usize {
        self.hosts
            .iter()
            .filter(|h| h.metrics.is_none() && h.state != HostState::Error)
            .count()
    }

    /// one-line summary for the web front end's startup log
    pub fn describe_targets(&self) -> String {
        let ts: Vec<&str> = self.hosts.iter().map(|h| h.target.as_str()).collect();
        format!("{} host(s): {}", ts.len(), ts.join(" "))
    }

    /// kill every live stream (used when the front end exits)
    pub fn kill_streams(&mut self) {
        let targets: Vec<String> = self.hosts.iter().map(|h| h.target.clone()).collect();
        for t in targets {
            if let Some(st) = self.streams.get_mut(&t) {
                if let Some(pid) = st.pid.take() {
                    kill_pid(pid);
                }
            }
        }
    }
}

fn apply_probe(h: &mut Host, probe: Probe) {
    // engine payloads arrive on the probe's ~6s refresh frames only; parse and
    // cache them here, then attach by pid onto every derived frame
    crate::engine::update(&mut h.engines, &probe);
    if let Some(prev) = &h.probe {
        let dt = h.last_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        if dt > 0.3 {
            let mut m = derive(&probe, prev, dt);
            crate::engine::attach(&mut m, &h.engines);
            let rx = m.nets.iter().map(|n| n.rx_bps / 8.0).fold(0.0f64, f64::max);
            let mem_pct = if m.mem_total > 0 {
                m.mem_used as f64 / m.mem_total as f64 * 100.0
            } else {
                0.0
            };
            h.push_hist(m.cpu_pct, rx, mem_pct);
            // per-card histories so every GPU bar (vram / compute / power)
            // gets its own trend on the same scale
            for g in &m.gpus {
                let gp = if g.mem_total_mb > 0 {
                    g.mem_used_mb as f64 / g.mem_total_mb as f64 * 100.0
                } else {
                    0.0
                };
                h.push_hist_gpu_vram(g.idx, gp);
                h.push_hist_gpu_util(g.idx, g.util.clamp(0.0, 100.0));
                // power as % of the card's cap; unscaled watts would make the
                // trend non-comparable between cards
                let pp = match (g.power_w, g.power_cap_w) {
                    (Some(p), Some(c)) if c > 0.0 => (p / c * 100.0).clamp(0.0, 100.0),
                    _ => 0.0,
                };
                h.push_hist_gpu_pow(g.idx, pp);
            }
            h.metrics = Some(m);
        }
    }
    h.last_at = Some(Instant::now());
    h.probe = Some(probe);
}

pub fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
        app.quit = true;
        return;
    }
    if app.input.is_some() {
        let buf = app.input.as_mut().unwrap();
        match code {
            KeyCode::Esc => app.input = None,
            KeyCode::Enter => {
                let raw = buf.clone();
                app.input = None;
                for t in raw.split([',', ' ', ';', '\t']) {
                    app.add_target(t);
                }
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
        return;
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Char('?') => app.help = !app.help,
        KeyCode::Up => {
            app.scroll_card(app.sel, -1);
        }
        KeyCode::Down => {
            app.scroll_card(app.sel, 1);
        }
        KeyCode::PageUp => {
            app.scroll_card(app.sel, -5);
        }
        KeyCode::PageDown => {
            app.scroll_card(app.sel, 5);
        }
        KeyCode::Home => {
            app.scroll_edge(app.sel, false);
        }
        KeyCode::End => {
            app.scroll_edge(app.sel, true);
        }
        KeyCode::Tab | KeyCode::Right => {
            if !app.hosts.is_empty() {
                app.sel = (app.sel + 1) % app.hosts.len();
            }
        }
        KeyCode::BackTab | KeyCode::Left => {
            if !app.hosts.is_empty() {
                app.sel = (app.sel + app.hosts.len() - 1) % app.hosts.len();
            }
        }
        KeyCode::Char('a') => {
            if app.hosts.len() >= MAX_HOSTS {
                app.note = Some(format!("max {MAX_HOSTS} hosts — drop one with [x] first"));
            } else {
                app.note = None;
                app.input = Some(String::new());
            }
        }
        KeyCode::Char(c @ '1'..='6') => {
            let i = c as usize - '1' as usize;
            if i < app.hosts.len() {
                app.sel = i;
            }
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            app.remove_selected();
            app.note = None;
        }
        KeyCode::Char('r') => {
            app.restart_all();
        }
        KeyCode::Char('p') => {
            let np = !app.paused;
            app.set_paused(np);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.set_interval(app.interval + 1);
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            app.set_interval(app.interval.saturating_sub(1));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MAX_ROWS;

    /// demo mode: real App bookkeeping, no ssh spawned
    fn app_with(names: &[&str]) -> App {
        let ts: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        App::new(&ts, 1, true)
    }

    fn order(app: &App) -> Vec<String> {
        app.hosts.iter().map(|h| h.target.clone()).collect()
    }

    #[test]
    fn first_three_hosts_get_a_column_each() {
        let app = app_with(&["a", "b", "c"]);
        assert_eq!(app.rows, vec![1, 1, 1]);
        assert_eq!(order(&app), ["a", "b", "c"]);
    }

    #[test]
    fn fourth_host_splits_the_highlighted_column_below_it() {
        let mut app = app_with(&["a", "b", "c"]);
        app.sel = 1; // highlight b's column
        app.add_target("d");
        assert_eq!(app.rows, vec![1, 2, 1]);
        assert_eq!(order(&app), ["a", "b", "d", "c"]);
        assert_eq!(app.sel, 2); // focus lands on the new lower card
        assert_eq!(app.col_of(1), 1);
        assert_eq!(app.col_of(2), 1);
        assert_eq!(app.col_of(3), 2);
    }

    #[test]
    fn adding_past_three_splits_the_next_single_column() {
        let mut app = app_with(&["a", "b", "c"]);
        app.sel = 0;
        app.add_target("d"); // a's column splits
        assert_eq!(app.rows, vec![2, 1, 1]);
        app.sel = 3; // c: its column is still single
        app.add_target("e");
        assert_eq!(app.rows, vec![2, 1, 2]);
        assert_eq!(order(&app), ["a", "d", "b", "c", "e"]);
        app.sel = 0; // a's column is already split
        app.add_target("f");
        assert_eq!(app.rows, vec![2, 2, 2]);
        assert_eq!(order(&app), ["a", "d", "b", "f", "c", "e"]);
        assert_eq!(app.sel, 3);
    }

    #[test]
    fn seventh_host_is_refused() {
        let mut app = app_with(&["a", "b", "c", "d", "e", "f"]);
        app.add_target("g");
        assert_eq!(app.hosts.len(), 6);
        assert!(app.note.is_some());
    }

    #[test]
    fn removing_gives_the_slot_back() {
        let mut app = app_with(&["a", "b", "c"]);
        app.sel = 1;
        app.add_target("d");
        assert_eq!(app.rows, vec![1, 2, 1]);
        app.sel = 2; // d, lower half of b's column
        app.remove_selected();
        assert_eq!(order(&app), ["a", "b", "c"]);
        assert_eq!(app.rows, vec![1, 1, 1]);
        // dropping a whole column closes it and the rest slide left
        let mut app = app_with(&["a", "b", "c"]);
        app.sel = 0;
        app.add_target("d");
        app.sel = 0;
        app.remove_selected(); // a
        app.sel = 0;
        app.remove_selected(); // d -> column 0 is gone
        assert_eq!(order(&app), ["b", "c"]);
        assert_eq!(app.rows, vec![1, 1]);
    }

    #[test]
    fn every_host_has_a_slot_after_any_add_remove_sequence() {
        let mut app = app_with(&["a", "b", "c", "d", "e", "f"]);
        assert_eq!(app.rows.iter().sum::<usize>(), app.hosts.len());
        app.sel = 4;
        app.remove_selected();
        assert_eq!(app.rows.iter().sum::<usize>(), app.hosts.len());
        app.add_target("g");
        assert_eq!(app.rows.iter().sum::<usize>(), app.hosts.len());
        assert!(app.rows.iter().all(|r| (1..=MAX_ROWS).contains(r)));
    }

    #[test]
    fn add_flow_types_and_commits() {
        let mut app = app_with(&["a"]);
        handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(app.input.as_deref(), Some(""));
        handle_key(&mut app, KeyCode::Char('b'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.input, None);
        assert_eq!(order(&app), ["a", "b"]);
        // Esc discards the buffer
        handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(order(&app), ["a", "b"]);
    }

    #[test]
    fn q_and_ctrl_c_set_quit_for_the_terminal_front_end() {
        let mut app = app_with(&["a"]);
        handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.quit);
        app.quit = false;
        handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit);
    }
}
