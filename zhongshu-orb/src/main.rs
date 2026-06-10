mod app;
mod render;
mod indicator;
mod overlay;
mod hotkey;
mod config;
mod gpu;
mod agent;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use zhongshu_core::event::{
    AgentState,
    Event, AgentEvent, ToolEvent,
    ResponseEvent, ResponseRole, MessageId,
    EventBus, EventRx, ResponseTx, ResponseRx,
};
use indicator::Indicator;
use app::{SessionState, AgentController, BackgroundRunner, AgentInbox, AgentTaskDispatcher};
use overlay::{ToolCallEntry, ToolStatus};
use overlay::EntryRole;
use overlay::Overlay;
use overlay::StreamingState;
use hotkey::HotkeyManager;
use config::AppConfig;
use gpu::GpuContext;
#[cfg(target_os = "linux")]
use indicator::tray::TrayEvent;
use zhongshu_core::tool::default_registry;
use zhongshu_core::agent::llm::OpenAiProvider;
use zhongshu_core::task::{TaskScheduler, IntervalTrigger};
use zhongshu_core::authority::{self, AuthorityGate};
use std::time::Duration;
use tokio::sync::mpsc;

fn preflight_checks() {
    // Event bus: subscribe first, then publish & receive must work.
    let bus = Arc::new(EventBus::new(4));
    let mut rx = bus.subscribe();
    bus.publish(Event::Agent(AgentEvent::StateChanged {
        from: AgentState::Idle, to: AgentState::Thinking,
    }));
    assert!(rx.try_recv().is_ok(), "preflight: event bus failed");

    // Response channel: send & receive must work.
    let (tx, mut response_rx) = mpsc::channel::<ResponseEvent>(4);
    let id = MessageId::new();
    assert!(tx.try_send(ResponseEvent::MessageStarted { id, role: ResponseRole::System }).is_ok(), "preflight: response tx failed");
    assert!(response_rx.try_recv().is_ok(), "preflight: response rx failed");

    // egui context: must be constructable and renderable.
    let ctx = egui::Context::default();
    let _out = ctx.run(Default::default(), |_cx| {});

    tracing::info!("preflight checks passed");
}

struct ZhongshuApp {
    config: AppConfig,
    controller: Arc<AgentController>,
    inbox: Arc<AgentInbox>,
    indicator: Option<Indicator>,
    indicator_state: AgentState,
    overlays: HashMap<WindowId, Overlay>,
    event_bus: Arc<EventBus>,
    event_rx: EventRx,
    response_tx: ResponseTx,
    response_rx: ResponseRx,
    hotkey: HotkeyManager,
    gpu: Arc<GpuContext>,
    last_activity: Instant,
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
        gpu: Arc<GpuContext>,
    ) -> anyhow::Result<Self> {
        let hotkey = HotkeyManager::new(&config.hotkey).unwrap_or_else(|e| {
            tracing::warn!("Global hotkey unavailable: {e:#}"); HotkeyManager::passive()
        });
        Ok(ZhongshuApp {
            config, controller, inbox, indicator: None, indicator_state: AgentState::Idle,
            overlays: HashMap::new(), event_bus, event_rx, response_tx, response_rx,
            hotkey, gpu, last_activity: Instant::now(),
        })
    }

    // ── Event reducers ──────────────────────────────────────────────

    fn drain(&mut self) {
        let activity = self.reduce_events() | self.reduce_responses();
        if activity { self.last_activity = Instant::now(); }
    }

    fn reduce_events(&mut self) -> bool {
        let mut active = false;
        loop {
            match self.event_rx.try_recv() {
                Ok(ev) => {
                    active = true;
                    match ev {
                        Event::Agent(AgentEvent::StateChanged { from: _, to }) => {
                            if matches!(to, AgentState::Done { .. }) || matches!(to, AgentState::Idle) {
                                for ov in self.overlays.values_mut() {
                                    ov.flush_streaming(self.config.ui.max_chat_entries);
                                }
                            }
                            self.indicator_state = to;
                            if let Some(ind) = self.indicator.as_mut() { ind.set_state(to); }
                        }
                        Event::Tool(ToolEvent::Started { name }) => {
                            self.indicator_state = AgentState::Executing;
                            if let Some(ind) = self.indicator.as_mut() { ind.set_state(AgentState::Executing); }
                            for ov in self.overlays.values_mut() {
                                if let Some(ref mut s) = ov.streaming {
                                    s.tool_calls.push(ToolCallEntry::new(name.clone()));
                                } else {
                                    ov.streaming = Some(StreamingState::new(EntryRole::Assistant));
                                    ov.streaming.as_mut().unwrap().tool_calls.push(ToolCallEntry::new(name.clone()));
                                }
                                ov.window.request_redraw();
                            }
                        }
                        Event::Tool(ToolEvent::Completed { name, success }) => {
                            for ov in self.overlays.values_mut() {
                                if let Some(ref mut s) = ov.streaming {
                                    if let Some(last) = s.tool_calls.iter_mut().rev().find(|t| matches!(t.status, ToolStatus::Running)) {
                                        let elapsed = last.started_at.elapsed().as_millis() as u64;
                                        last.status = ToolStatus::Done { success, duration_ms: elapsed };
                                    } else {
                                        let mut tc = ToolCallEntry::new(name.clone());
                                        tc.status = ToolStatus::Done { success, duration_ms: 0 };
                                        s.tool_calls.push(tc);
                                    }
                                }
                                ov.window.request_redraw();
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
        while let Ok(ev) = self.response_rx.try_recv() {
            active = true;
            match ev {
                ResponseEvent::MessageStarted { role, .. } => {
                    let erole = match role {
                        ResponseRole::User => EntryRole::User,
                        ResponseRole::Assistant => EntryRole::Assistant,
                        ResponseRole::System => EntryRole::System,
                    };
                    for ov in self.overlays.values_mut() {
                        ov.flush_streaming(self.config.ui.max_chat_entries);
                        ov.streaming = Some(StreamingState::new(erole));
                        ov.window.request_redraw();
                    }
                }
                ResponseEvent::MessageDelta { delta, .. } => {
                    for ov in self.overlays.values_mut() {
                        if let Some(ref mut s) = ov.streaming {
                            s.content.push_str(&delta);
                        } else {
                            let mut s = StreamingState::new(EntryRole::Assistant);
                            s.content.push_str(&delta);
                            ov.streaming = Some(s);
                        }
                        ov.window.request_redraw();
                    }
                }
                ResponseEvent::MessageCompleted { .. } => {
                    for ov in self.overlays.values_mut() {
                        ov.flush_streaming(self.config.ui.max_chat_entries);
                        ov.window.request_redraw();
                    }
                }
            }
        }
        active
    }

    // ── Overlay management ──────────────────────────────────────────

    fn try_open_overlay(&mut self, el: &ActiveEventLoop) {
        if self.overlays.is_empty() {
            self.controller.init_engine(&self.config.llm.api_key());
            let ov = Overlay::new(
                el, self.gpu.clone(),
                self.config.ui.overlay_width, self.config.ui.overlay_height,
                &self.config.ui.font_search_paths,
            );
            let id = ov.window.id();
            self.overlays.insert(id, ov);
        }
    }

    fn new_conversation(&mut self, el: &ActiveEventLoop) {
        let ids: Vec<WindowId> = self.overlays.keys().copied().collect();
        for id in ids { self.overlays.remove(&id); }
        self.try_open_overlay(el);
    }
}

impl ApplicationHandler for ZhongshuApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        self.indicator = Some(Indicator::create(el, self.config.ui.orb_size));
    }
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.drain();
        let is_ol = self.overlays.contains_key(&id);
        if is_ol {
            if let Some(ov) = self.overlays.get_mut(&id) {
                let _ = ov.state.on_window_event(&ov.window, &event);
                ov.window.request_redraw();
            }
        }
        let orb_id = self.indicator.as_ref().and_then(|i| i.window_id());
        match event {
            WindowEvent::CloseRequested => {
                if orb_id == Some(id) { el.exit(); } else { self.overlays.remove(&id); }
            }
            WindowEvent::RedrawRequested => {
                if orb_id == Some(id) {
                    self.indicator.as_mut().unwrap().render();
                } else if let Some(ov) = self.overlays.get_mut(&id) {
                    if let Some(input) = ov.render() {
                        self.inbox.submit(input);
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                if orb_id == Some(id) { self.try_open_overlay(el); }
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

        // Streaming timeout
        let elapsed = self.last_activity.elapsed().as_secs();
        let timeout = self.config.agent.streaming_timeout_secs;
        if !matches!(self.indicator_state, AgentState::Idle) && elapsed > timeout {
            tracing::warn!("streaming timeout after {elapsed}s (limit {timeout}s)");
            self.event_bus.publish(Event::Agent(AgentEvent::StateChanged {
                from: self.indicator_state, to: AgentState::Done { success: false },
            }));
            let mid = MessageId::new();
            let _ = self.response_tx.try_send(ResponseEvent::MessageStarted { id: mid, role: ResponseRole::System });
            let _ = self.response_tx.try_send(ResponseEvent::MessageDelta { id: mid, delta: format!("[连接超时: {elapsed}s 无响应]") });
            let _ = self.response_tx.try_send(ResponseEvent::MessageCompleted { id: mid });
            self.last_activity = Instant::now(); self.drain();
        }

        let idle = matches!(self.indicator_state, AgentState::Idle);
        let tray_active = cfg!(target_os = "linux");
        el.set_control_flow(
            if !self.overlays.is_empty() || !idle {
                ControlFlow::Poll
            } else if tray_active {
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200))
            } else {
                ControlFlow::Wait
            }
        );
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("ZHONGSHU_LOG")
                            .unwrap_or_else(|_| "info,wgpu_hal=error,wgpu_core=error,naga=error,sctk_adwaita=error".into()))
        .init();

    preflight_checks();

    let cfg = config::load();
    let ak = cfg.llm.api_key();
    if ak.is_empty() { tracing::warn!("{} not set; agent will not function", cfg.llm.api_key_env); }

    let eb = Arc::new(EventBus::new(256));
    let (response_tx, response_rx) = mpsc::channel::<ResponseEvent>(cfg.agent.response_capacity);
    let event_rx = eb.subscribe();

    authority::init(AuthorityGate::new(cfg.agent.authority.enabled, cfg.agent.authority.sudo_timeout_secs));

    let controller = Arc::new(AgentController::new(
        eb.clone(), response_tx.clone(),
        OpenAiProvider::new(&ak, &cfg.llm.model),
        default_registry().register(zhongshu_core::tool::search::WebSearchTool)
            .register(zhongshu_core::tool::browser::BrowserTool)
            .register(zhongshu_core::tool::screenshot::ScreenshotTool)
            .register(zhongshu_core::tool::automation::AutomationTool),
        cfg.llm.model.clone(), SessionState::new(), cfg.agent.system_prompt.clone(),
        config::config_dir().join("agent.json"),
    ));
    let inbox = Arc::new(AgentInbox::new(controller.clone()));

    let r = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let _g = r.enter();
    inbox.start();

    let mut task_scheduler = TaskScheduler::new(Duration::from_secs(1));
    task_scheduler.register(IntervalTrigger::new("hourly-check", "agent", serde_json::json!({"prompt":"[定时检查]"}),
        Duration::from_secs(3600)));
    let task_queue = task_scheduler.queue().clone();
    task_scheduler.spawn();
    AgentTaskDispatcher::spawn(task_queue, inbox.clone());

    let gpu = match GpuContext::new() { Ok(g) => Arc::new(g), Err(e) => { tracing::error!("GPU: {e:#}"); return; } };
    let mut app = match ZhongshuApp::new(cfg, controller, inbox.clone(), eb, event_rx, response_tx, response_rx, gpu) {
        Ok(app) => app, Err(e) => { tracing::error!("init: {e:#}"); return; }
    };

    if app.config.agent.background.enabled {
        BackgroundRunner::new(app.config.agent.background.interval_secs, app.config.agent.background.prompt.clone(), app.controller.state())
            .spawn(app.inbox.clone());
    }

    EventLoop::new().unwrap().run_app(&mut app).unwrap();
}
