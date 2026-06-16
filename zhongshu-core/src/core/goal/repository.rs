use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct GoalRepository {
    db: Database,
}

impl GoalRepository {
    pub fn new(db: Database) -> Self {
        GoalRepository { db }
    }

    pub fn create(
        &self,
        title: &str,
        description: Option<&str>,
        goal_type: GoalType,
    ) -> rusqlite::Result<Goal> {
        let conn = self.db.conn()?;
        let goal = Goal {
            id: id("goal"),
            title: title.to_string(),
            description: description.map(|s| s.to_string()),
            goal_type,
            status: GoalStatus::Active,
            trigger_config: None,
            metadata: None,
            created_at: now(),
            updated_at: now(),
        };
        conn.execute(
            "INSERT INTO goals (id, title, description, goal_type, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![goal.id, goal.title, goal.description, goal.goal_type.as_str(), goal.status.as_str(), goal.created_at, goal.updated_at],
        )?;
        Ok(goal)
    }

    pub fn list_active(&self) -> rusqlite::Result<Vec<Goal>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, description, goal_type, status, trigger_config, metadata, created_at, updated_at FROM goals WHERE status = 'active' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_goal)?;
        rows.collect()
    }

    pub fn list_all(&self) -> rusqlite::Result<Vec<Goal>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, description, goal_type, status, trigger_config, metadata, created_at, updated_at FROM goals ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_goal)?;
        rows.collect()
    }

    pub fn find_by_title(&self, title: &str) -> rusqlite::Result<Option<Goal>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, description, goal_type, status, trigger_config, metadata, created_at, updated_at \
             FROM goals WHERE title = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![title], Self::row_to_goal)?;
        match rows.next() {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Goal>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, description, goal_type, status, trigger_config, metadata, created_at, updated_at FROM goals WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Self::row_to_goal)?;
        match rows.next() {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    pub fn update_status(&self, id: &str, status: GoalStatus) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE goals SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now(), id],
        )?;
        Ok(n > 0)
    }

    fn row_to_goal(row: &rusqlite::Row) -> rusqlite::Result<Goal> {
        Ok(Goal {
            id: row.get(0)?,
            title: row.get(1)?,
            description: row.get(2)?,
            goal_type: GoalType::from_str(&row.get::<_, String>(3)?).unwrap_or(GoalType::OneShot),
            status: GoalStatus::from_str(&row.get::<_, String>(4)?).unwrap_or(GoalStatus::Active),
            trigger_config: row.get(5)?,
            metadata: row
                .get::<_, Option<String>>(6)?
                .and_then(|s| serde_json::from_str(&s).ok()),
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }
}
