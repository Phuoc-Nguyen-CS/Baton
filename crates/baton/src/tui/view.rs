//! Rendering: a pure function of `App`. Text is ASCII and every colour has a
//! text equivalent, so it reads the same without colour (NO_COLOR) or Unicode.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::app::{Act, App, Attention, BINDINGS, Key, Prompt, Screen, attention};
use crate::model::TaskView;

/// From this width the list and the selected task show side by side.
pub const WIDE: u16 = 120;
const LIST_WIDTH: u16 = 48;

struct Theme {
    color: bool,
}

impl Theme {
    fn fg(&self, c: Color) -> Style {
        if self.color { Style::new().fg(c) } else { Style::new() }
    }
    fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

pub fn render(f: &mut Frame, app: &App) {
    let theme = Theme { color: app.color };
    let area = f.area();
    let wide = area.width >= WIDE;
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(2)]).areas(area);
    f.render_widget(Paragraph::new(header_line(app, &theme)), header);

    match app.screen {
        Screen::List | Screen::Task if wide => {
            let [left, bar, right] =
                Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Length(2), Constraint::Fill(1)]).areas(body);
            list(f, left, app, &theme);
            f.render_widget(Paragraph::new(vec![Line::raw("|"); bar.height as usize]), bar);
            detail(f, right, app, &theme);
        }
        Screen::List => list(f, body, app, &theme),
        Screen::Task => detail(f, body, app, &theme),
        Screen::Diff => diff(f, body, app, &theme),
        Screen::Help => help(f, body, app),
    }
    footer_lines(f, footer, app, wide, &theme);
}

fn header_line(app: &App, theme: &Theme) -> Line<'static> {
    let count = |a: Attention| app.tasks.iter().filter(|t| attention(t) == a).count();
    let mut spans = vec![
        Span::styled("Baton", theme.bold()),
        Span::raw(format!(
            "  needs you {} | in progress {} | to review {} | done {}",
            count(Attention::NeedsYou),
            count(Attention::Progressing),
            count(Attention::Review),
            count(Attention::Done)
        )),
    ];
    let quota = match &app.quota {
        Some(q) => {
            let pct = |p: Option<f64>| p.map_or("?".to_owned(), |p| format!("{p:.0}%"));
            let age = (app.now_ms - q.observed_ms).max(0) / 1000;
            format!(" | 5h {} 7d {} ({age}s)", pct(q.five_hour_pct), pct(q.seven_day_pct))
        }
        None => " | quota unknown".to_owned(),
    };
    spans.push(Span::styled(quota, theme.fg(Color::DarkGray)));
    Line::from(spans)
}

fn list(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.tasks.is_empty() {
        let text = vec![Line::raw("No tasks yet."), Line::raw(""), Line::raw("Start one with: baton task \"<goal>\" --check \"<command>\"")];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
        return;
    }
    let lines: Vec<Line> = app
        .tasks
        .iter()
        .map(|t| {
            let selected = Some(t.task.id) == app.selected;
            let (mark, color) = match attention(t) {
                Attention::NeedsYou => ("!", Color::Yellow),
                Attention::Review => ("+", Color::Green),
                Attention::Progressing => (" ", Color::Cyan),
                Attention::Done => (" ", Color::DarkGray),
            };
            let text = format!(
                "{}{mark} #{:<3} {:<18} {}",
                if selected { ">" } else { " " },
                t.task.id,
                t.task.state.as_str(),
                clean(&t.task.goal)
            );
            let style = if selected { theme.fg(color).add_modifier(Modifier::REVERSED) } else { theme.fg(color) };
            Line::styled(text, style)
        })
        .collect();
    // Keep the selected row on screen.
    let at = app.tasks.iter().position(|t| Some(t.task.id) == app.selected).unwrap_or(0) as u16;
    let scroll = at.saturating_sub(area.height.saturating_sub(1));
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

fn detail(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let lines = match app.selected_task() {
        Some(t) => detail_lines(app, t, theme),
        None => vec![Line::raw("Nothing selected.")],
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.scroll, 0)), area);
}

fn detail_lines(app: &App, t: &TaskView, theme: &Theme) -> Vec<Line<'static>> {
    let task = &t.task;
    let heading = |s: &str| Line::styled(s.to_owned(), theme.bold());
    let mut out = vec![
        Line::from(vec![Span::styled(format!("#{} {}", task.id, task.state.as_str()), theme.bold())]),
        Line::raw(clean(task.state_reason.as_deref().unwrap_or(""))),
        Line::raw(format!("Goal: {}", clean(&task.goal))),
    ];
    if !t.decisions.is_empty() {
        out.push(Line::raw(""));
        out.push(Line::styled("Needs you".to_owned(), theme.bold().patch(theme.fg(Color::Yellow))));
        for d in &t.decisions {
            out.push(Line::raw(format!("  {} #{}: {}   (y allow | n deny)", d.kind, d.id, clean(&d.summary))));
        }
    }
    if let Some(w) = &t.worker {
        out.push(Line::raw(""));
        out.push(heading("Worker"));
        let session = match (&w.session, &w.liveness) {
            (Some(s), Some(l)) => format!("session {s} {l}"),
            (Some(s), None) => format!("session {s}"),
            _ => "no session yet".to_owned(),
        };
        out.push(Line::raw(format!("  attempt {} {} | {session}", w.attempt, w.attempt_state)));
        out.push(Line::raw(format!("  {} on {}", w.worktree.display(), w.branch)));
    }
    out.push(Line::raw(""));
    out.push(heading("Usage"));
    out.push(Line::raw(match &t.usage {
        Some(u) => {
            let k = u.tokens;
            let check = match u.transcript {
                Some(tr) if tr == k => "transcript agrees",
                Some(_) => "transcript differs",
                None => "transcript not read yet",
            };
            format!(
                "  {} requests | {} in / {} out / {} cache read / {} cache write | ${:.4} est. ({check})",
                u.requests, k.input, k.output, k.cache_read, k.cache_write, u.cost_usd
            )
        }
        None => "  unknown (no telemetry yet)".to_owned(),
    }));
    if let Some(c) = &t.candidate {
        out.push(Line::raw(""));
        out.push(heading(&format!("Candidate {} on baton/{}", &c.commit[..12], task.id)));
        if c.checks.is_empty() {
            out.push(Line::raw("  no checks defined: nothing was verified"));
        }
        for check in &c.checks {
            let color = match check.state.as_str() {
                "passed" => Color::Green,
                "failed" | "error" => Color::Red,
                _ => Color::Yellow,
            };
            let code = check.exit_code.map(|c| format!(" (exit {c})")).unwrap_or_default();
            out.push(Line::from(vec![
                Span::styled(format!("  {:<11}", check.state), theme.fg(color)),
                Span::raw(format!(" {}{code}", clean(&check.command))),
            ]));
        }
    }
    if let Some(d) = app.selected_detail() {
        if !d.stat.is_empty() {
            out.push(Line::raw(""));
            out.push(heading("Changes"));
            out.extend(d.stat.lines().map(|l| Line::raw(format!("  {}", clean(l)))));
        }
        if let Some(m) = &d.last_message {
            out.push(Line::raw(""));
            out.push(heading("Worker's handoff (its own claim, not evidence)"));
            out.extend(m.lines().map(|l| Line::styled(format!("  {}", clean(l)), theme.fg(Color::DarkGray))));
        }
    }
    out
}

fn diff(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(t) = app.selected_task() else { return };
    let mut lines = vec![Line::styled(
        format!(
            "Diff of task #{}: {}..{} (baton/{})",
            t.task.id,
            &t.task.base_rev[..12],
            t.candidate.as_ref().map_or("?", |c| &c.commit[..12]),
            t.task.id
        ),
        theme.bold(),
    )];
    match app.selected_detail() {
        Some(d) => {
            for l in d.patch.lines() {
                let color = match l.chars().next() {
                    Some('+') => Color::Green,
                    Some('-') => Color::Red,
                    Some('@') => Color::Cyan,
                    _ => Color::Reset,
                };
                lines.push(Line::styled(clean(l), theme.fg(color)));
            }
            if d.truncated {
                lines.push(Line::raw(format!("(cut short; see the full diff with: git diff {}..baton/{})", &t.task.base_rev[..12], t.task.id)));
            }
        }
        None => lines.push(Line::raw("loading...")),
    }
    f.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), area);
}

fn help(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![Line::raw("Keys"), Line::raw("")];
    for b in BINDINGS.iter().filter(|b| !b.help.is_empty()) {
        let keys: Vec<String> = b.keys.iter().map(key_name).collect();
        lines.push(Line::raw(format!("  {:<12} {}", keys.join(" "), b.help)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw("Enter never grants a permission or accepts work: those take their own key."));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.scroll, 0)), area);
}

fn footer_lines(f: &mut Frame, area: Rect, app: &App, wide: bool, theme: &Theme) {
    // Off the list, j/k scroll; the hint would crowd out actions at 80 columns.
    let hints: Vec<&str> = BINDINGS
        .iter()
        .filter(|b| !b.hint.is_empty() && app.enabled(b.act, wide))
        .filter(|b| b.act != Act::Up || app.screen == Screen::List)
        .map(|b| b.hint)
        .collect();
    let second = match &app.prompt {
        Some(Prompt::ConfirmAccept { task, candidate }) => Line::styled(
            format!("Accept candidate {} for task #{task}? y = accept, any other key = no", &candidate[..candidate.len().min(12)]),
            theme.bold().patch(theme.fg(Color::Yellow)),
        ),
        Some(Prompt::DenyReason { decision, text }) => {
            Line::raw(format!("Deny #{decision}. Reason for the worker (optional; enter sends, esc cancels): {text}_"))
        }
        Some(Prompt::Changes { task, text, .. }) => Line::raw(format!("Changes for #{task} (enter sends, esc cancels): {text}_")),
        None => Line::styled(app.message.clone().unwrap_or_default(), theme.fg(Color::Yellow)),
    };
    let lines = vec![Line::styled(hints.join("  "), theme.fg(Color::DarkGray)), second];
    f.render_widget(Paragraph::new(lines), area);
}

fn key_name(k: &Key) -> String {
    match k {
        Key::Char(c) => c.to_string(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::PageUp => "pgup".into(),
        Key::PageDown => "pgdn".into(),
        Key::Enter => "enter".into(),
        Key::Esc => "esc".into(),
        Key::Backspace => "backspace".into(),
        Key::Interrupt => "ctrl-c".into(),
    }
}

/// Untrusted text (goals, worker output, diffs) can't move the cursor or
/// restyle the terminal: control characters become `?`.
pub fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' { ' ' } else if c.is_control() { '?' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::{App, Key};
    use crate::tui::sample;

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Compares with `snapshots/<name>.txt`; `UPDATE_SNAPSHOTS=1` rewrites it.
    fn snapshot(name: &str, actual: &str) {
        let path = format!("{}/src/tui/snapshots/{name}.txt", env!("CARGO_MANIFEST_DIR"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, format!("{actual}\n")).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}; run with UPDATE_SNAPSHOTS=1"));
        // Exactly the newline written above: empty bottom rows are part of the screen.
        let expected = expected.strip_suffix('\n').unwrap_or(&expected);
        assert_eq!(actual, expected, "\n--- {name} changed; if intended, run with UPDATE_SNAPSHOTS=1\n{actual}\n");
    }

    #[test]
    fn narrow_list_puts_what_needs_you_first() {
        let app = sample::app();
        let s = screen(&app, 80, 24);
        snapshot("list-80x24", &s);
        assert!(s.contains("needs you 1 | in progress 0 | to review 1 | done 1 | 5h 85% 7d 22% (3s)"), "{s}");
        assert!(s.lines().any(|l| l.starts_with(">! #2")), "the task that needs the owner is selected");
    }

    #[test]
    fn narrow_review_screen_shows_the_evidence() {
        let mut app = sample::app();
        app.selected = Some(3);
        app.on_key(Key::Enter, false);
        let s = screen(&app, 80, 24);
        snapshot("review-80x24", &s);
        assert!(s.contains("Candidate 206e9624e4d8 on baton/3"));
        assert!(s.contains("a accept") && s.contains("c changes") && s.contains("d defer"));
        assert!(!s.contains('\u{1b}'), "worker output can't inject escapes");
    }

    #[test]
    fn wide_layout_shows_list_and_task_together() {
        let mut app = sample::app();
        app.selected = Some(3);
        let s = screen(&app, 160, 40);
        snapshot("wide-160x40", &s);
        assert!(s.contains("#3 review_ready"), "detail pane present");
        assert!(s.contains(">+ #3"), "list pane present");
        assert!(s.contains("its own claim, not evidence"));
    }

    #[test]
    fn prompts_and_help() {
        let mut app = sample::app();
        app.on_key(Key::Enter, false);
        app.on_key(Key::Char('n'), false);
        for c in "not needed".chars() {
            app.on_key(Key::Char(c), false);
        }
        snapshot("deny-prompt-80x24", &screen(&app, 80, 24));

        let mut app = sample::app();
        app.on_key(Key::Char('?'), false);
        snapshot("help-80x24", &screen(&app, 80, 24));
    }

    #[test]
    fn control_characters_are_neutralized() {
        assert_eq!(clean("ok\x1b[31mred\x07\tend"), "ok?[31mred? end");
    }
}
