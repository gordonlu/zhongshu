use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct ObservationStore {
    db: Database,
}

impl ObservationStore {
    pub fn new(db: Database) -> Self {
        ObservationStore { db }
    }

    pub fn insert(&self, type_: ObservationType, content: &str, source: Option<&str>, metadata: Option<&serde_json::Value>) -> rusqlite::Result<Observation> {
        let conn = self.db.conn()?;
        let obs = Observation {
            id: id("obs"),
            type_,
            content: content.to_string(),
            source: source.map(|s| s.to_string()),
            metadata: metadata.cloned(),
            created_at: now(),
            expires_at: Some(now() + 86400 * 3), // 3 days
        };
        conn.execute(
            "INSERT INTO observations (id, type, content, source, metadata, created_at, expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![obs.id, obs.type_.as_str(), obs.content, obs.source, obs.metadata.as_ref().map(|m| m.to_string()), obs.created_at, obs.expires_at],
        )?;
        Ok(obs)
    }

    pub fn recent(&self, limit: i64) -> rusqlite::Result<Vec<Observation>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, source, metadata, created_at, expires_at FROM observations WHERE expires_at > ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now(), limit], Self::row)?;
        rows.collect()
    }

    pub fn since(&self, since: i64) -> rusqlite::Result<Vec<Observation>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, source, metadata, created_at, expires_at FROM observations WHERE created_at > ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![since], Self::row)?;
        rows.collect()
    }

    pub fn by_type(&self, type_: &str, limit: i64) -> rusqlite::Result<Vec<Observation>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, source, metadata, created_at, expires_at FROM observations WHERE type = ?1 AND expires_at > ?2 ORDER BY created_at DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![type_, now(), limit], Self::row)?;
        rows.collect()
    }

    pub fn cleanup_expired(&self) -> rusqlite::Result<usize> {
        let conn = self.db.conn()?;
        conn.execute("DELETE FROM observations WHERE expires_at <= ?1", params![now()])
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<Observation> {
        Ok(Observation {
            id: row.get(0)?,
            type_: ObservationType::from_str(&row.get::<_, String>(1)?).unwrap_or(ObservationType::AgentAction),
            content: row.get(2)?,
            source: row.get(3)?,
            metadata: row.get::<_, Option<String>>(4)?.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: row.get(5)?,
            expires_at: row.get(6)?,
        })
    }
}
