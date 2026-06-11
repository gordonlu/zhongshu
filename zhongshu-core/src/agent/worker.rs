use crate::agent::attention::AttentionLevel;
use crate::agent::loop_::{run_agent, AgentCallbacks};
use crate::agent::llm::Message;
use crate::agent::profile::AgentProfile;
use crate::agent::report::Report;
use crate::agent::runtime::AgentRuntime;
use crate::task::Task;

/// 向后兼容类型别名。
/// `WorkerProfile` 已被 `AgentProfile` 取代。
pub type WorkerProfile = AgentProfile;

/// Worker Agent —— 一次性 LLM 任务执行节点。
///
/// 生命周期：Create → Execute → Report → Destroy
///
/// Worker 不保留长期状态。每次 `execute()` 是独立的。
/// Worker 禁止创建子 Worker（v1 强制 Depth = 1）。
pub struct Worker;

impl Worker {
    /// 执行一个 Task，返回 Report。
    ///
    /// 内部流程：
    /// 1. 根据 profile.tool_names 裁剪 ToolRegistry
    /// 2. 用 profile.budget 替换运行时预算
    /// 3. 注入 system prompt + task 上下文
    /// 4. 调用 `run_agent()` 执行 ReAct 循环
    /// 5. 解析输出为 Report
    pub async fn execute(
        runtime: &AgentRuntime,
        profile: &AgentProfile,
        task: Task,
        callbacks: Option<std::sync::Arc<AgentCallbacks>>,
    ) -> anyhow::Result<Report> {
        let scoped_runtime = Worker::build_scoped_runtime(runtime, profile);

        let messages = vec![
            Message::system(&profile.system_prompt),
            Message::user(format!(
                "## 任务来源\n{}\n\n## 任务参数\n{}",
                task.source,
                serde_json::to_string_pretty(&task.arguments).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to serialize task arguments");
                    String::new()
                })
            )),
        ];

        let result = run_agent(&scoped_runtime, messages, callbacks, &task.source).await?;

        let last_content = result
            .messages
            .last()
            .map(|m| m.content.as_str())
            .unwrap_or("");

        let summary = if last_content.chars().count() > 200 {
            format!("{}...", last_content.chars().take(200).collect::<String>())
        } else {
            last_content.to_string()
        };
        let attention = Worker::infer_attention(last_content);

        Ok(Report {
            task_id: task.id,
            worker: profile.name.clone(),
            summary,
            findings: last_content.to_string(),
            confidence: 0.5,
            attention,
        })
    }

    fn build_scoped_runtime(runtime: &AgentRuntime, profile: &AgentProfile) -> AgentRuntime {
        let scoped_registry = if profile.tool_names.is_empty() {
            runtime.registry.clone()
        } else {
            runtime.registry.select(&profile.tool_names.iter().map(String::as_str).collect::<Vec<_>>())
        };

        AgentRuntime {
            registry: scoped_registry,
            budget: profile.to_worker_budget(),
            provider: runtime.provider.clone(),
            model: runtime.model.clone(),
        }
    }

    /// 从输出内容推断 AttentionLevel。
    fn infer_attention(content: &str) -> AttentionLevel {
        let lower = content.to_lowercase();
        if lower.contains("<immediate>") || lower.contains("urgent") || lower.contains("紧急") {
            AttentionLevel::Immediate
        } else if lower.contains("<notify>") {
            AttentionLevel::Notify
        } else {
            AttentionLevel::Digest
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentBudget;

    fn dummy_profile() -> AgentProfile {
        AgentProfile::new(
            "test-worker",
            "你是一个测试助手。",
            vec![],
            AgentBudget::default(),
        )
    }

    #[test]
    fn worker_profile_defaults() {
        let p = dummy_profile();
        assert_eq!(p.name, "test-worker");
        assert!(p.tool_names.is_empty());
    }

    #[test]
    fn worker_profile_with_tools() {
        let p = AgentProfile::new(
            "narrow",
            "prompt",
            vec!["shell".into(), "read_file".into()],
            AgentBudget::default(),
        );
        assert_eq!(p.tool_names.len(), 2);
    }

    #[test]
    fn infer_attention_digest_default() {
        assert_eq!(Worker::infer_attention("一切正常"), AttentionLevel::Digest);
        assert_eq!(Worker::infer_attention(""), AttentionLevel::Digest);
    }

    #[test]
    fn infer_attention_immediate() {
        assert_eq!(
            Worker::infer_attention("URGENT: system crash detected"),
            AttentionLevel::Immediate
        );
        assert_eq!(
            Worker::infer_attention("检测到紧急情况"),
            AttentionLevel::Immediate
        );
    }

    #[test]
    fn infer_attention_notify() {
        assert_eq!(
            Worker::infer_attention("<notify> battery low"),
            AttentionLevel::Notify
        );
    }

    #[test]
    fn infer_attention_digest_tag() {
        assert_eq!(
            Worker::infer_attention("<digest> daily summary"),
            AttentionLevel::Digest
        );
    }
}
