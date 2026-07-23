use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::{CandidateStatus, PolicyArea, PolicyCandidate, id, now};

#[derive(Clone)]
pub struct PolicyCandidateStore {
    db: Database,
}

impl PolicyCandidateStore {
    pub fn new(db: Database) -> Self {
        PolicyCandidateStore { db }
    }

    pub fn insert(
        &self,
        area: PolicyArea,
        title: &str,
        config_snapshot: &str,
        proposed_value: &str,
        rationale: &str,
        source_run_id: Option<&str>,
    ) -> rusqlite::Result<PolicyCandidate> {
        let conn = self.db.conn()?;
        let pc = PolicyCandidate {
            id: id("pc"),
            area: area.as_str().into(),
            title: title.to_string(),
            config_snapshot: config_snapshot.to_string(),
            proposed_value: proposed_value.to_string(),
            rationale: rationale.to_string(),
            status: CandidateStatus::Proposed.as_str().into(),
            baseline_metric: None,
            canary_metric: None,
            source_run_id: source_run_id.map(|s| s.to_string()),
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO policy_candidates (id, area, title, config_snapshot, proposed_value, rationale, status, baseline_metric, canary_metric, source_run_id, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![pc.id, pc.area, pc.title, pc.config_snapshot, pc.proposed_value, pc.rationale, pc.status, pc.baseline_metric, pc.canary_metric, pc.source_run_id, pc.created_at],
        )?;
        Ok(pc)
    }

    pub fn list(&self, status: Option<&str>) -> rusqlite::Result<Vec<PolicyCandidate>> {
        let conn = self.db.conn()?;
        let (sql, params_vec) = match status {
            Some(s) => (
                "SELECT id, area, title, config_snapshot, proposed_value, rationale, status, baseline_metric, canary_metric, source_run_id, created_at FROM policy_candidates WHERE status = ?1 ORDER BY created_at DESC".to_string(),
                vec![Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT id, area, title, config_snapshot, proposed_value, rationale, status, baseline_metric, canary_metric, source_run_id, created_at FROM policy_candidates ORDER BY created_at DESC".to_string(),
                vec![],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), Self::row)?;
        rows.collect()
    }

    pub fn update_status(&self, id: &str, status: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE policy_candidates SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(n > 0)
    }

    pub fn record_baseline(&self, id: &str, metric: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE policy_candidates SET baseline_metric = ?1 WHERE id = ?2",
            params![metric, id],
        )?;
        Ok(n > 0)
    }

    pub fn record_canary(&self, id: &str, metric: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "UPDATE policy_candidates SET canary_metric = ?1 WHERE id = ?2",
            params![metric, id],
        )?;
        Ok(n > 0)
    }

    pub fn delete(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.db.conn()?;
        let n = conn.execute(
            "DELETE FROM policy_candidates WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    fn row(row: &rusqlite::Row) -> rusqlite::Result<PolicyCandidate> {
        Ok(PolicyCandidate {
            id: row.get(0)?,
            area: row.get(1)?,
            title: row.get(2)?,
            config_snapshot: row.get(3)?,
            proposed_value: row.get(4)?,
            rationale: row.get(5)?,
            status: row.get(6)?,
            baseline_metric: row.get(7)?,
            canary_metric: row.get(8)?,
            source_run_id: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}
