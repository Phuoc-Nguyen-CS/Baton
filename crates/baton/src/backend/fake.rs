//! Deterministic in-memory backend. It mirrors the Claude behaviours Baton must
//! handle: the backend assigns ids, and resuming a live session starts a copy.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, anyhow};

use super::{Backend, DispatchRequest, Liveness, Observation, Resumed, SessionRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Dispatch(DispatchRequest),
    Stop(String),
    Resume { short_id: String, prompt: String },
}

#[derive(Default)]
pub struct FakeBackend {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    counter: u32,
    sessions: Vec<Observation>,
    calls: Vec<Call>,
}

impl State {
    fn next(&mut self) -> u32 {
        self.counter += 1;
        self.counter
    }

    fn spawn(&mut self, name: Option<String>, cwd: Option<PathBuf>) -> SessionRef {
        let n = self.next();
        let short_id = format!("fake{n:04}");
        let session = SessionRef {
            uuid: Some(format!("{short_id}-0000-4000-8000-000000000000")),
            short_id,
        };
        self.sessions.push(Observation {
            session: session.clone(),
            name,
            pid: Some(10_000 + n),
            liveness: Liveness::Busy,
            waiting_for: None,
            cwd,
        });
        session
    }

    fn find(&mut self, short_id: &str) -> Result<&mut Observation> {
        self.sessions
            .iter_mut()
            .find(|o| o.session.short_id == short_id)
            .ok_or_else(|| anyhow!("no session {short_id}"))
    }
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Changes what polling reports for a session, as its process would.
    pub fn set_liveness(&self, short_id: &str, liveness: Liveness, waiting_for: Option<&str>) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let o = s.find(short_id)?;
        o.liveness = liveness;
        o.waiting_for = waiting_for.map(str::to_owned);
        Ok(())
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl Backend for FakeBackend {
    fn version(&self) -> Result<String> {
        Ok("fake".into())
    }

    fn dispatch(&self, req: &DispatchRequest) -> Result<SessionRef> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(Call::Dispatch(req.clone()));
        Ok(s.spawn(Some(req.name.clone()), Some(req.cwd.clone())))
    }

    fn list(&self) -> Result<Vec<Observation>> {
        Ok(self.state.lock().unwrap().sessions.clone())
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(Call::Stop(session.short_id.clone()));
        let o = s.find(&session.short_id)?;
        o.liveness = Liveness::NotRunning;
        o.pid = None;
        o.waiting_for = None;
        Ok(())
    }

    fn resume(&self, session: &SessionRef, prompt: &str) -> Result<Resumed> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(Call::Resume {
            short_id: session.short_id.clone(),
            prompt: prompt.to_owned(),
        });
        let o = s.find(&session.short_id)?;
        if o.liveness.is_running() {
            let (name, cwd) = (o.name.clone(), o.cwd.clone());
            return Ok(Resumed::Copy(s.spawn(name, cwd)));
        }
        let pid = 10_000 + s.next();
        let o = s.find(&session.short_id)?;
        o.liveness = Liveness::Busy;
        o.pid = Some(pid);
        Ok(Resumed::Same(o.session.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str) -> DispatchRequest {
        DispatchRequest {
            name: name.into(),
            cwd: PathBuf::from("/repo/.claude/worktrees").join(name),
            prompt: "do it".into(),
            model: None,
            role: None,
            settings: None,
        }
    }

    fn observe(b: &FakeBackend, short_id: &str) -> Observation {
        b.list()
            .unwrap()
            .into_iter()
            .find(|o| o.session.short_id == short_id)
            .unwrap()
    }

    #[test]
    fn dispatch_assigns_ids_and_lists_busy_sessions() {
        let b = FakeBackend::new();
        let a = b.dispatch(&request("baton-1-1")).unwrap();
        let c = b.dispatch(&request("baton-2-1")).unwrap();
        assert_ne!(a, c);
        let o = observe(&b, &a.short_id);
        assert_eq!(o.liveness, Liveness::Busy);
        assert_eq!(o.name.as_deref(), Some("baton-1-1"));
        assert_eq!(o.cwd, Some(PathBuf::from("/repo/.claude/worktrees/baton-1-1")));
    }

    #[test]
    fn resume_after_stop_wakes_the_same_session() {
        let b = FakeBackend::new();
        let s = b.dispatch(&request("baton-1-1")).unwrap();
        let pid = observe(&b, &s.short_id).pid;
        b.stop(&s).unwrap();
        assert_eq!(observe(&b, &s.short_id).liveness, Liveness::NotRunning);

        assert_eq!(b.resume(&s, "continue").unwrap(), Resumed::Same(s.clone()));
        let o = observe(&b, &s.short_id);
        assert_eq!(o.liveness, Liveness::Busy);
        assert_ne!(o.pid, pid);
    }

    #[test]
    fn resume_of_a_live_session_starts_a_copy() {
        let b = FakeBackend::new();
        let s = b.dispatch(&request("baton-1-1")).unwrap();
        b.set_liveness(&s.short_id, Liveness::Idle, None).unwrap();

        let Resumed::Copy(copy) = b.resume(&s, "continue").unwrap() else {
            panic!("live resume must copy");
        };
        assert_ne!(copy.short_id, s.short_id);
        assert_eq!(observe(&b, &copy.short_id).cwd, observe(&b, &s.short_id).cwd);
        assert_eq!(observe(&b, &s.short_id).liveness, Liveness::Idle);
    }

    #[test]
    fn unknown_sessions_are_errors_and_calls_are_recorded() {
        let b = FakeBackend::new();
        let ghost = SessionRef { short_id: "nope".into(), uuid: None };
        assert!(b.stop(&ghost).is_err());
        assert!(b.resume(&ghost, "x").is_err());
        assert_eq!(
            b.calls(),
            [
                Call::Stop("nope".into()),
                Call::Resume { short_id: "nope".into(), prompt: "x".into() }
            ]
        );
    }
}
