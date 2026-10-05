//! `baton` with no arguments: a view of the daemon's state that answers "what
//! needs me, what is progressing, what is ready to review" (PLAN §7). Quitting
//! never affects work; the daemon owns it.

mod app;
mod view;

use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::client;
use crate::paths::Paths;
use crate::protocol::{Request, Response};
use crate::store::now_ms;
use app::{App, Effect, Key};

const REFRESH: Duration = Duration::from_secs(1);

pub fn run(paths: &Paths) -> Result<()> {
    let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let mut app = App::new(color);
    // Fail before taking over the terminal if the daemon isn't there.
    refresh(paths, &mut app)?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, paths, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, paths: &Paths, app: &mut App) -> Result<()> {
    let mut refreshed = Instant::now();
    loop {
        let wide = terminal.size()?.width >= view::WIDE;
        terminal.draw(|f| view::render(f, app))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(k) = event::read()?
        {
            let Some(key) = (k.kind == KeyEventKind::Press).then(|| key(k.code, k.modifiers)).flatten() else { continue };
            match app.on_key(key, wide) {
                Effect::None => {}
                Effect::Quit => return Ok(()),
                Effect::Call(request) => {
                    app.message = Some(call(paths, &request));
                    refreshed = Instant::now() - REFRESH;
                }
                Effect::Attach(session) => {
                    app.message = Some(attach(terminal, &session)?);
                    refreshed = Instant::now() - REFRESH;
                }
            }
        }
        if refreshed.elapsed() >= REFRESH {
            if let Err(e) = refresh(paths, app) {
                app.message = Some(format!("{e:#}"));
            }
            refreshed = Instant::now();
        }
    }
}

fn key(code: KeyCode, mods: KeyModifiers) -> Option<Key> {
    Some(match code {
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Key::Interrupt,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    })
}

fn refresh(paths: &Paths, app: &mut App) -> Result<()> {
    let Response::Status { tasks, quota } = client::call(paths, &Request::Status)? else {
        bail!("unexpected reply from the daemon");
    };
    app.set_status(tasks, quota, now_ms());
    if let Some(id) = app.selected
        && let Response::Detail { detail } = client::call(paths, &Request::Detail { task: id })?
    {
        app.detail = Some((id, detail));
    }
    Ok(())
}

/// Carries out an action; returns what to tell the owner.
fn call(paths: &Paths, request: &Request) -> String {
    match client::call(paths, request) {
        Ok(Response::Decided { decision, delivery }) => {
            format!("#{}: {} ({delivery})", decision.id, decision.answer.unwrap_or_default())
        }
        Ok(Response::Reviewed { task, outcome }) => format!("task #{}: {outcome}", task.id),
        Ok(_) => "done".into(),
        Err(e) => format!("{e:#}"),
    }
}

/// Hands the terminal to `claude attach` and takes it back afterwards.
fn attach(terminal: &mut DefaultTerminal, session: &str) -> Result<String> {
    ratatui::restore();
    println!("Attaching to worker {session}. Detach (left arrow or /exit) to come back to Baton.");
    let status = Command::new("claude").args(["attach", session]).status();
    *terminal = ratatui::init();
    terminal.clear()?;
    Ok(match status {
        Ok(s) if s.success() => format!("back from worker {session}"),
        Ok(s) => format!("claude attach {session} exited with {s}"),
        Err(e) => format!("couldn't run claude attach: {e}"),
    })
}

#[cfg(test)]
mod sample;
