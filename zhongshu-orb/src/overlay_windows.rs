use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};
use wry::{Rect, WebViewBuilder};

use crate::overlay_assets::{legacy_chat_html, select_overlay_asset, OverlayAsset};
use crate::overlay_contract::{parse_ui_command, UiToOverlayCommand};

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
    pub pending_start_drag: Arc<Mutex<bool>>,
    pub pending_cancel_task: Arc<Mutex<Option<String>>>,
    pub pending_complete_task: Arc<Mutex<Option<String>>>,
    #[allow(dead_code)]
    pub request_quit: bool,
    #[allow(dead_code)]
    pub personality_selected: bool,
    window: Window,
    webview: Option<wry::WebView>,
    startup_error: Option<String>,
}

impl OverlayHandle {
    pub fn eval(&self, js: &str) {
        if let Some(webview) = self.webview.as_ref() {
            if let Err(e) = webview.evaluate_script(js) {
                tracing::warn!("windows webview eval error: {e}");
            }
        }
    }

    pub fn send(&self, msg: &serde_json::Value) {
        let js = format!(
            "window.handleIpc({})",
            serde_json::to_string(msg).unwrap_or_default()
        );
        self.eval(&js);
    }

    pub fn push_delta(&self, content: &str) {
        self.send(&json!({ "type": "delta", "content": content }));
    }

    pub fn complete_message(&self) {
        self.send(&json!({ "type": "complete" }));
    }

    pub fn set_history(&self, entries: &[ChatEntry], has_more: bool) {
        self.send(&json!({ "type": "history", "entries": entries, "has_more": has_more }));
    }

    pub fn prepend_history(&self, entries: &[ChatEntry], has_more: bool) {
        self.send(&json!({ "type": "prepend_history", "entries": entries, "has_more": has_more }));
    }

    pub fn show_auth(&self, req: &AuthRequest) {
        self.send(&json!({ "type": "auth", "request": req }));
    }

    pub fn show_settings(&self, config: &SettingsConfig) {
        self.send(&json!({ "type": "settings", "config": config }));
    }

    #[allow(dead_code)]
    pub fn show_personality_picker(&self) {
        self.send(&json!({ "type": "show_personality" }));
    }

    pub fn clear_chat(&self) {
        self.send(&json!({ "type": "clear" }));
    }

    pub fn toast(&self, text: &str) {
        self.send(&json!({ "type": "toast", "text": text }));
    }

    pub fn set_state(&self, state: &str) {
        self.send(&json!({ "type": "state_change", "state": state }));
    }

    pub fn show_window(&self, width: f32, height: f32) {
        let width = width.clamp(360.0, 2400.0);
        let height = height.clamp(520.0, 1600.0);
        let _ = self
            .window
            .request_inner_size(LogicalSize::new(width, height));
        self.window.set_visible(true);
        self.window.set_window_level(WindowLevel::AlwaysOnTop);
        self.window.focus_window();
        self.resize_webview();
        if self.startup_error.is_some() {
            self.window.request_user_attention(None);
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        Some(self.window.id())
    }

    pub fn handle_window_event(&self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => {
                self.window.set_visible(false);
                true
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.resize_webview();
                true
            }
            _ => false,
        }
    }

    pub fn resize_webview(&self) {
        let Some(webview) = self.webview.as_ref() else {
            return;
        };
        let size = self
            .window
            .inner_size()
            .to_logical::<u32>(self.window.scale_factor());
        if let Err(e) = webview.set_bounds(Rect {
            position: LogicalPosition::new(0, 0).into(),
            size: LogicalSize::new(size.width, size.height).into(),
        }) {
            tracing::warn!("windows webview resize error: {e}");
        }
    }

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
        std::mem::take(&mut *self.pending_start_drag.lock().unwrap())
    }

    pub fn start_drag_window(&self) {
        if let Err(e) = self.window.drag_window() {
            tracing::warn!("windows overlay drag_window failed: {e}");
        }
    }

    pub fn take_cancel_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_cancel_task.lock().unwrap())
    }

    pub fn take_complete_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_complete_task.lock().unwrap())
    }

    pub fn show_tasks(&self, tasks: &[serde_json::Value]) {
        self.send(&json!({ "type": "tasks", "tasks": tasks }));
    }

    pub fn show_runbooks(&self, runbooks: &[serde_json::Value]) {
        self.send(&json!({ "type": "runbooks", "runbooks": runbooks }));
    }

    pub fn show_equipment(&self, items: &[serde_json::Value]) {
        self.send(&json!({ "type": "equipment", "items": items }));
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        self.window.set_visible(false);
    }
}

pub fn show(event_loop: &ActiveEventLoop, width: f32, height: f32) -> OverlayHandle {
    let pending_input: Arc<Mutex<VecDeque<String>>> = Default::default();
    let pending_approve: Arc<Mutex<Option<String>>> = Default::default();
    let pending_deny: Arc<Mutex<Option<String>>> = Default::default();
    let pending_personality: Arc<Mutex<Option<String>>> = Default::default();
    let pending_settings: Arc<Mutex<Option<SettingsConfig>>> = Default::default();
    let request_new_conversation: Arc<Mutex<bool>> = Default::default();
    let request_stop: Arc<Mutex<bool>> = Default::default();
    let pending_open_settings: Arc<Mutex<bool>> = Default::default();
    let pending_load_more: Arc<Mutex<bool>> = Default::default();
    let pending_list_tasks: Arc<Mutex<bool>> = Default::default();
    let pending_list_runbooks: Arc<Mutex<bool>> = Default::default();
    let pending_list_equipment: Arc<Mutex<bool>> = Default::default();
    let pending_toggle_equipment: Arc<Mutex<Option<String>>> = Default::default();
    let pending_toggle_zoom: Arc<Mutex<bool>> = Default::default();
    let pending_start_drag: Arc<Mutex<bool>> = Default::default();
    let pending_cancel_task: Arc<Mutex<Option<String>>> = Default::default();
    let pending_complete_task: Arc<Mutex<Option<String>>> = Default::default();

    let attrs = Window::default_attributes()
        .with_title("Zhongshu")
        .with_inner_size(LogicalSize::new(
            width.clamp(360.0, 2400.0),
            height.clamp(520.0, 1600.0),
        ))
        .with_min_inner_size(LogicalSize::new(360.0, 520.0))
        .with_decorations(false)
        .with_resizable(true)
        .with_visible(false)
        .with_window_level(WindowLevel::AlwaysOnTop);

    let window = event_loop
        .create_window(attrs)
        .expect("windows overlay window creation failed");

    let asset = select_overlay_asset();
    match &asset {
        OverlayAsset::React { index_path, .. } => {
            tracing::info!(
                "windows overlay loading inlined react UI from {}",
                index_path.display()
            );
        }
        OverlayAsset::LegacyHtml { reason } => {
            tracing::info!("windows overlay loading legacy UI: {reason}");
        }
    }

    let builder = match asset {
        OverlayAsset::React { html, .. } => WebViewBuilder::new().with_html(html),
        OverlayAsset::LegacyHtml { .. } => WebViewBuilder::new().with_html(legacy_chat_html()),
    };

    let pi = pending_input.clone();
    let pa = pending_approve.clone();
    let pd = pending_deny.clone();
    let pp = pending_personality.clone();
    let ps = pending_settings.clone();
    let rnc = request_new_conversation.clone();
    let rs = request_stop.clone();
    let pos = pending_open_settings.clone();
    let plm = pending_load_more.clone();
    let plt = pending_list_tasks.clone();
    let plr = pending_list_runbooks.clone();
    let ple = pending_list_equipment.clone();
    let pte = pending_toggle_equipment.clone();
    let ptz = pending_toggle_zoom.clone();
    let psd = pending_start_drag.clone();
    let pct = pending_cancel_task.clone();
    let pcmt = pending_complete_task.clone();

    let mut startup_error = None;
    let webview = match builder
        .with_user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Zhongshu/1.0")
        .with_ipc_handler(move |request: http::Request<String>| match parse_ui_command(request.body()) {
            UiToOverlayCommand::Submit(text) => {
                pi.lock().unwrap().push_back(text);
            }
            UiToOverlayCommand::Stop => {
                *rs.lock().unwrap() = true;
            }
            UiToOverlayCommand::NewConversation | UiToOverlayCommand::DeleteHistory => {
                *rnc.lock().unwrap() = true;
            }
            UiToOverlayCommand::Approve(rid) => {
                *pa.lock().unwrap() = Some(rid);
            }
            UiToOverlayCommand::Deny(rid) => {
                *pd.lock().unwrap() = Some(rid);
            }
            UiToOverlayCommand::PickPersonality(personality) => {
                *pp.lock().unwrap() = Some(personality);
            }
            UiToOverlayCommand::SaveSettings(settings) => {
                *ps.lock().unwrap() = Some(settings);
            }
            UiToOverlayCommand::OpenSettings => {
                *pos.lock().unwrap() = true;
            }
            UiToOverlayCommand::LoadMore => {
                *plm.lock().unwrap() = true;
            }
            UiToOverlayCommand::ListTasks => {
                *plt.lock().unwrap() = true;
            }
            UiToOverlayCommand::ListRunbooks => {
                *plr.lock().unwrap() = true;
            }
            UiToOverlayCommand::ListEquipment => {
                *ple.lock().unwrap() = true;
            }
            UiToOverlayCommand::ToggleEquipment(id) => {
                *pte.lock().unwrap() = Some(id);
            }
            UiToOverlayCommand::ToggleZoom => {
                *ptz.lock().unwrap() = true;
            }
            UiToOverlayCommand::StartDrag => {
                *psd.lock().unwrap() = true;
            }
            UiToOverlayCommand::CancelTask(id) => {
                *pct.lock().unwrap() = Some(id);
            }
            UiToOverlayCommand::CompleteTask(id) => {
                *pcmt.lock().unwrap() = Some(id);
            }
            UiToOverlayCommand::Unknown => {}
        })
        .build_as_child(&window)
    {
        Ok(webview) => Some(webview),
        Err(e) => {
            let message = format!("WebView2 unavailable: {e}");
            tracing::error!("{message}");
            window.set_title(&format!("Zhongshu - {message}"));
            startup_error = Some(message);
            None
        }
    };

    let handle = OverlayHandle {
        pending_input,
        pending_approve,
        pending_deny,
        pending_personality,
        pending_settings,
        request_new_conversation,
        request_stop,
        pending_open_settings,
        pending_load_more,
        pending_list_tasks,
        pending_list_runbooks,
        pending_list_equipment,
        pending_toggle_equipment,
        pending_toggle_zoom,
        pending_start_drag,
        pending_cancel_task,
        pending_complete_task,
        request_quit: false,
        personality_selected: false,
        window,
        webview,
        startup_error,
    };
    handle.show_window(width, height);
    handle
}
