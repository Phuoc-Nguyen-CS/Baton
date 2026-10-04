-- Baton state, schema v1 (PLAN.md §4, docs/architecture.md §4).
-- Times are Unix epoch milliseconds (UTC). JSON columns hold small, structured
-- metadata only; bulk output (logs, check output) lives in files referenced by path.
-- State values are validated in Rust, not with CHECK constraints, so adding a
-- state later doesn't need a table rebuild.

-- The owner's unit of work: one goal against one repository and base revision.
CREATE TABLE task (
    id           INTEGER PRIMARY KEY,
    request_id   TEXT NOT NULL UNIQUE,   -- client-supplied; makes intake retries idempotent
    repo         TEXT NOT NULL,          -- absolute path of the repo's main worktree
    base_rev     TEXT NOT NULL,          -- commit the task starts from
    goal         TEXT NOT NULL,
    checks       TEXT NOT NULL,          -- JSON array of acceptance check commands
    model        TEXT,                   -- worker model; NULL = the backend's default
    state        TEXT NOT NULL,
    state_reason TEXT,
    observed_ms  INTEGER NOT NULL,       -- time of the observation behind `state`
    created_ms   INTEGER NOT NULL
);

-- A Baton-made checkout (D1). Attempts that resume a session share it.
CREATE TABLE workspace (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER NOT NULL REFERENCES task(id),
    path        TEXT NOT NULL UNIQUE,
    branch      TEXT NOT NULL,
    base_rev    TEXT NOT NULL,
    state       TEXT NOT NULL,           -- intended → ready (→ removed), or failed
    created_ms  INTEGER NOT NULL
);

-- One execution of a task, with the configuration it was started with.
CREATE TABLE attempt (
    id           INTEGER PRIMARY KEY,
    task_id      INTEGER NOT NULL REFERENCES task(id),
    seq          INTEGER NOT NULL,       -- 1, 2, … within the task
    workspace_id INTEGER NOT NULL REFERENCES workspace(id),
    config       TEXT NOT NULL,          -- JSON: model, role, settings file, policy refs; immutable
    state        TEXT NOT NULL,          -- preparing → dispatching → running ⇄ turn_ended, or failed
    state_reason TEXT,
    observed_ms  INTEGER NOT NULL,
    created_ms   INTEGER NOT NULL,
    ended_ms     INTEGER,
    UNIQUE (task_id, seq)
);

-- A backend's identity for a running conversation. Resume can surface a copy
-- under a new id (compat-record F9, F10); each id gets its own row.
CREATE TABLE session (
    id          INTEGER PRIMARY KEY,
    attempt_id  INTEGER NOT NULL REFERENCES attempt(id),
    backend     TEXT NOT NULL,           -- 'claude' (or 'fake' in tests)
    short_id    TEXT NOT NULL,
    uuid        TEXT,
    name        TEXT,
    origin      TEXT NOT NULL,           -- dispatch | copy
    agent_type  TEXT,                    -- role reported by SessionStart (F6)
    model       TEXT,                    -- reported by SessionStart at startup
    transcript  TEXT,                    -- transcript path reported by hooks
    liveness    TEXT,                    -- last polled status: busy | waiting | idle | not_running | unknown
    waiting_for TEXT,
    observed_ms INTEGER NOT NULL,
    created_ms  INTEGER NOT NULL,
    UNIQUE (backend, short_id)
);

-- The exact revision under review: a snapshot commit of the worktree.
CREATE TABLE candidate (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER NOT NULL REFERENCES task(id),
    attempt_id  INTEGER NOT NULL REFERENCES attempt(id),
    commit_sha  TEXT NOT NULL,
    tree_sha    TEXT NOT NULL,
    base_rev    TEXT NOT NULL,
    branch      TEXT NOT NULL,           -- baton/<task>
    created_ms  INTEGER NOT NULL
);

-- One check run against one candidate. A result never carries over to another candidate.
CREATE TABLE verification (
    id           INTEGER PRIMARY KEY,
    candidate_id INTEGER NOT NULL REFERENCES candidate(id),
    check_cmd    TEXT NOT NULL,
    env          TEXT NOT NULL,          -- JSON: tool and environment versions
    state        TEXT NOT NULL,          -- running | passed | failed | error
    exit_code    INTEGER,
    started_ms   INTEGER NOT NULL,
    ended_ms     INTEGER,
    artifact     TEXT                    -- path of the captured output
);

-- A durable request for the owner: a permission or a review outcome.
CREATE TABLE decision (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER REFERENCES task(id),
    attempt_id  INTEGER REFERENCES attempt(id),
    kind        TEXT NOT NULL,           -- permission | review
    request     TEXT NOT NULL,           -- JSON: the exact request (tool + input, or candidate)
    options     TEXT NOT NULL,           -- JSON array of allowed answers
    status      TEXT NOT NULL,           -- pending | answered | expired | withdrawn
    answer      TEXT,
    note        TEXT,                    -- owner's message, e.g. requested changes
    created_ms  INTEGER NOT NULL,
    answered_ms INTEGER,
    expires_ms  INTEGER
);

-- Sequenced record of significant transitions. Metadata only, so payloads can be
-- deleted without rewriting history (PLAN.md §10).
CREATE TABLE audit (
    seq       INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms     INTEGER NOT NULL,
    entity    TEXT NOT NULL,             -- task | workspace | attempt | session | candidate | verification | decision
    entity_id INTEGER NOT NULL,
    event     TEXT NOT NULL,
    detail    TEXT                       -- JSON
);

CREATE INDEX attempt_task ON attempt (task_id);
CREATE INDEX session_attempt ON session (attempt_id);
CREATE INDEX candidate_task ON candidate (task_id);
CREATE INDEX verification_candidate ON verification (candidate_id);
CREATE INDEX decision_status ON decision (status);
CREATE INDEX audit_entity ON audit (entity, entity_id);
