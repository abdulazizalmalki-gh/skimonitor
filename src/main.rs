mod demo;
mod model;
mod ssh;
mod ui;
mod widgets;

use crate::model::{derive, Host, HostState, Probe};
use crate::ssh::{run_probe_blocking, spawn_stream, Sample};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::{HashMap, HashSet};
use std::io::{self, stdout};
use std::process::Command;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

struct Args {
    targets: Vec<String>,
    interval: u64,
    once: bool,
    demo: bool,
}

fn parse_args() -> Args {
    fn die(msg: &str) -> ! {
        eprintln!("skimonitor: {msg}\n(try --help)");
        std::process::exit(2);
    }
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut interval = 1u64;
    let mut once = false;
    let mut demo = false;
    let mut targets: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "-i" | "--interval" => {
                // a missing or non-numeric value is an error, not a target
                match argv.get(i + 1).and_then(|v| v.parse::<u64>().ok()) {
                    Some(v) => {
                        interval = v.clamp(1, 60);
                        i += 1;
                    }
                    None => match argv.get(i + 1) {
                        None => die("-i needs a value in seconds (1..60)"),
                        Some(bad) => die(&format!(
                            "-i needs a number in seconds (1..60), got '{bad}'"
                        )),
                    },
                }
            }
            "--once" | "-o" => once = true,
            "--demo" => demo = true,
            "-h" | "--help" => {
                println!(
                    "skimonitor — multi-host SSH monitor TUI (streaming)\n\n\
                     usage: skimonitor [options] [host ...]\n\
                     \x20 host        user@hostname, ~/.ssh/config alias, or IP; 'local' self-scans\n\
                     \x20 -i SECONDS  stream frame interval (default 1)\n\
                     \x20 --once      probe each host once, print summary, exit (no TUI)\n\
                     \x20 --demo      replay invented hosts through the real UI (no ssh, no data)\n\n\
                     One persistent ssh connection per host streams one JSON frame per\n\
                     second; dead streams reconnect automatically with backoff.\n\
                     Auth uses the system ssh client: local ssh-agent, ~/.ssh/config\n\
                     identities and default keys. BatchMode: never prompts for passwords.\n\
                     Targets need bash + coreutils. Keys: a add · x remove · Tab/←→ switch ·\n\
                     r restart streams · p pause · +/- interval · ↑↓/PgUp/PgDn/wheel scroll\n\
                     a card that overflows (↕ in its title) · ? help · q quit"
                );
                std::process::exit(0);
            }
            _ => {
                // never treat an option-shaped argument as a target: it would
                // reach ssh's argv and could smuggle flags
                if a.starts_with('-') {
                    die(&format!("unknown option '{a}'"));
                }
                for t in a.split([',', ';', ' ']) {
                    let t = t.trim();
                    if !t.is_empty() {
                        targets.push(t.to_string());
                    }
                }
            }
        }
        i += 1;
    }
    if targets.is_empty() {
        targets.push("local".into());
    }
    Args {
        targets,
        interval,
        once,
        demo,
    }
}

/// One stream's supervision state
struct Stream {
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

struct App {
    hosts: Vec<Host>,
    sel: usize,
    interval: u64,
    /// true = hosts are fed by the built-in demo replay, no ssh at all
    demo: bool,
    paused: bool,
    help: bool,
    input: Option<String>,
    note: Option<String>,
    rx: mpsc::Receiver<Sample>,
    tx: mpsc::Sender<Sample>,
    streams: HashMap<String, Stream>,
    /// (target, epoch) of streams we still accept samples from
    active: HashSet<(String, u64)>,
    /// pid receivers not yet drained into streams
    pidrx: HashMap<String, mpsc::Receiver<u32>>,
    quit: bool,
    /// per-card vertical scroll offset (lines)
    scrolls: Vec<usize>,
    /// per-card max scroll from last draw (line overflow)
    card_max: Vec<usize>,
    /// per-card rect from last draw (mouse wheel hit-test)
    card_geoms: Vec<ui::CardGeom>,
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
    fn new(targets: &[String], interval: u64, demo: bool) -> Self {
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
            card_max: Vec::new(),
            card_geoms: Vec::new(),
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

    fn add_target(&mut self, target: &str) {
        let target = target.trim().to_string();
        if target.is_empty() || self.hosts.iter().any(|h| h.target == target) {
            return;
        }
        if self.hosts.len() >= crate::ui::MAX_HOSTS {
            self.note = Some(format!(
                "max {} hosts — drop one with [x] first",
                crate::ui::MAX_HOSTS
            ));
            return;
        }
        self.note = None;
        self.hosts.push(Host::new(&target));
        self.scrolls.push(0);
        self.streams.insert(target.clone(), Stream::new());
        if !self.demo {
            self.spawn(target);
        }
        self.sel = self.hosts.len() - 1;
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

    fn remove_selected(&mut self) {
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
        self.hosts.remove(idx);
        if idx < self.scrolls.len() {
            self.scrolls.remove(idx);
        }
        self.scrolls.truncate(self.hosts.len());
        if self.sel >= self.hosts.len() {
            self.sel = self.hosts.len().saturating_sub(1);
        }
    }

    /// scroll card `i` by `delta` lines (clamped; max from last draw)
    fn scroll_card(&mut self, i: usize, delta: isize) {
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
    fn scroll_edge(&mut self, i: usize, bottom: bool) {
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
    fn card_at(&self, x: u16, y: u16) -> Option<usize> {
        self.card_geoms.iter().position(|g| {
            x >= g.x0 && x <= g.x1 && y >= g.y0 && y <= g.y1
        })
    }

    fn set_paused(&mut self, paused: bool) {
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

    fn drain_samples(&mut self) {
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
    fn tick(&mut self) {
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

    fn pending(&self) -> usize {
        self.hosts
            .iter()
            .filter(|h| h.metrics.is_none() && h.state != HostState::Error)
            .count()
    }
}

fn apply_probe(h: &mut Host, probe: Probe) {
    if let Some(prev) = &h.probe {
        let dt = h.last_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        if dt > 0.3 {
            let m = derive(&probe, prev, dt);
            let rx = m.nets.iter().map(|n| n.rx_bps / 8.0).fold(0.0f64, f64::max);
            let mem_pct = if m.mem_total > 0 {
                m.mem_used as f64 / m.mem_total as f64 * 100.0
            } else {
                0.0
            };
            h.push_hist(m.cpu_pct, rx, mem_pct);
            h.metrics = Some(m);
        }
    }
    h.last_at = Some(Instant::now());
    h.probe = Some(probe);
}

fn main() -> io::Result<()> {
    let mut args = parse_args();
    if args.demo {
        // demo mode ignores given targets: the three invented hosts ARE the demo
        args.targets = demo::DEMO_HOSTS.iter().map(|h| h.target.to_string()).collect();
    }
    if args.once && args.demo {
        eprintln!("--demo is a TUI replay; drop --once");
        std::process::exit(2);
    }
    if args.once {
        let mut failed = 0usize;
        for t in &args.targets {
            let user_pinned = t.contains('@') || crate::ssh::is_local(t);
            let mut attempt = run_probe_blocking(t, t, Duration::from_secs(25));
            if let Err(e) = &attempt {
                let denied = e.to_lowercase().contains("permission denied")
                    || e.to_lowercase().contains("auth failed");
                if denied && !user_pinned {
                    eprintln!("{t}: auth denied for current user — retrying as root@…");
                    let dest = format!("root@{t}");
                    attempt = run_probe_blocking(t, &dest, Duration::from_secs(25));
                }
            }
            match attempt {
                Ok(p) => println!(
                    "{t} OK  host={} arch={} cores={} disks={} nets={} gpus={}",
                    p.hostname,
                    p.arch,
                    p.cpu.ncpu,
                    p.disks.len(),
                    p.nets.len(),
                    p.gpus.len()
                ),
                Err(e) => {
                    failed += 1;
                    eprintln!("{t} FAIL  {e}");
                }
            }
        }
        std::process::exit(if failed > 0 { 1 } else { 0 });
    }

    enable_raw_mode()?;
    let mut backend = CrosstermBackend::new(stdout());
    execute!(backend, EnterAlternateScreen)?;
    let mut term = Terminal::new(backend)?;
    let r = run(&mut term, args);
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
    r
}

fn run(term: &mut Terminal<CrosstermBackend<io::Stdout>>, args: Args) -> io::Result<()> {
    let mut app = App::new(&args.targets, args.interval, args.demo);
    if args.demo {
        demo::run(demo::DEMO_HOSTS, args.interval.max(1), app.tx.clone());
    }
    let mut last_draw = Instant::now();
    let mut last_tick = Instant::now();
    // mouse wheel scrolls the card under the cursor (if the terminal reports
    // mice at all; Shift-select still works in the terminal itself)
    let _ = execute!(term.backend_mut(), crossterm::event::EnableMouseCapture);

    loop {
        if app.quit {
            let _ = execute!(term.backend_mut(), crossterm::event::DisableMouseCapture);
            // kill all live streams so no orphan ssh clients linger
            let targets: Vec<String> = app.hosts.iter().map(|h| h.target.clone()).collect();
            for t in targets {
                if let Some(st) = app.streams.get_mut(&t) {
                    if let Some(pid) = st.pid.take() {
                        kill_pid(pid);
                    }
                }
            }
            return Ok(());
        }
        while event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                }) if kind == KeyEventKind::Press => {
                    handle_key(&mut app, code, modifiers);
                    last_draw = Instant::now();
                }
                Event::Mouse(me) => match me.kind {
                    MouseEventKind::ScrollDown => {
                        let i = app.card_at(me.column, me.row).unwrap_or(app.sel);
                        app.scroll_card(i, 3);
                        last_draw = Instant::now();
                    }
                    MouseEventKind::ScrollUp => {
                        let i = app.card_at(me.column, me.row).unwrap_or(app.sel);
                        app.scroll_card(i, -3);
                        last_draw = Instant::now();
                    }
                    _ => {}
                },
                Event::Resize(_, _) => last_draw = Instant::now(),
                _ => {}
            }
        }

        app.drain_samples();
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();
            app.drain_samples();
            app.tick();
        }

        if last_draw.elapsed() >= Duration::from_millis(150) {
            let scrolls = app.scrolls.clone();
            let mut result = (Vec::new(), Vec::new());
            term.draw(|f| {
                result = ui::draw(
                    f,
                    &app.hosts,
                    app.sel,
                    app.interval,
                    &app.input,
                    app.help,
                    app.pending(),
                    app.paused,
                    &app.note,
                    &scrolls,
                );
            })?;
            let (maxes, geoms) = result;
            // keep offsets honest after resize/data shrink
            app.scrolls = scrolls;
            for (i, mx) in maxes.iter().enumerate() {
                if let Some(cur) = app.scrolls.get_mut(i) {
                    *cur = (*cur).min(*mx);
                }
            }
            app.card_max = maxes;
            app.card_geoms = geoms;
            last_draw = Instant::now();
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
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
            if app.hosts.len() >= crate::ui::MAX_HOSTS {
                app.note = Some(format!(
                    "max {} hosts — drop one with [x] first",
                    crate::ui::MAX_HOSTS
                ));
            } else {
                app.note = None;
                app.input = Some(String::new());
            }
        }
        KeyCode::Char('1') | KeyCode::Char('2') | KeyCode::Char('3') => {
            let i =
                (code == KeyCode::Char('2')) as usize + (code == KeyCode::Char('3')) as usize * 2;
            if i < app.hosts.len() {
                app.sel = i;
            }
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            app.remove_selected();
            app.note = None;
        }
        KeyCode::Char('r') => {
            let ts: Vec<String> = app.hosts.iter().map(|h| h.target.clone()).collect();
            for t in ts {
                app.restart(&t, true);
            }
        }
        KeyCode::Char('p') => {
            let np = !app.paused;
            app.set_paused(np);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let ni = (app.interval + 1).clamp(1, 60);
            if ni != app.interval {
                app.interval = ni;
                let ts: Vec<String> = app.hosts.iter().map(|h| h.target.clone()).collect();
                for t in ts {
                    app.restart(&t, true);
                }
            }
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            let ni = app.interval.saturating_sub(1).max(1);
            if ni != app.interval {
                app.interval = ni;
                let ts: Vec<String> = app.hosts.iter().map(|h| h.target.clone()).collect();
                for t in ts {
                    app.restart(&t, true);
                }
            }
        }
        _ => {}
    }
}
