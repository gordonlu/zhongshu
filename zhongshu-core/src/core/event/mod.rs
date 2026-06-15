use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

pub struct EventLogStore {
    db: Database,
}

impl EventLogStore {
    pub fn new(db: Database) -> Self {
        EventLogStore { db }
    }

    pub fn insert(&self, event_type: &str, payload: Option<&str>) -> rusqlite::Result<EventLog> {
        let conn = self.db.conn()?;
        let ev = EventLog {
            id: id("evt"),
            event_type: event_type.to_string(),
            payload: payload.map(|s| s.to_string()),
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO events (id, type, payload, created_at) VALUES (?1,?2,?3,?4)",
            params![ev.id, ev.event_type, ev.payload, ev.created_at],
        )?;
        Ok(ev)
    }

    pub fn recent(&self, limit: i64) -> rusqlite::Result<Vec<EventLog>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, payload, created_at FROM events ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(EventLog {
                id: row.get(0)?,
                event_type: row.get(1)?,
                payload: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }
}
