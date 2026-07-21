use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use zhongshu_core::agent::orchestrator::OrganizationFileScope;
use zhongshu_core::agent::llm_registry::LlmRegistry;
use zhongshu_core::agent::run::RunController;
use zhongshu_core::agent::{
    AgentProfile, AgentRuntime, CollaborationMode, EmployeeCapability, EmployeeRole, Orchestrator,
    OrganizationExecutionReport, OrganizationExecutionStatus, OrganizationTaskRequest,
    RoleRequirement, StaffingRequest,
};
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, OrganizationEvent, ResponseTx,
};

use crate::app::publish_harness_events;
use crate::delegation_service::emit_assistant_message;
use crate::overlay_contract::{
    OrganizationEmployeeInfo, OrganizationRoleCommand, OrganizationTaskCommand,
};

/// Desktop controller for general, structured, read-only organization tasks.
/// Mutation-capable profiles remain visible but are rejected by the core
/// read-only gate before any model call.
pub struct OrganizationController {
    runtime: Arc<RwLock<AgentRuntime>>,
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
    deeplossless_config: Option<zhongshu_core::integration::DeeplosslessConfig>,
}

impl OrganizationController {
    pub fn new(
        runtime: Arc<RwLock<AgentRuntime>>,
        roster: Vec<AgentProfile>,
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        run_controller: Arc<RunController>,
        checkpoint_store: Option<zhongshu_core::core::checkpoint::OrganizationCheckpointStore>,
        workspace_root: PathBuf,
        deeplossless_config: Option<zhongshu_core::integration::DeeplosslessConfig>,
    ) -> Self {
        Self {
            runtime,
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
            deeplossless_config,
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
                }
            })
            .collect()
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
            let roster_json =
                serde_json::to_string(&self.roster).unwrap_or_else(|_| "[]".into());
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
            let organization_bus = event_bus.clone();
            let cancel = CancellationToken::new();
            *cancel_token.lock().unwrap() = Some(cancel.clone());
            let result = orchestrator
                .execute_organization_task_with_events(
                    &request,
                    &roster,
                    move |event| {
                        organization_bus.publish(Event::Organization(event));
                    },
                    Some(cancel),
                )
                .await;

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
                        | OrganizationExecutionStatus::WorkerFailed => {
                            AgentState::Done { success: false }
                        }
                    };
                    let reason = match report.status {
                        OrganizationExecutionStatus::Completed => "completed_verified",
                        OrganizationExecutionStatus::Submitted
                        | OrganizationExecutionStatus::AwaitingManager => "completed_unverified",
                        OrganizationExecutionStatus::Blocked => "blocked",
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
        if !command.mutation { return self.submit(command); }
        if self.busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return false; }
        let task_id = format!("mutation-{}", uuid::Uuid::new_v4());
        *self.current_session.lock().unwrap() = Some(task_id.clone());
        command.mutation = true;
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
        let workspace_root = self.workspace_root.clone();
        let dl_config = self.deeplossless_config.clone();
        if let Some(ref cs) = checkpoint_store {
            let sj = serde_json::to_string(&request.staffing).unwrap_or_default();
            let rj = serde_json::to_string(&self.roster).unwrap_or_default();
            if let Err(e) = cs.save(&task_id, &objective, &sj, &rj) {
                tracing::warn!(error = %e, "failed to save mutation checkpoint");
            }
        }
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        event_bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle, to: AgentState::Thinking,
        }));
        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            let runtime = runtime.read().await.clone();
            let orchestrator = Orchestrator::new(runtime, LlmRegistry::new());
            let cancel = CancellationToken::new();
            *cancel_token.lock().unwrap() = Some(cancel.clone());
            // Build file scopes from roster
            let file_scopes: Vec<OrganizationFileScope> = roster.iter().map(|p| {
                OrganizationFileScope {
                    employee: p.name.clone(),
                    owned_files: vec![], // User must specify file scopes in future iterations
                }
            }).collect();
            // For MVP, use simplified deeplossless proxy if configured
            let result: anyhow::Result<String> = (async {
                let cfg = dl_config.ok_or_else(|| anyhow::anyhow!("Deeplossless 未配置，mutation 需要文件协调服务"))?;
                let proxy = zhongshu_core::integration::DeeplosslessProxy::new(cfg).await?;
                let conv_id = 0i64;
                let mut engine = zhongshu_core::patch::PatchEngine::new(&workspace_root).map_err(|e| anyhow::anyhow!("PatchEngine 初始化失败: {e}"))?;
                let report = orchestrator
                    .execute_organization_mutation_from_workers(
                        &request, &roster, file_scopes, "zhongshu",
                        &proxy, conv_id, "mutation", &mut engine,
                    )
                    .await?;
                let summary = format!(
                    "mutation {}: {}",
                    report.task_id,
                    report.manager_acceptance.summary,
                );
                Ok(summary)
            }).await;
            let (message, final_state, stop_reason) = match result {
                Ok(summary) => (summary, AgentState::Done { success: true }, "completed"),
                Err(error) => {
                    event_bus.publish(Event::Organization(zhongshu_core::event::OrganizationEvent::TaskFinished {
                        task_id: task_id.clone(),
                        status: "worker_failed".into(),
                        reason: Some(error.to_string()),
                    }));
                    (format!("mutation 执行失败：{error}"), AgentState::Done { success: false }, "failed")
                }
            };
            emit_assistant_message(&response_tx, run_id, &message).await;
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Thinking, to: final_state,
            }));
            event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: final_state, to: AgentState::Idle,
            }));
            run_controller.finish_run(stop_reason).await;
            if let Some(ref cs) = checkpoint_store { let _ = cs.delete(&task_id); }
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
    request
}

fn role_requirement(command: &OrganizationRoleCommand) -> RoleRequirement {
    RoleRequirement {
        role: EmployeeRole::new(&command.role),
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

    #[test]
    fn command_conversion_preserves_dynamic_roles_and_handoff() {
        let command = OrganizationTaskCommand {
            objective: "复核现金流".into(),
            requirements: vec![OrganizationRoleCommand {
                role: "管理会计".into(),
                capabilities: vec!["现金流预测".into()],
                responsibility: "提交预测".into(),
                required: true,
            }],
            sequential_handoff: true,
            max_workers: Some(1),
            target_employee: None,
            mutation: false,
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
                capabilities: vec![],
                responsibility: "修改 API".into(),
                required: true,
            }],
            sequential_handoff: false,
            max_workers: Some(1),
            target_employee: None,
            mutation: false,
        };
        let mut mu = ro.clone();
        mu.mutation = true;
        let req_ro = organization_request("ro".into(), &ro);
        let req_mu = organization_request("mu".into(), &mu);
        // Staffing should be identical; only the execution path differs
        assert_eq!(req_ro.staffing.requirements.len(), req_mu.staffing.requirements.len());
        assert_eq!(req_ro.staffing.requirements[0].role, req_mu.staffing.requirements[0].role);
        assert_eq!(req_ro.collaboration, req_mu.collaboration);
    }
}
