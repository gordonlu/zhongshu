use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use glib;
use gtk::gdk::prelude::MonitorExt;
use gtk::prelude::*;
use serde_json::json;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use wry::WebViewBuilderExtUnix;

use crate::overlay_assets::select_overlay_asset;
use crate::overlay_contract::{parse_ui_command, UiToOverlayCommand};
use crate::overlay_host::{log_selected_asset, webview_builder_for_asset};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, ToolCallEntry, ToolStatus,
};

// ── Message types ────────────────────────────────────────────────────

// ── Global GTK thread state ─────────────────────────────────────────

pub(crate) enum GtkCommand {
    Eval(String),
    Show(f32, f32),
    Hide,
    Minimize,
    MaximizeRestore,
    CloseWindow,
    StartDrag,
}

pub(crate) static GTK_TX: once_cell::sync::Lazy<crossbeam_channel::Sender<GtkCommand>> =
    once_cell::sync::Lazy::new(|| {
        let (tx_i, rx_i) = crossbeam_channel::unbounded::<GtkCommand>();
        std::thread::spawn(move || {
            gtk::init().expect("GTK init failed");
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.set_title("Zhongshu");
            window.set_default_size(520, 800);
            window.set_default_size(520, 800);
            window.set_decorated(false);
            window.set_resizable(true);
            window.connect_delete_event(|w, _| {
                w.hide();
                glib::Propagation::Stop
            });

            let asset = select_overlay_asset();
            log_selected_asset("gtk", &asset);
            let builder = webview_builder_for_asset(asset);

            let webview = builder
                .with_user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.7827.102 Safari/537.36")
                .with_ipc_handler(move |request: http::Request<String>| {
                    IPC_HANDLER.lock().unwrap().as_ref().map(|h| h(request));
                })
                .build_gtk(&window)
                .expect("wry WebView build_gtk failed");

            window.show_all();

            glib::idle_add_local(move || {
                while let Ok(cmd) = rx_i.try_recv() {
                    match cmd {
                        GtkCommand::Eval(js) => {
                            if let Err(e) = webview.evaluate_script(&js) {
                                tracing::warn!("webview eval error: {e}");
                            }
                        }
                        GtkCommand::Show(w, h) => {
                            let (screen_w, screen_h) = gtk::gdk::Display::default()
                                .and_then(|display| display.primary_monitor())
                                .map(|monitor| {
                                    let area = monitor.workarea();
                                    (area.width() as f32, area.height() as f32)
                                })
                                .unwrap_or((1280.0, 900.0));
                            let max_w = (screen_w * 0.96).max(320.0);
                            let max_h = (screen_h * 0.92).max(480.0);
                            let clamped_w = w.min(max_w).max(360.0) as i32;
                            let clamped_h = h.min(max_h).max(520.0) as i32;
                            window.resize(clamped_w, clamped_h);
                            window.set_default_size(clamped_w, clamped_h);
                            window.show_all();
                        }
                        GtkCommand::Hide => {
                            window.hide();
                        }
                        GtkCommand::Minimize => {
                            window.iconify();
                        }
                        GtkCommand::MaximizeRestore => {
                            if window.is_maximized() {
                                window.unmaximize();
                            } else {
                                window.maximize();
                            }
                        }
                        GtkCommand::CloseWindow => {
                            window.close();
                        }
                        GtkCommand::StartDrag => {
                            window.begin_move_drag(1, 0, 0, 0);
                        }
                    }
                }
                glib::ControlFlow::Continue
            });

            gtk::main();
        });
        tx_i
    });

/// Thread-safe IPC handler set by the current OverlayHandle.
static IPC_HANDLER: once_cell::sync::Lazy<
    Mutex<Option<Box<dyn Fn(http::Request<String>) + Send>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(None));

// ── Overlay handle ───────────────────────────────────────────────────

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
    #[allow(dead_code)]
    pub request_quit: bool,
    #[allow(dead_code)]
    pub personality_selected: bool,
}

impl OverlayHandle {
    pub fn eval(&self, js: &str) {
        if let Err(e) = GTK_TX.send(GtkCommand::Eval(js.to_string())) {
            tracing::warn!("gtk tx send error: {e}");
        }
    }

    pub fn send(&self, msg: &serde_json::Value) {
        // Build: window.handleIpc({"type":"delta","content":"..."})
        // serde_json::to_string gives {"type":"delta","content":"..."} which is valid JS object literal
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

    #[allow(dead_code)]
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

    /// Show the window if it's hidden (re-opens from tray/notification).
    pub fn show_window(&self, width: f32, height: f32) {
        let _ = GTK_TX.send(GtkCommand::Show(width, height));
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
        false
    }

    pub fn take_minimize(&self) -> bool {
        false
    }

    pub fn take_maximize_restore(&self) -> bool {
        false
    }

    pub fn take_close_window(&self) -> bool {
        false
    }

    pub fn start_drag_window(&self) {
        let _ = GTK_TX.send(GtkCommand::StartDrag);
    }

    pub fn minimize_window(&self) {
        let _ = GTK_TX.send(GtkCommand::Minimize);
    }

    pub fn maximize_restore_window(&self) {
        let _ = GTK_TX.send(GtkCommand::MaximizeRestore);
    }

    pub fn close_window(&self) {
        let _ = GTK_TX.send(GtkCommand::CloseWindow);
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

    pub fn window_id(&self) -> Option<WindowId> {
        None
    }

    pub fn handle_window_event(&self, _event: &WindowEvent) -> bool {
        false
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        let _ = GTK_TX.send(GtkCommand::Hide);
    }
}

/// Show the overlay window and return a handle for IPC.
pub fn show(_event_loop: &ActiveEventLoop, width: f32, height: f32) -> OverlayHandle {
    // Initialize GTK thread (on first call only)
    let _ = *GTK_TX;

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

    // Install IPC handler that writes to these shared queues
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
                UiToOverlayCommand::StartDrag => {
                    let _ = GTK_TX.send(GtkCommand::StartDrag);
                }
                UiToOverlayCommand::Minimize => {
                    let _ = GTK_TX.send(GtkCommand::Minimize);
                }
                UiToOverlayCommand::MaximizeRestore => {
                    let _ = GTK_TX.send(GtkCommand::MaximizeRestore);
                }
                UiToOverlayCommand::CloseWindow => {
                    let _ = GTK_TX.send(GtkCommand::CloseWindow);
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

    let _ = GTK_TX.send(GtkCommand::Show(width, height));

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
