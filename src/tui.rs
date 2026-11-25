//! Terminal front end: crossterm events in, ratatui to stdout out.
//! All state lives in `app::App`; this file only drives it.

use crate::app::{handle_key, App};
use crate::ui;
use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, stdout};
use std::time::{Duration, Instant};

pub fn run(targets: &[String], interval: u64, demo: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut backend = CrosstermBackend::new(stdout());
    execute!(backend, EnterAlternateScreen)?;
    let mut term = Terminal::new(backend)?;
    let r = event_loop(&mut term, targets, interval, demo);
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
    r
}

fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    targets: &[String],
    interval: u64,
    demo: bool,
) -> io::Result<()> {
    let mut app = App::new(targets, interval, demo);
    if demo {
        crate::demo::run(crate::demo::DEMO_HOSTS, interval.max(1), app.tx.clone());
    }
    let mut last_draw = Instant::now();
    let mut last_tick = Instant::now();
    // mouse wheel scrolls the card under the cursor (if the terminal reports
    // mice at all; Shift-select still works in the terminal itself)
    let _ = execute!(term.backend_mut(), event::EnableMouseCapture);

    loop {
        if app.quit {
            let _ = execute!(term.backend_mut(), event::DisableMouseCapture);
            // kill all live streams so no orphan ssh clients linger
            app.kill_streams();
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
                    &app.rows,
                    app.sel,
                    app.interval,
                    &app.input,
                    app.help,
                    app.pending(),
                    app.paused,
                    &app.note,
                    &scrolls,
                    false, // terminal front end: quit keys are live
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
