// ── Append-only run ledger ─────────────────────────────────────────────
//
// Records run lifecycle, tool executions, and approval decisions to SQLite.
// The ledger is append-only — no UPDATE or DELETE — so it can be used to
// reconstruct the last known state after a restart.

use rusqlite::params;
use serde_json::json;

use crate::core::db::Database;
use crate::core::models::now;

#[derive(Clone)]
pub struct RunLedger {
    db: Database,
}

impl RunLedger {
    pub fn new(db: Database) -> Self {
        RunLedger { db }
    }

    fn insert(
        &self,
        run_id: &str,
        event_type: &str,
        payload: &str,
        idempotency_key: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT INTO run_ledger (run_id, event_type, payload, created_at, idempotency_key)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, event_type, payload, now(), idempotency_key],
        )?;
        Ok(())
    }

    fn insert_event(
        &self,
        run_id: &str,
        event_type: &str,
        payload: &str,
    ) -> rusqlite::Result<()> {
        self.insert(run_id, event_type, payload, None)
    }

    // ── Lifecycle ──────────────────────────────────────────────────────

    pub fn record_run_started(&self, run_id: &str, goal: &str) -> rusqlite::Result<()> {
        self.insert_event(
            run_id,
            "run_started",
            &json!({"goal": goal}).to_string(),
        )
    }

    pub fn record_run_interrupted(&self, run_id: &str, reason: &str) -> rusqlite::Result<()> {
        self.insert_event(
            run_id,
            "run_interrupted",
            &json!({"reason": reason}).to_string(),
        )
    }

    pub fn record_run_resumed(&self, run_id: &str) -> rusqlite::Result<()> {
        self.insert_event(run_id, "run_resumed", "{}")
    }

    pub fn record_run_finished(
        &self,
        run_id: &str,
        stop_reason: &str,
        outcome: &str,
    ) -> rusqlite::Result<()> {
        self.insert_event(
            run_id,
            "run_finished",
            &json!({"stop_reason": stop_reason, "outcome": outcome}).to_string(),
        )
    }

    // ── Tool calls ─────────────────────────────────────────────────────

    pub fn record_tool_call(
        &self,
        run_id: &str,
        tool_name: &str,
        args: &str,
        status: &str,
        error: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.insert(
            run_id,
            "tool_call",
            &json!({
                "tool": tool_name,
                "args": args,
                "status": status,
                "error": error,
            })
            .to_string(),
            idempotency_key,
        )
    }

    /// Check if a tool call with the given idempotency key was already
    /// recorded as completed in the current run. This survives restarts
    /// because the key is deterministic across processes.
    pub fn is_tool_completed(&self, run_id: &str, idempotency_key: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM run_ledger
             WHERE run_id = ?1
               AND idempotency_key = ?2
               AND event_type = 'tool_call'
               AND json_extract(payload, '$.status') = 'completed'",
        )?;
        let count: i64 = stmt.query_row(params![run_id, idempotency_key], |row| row.get(0))?;
        Ok(count > 0)
    }

    // ── Approval ───────────────────────────────────────────────────────

    pub fn record_approval(
        &self,
        run_id: &str,
        tool: &str,
        decision: &str, // "approved" | "denied" | "cancelled"
    ) -> rusqlite::Result<()> {
        self.insert_event(
            run_id,
            "approval",
            &json!({"tool": tool, "decision": decision}).to_string(),
        )
    }

    // ── Checkpoints ────────────────────────────────────────────────────

    pub fn record_checkpoint(
        &self,
        run_id: &str,
        step: u32,
        tool_calls_made: usize,
    ) -> rusqlite::Result<()> {
        self.insert_event(
            run_id,
            "checkpoint",
            &json!({
                "step": step,
                "tool_calls_made": tool_calls_made,
            })
            .to_string(),
        )
    }

    // ── State reconstruction ───────────────────────────────────────────

    /// Returns true if the run has any recorded events.
    pub fn has_run(&self, run_id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM run_ledger WHERE run_id = ?1",
        )?;
        let count: i64 = stmt.query_row(params![run_id], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Returns the last known state for a run, if any.
    /// Find all tool calls that were recorded as 'started' but never completed
    /// or failed. Returns a list of `(tool_name, args, idempotency_key)` for
    /// each in-flight call.
    ///
    /// After a crash/restart these tools have unknown outcome — the process
    /// cannot know whether the side effect actually happened. The caller
    /// should NOT skip these via the idempotency checker; instead the agent
    /// should be informed that the outcome is unknown.
    pub fn reconcile_inflight_tools(&self, run_id: &str) -> rusqlite::Result<Vec<(String, String, String)>> {
        let conn = self.db.conn()?;
        // Find all 'started' events that do NOT have a matching
        // 'completed' or 'failed' event with the same idempotency_key.
        let mut stmt = conn.prepare(
            "SELECT json_extract(payload, '$.tool'),
                    json_extract(payload, '$.args'),
                    idempotency_key
             FROM run_ledger
             WHERE run_id = ?1
               AND event_type = 'tool_call'
               AND json_extract(payload, '$.status') = 'started'
               AND idempotency_key IS NOT NULL
               AND idempotency_key NOT IN (
                   SELECT idempotency_key FROM run_ledger
                   WHERE run_id = ?1
                     AND event_type = 'tool_call'
                     AND json_extract(payload, '$.status') IN ('completed', 'failed')
                     AND idempotency_key IS NOT NULL
               )",
        )?;
        let mut rows = stmt.query(rusqlite::params![run_id])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let tool: Option<String> = row.get(0)?;
            let args: Option<String> = row.get(1)?;
            let idem_key: Option<String> = row.get(2)?;
            if let (Some(tool), Some(args), Some(idem_key)) = (tool, args, idem_key) {
                result.push((tool, args, idem_key));
            }
        }
        Ok(result)
    }

    /// Returns true if the run has any tool calls in 'started' state that
    /// were never completed — indicating a crash occurred during tool
    /// execution.
    pub fn has_inflight_tools(&self, run_id: &str) -> rusqlite::Result<bool> {
        Ok(!self.reconcile_inflight_tools(run_id)?.is_empty())
    }

    pub fn last_state(&self, run_id: &str) -> rusqlite::Result<Option<String>> {
        // Get the most recent lifecycle event by actual time order,
        // not by event-type priority.
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT payload FROM run_ledger
             WHERE run_id = ?1
               AND event_type IN ('run_finished', 'run_interrupted', 'run_resumed', 'run_started')
             ORDER BY id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }
}
