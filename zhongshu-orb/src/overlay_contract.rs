use serde::{Deserialize, Serialize};
use zhongshu_core::agent::ExecutionGraphSnapshot;
use zhongshu_core::event::OrganizationEvent;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchDiffPayload {
    pub summary: String,
    pub unified_diff: String,
    pub changed: bool,
    pub replace_all: bool,
    pub removed_lines: usize,
    pub added_lines: usize,
    pub before_hash: String,
    pub after_hash: String,
}

impl From<zhongshu_core::patch::PatchDiffPayload> for PatchDiffPayload {
    fn from(value: zhongshu_core::patch::PatchDiffPayload) -> Self {
        Self {
            summary: value.summary,
            unified_diff: value.unified_diff,
            changed: value.changed,
            replace_all: value.replace_all,
            removed_lines: value.removed_lines,
            added_lines: value.added_lines,
            before_hash: value.before_hash,
            after_hash: value.after_hash,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub name: String,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done { success: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatEntry {
    pub role: EntryRole,
    pub content: String,
    pub tool_calls: Vec<ToolCallEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRequest {
    pub request_id: String,
    pub source: String,
    pub tool: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsConfig {
    pub api_key: String,
    pub api_key_saved: bool,
    pub api_base: String,
    pub model: String,
    pub personality: String,
    pub proxy_port: Option<String>,
    pub bg_enabled: Option<bool>,
    pub bg_interval: Option<String>,
    pub bg_prompt: Option<String>,
    pub auto_evolve: Option<bool>,
    pub auto_multi_agent: Option<bool>,
    pub max_context_tokens: Option<u32>,
    pub mode: Option<String>,
}

/// Partial settings update sent from UI (mirrors TS `Partial<SettingsConfig>`).
/// All fields are `Option` — `None` means "don't change".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsUpdate {
    pub api_key: Option<String>,
    pub api_key_saved: Option<bool>,
    pub api_base: Option<String>,
    pub model: Option<String>,
    pub personality: Option<String>,
    pub proxy_port: Option<String>,
    pub bg_enabled: Option<bool>,
    pub bg_interval: Option<String>,
    pub bg_prompt: Option<String>,
    pub auto_evolve: Option<bool>,
    pub auto_multi_agent: Option<bool>,
    pub max_context_tokens: Option<u32>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationEmployeeInfo {
    pub name: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub focus: String,
    pub read_only_eligible: bool,
    pub blocked_by: Option<String>,
    pub sandbox_eligible: bool,
    pub sandbox_blocked_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationRoleCommand {
    pub role: String,
    #[serde(default)]
    pub employee: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub responsibility: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationFileScopeCommand {
    pub employee: String,
    pub owned_files: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationTaskCommand {
    pub objective: String,
    pub requirements: Vec<OrganizationRoleCommand>,
    #[serde(default)]
    pub sequential_handoff: bool,
    pub max_workers: Option<usize>,
    pub target_employee: Option<String>,
    #[serde(default)]
    pub mutation: bool,
    #[serde(default)]
    pub workspace_mode: zhongshu_core::agent::WorkerWorkspaceMode,
    #[serde(default)]
    pub file_scopes: Vec<OrganizationFileScopeCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationGraphView {
    pub store_version: u64,
    pub graph: ExecutionGraphSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationRecoveryCommand {
    pub task_id: String,
    pub node_id: String,
    pub action: OrganizationRecoveryAction,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRecoveryAction {
    Reconcile,
    Abandon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationRecoveryResult {
    pub task_id: String,
    pub node_id: String,
    pub action: OrganizationRecoveryAction,
    pub assessment: String,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub executed_cleanup_nodes: Vec<String>,
    pub graph: OrganizationGraphView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlayToUiEvent {
    Delta {
        content: String,
    },
    Complete,
    History {
        entries: Vec<ChatEntry>,
        has_more: bool,
    },
    PrependHistory {
        entries: Vec<ChatEntry>,
        has_more: bool,
    },
    ToolCall {
        name: String,
    },
    ToolResult {
        name: String,
        success: bool,
    },
    Auth {
        request: AuthRequest,
    },
    Settings {
        config: SettingsConfig,
    },
    Tasks {
        tasks: Vec<serde_json::Value>,
    },
    Runbooks {
        runbooks: Vec<serde_json::Value>,
    },
    Equipment {
        items: Vec<serde_json::Value>,
    },
    Toast {
        text: String,
    },
    StateChange {
        state: String,
    },
    ModeChange {
        mode: String,
    },
    Zoom {
        active: bool,
    },
    Coding {
        event: CodingUiEvent,
    },
    Organization {
        event: OrganizationEvent,
    },
    OrganizationRoster {
        employees: Vec<OrganizationEmployeeInfo>,
        max_workers: usize,
    },
    OrganizationGraphs {
        graphs: Vec<OrganizationGraphView>,
    },
    OrganizationRecovery {
        result: OrganizationRecoveryResult,
    },
    Verification {
        command: String,
        success: bool,
        exit_code: Option<i32>,
        step: Option<String>,
    },
    RecoveryFeedback {
        rule_id: String,
        message: String,
    },
    PhaseTransition {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodingUiEvent {
    PlanCreated {
        session_id: String,
        step_count: usize,
        risk: String,
    },
    PlanStepStarted {
        session_id: String,
        step_id: String,
        title: String,
    },
    PlanStepCompleted {
        session_id: String,
        step_id: String,
        status: String,
    },
    WorkerStarted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        owned_files: Vec<String>,
    },
    WorkerCompleted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        success: bool,
        status: String,
    },
    WorkerConflict {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        reason: String,
    },
    PatchPreview {
        session_id: Option<String>,
        path: String,
        operation: String,
        diff_summary: String,
        diff: Option<PatchDiffPayload>,
    },
    PatchApplied {
        session_id: Option<String>,
        path: String,
        operation: String,
        changed: bool,
    },
    Verification {
        command: String,
        success: bool,
        exit_code: Option<i32>,
    },
    RecoveryFeedback {
        rule_id: String,
        message: String,
    },
    ContextPressure {
        pressure_percent: u8,
        dropped_evidence: usize,
        dropped_recent: usize,
    },
    ContextIncluded {
        description: String,
        estimated_tokens: usize,
    },
    ReplayAvailable {
        conversation_id: Option<i64>,
        replay_execution_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiToOverlayCommand {
    Submit(String),
    DelegateReview(String),
    DelegateOrganization(OrganizationTaskCommand),
    ListOrganizationEmployees,
    ListOrganizationGraphs,
    RecoverOrganization(OrganizationRecoveryCommand),
    Stop,
    NewConversation,
    Approve(String),
    Deny(String),
    PickPersonality(String),
    SaveSettings(SettingsUpdate),
    OpenSettings,
    DeleteHistory,
    LoadMore,
    ListTasks,
    ListRunbooks,
    ListEquipment,
    ToggleEquipment(String),
    ToggleZoom,
    StartDrag,
    Minimize,
    MaximizeRestore,
    CloseWindow,
    CancelTask(String),
    CompleteTask(String),
    Unknown,
}

pub fn chat_coding_smoke_events() -> Vec<OverlayToUiEvent> {
    vec![
        OverlayToUiEvent::ModeChange {
            mode: "coding".into(),
        },
        OverlayToUiEvent::Delta {
            content: "offline proof: running safe self-test".into(),
        },
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::PlanCreated {
                session_id: "session-smoke".into(),
                step_count: 2,
                risk: "low".into(),
            },
        },
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::WorkerStarted {
                session_id: Some("session-smoke".into()),
                worker: "deepseek-worker".into(),
                task_id: "task-smoke".into(),
                owned_files: vec!["src/lib.rs".into()],
            },
        },
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::PatchPreview {
                session_id: Some("session-smoke".into()),
                path: "src/lib.rs".into(),
                operation: "update".into(),
                diff_summary: "1 file changed".into(),
                diff: None,
            },
        },
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::Verification {
                command: "cargo test -p zhongshu-core offline_proof".into(),
                success: true,
                exit_code: Some(0),
            },
        },
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::ContextPressure {
                pressure_percent: 78,
                dropped_evidence: 0,
                dropped_recent: 0,
            },
        },
        OverlayToUiEvent::Complete,
    ]
}

pub fn chat_coding_smoke_commands() -> Vec<&'static str> {
    vec![
        r#"{"type":"submit","text":"run offline proof"}"#,
        r#"{"type":"toggle_zoom"}"#,
    ]
}

#[cfg(test)]
fn chat_coding_smoke_command_fixtures() -> Vec<&'static str> {
    vec![
        r#"{"type":"submit","text":"run offline proof"}"#,
        r#"{"type":"stop"}"#,
        r#"{"type":"approve","request_id":"req-smoke"}"#,
        r#"{"type":"toggle_zoom"}"#,
        r#"{"type":"start_drag"}"#,
    ]
}

pub fn parse_ui_command(body: &str) -> UiToOverlayCommand {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(body) else {
        return UiToOverlayCommand::Unknown;
    };

    match msg["type"].as_str() {
        Some("submit") => msg["text"]
            .as_str()
            .map(|text| UiToOverlayCommand::Submit(text.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("delegate_review") => msg["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(|text| UiToOverlayCommand::DelegateReview(text.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("delegate_organization") => serde_json::from_value::<OrganizationTaskCommand>(
            msg.get("task").cloned().unwrap_or(serde_json::Value::Null),
        )
        .ok()
        .filter(valid_organization_task)
        .map(UiToOverlayCommand::DelegateOrganization)
        .unwrap_or(UiToOverlayCommand::Unknown),
        Some("list_organization_employees") => UiToOverlayCommand::ListOrganizationEmployees,
        Some("list_organization_graphs") => UiToOverlayCommand::ListOrganizationGraphs,
        Some("reconcile_organization") => {
            parse_organization_recovery(&msg, OrganizationRecoveryAction::Reconcile)
        }
        Some("abandon_organization_recovery") => {
            parse_organization_recovery(&msg, OrganizationRecoveryAction::Abandon)
        }
        Some("stop") => UiToOverlayCommand::Stop,
        Some("new_conversation") => UiToOverlayCommand::NewConversation,
        Some("approve") => {
            UiToOverlayCommand::Approve(msg["request_id"].as_str().unwrap_or("").to_string())
        }
        Some("deny") => {
            UiToOverlayCommand::Deny(msg["request_id"].as_str().unwrap_or("").to_string())
        }
        Some("pick_personality") => msg["personality"]
            .as_str()
            .map(|personality| UiToOverlayCommand::PickPersonality(personality.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("save_settings") => msg
            .get("config")
            .and_then(settings_from_value)
            .map(UiToOverlayCommand::SaveSettings)
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("open_settings") => UiToOverlayCommand::OpenSettings,
        Some("delete_history") => UiToOverlayCommand::DeleteHistory,
        Some("load_more") => UiToOverlayCommand::LoadMore,
        Some("list_tasks") => UiToOverlayCommand::ListTasks,
        Some("list_runbooks") => UiToOverlayCommand::ListRunbooks,
        Some("list_equipment") => UiToOverlayCommand::ListEquipment,
        Some("toggle_equipment") => msg
            .get("id")
            .and_then(|v| v.as_str())
            .map(|id| UiToOverlayCommand::ToggleEquipment(id.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("toggle_zoom") => UiToOverlayCommand::ToggleZoom,
        Some("start_drag") => UiToOverlayCommand::StartDrag,
        Some("minimize") => UiToOverlayCommand::Minimize,
        Some("maximize_restore") => UiToOverlayCommand::MaximizeRestore,
        Some("close_window") => UiToOverlayCommand::CloseWindow,
        Some("cancel_task") => msg["task_id"]
            .as_str()
            .map(|id| UiToOverlayCommand::CancelTask(id.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        Some("complete_task") => msg["task_id"]
            .as_str()
            .map(|id| UiToOverlayCommand::CompleteTask(id.to_string()))
            .unwrap_or(UiToOverlayCommand::Unknown),
        _ => UiToOverlayCommand::Unknown,
    }
}

fn parse_organization_recovery(
    message: &serde_json::Value,
    action: OrganizationRecoveryAction,
) -> UiToOverlayCommand {
    let Some(task_id) = message.get("task_id").and_then(serde_json::Value::as_str) else {
        return UiToOverlayCommand::Unknown;
    };
    let Some(node_id) = message.get("node_id").and_then(serde_json::Value::as_str) else {
        return UiToOverlayCommand::Unknown;
    };
    let reason = message
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned);
    if task_id.trim().is_empty()
        || node_id.trim().is_empty()
        || (action == OrganizationRecoveryAction::Abandon && reason.is_none())
    {
        return UiToOverlayCommand::Unknown;
    }
    UiToOverlayCommand::RecoverOrganization(OrganizationRecoveryCommand {
        task_id: task_id.to_owned(),
        node_id: node_id.to_owned(),
        action,
        reason,
    })
}

fn valid_organization_task(task: &OrganizationTaskCommand) -> bool {
    let worker_limit_valid = match task.max_workers {
        Some(limit) => limit > 0 && limit <= zhongshu_core::agent::DEFAULT_MAX_WORKERS_PER_TASK,
        None => true,
    };
    let target_valid = match task.target_employee.as_deref() {
        Some(employee) => !employee.trim().is_empty() && task.requirements.len() == 1,
        None => true,
    };
    let mut selected_employees = std::collections::BTreeSet::new();
    let selected_employees_valid = task.requirements.iter().all(|requirement| {
        requirement.employee.as_deref().map_or(true, |employee| {
            !employee.trim().is_empty() && selected_employees.insert(employee)
        })
    });
    let mut scoped_employees = std::collections::BTreeSet::new();
    let file_scopes_valid = task.file_scopes.iter().all(|scope| {
        !scope.employee.trim().is_empty()
            && scoped_employees.insert(scope.employee.as_str())
            && !scope.owned_files.is_empty()
            && scope.owned_files.iter().all(|file| {
                let path = std::path::Path::new(file);
                !file.trim().is_empty()
                    && !path.is_absolute()
                    && !path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
            })
    });
    let mutation_scopes_match = if task.mutation {
        !selected_employees.is_empty()
            && selected_employees
                == scoped_employees
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
    } else {
        task.file_scopes.is_empty()
    };
    let workspace_mode_valid = task.mutation
        || task.workspace_mode == zhongshu_core::agent::WorkerWorkspaceMode::ProposalOnly;
    !task.objective.trim().is_empty()
        && !task.requirements.is_empty()
        && task.requirements.len() <= zhongshu_core::agent::DEFAULT_MAX_WORKERS_PER_TASK
        && worker_limit_valid
        && selected_employees_valid
        && file_scopes_valid
        && mutation_scopes_match
        && workspace_mode_valid
        && task.requirements.iter().all(|requirement| {
            !requirement.role.trim().is_empty()
                && !requirement.responsibility.trim().is_empty()
                && requirement
                    .capabilities
                    .iter()
                    .all(|capability| !capability.trim().is_empty())
        })
        && target_valid
}

fn settings_from_value(value: &serde_json::Value) -> Option<SettingsUpdate> {
    let cfg = value.as_object()?;
    Some(SettingsUpdate {
        api_key: cfg
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(String::from),
        api_key_saved: cfg.get("api_key_saved").and_then(|v| v.as_bool()),
        api_base: cfg
            .get("api_base")
            .and_then(|v| v.as_str())
            .map(String::from),
        model: cfg.get("model").and_then(|v| v.as_str()).map(String::from),
        personality: cfg
            .get("personality")
            .and_then(|v| v.as_str())
            .map(String::from),
        proxy_port: cfg
            .get("proxy_port")
            .and_then(|v| v.as_str())
            .map(String::from),
        bg_enabled: cfg.get("bg_enabled").and_then(|v| v.as_bool()),
        bg_interval: cfg
            .get("bg_interval")
            .and_then(|v| v.as_str())
            .map(String::from),
        bg_prompt: cfg
            .get("bg_prompt")
            .and_then(|v| v.as_str())
            .map(String::from),
        auto_evolve: cfg.get("auto_evolve").and_then(|v| v.as_bool()),
        auto_multi_agent: cfg.get("auto_multi_agent").and_then(|v| v.as_bool()),
        max_context_tokens: cfg
            .get("max_context_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
        mode: cfg.get("mode").and_then(|v| v.as_str()).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_submit_command() {
        let cmd = parse_ui_command(r#"{"type":"submit","text":"hello"}"#);

        assert_eq!(cmd, UiToOverlayCommand::Submit("hello".into()));
    }

    #[test]
    fn parses_delegate_review_command() {
        assert_eq!(
            parse_ui_command(r#"{"type":"delegate_review","text":"review this change"}"#),
            UiToOverlayCommand::DelegateReview("review this change".into())
        );
        assert_eq!(
            parse_ui_command(r#"{"type":"delegate_review","text":"  "}"#),
            UiToOverlayCommand::Unknown
        );
    }

    #[test]
    fn parses_bounded_structured_organization_command() {
        let command = parse_ui_command(
            r#"{"type":"delegate_organization","task":{"objective":"review cash flow","requirements":[{"role":"management_accountant","capabilities":["cash_flow_forecasting"],"responsibility":"prepare forecast","required":true}],"sequential_handoff":false,"max_workers":1}}"#,
        );

        let UiToOverlayCommand::DelegateOrganization(task) = command else {
            panic!("expected organization command");
        };
        assert_eq!(task.objective, "review cash flow");
        assert_eq!(task.requirements[0].role, "management_accountant");
        assert_eq!(task.max_workers, Some(1));

        assert_eq!(
            parse_ui_command(
                r#"{"type":"delegate_organization","task":{"objective":"too many","requirements":[{"role":"a","responsibility":"a"},{"role":"b","responsibility":"b"},{"role":"c","responsibility":"c"},{"role":"d","responsibility":"d"}],"sequential_handoff":false}}"#,
            ),
            UiToOverlayCommand::Unknown
        );
    }

    #[test]
    fn mutation_command_requires_matching_safe_file_scopes() {
        let valid = parse_ui_command(
            r#"{"type":"delegate_organization","task":{"objective":"update copy","requirements":[{"role":"writer","employee":"writer-a","responsibility":"copy"}],"max_workers":1,"mutation":true,"workspace_mode":"isolated_sandbox","file_scopes":[{"employee":"writer-a","owned_files":["src/copy.rs"]}]}}"#,
        );
        let UiToOverlayCommand::DelegateOrganization(valid) = valid else {
            panic!("expected organization command");
        };
        assert_eq!(
            valid.workspace_mode,
            zhongshu_core::agent::WorkerWorkspaceMode::IsolatedSandbox
        );

        for invalid in [
            r#"{"type":"delegate_organization","task":{"objective":"update copy","requirements":[{"role":"writer","employee":"writer-a","responsibility":"copy"}],"max_workers":1,"mutation":true}}"#,
            r#"{"type":"delegate_organization","task":{"objective":"update copy","requirements":[{"role":"writer","employee":"writer-a","responsibility":"copy"}],"max_workers":1,"mutation":true,"file_scopes":[{"employee":"writer-a","owned_files":["../outside"]}]}}"#,
            r#"{"type":"delegate_organization","task":{"objective":"update copy","requirements":[{"role":"writer","employee":"writer-a","responsibility":"copy"}],"max_workers":1,"mutation":true,"file_scopes":[{"employee":"writer-b","owned_files":["src/copy.rs"]}]}}"#,
            r#"{"type":"delegate_organization","task":{"objective":"review copy","requirements":[{"role":"writer","employee":"writer-a","responsibility":"copy"}],"max_workers":1,"workspace_mode":"isolated_sandbox"}}"#,
        ] {
            assert_eq!(parse_ui_command(invalid), UiToOverlayCommand::Unknown);
        }
    }

    #[test]
    fn recovery_commands_require_graph_identity_and_explicit_abandon_reason() {
        assert_eq!(
            parse_ui_command(
                r#"{"type":"reconcile_organization","task_id":"mutation-1","node_id":"apply"}"#,
            ),
            UiToOverlayCommand::RecoverOrganization(OrganizationRecoveryCommand {
                task_id: "mutation-1".into(),
                node_id: "apply".into(),
                action: OrganizationRecoveryAction::Reconcile,
                reason: None,
            })
        );
        assert!(matches!(
            parse_ui_command(
                r#"{"type":"abandon_organization_recovery","task_id":"mutation-1","node_id":"apply","reason":"operator inspected partial output"}"#,
            ),
            UiToOverlayCommand::RecoverOrganization(OrganizationRecoveryCommand {
                action: OrganizationRecoveryAction::Abandon,
                ..
            })
        ));
        assert_eq!(
            parse_ui_command(
                r#"{"type":"abandon_organization_recovery","task_id":"mutation-1","node_id":"apply","reason":"  "}"#,
            ),
            UiToOverlayCommand::Unknown
        );
    }

    #[test]
    fn parses_settings_command() {
        let cmd = parse_ui_command(
            r#"{"type":"save_settings","config":{"api_base":"https://example.test","model":"m","mode":"coding","max_context_tokens":100000,"auto_multi_agent":true}}"#,
        );

        match cmd {
            UiToOverlayCommand::SaveSettings(settings) => {
                assert_eq!(settings.api_base.as_deref(), Some("https://example.test"));
                assert_eq!(settings.model.as_deref(), Some("m"));
                assert_eq!(settings.mode.as_deref(), Some("coding"));
                assert_eq!(settings.max_context_tokens, Some(100000));
                assert_eq!(settings.auto_multi_agent, Some(true));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn malformed_command_is_unknown() {
        assert_eq!(parse_ui_command("not json"), UiToOverlayCommand::Unknown);
    }

    #[test]
    fn chat_coding_smoke_events_serialize_for_webview2_ipc() {
        let events = chat_coding_smoke_events();
        assert!(events
            .iter()
            .any(|event| matches!(event, OverlayToUiEvent::Delta { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, OverlayToUiEvent::Complete)));

        for event in events {
            let json = serde_json::to_value(&event).expect("event json");
            assert!(json.get("type").is_some(), "missing event type: {json}");
        }
    }

    #[test]
    fn chat_coding_smoke_commands_parse_from_webview2_ipc() {
        let commands: Vec<_> = chat_coding_smoke_command_fixtures()
            .into_iter()
            .map(parse_ui_command)
            .collect();

        assert_eq!(
            commands,
            vec![
                UiToOverlayCommand::Submit("run offline proof".into()),
                UiToOverlayCommand::Stop,
                UiToOverlayCommand::Approve("req-smoke".into()),
                UiToOverlayCommand::ToggleZoom,
                UiToOverlayCommand::StartDrag,
            ]
        );
    }

    #[test]
    fn organization_event_serializes_with_stable_ipc_tags() {
        let event = OverlayToUiEvent::Organization {
            event: OrganizationEvent::EmployeeAssigned {
                task_id: "org-1".into(),
                employee: "analyst".into(),
                role: "architect".into(),
                responsibility: "review".into(),
                reports_to: "中书".into(),
            },
        };

        let json = serde_json::to_value(event).expect("organization event json");
        assert_eq!(json["type"], "organization");
        assert_eq!(json["event"]["kind"], "employee_assigned");
        assert_eq!(json["event"]["role"], "architect");
        assert_eq!(json["event"]["reports_to"], "中书");
    }
}
