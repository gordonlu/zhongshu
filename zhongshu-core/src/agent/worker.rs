use crate::agent::attention::AttentionLevel;
use crate::agent::contract::{
    CommandRecord, DelegationContract, PatchRecord, VerificationRecord, WorkerArtifacts,
    WorkerOutcome,
};
use crate::agent::llm::{LlmProvider, Message};
use crate::agent::loop_::{run_agent_with_verification_policy, AgentCallbacks};
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
        Self::execute_with_cancel(
            runtime,
            profile,
            task,
            callbacks,
            CancellationToken::new(),
            crate::runtime::ExecutionProfile::Worker,
        )
        .await
    }

    /// Execute a worker with a caller-owned cancellation scope. The token is
    /// passed through to the ReAct loop and tool executor; cancellation returns
    /// an observable `RunOutcome::Interrupted` report rather than detaching the
    /// in-flight worker future.
    ///
    /// `execution_profile` controls checkpoint/journal behavior:
    /// - `Durable` for background tasks (full recovery),
    /// - `Worker` for delegation workers (parent manages retry).
    pub async fn execute_with_cancel(
        runtime: &AgentRuntime,
        profile: &AgentProfile,
        task: Task,
        callbacks: Option<std::sync::Arc<AgentCallbacks>>,
        cancel_token: CancellationToken,
        execution_profile: crate::runtime::ExecutionProfile,
    ) -> anyhow::Result<Report> {
        let mut scoped_runtime = Worker::build_scoped_runtime(runtime, profile);
        scoped_runtime.profile = execution_profile;
        let run_id = callbacks
            .as_ref()
            .map(|callbacks| callbacks.run_id)
            .unwrap_or_else(uuid::Uuid::new_v4);

        if let Some(ledger) = scoped_runtime.ledger.clone() {
            let run_id_string = run_id.to_string();
            scoped_runtime.idempotency_checker = Some(std::sync::Arc::new({
                let ledger = ledger.clone();
                let run_id_string = run_id_string.clone();
                move |name: &str, args: &str| {
                    let key = crate::agent::run::RunController::idempotency_key(name, args);
                    ledger
                        .is_tool_completed(&run_id_string, &key)
                        .unwrap_or(false)
                }
            }));
        }

        // Note: ledger recording is now handled by ActionJournal
        // inside the dispatcher, so no internal callbacks are needed.

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

        let result = run_agent_with_verification_policy(
            &mut scoped_runtime,
            messages,
            callbacks,
            &task.source,
            cancel_token,
            profile.verification_policy.explicit_requirement(),
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
        let success = matches!(
            outcome,
            crate::agent::RunOutcome::CompletedVerified
                | crate::agent::RunOutcome::CompletedUnverified
        );

        Ok(Report {
            task_id: task.id,
            worker: profile.name.clone(),
            run_id: run_id.to_string(),
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
            // A parent run's idempotency closure is keyed to the parent run id.
            // `execute` installs a worker-run-specific checker when a ledger is
            // available; without a ledger there is nothing safe to inherit.
            idempotency_checker: None,
            // Worker restart needs its profile/contract to be persisted as well;
            // reusing the main-agent checkpoint store would resume it with the
            // wrong runtime after process restart. Keep this disabled until the
            // worker scheduler owns that recovery metadata.
            checkpoint_store: None,
            ledger: runtime.ledger.clone(),
            event_bus: runtime.event_bus.clone(),
            profile: crate::runtime::ExecutionProfile::Worker,
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
        let mut contract_runtime = runtime.clone();
        contract_runtime.registry = contract_runtime.registry.restrict_to_paths(
            &contract.scope.owned_files,
            &contract.scope.allowed_directories,
        );
        if contract.permissions.require_approval {
            contract_runtime.registry = contract_runtime
                .registry
                .require_approval_for_side_effects();
        }
        for denied in &contract.permissions.denied_tools {
            contract_runtime.registry.unregister(denied);
        }
        let report = Self::execute(&contract_runtime, &profile, task, callbacks).await?;
        let mut outcome = WorkerOutcome::from(report);
        outcome.artifacts = Self::extract_artifacts(&outcome.trace_events);
        // Verify acceptance criteria and downgrade status if unmet.
        if outcome.status.is_success() {
            let issues = Self::check_acceptance(&outcome.artifacts, contract);
            if !issues.is_empty() {
                outcome.summary = format!("{} (验收问题: {})", outcome.summary, issues.join("; "));
                outcome.status = crate::agent::contract::WorkerStatus::CompletedWithIssues;
            }
        }
        Ok(outcome)
    }

    /// Check acceptance criteria against the extracted artifacts.
    /// Returns a list of unmet criteria descriptions.
    fn check_acceptance(artifacts: &WorkerArtifacts, contract: &DelegationContract) -> Vec<String> {
        let criteria = &contract.acceptance;
        let mut issues = Vec::new();
        if criteria.verification_required && artifacts.verification_results.is_empty() {
            issues.push("需要验证但无验证记录".into());
        }
        if criteria.tests_must_pass && artifacts.verification_results.is_empty() {
            issues.push("要求测试通过但无测试记录".into());
        } else if criteria.tests_must_pass {
            let failed: Vec<&str> = artifacts
                .verification_results
                .iter()
                .filter(|v| !v.success)
                .map(|v| v.command.as_str())
                .collect();
            if !failed.is_empty() {
                issues.push(format!("以下验证未通过: {}", failed.join(", ")));
            }
        }
        if criteria.no_ownership_violations {
            for patch in &artifacts.patches {
                let owned = contract
                    .scope
                    .owned_files
                    .iter()
                    .any(|path| path == &patch.path);
                let allowed = contract
                    .scope
                    .allowed_directories
                    .iter()
                    .any(|directory| patch.path.starts_with(directory));
                if !owned && !allowed {
                    issues.push(format!("产出超出委派范围: {}", patch.path.display()));
                }
            }
        }
        if !criteria.custom_rules.is_empty() {
            issues.push(format!(
                "以下自定义规则需人工确认: {}",
                criteria.custom_rules.join("; "),
            ));
        }
        if contract.artifacts.require_patches && artifacts.patches.is_empty() {
            issues.push("契约要求 patch，但未产生 patch 记录".into());
        }
        if contract.artifacts.require_verification_evidence
            && artifacts.verification_results.is_empty()
        {
            issues.push("契约要求验证证据，但未产生验证记录".into());
        }
        if contract.artifacts.require_command_log && artifacts.commands_run.is_empty() {
            issues.push("契约要求命令日志，但未产生命令记录".into());
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
                HarnessEvent::PatchPreview {
                    path, diff_summary, ..
                } => {
                    if !patches.iter().any(|p: &PatchRecord| p.path == *path) {
                        patches.push(PatchRecord {
                            path: path.clone(),
                            diff_summary: diff_summary.clone(),
                            applied: false,
                        });
                    }
                }
                HarnessEvent::Verification {
                    command,
                    success: v_success,
                    exit_code,
                    ..
                } => {
                    verification_results.push(VerificationRecord {
                        command: command.clone(),
                        success: *v_success,
                        exit_code: *exit_code,
                    });
                }
                HarnessEvent::ToolCall {
                    tool_name,
                    success: t_success,
                    ..
                } => {
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

    #[test]
    fn acceptance_requires_evidence_and_declared_artifacts() {
        let contract = DelegationContract::new("worker", "change code");
        let issues = Worker::check_acceptance(&WorkerArtifacts::default(), &contract);

        assert!(issues.iter().any(|issue| issue.contains("验证")));
        assert!(issues.iter().any(|issue| issue.contains("patch")));
    }
}
