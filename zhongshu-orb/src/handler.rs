use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use zhongshu_core::agent::llm::OpenAiProvider;
use zhongshu_core::agent::{AgentRuntime, ModelRouter};
use zhongshu_core::authority;
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, EventRx, HarnessUiEvent, MessageId, ResponseEvent,
    ResponseRole, ResponseRx, ResponseTx, TaskEvent, ToolEvent,
};
use zhongshu_core::integration::DeeplosslessProxy;
use zhongshu_message_core::streaming::ControlTokenFilter;

use crate::app::{AgentController, AgentInbox};
use crate::config::AppConfig;
use crate::hotkey::HotkeyManager;
use crate::indicator::Indicator;
use crate::overlay::{AuthRequest, OverlayHandle};
use crate::overlay_contract::{CodingUiEvent, OverlayToUiEvent};
use zhongshu_core::equipment::{EquipmentObserver, EquipmentRegistry};
use zhongshu_core::tool::ToolRegistry;

fn send_coding_event(overlay: &OverlayHandle, event: CodingUiEvent) {
    overlay.send(&serde_json::to_value(OverlayToUiEvent::Coding { event }).unwrap_or_default());
}

// ── App state ────────────────────────────────────────────────────────

pub struct ZhongshuApp {
    pub config: AppConfig,
    pub controller: Arc<AgentController>,
    pub proxy: Arc<tokio::sync::Mutex<DeeplosslessProxy>>,
    pub runtime: tokio::runtime::Runtime,
    pub inbox: Arc<AgentInbox>,
    pub indicator: Option<Indicator>,
    pub indicator_state: AgentState,
    pub overlay: Option<OverlayHandle>,
    #[allow(dead_code)]
    pub event_bus: Arc<EventBus>,
    pub event_rx: EventRx,
    #[allow(dead_code)]
    pub response_tx: ResponseTx,
    pub response_rx: ResponseRx,
    pub hotkey: HotkeyManager,
    pub last_activity: Instant,
    pub is_dragging: bool,
    pub cursor_pos: (f64, f64),
    pub drag_start_cursor: (f64, f64),
    pub drag_start_win: (i32, i32),
    pub ctrl_held: bool,
    pub pending_auth_notified: bool,
    pub history_cache: Vec<(String, String)>,
    pub assistant_id: Option<MessageId>,
    pub filter: ControlTokenFilter,
    pub task_repo: zhongshu_core::core::TaskRepository,
    pub runbook_store: zhongshu_core::core::RunbookStore,
    pub observer: Arc<Mutex<EquipmentObserver>>,
    pub equipment: Arc<Mutex<EquipmentRegistry>>,
    pub worker_runtime: Arc<tokio::sync::RwLock<AgentRuntime>>,
    pub worker_base_tools: ToolRegistry,
    pub overlay_zoomed: bool,
    pub auth_watch: watch::Receiver<Option<zhongshu_core::authority::PendingRequest>>,
}

impl ZhongshuApp {
    pub fn new(
        config: AppConfig,
        controller: Arc<AgentController>,
        inbox: Arc<AgentInbox>,
        event_bus: Arc<EventBus>,
        event_rx: EventRx,
        response_tx: ResponseTx,
        response_rx: ResponseRx,
        proxy: Arc<tokio::sync::Mutex<DeeplosslessProxy>>,
        runtime: tokio::runtime::Runtime,
        task_repo: zhongshu_core::core::TaskRepository,
        runbook_store: zhongshu_core::core::RunbookStore,
        observer: Arc<Mutex<EquipmentObserver>>,
        equipment: Arc<Mutex<EquipmentRegistry>>,
        worker_runtime: Arc<tokio::sync::RwLock<AgentRuntime>>,
        worker_base_tools: ToolRegistry,
    ) -> anyhow::Result<Self> {
        let hotkey = HotkeyManager::new(&config.hotkey).unwrap_or_else(|e| {
            tracing::warn!("Global hotkey unavailable: {e:#}");
            HotkeyManager::passive()
        });
        let auth_watch = zhongshu_core::authority::subscribe_auth();
        Ok(ZhongshuApp {
            config,
            controller,
            inbox,
            indicator: None,
            indicator_state: AgentState::Idle,
            proxy,
            runtime,
            overlay: None,
            event_bus,
            event_rx,
            response_tx,
            response_rx,
            hotkey,
            last_activity: Instant::now(),
            is_dragging: false,
            cursor_pos: (0.0, 0.0),
            drag_start_cursor: (0.0, 0.0),
            drag_start_win: (0, 0),
            ctrl_held: false,
            pending_auth_notified: false,
            history_cache: Vec::new(),
            assistant_id: None,
            filter: ControlTokenFilter::new(),
            task_repo,
            runbook_store,
            observer,
            equipment,
            worker_runtime,
            worker_base_tools,
            overlay_zoomed: false,
            auth_watch,
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
            self.observer.lock().unwrap().record_user_message(&text);
            self.inbox.submit(text);
        }
        if let Some(rid) = ov.take_approve() {
            authority::approve_pending(&rid);
        }
        if let Some(rid) = ov.take_deny() {
            authority::deny_pending(&rid);
        }
        if let Some(p) = ov.take_personality() {
            let mut cfg = crate::config::load();
            cfg.agent.personality = p.clone();
            cfg.agent.personality_selected = true;
            crate::config::save(&cfg);
            self.controller
                .set_system_prompt(cfg.agent.effective_system_prompt());
        }
        if ov.take_list_equipment() {
            let equipment = self.equipment.lock().unwrap();
            let items: Vec<serde_json::Value> = equipment.list().iter().map(|eq| serde_json::json!({
                "id": eq.id, "name": eq.manifest.name, "version": eq.manifest.version,
                "enabled": matches!(eq.status, zhongshu_core::equipment::EquipmentStatus::Active),
            })).collect();
            ov.show_equipment(&items);
        }
        if ov.take_list_runbooks() {
            let runbooks = match self
                .runbook_store
                .migrate()
                .and_then(|_| self.runbook_store.list())
            {
                Ok(runbooks) => runbooks,
                Err(e) => {
                    tracing::warn!("list runbooks failed: {e}");
                    vec![]
                }
            };
            let items: Vec<serde_json::Value> = runbooks
                .iter()
                .map(|rb| {
                    serde_json::json!({
                        "id": rb.id,
                        "goal": rb.goal,
                        "conversation_id": rb.conversation_id,
                        "created_at": rb.created_at,
                        "total_steps": rb.total_steps,
                        "passed": rb.passed,
                        "failed": rb.failed,
                        "steps": rb.steps,
                    })
                })
                .collect();
            ov.show_runbooks(&items);
        }
        if let Some(eq_id) = ov.take_toggle_equipment() {
            let toggle_result = {
                let mut equipment = self.equipment.lock().unwrap();
                if let Some(eq) = equipment.get(&eq_id) {
                    let is_active =
                        matches!(eq.status, zhongshu_core::equipment::EquipmentStatus::Active);
                    let next = if is_active {
                        zhongshu_core::equipment::EquipmentStatus::Disabled
                    } else {
                        zhongshu_core::equipment::EquipmentStatus::Active
                    };
                    equipment.set_status(&eq_id, next)
                } else {
                    Err(format!("equipment '{eq_id}' not found"))
                }
            };
            match toggle_result {
                Ok(()) => {
                    self.controller.refresh_skill_prompts();
                    self.runtime
                        .block_on(self.controller.rebuild_equipment_tools_with_mcp());
                    let mut worker_tools = self.worker_base_tools.clone();
                    let mcp_reports = if let Ok(equipment) = self.equipment.lock() {
                        equipment.register_tools(&mut worker_tools);
                        self.runtime
                            .block_on(equipment.register_mcp_tools(&mut worker_tools))
                    } else {
                        Vec::new()
                    };
                    for report in mcp_reports {
                        if let Some(error) = report.error {
                            tracing::warn!(
                                "worker MCP server '{}' skipped: {}",
                                report.server_id,
                                error
                            );
                        }
                    }
                    self.runtime.block_on(async {
                        self.worker_runtime.write().await.registry = worker_tools;
                    });
                    ov.toast("Equipment 状态已更新");
                }
                Err(e) => {
                    tracing::warn!("toggle equipment '{eq_id}' failed: {e}");
                    ov.toast(&format!("Equipment 更新失败: {e}"));
                }
            }
        }
        if let Some(settings) = ov.take_settings() {
            let mut cfg = crate::config::load();
            let old_api_base = cfg.llm.api_base.clone();
            let old_proxy_port = cfg.deeplossless.proxy_port;
            if !settings.api_key.is_empty() {
                if let Err(e) = crate::config::store_api_key(&settings.api_key) {
                    tracing::warn!("save API key failed: {e:#}");
                    ov.toast("API Key 保存失败");
                } else {
                    ov.toast("API Key 已保存到系统凭据库");
                }
            }
            if !settings.api_base.is_empty() {
                cfg.llm.api_base = settings.api_base;
            }
            if !settings.model.is_empty() {
                cfg.llm.model = settings.model;
            }
            if let Some(port) = settings.proxy_port {
                if !port.is_empty() {
                    cfg.deeplossless.proxy_port = port.parse().unwrap_or(8081);
                }
            }
            if let Some(enabled) = settings.bg_enabled {
                cfg.agent.background.enabled = enabled;
            }
            if let Some(interval) = settings.bg_interval {
                if !interval.is_empty() {
                    cfg.agent.background.interval_secs = interval.parse().unwrap_or(600);
                }
            }
            if let Some(prompt) = settings.bg_prompt {
                cfg.agent.background.prompt = prompt;
            }
            if let Some(evolve) = settings.auto_evolve {
                cfg.agent.auto_evolve = evolve;
                self.controller.set_auto_evolve(evolve);
            }
            if let Some(ctx) = settings.max_context_tokens {
                if ctx >= 100_000 && ctx <= 1_000_000 {
                    cfg.llm.max_context_tokens = ctx;
                    self.controller.set_max_context_tokens(ctx);
                }
            }
            if let Some(ref mode) = settings.mode {
                cfg.agent.mode = mode.clone();
                self.controller.set_mode(mode.clone());
                // Resize window and notify frontend.
                if let Some(ref ov) = self.overlay {
                    let (w, h) = self.overlay_size();
                    ov.show_window(w, h);
                    ov.send(&serde_json::json!({"type":"mode_change","mode":mode}));
                }
            }
            if settings.personality != "默认" && !settings.personality.is_empty() {
                cfg.agent.personality = settings.personality;
                cfg.agent.personality_selected = true;
                self.controller
                    .set_system_prompt(cfg.agent.effective_system_prompt());
            }
            crate::config::save(&cfg);
            self.config = cfg.clone();
            let model_router = ModelRouter::new(
                &cfg.llm.model_routing.flash_model,
                &cfg.llm.model_routing.pro_model,
            );
            let base_url = self
                .runtime
                .block_on(async { self.proxy.lock().await.base_url().to_string() });
            let provider =
                OpenAiProvider::new(cfg.llm.api_key(), &cfg.llm.model).with_base_url(base_url);
            self.controller.update_llm_runtime(
                provider,
                cfg.llm.model.clone(),
                model_router,
                cfg.llm.model_routing.reasoning_complex.clone(),
                cfg.llm.model_routing.reasoning_agent.clone(),
            );
            if old_api_base != cfg.llm.api_base || old_proxy_port != cfg.deeplossless.proxy_port {
                ov.toast("API 地址或代理端口将在重启后生效");
            }
        }
        if ov.take_new_conversation() {
            self.delete_all_history();
        }
        if ov.take_stop() {
            self.controller.cancel();
        }
        if ov.take_toggle_zoom() {
            self.overlay_zoomed = !self.overlay_zoomed;
            let scale = if self.overlay_zoomed { 2.0 } else { 1.0 };
            ov.show_window(
                self.config.ui.overlay_width * scale,
                self.config.ui.overlay_height * scale,
            );
            ov.send(&serde_json::json!({"type":"zoom","active": self.overlay_zoomed}));
        }
        if ov.take_open_settings() {
            let cfg = crate::config::load();
            ov.show_settings(&crate::overlay::SettingsConfig {
                api_key: String::new(),
                api_key_saved: crate::config::has_stored_api_key()
                    || std::env::var(&cfg.llm.api_key_env)
                        .map(|v| !v.is_empty())
                        .unwrap_or(false),
                api_base: cfg.llm.api_base.clone(),
                model: cfg.llm.model.clone(),
                max_context_tokens: Some(cfg.llm.max_context_tokens),
                personality: cfg.agent.personality.clone(),
                proxy_port: Some(cfg.deeplossless.proxy_port.to_string()),
                bg_enabled: Some(cfg.agent.background.enabled),
                bg_interval: Some(cfg.agent.background.interval_secs.to_string()),
                bg_prompt: Some(cfg.agent.background.prompt.clone()),
                auto_evolve: Some(cfg.agent.auto_evolve),
                mode: Some(cfg.agent.mode.clone()),
            });
        }
        if ov.take_load_more() {
            const BATCH_SIZE: usize = 40;
            let cache_len = self.history_cache.len();
            if cache_len > 0 {
                let take = BATCH_SIZE.min(cache_len);
                let split = cache_len - take;
                let batch: Vec<(String, String)> = self.history_cache.drain(split..).collect();
                let has_more = self.history_cache.len() > 0;
                let entries: Vec<crate::overlay::ChatEntry> = batch
                    .iter()
                    .map(|(role, content)| crate::overlay::ChatEntry {
                        role: if role == "user" {
                            crate::overlay::EntryRole::User
                        } else {
                            crate::overlay::EntryRole::Assistant
                        },
                        content: content.clone(),
                        tool_calls: Vec::new(),
                    })
                    .collect();
                if !entries.is_empty() {
                    ov.prepend_history(&entries, has_more);
                }
            }
        }
        let mut refresh = ov.take_list_tasks();
        if let Some(task_id) = ov.take_cancel_task() {
            if let Err(e) = self
                .task_repo
                .update_status(&task_id, zhongshu_core::core::TaskStatus::Cancelled)
            {
                tracing::warn!("cancel task failed: {e}");
            }
            refresh = true;
        }
        if let Some(task_id) = ov.take_complete_task() {
            if let Err(e) = self
                .task_repo
                .update_status(&task_id, zhongshu_core::core::TaskStatus::Completed)
            {
                tracing::warn!("complete task failed: {e}");
            }
            refresh = true;
        }
        if refresh {
            let tasks = match self.task_repo.list_open() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("list tasks failed: {e}");
                    vec![]
                }
            };
            let items: Vec<serde_json::Value> = tasks.iter().map(|t| serde_json::json!({
                "id": t.id, "title": t.title, "status": t.status.as_str(), "created_at": t.created_at,
            })).collect();
            ov.show_tasks(&items);
        }
    }

    /// Check for pending authority requests via watch channel and show in overlay.
    fn poll_pending_auth(&mut self) {
        if self.auth_watch.has_changed().unwrap_or(false) {
            let req = self.auth_watch.borrow_and_update().clone();
            if let Some(req) = req {
                if !self.pending_auth_notified {
                    self.pending_auth_notified = true;
                    let title = if req.source.is_empty() {
                        "需要授权".into()
                    } else {
                        format!("需要授权 · {}", req.source)
                    };
                    let _ = zhongshu_core::desktop::notification::show_urgent(
                        &title,
                        &format!("{} - {}", req.tool, req.command),
                    );
                    let request = AuthRequest {
                        request_id: req.id.clone(),
                        tool: req.tool.clone(),
                        source: req.source.clone(),
                        command: req.command.clone(),
                    };
                    if let Some(ref ov) = self.overlay {
                        ov.show_auth(&request);
                    }
                }
            } else {
                self.pending_auth_notified = false;
            }
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
                                        let _ = zhongshu_core::desktop::notification::show(
                                            "完成",
                                            "任务完成",
                                        );
                                    } else {
                                        let _ = zhongshu_core::desktop::notification::show(
                                            "失败",
                                            "任务出错",
                                        );
                                    }
                                }
                            }
                            self.indicator_state = to;
                            if let Some(ind) = self.indicator.as_mut() {
                                ind.set_state(to);
                            }
                            if let Some(ref ov) = self.overlay {
                                match to {
                                    AgentState::Thinking | AgentState::Executing => {
                                        ov.set_state("thinking")
                                    }
                                    AgentState::Done { success } => {
                                        ov.set_state(if success { "done" } else { "stopped" })
                                    }
                                    AgentState::Idle => ov.set_state("idle"),
                                }
                            }
                        }
                        Event::Tool(ToolEvent::Started { name }) => {
                            self.indicator_state = AgentState::Executing;
                            if let Some(ind) = self.indicator.as_mut() {
                                ind.set_state(AgentState::Executing);
                            }
                            if let Some(ref ov) = self.overlay {
                                ov.send(&serde_json::json!({"type":"tool_call","name":name}));
                            }
                        }
                        Event::Tool(ToolEvent::Completed { name, success }) => {
                            if let Some(ref ov) = self.overlay {
                                ov.toast(&format!(
                                    "工具完成: {name} {}",
                                    if success { "✓" } else { "✗" }
                                ));
                            }
                        }
                        Event::Task(TaskEvent::Triggered { title, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ =
                                    zhongshu_core::desktop::notification::show("任务触发", &title);
                            }
                        }
                        Event::Task(TaskEvent::Completed { title, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ =
                                    zhongshu_core::desktop::notification::show("任务完成", &title);
                            }
                        }
                        Event::Harness(event) => {
                            if let Some(ref ov) = self.overlay {
                                match event {
                                    HarnessUiEvent::CodingSessionStarted {
                                        session_id: _,
                                        trace_id: _,
                                        intent: _,
                                        model: _,
                                        deeplossless_conversation_id,
                                        deeplossless_replay_execution_id,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::ReplayAvailable {
                                                conversation_id: deeplossless_conversation_id,
                                                replay_execution_id:
                                                    deeplossless_replay_execution_id,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::CodingPlanCreated {
                                        session_id,
                                        step_count,
                                        risk,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PlanCreated {
                                                session_id,
                                                step_count,
                                                risk,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::CodingStepStarted {
                                        session_id,
                                        step_id,
                                        kind: _,
                                        title,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PlanStepStarted {
                                                session_id,
                                                step_id,
                                                title,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::CodingStepCompleted {
                                        session_id,
                                        step_id,
                                        status,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PlanStepCompleted {
                                                session_id,
                                                step_id,
                                                status,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::WorkerStarted {
                                        session_id,
                                        worker,
                                        task_id,
                                        owned_files,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::WorkerStarted {
                                                session_id,
                                                worker,
                                                task_id,
                                                owned_files: owned_files
                                                    .iter()
                                                    .map(|path| path.display().to_string())
                                                    .collect(),
                                            },
                                        );
                                    }
                                    HarnessUiEvent::WorkerCompleted {
                                        session_id,
                                        worker,
                                        task_id,
                                        success,
                                        trace_event_count: _,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::WorkerCompleted {
                                                session_id,
                                                worker,
                                                task_id,
                                                success,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::WorkerConflict {
                                        session_id,
                                        worker,
                                        task_id,
                                        reason,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::WorkerConflict {
                                                session_id,
                                                worker,
                                                task_id,
                                                reason,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::PatchPreview {
                                        session_id,
                                        path,
                                        operation,
                                        diff_summary,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PatchPreview {
                                                session_id,
                                                path: path.display().to_string(),
                                                operation,
                                                diff_summary,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::PatchApplied {
                                        session_id,
                                        path,
                                        operation,
                                        changed,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PatchApplied {
                                                session_id,
                                                path: path.display().to_string(),
                                                operation,
                                                changed,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::ContextIncluded {
                                        description,
                                        estimated_tokens,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::ContextIncluded {
                                                description,
                                                estimated_tokens,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::ContextPressure {
                                        pressure_percent,
                                        dropped_evidence,
                                        dropped_recent,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::ContextPressure {
                                                pressure_percent,
                                                dropped_evidence,
                                                dropped_recent,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::ReplayAvailable {
                                        conversation_id,
                                        replay_execution_id,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::ReplayAvailable {
                                                conversation_id,
                                                replay_execution_id,
                                            },
                                        );
                                    }
                                    HarnessUiEvent::Verification {
                                        command,
                                        success,
                                        exit_code,
                                        step,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::Verification {
                                                command: command.clone(),
                                                success,
                                                exit_code,
                                            },
                                        );
                                        ov.send(&serde_json::json!({
                                            "type": "verification",
                                            "command": command,
                                            "success": success,
                                            "exit_code": exit_code,
                                            "step": step,
                                        }));
                                    }
                                    HarnessUiEvent::RecoveryFeedback { rule_id, message } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::RecoveryFeedback {
                                                rule_id: rule_id.clone(),
                                                message: message.clone(),
                                            },
                                        );
                                        ov.send(&serde_json::json!({
                                            "type": "recovery_feedback",
                                            "rule_id": rule_id,
                                            "message": message,
                                        }));
                                    }
                                    HarnessUiEvent::PhaseTransition { from, to } => {
                                        ov.send(&serde_json::json!({
                                            "type": "phase_transition",
                                            "from": from,
                                            "to": to,
                                        }));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("event bus lagged: {n} events dropped");
                    active = true;
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
                ResponseEvent::MessageStarted { id, role } => {
                    if matches!(role, ResponseRole::Assistant) {
                        self.assistant_id = Some(id);
                        if let Some(ref ov) = self.overlay {
                            ov.set_state("thinking");
                            ov.send(&serde_json::json!({"type":"model","label": self.controller.model_name()}));
                        }
                    } else {
                        self.assistant_id = None;
                        self.filter = ControlTokenFilter::new();
                    }
                }
                ResponseEvent::MessageDelta { id, delta } => {
                    if self.assistant_id.map(|aid| aid == id).unwrap_or(false) {
                        let cleaned = self.filter.feed(&delta);
                        if !cleaned.is_empty() {
                            if let Some(ref ov) = self.overlay {
                                ov.push_delta(&cleaned);
                            }
                        }
                    }
                }
                ResponseEvent::MessageCompleted { id } => {
                    if self.assistant_id.map(|aid| aid == id).unwrap_or(false) {
                        if let Some(ref ov) = self.overlay {
                            ov.complete_message();
                        }
                        self.filter.flush();
                        self.assistant_id = None;
                    }
                }
            }
        }
        active
    }

    // ── Overlay management ──────────────────────────────────────────

    pub fn delete_all_history(&self) {
        self.controller.set_chat_history(Vec::new());
        self.runtime.block_on(async {
            self.proxy.lock().await.delete_chat_history().await;
        });
        if let Some(ref ov) = self.overlay {
            ov.clear_chat();
        }
    }

    fn overlay_size(&self) -> (f32, f32) {
        if self.config.agent.mode == "coding" {
            (
                self.config.ui.coding_overlay_width,
                self.config.ui.coding_overlay_height,
            )
        } else {
            (self.config.ui.overlay_width, self.config.ui.overlay_height)
        }
    }

    pub fn try_open_overlay(&mut self, _el: &ActiveEventLoop) {
        let (w, h) = self.overlay_size();
        if let Some(ref ov) = self.overlay {
            ov.show_window(w, h);
            return;
        }
        let ov = crate::overlay::show(w, h);
        // Load previous conversation from lcm.db and send to JS
        let proxy = self.proxy.clone();
        let history = self
            .runtime
            .block_on(async { proxy.lock().await.load_chat_history().await });
        let cleaned_history: Vec<(String, String)> = history
            .into_iter()
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

        let entries: Vec<crate::overlay::ChatEntry> = initial
            .iter()
            .map(|(role, content)| crate::overlay::ChatEntry {
                role: if role == "user" {
                    crate::overlay::EntryRole::User
                } else {
                    crate::overlay::EntryRole::Assistant
                },
                content: content.clone(),
                tool_calls: Vec::new(),
            })
            .collect();
        if !entries.is_empty() {
            ov.set_history(&entries, has_more);
        }
        if let Some(ref ov) = self.overlay.as_ref() {
            ov.send(&serde_json::json!({"type":"mode_change","mode":self.config.agent.mode}));
        }
        self.overlay = Some(ov);
    }

    pub fn new_conversation(&mut self, _el: &ActiveEventLoop) {
        self.delete_all_history();
    }
}

// ── ApplicationHandler (winit event loop) ────────────────────────────

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
                if orb_id == Some(id) {
                    el.exit();
                } else {
                    // GTK overlay handles its own close
                }
            }
            WindowEvent::RedrawRequested => {
                if orb_id == Some(id) {
                    self.indicator.as_mut().unwrap().render();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
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
                    match crate::show_context_menu(self.indicator.as_ref().unwrap().window()) {
                        crate::MenuAction::NewConversation => self.new_conversation(el),
                        crate::MenuAction::Quit => el.exit(),
                        crate::MenuAction::None => {}
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if on_orb {
                    self.is_dragging = false;
                    if let Some(ind) = self.indicator.as_ref() {
                        ind.save_position();
                    }
                }
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
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if self.ctrl_held
                && event.state == ElementState::Pressed
                && event.logical_key == "q" =>
            {
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

        if self.hotkey.try_recv().is_some() {
            self.try_open_overlay(el);
        }

        #[cfg(target_os = "linux")]
        {
            let mut tray_events = Vec::new();
            if let Some(Indicator::Tray(t)) = self.indicator.as_mut() {
                while let Ok(ev) = t.rx.try_recv() {
                    tray_events.push(ev);
                }
            }
            for ev in tray_events {
                match ev {
                    crate::indicator::tray::TrayEvent::OpenOverlay => self.try_open_overlay(el),
                    crate::indicator::tray::TrayEvent::NewConversation => self.new_conversation(el),
                    crate::indicator::tray::TrayEvent::Quit => {
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
            tracing::warn!(
                "streaming timeout after {elapsed}s (limit {timeout}s), agent still running"
            );
            self.last_activity = Instant::now();
        }

        let idle = matches!(self.indicator_state, AgentState::Idle);
        let tray_active = cfg!(target_os = "linux");
        el.set_control_flow(if self.overlay.is_some() || !idle {
            ControlFlow::Poll
        } else if tray_active {
            ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200))
        } else {
            ControlFlow::Wait
        });
    }
}
