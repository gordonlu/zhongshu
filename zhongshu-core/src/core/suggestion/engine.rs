use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct SuggestionEngine {
    db: Database,
    min_confidence: f64,
}

impl SuggestionEngine {
    pub fn new(db: Database) -> Self {
        SuggestionEngine {
            db,
            min_confidence: 0.5,
        }
    }

    pub fn insert(
        &self,
        content: &str,
        type_: Option<&str>,
        confidence: f64,
        source_obs: Option<&str>,
    ) -> rusqlite::Result<Suggestion> {
        let conn = self.db.conn()?;
        let sug = Suggestion {
            id: id("sug"),
            type_: type_.map(|s| s.to_string()),
            content: content.to_string(),
            confidence,
            status: SuggestionStatus::Pending,
            source_observation: source_obs.map(|s| s.to_string()),
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO suggestions (id, type, content, confidence, status, source_observation, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![sug.id, sug.type_, sug.content, sug.confidence, sug.status.as_str(), sug.source_observation, sug.created_at],
        )?;
        Ok(sug)
    }

    /// Run the pattern analyzer: scan recent observations and generate suggestions.
    pub fn analyze(&self) -> rusqlite::Result<Vec<Suggestion>> {
        let conn = self.db.conn()?;
        let recent: Vec<Observation> = {
            let mut stmt = conn.prepare(
                "SELECT id, type, content, source, metadata, created_at, expires_at FROM observations WHERE created_at > ?1 ORDER BY created_at DESC LIMIT 50",
            )?;
            let rows = stmt
                .query_map(params![now() - 3600 * 24], |row| {
                    Ok(Observation {
                        id: row.get(0)?,
                        type_: ObservationType::from_str(&row.get::<_, String>(1)?)
                            .unwrap_or(ObservationType::AgentAction),
                        content: row.get(2)?,
                        source: row.get(3)?,
                        metadata: row
                            .get::<_, Option<String>>(4)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        created_at: row.get(5)?,
                        expires_at: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };

        let mut created = Vec::new();
        // Pattern: repeated user messages on the same topic → suggest a goal
        let user_msgs: Vec<&Observation> = recent
            .iter()
            .filter(|o| matches!(o.type_, ObservationType::UserMessage))
            .collect();
        if user_msgs.len() >= 3 {
            let topics: Vec<&str> = user_msgs.iter().map(|o| o.content.as_str()).collect();
            let content = format!("用户近期多次提到相关话题: {}", topics.join(" | "));
            created.push(self.insert(&content, Some("goal"), 0.4, None)?);
        }

        // Pattern: repeated tool failures → suggest investigation
        let failures: Vec<&Observation> = recent
            .iter()
            .filter(|o| matches!(o.type_, ObservationType::ToolResult))
            .collect();
        if !failures.is_empty() {
            created.push(self.insert(
                "检查近期工具执行失败，确认是否需要人工介入",
                Some("investigate"),
                0.3,
                None,
            )?);
        }

        Ok(created)
    }

    pub fn list_pending(&self) -> rusqlite::Result<Vec<Suggestion>> {
        self.list_by_status(&SuggestionStatus::Pending)
    }

    pub fn list_by_status(&self, status: &SuggestionStatus) -> rusqlite::Result<Vec<Suggestion>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, confidence, status, source_observation, created_at FROM suggestions WHERE status = ?1 ORDER BY confidence DESC",
        )?;
        let rows = stmt.query_map(params![status.as_str()], Self::row)?;
        rows.collect()
    }

    pub fn get_content(&self, id: &str) -> rusqlite::Result<String> {
        let conn = self.db.conn()?;
        conn.query_row(
            "SELECT content FROM suggestions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
    }

    pub fn update_status(&self, id: &str, status: &SuggestionStatus) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE suggestions SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        Ok(n > 0)
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<Suggestion> {
        Ok(Suggestion {
            id: row.get(0)?,
            type_: row.get(1)?,
            content: row.get(2)?,
            confidence: row.get(3)?,
            status: SuggestionStatus::from_str(&row.get::<_, String>(4)?)
                .unwrap_or(SuggestionStatus::Pending),
            source_observation: row.get(5)?,
            created_at: row.get(6)?,
        })
    }
}
