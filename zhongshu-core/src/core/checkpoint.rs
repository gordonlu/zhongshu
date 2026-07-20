use crate::agent::llm::Message;
use crate::core::db::Database;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Durable checkpoint of the agent loop's state.
///
/// Saved to SQLite at each step boundary so a crashed process can restore
/// the exact messages, step counter, and tool-tracking state before
/// continuing execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentCheckpoint {
    pub run_id: String,
    pub step: u32,
    pub tool_calls_made: usize,
    pub consecutive_failures: u32,
    pub tool_call_counts: HashMap<String, u32>,
    pub messages: Vec<Message>,
    pub created_at: u64,
}

/// Manages read/write of checkpoints to the `agent_checkpoints` table.
///
/// Uses a dirty flag to avoid rewriting the same checkpoint on every tool
/// call within the same step.
#[derive(Clone)]
pub struct CheckpointStore {
    db: Database,
    dirty: std::sync::Arc<AtomicBool>,
}

impl CheckpointStore {
    pub fn new(db: Database) -> Self {
        CheckpointStore {
            db,
            dirty: std::sync::Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mark that the state has changed and should be flushed on the next
    /// `save()` call.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Save a checkpoint to the database, but only if dirty or forced.
    pub fn save(&self, checkpoint: &AgentCheckpoint, force: bool) -> rusqlite::Result<()> {
        if !force && !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        let conn = self.db.conn()?;
        let messages_json = serde_json::to_string(&checkpoint.messages)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let tool_call_counts_json = serde_json::to_string(&checkpoint.tool_call_counts)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        conn.execute(
            "INSERT OR REPLACE INTO agent_checkpoints
             (run_id, step, tool_calls_made, consecutive_failures, tool_call_counts, messages, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                checkpoint.run_id,
                checkpoint.step,
                checkpoint.tool_calls_made as u64,
                checkpoint.consecutive_failures,
                tool_call_counts_json,
                messages_json,
                now,
            ],
        )?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }

    /// Load the latest checkpoint for a given run.
    pub fn load_latest(&self, run_id: &str) -> rusqlite::Result<Option<AgentCheckpoint>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT run_id, step, tool_calls_made, consecutive_failures, tool_call_counts, messages, created_at
             FROM agent_checkpoints
             WHERE run_id = ?1
             ORDER BY step DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![run_id])?;
        match rows.next()? {
            Some(row) => {
                let run_id: String = row.get(0)?;
                let step: u32 = row.get(1)?;
                let tool_calls_made: u64 = row.get(2)?;
                let consecutive_failures: u32 = row.get(3)?;
                let tool_call_counts_json: String = row.get(4)?;
                let messages_json: String = row.get(5)?;
                let created_at: u64 = row.get(6)?;

                let tool_call_counts: HashMap<String, u32> =
                    serde_json::from_str(&tool_call_counts_json)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                let messages: Vec<Message> = serde_json::from_str(&messages_json)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

                Ok(Some(AgentCheckpoint {
                    run_id,
                    step,
                    tool_calls_made: tool_calls_made as usize,
                    consecutive_failures,
                    tool_call_counts,
                    messages,
                    created_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Return all run_ids that have checkpoints (i.e., didn't finish cleanly).
    pub fn list_unfinished_runs(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.db.conn()?;
        let mut stmt =
            conn.prepare("SELECT DISTINCT run_id FROM agent_checkpoints ORDER BY run_id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Return the most recently persisted unfinished checkpoint.
    pub fn latest_unfinished(&self) -> rusqlite::Result<Option<AgentCheckpoint>> {
        let conn = self.db.conn()?;
        let run_id = conn
            .query_row(
                "SELECT c.run_id FROM agent_checkpoints AS c
                 WHERE EXISTS (
                     SELECT 1 FROM run_ledger AS l
                     WHERE l.run_id = c.run_id AND l.event_type = 'run_started'
                 )
                   AND NOT EXISTS (
                     SELECT 1 FROM run_ledger AS terminal
                     WHERE terminal.run_id = c.run_id
                       AND terminal.event_type = 'run_finished'
                 )
                 ORDER BY c.created_at DESC, c.rowid DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match run_id {
            Some(run_id) => self.load_latest(&run_id),
            None => Ok(None),
        }
    }

    /// Delete all checkpoints for a given run (used when a run finishes
    /// cleanly).
    pub fn delete_run(&self, run_id: &str) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "DELETE FROM agent_checkpoints WHERE run_id = ?1",
            rusqlite::params![run_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ledger::RunLedger;

    fn checkpoint(run_id: &str) -> AgentCheckpoint {
        AgentCheckpoint {
            run_id: run_id.into(),
            step: 1,
            tool_calls_made: 0,
            consecutive_failures: 0,
            tool_call_counts: HashMap::new(),
            messages: vec![Message::user("resume")],
            created_at: 0,
        }
    }

    #[test]
    fn latest_unfinished_ignores_terminal_runs_with_stale_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("checkpoint.db"));
        db.migrate().unwrap();
        let store = CheckpointStore::new(db.clone());
        let ledger = RunLedger::new(db);

        ledger.record_run_started("finished", "done").unwrap();
        store.save(&checkpoint("finished"), true).unwrap();
        ledger
            .record_run_finished("finished", "Finished", "CompletedVerified")
            .unwrap();

        assert!(store.latest_unfinished().unwrap().is_none());

        ledger.record_run_started("unfinished", "resume").unwrap();
        store.save(&checkpoint("unfinished"), true).unwrap();
        assert_eq!(
            store.latest_unfinished().unwrap().unwrap().run_id,
            "unfinished"
        );
    }
}
