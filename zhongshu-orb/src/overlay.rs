use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use glib;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use wry::WebViewBuilderExtUnix;

// ── Message types ────────────────────────────────────────────────────

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
    pub api_base: String,
    pub model: String,
    pub personality: String,
    pub proxy_port: Option<String>,
    pub bg_enabled: Option<bool>,
    pub bg_interval: Option<String>,
    pub bg_prompt: Option<String>,
    pub auto_evolve: Option<bool>,
    pub max_context_tokens: Option<u32>,
}

// ── Global GTK thread state ─────────────────────────────────────────

pub(crate) enum GtkCommand {
    Eval(String),
    Show(f32, f32),
    Hide,
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
            window.set_decorated(true);
            window.set_resizable(true);
            window.connect_delete_event(|w, _| {
                w.hide();
                glib::Propagation::Stop
            });

            let html = include_str!("../assets/chat.html");

            let webview = wry::WebViewBuilder::new()
                .with_html(html)
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
                            window.set_default_size(w as i32, h as i32);
                            window.show_all();
                        }
                        GtkCommand::Hide => {
                            window.hide();
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

    pub fn take_cancel_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_cancel_task.lock().unwrap())
    }

    pub fn take_complete_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_complete_task.lock().unwrap())
    }

    pub fn show_tasks(&self, tasks: &[serde_json::Value]) {
        self.send(&json!({ "type": "tasks", "tasks": tasks }));
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        let _ = GTK_TX.send(GtkCommand::Hide);
    }
}

/// Show the overlay window and return a handle for IPC.
pub fn show(width: f32, height: f32) -> OverlayHandle {
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
    let pct = pending_cancel_task.clone();
    let pcmt = pending_complete_task.clone();

    // Install IPC handler that writes to these shared queues
    *IPC_HANDLER.lock().unwrap() = Some(Box::new(move |request: http::Request<String>| {
        let body = request.body();
        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(body) {
            match msg["type"].as_str() {
                Some("submit") => {
                    if let Some(text) = msg["text"].as_str() {
                        pi.lock().unwrap().push_back(text.to_string());
                    }
                }
                Some("stop") => {
                    *rs.lock().unwrap() = true;
                }
                Some("new_conversation") => {
                    *rnc.lock().unwrap() = true;
                }
                Some("approve") => {
                    let rid = msg["request_id"].as_str().unwrap_or("").to_string();
                    *pa.lock().unwrap() = Some(rid);
                }
                Some("deny") => {
                    let rid = msg["request_id"].as_str().unwrap_or("").to_string();
                    *pd.lock().unwrap() = Some(rid);
                }
                Some("pick_personality") => {
                    if let Some(p) = msg["personality"].as_str() {
                        *pp.lock().unwrap() = Some(p.to_string());
                    }
                }
                Some("save_settings") => {
                    if let Some(cfg) = msg["config"].as_object() {
                        *ps.lock().unwrap() = Some(SettingsConfig {
                            api_key: cfg
                                .get("api_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
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
                                .map(|s| s.to_string()),
                            bg_enabled: cfg.get("bg_enabled").and_then(|v| v.as_bool()),
                            bg_interval: cfg
                                .get("bg_interval")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            bg_prompt: cfg
                                .get("bg_prompt")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            auto_evolve: cfg.get("auto_evolve").and_then(|v| v.as_bool()),
                            max_context_tokens: cfg
                                .get("max_context_tokens")
                                .and_then(|v| v.as_u64())
                                .map(|n| n as u32),
                        });
                    }
                }
                Some("open_settings") => {
                    *pos.lock().unwrap() = true;
                }
                Some("delete_history") => {
                    *rnc.lock().unwrap() = true;
                }
                Some("load_more") => {
                    *plm.lock().unwrap() = true;
                }
                Some("list_tasks") => {
                    *plt.lock().unwrap() = true;
                }
                Some("cancel_task") => {
                    if let Some(id) = msg["task_id"].as_str() {
                        *pct.lock().unwrap() = Some(id.to_string());
                    }
                }
                Some("complete_task") => {
                    if let Some(id) = msg["task_id"].as_str() {
                        *pcmt.lock().unwrap() = Some(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }));

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
        pending_cancel_task,
        pending_complete_task,
        request_quit: false,
        personality_selected: false,
    }
}
