-- Schema v3: usage per API request (from OpenTelemetry `api_request` events),
-- the latest account quota (from the status line), and the transcript cross-check.

CREATE TABLE usage (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES session(id),
    event_key   TEXT NOT NULL UNIQUE,    -- session + event timestamp + sequence: a replayed export counts once
    model       TEXT,
    input       INTEGER NOT NULL,
    output      INTEGER NOT NULL,
    cache_read  INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    cost_usd    REAL NOT NULL,           -- Claude Code's client-side estimate
    at_ms       INTEGER NOT NULL
);
CREATE INDEX usage_session ON usage (session_id);

-- The account's quota as last seen on any worker's status line. One row.
CREATE TABLE quota (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    five_hour_pct       REAL,
    five_hour_resets_at INTEGER,
    seven_day_pct       REAL,
    seven_day_resets_at INTEGER,
    observed_ms         INTEGER NOT NULL
);

-- Token totals read from the session's transcript at its last Stop (JSON).
ALTER TABLE session ADD COLUMN transcript_usage TEXT;
