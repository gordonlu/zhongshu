use crate::agent::llm::Message;
use crate::agent::{
    ExecutionGraphCheckpoint, ExecutionNodeState, EXECUTION_GRAPH_CHECKPOINT_VERSION,
};
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

/// Manages persistence of organization task checkpoints.
/// Saves the task_id, objective, and staffing request when a task starts.
/// On crash/restart, `list_unfinished()` returns tasks that never finished.
#[derive(Clone)]
pub struct OrganizationCheckpointStore {
    db: Database,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredExecutionGraphCheckpoint {
    pub version: u64,
    pub checkpoint: ExecutionGraphCheckpoint,
    pub updated_at: u64,
}

#[derive(Debug)]
pub enum ExecutionGraphStoreError {
    Database(rusqlite::Error),
    Serialization(serde_json::Error),
    VersionConflict { expected: u64, actual: u64 },
    UnsupportedCheckpointVersion(u32),
    EmptyTaskId,
}

impl std::fmt::Display for ExecutionGraphStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "execution graph database error: {error}"),
            Self::Serialization(error) => {
                write!(formatter, "execution graph serialization error: {error}")
            }
            Self::VersionConflict { expected, actual } => write!(
                formatter,
                "execution graph version conflict: expected {expected}, actual {actual}"
            ),
            Self::UnsupportedCheckpointVersion(version) => {
                write!(
                    formatter,
                    "unsupported execution graph checkpoint version {version}"
                )
            }
            Self::EmptyTaskId => write!(formatter, "execution graph task id is empty"),
        }
    }
}

impl std::error::Error for ExecutionGraphStoreError {}

impl From<rusqlite::Error> for ExecutionGraphStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for ExecutionGraphStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

/// Durable storage boundary for versioned execution-graph checkpoints.
/// `expected_version = 0` creates a new task; subsequent writes must use the
/// version returned by the preceding successful save.
pub trait ExecutionGraphStore {
    fn save_graph_cas(
        &self,
        checkpoint: &ExecutionGraphCheckpoint,
        expected_version: u64,
    ) -> Result<u64, ExecutionGraphStoreError>;

    fn load_graph(
        &self,
        task_id: &str,
    ) -> Result<Option<StoredExecutionGraphCheckpoint>, ExecutionGraphStoreError>;

    fn list_unfinished_graphs(&self) -> Result<Vec<String>, ExecutionGraphStoreError>;

    fn delete_graph(&self, task_id: &str) -> Result<(), ExecutionGraphStoreError>;
}

impl OrganizationCheckpointStore {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Save or update a checkpoint for an organization task.
    pub fn save(
        &self,
        task_id: &str,
        objective: &str,
        staffing_json: &str,
        roster_json: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        conn.execute(
            "INSERT OR REPLACE INTO organization_checkpoints
             (task_id, objective, staffing, roster, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![task_id, objective, staffing_json, roster_json, now],
        )?;
        Ok(())
    }

    /// Delete a checkpoint when a task finishes or is cancelled.
    pub fn delete(&self, task_id: &str) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "DELETE FROM organization_checkpoints WHERE task_id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    }

    /// List all unfinished organization task IDs (those with checkpoints).
    pub fn list_unfinished(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.db.conn()?;
        let mut stmt =
            conn.prepare("SELECT task_id FROM organization_checkpoints ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Return a bounded set of recent execution graphs for the desktop
    /// control plane, including clean terminal graphs for post-run audit.
    pub fn list_recent_graphs(
        &self,
        limit: usize,
    ) -> Result<Vec<String>, ExecutionGraphStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT task_id FROM organization_graph_checkpoints
             ORDER BY updated_at DESC, task_id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit as u64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ExecutionGraphStoreError::Database)
    }
}

impl ExecutionGraphStore for OrganizationCheckpointStore {
    fn save_graph_cas(
        &self,
        checkpoint: &ExecutionGraphCheckpoint,
        expected_version: u64,
    ) -> Result<u64, ExecutionGraphStoreError> {
        let task_id = checkpoint.graph.task_id.trim();
        if task_id.is_empty() {
            return Err(ExecutionGraphStoreError::EmptyTaskId);
        }
        if checkpoint.schema_version != EXECUTION_GRAPH_CHECKPOINT_VERSION {
            return Err(ExecutionGraphStoreError::UnsupportedCheckpointVersion(
                checkpoint.schema_version,
            ));
        }
        let encoded = serde_json::to_string(checkpoint)?;
        let clean_terminal = checkpoint_is_clean_terminal(checkpoint);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let actual = tx
            .query_row(
                "SELECT version FROM organization_graph_checkpoints WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if actual != expected_version {
            return Err(ExecutionGraphStoreError::VersionConflict {
                expected: expected_version,
                actual,
            });
        }
        let next_version = actual + 1;
        tx.execute(
            "INSERT INTO organization_graph_checkpoints
             (task_id, version, checkpoint, clean_terminal, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(task_id) DO UPDATE SET
                version = excluded.version,
                checkpoint = excluded.checkpoint,
                clean_terminal = excluded.clean_terminal,
                updated_at = excluded.updated_at",
            rusqlite::params![task_id, next_version, encoded, clean_terminal as i64, now],
        )?;
        tx.commit()?;
        Ok(next_version)
    }

    fn load_graph(
        &self,
        task_id: &str,
    ) -> Result<Option<StoredExecutionGraphCheckpoint>, ExecutionGraphStoreError> {
        let conn = self.db.conn()?;
        let row = conn
            .query_row(
                "SELECT version, checkpoint, updated_at
                 FROM organization_graph_checkpoints WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(version, encoded, updated_at)| {
            Ok(StoredExecutionGraphCheckpoint {
                version,
                checkpoint: serde_json::from_str(&encoded)?,
                updated_at,
            })
        })
        .transpose()
    }

    fn list_unfinished_graphs(&self) -> Result<Vec<String>, ExecutionGraphStoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT task_id FROM organization_graph_checkpoints
             WHERE clean_terminal = 0 ORDER BY updated_at, task_id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ExecutionGraphStoreError::Database)
    }

    fn delete_graph(&self, task_id: &str) -> Result<(), ExecutionGraphStoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            "DELETE FROM organization_graph_checkpoints WHERE task_id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    }
}

fn checkpoint_is_clean_terminal(checkpoint: &ExecutionGraphCheckpoint) -> bool {
    !checkpoint.graph.nodes.is_empty()
        && checkpoint.graph.nodes.iter().all(|node| {
            node.state.is_terminal() && node.state != ExecutionNodeState::RecoveryRequired
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ExecutionGraph, ExecutionNode, ExecutionNodeKind};
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

    #[test]
    fn organization_checkpoint_save_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("org_checkpoint.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);

        assert!(store.list_unfinished().unwrap().is_empty());

        store
            .save("task-1", "objective 1", r#"{"role":"analyst"}"#, "[]")
            .unwrap();
        store
            .save("task-2", "objective 2", r#"{"role":"writer"}"#, "[]")
            .unwrap();

        let unfinished = store.list_unfinished().unwrap();
        assert_eq!(unfinished.len(), 2);
        assert!(unfinished.contains(&"task-1".to_string()));
        assert!(unfinished.contains(&"task-2".to_string()));

        store.delete("task-1").unwrap();
        let unfinished = store.list_unfinished().unwrap();
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0], "task-2");

        store.delete("task-2").unwrap();
        assert!(store.list_unfinished().unwrap().is_empty());
    }

    #[test]
    fn execution_graph_store_reopens_and_rejects_stale_cas_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph_checkpoint.db");
        let db = Database::new(path.clone());
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let mut graph = ExecutionGraph::new("graph-task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "work",
                ExecutionNodeKind::Work,
                "external work",
            ))
            .unwrap();
        graph.start_node("work").unwrap();
        let running_checkpoint = graph.checkpoint();

        let version_one = store.save_graph_cas(&running_checkpoint, 0).unwrap();
        assert_eq!(version_one, 1);
        let loaded = store.load_graph("graph-task").unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        let (recovered, recovery) =
            ExecutionGraph::recover_from_checkpoint(loaded.checkpoint).unwrap();
        assert_eq!(recovery.recovery_required_nodes, vec!["work"]);
        let version_two = store.save_graph_cas(&recovered.checkpoint(), 1).unwrap();
        assert_eq!(version_two, 2);

        let stale_error = store.save_graph_cas(&running_checkpoint, 1).unwrap_err();
        assert!(matches!(
            stale_error,
            ExecutionGraphStoreError::VersionConflict {
                expected: 1,
                actual: 2
            }
        ));

        let reopened = OrganizationCheckpointStore::new(Database::new(path));
        let loaded = reopened.load_graph("graph-task").unwrap().unwrap();
        assert_eq!(loaded.version, 2);
        assert_eq!(
            loaded.checkpoint.graph.nodes[0].state,
            ExecutionNodeState::RecoveryRequired
        );
        assert_eq!(
            reopened.list_unfinished_graphs().unwrap(),
            vec!["graph-task"]
        );
        reopened.delete_graph("graph-task").unwrap();
        assert!(reopened.load_graph("graph-task").unwrap().is_none());
    }

    #[test]
    fn clean_terminal_graph_is_retained_but_not_listed_unfinished() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("terminal_graph.db"));
        db.migrate().unwrap();
        let store = OrganizationCheckpointStore::new(db);
        let mut graph = ExecutionGraph::new("finished-task").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "finalize",
                ExecutionNodeKind::Finalize,
                "finish",
            ))
            .unwrap();
        graph.start_node("finalize").unwrap();
        graph.complete_node("finalize", Vec::new()).unwrap();

        store.save_graph_cas(&graph.checkpoint(), 0).unwrap();

        assert!(store.list_unfinished_graphs().unwrap().is_empty());
        assert!(store.load_graph("finished-task").unwrap().is_some());
        assert_eq!(store.list_recent_graphs(8).unwrap(), vec!["finished-task"]);
        assert!(store.list_recent_graphs(0).unwrap().is_empty());
    }
}
