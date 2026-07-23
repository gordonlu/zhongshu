// Windows stub: no-op overlay (no GTK/WebView available)
// Provides the same public API as overlay.rs so handler.rs compiles unchanged.
#![allow(dead_code)]

use std::ops::Deref;

use serde::{Deserialize, Serialize};

use crate::overlay_host::{OverlayHandleExt, OverlayHostCommand, OverlayHostDiagnostics, OverlayState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntry {
    pub name: String,
    pub status: ToolStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Done { success: bool },
    Cancelled { reason: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEntry {
    pub role: EntryRole,
    pub content: String,
    pub tool_calls: Vec<ToolCallEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntryRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub request_id: String,
    pub source: String,
    pub tool: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<PatchDiffPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub struct OverlayHandle {
    pub state: OverlayState,
}

impl Deref for OverlayHandle {
    type Target = OverlayState;
    fn deref(&self) -> &OverlayState {
        &self.state
    }
}

impl OverlayHandleExt for OverlayHandle {
    fn webview_eval(&self, _js: &str) {}
}

impl OverlayHandle {
    pub fn eval(&self, _js: &str) {}

    pub fn show_window(&self, _width: f32, _height: f32) {}

    pub fn host_diagnostics(&self) -> OverlayHostDiagnostics {
        OverlayHostDiagnostics {
            platform: "stub".to_string(),
            webview_available: false,
            startup_error: Some("overlay host unavailable".to_string()),
        }
    }

    pub fn start_drag_window(&self) {}
    pub fn minimize_window(&self) {}
    pub fn maximize_restore_window(&self) {}
    pub fn close_window(&self) {}

    pub fn take_start_drag(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::StartDrag)
    }
    pub fn take_minimize(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::Minimize)
    }
    pub fn take_maximize_restore(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::MaximizeRestore)
    }
    pub fn take_close_window(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::CloseWindow)
    }
}

pub fn show(_width: f32, _height: f32) -> OverlayHandle {
    tracing::warn!("stub overlay host — no WebView available");
    OverlayHandle { state: OverlayState::new() }
}
