use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;
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
}

impl OrganizationController {
    pub fn new(
        runtime: Arc<RwLock<AgentRuntime>>,
        roster: Vec<AgentProfile>,
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        run_controller: Arc<RunController>,
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
            let result = orchestrator
                .execute_organization_task_with_events(&request, &roster, move |event| {
                    organization_bus.publish(Event::Organization(event));
                })
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
        handle.abort();
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
        };

        let request = organization_request("org-1".into(), &command);

        assert_eq!(request.staffing.requirements[0].role.as_str(), "管理会计");
        assert_eq!(
            request.staffing.requirements[0].capabilities[0].as_str(),
            "现金流预测"
        );
        assert_eq!(request.collaboration, CollaborationMode::SequentialHandoff);
    }
}
