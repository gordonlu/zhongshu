use rusqlite::params;

use crate::core::db::Database;
use crate::core::models::*;

#[derive(Clone)]
pub struct ArtifactRepository {
    db: Database,
}

impl ArtifactRepository {
    pub fn new(db: Database) -> Self {
        ArtifactRepository { db }
    }

    pub fn insert(
        &self,
        artifact_type: ArtifactType,
        title: Option<&str>,
        uri: Option<&str>,
        summary: Option<&str>,
    ) -> rusqlite::Result<Artifact> {
        let conn = self.db.conn()?;
        let a = Artifact {
            id: id("art"),
            artifact_type,
            title: title.map(|s| s.to_string()),
            uri: uri.map(|s| s.to_string()),
            summary: summary.map(|s| s.to_string()),
            metadata: None,
            created_at: now(),
        };
        conn.execute(
            "INSERT INTO artifacts (id, type, title, uri, summary, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![a.id, a.artifact_type.as_str(), a.title, a.uri, a.summary, a.created_at],
        )?;
        Ok(a)
    }

    pub fn link(&self, task_id: &str, artifact_id: &str, relation: &str) -> rusqlite::Result<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO task_artifacts (task_id, artifact_id, relation) VALUES (?1,?2,?3)",
            params![task_id, artifact_id, relation],
        )?;
        Ok(())
    }
}
