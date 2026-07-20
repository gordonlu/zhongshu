use crate::agent::attention::AttentionLevel;
use crate::agent::contract::{AcceptanceCriteria, DelegationContract, PatchRecord, VerificationRecord, CommandRecord, WorkerArtifacts, WorkerOutcome};
use crate::agent::llm::{LlmProvider, Message};
use crate::agent::loop_::{run_agent, AgentCallbacks};
use crate::agent::profile::AgentProfile;
use crate::agent::report::Report;
use crate::agent::runtime::AgentRuntime;
use crate::harness::trace::event::HarnessEvent;
use crate::task::Task;
use tokio_util::sync::CancellationToken;

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
        let mut scoped_runtime = Worker::build_scoped_runtime(runtime, profile);

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

        let result = run_agent(
            &mut scoped_runtime,
            messages,
            callbacks,
            &task.source,
            CancellationToken::new(),
        )
        .await?;

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
        let outcome = result.outcome;
        let success = outcome == crate::agent::RunOutcome::CompletedVerified
            || outcome == crate::agent::RunOutcome::CompletedUnverified;

        Ok(Report {
            task_id: task.id,
            worker: profile.name.clone(),
            summary,
            findings: last_content.to_string(),
            success,
            outcome,
            confidence: if success { 0.5 } else { 0.0 },
            attention,
            trace_events: result.trace_events,
        })
    }

    fn build_scoped_runtime(runtime: &AgentRuntime, profile: &AgentProfile) -> AgentRuntime {
        let scoped_registry = if profile.tool_names.is_empty() {
            runtime.registry.clone()
        } else {
            runtime.registry.select(
                &profile
                    .tool_names
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        };

        let provider: std::sync::Arc<dyn LlmProvider> = if let Some(ref m) = profile.llm_model {
            runtime.provider.change_model(m)
        } else {
            runtime.provider.clone()
        };
        AgentRuntime {
            registry: scoped_registry,
            budget: profile.to_worker_budget(),
            provider,
            model: profile
                .llm_model
                .clone()
                .unwrap_or_else(|| runtime.model.clone()),
            reasoning_effort: profile.llm_reasoning_effort.clone(),
            harness_state: crate::harness::HarnessState::new(),
            idempotency_checker: None,
            checkpoint_store: None,
            ledger: None,
        }
    }

    /// Execute a worker with a structured DelegationContract.
    ///
    /// Converts the contract into a Task + AgentProfile, runs the worker,
    /// and returns a structured WorkerOutcome with artifacts extracted from
    /// trace events.
    pub async fn execute_with_contract(
        runtime: &AgentRuntime,
        contract: &DelegationContract,
        callbacks: Option<std::sync::Arc<AgentCallbacks>>,
    ) -> anyhow::Result<WorkerOutcome> {
        let task = Task::from(contract);
        let profile = AgentProfile::from(contract);
        let report = Self::execute(runtime, &profile, task, callbacks).await?;
        let mut outcome = WorkerOutcome::from(report);
        outcome.artifacts = Self::extract_artifacts(&outcome.trace_events);
        // Verify acceptance criteria and downgrade status if unmet.
        if outcome.status.is_success() {
            let issues = Self::check_acceptance(&outcome.artifacts, &contract.acceptance);
            if !issues.is_empty() {
                outcome.summary = format!("{} (验收问题: {})", outcome.summary, issues.join("; "));
                outcome.status = crate::agent::contract::WorkerStatus::CompletedWithIssues;
            }
        }
        Ok(outcome)
    }

    /// Check acceptance criteria against the extracted artifacts.
    /// Returns a list of unmet criteria descriptions.
    fn check_acceptance(
        artifacts: &WorkerArtifacts,
        criteria: &AcceptanceCriteria,
    ) -> Vec<String> {
        let mut issues = Vec::new();
        if criteria.verification_required && artifacts.verification_results.is_empty() {
            issues.push("需要验证但无验证记录".into());
        }
        if criteria.tests_must_pass {
            let failed: Vec<&str> = artifacts
                .verification_results
                .iter()
                .filter(|v| !v.success)
                .map(|v| v.command.as_str())
                .collect();
            if !failed.is_empty() {
                issues.push(format!(
                    "以下验证未通过: {}",
                    failed.join(", ")
                ));
            }
        }
        if criteria.no_ownership_violations {
            // Ownership violations are detected at the orchestrator level;
            // here we have no access to the project index. The orchestrator
            // must enforce this before accepting the outcome.
        }
        if !criteria.custom_rules.is_empty() {
            issues.push(format!(
                "以下自定义规则需人工确认: {}",
                criteria.custom_rules.join("; "),
            ));
        }
        issues
    }

    /// Extract structured artifacts from trace events.
    fn extract_artifacts(events: &[HarnessEvent]) -> WorkerArtifacts {
        let mut patches = Vec::new();
        let mut verification_results = Vec::new();
        let mut commands_run = Vec::new();

        for event in events {
            match event {
                HarnessEvent::PatchApplied { path, .. } => {
                    patches.push(PatchRecord {
                        path: path.clone(),
                        diff_summary: String::new(),
                        applied: true,
                    });
                }
                HarnessEvent::PatchPreview { path, diff_summary, .. } => {
                    if !patches.iter().any(|p: &PatchRecord| p.path == *path) {
                        patches.push(PatchRecord {
                            path: path.clone(),
                            diff_summary: diff_summary.clone(),
                            applied: false,
                        });
                    }
                }
                HarnessEvent::Verification { command, success: v_success, exit_code, .. } => {
                    verification_results.push(VerificationRecord {
                        command: command.clone(),
                        success: *v_success,
                        exit_code: *exit_code,
                    });
                }
                HarnessEvent::ToolCall { tool_name, success: t_success, .. } => {
                    commands_run.push(CommandRecord {
                        command: tool_name.clone(),
                        exit_code: None,
                        success: *t_success,
                    });
                }
                _ => {}
            }
        }

        WorkerArtifacts {
            patches,
            verification_results,
            commands_run,
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
