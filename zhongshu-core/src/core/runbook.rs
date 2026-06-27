use crate::core::db::Database;
use rusqlite::params;

/// Phase 6D: Record of a completed automated task — auditable, reusable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Runbook {
    pub id: String,
    pub goal: String,
    pub conversation_id: Option<i64>,
    pub steps: Vec<RunbookStep>,
    pub created_at: String,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
                conversation_id INTEGER,
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
        let _ = conn.execute(
            "ALTER TABLE runbooks ADD COLUMN conversation_id INTEGER",
            [],
        );
        Ok(())
    }

    pub fn save(&self, rb: &Runbook) -> rusqlite::Result<()> {
        self.migrate()?;
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO runbooks (id, goal, conversation_id, created_at, total_steps, passed, failed) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![rb.id, rb.goal, rb.conversation_id, rb.created_at, rb.total_steps, rb.passed, rb.failed],
        )?;
        conn.execute(
            "DELETE FROM runbook_steps WHERE runbook_id = ?1",
            params![rb.id],
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
        self.migrate()?;
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare("SELECT id, goal, conversation_id, created_at, total_steps, passed, failed FROM runbooks ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Runbook {
                id: row.get(0)?,
                goal: row.get(1)?,
                conversation_id: row.get(2)?,
                created_at: row.get(3)?,
                total_steps: row.get(4)?,
                passed: row.get(5)?,
                failed: row.get(6)?,
                steps: Vec::new(),
            })
        })?;
        let mut runbooks: Vec<Runbook> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for runbook in &mut runbooks {
            runbook.steps = self.list_steps(&runbook.id)?;
        }
        Ok(runbooks)
    }

    fn list_steps(&self, runbook_id: &str) -> rusqlite::Result<Vec<RunbookStep>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT action, tool, input, output_status, output_preview, verification \
             FROM runbook_steps WHERE runbook_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![runbook_id], |row| {
            Ok(RunbookStep {
                action: row.get(0)?,
                tool: row.get(1)?,
                input: row.get(2)?,
                output_status: row.get(3)?,
                output_preview: row.get(4)?,
                verification: row.get(5)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> RunbookStore {
        let db = Database::new(std::env::temp_dir().join(format!(
            "zhongshu-runbook-test-{}.db",
            crate::core::models::id("db")
        )));
        let store = RunbookStore::new(db);
        store.migrate().unwrap();
        store
    }

    fn sample_runbook(id: &str, tool: &str) -> Runbook {
        Runbook {
            id: id.into(),
            goal: "audit run".into(),
            conversation_id: Some(7),
            created_at: "1".into(),
            total_steps: 1,
            passed: 1,
            failed: 0,
            steps: vec![RunbookStep {
                action: "tool call step 1".into(),
                tool: tool.into(),
                input: "args_hash=abc".into(),
                output_status: "passed".into(),
                output_preview: String::new(),
                verification: "ok".into(),
            }],
        }
    }

    #[test]
    fn list_includes_steps() {
        let store = temp_store();
        store.save(&sample_runbook("rb-1", "self_test")).unwrap();

        let runbooks = store.list().unwrap();

        assert_eq!(runbooks.len(), 1);
        assert_eq!(runbooks[0].conversation_id, Some(7));
        assert_eq!(runbooks[0].steps.len(), 1);
        assert_eq!(runbooks[0].steps[0].tool, "self_test");
    }

    #[test]
    fn save_replaces_existing_steps() {
        let store = temp_store();
        store.save(&sample_runbook("rb-1", "self_test")).unwrap();
        store.save(&sample_runbook("rb-1", "shell")).unwrap();

        let runbooks = store.list().unwrap();

        assert_eq!(runbooks.len(), 1);
        assert_eq!(runbooks[0].steps.len(), 1);
        assert_eq!(runbooks[0].steps[0].tool, "shell");
    }
}
