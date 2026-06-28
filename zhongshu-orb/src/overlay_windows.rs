#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::window::{Window, WindowId, WindowLevel};
use wry::{Rect, WebViewBuilder};

use crate::overlay_contract::{parse_ui_command, UiToOverlayCommand};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, ToolCallEntry, ToolStatus,
};

enum WindowsCommand {
    Eval(String),
    Show(f32, f32),
    Hide,
}

static WINDOWS_TX: once_cell::sync::Lazy<crossbeam_channel::Sender<WindowsCommand>> =
    once_cell::sync::Lazy::new(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<WindowsCommand>();
        std::thread::spawn(move || run_windows_overlay(rx));
        tx
    });

static IPC_HANDLER: once_cell::sync::Lazy<
    Mutex<Option<Box<dyn Fn(http::Request<String>) + Send>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(None));

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
    pub fn eval(&self, js: &str) {
        if let Err(e) = WINDOWS_TX.send(WindowsCommand::Eval(js.to_string())) {
            tracing::warn!("windows overlay tx send error: {e}");
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
        if let Err(e) = WINDOWS_TX.send(WindowsCommand::Show(width, height)) {
            tracing::warn!("windows overlay show send error: {e}");
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
        let _ = WINDOWS_TX.send(WindowsCommand::Hide);
    }
}

pub fn show(width: f32, height: f32) -> OverlayHandle {
    let _ = &*WINDOWS_TX;

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
    let pending_cancel_task: Arc<Mutex<Option<String>>> = Default::default();
    let pending_complete_task: Arc<Mutex<Option<String>>> = Default::default();

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
    let pct = pending_cancel_task.clone();
    let pcmt = pending_complete_task.clone();

    *IPC_HANDLER.lock().unwrap() =
        Some(Box::new(
            move |request: http::Request<String>| match parse_ui_command(request.body()) {
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
                UiToOverlayCommand::CancelTask(id) => {
                    *pct.lock().unwrap() = Some(id);
                }
                UiToOverlayCommand::CompleteTask(id) => {
                    *pcmt.lock().unwrap() = Some(id);
                }
                UiToOverlayCommand::Unknown => {}
            },
        ));

    let _ = WINDOWS_TX.send(WindowsCommand::Show(width, height));

    OverlayHandle {
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
        pending_cancel_task,
        pending_complete_task,
        request_quit: false,
        personality_selected: false,
    }
}

fn run_windows_overlay(rx: crossbeam_channel::Receiver<WindowsCommand>) {
    let mut builder = EventLoop::<WindowsCommand>::with_user_event();
    builder.with_any_thread(true);
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            tracing::error!("windows overlay event loop failed: {e}");
            return;
        }
    };
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            if proxy.send_event(cmd).is_err() {
                break;
            }
        }
    });

    let mut app = WindowsOverlayApp::default();
    if let Err(e) = event_loop.run_app(&mut app) {
        tracing::error!("windows overlay loop exited: {e}");
    }
}

#[derive(Default)]
struct WindowsOverlayApp {
    window: Option<Window>,
    window_id: Option<WindowId>,
    webview: Option<wry::WebView>,
    startup_error: Option<String>,
}

impl ApplicationHandler<WindowsCommand> for WindowsOverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Zhongshu")
            .with_inner_size(LogicalSize::new(520.0, 800.0))
            .with_min_inner_size(LogicalSize::new(360.0, 520.0))
            .with_decorations(false)
            .with_resizable(true)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop);

        let window = match event_loop.create_window(attrs) {
            Ok(window) => window,
            Err(e) => {
                tracing::error!("windows overlay window creation failed: {e}");
                return;
            }
        };
        let window_id = window.id();
        let html = include_str!("../assets/chat.html");

        match WebViewBuilder::new()
            .with_html(html)
            .with_user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Zhongshu/1.0")
            .with_ipc_handler(move |request: http::Request<String>| {
                IPC_HANDLER.lock().unwrap().as_ref().map(|h| h(request));
            })
            .build_as_child(&window)
        {
            Ok(webview) => {
                self.webview = Some(webview);
            }
            Err(e) => {
                let message = format!("WebView2 unavailable: {e}");
                tracing::error!("{message}");
                window.set_title(&format!("Zhongshu - {message}"));
                self.startup_error = Some(message);
            }
        }

        self.window_id = Some(window_id);
        self.window = Some(window);
        self.resize_webview();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowsCommand) {
        match event {
            WindowsCommand::Eval(js) => {
                if let Some(webview) = self.webview.as_ref() {
                    if let Err(e) = webview.evaluate_script(&js) {
                        tracing::warn!("windows webview eval error: {e}");
                    }
                }
            }
            WindowsCommand::Show(width, height) => {
                if let Some(window) = self.window.as_ref() {
                    let width = width.clamp(360.0, 2400.0);
                    let height = height.clamp(520.0, 1600.0);
                    let _ = window.request_inner_size(LogicalSize::new(width, height));
                    window.set_visible(true);
                    window.set_window_level(WindowLevel::AlwaysOnTop);
                    window.focus_window();
                    self.resize_webview();
                    if self.startup_error.is_some() {
                        window.request_user_attention(None);
                    }
                }
            }
            WindowsCommand::Hide => {
                if let Some(window) = self.window.as_ref() {
                    window.set_visible(false);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if Some(window_id) != self.window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                if let Some(window) = self.window.as_ref() {
                    window.set_visible(false);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.resize_webview();
            }
            _ => {}
        }
    }
}

impl WindowsOverlayApp {
    fn resize_webview(&self) {
        let (Some(window), Some(webview)) = (self.window.as_ref(), self.webview.as_ref()) else {
            return;
        };
        let size = window.inner_size().to_logical::<u32>(window.scale_factor());
        if let Err(e) = webview.set_bounds(Rect {
            position: LogicalPosition::new(0, 0).into(),
            size: LogicalSize::new(size.width, size.height).into(),
        }) {
            tracing::warn!("windows webview resize error: {e}");
        }
    }
}
