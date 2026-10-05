//! The TUI's state and key handling, free of I/O: a key returns an `Effect` that
//! the event loop carries out (a daemon call, attaching, quitting).

use crate::model::{Quota, TaskDetail, TaskState, TaskView};
use crate::protocol::{Request, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Task,
    Diff,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Esc,
    Backspace,
    /// Ctrl+C
    Interrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Up,
    Down,
    PageUp,
    PageDown,
    Open,
    Back,
    Diff,
    Allow,
    Deny,
    Accept,
    Changes,
    Defer,
    Attach,
    Help,
    Quit,
}

pub struct Binding {
    pub act: Act,
    pub keys: &'static [Key],
    /// Footer text; empty to leave it out of the footer.
    pub hint: &'static str,
    pub help: &'static str,
    pub screens: &'static [Screen],
}

use Screen::{Diff, Help, List, Task};

/// The one registry behind key handling, the footer and the help screen (PLAN §7).
pub const BINDINGS: &[Binding] = &[
    Binding { act: Act::Up, keys: &[Key::Up, Key::Char('k')], hint: "j/k move", help: "Move up (on the list) or scroll", screens: &[List, Task, Diff, Help] },
    Binding { act: Act::Down, keys: &[Key::Down, Key::Char('j')], hint: "", help: "Move down (on the list) or scroll", screens: &[List, Task, Diff, Help] },
    Binding { act: Act::PageUp, keys: &[Key::PageUp], hint: "", help: "Scroll a page up", screens: &[Task, Diff] },
    Binding { act: Act::PageDown, keys: &[Key::PageDown], hint: "", help: "Scroll a page down", screens: &[Task, Diff] },
    Binding { act: Act::Open, keys: &[Key::Enter], hint: "enter open", help: "Open the selected task", screens: &[List] },
    Binding { act: Act::Back, keys: &[Key::Esc], hint: "esc back", help: "Go back", screens: &[Task, Diff, Help] },
    Binding { act: Act::Allow, keys: &[Key::Char('y')], hint: "y allow", help: "Allow the task's pending permission request", screens: &[Task] },
    Binding { act: Act::Deny, keys: &[Key::Char('n')], hint: "n deny", help: "Deny it, with an optional reason for the worker", screens: &[Task] },
    Binding { act: Act::Accept, keys: &[Key::Char('a')], hint: "a accept", help: "Accept the candidate (asks to confirm); it stays on baton/<task>", screens: &[Task] },
    Binding { act: Act::Changes, keys: &[Key::Char('c')], hint: "c changes", help: "Ask the worker for changes", screens: &[Task] },
    Binding { act: Act::Defer, keys: &[Key::Char('d')], hint: "d defer", help: "Decide later", screens: &[Task] },
    Binding { act: Act::Diff, keys: &[Key::Char('v')], hint: "v diff", help: "View the candidate's full diff", screens: &[Task] },
    Binding { act: Act::Attach, keys: &[Key::Char('t')], hint: "t attach", help: "Take over the worker's terminal (claude attach); detach to return", screens: &[Task] },
    Binding { act: Act::Help, keys: &[Key::Char('?')], hint: "? help", help: "Show this help", screens: &[List, Task, Diff] },
    Binding { act: Act::Quit, keys: &[Key::Char('q'), Key::Interrupt], hint: "q quit", help: "Quit; workers keep running", screens: &[List, Task, Diff, Help] },
];

/// Text the owner is typing, or a question awaiting y/n. Captures every key, so
/// typing can't trigger actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    ConfirmAccept { task: i64, candidate: String },
    DenyReason { decision: i64, text: String },
    Changes { task: i64, candidate: String, text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    Quit,
    Call(Request),
    Attach(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attention {
    NeedsYou,
    Review,
    Progressing,
    Done,
}

pub fn attention(t: &TaskView) -> Attention {
    use TaskState::*;
    if !t.decisions.is_empty() {
        return Attention::NeedsYou;
    }
    match t.task.state {
        WaitingPermission | WaitingInput | Failed => Attention::NeedsYou,
        ReviewReady => Attention::Review,
        Queued | Running | Verifying | Reconciling => Attention::Progressing,
        Accepted | Cancelled => Attention::Done,
    }
}

pub struct App {
    /// Newest first; the order never depends on state, so rows don't jump.
    pub tasks: Vec<TaskView>,
    pub quota: Option<Quota>,
    pub detail: Option<(i64, TaskDetail)>,
    pub selected: Option<i64>,
    pub screen: Screen,
    pub prompt: Option<Prompt>,
    pub scroll: u16,
    /// The outcome of the last action, or an error.
    pub message: Option<String>,
    pub now_ms: i64,
    pub color: bool,
}

impl App {
    pub fn new(color: bool) -> Self {
        Self { tasks: vec![], quota: None, detail: None, selected: None, screen: List, prompt: None, scroll: 0, message: None, now_ms: 0, color }
    }

    /// Takes a new status, keeping the selection on the same task.
    pub fn set_status(&mut self, mut tasks: Vec<TaskView>, quota: Option<Quota>, now_ms: i64) {
        tasks.sort_by_key(|t| std::cmp::Reverse(t.task.id));
        self.tasks = tasks;
        self.quota = quota;
        self.now_ms = now_ms;
        if !self.selected.is_some_and(|id| self.tasks.iter().any(|t| t.task.id == id)) {
            self.selected = self.tasks.iter().min_by_key(|t| attention(t)).map(|t| t.task.id);
        }
    }

    pub fn selected_task(&self) -> Option<&TaskView> {
        self.tasks.iter().find(|t| Some(t.task.id) == self.selected)
    }

    pub fn selected_detail(&self) -> Option<&TaskDetail> {
        self.detail.as_ref().filter(|(id, _)| Some(*id) == self.selected).map(|(_, d)| d)
    }

    /// The task screen's actions also work from the list when both are visible.
    fn shows_task(&self, wide: bool) -> bool {
        self.screen == Task || (wide && self.screen == List)
    }

    /// Whether `act` applies right now: used for both keys and the footer.
    pub fn enabled(&self, act: Act, wide: bool) -> bool {
        let screen_ok = |b: &Binding| b.screens.contains(&self.screen) || (self.shows_task(wide) && b.screens.contains(&Task));
        if !BINDINGS.iter().any(|b| b.act == act && screen_ok(b)) {
            return false;
        }
        let t = self.selected_task();
        let review = t.is_some_and(|t| t.task.state == TaskState::ReviewReady && t.candidate.is_some());
        match act {
            Act::Allow | Act::Deny => t.is_some_and(|t| t.decisions.iter().any(|d| d.kind == "permission")),
            Act::Accept | Act::Changes | Act::Defer => review,
            Act::Diff => t.is_some_and(|t| t.candidate.is_some()),
            Act::Attach => t.and_then(|t| t.worker.as_ref()).is_some_and(|w| w.session.is_some()),
            Act::Open => t.is_some(),
            _ => true,
        }
    }

    pub fn on_key(&mut self, key: Key, wide: bool) -> Effect {
        if let Some(prompt) = self.prompt.take() {
            return self.on_prompt_key(prompt, key);
        }
        let Some(act) = BINDINGS.iter().find(|b| b.keys.contains(&key)).map(|b| b.act) else {
            return Effect::None;
        };
        if !self.enabled(act, wide) {
            return Effect::None;
        }
        self.message = None;
        let task = self.selected_task().cloned();
        let candidate = task.as_ref().and_then(|t| t.candidate.as_ref()).map(|c| c.commit.clone()).unwrap_or_default();
        match act {
            // The list moves the selection; every other screen scrolls.
            Act::Up | Act::Down if self.screen != List => {
                self.scroll = if act == Act::Up { self.scroll.saturating_sub(1) } else { self.scroll.saturating_add(1) };
            }
            Act::Up | Act::Down => self.move_selection(if act == Act::Up { -1 } else { 1 }),
            Act::PageUp => self.scroll = self.scroll.saturating_sub(10),
            Act::PageDown => self.scroll = self.scroll.saturating_add(10),
            Act::Open => self.go(Task),
            Act::Back => self.go(if self.screen == Diff { Task } else { List }),
            Act::Diff => self.go(Diff),
            Act::Help => self.go(Help),
            Act::Quit => return Effect::Quit,
            Act::Allow => {
                let id = task.and_then(|t| t.decisions.iter().find(|d| d.kind == "permission").map(|d| d.id));
                return id.map_or(Effect::None, |id| Effect::Call(Request::Decide { id, answer: "allow".into(), note: None }));
            }
            Act::Deny => {
                let id = task.and_then(|t| t.decisions.iter().find(|d| d.kind == "permission").map(|d| d.id));
                self.prompt = id.map(|decision| Prompt::DenyReason { decision, text: String::new() });
            }
            Act::Accept => self.prompt = task.map(|t| Prompt::ConfirmAccept { task: t.task.id, candidate }),
            Act::Changes => self.prompt = task.map(|t| Prompt::Changes { task: t.task.id, candidate, text: String::new() }),
            Act::Defer => {
                return task.map_or(Effect::None, |t| {
                    Effect::Call(Request::Review { task: t.task.id, verdict: Verdict::Defer, candidate: Some(candidate), note: None })
                });
            }
            Act::Attach => {
                return task.and_then(|t| t.worker.and_then(|w| w.session)).map_or(Effect::None, Effect::Attach);
            }
        }
        Effect::None
    }

    fn on_prompt_key(&mut self, prompt: Prompt, key: Key) -> Effect {
        match (prompt, key) {
            (_, Key::Esc | Key::Interrupt) => {
                self.message = Some("cancelled".into());
                Effect::None
            }
            (Prompt::ConfirmAccept { task, candidate }, Key::Char('y')) => {
                Effect::Call(Request::Review { task, verdict: Verdict::Accept, candidate: Some(candidate), note: None })
            }
            (Prompt::ConfirmAccept { .. }, _) => {
                self.message = Some("not accepted".into());
                Effect::None
            }
            (Prompt::DenyReason { decision, text }, Key::Enter) => {
                let note = Some(text.trim().to_owned()).filter(|t| !t.is_empty());
                Effect::Call(Request::Decide { id: decision, answer: "deny".into(), note })
            }
            (Prompt::Changes { task, candidate, text }, Key::Enter) if !text.trim().is_empty() => Effect::Call(Request::Review {
                task,
                verdict: Verdict::Changes,
                candidate: Some(candidate),
                note: Some(text.trim().to_owned()),
            }),
            (mut prompt, key) => {
                if let Prompt::DenyReason { text, .. } | Prompt::Changes { text, .. } = &mut prompt {
                    match key {
                        Key::Char(c) => text.push(c),
                        Key::Backspace => drop(text.pop()),
                        _ => {}
                    }
                }
                self.prompt = Some(prompt);
                Effect::None
            }
        }
    }

    fn go(&mut self, screen: Screen) {
        self.screen = screen;
        self.scroll = 0;
    }

    #[cfg(test)]
    pub fn task_ids(&self) -> Vec<i64> {
        self.tasks.iter().map(|t| t.task.id).collect()
    }

    fn move_selection(&mut self, by: isize) {
        let Some(i) = self.tasks.iter().position(|t| Some(t.task.id) == self.selected) else {
            self.selected = self.tasks.first().map(|t| t.task.id);
            return;
        };
        let j = (i as isize + by).clamp(0, self.tasks.len() as isize - 1) as usize;
        self.selected = Some(self.tasks[j].task.id);
        self.scroll = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::sample;

    fn review_call(e: &Effect) -> Option<(Verdict, Option<String>)> {
        match e {
            Effect::Call(Request::Review { verdict, candidate, .. }) => Some((*verdict, candidate.clone())),
            _ => None,
        }
    }

    #[test]
    fn rows_stay_in_place_and_the_selection_follows_its_task() {
        let mut app = sample::app();
        assert_eq!(app.task_ids(), [3, 2, 1], "newest first");
        assert_eq!(app.selected, Some(2), "what needs the owner is selected first");
        let mut tasks = app.tasks.clone();
        tasks[1].task.state = TaskState::Running;
        tasks[1].decisions.clear();
        tasks.reverse();
        app.set_status(tasks, None, sample::NOW);
        assert_eq!((app.task_ids(), app.selected), (vec![3, 2, 1], Some(2)), "a state change moves nothing");
    }

    #[test]
    fn accepting_needs_its_own_key_and_a_confirmation() {
        let mut app = sample::app();
        app.selected = Some(3);
        app.on_key(Key::Enter, false);
        assert_eq!(app.screen, Screen::Task);
        assert_eq!(app.on_key(Key::Enter, false), Effect::None, "enter does nothing on a task");
        assert_eq!(app.on_key(Key::Char('a'), false), Effect::None);
        assert!(matches!(app.prompt, Some(Prompt::ConfirmAccept { task: 3, .. })));
        assert_eq!(app.on_key(Key::Char('x'), false), Effect::None);
        assert_eq!(app.message.as_deref(), Some("not accepted"));

        app.on_key(Key::Char('a'), false);
        let effect = app.on_key(Key::Char('y'), false);
        let commit = app.selected_task().unwrap().candidate.as_ref().unwrap().commit.clone();
        assert_eq!(review_call(&effect), Some((Verdict::Accept, Some(commit))), "bound to the candidate on screen");
    }

    #[test]
    fn permissions_take_y_or_n_and_typing_never_triggers_actions() {
        let mut app = sample::app();
        app.on_key(Key::Enter, false);
        assert_eq!(app.on_key(Key::Char('y'), false), Effect::Call(Request::Decide { id: 7, answer: "allow".into(), note: None }));

        app.on_key(Key::Char('n'), false);
        for c in "q quits? no".chars() {
            assert_eq!(app.on_key(Key::Char(c), false), Effect::None, "typed {c:?}");
        }
        app.on_key(Key::Backspace, false);
        let effect = app.on_key(Key::Enter, false);
        assert_eq!(effect, Effect::Call(Request::Decide { id: 7, answer: "deny".into(), note: Some("q quits? n".into()) }));

        app.on_key(Key::Char('n'), false);
        assert_eq!(app.on_key(Key::Interrupt, false), Effect::None, "ctrl-c cancels a prompt");
        assert_eq!(app.message.as_deref(), Some("cancelled"));
        assert_eq!(app.on_key(Key::Interrupt, false), Effect::Quit);
    }

    #[test]
    fn task_actions_work_from_the_list_only_when_the_task_is_visible() {
        let mut app = sample::app();
        app.selected = Some(3);
        assert_eq!(app.on_key(Key::Char('d'), false), Effect::None, "narrow list: open the task first");
        assert_eq!(review_call(&app.on_key(Key::Char('d'), true)).map(|r| r.0), Some(Verdict::Defer));
        assert_eq!(app.on_key(Key::Char('t'), true), Effect::Attach("411afd47".into()));
        app.selected = Some(1);
        assert_eq!(app.on_key(Key::Char('t'), true), Effect::None, "no session to attach to");
    }

    #[test]
    fn request_changes_needs_a_message() {
        let mut app = sample::app();
        app.selected = Some(3);
        app.on_key(Key::Enter, false);
        app.on_key(Key::Char('c'), false);
        assert_eq!(app.on_key(Key::Enter, false), Effect::None, "nothing to send yet");
        for c in "say hi".chars() {
            app.on_key(Key::Char(c), false);
        }
        let effect = app.on_key(Key::Enter, false);
        assert!(matches!(effect, Effect::Call(Request::Review { verdict: Verdict::Changes, note: Some(ref n), .. }) if n == "say hi"));
    }
}
