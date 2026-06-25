use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent::loop_::AgentBudget;

const MIN_STEPS: usize = 1;
const MAX_STEPS: usize = 100;
const MIN_TOOL_CALLS: usize = 1;
const MAX_TOOL_CALLS: usize = 200;
const MIN_TOKEN_LIMIT: usize = 1_000;
const MAX_TOKEN_LIMIT: usize = 1_000_000;

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
        let mut profile: AgentProfile = serde_json::from_str(&content)?;
        profile.budget.validate(&path.display().to_string());
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
        }
    }
}

/// Profile 中可序列化的预算配置。
///
/// 与 `AgentBudget` 字段对应，但多了 `#[serde(default)]` 方便配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBudgetProfile {
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: usize,
    #[serde(default = "default_per_tool_limit")]
    pub per_tool_limit: usize,
    #[serde(default = "default_token_limit")]
    pub token_limit: usize,
}

fn default_max_steps() -> usize {
    10
}
fn default_max_tool_calls() -> usize {
    5
}
fn default_per_tool_limit() -> usize {
    20
}
fn default_token_limit() -> usize {
    32_000
}

impl AgentBudgetProfile {
    fn from_budget(budget: &AgentBudget) -> Self {
        let mut p = AgentBudgetProfile {
            max_steps: budget.max_steps,
            max_tool_calls: budget.max_tool_calls,
            per_tool_limit: budget.per_tool_limit,
            token_limit: budget.token_limit,
        };
        p.validate("from_budget");
        p
    }

    /// 校验预算值是否在合理范围内，越界则 clamp 并记录警告。
    fn validate(&mut self, context: &str) {
        if self.max_steps < MIN_STEPS || self.max_steps > MAX_STEPS {
            tracing::warn!(
                context,
                field = "max_steps",
                value = self.max_steps,
                min = MIN_STEPS,
                max = MAX_STEPS,
                "clamping to range"
            );
            self.max_steps = self.max_steps.clamp(MIN_STEPS, MAX_STEPS);
        }
        if self.max_tool_calls < MIN_TOOL_CALLS || self.max_tool_calls > MAX_TOOL_CALLS {
            tracing::warn!(
                context,
                field = "max_tool_calls",
                value = self.max_tool_calls,
                min = MIN_TOOL_CALLS,
                max = MAX_TOOL_CALLS,
                "clamping to range"
            );
            self.max_tool_calls = self.max_tool_calls.clamp(MIN_TOOL_CALLS, MAX_TOOL_CALLS);
        }
        if self.token_limit < MIN_TOKEN_LIMIT || self.token_limit > MAX_TOKEN_LIMIT {
            tracing::warn!(
                context,
                field = "token_limit",
                value = self.token_limit,
                min = MIN_TOKEN_LIMIT,
                max = MAX_TOKEN_LIMIT,
                "clamping to range"
            );
            self.token_limit = self.token_limit.clamp(MIN_TOKEN_LIMIT, MAX_TOKEN_LIMIT);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        );
        let json = serde_json::to_string(&p).unwrap();
        let loaded: AgentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "qintianjian");
        assert_eq!(loaded.budget.max_steps, 5);
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
            },
        );
        let b = p.to_worker_budget();
        assert_eq!(b.max_steps, 3);
        assert_eq!(b.max_tool_calls, 2);
        assert_eq!(b.token_limit, 5000);
    }
}
