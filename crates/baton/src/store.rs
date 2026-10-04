//! SQLite state owned by the daemon (PLAN.md §4). A state change and its audit
//! entry are written in the same transaction.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde_json::json;

use crate::model::{Candidate, CheckResult, Task, TaskState, TaskView, Worker};

/// Entry `i` moves the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[include_str!("schema.sql")];

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }

    /// Creates a queued task. Repeating a request id returns the task it created
    /// (`false`) instead of a duplicate; reusing it for a different task is an error.
    pub fn create_task(&mut self, new: &NewTask) -> Result<(Task, bool)> {
        let tx = self.transaction()?;
        let existing = tx
            .query_row(&format!("{TASK_SELECT} WHERE request_id = ?1"), [&new.request_id], task_row)
            .optional()?;
        if let Some(task) = existing {
            if task.repo != new.repo || task.goal != new.goal || task.checks != new.checks || task.model != new.model {
                bail!("request id {} was already used for task {}", new.request_id, task.id);
            }
            return Ok((task, false));
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO task (request_id, repo, base_rev, goal, checks, model, state, observed_ms, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                new.request_id,
                new.repo.to_string_lossy(),
                new.base_rev,
                new.goal,
                serde_json::to_string(&new.checks)?,
                new.model,
                TaskState::Queued,
                now
            ],
        )?;
        let id = tx.last_insert_rowid();
        audit(&tx, "task", id, "created", Some(&json!({ "state": TaskState::Queued, "request_id": new.request_id })))?;
        let task = tx.query_row(&format!("{TASK_SELECT} WHERE id = ?1"), [id], task_row)?;
        tx.commit()?;
        Ok((task, true))
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!("{TASK_SELECT} ORDER BY id"))?;
        let tasks = stmt.query_map([], task_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(tasks)
    }

    pub fn task(&self, id: i64) -> Result<Task> {
        Ok(self.conn.query_row(&format!("{TASK_SELECT} WHERE id = ?1"), [id], task_row)?)
    }

    /// Every task with its latest attempt and that attempt's dispatched session.
    pub fn task_views(&self) -> Result<Vec<TaskView>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.state, w.path, w.branch, s.short_id, s.liveness, s.waiting_for, s.agent_type
             FROM attempt a JOIN workspace w ON w.id = a.workspace_id
             LEFT JOIN session s ON s.attempt_id = a.id AND s.origin = 'dispatch'
             WHERE a.task_id = ?1 ORDER BY a.seq DESC, s.id DESC LIMIT 1",
        )?;
        self.tasks()?
            .into_iter()
            .map(|task| {
                let worker = stmt
                    .query_row([task.id], |r| {
                        Ok(Worker {
                            attempt: r.get(0)?,
                            attempt_state: r.get(1)?,
                            worktree: PathBuf::from(r.get::<_, String>(2)?),
                            branch: r.get(3)?,
                            session: r.get(4)?,
                            liveness: r.get(5)?,
                            waiting_for: r.get(6)?,
                            agent_type: r.get(7)?,
                        })
                    })
                    .optional()?;
                let candidate = self.latest_candidate(task.id)?;
                Ok(TaskView { task, worker, candidate })
            })
            .collect()
    }

    /// The task's newest candidate with the latest result of each check run on it.
    pub fn latest_candidate(&self, task_id: i64) -> Result<Option<Candidate>> {
        let Some((id, commit, tree)) = self
            .conn
            .query_row(
                "SELECT id, commit_sha, tree_sha FROM candidate WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT check_cmd, state, exit_code, artifact FROM verification
             WHERE id IN (SELECT max(id) FROM verification WHERE candidate_id = ?1 GROUP BY check_cmd)
             ORDER BY id",
        )?;
        let checks = stmt
            .query_map([id], |r| {
                Ok(CheckResult {
                    command: r.get(0)?,
                    state: r.get(1)?,
                    exit_code: r.get(2)?,
                    output: r.get::<_, Option<String>>(3)?.map(PathBuf::from),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(Candidate { id, commit, tree, checks }))
    }

    /// Tasks waiting for Baton's checks whose worker has finished its turn.
    pub fn tasks_to_verify(&self) -> Result<Vec<(Task, Attempt)>> {
        let ids: Vec<(i64, i64)> = self
            .conn
            .prepare(
                "SELECT t.id, a.id FROM task t JOIN attempt a ON a.task_id = t.id
                 WHERE t.state = ?1 AND a.state = 'turn_ended'
                   AND a.seq = (SELECT max(seq) FROM attempt WHERE task_id = t.id)
                 ORDER BY t.id",
            )?
            .query_map([TaskState::Verifying], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        ids.into_iter().map(|(t, a)| Ok((self.task(t)?, self.attempt(a)?))).collect()
    }

    /// Records a candidate, reusing the newest one when it is the same commit
    /// (a verification restarted after a crash).
    pub fn add_candidate(&mut self, attempt: &Attempt, commit: &str, tree: &str, base_rev: &str) -> Result<i64> {
        let tx = self.transaction()?;
        let latest: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, commit_sha FROM candidate WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
                [attempt.task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, _)) = latest.filter(|(_, c)| c == commit) {
            return Ok(id);
        }
        tx.execute(
            "INSERT INTO candidate (task_id, attempt_id, commit_sha, tree_sha, base_rev, branch, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![attempt.task_id, attempt.id, commit, tree, base_rev, attempt.branch, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        audit(&tx, "candidate", id, "created", Some(&json!({ "task": attempt.task_id, "attempt": attempt.id, "commit": commit, "tree": tree })))?;
        tx.commit()?;
        Ok(id)
    }

    /// Marks checks left `running` on a candidate (by a daemon that died) as errors.
    pub fn interrupt_checks(&mut self, candidate_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE verification SET state = 'error', ended_ms = ?1 WHERE candidate_id = ?2 AND state = 'running'",
            params![now_ms(), candidate_id],
        )?;
        Ok(())
    }

    pub fn start_check(&mut self, candidate_id: i64, command: &str, env: &serde_json::Value) -> Result<i64> {
        let tx = self.transaction()?;
        tx.execute(
            "INSERT INTO verification (candidate_id, check_cmd, env, state, started_ms) VALUES (?1, ?2, ?3, 'running', ?4)",
            params![candidate_id, command, env.to_string(), now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        audit(&tx, "verification", id, "started", Some(&json!({ "candidate": candidate_id })))?;
        tx.commit()?;
        Ok(id)
    }

    pub fn finish_check(&mut self, id: i64, state: &str, exit_code: Option<i32>, output: &Path) -> Result<()> {
        let tx = self.transaction()?;
        tx.execute(
            "UPDATE verification SET state = ?1, exit_code = ?2, artifact = ?3, ended_ms = ?4 WHERE id = ?5",
            params![state, exit_code, output.to_string_lossy(), now_ms(), id],
        )?;
        audit(&tx, "verification", id, state, Some(&json!({ "exit_code": exit_code })))?;
        tx.commit()?;
        Ok(())
    }

    /// Marks a candidate's results as not applying to it: the worktree changed under them.
    pub fn invalidate_checks(&mut self, candidate_id: i64) -> Result<()> {
        let tx = self.transaction()?;
        tx.execute(
            "UPDATE verification SET state = 'invalidated' WHERE candidate_id = ?1 AND state IN ('passed', 'failed', 'error')",
            [candidate_id],
        )?;
        audit(&tx, "candidate", candidate_id, "checks_invalidated", None)?;
        tx.commit()?;
        Ok(())
    }

    /// Moves the task from `from` to `to`; false, changing nothing, if it has moved on.
    pub fn transition_task(&mut self, task_id: i64, from: TaskState, to: TaskState, reason: &str) -> Result<bool> {
        let tx = self.transaction()?;
        let current: TaskState = tx.query_row("SELECT state FROM task WHERE id = ?1", [task_id], |r| r.get(0))?;
        if current != from {
            return Ok(false);
        }
        update_task(&tx, task_id, Some(to), reason)?;
        tx.commit()?;
        Ok(true)
    }

    /// The oldest queued task, unless an attempt is already live: M1 runs one worker at a time.
    pub fn next_to_dispatch(&self) -> Result<Option<Task>> {
        let live: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM attempt WHERE state IN {LIVE_ATTEMPT}"),
            [],
            |r| r.get(0),
        )?;
        if live > 0 {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(&format!("{TASK_SELECT} WHERE state = ?1 ORDER BY id LIMIT 1"), [TaskState::Queued], task_row)
            .optional()?)
    }

    pub fn next_attempt_seq(&self, task_id: i64) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT coalesce(max(seq), 0) + 1 FROM attempt WHERE task_id = ?1", [task_id], |r| r.get(0))?)
    }

    /// Records the intent to start an attempt before anything outside Baton happens:
    /// its workspace (`intended`) and the attempt (`preparing`).
    pub fn begin_attempt(&mut self, new: &NewAttempt) -> Result<(i64, i64)> {
        let tx = self.transaction()?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO workspace (task_id, path, branch, base_rev, state, created_ms)
             VALUES (?1, ?2, ?3, ?4, 'intended', ?5)",
            params![new.task_id, new.worktree.to_string_lossy(), new.branch, new.base_rev, now],
        )?;
        let workspace_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO attempt (task_id, seq, workspace_id, config, state, state_reason, observed_ms, created_ms)
             VALUES (?1, ?2, ?3, ?4, 'preparing', 'creating the worktree', ?5, ?5)",
            params![new.task_id, new.seq, workspace_id, new.config.to_string(), now],
        )?;
        let attempt_id = tx.last_insert_rowid();
        audit(&tx, "attempt", attempt_id, "intended", Some(&json!({ "task": new.task_id, "seq": new.seq, "workspace": workspace_id })))?;
        update_task(&tx, new.task_id, Some(TaskState::Running), "preparing the worker")?;
        tx.commit()?;
        Ok((attempt_id, workspace_id))
    }

    pub fn set_workspace_state(&mut self, workspace_id: i64, state: &str) -> Result<()> {
        let tx = self.transaction()?;
        tx.execute("UPDATE workspace SET state = ?1 WHERE id = ?2", params![state, workspace_id])?;
        audit(&tx, "workspace", workspace_id, state, None)?;
        tx.commit()?;
        Ok(())
    }

    /// Moves an attempt from one of `from` to `to` and, unless the task is final,
    /// the task to `task_state` (or keeps its state) with `reason`. Returns false,
    /// changing nothing, when the attempt is in another state: a later observation
    /// already moved it on.
    pub fn transition_attempt(
        &mut self,
        attempt_id: i64,
        from: &[&str],
        to: &str,
        task_state: Option<TaskState>,
        reason: &str,
    ) -> Result<bool> {
        let tx = self.transaction()?;
        let (task_id, current): (i64, String) =
            tx.query_row("SELECT task_id, state FROM attempt WHERE id = ?1", [attempt_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if !from.contains(&current.as_str()) {
            return Ok(false);
        }
        let now = now_ms();
        let ended = (to == "failed").then_some(now);
        tx.execute(
            "UPDATE attempt SET state = ?1, state_reason = ?2, observed_ms = ?3, ended_ms = coalesce(?4, ended_ms) WHERE id = ?5",
            params![to, reason, now, ended, attempt_id],
        )?;
        audit(&tx, "attempt", attempt_id, to, Some(&json!({ "from": current })))?;
        update_task(&tx, task_id, task_state, reason)?;
        tx.commit()?;
        Ok(true)
    }

    pub fn attempt(&self, attempt_id: i64) -> Result<Attempt> {
        self.conn
            .query_row(
                "SELECT a.id, a.task_id, a.seq, a.state, a.config, w.path, w.branch FROM attempt a
                 JOIN workspace w ON w.id = a.workspace_id WHERE a.id = ?1",
                [attempt_id],
                |r| {
                    Ok(Attempt {
                        id: r.get(0)?,
                        task_id: r.get(1)?,
                        seq: r.get(2)?,
                        state: r.get(3)?,
                        config: r.get(4)?,
                        worktree: PathBuf::from(r.get::<_, String>(5)?),
                        branch: r.get(6)?,
                    })
                },
            )
            .optional()?
            .with_context(|| format!("no attempt {attempt_id}"))
    }

    /// Records a session seen for an attempt, filling in whatever is newly known.
    /// Returns the row id and whether the session is new.
    pub fn upsert_session(&mut self, attempt_id: i64, backend: &str, s: &SessionInfo) -> Result<(i64, bool)> {
        let tx = self.transaction()?;
        let now = now_ms();
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM session WHERE backend = ?1 AND short_id = ?2",
                params![backend, s.short_id],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                tx.execute(
                    "UPDATE session SET uuid = coalesce(?1, uuid), name = coalesce(?2, name),
                     agent_type = coalesce(?3, agent_type), model = coalesce(?4, model),
                     transcript = coalesce(?5, transcript), observed_ms = ?6 WHERE id = ?7",
                    params![s.uuid, s.name, s.agent_type, s.model, s.transcript, now, id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO session (attempt_id, backend, short_id, uuid, name, origin, agent_type, model, transcript, observed_ms, created_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    params![attempt_id, backend, s.short_id, s.uuid, s.name, s.origin, s.agent_type, s.model, s.transcript, now],
                )?;
                let id = tx.last_insert_rowid();
                audit(&tx, "session", id, "seen", Some(&json!({ "attempt": attempt_id, "short_id": s.short_id, "origin": s.origin })))?;
                id
            }
        };
        tx.commit()?;
        Ok((id, existing.is_none()))
    }

    pub fn sessions(&self, attempt_id: i64) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!("{SESSION_SELECT} WHERE s.attempt_id = ?1 ORDER BY s.id"))?;
        let rows = stmt.query_map([attempt_id], session_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Sessions of attempts that may still have a live worker.
    pub fn live_sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!("{SESSION_SELECT} WHERE a.state IN {LIVE_ATTEMPT} ORDER BY s.id"))?;
        let rows = stmt.query_map([], session_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn set_liveness(&mut self, session_id: i64, liveness: &str, waiting_for: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET liveness = ?1, waiting_for = ?2, observed_ms = ?3 WHERE id = ?4",
            params![liveness, waiting_for, now_ms(), session_id],
        )?;
        Ok(())
    }

    pub fn audit_event(&mut self, entity: &str, entity_id: i64, event: &str, detail: &serde_json::Value) -> Result<()> {
        let tx = self.transaction()?;
        audit(&tx, entity, entity_id, event, Some(detail))?;
        tx.commit()?;
        Ok(())
    }

    /// Records what a running task's worker is doing. In any other state the reason
    /// explains that state (e.g. the check counts), so it is left alone.
    pub fn note_activity(&mut self, task_id: i64, activity: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE task SET state_reason = ?1, observed_ms = ?2 WHERE id = ?3 AND state = ?4",
            params![activity, now_ms(), task_id, TaskState::Running],
        )?;
        Ok(())
    }

    /// Updates the task's state and reason unless it is final; audited when the state changes.
    pub fn set_task_state(&mut self, task_id: i64, state: Option<TaskState>, reason: &str) -> Result<()> {
        let tx = self.transaction()?;
        update_task(&tx, task_id, state, reason)?;
        tx.commit()?;
        Ok(())
    }
}

/// Attempt states in which a worker may be alive.
const LIVE_ATTEMPT: &str = "('preparing', 'dispatching', 'running', 'turn_ended')";

fn update_task(tx: &Transaction, task_id: i64, state: Option<TaskState>, reason: &str) -> Result<()> {
    let current: TaskState = tx.query_row("SELECT state FROM task WHERE id = ?1", [task_id], |r| r.get(0))?;
    if current.is_terminal() {
        return Ok(());
    }
    let next = state.unwrap_or(current);
    tx.execute(
        "UPDATE task SET state = ?1, state_reason = ?2, observed_ms = ?3 WHERE id = ?4",
        params![next, reason, now_ms(), task_id],
    )?;
    if next != current {
        audit(tx, "task", task_id, "state", Some(&json!({ "from": current, "to": next })))?;
    }
    Ok(())
}

pub struct NewTask {
    pub request_id: String,
    pub repo: PathBuf,
    pub base_rev: String,
    pub goal: String,
    pub checks: Vec<String>,
    pub model: Option<String>,
}

pub struct NewAttempt<'a> {
    pub task_id: i64,
    pub seq: i64,
    pub worktree: &'a Path,
    pub branch: &'a str,
    pub base_rev: &'a str,
    pub config: &'a serde_json::Value,
}

#[derive(Debug)]
pub struct Attempt {
    pub id: i64,
    pub task_id: i64,
    pub seq: i64,
    pub state: String,
    /// JSON, as recorded at intent.
    pub config: String,
    pub worktree: PathBuf,
    pub branch: String,
}

#[derive(Debug, Default)]
pub struct SessionInfo {
    pub short_id: String,
    pub uuid: Option<String>,
    pub name: Option<String>,
    /// `dispatch` or `copy`; only used when the session is new.
    pub origin: &'static str,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub transcript: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    pub attempt_id: i64,
    pub task_id: i64,
    pub short_id: String,
    pub uuid: Option<String>,
    pub origin: String,
    pub liveness: Option<String>,
    pub waiting_for: Option<String>,
}

const SESSION_SELECT: &str = "SELECT s.id, s.attempt_id, a.task_id, s.short_id, s.uuid, s.origin, s.liveness, s.waiting_for
     FROM session s JOIN attempt a ON a.id = s.attempt_id";

fn session_row(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        attempt_id: r.get(1)?,
        task_id: r.get(2)?,
        short_id: r.get(3)?,
        uuid: r.get(4)?,
        origin: r.get(5)?,
        liveness: r.get(6)?,
        waiting_for: r.get(7)?,
    })
}

const TASK_SELECT: &str = "SELECT id, request_id, repo, base_rev, goal, checks, model, state, state_reason, observed_ms, created_ms FROM task";

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    let checks: String = r.get(5)?;
    Ok(Task {
        id: r.get(0)?,
        request_id: r.get(1)?,
        repo: PathBuf::from(r.get::<_, String>(2)?),
        base_rev: r.get(3)?,
        goal: r.get(4)?,
        checks: serde_json::from_str(&checks)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(e)))?,
        model: r.get(6)?,
        state: r.get(7)?,
        state_reason: r.get(8)?,
        observed_ms: r.get(9)?,
        created_ms: r.get(10)?,
    })
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let latest = MIGRATIONS.len() as i64;
    if current > latest {
        bail!("state database is schema v{current}, newer than this baton (v{latest}); upgrade baton");
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying schema v{}", i + 1))?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

/// Appends an audit entry inside `tx`; returns its sequence number.
pub fn audit(
    tx: &Transaction,
    entity: &str,
    entity_id: i64,
    event: &str,
    detail: Option<&serde_json::Value>,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO audit (at_ms, entity, entity_id, event, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now_ms(), entity, entity_id, event, detail.map(|d| d.to_string())],
    )?;
    Ok(tx.last_insert_rowid())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_task(request_id: &str, goal: &str) -> NewTask {
        NewTask {
            request_id: request_id.into(),
            repo: PathBuf::from("/repo"),
            base_rev: "abc123".into(),
            goal: goal.into(),
            checks: vec!["cargo test".into()],
            model: None,
        }
    }

    fn begin(store: &mut Store, task_id: i64) -> (i64, i64) {
        let seq = store.next_attempt_seq(task_id).unwrap();
        let worktree = PathBuf::from(format!("/repo/.claude/worktrees/baton-{task_id}-{seq}"));
        store
            .begin_attempt(&NewAttempt {
                task_id,
                seq,
                worktree: &worktree,
                branch: &format!("baton/{task_id}"),
                base_rev: "abc123",
                config: &json!({}),
            })
            .unwrap()
    }

    #[test]
    fn one_live_attempt_at_a_time() {
        let mut store = Store::open_in_memory().unwrap();
        let (t1, _) = store.create_task(&new_task("r1", "first")).unwrap();
        let (t2, _) = store.create_task(&new_task("r2", "second")).unwrap();
        assert_eq!(store.next_to_dispatch().unwrap().unwrap().id, t1.id);

        let (a1, _) = begin(&mut store, t1.id);
        assert_eq!(store.task(t1.id).unwrap().state, TaskState::Running);
        assert!(store.next_to_dispatch().unwrap().is_none(), "t1's attempt is live");

        // A transition from the wrong state changes nothing.
        assert!(!store.transition_attempt(a1, &["running"], "turn_ended", None, "x").unwrap());
        assert!(store.transition_attempt(a1, &["preparing"], "failed", Some(TaskState::Failed), "boom").unwrap());
        assert_eq!(store.next_to_dispatch().unwrap().unwrap().id, t2.id);
        assert_eq!(store.next_attempt_seq(t1.id).unwrap(), 2);
    }

    #[test]
    fn final_task_states_stick() {
        let mut store = Store::open_in_memory().unwrap();
        let (t, _) = store.create_task(&new_task("r1", "first")).unwrap();
        let (a, _) = begin(&mut store, t.id);
        store.transition_attempt(a, &["preparing"], "failed", Some(TaskState::Failed), "role mismatch").unwrap();
        store.set_task_state(t.id, Some(TaskState::Running), "late hook").unwrap();
        let t = store.task(t.id).unwrap();
        assert_eq!((t.state, t.state_reason.as_deref()), (TaskState::Failed, Some("role mismatch")));
    }

    #[test]
    fn sessions_upsert_and_views_show_the_dispatched_one() {
        let mut store = Store::open_in_memory().unwrap();
        let (t, _) = store.create_task(&new_task("r1", "first")).unwrap();
        let (a, _) = begin(&mut store, t.id);

        // A hook can report the session before dispatch returns; both land on one row.
        let from_hook = SessionInfo { short_id: "abcd1234".into(), uuid: Some("abcd1234-uuid".into()), agent_type: Some("baton-worker".into()), origin: "dispatch", ..Default::default() };
        let (id, new) = store.upsert_session(a, "claude", &from_hook).unwrap();
        assert!(new);
        let from_dispatch = SessionInfo { short_id: "abcd1234".into(), name: Some("baton-1-1".into()), origin: "dispatch", ..Default::default() };
        assert_eq!(store.upsert_session(a, "claude", &from_dispatch).unwrap(), (id, false));
        store.set_liveness(id, "busy", None).unwrap();
        store.upsert_session(a, "claude", &SessionInfo { short_id: "ffff0000".into(), origin: "copy", ..Default::default() }).unwrap();

        let sessions = store.live_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].uuid.as_deref(), Some("abcd1234-uuid"), "uuid kept when a later report lacks it");

        let views = store.task_views().unwrap();
        let w = views[0].worker.as_ref().unwrap();
        assert_eq!((w.session.as_deref(), w.liveness.as_deref()), (Some("abcd1234"), Some("busy")));
        assert_eq!(w.agent_type.as_deref(), Some("baton-worker"));
        assert_eq!(w.branch, "baton/1");
    }

    #[test]
    fn create_task_is_idempotent_per_request_id() {
        let mut store = Store::open_in_memory().unwrap();
        let (t1, created) = store.create_task(&new_task("r1", "add a greeting")).unwrap();
        assert!(created);
        assert_eq!(t1.state, TaskState::Queued);
        assert_eq!(t1.checks, ["cargo test"]);

        let (again, created) = store.create_task(&new_task("r1", "add a greeting")).unwrap();
        assert!(!created);
        assert_eq!(again, t1);

        let err = store.create_task(&new_task("r1", "something else")).err().unwrap();
        assert!(err.to_string().contains("already used"), "{err}");

        store.create_task(&new_task("r2", "second")).unwrap();
        let goals: Vec<String> = store.tasks().unwrap().into_iter().map(|t| t.goal).collect();
        assert_eq!(goals, ["add a greeting", "second"]);

        let audits: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM audit WHERE event = 'created'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(audits, 2, "one audit entry per created task, none for repeats");
    }

    fn insert_task(conn: &Connection, request_id: &str) -> rusqlite::Result<i64> {
        conn.execute(
            "INSERT INTO task (request_id, repo, base_rev, goal, checks, state, observed_ms, created_ms)
             VALUES (?1, '/repo', 'abc123', 'goal', '[]', 'queued', 0, 0)",
            [request_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    #[test]
    fn fresh_store_has_every_table() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), 1);
        let mut stmt = store
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            tables,
            ["attempt", "audit", "candidate", "decision", "session", "task", "verification", "workspace"]
        );
    }

    #[test]
    fn reopening_keeps_data_and_does_not_remigrate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baton.db");
        {
            let store = Store::open(&path).unwrap();
            insert_task(store.conn(), "r1").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 1);
        let n: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM task", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baton.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let err = Store::open(&path).err().expect("newer schema must be refused");
        assert!(err.to_string().contains("newer than this baton"), "{err}");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().unwrap();
        let err = store.conn().execute(
            "INSERT INTO workspace (task_id, path, branch, base_rev, state, created_ms)
             VALUES (42, '/w', 'b', 'abc', 'intended', 0)",
            [],
        );
        assert!(err.is_err(), "workspace for a missing task was accepted");
    }

    #[test]
    fn task_request_id_is_unique() {
        let store = Store::open_in_memory().unwrap();
        insert_task(store.conn(), "same").unwrap();
        assert!(insert_task(store.conn(), "same").is_err());
    }

    #[test]
    fn audit_is_sequenced_and_atomic_with_state() {
        let mut store = Store::open_in_memory().unwrap();

        let tx = store.transaction().unwrap();
        let id = insert_task(&tx, "r1").unwrap();
        let s1 = audit(&tx, "task", id, "created", Some(&json!({"state": "queued"}))).unwrap();
        let s2 = audit(&tx, "task", id, "state", None).unwrap();
        tx.commit().unwrap();
        assert!(s2 > s1);

        // A rolled-back change leaves neither the state nor its audit entry.
        let tx = store.transaction().unwrap();
        let id2 = insert_task(&tx, "r2").unwrap();
        audit(&tx, "task", id2, "created", None).unwrap();
        drop(tx);

        let events: Vec<(i64, String)> = store
            .conn()
            .prepare("SELECT entity_id, event FROM audit ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(events, [(id, "created".into()), (id, "state".into())]);
    }
}
