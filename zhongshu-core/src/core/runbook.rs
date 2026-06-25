use crate::core::db::Database;
use rusqlite::params;

/// Phase 6D: Record of a completed automated task — auditable, reusable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Runbook {
    pub id: String,
    pub goal: String,
    pub steps: Vec<RunbookStep>,
    pub created_at: String,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunbookStep {
    pub action: String,
    pub tool: String,
    pub input: String,
    pub output_status: String,
    pub output_preview: String,
    pub verification: String,
}

pub struct RunbookStore {
    db: Database,
}

impl RunbookStore {
    pub fn new(db: Database) -> Self {
        RunbookStore { db }
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runbooks (
                id TEXT PRIMARY KEY,
                goal TEXT NOT NULL,
                created_at TEXT NOT NULL,
                total_steps INTEGER NOT NULL DEFAULT 0,
                passed INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS runbook_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                runbook_id TEXT NOT NULL REFERENCES runbooks(id),
                seq INTEGER NOT NULL,
                action TEXT NOT NULL,
                tool TEXT NOT NULL,
                input TEXT NOT NULL DEFAULT '',
                output_status TEXT NOT NULL DEFAULT '',
                output_preview TEXT NOT NULL DEFAULT '',
                verification TEXT NOT NULL DEFAULT ''
            );",
        )?;
        Ok(())
    }

    pub fn save(&self, rb: &Runbook) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO runbooks (id, goal, created_at, total_steps, passed, failed) VALUES (?1,?2,?3,?4,?5,?6)",
            params![rb.id, rb.goal, rb.created_at, rb.total_steps, rb.passed, rb.failed],
        )?;
        for (i, step) in rb.steps.iter().enumerate() {
            conn.execute(
                "INSERT INTO runbook_steps (runbook_id, seq, action, tool, input, output_status, output_preview, verification) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![rb.id, i as i64, step.action, step.tool, step.input, step.output_status, step.output_preview, step.verification],
            )?;
        }
        Ok(())
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Runbook>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare("SELECT id, goal, created_at, total_steps, passed, failed FROM runbooks ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Runbook {
                id: row.get(0)?,
                goal: row.get(1)?,
                created_at: row.get(2)?,
                total_steps: row.get(3)?,
                passed: row.get(4)?,
                failed: row.get(5)?,
                steps: Vec::new(),
            })
        })?;
        rows.collect()
    }
}
