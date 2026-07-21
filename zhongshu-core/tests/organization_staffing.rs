use std::path::Path;

use serde::Deserialize;
use zhongshu_core::agent::{
    AgentBudget, AgentProfile, EmployeeCapability, EmployeeRole, OrganizationRouter, StaffingMode,
    StaffingRequest,
};

#[derive(Debug, Deserialize)]
struct Suite {
    schema_version: u32,
    id: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    fixture: String,
    request: StaffingRequest,
    expected_mode: StaffingMode,
    expected_roles: Vec<EmployeeRole>,
    forbidden_roles: Vec<EmployeeRole>,
}

fn employee(name: &str, role: EmployeeRole, capabilities: Vec<EmployeeCapability>) -> AgentProfile {
    let focus = role.as_str().to_string();
    AgentProfile::new(name, "specialist", vec![], AgentBudget::default()).with_specialty(
        role,
        capabilities,
        focus,
    )
}

#[test]
fn organization_fixture_routes_only_required_specialists() {
    let suite: Suite =
        serde_json::from_str(include_str!("../../benchmarks/organization-v1/suite.json"))
            .expect("organization suite must deserialize");
    assert_eq!(suite.schema_version, 1);
    assert_eq!(suite.id, "organization-v1");

    let roster = vec![
        employee(
            "frontend-employee",
            EmployeeRole::frontend(),
            vec![
                EmployeeCapability::ui_implementation(),
                EmployeeCapability::browser_verification(),
            ],
        ),
        employee(
            "backend-employee",
            EmployeeRole::backend(),
            vec![
                EmployeeCapability::api_implementation(),
                EmployeeCapability::contract_review(),
            ],
        ),
        employee(
            "writer-employee",
            EmployeeRole::writer(),
            vec![
                EmployeeCapability::product_copy(),
                EmployeeCapability::documentation(),
            ],
        ),
        employee(
            "tester-employee",
            EmployeeRole::tester(),
            vec![EmployeeCapability::test_execution()],
        ),
        employee(
            "architect-employee",
            EmployeeRole::architect(),
            vec![EmployeeCapability::architecture_review()],
        ),
        employee(
            "management-accountant",
            EmployeeRole::new("management_accountant"),
            vec![
                EmployeeCapability::new("cash_flow_forecasting"),
                EmployeeCapability::new("variance_analysis"),
            ],
        ),
        employee(
            "treasury-reviewer",
            EmployeeRole::new("treasury_reviewer"),
            vec![EmployeeCapability::new("liquidity_policy_review")],
        ),
        employee(
            "customer-support",
            EmployeeRole::new("customer_support"),
            vec![EmployeeCapability::new("ticket_triage")],
        ),
    ];
    let suite_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../benchmarks/organization-v1");

    for case in suite.cases {
        assert!(!case.id.trim().is_empty());
        assert!(suite_dir.join(&case.fixture).is_dir());
        let decision = OrganizationRouter::default().route(&case.request, &roster);
        let selected_roles: Vec<EmployeeRole> = decision
            .assignments
            .iter()
            .map(|assignment| assignment.role.clone())
            .collect();

        assert_eq!(decision.mode, case.expected_mode);
        assert!(decision.can_execute());
        for expected in case.expected_roles {
            assert!(
                selected_roles.contains(&expected),
                "missing role {expected:?}"
            );
        }
        for forbidden in case.forbidden_roles {
            assert!(
                !selected_roles.contains(&forbidden),
                "unnecessary role {forbidden:?} was dispatched"
            );
        }
    }
}
