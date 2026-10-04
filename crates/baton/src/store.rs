//! SQLite state owned by the daemon (PLAN.md §4). A state change and its audit
//! entry are written in the same transaction.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde_json::json;

use crate::model::{Task, TaskState};

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
            if task.repo != new.repo || task.goal != new.goal || task.checks != new.checks {
                bail!("request id {} was already used for task {}", new.request_id, task.id);
            }
            return Ok((task, false));
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO task (request_id, repo, base_rev, goal, checks, state, observed_ms, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                new.request_id,
                new.repo.to_string_lossy(),
                new.base_rev,
                new.goal,
                serde_json::to_string(&new.checks)?,
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
}

pub struct NewTask {
    pub request_id: String,
    pub repo: PathBuf,
    pub base_rev: String,
    pub goal: String,
    pub checks: Vec<String>,
}

const TASK_SELECT: &str = "SELECT id, request_id, repo, base_rev, goal, checks, state, state_reason, observed_ms, created_ms FROM task";

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
        state: r.get(6)?,
        state_reason: r.get(7)?,
        observed_ms: r.get(8)?,
        created_ms: r.get(9)?,
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
        }
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
