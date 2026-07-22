use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use zhongshu_core::agent::llm_registry::LlmRegistry;
use zhongshu_core::agent::orchestrator::OrganizationFileScope;
use zhongshu_core::agent::run::RunController;
use zhongshu_core::agent::{
    AgentProfile, AgentRuntime, AutoDelegationDecision, AutoDelegationPlanner, CollaborationMode,
    EmployeeCapability, EmployeeRole, Orchestrator, OrganizationExecutionReport,
    OrganizationExecutionStatus, OrganizationTaskRequest, RoleRequirement, StaffingRequest,
};
use zhongshu_core::core::{
    DurableExecutionRunner, ExecutionGraphStore, ExternalFactAssessment,
    MutationRecoveryCoordinator,
};
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, OrganizationEvent, ResponseTx,
};

use crate::app::publish_harness_events;
use crate::delegation_service::emit_assistant_message;
use crate::overlay_contract::{
    OrganizationEmployeeInfo, OrganizationGraphView, OrganizationRecoveryAction,
    OrganizationRecoveryCommand, OrganizationRecoveryResult, OrganizationRoleCommand,
    OrganizationTaskCommand,
};

const DAG_CONTROL_RECENT_LIMIT: usize = 8;

/// Desktop controller for general, structured, read-only organization tasks.
/// Mutation-capable profiles remain visible but are rejected by the core
/// read-only gate before any model call.
pub struct OrganizationController {
    runtime: Arc<RwLock<AgentRuntime>>,
    planner_runtime: Arc<RwLock<AgentRuntime>>,
    roster: Vec<AgentProfile>,
    event_bus: Arc<EventBus>,
    response_tx: ResponseTx,
    run_controller: Arc<RunController>,
    busy: Arc<AtomicBool>,
    current_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    current_session: Arc<Mutex<Option<String>>>,
    cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    checkpoint_store: Option<zhongshu_core::core::checkpoint::OrganizationCheckpointStore>,
    workspace_root: PathBuf,
    deeplossless: Arc<tokio::sync::Mutex<zhongshu_core::integration::DeeplosslessProxy>>,
}

impl OrganizationController {
    pub fn new(
        runtime: Arc<RwLock<AgentRuntime>>,
        planner_runtime: Arc<RwLock<AgentRuntime>>,
        roster: Vec<AgentProfile>,
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        run_controller: Arc<RunController>,
        checkpoint_store: Option<zhongshu_core::core::checkpoint::OrganizationCheckpointStore>,
        workspace_root: PathBuf,
        deeplossless: Arc<tokio::sync::Mutex<zhongshu_core::integration::DeeplosslessProxy>>,
    ) -> Self {
        Self {
            runtime,
            planner_runtime,
            roster,
            event_bus,
            response_tx,
            run_controller,
            busy: Arc::new(AtomicBool::new(false)),
            current_task: Arc::new(Mutex::new(None)),
            current_session: Arc::new(Mutex::new(None)),
            cancel_token: Arc::new(Mutex::new(None)),
            checkpoint_store,
            workspace_root,
            deeplossless,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub async fn employees(&self) -> Vec<OrganizationEmployeeInfo> {
        let runtime = self.runtime.read().await.clone();
        let eligibility = Orchestrator::new(runtime, LlmRegistry::new());
        self.roster
            .iter()
            .map(|profile| {
                let blocked_by = eligibility.organization_read_only_blocker(profile);
                let sandbox_blocked_by = eligibility.organization_sandbox_blocker(profile);
                OrganizationEmployeeInfo {
                    name: profile.name.clone(),
                    role: profile.specialty.role.as_str().to_string(),
                    capabilities: profile
                        .specialty
                        .capabilities
                        .iter()
                        .map(|capability| capability.as_str().to_string())
                        .collect(),
                    focus: profile.specialty.focus.clone(),
                    read_only_eligible: blocked_by.is_none(),
                    blocked_by,
                    sandbox_eligible: sandbox_blocked_by.is_none(),
                    sandbox_blocked_by,
                }
            })
            .collect()
    }

    pub async fn plan_automatic(&self, objective: &str) -> AutoDelegationDecision {
        let runtime = self.runtime.read().await.clone();
        let planner_runtime = self.planner_runtime.read().await.clone();
        let eligibility = Orchestrator::new(runtime.clone(), LlmRegistry::new());
        let eligible_roster = self
            .roster
            .iter()
            .filter(|profile| {
                eligibility
                    .organization_read_only_blocker(profile)
                    .is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        AutoDelegationPlanner::decide(&planner_runtime, objective, &eligible_roster).await
    }

    pub async fn update_planner_runtime(
        &self,
        provider: Arc<dyn zhongshu_core::agent::llm::LlmProvider>,
        model: String,
        reasoning_effort: Option<String>,
    ) {
        let mut runtime = self.planner_runtime.write().await;
        runtime.provider = provider;
        runtime.model = model;
        runtime.reasoning_effort = reasoning_effort;
    }

    pub async fn recovery_graphs(&self) -> anyhow::Result<Vec<OrganizationGraphView>> {
        let store = self
            .checkpoint_store
            .clone()
            .ok_or_else(|| anyhow::anyhow!("organization graph store is not configured"))?;
        recovery_graphs_from_store(store).await
    }

    pub async fn recover_graph(
        &self,
        command: OrganizationRecoveryCommand,
    ) -> anyhow::Result<OrganizationRecoveryResult> {
        let store = self
            .checkpoint_store
            .clone()
            .ok_or_else(|| anyhow::anyhow!("organization graph store is not configured"))?;
        let proxy = self.deeplossless.lock().await;
        recover_graph_with_proxy(&self.workspace_root, store, &proxy, command).await
    }

    pub fn submit(&self, command: OrganizationTaskCommand) -> bool {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        let task_id = format!("organization-{}", uuid::Uuid::new_v4());
        *self.current_session.lock().unwrap() = Some(task_id.clone());
        let request = organization_request(task_id.clone(), &command);
        let objective = command.objective.clone();
        let runtime = self.runtime.clone();
        let roster = self.roster.clone();
        let event_bus = self.event_bus.clone();
        let response_tx = self.response_tx.clone();
        let run_controller = self.run_controller.clone();
        let run_id = run_controller.start_run(&objective);
        let busy = self.busy.clone();
        let current_task = self.current_task.clone();
        let current_session = self.current_session.clone();
        let cancel_token = self.cancel_token.clone();
        let checkpoint_store = self.checkpoint_store.clone();
        // Save checkpoint before spawning so crash recovery can find it.
        if let Some(ref cs) = checkpoint_store {
            let staffing_json =
                serde_json::to_string(&request.staffing).unwrap_or_else(|_| "{}".into());
            let roster_json = serde_json::to_string(&self.roster).unwrap_or_else(|_| "[]".into());
            if let Err(e) = cs.save(&task_id, &objective, &staffing_json, &roster_json) {
                tracing::warn!(error = %e, "failed to save organization checkpoint");
            }
        }
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();

        event_bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));

        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            let runtime = runtime.read().await.clone();
            let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
            let cancel = CancellationToken::new();
            *cancel_token.lock().unwrap() = Some(cancel.clone());
            let result = if let Some(store) = checkpoint_store.clone() {
                let organization_bus = event_bus.clone();
                orchestrator
                    .execute_organization_task_with_events_durable(
                        &request,
                        &roster,
                        move |event| {
                            organization_bus.publish(Event::Organization(event));
                        },
                        Some(cancel),
                        store,
                    )
                    .await
            } else {
                let organization_bus = event_bus.clone();
                orchestrator
                    .execute_organization_task_with_events(
                        &request,
                        &roster,
                        move |event| {
                            organization_bus.publish(Event::Organization(event));
                        },
                        Some(cancel),
                    )
                    .await
            };

            let (message, final_state, stop_reason) = match result {
                Ok(report) => {
                    publish_harness_events(&event_bus, &report.trace_events);
                    let state = match report.status {
                        OrganizationExecutionStatus::Completed => {
                            AgentState::Done { success: true }
                        }
                        OrganizationExecutionStatus::Submitted
                        | OrganizationExecutionStatus::AwaitingManager => AgentState::Submitted,
                        OrganizationExecutionStatus::Blocked
                        | OrganizationExecutionStatus::Cancelled
                        | OrganizationExecutionStatus::WorkerFailed => {
                            AgentState::Done { success: false }
                        }
                    };
                    let reason = match report.status {
                        OrganizationExecutionStatus::Completed => "completed_verified",
                        OrganizationExecutionStatus::Submitted
                        | OrganizationExecutionStatus::AwaitingManager => "completed_unverified",
                        OrganizationExecutionStatus::Blocked => "blocked",
                        OrganizationExecutionStatus::Cancelled => "cancelled",
                        OrganizationExecutionStatus::WorkerFailed => "failed",
                    };
                    (organization_summary(&report), state, reason)
                }
                Err(error) => {
                    event_bus.publish(Event::Organization(OrganizationEvent::TaskFinished {
                        task_id: task_id.clone(),
                        status: "worker_failed".into(),
                        reason: Some(error.to_string()),
                    }));
                    (
                        format!("组织任务执行失败：{error}"),
                        AgentState::Done { success: false },
                        "failed",
                    )
                }
            };

            emit_assistant_message(&response_tx, run_id, &message).await;
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking,
                to: final_state,
            }));
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: final_state,
                to: AgentState::Idle,
            }));
            run_controller.finish_run(stop_reason).await;
            // Clean up the organization checkpoint on normal completion.
            if let Some(ref cs) = checkpoint_store {
                if let Err(e) = cs.delete(&task_id) {
                    tracing::warn!(error = %e, "failed to delete organization checkpoint");
                }
            }
            current_task.lock().unwrap().take();
            current_session.lock().unwrap().take();
            busy.store(false, Ordering::Release);
        });
        *self.current_task.lock().unwrap() = Some(handle);
        let _ = start_tx.send(());
        true
    }

    pub fn submit_mutation(&self, mut command: OrganizationTaskCommand) -> bool {
        if !command.mutation {
            return self.submit(command);
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let task_id = format!("mutation-{}", uuid::Uuid::new_v4());
        *self.current_session.lock().unwrap() = Some(task_id.clone());
        command.mutation = true;
        let request = organization_request(task_id.clone(), &command);
        let file_scopes = command
            .file_scopes
            .iter()
            .map(|scope| OrganizationFileScope {
                employee: scope.employee.clone(),
                owned_files: scope.owned_files.iter().map(PathBuf::from).collect(),
            })
            .collect::<Vec<_>>();
        let objective = command.objective.clone();
        let runtime = self.runtime.clone();
        let roster = self.roster.clone();
        let event_bus = self.event_bus.clone();
        let response_tx = self.response_tx.clone();
        let run_controller = self.run_controller.clone();
        let run_id = run_controller.start_run(&objective);
        let busy = self.busy.clone();
        let current_task = self.current_task.clone();
        let current_session = self.current_session.clone();
        let cancel_token = self.cancel_token.clone();
        let checkpoint_store = self.checkpoint_store.clone();
        let workspace_root = self.workspace_root.clone();
        let deeplossless = self.deeplossless.clone();
        if let Some(ref cs) = checkpoint_store {
            let sj = serde_json::to_string(&request.staffing).unwrap_or_default();
            let rj = serde_json::to_string(&self.roster).unwrap_or_default();
            if let Err(e) = cs.save(&task_id, &objective, &sj, &rj) {
                tracing::warn!(error = %e, "failed to save mutation checkpoint");
            }
        }
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        event_bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));
        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            let runtime = runtime.read().await.clone();
            let orchestrator = Orchestrator::new(runtime, LlmRegistry::new())
                .with_worker_workspace_root(workspace_root.clone());
            let cancel = CancellationToken::new();
            *cancel_token.lock().unwrap() = Some(cancel.clone());
            let result: anyhow::Result<String> = (async {
                let proxy = deeplossless.lock().await;
                let conv_id = proxy.current_conv_id().await.ok_or_else(|| {
                    anyhow::anyhow!("Deeplossless 当前没有可用于文件 claim 的 conversation")
                })?;
                let mut engine = zhongshu_core::patch::PatchEngine::new(&workspace_root)
                    .map_err(|e| anyhow::anyhow!("PatchEngine 初始化失败: {e}"))?;
                let report = if let Some(store) = checkpoint_store.clone() {
                    orchestrator
                        .execute_organization_mutation_from_workers_durable(
                            &request,
                            &roster,
                            file_scopes,
                            "zhongshu",
                            &*proxy,
                            conv_id,
                            "mutation",
                            &mut engine,
                            store,
                        )
                        .await?
                } else {
                    orchestrator
                        .execute_organization_mutation_from_workers(
                            &request,
                            &roster,
                            file_scopes,
                            "zhongshu",
                            &*proxy,
                            conv_id,
                            "mutation",
                            &mut engine,
                        )
                        .await?
                };
                if !report.can_finalize() {
                    let reasons = if report.manager_acceptance.reasons.is_empty() {
                        String::new()
                    } else {
                        format!("；{}", report.manager_acceptance.reasons.join("；"))
                    };
                    anyhow::bail!(
                        "mutation {} 未通过完成门：{}{}",
                        report.task_id,
                        report.manager_acceptance.summary,
                        reasons
                    );
                }
                Ok(format!(
                    "mutation {}: {}",
                    report.task_id, report.manager_acceptance.summary,
                ))
            })
            .await;
            let (message, final_state, stop_reason) = match result {
                Ok(summary) => (summary, AgentState::Done { success: true }, "completed"),
                Err(error) => {
                    event_bus.publish(Event::Organization(
                        zhongshu_core::event::OrganizationEvent::TaskFinished {
                            task_id: task_id.clone(),
                            status: "worker_failed".into(),
                            reason: Some(error.to_string()),
                        },
                    ));
                    (
                        format!("mutation 执行失败：{error}"),
                        AgentState::Done { success: false },
                        "failed",
                    )
                }
            };
            emit_assistant_message(&response_tx, run_id, &message).await;
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking,
                to: final_state,
            }));
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: final_state,
                to: AgentState::Idle,
            }));
            run_controller.finish_run(stop_reason).await;
            if let Some(ref cs) = checkpoint_store {
                let _ = cs.delete(&task_id);
            }
            current_task.lock().unwrap().take();
            current_session.lock().unwrap().take();
            busy.store(false, Ordering::Release);
        });
        *self.current_task.lock().unwrap() = Some(handle);
        let _ = start_tx.send(());
        true
    }

    pub fn cancel(&self) -> bool {
        let Some(handle) = self.current_task.lock().unwrap().take() else {
            return false;
        };
        // Trigger graceful cancellation first, then abort the task handle
        // as a hard fallback for workers that don't check the token.
        if let Some(token) = self.cancel_token.lock().unwrap().take() {
            token.cancel();
        }
        handle.abort();
        if let Some(ref cs) = self.checkpoint_store {
            // Clean up checkpoint on cancel (use the session task_id if available).
            if let Some(task_id) = self.current_session.lock().unwrap().as_ref() {
                if let Err(e) = cs.delete(task_id) {
                    tracing::warn!(error = %e, "failed to delete organization checkpoint on cancel");
                }
            }
        }
        if let Some(task_id) = self.current_session.lock().unwrap().take() {
            self.event_bus
                .publish(Event::Organization(OrganizationEvent::TaskFinished {
                    task_id,
                    status: "cancelled".into(),
                    reason: Some("cancelled by user".into()),
                }));
        }
        self.busy.store(false, Ordering::Release);
        let run_controller = self.run_controller.clone();
        tokio::spawn(async move {
            run_controller.finish_run("cancelled").await;
        });
        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking,
                to: AgentState::Done { success: false },
            }));
        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Done { success: false },
                to: AgentState::Idle,
            }));
        true
    }
}

async fn recovery_graphs_from_store(
    store: zhongshu_core::core::checkpoint::OrganizationCheckpointStore,
) -> anyhow::Result<Vec<OrganizationGraphView>> {
    let task_ids = store.list_recent_graphs(DAG_CONTROL_RECENT_LIMIT)?;
    let mut graphs = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        if let Some(stored) = store.load_graph(&task_id)? {
            graphs.push(OrganizationGraphView {
                store_version: stored.version,
                graph: stored.checkpoint.graph,
            });
        }
    }
    Ok(graphs)
}

async fn recover_graph_with_proxy(
    workspace_root: &std::path::Path,
    store: zhongshu_core::core::checkpoint::OrganizationCheckpointStore,
    proxy: &zhongshu_core::integration::DeeplosslessProxy,
    command: OrganizationRecoveryCommand,
) -> anyhow::Result<OrganizationRecoveryResult> {
    let runner = DurableExecutionRunner::new(store);
    let mut recovery = runner
        .recover(&command.task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("execution graph '{}' was not found", command.task_id))?;
    let node = recovery
        .graph
        .node(&command.node_id)
        .ok_or_else(|| anyhow::anyhow!("recovery node '{}' was not found", command.node_id))?;
    if node.state != zhongshu_core::agent::ExecutionNodeState::RecoveryRequired {
        anyhow::bail!(
            "node '{}' is {:?}, not recovery_required",
            command.node_id,
            node.state
        );
    }

    let coordinator = MutationRecoveryCoordinator::new(workspace_root, proxy);
    let action = command.action;
    match action {
        OrganizationRecoveryAction::Reconcile => {
            let evidence = coordinator
                .assess(&recovery.graph, &command.node_id)
                .await?;
            if evidence.assessment == ExternalFactAssessment::Inconclusive {
                return Ok(OrganizationRecoveryResult {
                    task_id: command.task_id.clone(),
                    node_id: command.node_id,
                    action,
                    assessment: assessment_name(evidence.assessment).into(),
                    reason: evidence.reason,
                    evidence_refs: evidence.evidence_refs,
                    executed_cleanup_nodes: Vec::new(),
                    graph: OrganizationGraphView {
                        store_version: recovery.store_version,
                        graph: recovery.graph.snapshot(),
                    },
                });
            }
            let progress = coordinator
                .reconcile_evidence_and_continue(&runner, &mut recovery, evidence)
                .await?;
            Ok(recovery_result(command, action, progress))
        }
        OrganizationRecoveryAction::Abandon => {
            let reason = command
                .reason
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("abandon recovery requires a reason"))?;
            let decision_id = uuid::Uuid::new_v4().to_string();
            let progress = coordinator
                .abandon_and_continue(
                    &runner,
                    &mut recovery,
                    &command.node_id,
                    &decision_id,
                    reason,
                )
                .await?;
            Ok(recovery_result(command, action, progress))
        }
    }
}

fn assessment_name(assessment: ExternalFactAssessment) -> &'static str {
    match assessment {
        ExternalFactAssessment::ConfirmedSucceeded => "confirmed_succeeded",
        ExternalFactAssessment::ConfirmedFailed => "confirmed_failed",
        ExternalFactAssessment::Inconclusive => "inconclusive",
    }
}

fn recovery_result(
    command: OrganizationRecoveryCommand,
    action: OrganizationRecoveryAction,
    progress: zhongshu_core::core::MutationRecoveryProgress,
) -> OrganizationRecoveryResult {
    OrganizationRecoveryResult {
        task_id: command.task_id,
        node_id: command.node_id,
        action,
        assessment: assessment_name(progress.evidence.assessment).into(),
        reason: progress.evidence.reason,
        evidence_refs: progress.evidence.evidence_refs,
        executed_cleanup_nodes: progress.executed_cleanup_nodes,
        graph: OrganizationGraphView {
            store_version: progress.store_version,
            graph: progress.graph,
        },
    }
}

fn organization_request(
    task_id: String,
    command: &OrganizationTaskCommand,
) -> OrganizationTaskRequest {
    let staffing = StaffingRequest {
        objective: command.objective.clone(),
        requirements: command.requirements.iter().map(role_requirement).collect(),
        max_workers: command.max_workers,
    };
    let mut request = match command.target_employee.as_deref() {
        Some(employee) => OrganizationTaskRequest::user_to_employee(task_id, employee, staffing),
        None => OrganizationTaskRequest::manager_selected(task_id, "中书", staffing),
    };
    request.collaboration = if command.sequential_handoff {
        CollaborationMode::SequentialHandoff
    } else {
        CollaborationMode::Independent
    };
    request.workspace_mode = command.workspace_mode;
    request
}

fn role_requirement(command: &OrganizationRoleCommand) -> RoleRequirement {
    RoleRequirement {
        role: EmployeeRole::new(&command.role),
        employee: command.employee.clone(),
        capabilities: command
            .capabilities
            .iter()
            .map(EmployeeCapability::new)
            .collect(),
        responsibility: command.responsibility.clone(),
        required: command.required,
    }
}

fn organization_summary(report: &OrganizationExecutionReport) -> String {
    let mut text = format!("组织任务状态：{}", status_label(report.status));
    for employee in &report.employee_reports {
        text.push_str(&format!(
            "\n\n{}（{}）向 {} 汇报：\n{}",
            employee.assignment.employee,
            employee.assignment.role.as_str(),
            authority_label(&employee.reports_to),
            employee.report.summary
        ));
    }
    for unfilled in &report.staffing.unfilled {
        text.push_str(&format!(
            "\n\n未配置岗位 {}：{}",
            unfilled.role.as_str(),
            unfilled.reason
        ));
    }
    for reason in &report.staffing.rationale {
        text.push_str("\n- ");
        text.push_str(reason);
    }
    if let Some(error) = &report.execution_error {
        text.push_str("\n\n执行错误：");
        text.push_str(error);
    }
    text
}

fn status_label(status: OrganizationExecutionStatus) -> &'static str {
    match status {
        OrganizationExecutionStatus::AwaitingManager => "等待经理处理",
        OrganizationExecutionStatus::Blocked => "已阻塞",
        OrganizationExecutionStatus::Completed => "经理验收通过",
        OrganizationExecutionStatus::Submitted => "员工已提交，尚未满足验收条件",
        OrganizationExecutionStatus::Cancelled => "用户已取消",
        OrganizationExecutionStatus::WorkerFailed => "员工执行失败",
    }
}

fn authority_label(authority: &zhongshu_core::agent::AssignmentAuthority) -> &str {
    match authority {
        zhongshu_core::agent::AssignmentAuthority::Manager { manager } => manager,
        zhongshu_core::agent::AssignmentAuthority::User => "用户",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_crashed_orb_recovery_fixture(
        directory: &std::path::Path,
    ) -> zhongshu_core::integration::DeeplosslessProxy {
        use zhongshu_core::agent::{
            ExecutionEdge, ExecutionEdgeKind, ExecutionGraph, ExecutionNode, ExecutionNodeKind,
        };
        use zhongshu_core::core::{file_claim_effect_intents, workspace_effect_intents};
        use zhongshu_core::integration::{DeeplosslessConfig, DeeplosslessFileClaimOutcome};
        use zhongshu_core::patch::{content_hash, PatchFileEffectPlan};

        let workspace = directory.join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), "new\n").unwrap();
        let database = zhongshu_core::core::Database::new(directory.join("core.db"));
        database.migrate().unwrap();
        let store = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(database);
        let mut proxy = zhongshu_core::integration::DeeplosslessProxy::new(DeeplosslessConfig {
            db_path: directory.join("lcm.db").display().to_string(),
            proxy_port: 0,
            ..Default::default()
        })
        .await
        .unwrap();
        proxy.start(0).await.unwrap();
        assert!(matches!(
            proxy
                .claim_file("worker", "src/lib.rs", "edit", 1)
                .await
                .unwrap(),
            DeeplosslessFileClaimOutcome::Claimed { .. }
        ));

        let mut graph = ExecutionGraph::new("orb-process-recovery").unwrap();
        for (id, kind) in [
            ("claim", ExecutionNodeKind::Claim),
            ("apply", ExecutionNodeKind::Apply),
            ("release", ExecutionNodeKind::Release),
            ("finalize", ExecutionNodeKind::Finalize),
        ] {
            graph
                .add_node(ExecutionNode::pending(id, kind, id))
                .unwrap();
        }
        for (from, to, kind) in [
            ("claim", "apply", ExecutionEdgeKind::Requires),
            ("apply", "release", ExecutionEdgeKind::Finally),
            ("apply", "finalize", ExecutionEdgeKind::Requires),
            ("release", "finalize", ExecutionEdgeKind::Requires),
        ] {
            graph
                .add_edge(ExecutionEdge {
                    from: from.into(),
                    to: to.into(),
                    kind,
                })
                .unwrap();
        }
        graph
            .record_effect_intents(
                "claim",
                file_claim_effect_intents(
                    "claim",
                    vec![("worker".into(), "src/lib.rs".into(), "edit".into(), 1)],
                    true,
                ),
            )
            .unwrap();
        graph.start_node("claim").unwrap();
        graph.complete_node("claim", Vec::new()).unwrap();
        graph
            .record_effect_intents(
                "apply",
                workspace_effect_intents(
                    "apply",
                    &[PatchFileEffectPlan {
                        path: PathBuf::from("src/lib.rs"),
                        before_hash: content_hash("old\n"),
                        after_hash: content_hash("new\n"),
                        existed_before: true,
                    }],
                ),
            )
            .unwrap();
        graph.start_node("apply").unwrap();
        store.save_graph_cas(&graph.checkpoint(), 0).unwrap();
        proxy
    }

    #[test]
    fn command_conversion_preserves_dynamic_roles_and_handoff() {
        let command = OrganizationTaskCommand {
            objective: "复核现金流".into(),
            requirements: vec![OrganizationRoleCommand {
                role: "管理会计".into(),
                employee: Some("accountant".into()),
                capabilities: vec!["现金流预测".into()],
                responsibility: "提交预测".into(),
                required: true,
            }],
            sequential_handoff: true,
            max_workers: Some(1),
            target_employee: None,
            mutation: false,
            workspace_mode: zhongshu_core::agent::WorkerWorkspaceMode::ProposalOnly,
            file_scopes: Vec::new(),
        };

        let request = organization_request("org-1".into(), &command);

        assert_eq!(request.staffing.requirements[0].role.as_str(), "管理会计");
        assert_eq!(
            request.staffing.requirements[0].capabilities[0].as_str(),
            "现金流预测"
        );
        assert_eq!(request.collaboration, CollaborationMode::SequentialHandoff);
    }

    #[test]
    fn mutation_flag_produces_same_staffing_as_read_only() {
        let ro = OrganizationTaskCommand {
            objective: "修改配置".into(),
            requirements: vec![OrganizationRoleCommand {
                role: "后端".into(),
                employee: Some("backend".into()),
                capabilities: vec![],
                responsibility: "修改 API".into(),
                required: true,
            }],
            sequential_handoff: false,
            max_workers: Some(1),
            target_employee: None,
            mutation: false,
            workspace_mode: zhongshu_core::agent::WorkerWorkspaceMode::ProposalOnly,
            file_scopes: Vec::new(),
        };
        let mut mu = ro.clone();
        mu.mutation = true;
        mu.workspace_mode = zhongshu_core::agent::WorkerWorkspaceMode::IsolatedSandbox;
        let req_ro = organization_request("ro".into(), &ro);
        let req_mu = organization_request("mu".into(), &mu);
        // Staffing should be identical; only the execution path differs
        assert_eq!(
            req_ro.staffing.requirements.len(),
            req_mu.staffing.requirements.len()
        );
        assert_eq!(
            req_ro.staffing.requirements[0].role,
            req_mu.staffing.requirements[0].role
        );
        assert_eq!(req_ro.collaboration, req_mu.collaboration);
        assert_eq!(
            req_mu.workspace_mode,
            zhongshu_core::agent::WorkerWorkspaceMode::IsolatedSandbox
        );
    }

    #[test]
    fn cancelled_organization_has_distinct_user_visible_status() {
        assert_eq!(
            status_label(OrganizationExecutionStatus::Cancelled),
            "用户已取消"
        );
    }

    #[tokio::test]
    async fn control_plane_listing_does_not_reclassify_an_active_running_node() {
        use zhongshu_core::agent::{ExecutionGraph, ExecutionNode, ExecutionNodeKind};

        let db_path = std::env::temp_dir().join(format!(
            "zhongshu-dag-control-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let database = zhongshu_core::core::Database::new(db_path);
        database.migrate().unwrap();
        let store = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(database);
        let runner = DurableExecutionRunner::new(store.clone());
        let mut graph = ExecutionGraph::new("control-recovery").unwrap();
        graph
            .add_node(ExecutionNode::pending(
                "apply",
                ExecutionNodeKind::Apply,
                "apply patch",
            ))
            .unwrap();
        let mut version = runner.initialize(&graph).await.unwrap();
        runner
            .admit_node(&mut graph, &mut version, "apply")
            .await
            .unwrap();

        let views = recovery_graphs_from_store(store.clone()).await.unwrap();

        assert_eq!(views.len(), 1);
        assert_eq!(
            views[0].graph.nodes[0].state,
            zhongshu_core::agent::ExecutionNodeState::Running
        );
        let stored = store.load_graph("control-recovery").unwrap().unwrap();
        assert_eq!(stored.version, views[0].store_version);
        assert_eq!(
            stored.checkpoint.graph.nodes[0].state,
            zhongshu_core::agent::ExecutionNodeState::Running
        );
    }

    #[tokio::test]
    async fn orb_recovery_adapter_reopens_store_and_uses_real_shared_proxy_cleanup() {
        use zhongshu_core::agent::{
            ExecutionEdge, ExecutionEdgeKind, ExecutionGraph, ExecutionNode, ExecutionNodeKind,
        };
        use zhongshu_core::core::{file_claim_effect_intents, workspace_effect_intents};
        use zhongshu_core::integration::{DeeplosslessConfig, DeeplosslessFileClaimOutcome};
        use zhongshu_core::patch::{content_hash, PatchFileEffectPlan};

        let directory = std::env::temp_dir().join(format!(
            "zhongshu-orb-recovery-canary-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let workspace = directory.join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), "new\n").unwrap();
        let graph_db_path = directory.join("core.db");
        let database = zhongshu_core::core::Database::new(graph_db_path.clone());
        database.migrate().unwrap();
        let store = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(database);

        let mut proxy = zhongshu_core::integration::DeeplosslessProxy::new(DeeplosslessConfig {
            db_path: directory.join("lcm.db").display().to_string(),
            proxy_port: 0,
            ..Default::default()
        })
        .await
        .unwrap();
        proxy.start(0).await.unwrap();
        assert!(matches!(
            proxy
                .claim_file("worker", "src/lib.rs", "edit", 1)
                .await
                .unwrap(),
            DeeplosslessFileClaimOutcome::Claimed { .. }
        ));

        let mut graph = ExecutionGraph::new("orb-recovery-canary").unwrap();
        for (id, kind) in [
            ("claim", ExecutionNodeKind::Claim),
            ("apply", ExecutionNodeKind::Apply),
            ("release", ExecutionNodeKind::Release),
            ("finalize", ExecutionNodeKind::Finalize),
        ] {
            graph
                .add_node(ExecutionNode::pending(id, kind, id))
                .unwrap();
        }
        for (from, to, kind) in [
            ("claim", "apply", ExecutionEdgeKind::Requires),
            ("apply", "release", ExecutionEdgeKind::Finally),
            ("apply", "finalize", ExecutionEdgeKind::Requires),
            ("release", "finalize", ExecutionEdgeKind::Requires),
        ] {
            graph
                .add_edge(ExecutionEdge {
                    from: from.into(),
                    to: to.into(),
                    kind,
                })
                .unwrap();
        }
        graph
            .record_effect_intents(
                "claim",
                file_claim_effect_intents(
                    "claim",
                    vec![("worker".into(), "src/lib.rs".into(), "edit".into(), 1)],
                    true,
                ),
            )
            .unwrap();
        graph.start_node("claim").unwrap();
        graph.complete_node("claim", Vec::new()).unwrap();
        graph
            .record_effect_intents(
                "apply",
                workspace_effect_intents(
                    "apply",
                    &[PatchFileEffectPlan {
                        path: PathBuf::from("src/lib.rs"),
                        before_hash: content_hash("old\n"),
                        after_hash: content_hash("new\n"),
                        existed_before: true,
                    }],
                ),
            )
            .unwrap();
        graph.start_node("apply").unwrap();
        store.save_graph_cas(&graph.checkpoint(), 0).unwrap();
        drop(store);

        let reopened = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(
            zhongshu_core::core::Database::new(graph_db_path),
        );
        let result = recover_graph_with_proxy(
            &workspace,
            reopened.clone(),
            &proxy,
            OrganizationRecoveryCommand {
                task_id: "orb-recovery-canary".into(),
                node_id: "apply".into(),
                action: OrganizationRecoveryAction::Reconcile,
                reason: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.assessment, "confirmed_succeeded");
        assert_eq!(result.executed_cleanup_nodes, vec!["release", "finalize"]);
        assert!(result
            .graph
            .graph
            .nodes
            .iter()
            .all(|node| node.state.is_terminal()));
        assert!(proxy.file_claim_facts().await.unwrap().is_empty());
        assert!(reopened.list_unfinished_graphs().unwrap().is_empty());
        proxy.shutdown().await;
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn orb_recovery_process_child() {
        let Ok(directory) = std::env::var("ZHONGSHU_ORB_RECOVERY_CHILD_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let _proxy = seed_crashed_orb_recovery_fixture(&directory).await;
        std::fs::write(directory.join("effect-persisted.marker"), b"ready").unwrap();
        std::future::pending::<()>().await;
    }

    #[tokio::test]
    async fn kill_restart_orb_adapter_reuses_persisted_claim_and_finishes_cleanup() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        use zhongshu_core::integration::DeeplosslessConfig;

        let directory = std::env::temp_dir().join(format!(
            "zhongshu-orb-process-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("organization_service::tests::orb_recovery_process_child")
            .arg("--nocapture")
            .env("ZHONGSHU_ORB_RECOVERY_CHILD_DIR", &directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let marker = directory.join("effect-persisted.marker");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !marker.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not persist the external effect and Running checkpoint");
        }
        child.kill().unwrap();
        child.wait().unwrap();

        let mut proxy = zhongshu_core::integration::DeeplosslessProxy::new(DeeplosslessConfig {
            db_path: directory.join("lcm.db").display().to_string(),
            proxy_port: 0,
            ..Default::default()
        })
        .await
        .unwrap();
        proxy.start(0).await.unwrap();
        let store = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(
            zhongshu_core::core::Database::new(directory.join("core.db")),
        );
        let result = recover_graph_with_proxy(
            &directory.join("workspace"),
            store.clone(),
            &proxy,
            OrganizationRecoveryCommand {
                task_id: "orb-process-recovery".into(),
                node_id: "apply".into(),
                action: OrganizationRecoveryAction::Reconcile,
                reason: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.assessment, "confirmed_succeeded");
        assert_eq!(result.executed_cleanup_nodes, vec!["release", "finalize"]);
        assert!(proxy.file_claim_facts().await.unwrap().is_empty());
        assert!(store.list_unfinished_graphs().unwrap().is_empty());
        proxy.shutdown().await;
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
