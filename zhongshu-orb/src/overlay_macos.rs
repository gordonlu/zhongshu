#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::overlay_host::{OverlayHostCommand, OverlayHostCommandQueue, OverlayHostDiagnostics};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, ToolCallEntry, ToolStatus,
};

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
    pub host_commands: OverlayHostCommandQueue,
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
        self.pending_input.lock().unwrap().pop_front()
    }
    pub fn take_approve(&self) -> Option<String> {
        self.pending_approve.lock().unwrap().take()
    }
    pub fn take_deny(&self) -> Option<String> {
        self.pending_deny.lock().unwrap().take()
    }
    pub fn take_personality(&self) -> Option<String> {
        self.pending_personality.lock().unwrap().take()
    }
    pub fn take_settings(&self) -> Option<SettingsConfig> {
        self.pending_settings.lock().unwrap().take()
    }
    pub fn take_new_conversation(&self) -> bool {
        std::mem::take(&mut *self.request_new_conversation.lock().unwrap())
    }
    pub fn take_stop(&self) -> bool {
        std::mem::take(&mut *self.request_stop.lock().unwrap())
    }
    pub fn take_open_settings(&self) -> bool {
        std::mem::take(&mut *self.pending_open_settings.lock().unwrap())
    }
    pub fn take_load_more(&self) -> bool {
        std::mem::take(&mut *self.pending_load_more.lock().unwrap())
    }
    pub fn take_list_tasks(&self) -> bool {
        std::mem::take(&mut *self.pending_list_tasks.lock().unwrap())
    }
    pub fn take_list_runbooks(&self) -> bool {
        std::mem::take(&mut *self.pending_list_runbooks.lock().unwrap())
    }
    pub fn take_list_equipment(&self) -> bool {
        std::mem::take(&mut *self.pending_list_equipment.lock().unwrap())
    }
    pub fn take_toggle_equipment(&self) -> Option<String> {
        self.pending_toggle_equipment.lock().unwrap().take()
    }
    pub fn take_toggle_zoom(&self) -> bool {
        std::mem::take(&mut *self.pending_toggle_zoom.lock().unwrap())
    }
    pub fn take_start_drag(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::StartDrag)
    }
    pub fn start_drag_window(&self) {}
    pub fn take_minimize(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::Minimize)
    }
    pub fn take_maximize_restore(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::MaximizeRestore)
    }
    pub fn take_close_window(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::CloseWindow)
    }
    pub fn minimize_window(&self) {}
    pub fn maximize_restore_window(&self) {}
    pub fn close_window(&self) {}
    pub fn take_cancel_task(&self) -> Option<String> {
        self.pending_cancel_task.lock().unwrap().take()
    }
    pub fn take_complete_task(&self) -> Option<String> {
        self.pending_complete_task.lock().unwrap().take()
    }
    pub fn show_tasks(&self, _tasks: &[serde_json::Value]) {}
    pub fn show_runbooks(&self, _runbooks: &[serde_json::Value]) {}
    pub fn show_equipment(&self, _items: &[serde_json::Value]) {}
    pub fn window_id(&self) -> Option<WindowId> {
        None
    }
    pub fn handle_window_event(&self, _event: &WindowEvent) -> bool {
        false
    }
    pub fn host_diagnostics(&self) -> OverlayHostDiagnostics {
        OverlayHostDiagnostics {
            platform: "macos".to_string(),
            webview_available: false,
            startup_error: Some("macOS overlay host not implemented yet".to_string()),
        }
    }
}

pub fn show(_event_loop: &ActiveEventLoop, _width: f32, _height: f32) -> OverlayHandle {
    tracing::warn!("macOS overlay host shape exists but is not implemented yet");
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
        host_commands: Default::default(),
        pending_cancel_task: Default::default(),
        pending_complete_task: Default::default(),
        request_quit: false,
        personality_selected: false,
    }
}
