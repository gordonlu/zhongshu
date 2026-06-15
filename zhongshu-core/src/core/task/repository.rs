use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

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
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        conn.execute(
            "INSERT INTO tasks (id, goal_id, title, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task.id, task.goal_id, task.title, task.status.as_str(), task.created_at],
        )?;
        Ok(task)
    }

    pub fn list_by_goal(&self, goal_id: &str) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, created_at, started_at, finished_at \
             FROM tasks WHERE goal_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![goal_id], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_pending(&self) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, created_at, started_at, finished_at \
             FROM tasks WHERE status IN ('pending', 'planning') ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], Self::row_to_task)?;
        rows.collect()
    }

    pub fn list_recent(&self, limit: i64) -> rusqlite::Result<Vec<Task>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, status, input, output, error, created_at, started_at, finished_at \
             FROM tasks ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], Self::row_to_task)?;
        rows.collect()
    }

    pub fn update_status(&self, id: &str, status: TaskStatus) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = match status {
            TaskStatus::Running => conn.execute(
                "UPDATE tasks SET status = ?1, started_at = ?2 WHERE id = ?3",
                params![status.as_str(), now(), id],
            )?,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => conn.execute(
                "UPDATE tasks SET status = ?1, finished_at = ?2 WHERE id = ?3",
                params![status.as_str(), now(), id],
            )?,
            _ => conn.execute(
                "UPDATE tasks SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id],
            )?,
        };
        Ok(n > 0)
    }

    pub fn set_output(&self, id: &str, output: &str, error: Option<&str>) -> rusqlite::Result<bool> {
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
            "SELECT id, goal_id, title, status, input, output, error, created_at, started_at, finished_at \
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
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO task_steps (id, task_id, step_order, action, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![step.id, step.task_id, step.step_order, step.action, step.status.as_str(), step.created_at],
        )?;
        Ok(step)
    }

    pub fn list_steps(&self, task_id: &str) -> rusqlite::Result<Vec<TaskStep>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, step_order, action, status, input, output, created_at \
             FROM task_steps WHERE task_id = ?1 ORDER BY step_order ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(TaskStep {
                id: row.get(0)?,
                task_id: row.get(1)?,
                step_order: row.get(2)?,
                action: row.get(3)?,
                status: StepStatus::from_str(&row.get::<_, String>(4)?).unwrap_or(StepStatus::Pending),
                input: row.get(5)?,
                output: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect()
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
            created_at: row.get(7)?,
            started_at: row.get(8)?,
            finished_at: row.get(9)?,
        })
    }
}
