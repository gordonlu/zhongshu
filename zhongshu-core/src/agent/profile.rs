use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent::loop_::AgentBudget;

/// Agent 的静态配置（Profile）。
///
/// Profile 不是 Agent 实现，而是 Agent 配置。
/// 可以从 JSON 文件加载（`load()` / `load_dir()`）。
///
/// 后续迭代会支持 YAML、权限声明、插件关联等复杂语义。
/// 当前阶段仅保留核心字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// 唯一名称，如 "qintianjian"
    pub name: String,
    /// System prompt 模板
    pub system_prompt: String,
    /// 允许使用的工具名称列表（空 = 全部工具）
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// 权限层级（占位字段，暂不实现复杂语义）
    #[serde(default = "default_authority")]
    pub authority: String,
    /// Token / 步数预算
    #[serde(default)]
    pub budget: AgentBudgetProfile,
    /// LLM profile selector (Phase 7).
    #[serde(default)]
    pub llm_profile: Option<String>,
    #[serde(default)]
    pub llm_model: Option<String>,
    #[serde(default)]
    pub llm_reasoning_effort: Option<String>,
}

fn default_authority() -> String {
    "standard".into()
}

impl AgentProfile {
    /// 创建一个新的 Profile。
    pub fn new(
        name: impl Into<String>,
        system_prompt: impl Into<String>,
        tool_names: Vec<String>,
        budget: AgentBudget,
    ) -> Self {
        AgentProfile {
            name: name.into(),
            system_prompt: system_prompt.into(),
            tool_names,
            authority: default_authority(),
            budget: AgentBudgetProfile::from_budget(&budget),
            llm_profile: None,
            llm_model: None,
            llm_reasoning_effort: None,
        }
    }

    /// 从 JSON 文件加载单个 Profile。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let profile: AgentProfile = serde_json::from_str(&content)?;
        Ok(profile)
    }

    /// 从目录加载所有 .json Profile 文件。
    ///
    /// 非递归，忽略解析失败的文件并记录警告。
    pub fn load_dir(path: &Path) -> Vec<Self> {
        let dir = match std::fs::read_dir(path) {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "cannot read profile directory");
                return Vec::new();
            }
        };

        dir.filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            match Self::load(&path) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to load profile");
                    None
                }
            }
        })
        .collect()
    }

    /// 转换为 Worker 运行时使用的 `AgentBudget`。
    pub fn to_worker_budget(&self) -> AgentBudget {
        AgentBudget {
            max_steps: self.budget.max_steps,
            max_tool_calls: self.budget.max_tool_calls,
            per_tool_limit: self.budget.per_tool_limit,
            token_limit: self.budget.token_limit,
            llm_timeout: std::time::Duration::from_secs(self.budget.llm_timeout_secs),
            tool_timeout: std::time::Duration::from_secs(self.budget.tool_timeout_secs),
        }
    }
}

/// Profile 中可序列化的预算配置。
///
/// 使用 `#[serde(default)]` 确保旧配置文件的向后兼容性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBudgetProfile {
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    #[serde(default = "default_per_tool_limit")]
    pub per_tool_limit: u32,
    #[serde(default = "default_token_limit")]
    pub token_limit: usize,
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

fn default_max_steps() -> u32 {
    AgentBudget::assistant_default().max_steps
}
fn default_max_tool_calls() -> u32 {
    AgentBudget::assistant_default().max_tool_calls
}
fn default_per_tool_limit() -> u32 {
    AgentBudget::assistant_default().per_tool_limit
}
fn default_token_limit() -> usize {
    32_000
}
fn default_llm_timeout_secs() -> u64 {
    AgentBudget::assistant_default().llm_timeout.as_secs()
}
fn default_tool_timeout_secs() -> u64 {
    AgentBudget::assistant_default().tool_timeout.as_secs()
}

impl AgentBudgetProfile {
    fn from_budget(budget: &AgentBudget) -> Self {
        AgentBudgetProfile {
            max_steps: budget.max_steps,
            max_tool_calls: budget.max_tool_calls,
            per_tool_limit: budget.per_tool_limit,
            token_limit: budget.token_limit,
            llm_timeout_secs: budget.llm_timeout.as_secs(),
            tool_timeout_secs: budget.tool_timeout.as_secs(),
        }
    }
}

impl Default for AgentBudgetProfile {
    fn default() -> Self {
        AgentBudgetProfile {
            max_steps: default_max_steps(),
            max_tool_calls: default_max_tool_calls(),
            per_tool_limit: default_per_tool_limit(),
            token_limit: default_token_limit(),
            llm_timeout_secs: default_llm_timeout_secs(),
            tool_timeout_secs: default_tool_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn profile_default_budget() {
        let p = AgentProfile::new("test", "prompt", vec![], AgentBudget::default());
        assert_eq!(p.name, "test");
        assert_eq!(p.system_prompt, "prompt");
        assert!(p.tool_names.is_empty());
    }

    #[test]
    fn profile_with_tools() {
        let p = AgentProfile::new(
            "narrow",
            "prompt",
            vec!["shell".into(), "read_file".into()],
            AgentBudget::default(),
        );
        assert_eq!(p.tool_names.len(), 2);
    }

    #[test]
    fn profile_roundtrip_json() {
        let p = AgentProfile::new(
            "qintianjian",
            "你是一个天气助手。",
            vec!["weather".into(), "calendar".into()],
            AgentBudget {
                max_steps: 5,
                max_tool_calls: 3,
                per_tool_limit: 3,
                token_limit: 10_000,
                llm_timeout: Duration::from_secs(60),
                tool_timeout: Duration::from_secs(30),
            },
        );
        let json = serde_json::to_string(&p).unwrap();
        let loaded: AgentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "qintianjian");
        assert_eq!(loaded.budget.token_limit, 10_000);
    }

    #[test]
    fn profile_load_dir_empty() {
        let tmp = std::env::temp_dir().join("zhongshu_profiles_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let profiles = AgentProfile::load_dir(&tmp);
        assert!(profiles.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn profile_load_dir_skips_non_json() {
        let tmp = std::env::temp_dir().join("zhongshu_profiles_test_skip");
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::write(tmp.join("readme.txt"), "not a profile");
        let profiles = AgentProfile::load_dir(&tmp);
        assert!(profiles.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn profile_load_dir_with_valid() {
        let tmp = std::env::temp_dir().join("zhongshu_profiles_test_valid");
        let _ = std::fs::create_dir_all(&tmp);
        let p = AgentProfile::new("worker-a", "prompt-a", vec![], AgentBudget::default());
        let json = serde_json::to_string(&p).unwrap();
        let _ = std::fs::write(tmp.join("a.json"), &json);
        let _ = std::fs::write(tmp.join("b.json"), &json);
        let profiles = AgentProfile::load_dir(&tmp);
        assert_eq!(profiles.len(), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn budget_conversion() {
        let p = AgentProfile::new(
            "x",
            "prompt",
            vec![],
            AgentBudget {
                max_steps: 3,
                max_tool_calls: 2,
                per_tool_limit: 2,
                token_limit: 5000,
                llm_timeout: Duration::from_secs(60),
                tool_timeout: Duration::from_secs(30),
            },
        );
        let b = p.to_worker_budget();
        assert_eq!(b.max_steps, 3);
        assert_eq!(b.max_tool_calls, 2);
        assert_eq!(b.per_tool_limit, 2);
        assert_eq!(b.token_limit, 5000);
        assert_eq!(b.llm_timeout, Duration::from_secs(60));
        assert_eq!(b.tool_timeout, Duration::from_secs(30));
    }

    #[test]
    fn legacy_profile_budget_fills_runtime_limits() {
        let profile: AgentProfile = serde_json::from_value(serde_json::json!({
            "name": "legacy",
            "system_prompt": "prompt",
            "tool_names": [],
            "budget": { "token_limit": 4096 }
        }))
        .expect("legacy profile");
        let budget = profile.to_worker_budget();
        assert_eq!(budget.max_steps, AgentBudget::assistant_default().max_steps);
        assert_eq!(
            budget.max_tool_calls,
            AgentBudget::assistant_default().max_tool_calls
        );
        assert_eq!(budget.token_limit, 4096);
    }
}
