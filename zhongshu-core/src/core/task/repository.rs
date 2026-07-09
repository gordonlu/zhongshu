use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

const DEFAULT_MAX_RETRIES: i32 = 3;

#[derive(Clone)]
pub struct TaskRepository {
    db: Database,
}

impl TaskRepository {
    pub fn new(db: Database) -> Self {
        TaskRepository { db }
    }

    pub fn create(&self, goal_id: Option<&str>, title: &str) -> rusqlite::Result<Task> {
        let conn = self.db.conn()?;
        let task = Task {
            id: id("task"),
            goal_id: goal_id.map(|s| s.to_string()),
            title: title.to_string(),
            status: TaskStatus::Pending,
            input: None,
            output: None,
            error: None,
            claimed_by: None,
            claimed_at: None,
            lease_until: None,
            retry_count: 0,
            max_retries: DEFAULT_MAX_RETRIES,
            summary: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        conn.execute(
            "INSERT INTO tasks (id, goal_id, title, status, retry_count, max_retries, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![task.id, task.goal_id, task.title, task.status.as_str(), task.retry_count, task.max_retries, task.created_at],
        )?;
        Ok(task)
    }

    pub fn create_with_max_retries(
        &self,
        goal_id: Option<&str>,
        title: &str,
        max_retries: i32,
    ) -> rusqlite::Result<Task> {
        let conn = self.db.conn()?;
        let task = Task {
            id: id("task"),
            goal_id: goal_id.map(|s| s.to_string()),
            title: title.to_string(),
            status: TaskStatus::Pending,
            input: None,
            output: None,
            error: None,
            claimed_by: None,
            claimed_at: None,
            lease_until: None,
            retry_count: 0,
            max_retries,
            summary: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        conn.execute(
            "INSERT INTO tasks (id, goal_id, title, status, retry_count, max_retries, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![task.id, task.goal_id, task.title, task.status.as_str(), task.retry_count, task.max_retries, task.created_at],
        )?;
        Ok(task)
    }

    /// Atomically claim a task by ID. Only succeeds for non-terminal tasks.
    pub fn claim_task(&self, id: &str, worker_id: &str, lease_secs: i64) -> rusqlite::Result<ClaimResult> {
        let conn = self.db.conn()?;
        let now_ts = now();
        let lease_end = now_ts + lease_secs;

        // Peek current state first
        let current: Option<Task> = self.get(id)?;
        let task = match current {
            Some(t) => t,
            None => return Ok(ClaimResult::NotFound),
        };
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) {
            return Ok(ClaimResult::NotClaimable { status: task.status });
        }
        if task.retry_count >= task.max_retries && task.max_retries > 0 {
            return Ok(ClaimResult::RetriesExhausted { retry_count: task.retry_count });
        }
        if let Some(ref w) = task.claimed_by {
            if w == worker_id {
                // Already claimed by this worker — extend lease
                let _ = conn.execute(
                    "UPDATE tasks SET lease_until = ?1 WHERE id = ?2 AND claimed_by = ?3",
                    params![lease_end, id, worker_id],
                )?;
            }
        }

        // Atomic claim: only if not terminal, and (unclaimed OR lease expired)
        let n = conn.execute(
            "UPDATE tasks SET status = ?1, claimed_by = ?2, claimed_at = ?3, \
             lease_until = ?4, retry_count = retry_count + 1 \
             WHERE id = ?5 AND status NOT IN ('completed', 'failed', 'cancelled') \
             AND (claimed_by IS NULL OR lease_until < ?6)",
            params![TaskStatus::Running.as_str(), worker_id, now_ts, lease_end, id, now_ts],
        )?;
        if n == 0 {
            // Re-read to give precise reason
            let t = self.get(id)?;
            return match t {
                Some(t) if matches!(t.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) =>
                    Ok(ClaimResult::NotClaimable { status: t.status }),
                Some(t) if t.retry_count >= t.max_retries =>
                    Ok(ClaimResult::RetriesExhausted { retry_count: t.retry_count }),
                Some(t) if t.claimed_by.is_some() && t.claimed_by.as_deref() != Some(worker_id) =>
                    Ok(ClaimResult::AlreadyClaimed { worker_id: t.claimed_by.unwrap_or_default() }),
                _ => Ok(ClaimResult::NotFound),
            };
        }
        self.get(id).map(|t| ClaimResult::Claimed(t.unwrap()))
    }

    pub fn renew_lease(&self, id: &str, worker_id: &str, lease_secs: i64) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let lease_end = now() + lease_secs;
        let n = conn.execute(
            "UPDATE tasks SET lease_until = ?1 WHERE id = ?2 AND claimed_by = ?3",
            params![lease_end, id, worker_id],
        )?;
        Ok(n > 0)
    }

    pub fn release_claim(&self, id: &str, worker_id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE tasks SET claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
             WHERE id = ?1 AND claimed_by = ?2",
            params![id, worker_id],
        )?;
        Ok(n > 0)
    }

    /// Mark a task as permanently failed. Includes terminal-state guard.
    pub fn mark_failed(&self, id: &str, output: &str, error: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE tasks SET status = ?1, output = ?2, error = ?3, finished_at = ?4, \
             claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
             WHERE id = ?5 AND status NOT IN ('completed', 'failed', 'cancelled')",
            params![TaskStatus::Failed.as_str(), output, error, now(), id],
        )?;
        Ok(n > 0)
    }

    /// Mark a task as completed with terminal-state guard.
    pub fn mark_completed(&self, id: &str, output: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE tasks SET status = ?1, output = ?2, error = NULL, finished_at = ?3, \
             claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
             WHERE id = ?4 AND status NOT IN ('cancelled')",
            params![TaskStatus::Completed.as_str(), output, now(), id],
        )?;
        Ok(n > 0)
    }

    /// Mark a task as cancelled with terminal-state guard.
    /// Preserves existing output when called with empty output.
    pub fn mark_cancelled(&self, id: &str, output: &str, reason: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        if output.is_empty() {
            // Preserve existing output
            let n = conn.execute(
                "UPDATE tasks SET status = ?1, error = ?2, finished_at = ?3, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
                 WHERE id = ?4 AND status NOT IN ('completed', 'failed', 'cancelled')",
                params![TaskStatus::Cancelled.as_str(), reason, now(), id],
            )?;
            Ok(n > 0)
        } else {
            let n = conn.execute(
                "UPDATE tasks SET status = ?1, output = ?2, error = ?3, finished_at = ?4, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
                 WHERE id = ?5 AND status NOT IN ('completed', 'failed', 'cancelled')",
                params![TaskStatus::Cancelled.as_str(), output, reason, now(), id],
            )?;
            Ok(n > 0)
        }
    }

    /// Record a failure with retry logic. If retries remain, requeues to Pending.
    /// If exhausted, marks Failed permanently.
    pub fn record_failure(&self, id: &str, _worker_id: &str, error: &str) -> rusqlite::Result<RetryOutcome> {
        let task = match self.get(id)? {
            Some(t) => t,
            None => return Ok(RetryOutcome::NotFound),
        };
        let conn = self.db.conn()?;
        let now_ts = now();

        if task.status == TaskStatus::Cancelled {
            return Ok(RetryOutcome::PermanentlyFailed);
        }

        // Increment retry_count and check if exhausted
        let new_retry_count = task.retry_count + 1;
        if new_retry_count >= task.max_retries {
            conn.execute(
                "UPDATE tasks SET status = ?1, error = ?2, finished_at = ?3, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL, \
                 retry_count = ?4 \
                 WHERE id = ?5",
                params![TaskStatus::Failed.as_str(), error, now_ts, new_retry_count, id],
            )?;
            Ok(RetryOutcome::PermanentlyFailed)
        } else {
            // Requeue: clear all intermediate state
            conn.execute(
                "UPDATE tasks SET status = ?1, error = ?2, claimed_by = NULL, claimed_at = NULL, \
                 lease_until = NULL, retry_count = ?3, input = NULL, output = NULL, summary = NULL \
                 WHERE id = ?4",
                params![TaskStatus::Pending.as_str(), error, new_retry_count, id],
            )?;
            // Also clear all task_steps intermediate state
            let _ = conn.execute(
                "UPDATE task_steps SET status = 'pending', input = NULL, output = NULL, \
                 error = NULL, tool_summary = NULL, verification = NULL, \
                 started_at = NULL, finished_at = NULL \
                 WHERE task_id = ?1",
                params![id],
            )?;
            Ok(RetryOutcome::Scheduled)
        }
    }

    /// Schedule a retry for a failed/cancelled task. Clears all intermediate state.
    pub fn schedule_retry(&self, id: &str) -> rusqlite::Result<ScheduleRetryResult> {
        let task = match self.get(id)? {
            Some(t) => t,
            None => return Ok(ScheduleRetryResult::NotFound),
        };
        if task.status == TaskStatus::Cancelled {
            return Ok(ScheduleRetryResult::NotRetriable { reason: "task was cancelled".into() });
        }
        if task.retry_count >= task.max_retries {
            return Ok(ScheduleRetryResult::RetriesExhausted {
                retry_count: task.retry_count,
                max_retries: task.max_retries,
            });
        }
        let conn = self.db.conn()?;
        conn.execute(
            "UPDATE tasks SET status = ?1, error = NULL, finished_at = NULL, \
             claimed_by = NULL, claimed_at = NULL, lease_until = NULL, \
             input = NULL, output = NULL, summary = NULL \
             WHERE id = ?2",
            params![TaskStatus::Pending.as_str(), id],
        )?;
        let _ = conn.execute(
            "UPDATE task_steps SET status = 'pending', input = NULL, output = NULL, \
             error = NULL, tool_summary = NULL, verification = NULL, \
             started_at = NULL, finished_at = NULL \
             WHERE task_id = ?1",
            params![id],
        )?;
        Ok(ScheduleRetryResult::Scheduled)
    }

    pub fn set_summary(&self, id: &str, summary: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE tasks SET summary = ?1 WHERE id = ?2",
            params![summary, id],
        )?;
        Ok(n > 0)
    }

    /// Find stale in-flight tasks running/planning/waiting_approval older than threshold.
    pub fn list_stale_inflight(&self, older_than_secs: i64) -> rusqlite::Result<Vec<Task>> {
        let cutoff = now() - older_than_secs;
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks \
             WHERE status IN ('running', 'planning', 'waiting_approval') \
             AND COALESCE(started_at, created_at) <= ?1",
        )?;
        let rows = stmt.query_map(params![cutoff], Self::row_to_task)?;
        rows.collect()
    }

    /// Recover stale in-flight tasks: mark them failed and return recovered tasks.
    pub fn recover_stale_inflight(&self, older_than_secs: i64) -> rusqlite::Result<Vec<Task>> {
        let stale = self.list_stale_inflight(older_than_secs)?;
        let mut recovered = Vec::new();
        for task in &stale {
            if self.mark_failed(
                &task.id,
                "",
                &format!("task interrupted: stale while in '{}' state", task.status.as_str()),
            )? {
                recovered.push(task.clone());
            }
        }
        Ok(recovered)
    }

    pub fn list_by_goal(&self, goal_id: &str) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks WHERE goal_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![goal_id], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_pending(&self) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks WHERE status IN ('pending', 'planning') ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_open(&self) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks WHERE status IN ('pending', 'planning', 'running', 'waiting_approval') ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_stale_pending(&self, older_than_secs: i64) -> rusqlite::Result<Vec<Task>> {
        let cutoff = now() - older_than_secs;
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks WHERE status IN ('pending', 'planning') AND created_at <= ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![cutoff], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_recent(&self, limit: i64) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], Self::row_to_task)?;
        rows.collect()
    }

    /// Update status with terminal-state guards on relevant arms.
    pub fn update_status(&self, id: &str, status: TaskStatus) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = match status {
            TaskStatus::Running => conn.execute(
                "UPDATE tasks SET status = ?1, started_at = ?2 \
                 WHERE id = ?3 AND status NOT IN ('completed', 'failed', 'cancelled')",
                params![status.as_str(), now(), id],
            )?,
            TaskStatus::Completed => conn.execute(
                "UPDATE tasks SET status = ?1, finished_at = ?2, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
                 WHERE id = ?3 AND status NOT IN ('cancelled')",
                params![status.as_str(), now(), id],
            )?,
            TaskStatus::Failed => conn.execute(
                "UPDATE tasks SET status = ?1, finished_at = ?2, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
                 WHERE id = ?3 AND status NOT IN ('completed', 'failed', 'cancelled')",
                params![status.as_str(), now(), id],
            )?,
            TaskStatus::Cancelled => conn.execute(
                "UPDATE tasks SET status = ?1, finished_at = ?2, \
                 claimed_by = NULL, claimed_at = NULL, lease_until = NULL \
                 WHERE id = ?3 AND status NOT IN ('completed', 'failed', 'cancelled')",
                params![status.as_str(), now(), id],
            )?,
            _ => conn.execute(
                "UPDATE tasks SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id],
            )?,
        };
        Ok(n > 0)
    }

    pub fn set_output(
        &self,
        id: &str,
        output: &str,
        error: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE tasks SET output = ?1, error = ?2 WHERE id = ?3",
            params![output, error, id],
        )?;
        Ok(n > 0)
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, \
             claimed_by, claimed_at, lease_until, retry_count, max_retries, summary, \
             created_at, started_at, finished_at \
             FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Self::row_to_task)?;
        match rows.next() {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    pub fn add_step(&self, task_id: &str, order: i32, action: &str) -> rusqlite::Result<TaskStep> {
        let conn = self.db.conn()?;
        let step = TaskStep {
            id: id("step"),
            task_id: task_id.to_string(),
            step_order: order,
            action: action.to_string(),
            status: StepStatus::Pending,
            input: None,
            output: None,
            error: None,
            tool_summary: None,
            verification: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        conn.execute(
            "INSERT INTO task_steps (id, task_id, step_order, action, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                step.id,
                step.task_id,
                step.step_order,
                step.action,
                step.status.as_str(),
                step.created_at
            ],
        )?;
        Ok(step)
    }

    pub fn list_steps(&self, task_id: &str) -> rusqlite::Result<Vec<TaskStep>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, step_order, action, status, input, output, \
             error, tool_summary, verification, created_at, started_at, finished_at \
             FROM task_steps WHERE task_id = ?1 ORDER BY step_order ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(TaskStep {
                id: row.get(0)?,
                task_id: row.get(1)?,
                step_order: row.get(2)?,
                action: row.get(3)?,
                status: StepStatus::from_str(&row.get::<_, String>(4)?)
                    .unwrap_or(StepStatus::Pending),
                input: row.get(5)?,
                output: row.get(6)?,
                error: row.get(7)?,
                tool_summary: row.get(8)?,
                verification: row.get(9)?,
                created_at: row.get(10)?,
                started_at: row.get(11)?,
                finished_at: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_step_status(&self, id: &str, status: StepStatus) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = match status {
            StepStatus::Running => conn.execute(
                "UPDATE task_steps SET status = ?1, started_at = ?2 WHERE id = ?3",
                params![status.as_str(), now(), id],
            )?,
            StepStatus::Completed | StepStatus::Failed => conn.execute(
                "UPDATE task_steps SET status = ?1, finished_at = ?2 WHERE id = ?3",
                params![status.as_str(), now(), id],
            )?,
            _ => conn.execute(
                "UPDATE task_steps SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id],
            )?,
        };
        Ok(n > 0)
    }

    pub fn set_step_output(&self, id: &str, output: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE task_steps SET output = ?1 WHERE id = ?2",
            params![output, id],
        )?;
        Ok(n > 0)
    }

    pub fn set_step_input(&self, id: &str, input: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE task_steps SET input = ?1 WHERE id = ?2",
            params![input, id],
        )?;
        Ok(n > 0)
    }

    pub fn set_step_error(&self, id: &str, error: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE task_steps SET error = ?1, status = ?2, finished_at = ?3 WHERE id = ?4",
            params![error, StepStatus::Failed.as_str(), now(), id],
        )?;
        Ok(n > 0)
    }

    pub fn set_step_tool_summary(&self, id: &str, summary: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE task_steps SET tool_summary = ?1 WHERE id = ?2",
            params![summary, id],
        )?;
        Ok(n > 0)
    }

    pub fn set_step_verification(&self, id: &str, result: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE task_steps SET verification = ?1 WHERE id = ?2",
            params![result, id],
        )?;
        Ok(n > 0)
    }

    fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            goal_id: row.get(1)?,
            title: row.get(2)?,
            status: TaskStatus::from_str(&row.get::<_, String>(3)?).unwrap_or(TaskStatus::Pending),
            input: row.get(4)?,
            output: row.get(5)?,
            error: row.get(6)?,
            claimed_by: row.get(7)?,
            claimed_at: row.get(8)?,
            lease_until: row.get(9)?,
            retry_count: row.get(10)?,
            max_retries: row.get(11)?,
            summary: row.get(12)?,
            created_at: row.get(13)?,
            started_at: row.get(14)?,
            finished_at: row.get(15)?,
        })
    }
}
