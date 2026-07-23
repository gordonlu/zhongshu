use rusqlite::params;

use crate::agent::llm::LlmProvider;
use crate::core::db::Database;
use crate::core::models::*;
use crate::event::{Event, EventBus, MemoryEvent};

fn serialize_embedding(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    (dot / (norm_a * norm_b)) as f64
}

#[derive(Clone)]
pub struct MemoryPolicy {
    db: Database,
    min_confidence: f64,
    event_bus: Option<EventBus>,
}

impl MemoryPolicy {
    pub fn new(db: Database) -> Self {
        MemoryPolicy {
            db,
            min_confidence: 0.7,
            event_bus: None,
        }
    }

    pub fn with_event_bus(mut self, event_bus: EventBus) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    pub fn set_min_confidence(&mut self, v: f64) {
        self.min_confidence = v;
    }

    /// Promote pending candidates above the confidence threshold to active memories.
    pub fn promote_candidates(&self) -> rusqlite::Result<Vec<Memory>> {
        let mut accepted = Vec::new();
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, content, memory_type, confidence, source_type, source_id, run_id, runbook_id, source_task_id, status, created_at \
             FROM memory_candidates WHERE status IN ('proposed', 'under_review') AND confidence >= ?1 ORDER BY confidence DESC",
        )?;
        let candidates: Vec<MemoryCandidate> = stmt
            .query_map(params![self.min_confidence], |row| {
                Ok(MemoryCandidate {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    memory_type: row.get(2)?,
                    confidence: row.get(3)?,
                    source_type: row.get(4)?,
                    source_id: row.get(5)?,
                    run_id: row.get(6)?,
                    runbook_id: row.get(7)?,
                    source_task_id: row.get(8)?,
                    status: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for c in &candidates {
            let memory_type = c
                .memory_type
                .as_deref()
                .and_then(MemoryType::from_str)
                .unwrap_or(MemoryType::Preference);
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
            conn.execute(
                "UPDATE memory_candidates SET status = 'accepted' WHERE id = ?1",
                params![c.id],
            )?;
            accepted.push(mem);
        }
        Ok(accepted)
    }

    /// Promote candidates above threshold and generate embeddings.
    /// If the provider does not support embeddings, memories are stored without embeddings.
    pub async fn promote_with_embeddings(
        &self,
        provider: &dyn LlmProvider,
    ) -> anyhow::Result<Vec<Memory>> {
        let accepted = self.promote_candidates()?;
        if accepted.is_empty() {
            return Ok(accepted);
        }
        let conn = self.db.conn()?;
        for mem in &accepted {
            match provider.embed(&mem.content).await {
                Ok(vec) => {
                    let blob = serialize_embedding(&vec);
                    conn.execute(
                        "UPDATE memories SET embedding = ?1 WHERE id = ?2",
                        params![blob, mem.id],
                    )?;
                }
                Err(e) => {
                    tracing::debug!(
                        "memory: embedding not available for '{}': {e}",
                        mem.content.chars().take(60).collect::<String>()
                    );
                }
            }
        }
        Ok(accepted)
    }

    fn emit_memory_hit(&self, query: &str, memories: &[Memory]) {
        if let Some(ref eb) = self.event_bus {
            let entries: Vec<serde_json::Value> = memories
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "content": m.content,
                        "source": m.memory_type.as_str(),
                    })
                })
                .collect();
            eb.publish(Event::Memory(MemoryEvent::MemoryHit {
                query: query.to_string(),
                count: memories.len(),
                entries,
            }));
        }
    }

    pub fn list_memories(&self, limit: i64) -> rusqlite::Result<Vec<Memory>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, embedding, created_at, updated_at FROM memories ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let result: rusqlite::Result<Vec<Memory>> = stmt
            .query_map(params![limit], map_memory_row)?
            .collect();
        if let Ok(ref memories) = result {
            self.emit_memory_hit("list", memories);
        }
        result
    }

    /// Search by keyword (LIKE match). Fallback when embedding is unavailable.
    pub fn search(&self, keyword: &str, limit: i64) -> rusqlite::Result<Vec<Memory>> {
        let conn = self.db.conn()?;
        let pattern = format!("%{}%", keyword);
        let mut stmt = conn.prepare(
            "SELECT id, type, content, embedding, created_at, updated_at FROM memories WHERE content LIKE ?1 ORDER BY updated_at DESC LIMIT ?2",
        )?;
        let result: rusqlite::Result<Vec<Memory>> = stmt
            .query_map(params![pattern, limit], map_memory_row)?
            .collect();
        if let Ok(ref memories) = result {
            self.emit_memory_hit(keyword, memories);
        }
        result
    }

    /// Search by semantic similarity using embeddings.
    /// Falls back to LIKE search if the provider does not support embeddings.
    pub async fn search_with(
        &self,
        query: &str,
        provider: &dyn LlmProvider,
        limit: i64,
    ) -> anyhow::Result<Vec<Memory>> {
        let query_vec = match provider.embed(query).await {
            Ok(v) => v,
            Err(_) => {
                let result = self.search(query, limit)?;
                self.emit_memory_hit(query, &result);
                return Ok(result);
            }
        };

        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, type, content, embedding, created_at, updated_at FROM memories WHERE embedding IS NOT NULL",
        )?;
        let memories: Vec<Memory> = stmt
            .query_map([], map_memory_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut scored: Vec<(Memory, f64)> = memories
            .into_iter()
            .filter_map(|m| {
                let emb = deserialize_embedding(m.embedding.as_ref()?);
                Some((m, cosine_similarity(&query_vec, &emb)))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit as usize);
        let result: Vec<Memory> = scored.into_iter().map(|(m, _)| m).collect();
        self.emit_memory_hit(query, &result);
        Ok(result)
    }
}

fn map_memory_row(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        memory_type: MemoryType::from_str(&row.get::<_, String>(1)?)
            .unwrap_or(MemoryType::Preference),
        content: row.get(2)?,
        embedding: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}
