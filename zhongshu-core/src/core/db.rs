use std::path::PathBuf;

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Database { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn conn(&self) -> rusqlite::Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(conn)
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn()?;

        // Phase 4 migration: add columns to existing task_steps table
        for sql in &[
            "ALTER TABLE task_steps ADD COLUMN error TEXT",
            "ALTER TABLE task_steps ADD COLUMN tool_summary TEXT",
            "ALTER TABLE task_steps ADD COLUMN verification TEXT",
            "ALTER TABLE task_steps ADD COLUMN started_at INTEGER",
            "ALTER TABLE task_steps ADD COLUMN finished_at INTEGER",
        ] {
            let _ = conn.execute_batch(sql);
        }

        // Phase 6 migration: idempotency_key for tool dedup across restarts
        let _ = conn.execute_batch(
            "ALTER TABLE run_ledger ADD COLUMN idempotency_key TEXT;
             CREATE INDEX IF NOT EXISTS idx_run_ledger_idempotency
                 ON run_ledger(run_id, idempotency_key)
                 WHERE idempotency_key IS NOT NULL;",
        );

        // Phase 5 migration: claim/lease/retry/summary columns
        for sql in &[
            "ALTER TABLE tasks ADD COLUMN claimed_by TEXT",
            "ALTER TABLE tasks ADD COLUMN claimed_at INTEGER",
            "ALTER TABLE tasks ADD COLUMN lease_until INTEGER",
            "ALTER TABLE tasks ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE tasks ADD COLUMN max_retries INTEGER NOT NULL DEFAULT 3",
            "ALTER TABLE tasks ADD COLUMN summary TEXT",
        ] {
            let _ = conn.execute_batch(sql);
        }

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS run_ledger (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id          TEXT NOT NULL,
                event_type      TEXT NOT NULL,
                payload         TEXT NOT NULL,
                created_at      INTEGER NOT NULL,
                idempotency_key TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_run_ledger_run_id ON run_ledger(run_id);
            CREATE INDEX IF NOT EXISTS idx_run_ledger_idempotency
                ON run_ledger(run_id, idempotency_key)
                WHERE idempotency_key IS NOT NULL;

            CREATE TABLE IF NOT EXISTS observations (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                content     TEXT NOT NULL,
                source      TEXT,
                metadata    TEXT,
                created_at  INTEGER NOT NULL,
                expires_at  INTEGER
            );

            CREATE TABLE IF NOT EXISTS suggestions (
                id                  TEXT PRIMARY KEY,
                type                TEXT,
                content             TEXT NOT NULL,
                confidence          REAL NOT NULL DEFAULT 0.0,
                status              TEXT NOT NULL DEFAULT 'pending',
                source_observation  TEXT,
                created_at          INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS goals (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL,
                description     TEXT,
                goal_type       TEXT NOT NULL DEFAULT 'one_shot',
                status          TEXT NOT NULL DEFAULT 'active',
                trigger_config  TEXT,
                metadata        TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                id              TEXT PRIMARY KEY,
                goal_id         TEXT REFERENCES goals(id),
                title           TEXT NOT NULL,
                status          TEXT NOT NULL DEFAULT 'pending',
                input           TEXT,
                output          TEXT,
                error           TEXT,
                claimed_by      TEXT,
                claimed_at      INTEGER,
                lease_until     INTEGER,
                retry_count     INTEGER NOT NULL DEFAULT 0,
                max_retries     INTEGER NOT NULL DEFAULT 3,
                summary         TEXT,
                created_at      INTEGER NOT NULL,
                started_at      INTEGER,
                finished_at     INTEGER
            );

            CREATE TABLE IF NOT EXISTS task_steps (
                id          TEXT PRIMARY KEY,
                task_id     TEXT NOT NULL REFERENCES tasks(id),
                step_order  INTEGER NOT NULL,
                action      TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'pending',
                input       TEXT,
                output      TEXT,
                error       TEXT,
                tool_summary TEXT,
                verification TEXT,
                created_at  INTEGER NOT NULL,
                started_at  INTEGER,
                finished_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS task_runs (
                id          TEXT PRIMARY KEY,
                task_id     TEXT NOT NULL REFERENCES tasks(id),
                context     TEXT,
                tool_calls  TEXT,
                started_at  INTEGER NOT NULL,
                finished_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS artifacts (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                title       TEXT,
                uri         TEXT,
                summary     TEXT,
                metadata    TEXT,
                created_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS task_artifacts (
                task_id     TEXT NOT NULL REFERENCES tasks(id),
                artifact_id TEXT NOT NULL REFERENCES artifacts(id),
                relation    TEXT NOT NULL,
                PRIMARY KEY (task_id, artifact_id, relation)
            );

            CREATE TABLE IF NOT EXISTS memory_candidates (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                memory_type TEXT,
                confidence  REAL NOT NULL DEFAULT 0.0,
                source_type TEXT,
                source_id   TEXT,
                status      TEXT NOT NULL DEFAULT 'pending',
                created_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                content     TEXT NOT NULL,
                embedding   BLOB,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS events (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                payload     TEXT,
                created_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS runbooks (
                id TEXT PRIMARY KEY,
                goal TEXT NOT NULL,
                conversation_id INTEGER,
                created_at TEXT NOT NULL,
                total_steps INTEGER NOT NULL DEFAULT 0,
                passed INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS runbook_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                runbook_id TEXT NOT NULL REFERENCES runbooks(id),
                seq INTEGER NOT NULL,
                action TEXT NOT NULL,
                tool TEXT NOT NULL,
                input TEXT NOT NULL DEFAULT '',
                output_status TEXT NOT NULL DEFAULT '',
                output_preview TEXT NOT NULL DEFAULT '',
                verification TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS agent_checkpoints (
                run_id              TEXT NOT NULL,
                step                INTEGER NOT NULL,
                tool_calls_made     INTEGER NOT NULL DEFAULT 0,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                tool_call_counts    TEXT NOT NULL DEFAULT '{}',
                messages            TEXT NOT NULL,
                created_at          INTEGER NOT NULL,
                PRIMARY KEY (run_id, step)
            );
            ",
        )?;

        tracing::info!("core database migrated at {}", self.path.display());
        Ok(())
    }
}
