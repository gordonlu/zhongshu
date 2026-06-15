use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct MemoryPolicy {
    db: Database,
    min_confidence: f64,
}

impl MemoryPolicy {
    pub fn new(db: Database) -> Self {
        MemoryPolicy { db, min_confidence: 0.7 }
    }

    pub fn set_min_confidence(&mut self, v: f64) { self.min_confidence = v; }

    /// Evaluate pending candidates. Those above the confidence threshold
    /// are promoted to memories; others stay pending.
    pub fn evaluate(&self) -> rusqlite::Result<Vec<Memory>> {
        let mut accepted = Vec::new();
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, content, memory_type, confidence, source_type, source_id, status, created_at \
             FROM memory_candidates WHERE status = 'pending' AND confidence >= ?1 ORDER BY confidence DESC",
        )?;
        let candidates: Vec<MemoryCandidate> = stmt.query_map(params![self.min_confidence], |row| {
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
        })?.collect::<Result<Vec<_>, _>>()?;

        for c in &candidates {
            let memory_type = c.memory_type.as_deref().and_then(MemoryType::from_str).unwrap_or(MemoryType::Preference);
            let mem = Memory {
                id: id("mem"),
                memory_type,
                content: c.content.clone(),
                embedding: None,
                created_at: now(),
                updated_at: now(),
            };
            conn.execute(
                "INSERT INTO memories (id, type, content, created_at, updated_at) VALUES (?1,?2,?3,?4,?5)",
                params![mem.id, mem.memory_type.as_str(), mem.content, mem.created_at, mem.updated_at],
            )?;
            conn.execute("UPDATE memory_candidates SET status = 'accepted' WHERE id = ?1", params![c.id])?;
            accepted.push(mem);
        }
        Ok(accepted)
    }

    pub fn list_memories(&self, limit: i64) -> rusqlite::Result<Vec<Memory>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, created_at, updated_at FROM memories ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(Memory {
                id: row.get(0)?,
                memory_type: MemoryType::from_str(&row.get::<_, String>(1)?).unwrap_or(MemoryType::Preference),
                content: row.get(2)?,
                embedding: None,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn search(&self, keyword: &str, limit: i64) -> rusqlite::Result<Vec<Memory>> {
        let conn = self.db.conn()?;
        let pattern = format!("%{}%", keyword);
        let mut stmt = conn.prepare(
            "SELECT id, type, content, created_at, updated_at FROM memories WHERE content LIKE ?1 ORDER BY updated_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| {
            Ok(Memory {
                id: row.get(0)?,
                memory_type: MemoryType::from_str(&row.get::<_, String>(1)?).unwrap_or(MemoryType::Preference),
                content: row.get(2)?,
                embedding: None,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}
