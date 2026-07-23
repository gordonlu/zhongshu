use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::{id, now, CandidateStatus, SkillCandidate};

#[derive(Clone)]
pub struct SkillCandidateStore {
    db: Database,
}

impl SkillCandidateStore {
    pub fn new(db: Database) -> Self {
        SkillCandidateStore { db }
    }

    pub fn insert(
        &self,
        name: &str,
        manifest_json: &str,
        source_runbook_id: Option<&str>,
        source_task_id: Option<&str>,
        run_id: Option<&str>,
    ) -> rusqlite::Result<SkillCandidate> {
        let conn = self.db.conn()?;
        let sc = SkillCandidate {
            id: id("sc"),
            name: name.to_string(),
            manifest_json: manifest_json.to_string(),
            source_runbook_id: source_runbook_id.map(|s| s.to_string()),
            source_task_id: source_task_id.map(|s| s.to_string()),
            run_id: run_id.map(|s| s.to_string()),
            status: CandidateStatus::Proposed.as_str().into(),
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO skill_candidates (id, name, manifest_json, source_runbook_id, source_task_id, run_id, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![sc.id, sc.name, sc.manifest_json, sc.source_runbook_id, sc.source_task_id, sc.run_id, sc.status, sc.created_at],
        )?;
        Ok(sc)
    }

    pub fn list(&self, status: Option<&str>) -> rusqlite::Result<Vec<SkillCandidate>> {
        let conn = self.db.conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
            Some(s) => (
                format!("SELECT id, name, manifest_json, source_runbook_id, source_task_id, run_id, status, created_at FROM skill_candidates WHERE status = ?1 ORDER BY created_at DESC"),
                vec![Box::new(s.to_string())],
            ),
            None => (
                "SELECT id, name, manifest_json, source_runbook_id, source_task_id, run_id, status, created_at FROM skill_candidates ORDER BY created_at DESC".into(),
                vec![],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), Self::row)?;
        rows.collect()
    }

    pub fn list_active(&self) -> rusqlite::Result<Vec<SkillCandidate>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, manifest_json, source_runbook_id, source_task_id, run_id, status, created_at FROM skill_candidates WHERE status IN ('active', 'limited') ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], Self::row)?;
        rows.collect()
    }

    pub fn update_status(&self, id: &str, status: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE skill_candidates SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(n > 0)
    }

    /// Activate a candidate: moves from `approved` or `limited` to `active`.
    /// Returns `Ok(false)` if the current status does not allow activation.
    pub fn activate(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let current: Option<String> = conn
            .query_row(
                "SELECT status FROM skill_candidates WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        match current.as_deref() {
            Some("approved" | "limited") => {
                self.update_status(id, CandidateStatus::Active.as_str())
            }
            _ => Ok(false),
        }
    }

    /// Rollback a previously active candidate: moves from `active` or `limited` to `rolled_back`.
    /// Returns `Ok(false)` if the current status does not allow rollback.
    pub fn rollback(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let current: Option<String> = conn
            .query_row(
                "SELECT status FROM skill_candidates WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        match current.as_deref() {
            Some("active" | "limited") => {
                self.update_status(id, CandidateStatus::RolledBack.as_str())
            }
            _ => Ok(false),
        }
    }

    pub fn delete(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute("DELETE FROM skill_candidates WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<SkillCandidate> {
        Ok(SkillCandidate {
            id: row.get(0)?,
            name: row.get(1)?,
            manifest_json: row.get(2)?,
            source_runbook_id: row.get(3)?,
            source_task_id: row.get(4)?,
            run_id: row.get(5)?,
            status: row.get(6)?,
            created_at: row.get(7)?,
        })
    }
}
