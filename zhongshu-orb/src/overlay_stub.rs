// Windows stub: no-op overlay (no GTK/WebView available)
// Provides the same public API as overlay.rs so handler.rs compiles unchanged.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntry {
    pub name: String,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Done { success: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEntry {
    pub role: EntryRole,
    pub content: String,
    pub tool_calls: Vec<ToolCallEntry>,
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
    pub max_context_tokens: Option<u32>,
    pub mode: Option<String>,
}

pub struct OverlayHandle {
    pub pending_input: Arc<Mutex<VecDeque<String>>>,
    pub pending_approve: Arc<Mutex<Option<String>>>,
    pub pending_deny: Arc<Mutex<Option<String>>>,
    pub pending_personality: Arc<Mutex<Option<String>>>,
    pub pending_settings: Arc<Mutex<Option<SettingsConfig>>>,
    pub request_new_conversation: Arc<Mutex<bool>>,
    pub request_stop: Arc<Mutex<bool>>,
    pub pending_open_settings: Arc<Mutex<bool>>,
    pub pending_load_more: Arc<Mutex<bool>>,
    pub pending_list_tasks: Arc<Mutex<bool>>,
    pub pending_list_runbooks: Arc<Mutex<bool>>,
    pub pending_list_equipment: Arc<Mutex<bool>>,
    pub pending_toggle_equipment: Arc<Mutex<Option<String>>>,
    pub pending_toggle_zoom: Arc<Mutex<bool>>,
    pub pending_cancel_task: Arc<Mutex<Option<String>>>,
    pub pending_complete_task: Arc<Mutex<Option<String>>>,
    pub request_quit: bool,
    pub personality_selected: bool,
}

impl OverlayHandle {
    pub fn eval(&self, _js: &str) {}
    pub fn send(&self, _msg: &serde_json::Value) {}
    pub fn push_delta(&self, _content: &str) {}
    pub fn complete_message(&self) {}
    pub fn set_history(&self, _entries: &[ChatEntry], _has_more: bool) {}
    pub fn prepend_history(&self, _entries: &[ChatEntry], _has_more: bool) {}
    pub fn show_auth(&self, _req: &AuthRequest) {}
    pub fn show_settings(&self, _config: &SettingsConfig) {}
    pub fn show_personality_picker(&self) {}
    pub fn clear_chat(&self) {}
    pub fn toast(&self, _text: &str) {}
    pub fn set_state(&self, _state: &str) {}
    pub fn show_window(&self, _width: f32, _height: f32) {}
    pub fn take_input(&self) -> Option<String> {
        None
    }
    pub fn take_approve(&self) -> Option<String> {
        None
    }
    pub fn take_deny(&self) -> Option<String> {
        None
    }
    pub fn take_personality(&self) -> Option<String> {
        None
    }
    pub fn take_settings(&self) -> Option<SettingsConfig> {
        None
    }
    pub fn take_new_conversation(&self) -> bool {
        false
    }
    pub fn take_stop(&self) -> bool {
        false
    }
    pub fn take_open_settings(&self) -> bool {
        false
    }
    pub fn take_load_more(&self) -> bool {
        false
    }
    pub fn take_list_tasks(&self) -> bool {
        false
    }
    pub fn take_list_runbooks(&self) -> bool {
        false
    }
    pub fn take_list_equipment(&self) -> bool {
        false
    }
    pub fn take_toggle_equipment(&self) -> Option<String> {
        None
    }
    pub fn take_toggle_zoom(&self) -> bool {
        false
    }
    pub fn take_start_drag(&self) -> bool {
        false
    }
    pub fn start_drag_window(&self) {}
    pub fn take_minimize(&self) -> bool {
        false
    }
    pub fn take_maximize_restore(&self) -> bool {
        false
    }
    pub fn take_close_window(&self) -> bool {
        false
    }
    pub fn minimize_window(&self) {}
    pub fn maximize_restore_window(&self) {}
    pub fn close_window(&self) {}
    pub fn take_cancel_task(&self) -> Option<String> {
        None
    }
    pub fn take_complete_task(&self) -> Option<String> {
        None
    }
    pub fn show_tasks(&self, _tasks: &[serde_json::Value]) {}
    pub fn show_runbooks(&self, _runbooks: &[serde_json::Value]) {}
    pub fn show_equipment(&self, _items: &[serde_json::Value]) {}
}

pub fn show(_width: f32, _height: f32) -> OverlayHandle {
    tracing::info!("overlay stub: overlay not available on Windows");
    OverlayHandle {
        pending_input: Default::default(),
        pending_approve: Default::default(),
        pending_deny: Default::default(),
        pending_personality: Default::default(),
        pending_settings: Default::default(),
        request_new_conversation: Default::default(),
        request_stop: Default::default(),
        pending_open_settings: Default::default(),
        pending_load_more: Default::default(),
        pending_list_tasks: Default::default(),
        pending_list_runbooks: Default::default(),
        pending_list_equipment: Default::default(),
        pending_toggle_equipment: Default::default(),
        pending_toggle_zoom: Default::default(),
        pending_cancel_task: Default::default(),
        pending_complete_task: Default::default(),
        request_quit: false,
        personality_selected: false,
    }
}
