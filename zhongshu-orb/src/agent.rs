use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Data model ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    #[serde(default = "default_version")]
    pub version: u32,
    pub id: String,
    pub created_at: String,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    #[serde(default)]
    pub long_term_memory: Vec<MemoryEntry>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub text: String,
    pub status: GoalStatus,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Completed,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub done: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub source: String,
    pub created_at: String,
}

impl AgentProfile {
    pub fn new() -> Self {
        let ts = timestamp();
        AgentProfile {
            version: 1,
            id: format!("zhongshu-{ts}"),
            created_at: ts.to_string(),
            goals: Vec::new(),
            todos: Vec::new(),
            long_term_memory: Vec::new(),
        }
    }

    pub fn to_prompt_context(&self) -> String {
        let active_goals: Vec<_> = self
            .goals
            .iter()
            .filter(|g| g.status == GoalStatus::Active)
            .collect();
        let pending_todos: Vec<_> = self.todos.iter().filter(|t| !t.done).collect();
        if active_goals.is_empty() && pending_todos.is_empty() && self.long_term_memory.is_empty() {
            return String::new();
        }
        let mut ctx = String::from("## 当前状态\n");
        if !active_goals.is_empty() {
            ctx.push_str("活跃目标:\n");
            for g in &active_goals {
                ctx.push_str(&format!("- {}\n", g.text));
            }
        }
        if !pending_todos.is_empty() {
            ctx.push_str("待办:\n");
            for t in &pending_todos {
                ctx.push_str(&format!("- [ ] {}\n", t.text));
            }
        }
        if !self.long_term_memory.is_empty() {
            let total_chars: usize = self
                .long_term_memory
                .iter()
                .map(|m| m.text.chars().count())
                .sum();
            ctx.push_str(&format!("长期记忆（{} 字符 / 上限 2000）:\n", total_chars));
            for m in &self.long_term_memory {
                ctx.push_str(&format!("- {}\n", m.text));
            }
        }
        ctx
    }
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Agent memory ────────────────────────────────────────────────────

pub struct AgentMemory {
    profile: Arc<RwLock<AgentProfile>>,
    path: PathBuf,
}

impl Clone for AgentMemory {
    fn clone(&self) -> Self {
        AgentMemory {
            profile: self.profile.clone(),
            path: self.path.clone(),
        }
    }
}

impl AgentMemory {
    pub fn load(path: &PathBuf) -> Self {
        let profile = load_from_disk(path).unwrap_or_else(|| {
            let p = AgentProfile::new();
            save_to_disk(path, &p);
            p
        });
        AgentMemory {
            profile: Arc::new(RwLock::new(profile)),
            path: path.clone(),
        }
    }

    pub fn prompt_context(&self) -> String {
        self.profile
            .try_read()
            .map(|p| p.to_prompt_context())
            .unwrap_or_default()
    }

    /// Add a new active goal.  Deduplicates by text.
    #[allow(dead_code)] // called from agent prompt / inbox in future
    pub fn add_goal(&self, text: &str) {
        if let Ok(mut p) = self.profile.try_write() {
            if p.goals
                .iter()
                .any(|g| g.text == text && g.status == GoalStatus::Active)
            {
                return;
            }
            let now = timestamp().to_string();
            p.goals.push(Goal {
                id: format!("goal-{now}"),
                text: text.to_string(),
                status: GoalStatus::Active,
                created_at: now.clone(),
                completed_at: None,
            });
            save_to_disk(&self.path, &p);
        }
    }

    /// Mark a goal completed by matching text.  Returns true if found.
    pub fn complete_goal(&self, text: &str) -> bool {
        if let Ok(mut p) = self.profile.try_write() {
            if let Some(g) = p
                .goals
                .iter_mut()
                .find(|g| g.text == text && g.status == GoalStatus::Active)
            {
                g.status = GoalStatus::Completed;
                g.completed_at = Some(timestamp().to_string());
                save_to_disk(&self.path, &p);
                return true;
            }
        }
        false
    }

    /// Parse todo checkboxes from the assistant response.
    pub fn extract_todos(&self, response: &str) {
        if let Ok(mut p) = self.profile.try_write() {
            let added = parse_checkboxes(response);
            if added.is_empty() {
                return;
            }
            let now = timestamp().to_string();
            for text in added {
                if p.todos.iter().any(|t| !t.done && t.text == text) {
                    continue;
                }
                p.todos.push(TodoItem {
                    text,
                    done: false,
                    created_at: now.clone(),
                });
            }
            save_to_disk(&self.path, &p);
        }
    }

    /// Keep at most `keep_completed` completed goals, archiving the
    /// oldest excess ones.  Active and already-archived goals are
    /// left untouched.
    pub fn archive_completed_goals(&self, keep_completed: usize) {
        if let Ok(mut p) = self.profile.try_write() {
            let completed_indices: Vec<usize> = p
                .goals
                .iter()
                .enumerate()
                .filter(|(_, g)| g.status == GoalStatus::Completed)
                .map(|(i, _)| i)
                .collect();
            let to_archive = completed_indices.len().saturating_sub(keep_completed);
            if to_archive == 0 {
                return;
            }
            for &i in completed_indices.iter().take(to_archive) {
                p.goals[i].status = GoalStatus::Archived;
            }
            save_to_disk(&self.path, &p);
        }
    }

    /// Store a structured memory observation (fact, preference, etc.).
    #[allow(dead_code)]
    pub fn add_memory_entry(&self, text: &str, source: &str) {
        if let Ok(mut p) = self.profile.try_write() {
            let ts = timestamp().to_string();
            p.long_term_memory.push(MemoryEntry {
                id: format!("mem-{ts}"),
                text: text.to_string(),
                source: source.to_string(),
                created_at: ts,
            });
            save_to_disk(&self.path, &p);
        }
    }

    /// Scan the assistant response for completed-goal markers
    /// (lines starting with `- [x] goal-name`) and mark matching
    /// active goals as completed.
    pub fn extract_goal_completions(&self, response: &str) {
        let completed: Vec<String> = response
            .lines()
            .filter(|l| l.trim().starts_with("- [x] ") || l.trim().starts_with("* [x] "))
            .map(|l| l.trim()[6..].trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for text in &completed {
            self.complete_goal(text);
        }
    }
}

// ── Persistence ─────────────────────────────────────────────────────

fn load_from_disk(path: &PathBuf) -> Option<AgentProfile> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(p) => {
            tracing::info!("memory: agent memory loaded from {}", path.display());
            Some(p)
        }
        Err(e) => {
            tracing::warn!("corrupt agent profile at {}: {e}", path.display());
            None
        }
    }
}

fn save_to_disk(path: &PathBuf, profile: &AgentProfile) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = match serde_json::to_string_pretty(profile) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("failed to serialize agent profile: {e}");
            return;
        }
    };
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        tracing::warn!("failed to write agent profile: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::warn!("failed to rename agent profile: {e}");
    }
}

fn parse_checkboxes(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.trim().starts_with("- [ ] ") || l.trim().starts_with("* [ ] "))
        .map(|l| l.trim()[6..].trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_memory() -> AgentMemory {
        let path = std::env::temp_dir().join("zhongshu_test_goal.json");
        let _ = std::fs::remove_file(&path);
        AgentMemory::load(&path)
    }

    #[test]
    fn add_and_complete_goal() {
        let mem = test_memory();
        mem.add_goal("学习 Rust");
        mem.add_goal("写周报");
        {
            let p = mem.profile.try_read().unwrap();
            assert_eq!(p.goals.len(), 2);
            assert_eq!(p.goals[0].status, GoalStatus::Active);
        }
        assert!(mem.complete_goal("学习 Rust"));
        {
            let p = mem.profile.try_read().unwrap();
            assert_eq!(p.goals[0].status, GoalStatus::Completed);
            assert!(p.goals[0].completed_at.is_some());
            assert_eq!(p.goals[1].status, GoalStatus::Active);
        }
    }

    #[test]
    fn goal_deduplication() {
        let mem = test_memory();
        mem.add_goal("学习 Rust");
        mem.add_goal("学习 Rust");
        assert_eq!(mem.profile.try_read().unwrap().goals.len(), 1);
    }

    #[test]
    fn extract_goal_completions_from_response() {
        let mem = test_memory();
        mem.add_goal("fix bug");
        mem.add_goal("write docs");
        mem.extract_goal_completions("done\n- [x] fix bug\nmore text\n- [x] write docs");
        let p = mem.profile.try_read().unwrap();
        assert!(p.goals.iter().all(|g| g.status == GoalStatus::Completed));
    }

    #[test]
    fn prompt_context_shows_only_active_goals() {
        let mem = test_memory();
        mem.add_goal("active goal");
        mem.add_goal("done goal");
        mem.complete_goal("done goal");
        let ctx = mem.prompt_context();
        assert!(ctx.contains("active goal"));
        assert!(!ctx.contains("done goal"));
    }

    #[test]
    fn archive_completed_goals_removes_old_completed() {
        let mem = test_memory();
        mem.add_goal("g1");
        mem.add_goal("g2");
        mem.add_goal("g3");
        mem.complete_goal("g1");
        mem.complete_goal("g2");
        // 1 active (g3), 2 completed (g1, g2). Archive oldest completed.
        mem.archive_completed_goals(1);
        {
            let p = mem.profile.try_read().unwrap();
            assert_eq!(
                p.goals
                    .iter()
                    .filter(|g| g.status == GoalStatus::Active)
                    .count(),
                1
            );
            assert_eq!(p.goals[0].status, GoalStatus::Archived, "g1 archived");
            assert_eq!(p.goals[1].status, GoalStatus::Completed, "g2 kept");
            assert_eq!(p.goals[2].status, GoalStatus::Active, "g3 active");
        }
    }

    #[test]
    fn archive_completed_goals_does_nothing_when_under_limit() {
        let mem = test_memory();
        mem.add_goal("g1");
        mem.add_goal("g2");
        mem.complete_goal("g1");
        // 1 completed, keep 2 → nothing archived.
        mem.archive_completed_goals(2);
        {
            let p = mem.profile.try_read().unwrap();
            assert_eq!(
                p.goals
                    .iter()
                    .filter(|g| g.status == GoalStatus::Completed)
                    .count(),
                1
            );
            assert_eq!(
                p.goals
                    .iter()
                    .filter(|g| g.status == GoalStatus::Archived)
                    .count(),
                0
            );
        }
    }

    #[test]
    fn archive_completed_goals_no_completed_noop() {
        let mem = test_memory();
        mem.add_goal("g1");
        // 1 active, 0 completed → nothing to archive.
        mem.archive_completed_goals(0);
        {
            let p = mem.profile.try_read().unwrap();
            assert_eq!(
                p.goals
                    .iter()
                    .filter(|g| g.status == GoalStatus::Active)
                    .count(),
                1
            );
            assert_eq!(
                p.goals
                    .iter()
                    .filter(|g| g.status == GoalStatus::Archived)
                    .count(),
                0
            );
        }
    }

    #[test]
    fn add_memory_entry_stores_observation() {
        let mem = test_memory();
        mem.add_memory_entry("user prefers dark mode", "observation");
        let p = mem.profile.try_read().unwrap();
        assert_eq!(p.long_term_memory.len(), 1);
        assert_eq!(p.long_term_memory[0].text, "user prefers dark mode");
        assert_eq!(p.long_term_memory[0].source, "observation");
        assert!(!p.long_term_memory[0].id.is_empty());
    }
}
