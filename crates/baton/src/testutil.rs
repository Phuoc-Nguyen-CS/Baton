//! Shared test fixture: a daemon context on an in-memory store and the fake
//! backend, with a real git repository to work in.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::backend::fake::FakeBackend;
use crate::git::{self, tests::git_in};
use crate::model::{Task, Worker};
use crate::paths::Paths;
use crate::permission::Waiters;
use crate::store::{NewTask, Store};
use crate::worker::{Ctx, on_hook};

pub struct Fixture {
    pub ctx: Ctx,
    pub fake: Arc<FakeBackend>,
    pub repo: PathBuf,
    pub head: String,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

pub fn fixture() -> Fixture {
    let (home, repos) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let repo = repos.path().canonicalize().unwrap().join("r");
    fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q"]);
    git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let head = git::repo(&repo).unwrap().head;
    let fake = Arc::new(FakeBackend::new());
    let ctx = Ctx {
        paths: Paths { home: home.path().into() },
        store: Mutex::new(Store::open_in_memory().unwrap()),
        backend: fake.clone(),
        backend_name: "fake",
        exe: "/opt/baton/bin/baton".into(),
        waiters: Waiters::default(),
        permission_wait: Duration::from_millis(300),
    };
    Fixture { ctx, fake, repo, head, _dirs: (home, repos) }
}

impl Fixture {
    pub fn add_task(&self, goal: &str) -> i64 {
        let new = NewTask {
            request_id: goal.into(),
            repo: self.repo.clone(),
            base_rev: self.head.clone(),
            goal: goal.into(),
            checks: vec!["test -f greeting.txt".into()],
            model: Some("haiku".into()),
        };
        self.ctx.store.lock().unwrap().create_task(&new).unwrap().0.id
    }

    pub fn task(&self, id: i64) -> Task {
        self.ctx.store.lock().unwrap().task(id).unwrap()
    }

    pub fn worker(&self, id: i64) -> Worker {
        let views = self.ctx.store.lock().unwrap().task_views().unwrap();
        views.into_iter().find(|v| v.task.id == id).unwrap().worker.unwrap()
    }

    pub fn hook(&self, attempt: i64, event: &str, mut input: Value) -> Option<Value> {
        input["hook_event_name"] = event.into();
        on_hook(&self.ctx, attempt, event, &input, None).unwrap()
    }
}

/// A `SessionStart` input for the session with this short id.
pub fn start(session: &str, agent_type: Option<&str>) -> Value {
    let mut v = json!({ "session_id": format!("{session}-0000-4000-8000-000000000000"), "source": "startup", "model": "claude-haiku-4-5" });
    if let Some(a) = agent_type {
        v["agent_type"] = a.into();
    }
    v
}
