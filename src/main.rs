//! skimonitor — multi-host SSH monitor.
//!
//! Two front ends, one dashboard: the terminal UI (`tui`) and the browser
//! dashboard (`--web`, `server` + `view`). Both drive the same `app::App` and
//! draw with the same `ui::draw`, so they cannot drift apart.

mod app;
mod demo;
mod engine;
mod model;
mod server;
mod ssh;
mod tui;
mod ui;
mod view;
mod widgets;

use app::App;
use std::io;
use std::time::Duration;

struct Args {
    targets: Vec<String>,
    interval: u64,
    once: bool,
    demo: bool,
    web: bool,
    port: u16,
    bind: String,
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
    let mut web = false;
    let mut port = 8383u16;
    let mut bind = "0.0.0.0".to_string();
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
            "--web" => web = true,
            "-p" | "--port" => match argv.get(i + 1).and_then(|v| v.parse::<u16>().ok()) {
                Some(v) => {
                    port = v;
                    i += 1;
                }
                None => die("-p needs a TCP port number"),
            },
            "--bind" => match argv.get(i + 1) {
                Some(v) => {
                    bind = v.clone();
                    i += 1;
                }
                None => die("--bind needs an address (e.g. 0.0.0.0 or 127.0.0.1)"),
            },
            "--once" | "-o" => once = true,
            "--demo" => demo = true,
            "-h" | "--help" => {
                println!(
                    "skimonitor — multi-host SSH monitor (TUI, or the same screen in a browser)\n\n\
                     usage: skimonitor [options] [host ...]\n\
                     \x20 host        user@hostname, ~/.ssh/config alias, or IP; 'local' self-scans\n\
                     \x20 -i SECONDS  stream frame interval (default 1)\n\
                     \x20 --once      probe each host once, print summary, exit (no TUI)\n\
                     \x20 --demo      replay invented hosts through the real UI (no ssh, no data)\n\
                     \x20 --web       serve the dashboard to a browser instead of drawing a TUI\n\
                     \x20 -p PORT     port for --web (default 8383)\n\
                     \x20 --bind ADDR address for --web (default 0.0.0.0; try 127.0.0.1 behind a proxy)\n\n\
                     One persistent ssh connection per host streams one JSON frame per\n\
                     second; dead streams reconnect automatically with backoff.\n\
                     Auth uses the system ssh client: local ssh-agent, ~/.ssh/config\n\
                     identities and default keys. BatchMode: never prompts for passwords.\n\
                     Targets need bash + coreutils. Keys: a add · x remove · Tab/←→ switch ·\n\
                     r restart streams · p pause · +/- interval · ↑↓/PgUp/PgDn/wheel scroll\n\
                     a card that overflows (↕ in its title) · ? help · q quit.\n\n\
                     --web renders the exact same screen: the server paints the ratatui\n\
                     buffer and ships the cells, so the page and the terminal look identical.\n\
                     It has no login of its own — keep it on a network you trust.\n\n\
                     Up to 6 hosts: the first three side by side, then the highlighted\n\
                     card splits in two and the new host takes the lower half."
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
        web,
        port,
        bind,
    }
}

fn main() -> io::Result<()> {
    let mut args = parse_args();
    if args.demo {
        // demo mode ignores given targets: the three invented hosts ARE the demo
        args.targets = demo::DEMO_HOSTS
            .iter()
            .map(|h| h.target.to_string())
            .collect();
    }
    if args.once && args.demo {
        eprintln!("--demo is a live replay; drop --once");
        std::process::exit(2);
    }
    if args.once {
        return once(&args);
    }
    if args.web {
        let app = App::new(&args.targets, args.interval, args.demo);
        if args.demo {
            demo::run(demo::DEMO_HOSTS, args.interval.max(1), app.tx.clone());
        }
        return server::run(app, &args.bind, args.port);
    }
    tui::run(&args.targets, args.interval, args.demo)
}

fn once(args: &Args) -> io::Result<()> {
    let mut failed = 0usize;
    for t in &args.targets {
        let user_pinned = t.contains('@') || crate::ssh::is_local(t);
        let mut attempt = ssh::run_probe_blocking(t, t, Duration::from_secs(25));
        if let Err(e) = &attempt {
            let denied = e.to_lowercase().contains("permission denied")
                || e.to_lowercase().contains("auth failed");
            if denied && !user_pinned {
                eprintln!("{t}: auth denied for current user — retrying as root@…");
                let dest = format!("root@{t}");
                attempt = ssh::run_probe_blocking(t, &dest, Duration::from_secs(25));
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
