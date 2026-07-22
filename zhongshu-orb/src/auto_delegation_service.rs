use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use zhongshu_core::agent::{AutoDelegationStrategy, CollaborationMode};
use zhongshu_core::event::{Event, EventBus, OrganizationEvent};

use crate::app::AgentInbox;
use crate::organization_service::OrganizationController;
use crate::overlay_contract::{OrganizationRoleCommand, OrganizationTaskCommand};

/// Owns the short planning interval between the primary input and either the
/// regular inbox or the durable organization executor. It does not recursively
/// delegate and it never creates a mutation task automatically.
pub struct AutoDelegationController {
    organization: Arc<OrganizationController>,
    inbox: Arc<AgentInbox>,
    event_bus: Arc<EventBus>,
    busy: Arc<AtomicBool>,
    current_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AutoDelegationController {
    pub fn new(
        organization: Arc<OrganizationController>,
        inbox: Arc<AgentInbox>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            organization,
            inbox,
            event_bus,
            busy: Arc::new(AtomicBool::new(false)),
            current_task: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub fn submit(&self, objective: String) -> bool {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        let organization = self.organization.clone();
        let inbox = self.inbox.clone();
        let event_bus = self.event_bus.clone();
        let busy = self.busy.clone();
        let current_task = self.current_task.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            let routing_id = format!("auto-route-{}", uuid::Uuid::new_v4());
            let decision = organization.plan_automatic(&objective).await;
            event_bus.publish(Event::Organization(OrganizationEvent::RoutingDecided {
                routing_id: routing_id.clone(),
                strategy: decision.strategy.as_str().into(),
                reason: decision.reason.clone(),
                worker_count: decision.worker_count(),
            }));

            // Release the routing claim immediately before the synchronous
            // handoff. The selected executor has its own atomic busy gate.
            current_task.lock().unwrap().take();
            busy.store(false, Ordering::Release);

            match decision.strategy {
                AutoDelegationStrategy::SingleAgent => inbox.submit(objective),
                AutoDelegationStrategy::MultiAgent => {
                    let command = command_from_decision(objective.clone(), &decision);
                    if !organization.submit(command) {
                        let reason = "自动多 Agent 在交接时遇到并发任务，已回退主 AI";
                        event_bus.publish(Event::Organization(OrganizationEvent::RoutingDecided {
                            routing_id,
                            strategy: AutoDelegationStrategy::SingleAgent.as_str().into(),
                            reason: reason.into(),
                            worker_count: 0,
                        }));
                        inbox.submit(objective);
                    }
                }
            }
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
        self.busy.store(false, Ordering::Release);
        true
    }
}

fn command_from_decision(
    objective: String,
    decision: &zhongshu_core::agent::AutoDelegationDecision,
) -> OrganizationTaskCommand {
    OrganizationTaskCommand {
        objective,
        requirements: decision
            .staffing
            .requirements
            .iter()
            .map(|requirement| OrganizationRoleCommand {
                role: requirement.role.as_str().into(),
                employee: requirement.employee.clone(),
                capabilities: requirement
                    .capabilities
                    .iter()
                    .map(|capability| capability.as_str().into())
                    .collect(),
                responsibility: requirement.responsibility.clone(),
                required: requirement.required,
            })
            .collect(),
        sequential_handoff: decision.collaboration == CollaborationMode::SequentialHandoff,
        max_workers: decision.staffing.max_workers,
        target_employee: None,
        mutation: false,
        workspace_mode: zhongshu_core::agent::WorkerWorkspaceMode::ProposalOnly,
        file_scopes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zhongshu_core::agent::{
        AutoDelegationDecision, EmployeeCapability, EmployeeRole, RoleRequirement, StaffingRequest,
    };

    #[test]
    fn automatic_command_is_always_non_mutating_and_bounded() {
        let decision = AutoDelegationDecision {
            strategy: AutoDelegationStrategy::MultiAgent,
            reason: "test".into(),
            collaboration: CollaborationMode::Independent,
            staffing: StaffingRequest {
                objective: "report".into(),
                requirements: vec![
                    RoleRequirement {
                        role: EmployeeRole::new("researcher"),
                        employee: Some("researcher-1".into()),
                        capabilities: vec![EmployeeCapability::new("source_review")],
                        responsibility: "research".into(),
                        required: true,
                    },
                    RoleRequirement {
                        role: EmployeeRole::new("writer"),
                        employee: Some("writer-1".into()),
                        capabilities: vec![EmployeeCapability::new("synthesis")],
                        responsibility: "draft".into(),
                        required: true,
                    },
                ],
                max_workers: Some(2),
            },
        };
        let command = command_from_decision("report".into(), &decision);
        assert!(!command.mutation);
        assert!(command.file_scopes.is_empty());
        assert_eq!(command.max_workers, Some(2));
        assert_eq!(
            command.requirements[1].employee.as_deref(),
            Some("writer-1")
        );
    }
}
