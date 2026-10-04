//! SQLite state owned by the daemon (PLAN.md §4). A state change and its audit
//! entry are written in the same transaction.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, params};

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
    use serde_json::json;

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
