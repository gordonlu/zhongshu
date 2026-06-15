use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct MemoryCandidateStore {
    db: Database,
}

impl MemoryCandidateStore {
    pub fn new(db: Database) -> Self {
        MemoryCandidateStore { db }
    }

    pub fn insert(&self, content: &str, memory_type: Option<&str>, confidence: f64, source_type: Option<&str>, source_id: Option<&str>) -> rusqlite::Result<MemoryCandidate> {
        let conn = self.db.conn()?;
        let mc = MemoryCandidate {
            id: id("mc"),
            content: content.to_string(),
            memory_type: memory_type.map(|s| s.to_string()),
            confidence,
            source_type: source_type.map(|s| s.to_string()),
            source_id: source_id.map(|s| s.to_string()),
            status: "pending".into(),
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO memory_candidates (id, content, memory_type, confidence, source_type, source_id, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![mc.id, mc.content, mc.memory_type, mc.confidence, mc.source_type, mc.source_id, mc.status, mc.created_at],
        )?;
        Ok(mc)
    }

    pub fn list_pending(&self) -> rusqlite::Result<Vec<MemoryCandidate>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, content, memory_type, confidence, source_type, source_id, status, created_at FROM memory_candidates WHERE status = 'pending' ORDER BY confidence DESC",
        )?;
        let rows = stmt.query_map([], Self::row)?;
        rows.collect()
    }

    pub fn update_status(&self, id: &str, status: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute("UPDATE memory_candidates SET status = ?1 WHERE id = ?2", params![status, id])?;
        Ok(n > 0)
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<MemoryCandidate> {
        Ok(MemoryCandidate {
            id: row.get(0)?,
            content: row.get(1)?,
            memory_type: row.get(2)?,
            confidence: row.get(3)?,
            source_type: row.get(4)?,
            source_id: row.get(5)?,
            status: row.get(6)?,
            created_at: row.get(7)?,
        })
    }
}
