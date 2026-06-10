use std::path::PathBuf;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
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
}

fn default_version() -> u32 { 1 }

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

impl AgentProfile {
    pub fn new() -> Self {
        let ts = timestamp();
        AgentProfile {
            version: 1,
            id: format!("zhongshu-{ts}"),
            created_at: ts.to_string(),
            goals: Vec::new(),
            todos: Vec::new(),
        }
    }

    pub fn to_prompt_context(&self) -> String {
        let active_goals: Vec<_> = self.goals.iter().filter(|g| g.status == GoalStatus::Active).collect();
        let pending_todos: Vec<_> = self.todos.iter().filter(|t| !t.done).collect();
        if active_goals.is_empty() && pending_todos.is_empty() {
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
        AgentMemory { profile: self.profile.clone(), path: self.path.clone() }
    }
}

impl AgentMemory {
    pub fn load(path: &PathBuf) -> Self {
        let profile = load_from_disk(path).unwrap_or_else(|| {
            let p = AgentProfile::new();
            save_to_disk(path, &p);
            p
        });
        AgentMemory { profile: Arc::new(RwLock::new(profile)), path: path.clone() }
    }

    pub fn prompt_context(&self) -> String {
        self.profile.try_read().map(|p| p.to_prompt_context()).unwrap_or_default()
    }

    /// Add a new active goal.  Deduplicates by text.
    #[allow(dead_code)] // called from agent prompt / inbox in future
    pub fn add_goal(&self, text: &str) {
        if let Ok(mut p) = self.profile.try_write() {
            if p.goals.iter().any(|g| g.text == text && g.status == GoalStatus::Active) {
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
            if let Some(g) = p.goals.iter_mut().find(|g| g.text == text && g.status == GoalStatus::Active) {
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
            if added.is_empty() { return; }
            let now = timestamp().to_string();
            for text in added {
                if p.todos.iter().any(|t| !t.done && t.text == text) {
                    continue;
                }
                p.todos.push(TodoItem { text, done: false, created_at: now.clone() });
            }
            save_to_disk(&self.path, &p);
        }
    }

    /// Scan the assistant response for completed-goal markers
    /// (lines starting with `- [x] goal-name`) and mark matching
    /// active goals as completed.
    pub fn extract_goal_completions(&self, response: &str) {
        let completed: Vec<String> = response.lines()
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
        Ok(p) => { tracing::info!("agent profile loaded from {}", path.display()); Some(p) }
        Err(e) => { tracing::warn!("corrupt agent profile at {}: {e}", path.display()); None }
    }
}

fn save_to_disk(path: &PathBuf, profile: &AgentProfile) {
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let json = match serde_json::to_string_pretty(profile) {
        Ok(j) => j,
        Err(e) => { tracing::warn!("failed to serialize agent profile: {e}"); return; }
    };
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, &json) { tracing::warn!("failed to write agent profile: {e}"); return; }
    if let Err(e) = std::fs::rename(&tmp, path) { tracing::warn!("failed to rename agent profile: {e}"); }
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
}
