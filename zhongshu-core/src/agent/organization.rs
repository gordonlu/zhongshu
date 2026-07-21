use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::agent::profile::{AgentProfile, EmployeeCapability, EmployeeRole};

pub const DEFAULT_MAX_WORKERS_PER_TASK: usize = 3;
pub const DEFAULT_MAX_EMPLOYEE_ROSTER: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssignmentAuthority {
    Manager { manager: String },
    User,
}

impl AssignmentAuthority {
    pub fn manager(name: impl Into<String>) -> Self {
        Self::Manager {
            manager: name.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchTarget {
    #[default]
    ManagerSelected,
    Employee {
        employee: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    #[default]
    Independent,
    SequentialHandoff,
}

/// One role/capability requirement produced by the Lead's planning step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleRequirement {
    pub role: EmployeeRole,
    #[serde(default)]
    pub capabilities: Vec<EmployeeCapability>,
    pub responsibility: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

impl RoleRequirement {
    pub fn required(role: EmployeeRole, responsibility: impl Into<String>) -> Self {
        Self {
            role,
            capabilities: Vec::new(),
            responsibility: responsibility.into(),
            required: true,
        }
    }

    pub fn optional(role: EmployeeRole, responsibility: impl Into<String>) -> Self {
        Self {
            role,
            capabilities: Vec::new(),
            responsibility: responsibility.into(),
            required: false,
        }
    }

    pub fn with_capabilities(mut self, capabilities: Vec<EmployeeCapability>) -> Self {
        self.capabilities = capabilities;
        self
    }
}

/// Structured staffing input. Natural-language classification is deliberately
/// outside this deterministic boundary: the Lead may propose requirements,
/// while this contract enforces limits and exact capability matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffingRequest {
    pub objective: String,
    #[serde(default)]
    pub requirements: Vec<RoleRequirement>,
    #[serde(default)]
    pub max_workers: Option<usize>,
}

impl StaffingRequest {
    pub fn direct(objective: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            requirements: Vec::new(),
            max_workers: Some(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationTaskRequest {
    pub task_id: String,
    pub assigned_by: AssignmentAuthority,
    #[serde(default)]
    pub target: DispatchTarget,
    #[serde(default)]
    pub collaboration: CollaborationMode,
    pub staffing: StaffingRequest,
}

impl OrganizationTaskRequest {
    pub fn manager_selected(
        task_id: impl Into<String>,
        manager: impl Into<String>,
        staffing: StaffingRequest,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            assigned_by: AssignmentAuthority::manager(manager),
            target: DispatchTarget::ManagerSelected,
            collaboration: CollaborationMode::Independent,
            staffing,
        }
    }

    pub fn user_to_employee(
        task_id: impl Into<String>,
        employee: impl Into<String>,
        staffing: StaffingRequest,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            assigned_by: AssignmentAuthority::User,
            target: DispatchTarget::Employee {
                employee: employee.into(),
            },
            collaboration: CollaborationMode::Independent,
            staffing,
        }
    }

    pub fn with_collaboration(mut self, collaboration: CollaborationMode) -> Self {
        self.collaboration = collaboration;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaffingMode {
    Direct,
    SingleSpecialist,
    MultiSpecialist,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmployeeAssignment {
    pub employee: String,
    pub role: EmployeeRole,
    pub responsibility: String,
    pub matched_capabilities: Vec<EmployeeCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnfilledRequirement {
    pub role: EmployeeRole,
    pub responsibility: String,
    pub required: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffingDecision {
    pub mode: StaffingMode,
    pub assignments: Vec<EmployeeAssignment>,
    pub unfilled: Vec<UnfilledRequirement>,
    pub worker_limit: usize,
    pub rationale: Vec<String>,
}

impl StaffingDecision {
    pub fn can_execute(&self) -> bool {
        self.mode != StaffingMode::Blocked
            && self
                .unfilled
                .iter()
                .all(|requirement| !requirement.required)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaffingPolicy {
    pub max_workers_per_task: usize,
    pub max_employee_roster: usize,
}

impl Default for StaffingPolicy {
    fn default() -> Self {
        Self {
            max_workers_per_task: DEFAULT_MAX_WORKERS_PER_TASK,
            max_employee_roster: DEFAULT_MAX_EMPLOYEE_ROSTER,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrganizationRouter {
    policy: StaffingPolicy,
}

impl OrganizationRouter {
    pub fn new(policy: StaffingPolicy) -> Self {
        Self { policy }
    }

    pub fn route(&self, request: &StaffingRequest, roster: &[AgentProfile]) -> StaffingDecision {
        let mut employee_names = HashSet::new();
        if let Some(duplicate) = roster
            .iter()
            .map(|profile| profile.name.as_str())
            .find(|name| !employee_names.insert(*name))
        {
            return StaffingDecision {
                mode: StaffingMode::Blocked,
                assignments: Vec::new(),
                unfilled: request
                    .requirements
                    .iter()
                    .map(|requirement| UnfilledRequirement {
                        role: requirement.role.clone(),
                        responsibility: requirement.responsibility.clone(),
                        required: requirement.required,
                        reason: format!("duplicate employee identity '{duplicate}'"),
                    })
                    .collect(),
                worker_limit: self.policy.max_workers_per_task,
                rationale: vec![format!(
                    "employee names must be unique; duplicate identity '{duplicate}' is ambiguous"
                )],
            };
        }
        if roster.len() > self.policy.max_employee_roster {
            return StaffingDecision {
                mode: StaffingMode::Blocked,
                assignments: Vec::new(),
                unfilled: request
                    .requirements
                    .iter()
                    .map(|requirement| UnfilledRequirement {
                        role: requirement.role.clone(),
                        responsibility: requirement.responsibility.clone(),
                        required: requirement.required,
                        reason: format!(
                            "employee roster exceeds limit {}",
                            self.policy.max_employee_roster
                        ),
                    })
                    .collect(),
                worker_limit: self.policy.max_workers_per_task,
                rationale: vec![format!(
                    "roster has {} employees but policy allows at most {}",
                    roster.len(),
                    self.policy.max_employee_roster
                )],
            };
        }

        let worker_limit = request
            .max_workers
            .unwrap_or(self.policy.max_workers_per_task)
            .min(self.policy.max_workers_per_task);
        if request.requirements.is_empty() {
            return StaffingDecision {
                mode: StaffingMode::Direct,
                assignments: Vec::new(),
                unfilled: Vec::new(),
                worker_limit,
                rationale: vec!["Lead handles the task directly; no specialist is required".into()],
            };
        }
        if let Some(requirement) = request.requirements.iter().find(|requirement| {
            !requirement.role.is_valid()
                || requirement
                    .capabilities
                    .iter()
                    .any(|capability| !capability.is_valid())
        }) {
            return StaffingDecision {
                mode: StaffingMode::Blocked,
                assignments: Vec::new(),
                unfilled: vec![UnfilledRequirement {
                    role: requirement.role.clone(),
                    responsibility: requirement.responsibility.clone(),
                    required: requirement.required,
                    reason: "role and capability identifiers must be non-empty".into(),
                }],
                worker_limit,
                rationale: vec!["staffing request contains an invalid open identifier".into()],
            };
        }
        if worker_limit == 0 {
            return StaffingDecision {
                mode: StaffingMode::Blocked,
                assignments: Vec::new(),
                unfilled: request
                    .requirements
                    .iter()
                    .map(|requirement| UnfilledRequirement {
                        role: requirement.role.clone(),
                        responsibility: requirement.responsibility.clone(),
                        required: requirement.required,
                        reason: "per-task worker limit is zero".into(),
                    })
                    .collect(),
                worker_limit,
                rationale: vec![
                    "specialists were requested but the effective worker limit is zero".into(),
                ],
            };
        }

        let mut assignments = Vec::new();
        let mut unfilled = Vec::new();
        let mut assigned_employees = HashSet::new();
        let mut rationale = Vec::new();

        for requirement in &request.requirements {
            if assignments.len() >= worker_limit {
                unfilled.push(UnfilledRequirement {
                    role: requirement.role.clone(),
                    responsibility: requirement.responsibility.clone(),
                    required: requirement.required,
                    reason: format!("per-task worker limit {worker_limit} reached"),
                });
                continue;
            }

            let employee = roster.iter().find(|profile| {
                !assigned_employees.contains(profile.name.as_str())
                    && profile.specialty.role == requirement.role
                    && requirement
                        .capabilities
                        .iter()
                        .all(|required| profile.specialty.capabilities.contains(required))
            });
            match employee {
                Some(profile) => {
                    assigned_employees.insert(profile.name.as_str());
                    assignments.push(EmployeeAssignment {
                        employee: profile.name.clone(),
                        role: requirement.role.clone(),
                        responsibility: requirement.responsibility.clone(),
                        matched_capabilities: requirement.capabilities.clone(),
                    });
                }
                None => unfilled.push(UnfilledRequirement {
                    role: requirement.role.clone(),
                    responsibility: requirement.responsibility.clone(),
                    required: requirement.required,
                    reason: "no employee matches the required role and capabilities".into(),
                }),
            }
        }

        let blocked = unfilled.iter().any(|requirement| requirement.required);
        let mode = if blocked {
            StaffingMode::Blocked
        } else if assignments.len() == 1 {
            StaffingMode::SingleSpecialist
        } else if assignments.len() > 1 {
            StaffingMode::MultiSpecialist
        } else {
            StaffingMode::Direct
        };
        rationale.push(format!(
            "selected {} of {} requested specialist assignments",
            assignments.len(),
            request.requirements.len()
        ));
        if !unfilled.is_empty() {
            rationale.push(format!("{} requirements remain unfilled", unfilled.len()));
        }

        StaffingDecision {
            mode,
            assignments,
            unfilled,
            worker_limit,
            rationale,
        }
    }

    /// Route a user-direct assignment to one named employee while preserving
    /// the same roster, role, capability, and worker-limit checks as manager
    /// selection. Direct assignment does not grant capabilities the employee
    /// profile does not declare.
    pub fn route_to_employee(
        &self,
        request: &StaffingRequest,
        roster: &[AgentProfile],
        employee_name: &str,
    ) -> StaffingDecision {
        let matching = roster
            .iter()
            .filter(|profile| profile.name == employee_name)
            .count();
        if matching != 1 {
            return blocked_decision(
                request,
                self.policy.max_workers_per_task,
                format!(
                    "direct employee '{employee_name}' must resolve to exactly one roster entry; found {matching}"
                ),
            );
        }
        if request.requirements.len() != 1 {
            return blocked_decision(
                request,
                self.policy.max_workers_per_task,
                "direct employee assignment requires exactly one role requirement".into(),
            );
        }

        let mut reordered = roster.to_vec();
        let target_index = reordered
            .iter()
            .position(|profile| profile.name == employee_name)
            .expect("matching employee counted above");
        reordered.swap(0, target_index);
        let mut decision = self.route(request, &reordered);
        if decision.can_execute() {
            let requirement = &request.requirements[0];
            let target = &reordered[0];
            let target_matches = target.specialty.role == requirement.role
                && requirement
                    .capabilities
                    .iter()
                    .all(|required| target.specialty.capabilities.contains(required));
            let target_was_selected = decision.assignments.len() == 1
                && decision.assignments[0].employee == employee_name;
            if !target_matches || !target_was_selected {
                return blocked_decision(
                    request,
                    decision.worker_limit,
                    format!(
                        "direct employee '{employee_name}' did not match the requested capability contract"
                    ),
                );
            }
            decision.rationale.push(format!(
                "user directly assigned the task to employee '{employee_name}'"
            ));
        }
        decision
    }
}

fn blocked_decision(
    request: &StaffingRequest,
    worker_limit: usize,
    reason: String,
) -> StaffingDecision {
    StaffingDecision {
        mode: StaffingMode::Blocked,
        assignments: Vec::new(),
        unfilled: request
            .requirements
            .iter()
            .map(|requirement| UnfilledRequirement {
                role: requirement.role.clone(),
                responsibility: requirement.responsibility.clone(),
                required: requirement.required,
                reason: reason.clone(),
            })
            .collect(),
        worker_limit,
        rationale: vec![reason],
    }
}

impl Default for OrganizationRouter {
    fn default() -> Self {
        Self::new(StaffingPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentBudget;

    fn employee(
        name: &str,
        role: EmployeeRole,
        capabilities: Vec<EmployeeCapability>,
    ) -> AgentProfile {
        let focus = role.as_str().to_string();
        AgentProfile::new(name, "specialist", vec![], AgentBudget::default()).with_specialty(
            role,
            capabilities,
            focus,
        )
    }

    fn roster() -> Vec<AgentProfile> {
        vec![
            employee(
                "frontend-1",
                EmployeeRole::frontend(),
                vec![
                    EmployeeCapability::ui_implementation(),
                    EmployeeCapability::browser_verification(),
                ],
            ),
            employee(
                "backend-1",
                EmployeeRole::backend(),
                vec![
                    EmployeeCapability::api_implementation(),
                    EmployeeCapability::data_consistency(),
                ],
            ),
            employee(
                "writer-1",
                EmployeeRole::writer(),
                vec![EmployeeCapability::product_copy()],
            ),
            employee(
                "tester-1",
                EmployeeRole::tester(),
                vec![EmployeeCapability::test_execution()],
            ),
        ]
    }

    #[test]
    fn direct_request_does_not_dispatch_workers() {
        let decision = OrganizationRouter::default().route(
            &StaffingRequest::direct("answer a focused question"),
            &roster(),
        );

        assert_eq!(decision.mode, StaffingMode::Direct);
        assert!(decision.assignments.is_empty());
        assert!(decision.can_execute());
    }

    #[test]
    fn optional_requirement_does_not_let_direct_assignment_bypass_capabilities() {
        let roster = vec![employee(
            "writer-1",
            EmployeeRole::writer(),
            vec![EmployeeCapability::product_copy()],
        )];
        let request = StaffingRequest {
            objective: "prepare a cash-flow forecast".into(),
            requirements: vec![RoleRequirement::optional(
                EmployeeRole::new("management_accountant"),
                "prepare forecast",
            )
            .with_capabilities(vec![EmployeeCapability::new("cash_flow_forecasting")])],
            max_workers: Some(1),
        };

        let decision =
            OrganizationRouter::default().route_to_employee(&request, &roster, "writer-1");

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert!(!decision.can_execute());
        assert!(decision.rationale[0].contains("did not match"));
    }

    #[test]
    fn routes_only_required_specialists() {
        let request = StaffingRequest {
            objective: "ship a full-stack settings screen".into(),
            requirements: vec![
                RoleRequirement::required(EmployeeRole::backend(), "implement API")
                    .with_capabilities(vec![EmployeeCapability::api_implementation()]),
                RoleRequirement::required(EmployeeRole::frontend(), "implement UI")
                    .with_capabilities(vec![EmployeeCapability::ui_implementation()]),
                RoleRequirement::optional(EmployeeRole::writer(), "write empty-state copy"),
            ],
            max_workers: None,
        };

        let decision = OrganizationRouter::default().route(&request, &roster());

        assert_eq!(decision.mode, StaffingMode::MultiSpecialist);
        assert_eq!(decision.assignments.len(), 3);
        assert!(decision.can_execute());
        assert!(!decision
            .assignments
            .iter()
            .any(|assignment| assignment.role == EmployeeRole::tester()));
    }

    #[test]
    fn required_role_over_worker_limit_blocks_instead_of_silently_dropping_it() {
        let request = StaffingRequest {
            objective: "cross-domain delivery".into(),
            requirements: vec![
                RoleRequirement::required(EmployeeRole::backend(), "API"),
                RoleRequirement::required(EmployeeRole::frontend(), "UI"),
                RoleRequirement::required(EmployeeRole::writer(), "copy"),
            ],
            max_workers: Some(2),
        };

        let decision = OrganizationRouter::default().route(&request, &roster());

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert_eq!(decision.assignments.len(), 2);
        assert_eq!(decision.unfilled.len(), 1);
        assert!(decision.unfilled[0].reason.contains("worker limit"));
        assert!(!decision.can_execute());
    }

    #[test]
    fn capability_mismatch_is_visible() {
        let request = StaffingRequest {
            objective: "browser validation".into(),
            requirements: vec![RoleRequirement::required(
                EmployeeRole::tester(),
                "validate in browser",
            )
            .with_capabilities(vec![EmployeeCapability::browser_verification()])],
            max_workers: None,
        };

        let decision = OrganizationRouter::default().route(&request, &roster());

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert!(decision.assignments.is_empty());
        assert_eq!(decision.unfilled[0].role, EmployeeRole::tester());
    }

    #[test]
    fn oversized_roster_is_blocked_before_dispatch() {
        let mut employees = roster();
        for index in 0..5 {
            employees.push(employee(
                &format!("extra-{index}"),
                EmployeeRole::generalist(),
                vec![],
            ));
        }
        let request = StaffingRequest {
            objective: "API".into(),
            requirements: vec![RoleRequirement::required(EmployeeRole::backend(), "API")],
            max_workers: None,
        };

        let decision = OrganizationRouter::default().route(&request, &employees);

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert!(decision.rationale[0].contains("roster has 9"));
    }

    #[test]
    fn zero_worker_limit_cannot_silently_drop_required_roles() {
        let request = StaffingRequest {
            objective: "API".into(),
            requirements: vec![RoleRequirement::required(EmployeeRole::backend(), "API")],
            max_workers: Some(0),
        };

        let decision = OrganizationRouter::default().route(&request, &roster());

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert!(!decision.can_execute());
        assert_eq!(decision.unfilled.len(), 1);
        assert!(decision.unfilled[0].reason.contains("zero"));
    }

    #[test]
    fn duplicate_employee_names_are_blocked_as_ambiguous() {
        let employees = vec![
            employee("same", EmployeeRole::backend(), vec![]),
            employee("same", EmployeeRole::frontend(), vec![]),
        ];
        let request = StaffingRequest {
            objective: "UI".into(),
            requirements: vec![RoleRequirement::required(EmployeeRole::frontend(), "UI")],
            max_workers: None,
        };

        let decision = OrganizationRouter::default().route(&request, &employees);

        assert_eq!(decision.mode, StaffingMode::Blocked);
        assert!(decision.assignments.is_empty());
        assert!(decision.rationale[0].contains("duplicate identity"));
    }

    #[test]
    fn routes_non_software_roles_defined_at_runtime() {
        let accountant = employee(
            "accountant-1",
            EmployeeRole::new("management_accountant"),
            vec![
                EmployeeCapability::new("cash_flow_forecasting"),
                EmployeeCapability::new("variance_analysis"),
            ],
        );
        let request = StaffingRequest {
            objective: "prepare a quarterly cash-flow forecast".into(),
            requirements: vec![RoleRequirement::required(
                EmployeeRole::new("management_accountant"),
                "forecast cash flow and explain material variance",
            )
            .with_capabilities(vec![EmployeeCapability::new("cash_flow_forecasting")])],
            max_workers: Some(1),
        };

        let decision = OrganizationRouter::default().route(&request, &[accountant]);

        assert_eq!(decision.mode, StaffingMode::SingleSpecialist);
        assert!(decision.can_execute());
        assert_eq!(decision.assignments[0].employee, "accountant-1");
        assert_eq!(
            decision.assignments[0].role,
            EmployeeRole::new("management_accountant")
        );
    }
}
