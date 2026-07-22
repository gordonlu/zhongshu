use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use zhongshu_core::agent::run::RunController;
use zhongshu_core::agent::{AgentRuntime, ModelRouter};
use zhongshu_core::authority;
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, EventRx, HarnessUiEvent, MessageId, ResponseEvent,
    ResponseRole, ResponseRx, ResponseTx, RunEvent, TaskEvent, ToolEvent,
};
use zhongshu_core::integration::DeeplosslessProxy;
use zhongshu_message_core::streaming::ControlTokenFilter;

use crate::app::{AgentController, AgentInbox};
use crate::auto_delegation_service::AutoDelegationController;
use crate::config::AppConfig;
use crate::delegation_service::DelegationController;
use crate::hotkey::HotkeyManager;
use crate::indicator::Indicator;
use crate::organization_service::OrganizationController;
use uuid::Uuid;

use crate::overlay::{AuthRequest, OverlayHandle};
use crate::overlay_contract::{
    chat_coding_smoke_commands, chat_coding_smoke_events, CodingUiEvent, OverlayToUiEvent,
};
use crate::overlay_host::OverlayHandleExt;
use zhongshu_core::equipment::{EquipmentObserver, EquipmentRegistry};
use zhongshu_core::tool::ToolRegistry;

const MIN_STREAMING_TIMEOUT_SECS: u64 = 120;

fn effective_streaming_timeout_secs(configured: u64) -> u64 {
    configured.max(MIN_STREAMING_TIMEOUT_SECS)
}

fn send_coding_event(overlay: &OverlayHandle, event: CodingUiEvent) {
    overlay.send(&serde_json::to_value(OverlayToUiEvent::Coding { event }).unwrap_or_default());
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

// ── App state ────────────────────────────────────────────────────────

pub struct ZhongshuApp {
    pub config: AppConfig,
    pub controller: Arc<AgentController>,
    pub proxy: Arc<tokio::sync::Mutex<DeeplosslessProxy>>,
    pub runtime: tokio::runtime::Runtime,
    pub inbox: Arc<AgentInbox>,
    pub delegation: Arc<DelegationController>,
    pub organization: Arc<OrganizationController>,
    pub auto_delegation: Arc<AutoDelegationController>,
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
    /// Cursor for events already sent to the overlay, so reconnects only
    /// replay events the overlay hasn't seen.
    pub overlay_event_cursor: u64,
    pub auth_watch: watch::Receiver<Option<zhongshu_core::authority::PendingRequest>>,
    pub run_controller: Arc<RunController>,
    pub active_run_id: Option<Uuid>,
    pub organization_graph_versions: Vec<(String, u64)>,
    pub last_organization_graph_poll: Instant,
}

impl ZhongshuApp {
    pub fn new(
        config: AppConfig,
        controller: Arc<AgentController>,
        inbox: Arc<AgentInbox>,
        delegation: Arc<DelegationController>,
        organization: Arc<OrganizationController>,
        auto_delegation: Arc<AutoDelegationController>,
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
        run_controller: Arc<RunController>,
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
            delegation,
            organization,
            auto_delegation,
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
            overlay_event_cursor: 0,
            auth_watch,
            run_controller,
            active_run_id: None,
            organization_graph_versions: Vec::new(),
            last_organization_graph_poll: Instant::now(),
        })
    }

    // ── Event reducers ──────────────────────────────────────────────

    fn drain(&mut self) {
        let activity = self.reduce_events() | self.reduce_responses();
        self.poll_pending_auth();
        self.poll_overlay_actions();
        self.poll_organization_graphs();
        if activity {
            self.last_activity = Instant::now();
        }
    }

    fn poll_organization_graphs(&mut self) {
        let Some(overlay) = self.overlay.as_ref() else {
            return;
        };
        let force = overlay.take_list_organization_graphs();
        if !force && self.last_organization_graph_poll.elapsed() < Duration::from_millis(750) {
            return;
        }
        self.last_organization_graph_poll = Instant::now();
        match self.runtime.block_on(self.organization.recovery_graphs()) {
            Ok(graphs) => {
                let versions = graphs
                    .iter()
                    .map(|view| (view.graph.task_id.clone(), view.store_version))
                    .collect::<Vec<_>>();
                if force || versions != self.organization_graph_versions {
                    self.organization_graph_versions = versions;
                    overlay.send(
                        &serde_json::to_value(OverlayToUiEvent::OrganizationGraphs { graphs })
                            .unwrap_or_default(),
                    );
                }
            }
            Err(error) if force => {
                overlay.toast(&format!("无法读取 DAG 控制面：{error}"));
            }
            Err(error) => {
                tracing::warn!(%error, "failed to poll organization DAG control plane");
            }
        }
    }

    pub fn sync_active_run_id(&mut self) {
        let prev = self.active_run_id;
        self.active_run_id = self.run_controller.run_id();
        if prev != self.active_run_id {
            tracing::debug!(?self.active_run_id, "active_run_id synced");
        }
    }

    /// Poll pending actions from overlay IPC.
    fn poll_overlay_actions(&mut self) {
        let ov = match self.overlay.as_ref() {
            Some(ov) => ov,
            None => return,
        };

        if ov.take_list_organization() {
            let employees = self.runtime.block_on(self.organization.employees());
            ov.send(
                &serde_json::to_value(OverlayToUiEvent::OrganizationRoster {
                    employees,
                    max_workers: zhongshu_core::agent::DEFAULT_MAX_WORKERS_PER_TASK,
                })
                .unwrap_or_default(),
            );
        }
        if let Some(command) = ov.take_organization_recovery() {
            if !self.controller.is_idle()
                || self.delegation.is_busy()
                || self.organization.is_busy()
                || self.auto_delegation.is_busy()
            {
                ov.toast("当前已有任务运行，恢复操作未执行。");
            } else {
                match self
                    .runtime
                    .block_on(self.organization.recover_graph(command))
                {
                    Ok(result) => ov.send(
                        &serde_json::to_value(OverlayToUiEvent::OrganizationRecovery { result })
                            .unwrap_or_default(),
                    ),
                    Err(error) => ov.toast(&format!("DAG 恢复操作失败：{error}")),
                }
            }
        }
        if let Some(task) = ov.take_delegate_organization() {
            if !self.controller.is_idle()
                || self.delegation.is_busy()
                || self.organization.is_busy()
                || self.auto_delegation.is_busy()
            {
                ov.toast("当前已有任务运行，请结束后再组建团队。");
            } else {
                ov.send(&serde_json::json!({
                    "type": "user_message",
                    "content": task.objective,
                }));
                self.observer
                    .lock()
                    .unwrap()
                    .record_user_message(&task.objective);
                let ok = if task.mutation {
                    self.organization.submit_mutation(task)
                } else {
                    self.organization.submit(task)
                };
                if !ok {
                    ov.toast("组织任务已经在运行。");
                }
            }
        }
        if let Some(text) = ov.take_delegate_review() {
            if !self.controller.is_idle()
                || self.delegation.is_busy()
                || self.organization.is_busy()
                || self.auto_delegation.is_busy()
            {
                ov.toast("当前已有任务运行，请结束后再委派。");
            } else {
                ov.send(&serde_json::json!({"type": "user_message", "content": text}));
                self.observer.lock().unwrap().record_user_message(&text);
                if !self.delegation.submit_review(text) {
                    ov.toast("双员工协作已经在运行。");
                }
            }
        }
        if let Some(text) = ov.take_input() {
            if env_flag("ZHONGSHU_ORB_SMOKE_INTERACTION") {
                tracing::info!(
                    text_len = text.len(),
                    "webview2 interaction smoke submit ipc received"
                );
            }
            ov.send(&serde_json::json!({"type": "user_message", "content": text}));
            self.observer.lock().unwrap().record_user_message(&text);
            if self.delegation.is_busy()
                || self.organization.is_busy()
                || self.auto_delegation.is_busy()
            {
                ov.toast("员工协作正在运行，请先停止或等待汇报。");
            } else if self.config.agent.auto_multi_agent && !self.controller.is_idle() {
                ov.toast("主 AI 正在运行，自动多 Agent 路由未启动。");
            } else if self.config.agent.auto_multi_agent {
                if !self.auto_delegation.submit(text) {
                    ov.toast("自动多 Agent 路由已经在运行。");
                }
            } else {
                self.inbox.submit(text);
            }
        }
        if let Some(rid) = ov.take_approve() {
            if let Some(req) = authority::get_pending(&rid) {
                self.run_controller.record_approval(&req.tool, "approved");
            }
            authority::approve_pending(&rid);
        }
        if let Some(rid) = ov.take_deny() {
            if let Some(req) = authority::get_pending(&rid) {
                self.run_controller.record_approval(&req.tool, "denied");
            }
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
            if let Some(key) = &settings.api_key {
                if !key.is_empty() {
                    if let Err(e) = crate::config::store_api_key(key) {
                        tracing::warn!("save API key failed: {e:#}");
                        ov.toast("API Key 保存失败");
                    } else {
                        ov.toast("API Key 已保存到系统凭据库");
                    }
                }
            }
            if let Some(base) = &settings.api_base {
                if !base.is_empty() {
                    cfg.llm.api_base.clone_from(base);
                }
            }
            if let Some(model) = &settings.model {
                if !model.is_empty() {
                    cfg.llm.model.clone_from(model);
                }
            }
            if let Some(port) = &settings.proxy_port {
                if !port.is_empty() {
                    cfg.deeplossless.proxy_port = port.parse().unwrap_or(8081);
                }
            }
            if let Some(enabled) = settings.bg_enabled {
                cfg.agent.background.enabled = enabled;
            }
            if let Some(interval) = &settings.bg_interval {
                if !interval.is_empty() {
                    cfg.agent.background.interval_secs = interval.parse().unwrap_or(600);
                }
            }
            if let Some(prompt) = &settings.bg_prompt {
                cfg.agent.background.prompt.clone_from(prompt);
            }
            if let Some(evolve) = settings.auto_evolve {
                cfg.agent.auto_evolve = evolve;
                self.controller.set_auto_evolve(evolve);
            }
            if let Some(enabled) = settings.auto_multi_agent {
                cfg.agent.auto_multi_agent = enabled;
            }
            if let Some(ctx) = settings.max_context_tokens {
                if ctx >= 100_000 && ctx <= 1_000_000 {
                    cfg.llm.max_context_tokens = ctx;
                    self.controller.set_max_context_tokens(ctx);
                }
            }
            if let Some(mode) = &settings.mode {
                cfg.agent.mode.clone_from(mode);
                self.controller.set_mode(mode.clone());
                if let Some(ref ov) = self.overlay {
                    ov.send(&serde_json::json!({"type":"mode_change","mode":mode}));
                }
            }
            if let Some(personality) = &settings.personality {
                if personality != "默认" && !personality.is_empty() {
                    cfg.agent.personality.clone_from(personality);
                    cfg.agent.personality_selected = true;
                    self.controller
                        .set_system_prompt(cfg.agent.effective_system_prompt());
                }
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
            let provider = cfg.llm.build_provider(&base_url);
            self.runtime.block_on(async {
                let mut worker_runtime = self.worker_runtime.write().await;
                worker_runtime.provider = provider.clone();
                worker_runtime.model = cfg.llm.model.clone();
            });
            let planner_model = if cfg.llm.model_routing.enabled {
                cfg.llm.model_routing.pro_model.clone()
            } else {
                cfg.llm.model.clone()
            };
            let planner_reasoning = cfg
                .llm
                .model_routing
                .enabled
                .then(|| cfg.llm.model_routing.reasoning_agent.clone());
            self.runtime
                .block_on(self.organization.update_planner_runtime(
                    provider.change_model(&planner_model),
                    planner_model,
                    planner_reasoning,
                ));
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
            if self.auto_delegation.cancel()
                || self.delegation.cancel()
                || self.organization.cancel()
            {
                ov.send(&serde_json::json!({"type": "stop"}));
                return;
            }
            use zhongshu_core::agent::run::RunState;
            if matches!(self.run_controller.state(), RunState::Interrupted { .. }) {
                // Already interrupted — fully cancel
                self.controller.cancel();
            } else {
                // Interrupt current run
                self.run_controller.interrupt("stop");
                ov.send(&serde_json::json!({"type": "stop"}));
            }
        }
        if ov.take_toggle_zoom() {
            self.overlay_zoomed = !self.overlay_zoomed;
            let scale = if self.overlay_zoomed { 2.0 } else { 1.0 };
            let (w, h) = self.overlay_size();
            ov.show_window(w * scale, h * scale);
            ov.send(&serde_json::json!({"type":"zoom","active": self.overlay_zoomed}));
            if env_flag("ZHONGSHU_ORB_SMOKE_INTERACTION") {
                tracing::info!(
                    zoomed = self.overlay_zoomed,
                    "webview2 interaction smoke zoom ipc received"
                );
            }
        }
        if ov.take_start_drag() {
            ov.start_drag_window();
        }
        if ov.take_minimize() {
            ov.minimize_window();
        }
        if ov.take_maximize_restore() {
            ov.maximize_restore_window();
        }
        if ov.take_close_window() {
            ov.close_window();
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
                auto_multi_agent: Some(cfg.agent.auto_multi_agent),
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
            } else if let Ok(Some(task)) = self.task_repo.get(&task_id) {
                self.event_bus.publish(Event::Task(TaskEvent::Cancelled {
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    reason: "UI 取消".into(),
                }));
            }
            refresh = true;
        }
        if let Some(task_id) = ov.take_complete_task() {
            if let Err(e) = self
                .task_repo
                .update_status(&task_id, zhongshu_core::core::TaskStatus::Completed)
            {
                tracing::warn!("complete task failed: {e}");
            } else if let Ok(Some(task)) = self.task_repo.get(&task_id) {
                self.event_bus.publish(Event::Task(TaskEvent::Completed {
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    output: String::new(),
                }));
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
                    let event_sequence = self
                        .event_bus
                        .sequence_for_event_since(self.overlay_event_cursor, &ev);
                    if self.overlay_event_cursor > 0 && event_sequence.is_none() {
                        // This occurrence was already delivered by replay.
                        continue;
                    }
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
                                    AgentState::Submitted => {
                                        ov.set_state("done");
                                        ov.toast("结果已提交，但尚未获得验证证据。");
                                    }
                                    AgentState::Done { success } => {
                                        ov.set_state(if success { "done" } else { "stopped" })
                                    }
                                    AgentState::Idle => ov.set_state("idle"),
                                }
                            }
                        }
                        Event::Tool(ToolEvent::Started { name, .. }) => {
                            self.indicator_state = AgentState::Executing;
                            self.last_activity = Instant::now();
                            if let Some(ind) = self.indicator.as_mut() {
                                ind.set_state(AgentState::Executing);
                            }
                            if let Some(ref ov) = self.overlay {
                                ov.send(&serde_json::json!({"type":"tool_call","name":name}));
                            }
                        }
                        Event::Tool(ToolEvent::Completed { name, success, .. }) => {
                            if let Some(ref ov) = self.overlay {
                                ov.send(&serde_json::json!({
                                    "type": "tool_result",
                                    "name": name.clone(),
                                    "success": success,
                                }));
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
                        Event::Task(TaskEvent::Claimed { task_id, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ = zhongshu_core::desktop::notification::show(
                                    "任务已开始执行",
                                    &task_id,
                                );
                            }
                        }
                        Event::Task(TaskEvent::Completed { title, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ =
                                    zhongshu_core::desktop::notification::show("任务完成", &title);
                            }
                        }
                        Event::Task(TaskEvent::Failed { title, error, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ = zhongshu_core::desktop::notification::show(
                                    "任务失败",
                                    &format!("{title}: {error}"),
                                );
                            }
                        }
                        Event::Task(TaskEvent::Cancelled { title, reason, .. }) => {
                            if self.config.agent.desktop_notification {
                                let _ = zhongshu_core::desktop::notification::show(
                                    "任务已取消",
                                    &format!("{title}: {reason}"),
                                );
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
                                        status,
                                        trace_event_count: _,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::WorkerCompleted {
                                                session_id,
                                                worker,
                                                task_id,
                                                success,
                                                status,
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
                                        diff,
                                    } => {
                                        send_coding_event(
                                            ov,
                                            CodingUiEvent::PatchPreview {
                                                session_id,
                                                path: path.display().to_string(),
                                                operation,
                                                diff_summary,
                                                diff: diff.map(Into::into),
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
                        Event::Organization(event) => {
                            if let Some(ref ov) = self.overlay {
                                ov.send(
                                    &serde_json::to_value(OverlayToUiEvent::Organization { event })
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        Event::Run(run_event) => match run_event {
                            RunEvent::Interrupted { .. } => {
                                if let Some(ref ov) = self.overlay {
                                    ov.toast("已按你的新消息暂停当前步骤。");
                                    ov.set_state("paused");
                                }
                            }
                            RunEvent::Resuming { .. } => {
                                if let Some(ref ov) = self.overlay {
                                    ov.toast("正在按新约束重新调整。");
                                    ov.set_state("thinking");
                                }
                            }
                            RunEvent::Cancelled { .. } => {
                                if let Some(ref ov) = self.overlay {
                                    ov.toast("已停止后续操作。");
                                    ov.set_state("idle");
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                    if let Some(sequence) = event_sequence {
                        self.overlay_event_cursor = sequence + 1;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("event bus lagged: {n} events dropped, replaying snapshot");
                    active = true;
                    self.replay_to_overlay();
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed)
                | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            }
        }
        active
    }

    fn reduce_responses(&mut self) -> bool {
        let mut active = false;
        self.sync_active_run_id();
        while let Ok(ev) = self.response_rx.try_recv() {
            active = true;
            let accept = match self.active_run_id {
                Some(rid) => {
                    let ev_rid = match ev {
                        ResponseEvent::MessageStarted { ref run_id, .. }
                        | ResponseEvent::MessageDelta { ref run_id, .. }
                        | ResponseEvent::MessageCompleted { ref run_id, .. } => run_id,
                    };
                    *ev_rid == rid
                }
                None => true,
            };
            if !accept {
                continue;
            }
            match ev {
                ResponseEvent::MessageStarted { id, role, .. } => {
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
                ResponseEvent::MessageDelta { id, delta, .. } => {
                    if self.assistant_id.map(|aid| aid == id).unwrap_or(false) {
                        let cleaned = self.filter.feed(&delta);
                        if !cleaned.is_empty() {
                            if let Some(ref ov) = self.overlay {
                                ov.push_delta(&cleaned);
                            }
                        }
                    }
                }
                ResponseEvent::MessageCompleted { id, .. } => {
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

    pub fn try_open_overlay(&mut self, el: &ActiveEventLoop) {
        let (w, h) = self.overlay_size();
        if let Some(ref ov) = self.overlay {
            ov.show_window(w, h);
            return;
        }
        let ov = crate::overlay::show(el, w, h);
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
        let diagnostics = ov.host_diagnostics();
        tracing::info!(
            platform = %diagnostics.platform,
            webview_available = diagnostics.webview_available,
            startup_error = diagnostics.startup_error.as_deref().unwrap_or(""),
            "overlay host diagnostics"
        );
        ov.send(&serde_json::json!({"type":"mode_change","mode":self.config.agent.mode}));
        if env_flag("ZHONGSHU_ORB_SMOKE_CODING") {
            tracing::info!("sending overlay coding smoke events");
            let events_json =
                serde_json::to_string(&chat_coding_smoke_events()).unwrap_or_default();
            ov.eval(&format!(
                r#"(function zhongshuSmokeDeliver() {{
                    const events = {events_json};
                    if (window.handleIpc) {{
                        events.forEach((event) => window.handleIpc(event));
                    }} else {{
                        setTimeout(zhongshuSmokeDeliver, 200);
                    }}
                }})();"#
            ));
        }
        if env_flag("ZHONGSHU_ORB_SMOKE_INTERACTION") {
            tracing::info!("sending webview2 interaction smoke commands");
            let commands_json =
                serde_json::to_string(&chat_coding_smoke_commands()).unwrap_or_default();
            ov.eval(&format!(
                r#"(function zhongshuInteractionSmokeDeliver() {{
                    const commands = {commands_json};
                    const bridge = window.chrome && window.chrome.webview;
                    if (bridge && bridge.postMessage) {{
                        commands.forEach((command) => bridge.postMessage(command));
                    }} else {{
                        setTimeout(zhongshuInteractionSmokeDeliver, 200);
                    }}
                }})();"#
            ));
        }
        self.overlay = Some(ov);
        // Replay buffered events so the newly-opened overlay catches up on
        // tool calls, coding session state, run state, etc.
        self.replay_to_overlay();
    }

    /// Send buffered events since the last cursor to the overlay.
    /// Includes the cursor value so the UI can maintain its own cursor
    /// and detect gaps.
    fn replay_to_overlay(&mut self) {
        let Some(ref ov) = self.overlay else {
            return;
        };
        let cursor = self.overlay_event_cursor;
        let events = self.event_bus.recent_since_cursor(cursor);
        if events.is_empty() {
            return;
        }
        tracing::info!(
            count = events.len(),
            from_cursor = cursor,
            "replaying events to overlay"
        );
        // Send a snapshot of the current cursor before individual events,
        // so the UI can gap-detect.
        ov.send(&serde_json::json!({
            "type": "cursor_snapshot",
            "cursor": self.event_bus.current_cursor(),
        }));
        for (_seq, event) in &events {
            match event {
                Event::Agent(AgentEvent::StateChanged { from: _, to }) => match to {
                    AgentState::Thinking | AgentState::Executing => {
                        ov.set_state("thinking");
                    }
                    AgentState::Submitted => {
                        ov.set_state("done");
                        ov.toast("结果已提交，但尚未获得验证证据。");
                    }
                    AgentState::Done { success } => {
                        ov.set_state(if *success { "done" } else { "stopped" });
                    }
                    AgentState::Idle => ov.set_state("idle"),
                },
                Event::Tool(ToolEvent::Started { name, .. }) => {
                    ov.send(&serde_json::json!({"type":"tool_call","name":name}));
                }
                Event::Tool(ToolEvent::Completed { name, success, .. }) => {
                    ov.send(&serde_json::json!({
                        "type": "tool_result",
                        "name": name.clone(),
                        "success": success,
                    }));
                }
                Event::Tool(ToolEvent::Interrupted { name, .. }) => {
                    ov.send(&serde_json::json!({
                        "type": "tool_result",
                        "name": name.clone(),
                        "success": false,
                    }));
                }
                Event::Harness(event) => match event {
                    HarnessUiEvent::CodingSessionStarted { .. } => {}
                    HarnessUiEvent::CodingPlanCreated {
                        session_id,
                        step_count,
                        risk,
                    } => {
                        send_coding_event(
                            ov,
                            CodingUiEvent::PlanCreated {
                                session_id: session_id.clone(),
                                step_count: *step_count,
                                risk: risk.clone(),
                            },
                        );
                    }
                    HarnessUiEvent::CodingStepStarted {
                        session_id,
                        step_id,
                        title,
                        ..
                    } => {
                        send_coding_event(
                            ov,
                            CodingUiEvent::PlanStepStarted {
                                session_id: session_id.clone(),
                                step_id: step_id.clone(),
                                title: title.clone(),
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
                                session_id: session_id.clone(),
                                step_id: step_id.clone(),
                                status: status.clone(),
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
                                session_id: session_id.clone(),
                                worker: worker.clone(),
                                task_id: task_id.clone(),
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
                        status,
                        ..
                    } => {
                        send_coding_event(
                            ov,
                            CodingUiEvent::WorkerCompleted {
                                session_id: session_id.clone(),
                                worker: worker.clone(),
                                task_id: task_id.clone(),
                                success: *success,
                                status: status.clone(),
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
                                session_id: session_id.clone(),
                                worker: worker.clone(),
                                task_id: task_id.clone(),
                                reason: reason.clone(),
                            },
                        );
                    }
                    HarnessUiEvent::PatchPreview {
                        session_id,
                        path,
                        operation,
                        diff_summary,
                        diff,
                    } => {
                        send_coding_event(
                            ov,
                            CodingUiEvent::PatchPreview {
                                session_id: session_id.clone(),
                                path: path.display().to_string(),
                                operation: operation.clone(),
                                diff_summary: diff_summary.clone(),
                                diff: diff.clone().map(Into::into),
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
                                session_id: session_id.clone(),
                                path: path.display().to_string(),
                                operation: operation.clone(),
                                changed: *changed,
                            },
                        );
                    }
                    HarnessUiEvent::ContextIncluded { .. } => {}
                    HarnessUiEvent::ContextPressure {
                        pressure_percent,
                        dropped_evidence,
                        dropped_recent,
                    } => {
                        send_coding_event(
                            ov,
                            CodingUiEvent::ContextPressure {
                                pressure_percent: *pressure_percent,
                                dropped_evidence: *dropped_evidence,
                                dropped_recent: *dropped_recent,
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
                                conversation_id: *conversation_id,
                                replay_execution_id: replay_execution_id.clone(),
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
                                success: *success,
                                exit_code: *exit_code,
                            },
                        );
                        ov.send(&serde_json::json!({
                            "type": "verification",
                            "command": command.clone(),
                            "success": success,
                            "exit_code": exit_code,
                            "step": step,
                        }));
                    }
                    HarnessUiEvent::RecoveryFeedback { rule_id, message } => {
                        ov.send(&serde_json::json!({
                            "type": "recovery_feedback",
                            "rule_id": rule_id.clone(),
                            "message": message.clone(),
                        }));
                    }
                    HarnessUiEvent::PhaseTransition { from, to } => {
                        ov.send(&serde_json::json!({
                            "type": "phase_transition",
                            "from": from.clone(),
                            "to": to.clone(),
                        }));
                    }
                },
                Event::Run(run_event) => match run_event {
                    RunEvent::Interrupted { .. } => {
                        ov.toast("已按你的新消息暂停当前步骤。");
                        ov.set_state("paused");
                    }
                    RunEvent::Resuming { .. } => {
                        ov.toast("正在按新约束重新调整。");
                        ov.set_state("thinking");
                    }
                    RunEvent::Cancelled { .. } => {
                        ov.toast("已停止后续操作。");
                        ov.set_state("idle");
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        // Update the stored cursor so subsequent replays skip seen events.
        if let Some(last_seq) = events.last().map(|(seq, _)| *seq) {
            self.overlay_event_cursor = last_seq + 1;
        }
    }

    pub fn new_conversation(&mut self, _el: &ActiveEventLoop) {
        self.delete_all_history();
    }
}

// ── ApplicationHandler (winit event loop) ────────────────────────────

impl ApplicationHandler for ZhongshuApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        self.indicator = Some(Indicator::create(el, self.config.ui.orb_size));
        if env_flag("ZHONGSHU_ORB_OPEN_ON_START") {
            self.try_open_overlay(el);
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.drain();
        let orb_id = self.indicator.as_ref().and_then(|i| i.window_id());
        let overlay_id = self.overlay.as_ref().and_then(|ov| ov.window_id());
        if overlay_id == Some(id)
            && self
                .overlay
                .as_mut()
                .map(|ov| ov.handle_window_event(&event))
                .unwrap_or(false)
        {
            return;
        }
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
        let timeout = effective_streaming_timeout_secs(self.config.agent.streaming_timeout_secs);
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

#[cfg(test)]
mod tests {
    use super::effective_streaming_timeout_secs;

    #[test]
    fn streaming_timeout_has_a_two_minute_floor() {
        assert_eq!(effective_streaming_timeout_secs(60), 120);
        assert_eq!(effective_streaming_timeout_secs(120), 120);
        assert_eq!(effective_streaming_timeout_secs(300), 300);
    }
}
