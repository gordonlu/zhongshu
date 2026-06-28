use serde::{Deserialize, Serialize};

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
    pub max_context_tokens: Option<u32>,
    pub mode: Option<String>,
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
    Stop,
    NewConversation,
    Approve(String),
    Deny(String),
    PickPersonality(String),
    SaveSettings(SettingsConfig),
    OpenSettings,
    DeleteHistory,
    LoadMore,
    ListTasks,
    ListRunbooks,
    ListEquipment,
    ToggleEquipment(String),
    ToggleZoom,
    StartDrag,
    CancelTask(String),
    CompleteTask(String),
    Unknown,
}

#[cfg(test)]
fn chat_coding_smoke_events() -> Vec<OverlayToUiEvent> {
    vec![
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
        OverlayToUiEvent::Coding {
            event: CodingUiEvent::ReplayAvailable {
                conversation_id: Some(42),
                replay_execution_id: Some("replay-smoke".into()),
            },
        },
        OverlayToUiEvent::Complete,
    ]
}

#[cfg(test)]
fn chat_coding_smoke_commands() -> Vec<&'static str> {
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

fn settings_from_value(value: &serde_json::Value) -> Option<SettingsConfig> {
    let cfg = value.as_object()?;
    Some(SettingsConfig {
        api_key: cfg
            .get("api_key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        api_key_saved: cfg
            .get("api_key_saved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        api_base: cfg
            .get("api_base")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        model: cfg
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        personality: cfg
            .get("personality")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        proxy_port: cfg
            .get("proxy_port")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        bg_enabled: cfg.get("bg_enabled").and_then(|v| v.as_bool()),
        bg_interval: cfg
            .get("bg_interval")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        bg_prompt: cfg
            .get("bg_prompt")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        auto_evolve: cfg.get("auto_evolve").and_then(|v| v.as_bool()),
        max_context_tokens: cfg
            .get("max_context_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
        mode: cfg
            .get("mode")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
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
    fn parses_settings_command() {
        let cmd = parse_ui_command(
            r#"{"type":"save_settings","config":{"api_base":"https://example.test","model":"m","mode":"coding","max_context_tokens":100000}}"#,
        );

        match cmd {
            UiToOverlayCommand::SaveSettings(settings) => {
                assert_eq!(settings.api_base, "https://example.test");
                assert_eq!(settings.model, "m");
                assert_eq!(settings.mode.as_deref(), Some("coding"));
                assert_eq!(settings.max_context_tokens, Some(100000));
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
        let commands: Vec<_> = chat_coding_smoke_commands()
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
}
