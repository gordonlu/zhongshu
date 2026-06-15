mod app;
mod indicator;
mod overlay;
mod hotkey;
mod config;
mod agent;

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use zhongshu_core::event::{
    AgentState,
    Event, AgentEvent, ToolEvent, TaskEvent,
    ResponseEvent, ResponseRole, MessageId,
    EventBus, EventLogger, EventRx, ResponseTx, ResponseRx,
};
use indicator::Indicator;
use app::{SessionState, AgentController, AgentInbox, TaskWorkerDispatcher};
use overlay::{OverlayHandle, AuthRequest};
use hotkey::HotkeyManager;
use config::AppConfig;
#[cfg(target_os = "linux")]
use indicator::tray::TrayEvent;
use zhongshu_core::tool::default_registry;
use zhongshu_core::agent::llm::OpenAiProvider;
use zhongshu_core::agent::{AgentBudget, AgentRuntime, AgentProfile, AttentionManager, AttentionDispatcher};
use zhongshu_core::rule::{Rule, RuleCondition, RuleTask, RuleEngine};
use zhongshu_core::heartbeat::Heartbeat;
use zhongshu_core::digest::DigestBuilder;
use zhongshu_core::task::{TaskScheduler, ReminderTrigger, FileWatchTrigger};
use zhongshu_core::authority::{self, AuthorityGate};
use zhongshu_core::source::{DiskUsageSource, BatterySource, SourceManager, TimerSource};
use tokio::sync::mpsc;

fn preflight_checks() {
    let bus = Arc::new(EventBus::new(4));
    let mut rx = bus.subscribe();
    bus.publish(Event::Agent(AgentEvent::StateChanged {
        from: AgentState::Idle, to: AgentState::Thinking,
    }));
    assert!(rx.try_recv().is_ok(), "preflight: event bus failed");
    let (tx, mut response_rx) = mpsc::channel::<ResponseEvent>(4);
    let id = MessageId::new();
    assert!(tx.try_send(ResponseEvent::MessageStarted { id, role: ResponseRole::System }).is_ok(), "preflight: response tx failed");
    assert!(response_rx.try_recv().is_ok(), "preflight: response rx failed");
    tracing::info!("preflight checks passed");
}

struct ZhongshuApp {
    config: AppConfig,
    controller: Arc<AgentController>,
    proxy: Arc<tokio::sync::Mutex<zhongshu_core::integration::DeeplosslessProxy>>,
    runtime: tokio::runtime::Runtime,
    inbox: Arc<AgentInbox>,
    indicator: Option<Indicator>,
    indicator_state: AgentState,
    overlay: Option<OverlayHandle>,
    event_bus: Arc<EventBus>,
    event_rx: EventRx,
    response_tx: ResponseTx,
    response_rx: ResponseRx,
    hotkey: HotkeyManager,
    last_activity: Instant,
    is_dragging: bool,
    cursor_pos: (f64, f64),
    drag_start_cursor: (f64, f64),
    drag_start_win: (i32, i32),
    ctrl_held: bool,
    pending_auth_notified: bool,
    history_cache: Vec<(String, String)>,
}

impl ZhongshuApp {
    fn new(
        config: AppConfig,
        controller: Arc<AgentController>,
        inbox: Arc<AgentInbox>,
        event_bus: Arc<EventBus>,
        event_rx: EventRx,
        response_tx: ResponseTx,
        response_rx: ResponseRx,
        proxy: zhongshu_core::integration::DeeplosslessProxy,
        runtime: tokio::runtime::Runtime,
    ) -> anyhow::Result<Self> {
        let hotkey = HotkeyManager::new(&config.hotkey).unwrap_or_else(|e| {
            tracing::warn!("Global hotkey unavailable: {e:#}"); HotkeyManager::passive()
        });
        Ok(ZhongshuApp {
            config, controller, inbox, indicator: None, indicator_state: AgentState::Idle,
            proxy: Arc::new(tokio::sync::Mutex::new(proxy)),
            runtime,
            overlay: None, event_bus, event_rx, response_tx, response_rx,
            hotkey, last_activity: Instant::now(),
            is_dragging: false, cursor_pos: (0.0, 0.0), drag_start_cursor: (0.0, 0.0), drag_start_win: (0, 0), ctrl_held: false,
            pending_auth_notified: false,
            history_cache: Vec::new(),
        })
    }

    // ── Event reducers ──────────────────────────────────────────────

    fn drain(&mut self) {
        let activity = self.reduce_events() | self.reduce_responses();
        self.poll_pending_auth();
        self.poll_overlay_actions();
        if activity { 
            self.last_activity = Instant::now();
        }
    }

    /// Poll pending actions from overlay IPC.
    fn poll_overlay_actions(&mut self) {
        let ov = match self.overlay.as_ref() {
            Some(ov) => ov,
            None => return,
        };

        if let Some(text) = ov.take_input() {
            self.inbox.submit(text);
        }
        if ov.take_approve() {
            authority::approve_pending();
        }
        if ov.take_deny() {
            authority::deny_pending();
        }
        if let Some(p) = ov.take_personality() {
            let mut cfg = config::load();
            cfg.agent.personality = p.clone();
            cfg.agent.personality_selected = true;
            config::save(&cfg);
            self.controller.set_system_prompt(cfg.agent.effective_system_prompt());
        }
        if let Some(settings) = ov.take_settings() {
            let mut cfg = config::load();
            if !settings.api_key.is_empty() { cfg.llm.api_key_env = settings.api_key; }
            if !settings.api_base.is_empty() { cfg.llm.api_base = settings.api_base; }
            if !settings.model.is_empty() { cfg.llm.model = settings.model; }
            if let Some(port) = settings.proxy_port { if !port.is_empty() { cfg.deeplossless.proxy_port = port.parse().unwrap_or(8081); } }
            if let Some(enabled) = settings.bg_enabled { cfg.agent.background.enabled = enabled; }
            if let Some(interval) = settings.bg_interval { if !interval.is_empty() { cfg.agent.background.interval_secs = interval.parse().unwrap_or(600); } }
            if let Some(prompt) = settings.bg_prompt { cfg.agent.background.prompt = prompt; }
            if let Some(evolve) = settings.auto_evolve { cfg.agent.auto_evolve = evolve; }
            if settings.personality != "默认" && !settings.personality.is_empty() {
                cfg.agent.personality = settings.personality;
                cfg.agent.personality_selected = true;
                self.controller.set_system_prompt(cfg.agent.effective_system_prompt());
            }
            config::save(&cfg);
        }
        if ov.take_new_conversation() {
            self.controller.set_chat_history(Vec::new());
            ov.clear_chat();
        }
        if ov.take_stop() {
            self.controller.cancel();
        }
        if ov.take_open_settings() {
            let cfg = config::load();
            ov.show_settings(&overlay::SettingsConfig {
                api_key: cfg.llm.api_key(),
                api_base: cfg.llm.api_base.clone(),
                model: cfg.llm.model.clone(),
                personality: cfg.agent.personality.clone(),
                proxy_port: Some(cfg.deeplossless.proxy_port.to_string()),
                bg_enabled: Some(cfg.agent.background.enabled),
                bg_interval: Some(cfg.agent.background.interval_secs.to_string()),
                bg_prompt: Some(cfg.agent.background.prompt.clone()),
                auto_evolve: Some(cfg.agent.auto_evolve),
            });
        }
        if ov.take_load_more() {
            const BATCH_SIZE: usize = 40; // 20 pairs
            let cache_len = self.history_cache.len();
            if cache_len > 0 {
                let take = BATCH_SIZE.min(cache_len);
                let split = cache_len - take;
                let batch: Vec<(String, String)> = self.history_cache.drain(split..).collect();
                let has_more = self.history_cache.len() > 0;
                let entries: Vec<overlay::ChatEntry> = batch.iter().map(|(role, content)| {
                    overlay::ChatEntry {
                        role: if role == "User" { overlay::EntryRole::User } else { overlay::EntryRole::Assistant },
                        content: content.clone(),
                        tool_calls: Vec::new(),
                    }
                }).collect();
                ov.prepend_history(&entries, has_more);
            }
        }
    }

    /// Poll for pending authority requests and show in overlay.
    fn poll_pending_auth(&mut self) {
        if let Some(req) = zhongshu_core::authority::peek_pending() {
            if !self.pending_auth_notified {
                self.pending_auth_notified = true;
                let title = if req.source.is_empty() { "需要授权".into() } else { format!("需要授权 · {}", req.source) };
                let _ = zhongshu_core::desktop::notification::show_urgent(&title, &format!("{} - {}", req.tool, req.command));
            }
            let request = AuthRequest {
                tool: req.tool.clone(),
                source: req.source.clone(),
                command: req.command.clone(),
            };
            if let Some(ref ov) = self.overlay {
                ov.show_auth(&request);
            }
        } else {
            self.pending_auth_notified = false;
        }
    }

    fn reduce_events(&mut self) -> bool {
        let mut active = false;
        loop {
            match self.event_rx.try_recv() {
                Ok(ev) => {
                    active = true;
                    match ev {
                        Event::Agent(AgentEvent::StateChanged { from: _, to }) => {
                            if self.config.agent.desktop_notification {
                                if let AgentState::Done { success } = to {
                                    if success {
                                        let _ = zhongshu_core::desktop::notification::show("完成", "任务完成");
                                    } else {
                                        let _ = zhongshu_core::desktop::notification::show("失败", "任务出错");
                                    }
                                }
                            }
                            self.indicator_state = to;
                            if let Some(ind) = self.indicator.as_mut() { ind.set_state(to); }
                        }
                        Event::Tool(ToolEvent::Started { name }) => {
                            self.indicator_state = AgentState::Executing;
                            if let Some(ind) = self.indicator.as_mut() { ind.set_state(AgentState::Executing); }
                            if let Some(ref ov) = self.overlay {
                                ov.toast(&format!("工具调用: {name}"));
                            }
                        }
                        Event::Tool(ToolEvent::Completed { name, success }) => {
                            if let Some(ref ov) = self.overlay {
                                ov.toast(&format!("工具完成: {name} {}", if success {"✓"} else {"✗"}));
                            }
                        }
                        Event::Task(TaskEvent::Triggered { name }) => {
                            if self.config.agent.desktop_notification {
                                let _ = zhongshu_core::desktop::notification::show("任务触发", &name);
                            }
                        }
                        Event::Task(TaskEvent::Completed { name }) => {
                            if self.config.agent.desktop_notification {
                                let _ = zhongshu_core::desktop::notification::show("任务完成", &name);
                            }
                        }
                        _ => {}
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("event bus lagged: {n} events dropped"); active = true;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed)
                | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            }
        }
        active
    }

    fn reduce_responses(&mut self) -> bool {
        let mut active = false;
        let mut assistant_id: Option<MessageId> = None;
        let mut filter = zhongshu_message_core::streaming::ControlTokenFilter::new();
        while let Ok(ev) = self.response_rx.try_recv() {
            active = true;
            match ev {
                ResponseEvent::MessageStarted { id, role } => {
                    if matches!(role, ResponseRole::Assistant) {
                        assistant_id = Some(id);
                        if let Some(ref ov) = self.overlay {
                            ov.set_state("thinking");
                        }
                    } else {
                        assistant_id = None;
                    }
                }
                ResponseEvent::MessageDelta { id, delta } => {
                    if assistant_id.map(|aid| aid == id).unwrap_or(false) {
                        let cleaned = filter.feed(&delta);
                        if !cleaned.is_empty() {
                            if let Some(ref ov) = self.overlay {
                                ov.push_delta(&cleaned);
                            }
                        }
                    }
                }
                ResponseEvent::MessageCompleted { id } => {
                    if assistant_id.map(|aid| aid == id).unwrap_or(false) {
                        if let Some(ref ov) = self.overlay {
                            ov.complete_message();
                        }
                        filter.flush();
                        assistant_id = None;
                    }
                }
            }
        }
        active
    }

    // ── Overlay management ──────────────────────────────────────────

    fn try_open_overlay(&mut self, _el: &ActiveEventLoop) {
        if let Some(ref ov) = self.overlay {
            ov.show_window(self.config.ui.overlay_width, self.config.ui.overlay_height);
            return;
        }
        let ov = overlay::show(
            self.config.ui.overlay_width, self.config.ui.overlay_height,
        );
        // Load previous conversation from lcm.db and send to JS
        let proxy = self.proxy.clone();
        let history = self.runtime.block_on(async { proxy.lock().await.load_chat_history().await });
        let cleaned_history: Vec<(String, String)> = history.into_iter()
            .map(|(role, content)| (role, zhongshu_message_core::strip_control_tokens(&content)))
            .collect();

        // Show only the last 20 pairs (40 entries) initially; cache older entries for lazy loading.
        const INITIAL_PAIRS: usize = 20;
        let max_initial = INITIAL_PAIRS * 2;
        let has_more = cleaned_history.len() > max_initial;
        let (initial, cache) = if has_more {
            let split = cleaned_history.len() - max_initial;
            let cache = cleaned_history[..split].to_vec();
            let initial = cleaned_history[split..].to_vec();
            (initial, cache)
        } else {
            (cleaned_history.clone(), Vec::new())
        };
        self.history_cache = cache;

        // Full history (all entries) goes to controller for LLM context.
        self.controller.set_chat_history(cleaned_history);

        let entries: Vec<overlay::ChatEntry> = initial.iter().map(|(role, content)| {
            overlay::ChatEntry {
                role: if role == "User" { overlay::EntryRole::User } else { overlay::EntryRole::Assistant },
                content: content.clone(),
                tool_calls: Vec::new(),
            }
        }).collect();
        if !entries.is_empty() {
            ov.set_history(&entries, has_more);
        }
        self.overlay = Some(ov);
    }

    fn new_conversation(&mut self, _el: &ActiveEventLoop) {
        self.controller.set_chat_history(Vec::new());
        self.history_cache = Vec::new();
        if let Some(ref ov) = self.overlay {
            ov.clear_chat();
        }
    }
}

impl ApplicationHandler for ZhongshuApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        self.indicator = Some(Indicator::create(el, self.config.ui.orb_size));
    }
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.drain();
        let orb_id = self.indicator.as_ref().and_then(|i| i.window_id());
        #[allow(unused)]
        let on_orb = orb_id == Some(id);
        match event {
            WindowEvent::CloseRequested => {
                if orb_id == Some(id) { el.exit(); } else {
                    // GTK overlay handles its own close
                }
            }
            WindowEvent::RedrawRequested => {
                if orb_id == Some(id) {
                    self.indicator.as_mut().unwrap().render();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                if on_orb && button == MouseButton::Left {
                    if let Some(w) = self.indicator.as_ref().unwrap().window() {
                        if let Ok(p) = w.outer_position() {
                            self.drag_start_win = (p.x, p.y);
                            self.drag_start_cursor = self.cursor_pos;
                            self.is_dragging = true;
                        }
                    }
                }
                if on_orb && button == MouseButton::Right {
                    match show_context_menu(self.indicator.as_ref().unwrap().window()) {
                        MenuAction::NewConversation => self.new_conversation(el),
                        MenuAction::Quit => el.exit(),
                        MenuAction::None => {}
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                if on_orb { self.is_dragging = false; if let Some(ind) = self.indicator.as_ref() { ind.save_position(); } }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if on_orb && self.is_dragging {
                    let dx = position.x - self.cursor_pos.0;
                    let dy = position.y - self.cursor_pos.1;
                    if let Some(w) = self.indicator.as_ref().unwrap().window() {
                        if let Ok(p) = w.outer_position() {
                            let _ = w.set_outer_position(winit::dpi::PhysicalPosition::new(
                                p.x + dx as i32,
                                p.y + dy as i32,
                            ));
                        }
                    }
                }
                self.cursor_pos = (position.x, position.y);
            }
            WindowEvent::ModifiersChanged(m) => {
                self.ctrl_held = m.state().control_key();
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if self.ctrl_held && event.state == ElementState::Pressed && event.logical_key == "q" => {
                el.exit();
            }
            WindowEvent::Focused(true) => {
                // GTK overlay handles its own focus/IME
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain();

        if self.hotkey.try_recv().is_some() { self.try_open_overlay(el); }

        #[cfg(target_os = "linux")]
        {
            let mut tray_events = Vec::new();
            if let Some(Indicator::Tray(t)) = self.indicator.as_mut() {
                while let Ok(ev) = t.rx.try_recv() { tray_events.push(ev); }
            }
            for ev in tray_events {
                match ev {
                    TrayEvent::OpenOverlay => self.try_open_overlay(el),
                    TrayEvent::NewConversation => self.new_conversation(el),
                    TrayEvent::Quit => {
                        tracing::info!("tray quit");
                        el.exit();
                    }
                }
            }
        }

        // Notification click → focus overlay
        if zhongshu_core::desktop::notification::consume_focus_request() {
            self.try_open_overlay(el);
        }

        // Streaming timeout — warn only, don't kill the message
        let elapsed = self.last_activity.elapsed().as_secs();
        let timeout = self.config.agent.streaming_timeout_secs;
        if !matches!(self.indicator_state, AgentState::Idle) && elapsed > timeout {
            tracing::warn!("streaming timeout after {elapsed}s (limit {timeout}s), agent still running");
            self.last_activity = Instant::now();
        }

        let idle = matches!(self.indicator_state, AgentState::Idle);
        let tray_active = cfg!(target_os = "linux");
        el.set_control_flow(
            if self.overlay.is_some() || !idle {
                ControlFlow::Poll
            } else if tray_active {
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200))
            } else {
                ControlFlow::Wait
            }
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum MenuAction { NewConversation, Quit, None }

/// Show a right-click context menu on the orb.
#[cfg(target_os = "windows")]
fn show_context_menu(orb_window: Option<&std::sync::Arc<winit::window::Window>>) -> MenuAction {
    use winit::raw_window_handle::HasWindowHandle;
    let w = match orb_window { Some(w) => w, None => return MenuAction::None };
    let handle = match w.window_handle() { Ok(h) => h, Err(_) => return MenuAction::None };
    let hwnd = match handle.as_ref() {
        winit::raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get(),
        _ => return MenuAction::None,
    };

    const MF_STRING: u32 = 0;
    const TPM_RETURNCMD: u32 = 0x0100;

    #[repr(C)]
    struct POINT { x: i32, y: i32 }

    extern "system" {
        fn CreatePopupMenu() -> *mut std::ffi::c_void;
        fn AppendMenuW(hmenu: *mut std::ffi::c_void, flags: u32, id: usize, text: *const u16) -> i32;
        fn TrackPopupMenu(hmenu: *mut std::ffi::c_void, flags: u32, x: i32, y: i32, reserved: i32, hwnd: isize, rect: *const std::ffi::c_void) -> u32;
        fn DestroyMenu(hmenu: *mut std::ffi::c_void) -> i32;
        fn GetCursorPos(pt: *mut POINT) -> i32;
    }

    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() { return MenuAction::None; }

        let new_conv: Vec<u16> = "新建对话\0".encode_utf16().collect();
        let quit: Vec<u16> = "退出\0".encode_utf16().collect();

        AppendMenuW(hmenu, MF_STRING, 1, new_conv.as_ptr());
        AppendMenuW(hmenu, MF_STRING, 2, quit.as_ptr());

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let cmd = TrackPopupMenu(hmenu, TPM_RETURNCMD, pt.x, pt.y, 0, hwnd as isize, std::ptr::null());

        DestroyMenu(hmenu);

        match cmd {
            1 => MenuAction::NewConversation,
            2 => MenuAction::Quit,
            _ => MenuAction::None,
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn show_context_menu(_orb_window: Option<&std::sync::Arc<winit::window::Window>>) -> MenuAction {
    MenuAction::None
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("ZHONGSHU_LOG")
                            .unwrap_or_else(|_| "info,wgpu_hal=off,wgpu_core=error,naga=error,sctk_adwaita=error".into()))
        .init();

    preflight_checks();

    let cfg = config::load();
    let ak = cfg.llm.api_key();
    if ak.is_empty() { tracing::warn!("{} not set; agent will not function", cfg.llm.api_key_env); }

    // Shared tokio runtime for all async work (proxy, agent, background).
    let r = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let _g = r.enter();

    // Start deeplossless proxy.
    let proxy_port = cfg.deeplossless.proxy_port;
    let mut proxy = r.block_on(async {
        zhongshu_core::integration::DeeplosslessProxy::new(
            zhongshu_core::integration::DeeplosslessConfig {
                api_key: ak.clone(),
                upstream: cfg.llm.api_base.clone(),
                proxy_port,
                ..Default::default()
            }
        ).await
    }).expect("deeplossless proxy failed to build");

    let actual_port = r.block_on(async { proxy.start(proxy_port).await }).expect("deeplossless proxy failed to start");
    let base_url = format!("http://127.0.0.1:{actual_port}/v1");
    tracing::info!("deeplossless proxy at {base_url}");

    let eb = Arc::new(EventBus::new(256));
    let (response_tx, response_rx) = mpsc::channel::<ResponseEvent>(cfg.agent.response_capacity);
    let event_rx = eb.subscribe();

    authority::init(AuthorityGate::new(cfg.agent.authority.enabled, cfg.agent.authority.sudo_timeout_secs));

    // AttentionDispatcher: shows desktop notifications for attention events.
    let desktop_notif = cfg.agent.desktop_notification;
    let dispatcher = AttentionDispatcher::new(
        Box::new(move |worker, summary| {
            if desktop_notif {
                let _ = zhongshu_core::desktop::notification::show(worker, summary);
            }
        }),
    );
    let _dispatcher_handle = dispatcher.spawn(&eb);

    // ── 军器监初始化 ──
    let equipment = {
        let dir = config::config_dir().join("equipment");
        std::fs::create_dir_all(&dir).unwrap_or(());
        let mut reg = zhongshu_core::equipment::EquipmentRegistry::new(dir);
        reg.install_defaults(); // 内部已调用 scan()
        reg
    };
    let equip_prompts = equipment.skill_prompts();
    let mut system_prompt = cfg.agent.effective_system_prompt();
    for (_id, prompt) in &equip_prompts {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(prompt);
    }

    let provider = OpenAiProvider::new(&ak, &cfg.llm.model).with_base_url(base_url);
    let controller = Arc::new(AgentController::new(
        eb.clone(), response_tx.clone(),
        provider.clone(),
        default_registry().register(zhongshu_core::tool::search::WebSearchTool)
            .register(zhongshu_core::tool::browser::BrowserTool)
            .register(zhongshu_core::tool::webfetch::WebFetchTool)
            .register(zhongshu_core::tool::screenshot::ScreenshotTool)
            .register(zhongshu_core::tool::automation::AutomationTool),
        cfg.llm.model.clone(), SessionState::new(), system_prompt,
        config::config_dir().join("agent.json"),
    ));
    let inbox = Arc::new(AgentInbox::new(controller.clone()));

    // AttentionDispatcher: shows desktop notifications for attention events.
    inbox.start();

    let mut task_scheduler = TaskScheduler::new(Duration::from_secs(1));

    // Register reminders from config.
    for r in &cfg.scheduler.reminders {
        if let Some(trigger) = ReminderTrigger::from_rfc3339(&r.id, &r.message, &r.at) {
            task_scheduler.register(trigger);
            tracing::info!("registered reminder '{}' at {}", r.id, r.at);
        } else {
            tracing::warn!("failed to parse reminder '{}' at {}", r.id, r.at);
        }
    }

    // Register file watches from config.
    for w in &cfg.scheduler.file_watches {
        let watch = FileWatchTrigger::new(&w.id, std::path::PathBuf::from(&w.path));
        task_scheduler.register(watch);
        tracing::info!("registered file watch '{}' on {}", w.id, w.path);
    }

    let task_queue = task_scheduler.queue().clone();
    let rule_queue = task_scheduler.queue().clone();
    task_scheduler.spawn();

    // Worker runtime: shares provider + registry with the primary agent.
    let worker_runtime = Arc::new(AgentRuntime::new(
        provider.clone(),
        default_registry().register(zhongshu_core::tool::search::WebSearchTool)
            .register(zhongshu_core::tool::browser::BrowserTool)
            .register(zhongshu_core::tool::webfetch::WebFetchTool)
            .register(zhongshu_core::tool::screenshot::ScreenshotTool)
            .register(zhongshu_core::tool::automation::AutomationTool),
        cfg.llm.model.clone(),
        AgentBudget {
            max_steps: 10,
            max_tool_calls: 5,
            token_limit: 32_000,
        },
    ));

    // Worker profiles: load from ~/.config/zhongshu/profiles/*.json, fallback to default.
    let profile_dir = config::config_dir().join("profiles");
    let _ = std::fs::create_dir_all(&profile_dir);
    let mut worker_profiles = AgentProfile::load_dir(&profile_dir);
    if worker_profiles.is_empty() {
        tracing::info!("no worker profiles in {:?}, using default task-handler", profile_dir);
        worker_profiles.push(AgentProfile::new(
            "task-handler",
            "你是一个后台任务处理助手。收到定时任务或事件后，分析任务内容并执行必要的操作。",
            vec![],
            AgentBudget::default(),
        ));
    } else {
        tracing::info!(count = worker_profiles.len(), "loaded worker profiles");
    }

    // AttentionManager: listens for WorkerReport events, routes by level.
    let attention_mgr = AttentionManager::new((*eb).clone());
    let (digest_queue, _attention_handle) = attention_mgr.spawn();

    // SourceManager: polls event sources and publishes to EventBus.
    let mut source_mgr = SourceManager::new((*eb).clone());
    source_mgr.register(TimerSource::new("heartbeat", Duration::from_secs(300)));
    #[cfg(target_os = "windows")]
    source_mgr.register(DiskUsageSource::new("disk-root", "C:\\", 0.90, Duration::from_secs(3600)));
    #[cfg(not(target_os = "windows"))]
    source_mgr.register(DiskUsageSource::new("disk-root", "/", 0.90, Duration::from_secs(3600)));
    source_mgr.register(BatterySource::new("battery", 20, Duration::from_secs(3600)));
    let _source_handle = source_mgr.spawn();

    // Spawn one TaskWorkerDispatcher per profile.
    for profile in worker_profiles {
        TaskWorkerDispatcher::spawn(task_queue.clone(), worker_runtime.clone(), profile, eb.clone());
    }

    // RuleEngine: subscribes to EventBus, matches rules → submits Tasks.
    let mut rule_engine = RuleEngine::new((*eb).clone(), rule_queue);
    if cfg.agent.background.enabled {
        rule_engine.add_rule(Rule {
            id: "heartbeat-check".into(),
            event_pattern: "tick".into(),
            source: None,
            condition: RuleCondition::Always,
            task: RuleTask {
                source: "heartbeat".into(),
                tool: "agent".into(),
                arguments: serde_json::json!({"prompt": "[定时检查] 使用 system_info 工具收集系统信息并检查异常，不要使用 shell。"}),
            },
        });
        tracing::info!("background rule check enabled");
    }
    let _rule_handle = rule_engine.spawn();

    // Heartbeat: periodic runtime maintenance (no LLM).
    if cfg.agent.background.enabled {
        let _heartbeat_handle = Heartbeat::default().spawn();
    }

    // Daily Digest: scheduled task that aggregates digest-queue Reports.
    {
        let digest_eb = (*eb).clone();
        let dq = digest_queue.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(86400));
            // 首次 tick 立即完成，使第一次运行在完整 interval 后
            tick.tick().await;
            loop {
                tick.tick().await;
                let reports = AttentionManager::drain_queue(&dq);
                if !reports.is_empty() {
                    let builder = DigestBuilder::new(digest_eb.clone());
                    builder.build_and_send(reports);
                }
            }
        });
    }

    // Event log: replay past events into current subscribers, then log future events.
    let event_log_path = config::config_dir().join("event_log.jsonl");
    EventLogger::replay(&event_log_path, &eb);
    let _event_logger = EventLogger::new(event_log_path).unwrap().spawn(&eb);

    let mut app = match ZhongshuApp::new(cfg, controller, inbox.clone(), eb, event_rx, response_tx, response_rx, proxy, r) {
        Ok(app) => app, Err(e) => { tracing::error!("init: {e:#}"); return; }
    };

    EventLoop::new().unwrap().run_app(&mut app).unwrap();
}
