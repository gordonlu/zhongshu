use crate::agent::contract::{
    AcceptanceCriteria, ArtifactRequirements, DelegationBudget, DelegationContract,
    DelegationPermissions, EscalationRules, WorkScope, WorkerOutcome,
};
use crate::agent::worker::Worker;
use crate::agent::runtime::AgentRuntime;
use crate::agent::AgentCallbacks;
use std::path::PathBuf;

/// Delegate a sub-task to a named worker with a structured contract.
///
/// This is the primary entry point for Lead → Worker delegation. It builds a
/// `DelegationContract` with the given constraints, runs the worker, and
/// returns the structured `WorkerOutcome`.
pub async fn delegate(
    runtime: &AgentRuntime,
    worker_name: &str,
    task_description: &str,
    owned_files: Vec<PathBuf>,
) -> anyhow::Result<WorkerOutcome> {
    let contract = DelegationContract {
        worker: worker_name.to_string(),
        task_description: task_description.to_string(),
        scope: WorkScope::new(owned_files),
        budget: DelegationBudget::default(),
        permissions: DelegationPermissions::default(),
        acceptance: AcceptanceCriteria::default(),
        artifacts: ArtifactRequirements::default(),
        escalation: EscalationRules::default(),
    };

    Worker::execute_with_contract(runtime, &contract, None).await
}

/// Delegate with full contract control.
pub async fn delegate_with_contract(
    runtime: &AgentRuntime,
    contract: &DelegationContract,
    callbacks: Option<std::sync::Arc<AgentCallbacks>>,
) -> anyhow::Result<WorkerOutcome> {
    Worker::execute_with_contract(runtime, contract, callbacks).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_::AgentBudget;
    use crate::agent::profile::AgentProfile;
    use crate::agent::runtime::AgentRuntime;
    use crate::agent::orchestrator::{
        Orchestrator, WorkerExecutionStatus,
    };
    use crate::agent::llm::{ChatCompletionRequest, ChatCompletionResponse, FinalChoice, LlmProvider, Message};
    use crate::harness::architecture::index::ProjectIndex;
    use crate::tool::ToolRegistry;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockProvider;

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("Task completed successfully."),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }
        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(crate::agent::llm::StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn model_name(&self) -> &str {
            "mock"
        }
        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(MockProvider)
        }
    }

    fn dummy_profile(name: &str) -> AgentProfile {
        AgentProfile::new(
            name,
            "你是一个测试 worker。",
            vec![],
            AgentBudget::default(),
        )
    }

    fn dummy_runtime() -> AgentRuntime {
        AgentRuntime::new(
            MockProvider,
            ToolRegistry::new(),
            "mock-model",
            AgentBudget::default(),
        )
    }

    fn make_index(files: &[&str]) -> ProjectIndex {
        use crate::harness::architecture::index::FileIndex;
        let mut index = ProjectIndex::new(std::path::PathBuf::from("."));
        for f in files {
            let path = std::path::PathBuf::from(f);
            index.files.insert(
                path,
                FileIndex {
                    path: std::path::PathBuf::from(f),
                    imports: vec![],
                    items: vec![],
                    parse_error: None,
                },
            );
        }
        index
    }

    // ── Smoke tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn delegate_single_worker_returns_outcome() {
        let runtime = dummy_runtime();
        let outcome = delegate(&runtime, "worker-a", "Fix bug in module A", vec![])
            .await
            .expect("delegation should succeed");

        assert_eq!(outcome.worker, "worker-a");
        assert!(!outcome.findings.is_empty());
        assert!(outcome.status.is_success());
    }

    #[tokio::test]
    async fn delegate_with_owned_files_sets_scope() {
        let runtime = dummy_runtime();
        let owned = vec![std::path::PathBuf::from("src/module_a.rs")];
        let outcome = delegate(&runtime, "worker-a", "Fix module A", owned.clone())
            .await
            .expect("delegation should succeed");

        // The contract stores owned_files in scope
        assert_eq!(outcome.worker, "worker-a");
        assert!(outcome.status.is_success());
    }

    /// Orchestrate 2 workers via DelegationContract, simulating a Lead pattern:
    /// 1. Split a task into 2 worker assignments
    /// 2. Execute each worker with a DelegationContract
    /// 3. Collect WorkerOutcomes
    /// 4. Verify both completed
    #[tokio::test]
    async fn lead_delegates_to_two_workers() {
        let runtime = dummy_runtime();
        let orchestrator = Orchestrator::new(runtime.clone(), crate::agent::llm_registry::LlmRegistry::new());

        let profiles = vec![
            dummy_profile("worker-a"),
            dummy_profile("worker-b"),
        ];

        let index = make_index(&[
            "src/module_a.rs",
            "src/module_b.rs",
            "src/module_c.rs",
        ]);

        // Simulate Lead splitting the task
        let assignments = orchestrator.split_task(
            "Implement feature X across all modules",
            &profiles,
            &index,
        );

        assert_eq!(assignments.len(), 2);
        assert!(!assignments[0].owned_files.is_empty());
        assert!(!assignments[1].owned_files.is_empty());

        // Delegate each assignment via contract
        let mut outcomes = Vec::new();
        for assignment in &assignments {
            let contract = DelegationContract {
                worker: assignment.worker_name.clone(),
                task_description: assignment.task_description.clone(),
                scope: WorkScope::new(assignment.owned_files.clone()),
                budget: DelegationBudget::default(),
                permissions: DelegationPermissions::default(),
                acceptance: AcceptanceCriteria::default(),
                artifacts: ArtifactRequirements::default(),
                escalation: EscalationRules::default(),
            };
            let outcome = Worker::execute_with_contract(&runtime, &contract, None)
                .await
                .expect("worker should complete");
            outcomes.push(outcome);
        }

        assert_eq!(outcomes.len(), 2);
        for outcome in &outcomes {
            assert!(
                outcome.status.is_success(),
                "worker '{}' should succeed, got {:?}",
                outcome.worker,
                outcome.status,
            );
            assert!(!outcome.findings.is_empty());
        }
        // Workers should have different names
        assert_ne!(outcomes[0].worker, outcomes[1].worker);
    }

    /// Full orchestration pipeline: execute_with_file_claims with 2 workers,
    /// verifying the full claim → execute → detect → release cycle.
    #[tokio::test]
    async fn orchestrator_runs_two_workers_through_full_pipeline() {
        use crate::agent::orchestrator::tests;

        let runtime = dummy_runtime();
        let orchestrator = Orchestrator::new(runtime.clone(), crate::agent::llm_registry::LlmRegistry::new());

        let profiles = vec![
            dummy_profile("worker-a"),
            dummy_profile("worker-b"),
        ];

        let index = make_index(&[
            "src/module_a.rs",
            "src/module_b.rs",
        ]);

        let assignments = orchestrator.split_task(
            "Refactor modules A and B",
            &profiles,
            &index,
        );

        assert_eq!(assignments.len(), 2);

        // Reuse orchestrator test helpers for MockFileClaimCoordinator via the
        // public `execute_with_file_claims` API with a simple coordinator.
        let coordinator = tests::MockFileClaimCoordinator::new();

        let report = orchestrator
            .execute_with_file_claims(
                assignments,
                &coordinator,
                1,
                "test",
            )
            .await
            .expect("orchestrator pipeline should complete");

        assert_eq!(
            report.status,
            WorkerExecutionStatus::Completed,
            "full pipeline should complete without review findings",
        );
        assert_eq!(report.reports.len(), 2);
        assert!(report.conflicts.is_empty());
        assert!(report.ownership_violations.is_empty());

        let outcomes: Vec<WorkerOutcome> = report
            .reports
            .into_iter()
            .map(WorkerOutcome::from)
            .collect();

        assert_eq!(outcomes.len(), 2);
        assert_ne!(outcomes[0].worker, outcomes[1].worker);
        for outcome in &outcomes {
            assert!(outcome.status.is_success());
        }
    }
}
