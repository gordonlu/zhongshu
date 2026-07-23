mod agent;
mod app;
mod auto_delegation_service;
mod config;
mod delegation_service;
mod handler;
mod hotkey;
mod indicator;
mod organization_service;
#[cfg(target_os = "linux")]
mod overlay;
#[cfg(target_os = "macos")]
#[path = "overlay_macos.rs"]
mod overlay;
#[cfg(windows)]
#[path = "overlay_windows.rs"]
mod overlay;
mod overlay_assets;
mod overlay_contract;
mod overlay_host;
mod render;
mod services;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use winit::event_loop::EventLoop;

use zhongshu_core::agent::{
    AgentBudget, AgentProfile, AgentRuntime, AttentionDispatcher, AttentionManager,
    EmployeeCapability, EmployeeRole, ModelRouter, VerificationPolicy,
};
use zhongshu_core::authority::{self, AuthorityGate};
use zhongshu_core::core::{
    Database, DurableExecutionRunner, EventLogStore, ExecutionGraphStore, GoalRepository, GoalTool,
    MemoryCandidateStore, MemoryPolicy, MemoryQueryTool, ObservationStore, RunLedger, RunbookStore,
    Scheduler, SuggestionEngine, SuggestionTool, TaskRepository, TaskTool,
};
use zhongshu_core::digest::DigestBuilder;
use zhongshu_core::equipment::EquipmentObserver;
use zhongshu_core::event::{Event, EventBus, EventLogger, MessageId, ResponseEvent, ResponseRole};
use zhongshu_core::heartbeat::Heartbeat;
use zhongshu_core::integration::DeeplosslessProxy;
use zhongshu_core::rule::{Rule, RuleCondition, RuleEngine, RuleTask};
use zhongshu_core::source::{BatterySource, DiskUsageSource, SourceManager, TimerSource};
use zhongshu_core::task::{FileWatchTrigger, ReminderTrigger, TaskScheduler};
use zhongshu_core::tool::default_registry;

use app::{AgentController, AgentInbox, TaskWorkerDispatcher};
use auto_delegation_service::AutoDelegationController;
use delegation_service::DelegationController;
use organization_service::OrganizationController;

use handler::ZhongshuApp;
use tokio::sync::mpsc;

fn preflight_checks() {
    let bus = Arc::new(EventBus::new(4));
    let mut rx = bus.subscribe();
    bus.publish(zhongshu_core::event::Event::Agent(
        zhongshu_core::event::AgentEvent::StateChanged {
            from: zhongshu_core::event::AgentState::Idle,
            to: zhongshu_core::event::AgentState::Thinking,
        },
    ));
    assert!(rx.try_recv().is_ok(), "preflight: event bus failed");
    let (tx, mut response_rx) = mpsc::channel::<ResponseEvent>(4);
    let id = MessageId::new();
    assert!(
        tx.try_send(ResponseEvent::MessageStarted {
            id,
            role: ResponseRole::System,
            run_id: uuid::Uuid::default(),
        })
        .is_ok(),
        "preflight: response tx failed"
    );
    assert!(
        response_rx.try_recv().is_ok(),
        "preflight: response rx failed"
    );
    tracing::info!("preflight checks passed");
}

// ── Context menu (for orb window) ───────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    NewConversation,
    Quit,
    None,
}

#[cfg(target_os = "windows")]
pub fn show_context_menu(orb_window: Option<&std::sync::Arc<winit::window::Window>>) -> MenuAction {
    use winit::raw_window_handle::HasWindowHandle;
    let w = match orb_window {
        Some(w) => w,
        None => return MenuAction::None,
    };
    let handle = match w.window_handle() {
        Ok(h) => h,
        Err(_) => return MenuAction::None,
    };
    let hwnd = match handle.as_ref() {
        winit::raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get(),
        _ => return MenuAction::None,
    };

    const MF_STRING: u32 = 0;
    const TPM_RETURNCMD: u32 = 0x0100;

    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }

    extern "system" {
        fn CreatePopupMenu() -> *mut std::ffi::c_void;
        fn AppendMenuW(
            hmenu: *mut std::ffi::c_void,
            flags: u32,
            id: usize,
            text: *const u16,
        ) -> i32;
        fn TrackPopupMenu(
            hmenu: *mut std::ffi::c_void,
            flags: u32,
            x: i32,
            y: i32,
            reserved: i32,
            hwnd: isize,
            rect: *const std::ffi::c_void,
        ) -> u32;
        fn DestroyMenu(hmenu: *mut std::ffi::c_void) -> i32;
        fn GetCursorPos(pt: *mut POINT) -> i32;
    }

    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() {
            return MenuAction::None;
        }

        let new_conv: Vec<u16> = "新建对话\0".encode_utf16().collect();
        let quit: Vec<u16> = "退出\0".encode_utf16().collect();

        AppendMenuW(hmenu, MF_STRING, 1, new_conv.as_ptr());
        AppendMenuW(hmenu, MF_STRING, 2, quit.as_ptr());

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            hwnd as isize,
            std::ptr::null(),
        );

        DestroyMenu(hmenu);

        match cmd {
            1 => MenuAction::NewConversation,
            2 => MenuAction::Quit,
            _ => MenuAction::None,
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn show_context_menu(
    _orb_window: Option<&std::sync::Arc<winit::window::Window>>,
) -> MenuAction {
    MenuAction::None
}

// ── Entry point ──────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ZHONGSHU_LOG").unwrap_or_else(|_| {
                "info,wgpu_hal=off,wgpu_core=error,naga=error,sctk_adwaita=error,deeplossless=warn".into()
            }),
        )
        .init();

    preflight_checks();

    let cfg = config::load();
    let ak = cfg.llm.api_key();
    if ak.is_empty() && !cfg.llm.offline_enabled() {
        tracing::warn!("{} not set; agent will not function", cfg.llm.api_key_env);
    } else if cfg.llm.offline_enabled() {
        tracing::info!("offline scripted LLM provider enabled");
    }

    // Shared tokio runtime for all async work (proxy, agent, background).
    let r = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _g = r.enter();

    // Start deeplossless proxy.
    let proxy_port = cfg.deeplossless.proxy_port;
    let mut proxy = r
        .block_on(async {
            DeeplosslessProxy::new(zhongshu_core::integration::DeeplosslessConfig {
                api_key: ak.clone(),
                upstream: cfg.llm.proxy_upstream(),
                proxy_port,
                ..Default::default()
            })
            .await
        })
        .expect("deeplossless proxy failed to build");

    let actual_port = r
        .block_on(async { proxy.start(proxy_port).await })
        .expect("deeplossless proxy failed to start");
    let base_url = format!("http://127.0.0.1:{actual_port}/v1");
    tracing::info!("deeplossless proxy at {base_url}");
    let proxy = Arc::new(tokio::sync::Mutex::new(proxy));

    let eb = Arc::new(EventBus::new(4096));
    let (response_tx, response_rx) = mpsc::channel::<ResponseEvent>(cfg.agent.response_capacity);
    let event_rx = eb.subscribe();

    authority::init(AuthorityGate::new(
        cfg.agent.authority.enabled,
        cfg.agent.authority.sudo_timeout_secs,
    ));

    // AttentionDispatcher: shows desktop notifications for attention events.
    let desktop_notif = cfg.agent.desktop_notification;
    let dispatcher = AttentionDispatcher::new(Box::new(move |worker, summary| {
        if desktop_notif {
            let _ = zhongshu_core::desktop::notification::show(worker, summary);
        }
    }));
    let _dispatcher_handle = dispatcher.spawn(&eb);

    // ── 军器监初始化 ──
    let equipment_dir = config::config_dir().join("equipment");
    std::fs::create_dir_all(&equipment_dir).unwrap_or(());
    let _ = std::fs::remove_dir_all(equipment_dir.join("search-files"));
    let mut reg = zhongshu_core::equipment::EquipmentRegistry::new(equipment_dir);
    reg.install_defaults();
    let base_system_prompt = cfg.agent.effective_system_prompt();
    let mut system_prompt = base_system_prompt.clone();
    for (_id, prompt) in &reg.skill_prompts() {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(prompt);
    }
    let equipment = Arc::new(Mutex::new(reg));
    let (_observer_handle, observer) = EquipmentObserver::new().spawn(&eb);

    // ── 核心数据库 ──
    let core_db_path = config::config_dir().join("core.db");
    {
        let db = Database::new(core_db_path.clone());
        if let Err(e) = db.migrate() {
            tracing::warn!("core database migration failed: {e}");
        }
    }
    let observation_store = ObservationStore::new(Database::new(core_db_path.clone()));
    let suggestion_engine = SuggestionEngine::new(Database::new(core_db_path.clone()));
    let suggestion_tool = SuggestionTool::new(suggestion_engine.clone()).with_event_bus(eb.clone());
    let memory_policy = MemoryPolicy::new(Database::new(core_db_path.clone()))
        .with_event_bus((*eb).clone());
    let memory_candidate_store = MemoryCandidateStore::new(Database::new(core_db_path.clone()));
    let provider = cfg.llm.build_provider(&base_url);

    let memory_query_tool =
        MemoryQueryTool::new(memory_policy.clone(), memory_candidate_store.clone())
            .with_provider(provider.clone());
    let _event_log = EventLogStore::new(Database::new(core_db_path.clone()));
    let scheduler = Scheduler::new(Database::new(core_db_path.clone())).with_event_bus(eb.clone());

    let goal_tool = GoalTool::new(GoalRepository::new(Database::new(core_db_path.clone())));
    let task_tool = TaskTool::new(TaskRepository::new(Database::new(core_db_path.clone())))
        .with_event_bus(eb.clone());
    let llm_registry = std::sync::Arc::new(cfg.llm.to_registry());
    if let Ok(primary) = llm_registry.client_for_role("primary") {
        tracing::info!("LLM registry: primary={}", primary.model);
    }

    // ── Background services ──
    services::spawn_scheduler(scheduler);
    services::spawn_memory_evaluation(memory_policy.clone(), llm_registry.clone());
    services::spawn_suggestion_analysis(observation_store.clone(), suggestion_engine.clone());
    services::spawn_event_observation_feed(eb.clone(), observation_store.clone());
    services::spawn_event_workflow(eb.clone(), core_db_path.clone());
    services::spawn_llm_suggestion_engine(llm_registry.clone(), core_db_path.clone());
    services::spawn_compensation(eb.clone(), core_db_path.clone());

    let memory_tool =
        zhongshu_core::tool::memory::MemoryTool::new(config::config_dir().join("agent.json"));

    // Create model router from config.
    let model_router = ModelRouter::new(
        &cfg.llm.model_routing.flash_model,
        &cfg.llm.model_routing.pro_model,
    );
    let base_main_registry = default_registry()
        .register(zhongshu_core::tool::search::WebSearchTool)
        .register(zhongshu_core::tool::browser::BrowserTool)
        .register(zhongshu_core::tool::browser_automation::BrowserAutomationTool)
        .register(zhongshu_core::tool::webfetch::WebFetchTool)
        .register(zhongshu_core::tool::screenshot::ScreenshotTool)
        .register(zhongshu_core::tool::search_files::SearchFilesTool)
        .register(zhongshu_core::tool::fs::GrepTool)
        .register(zhongshu_core::tool::fs::GlobTool)
        .register(zhongshu_core::tool::fs::EditTool)
        .register(zhongshu_core::tool::automation::AutomationTool)
        .register(zhongshu_core::tool::self_test::SelfTestTool)
        .register(memory_tool.clone())
        .register(goal_tool.clone())
        .register(task_tool.clone())
        .register(suggestion_tool.clone())
        .register(memory_query_tool.clone());
    let mut main_registry = base_main_registry.clone();
    // Register equipment-provided tools into the main agent registry
    {
        let reports = r.block_on(async {
            let equipment = equipment.lock().unwrap();
            equipment.register_tools(&mut main_registry);
            equipment.register_mcp_tools(&mut main_registry).await
        });
        for report in reports {
            if let Some(error) = report.error {
                tracing::warn!("MCP server '{}' skipped: {}", report.server_id, error);
            }
        }
    }
    let controller = Arc::new(AgentController::new(
        eb.clone(),
        response_tx.clone(),
        provider.clone(),
        base_main_registry,
        main_registry,
        cfg.llm.model.clone(),
        app::SessionState::new(),
        base_system_prompt,
        system_prompt,
        config::config_dir().join("agent.json"),
        proxy.clone(),
        model_router,
        cfg.llm.model_routing.reasoning_complex.clone(),
        cfg.llm.model_routing.reasoning_agent.clone(),
        cfg.llm.max_context_tokens,
        equipment.clone(),
        core_db_path.clone(),
    ));
    controller.set_auto_evolve(cfg.agent.auto_evolve);
    controller
        .run_controller
        .set_ledger(RunLedger::new(Database::new(core_db_path.clone())));
    let checkpoint_store =
        zhongshu_core::core::checkpoint::CheckpointStore::new(Database::new(core_db_path.clone()));
    match checkpoint_store.latest_unfinished() {
        Ok(Some(checkpoint)) => match uuid::Uuid::parse_str(&checkpoint.run_id) {
            Ok(run_id) => {
                let goal = checkpoint
                    .messages
                    .iter()
                    .find(|message| message.role == zhongshu_core::agent::llm::Role::User)
                    .map(|message| message.content.as_str())
                    .unwrap_or("恢复未完成任务");
                controller
                    .run_controller
                    .restore_interrupted_run(run_id, goal);
                tracing::warn!(%run_id, "unfinished agent run is ready for explicit recovery");
            }
            Err(error) => tracing::error!(
                run_id = %checkpoint.run_id,
                %error,
                "cannot recover checkpoint with invalid run id"
            ),
        },
        Ok(None) => {}
        Err(error) => tracing::error!(%error, "failed to inspect unfinished agent checkpoints"),
    }
    let run_controller = controller.run_controller.clone();
    let inbox = Arc::new(AgentInbox::new(controller.clone()));
    inbox.start();
    services::spawn_auto_evolution(observer.clone(), controller.clone(), equipment.clone(), core_db_path.clone());
    services::spawn_runbook_to_skill(
        eb.clone(),
        llm_registry.clone(),
        core_db_path.clone(),
        equipment.clone(),
        controller.clone(),
    );
    services::spawn_policy_learner(core_db_path.clone());

    let mut task_scheduler = TaskScheduler::new(Duration::from_secs(1));

    for r in &cfg.scheduler.reminders {
        if let Some(trigger) = ReminderTrigger::from_rfc3339(&r.id, &r.message, &r.at) {
            task_scheduler.register(trigger);
            tracing::info!("registered reminder '{}' at {}", r.id, r.at);
        } else {
            tracing::warn!("failed to parse reminder '{}' at {}", r.id, r.at);
        }
    }

    for w in &cfg.scheduler.file_watches {
        let watch = FileWatchTrigger::new(&w.id, std::path::PathBuf::from(&w.path));
        task_scheduler.register(watch);
        tracing::info!("registered file watch '{}' on {}", w.id, w.path);
    }

    let task_queue = task_scheduler.queue().clone();
    let rule_queue = task_scheduler.queue().clone();
    task_scheduler.spawn();

    let base_worker_registry = default_registry()
        .register(zhongshu_core::tool::search::WebSearchTool)
        .register(zhongshu_core::tool::browser::BrowserTool)
        .register(zhongshu_core::tool::browser_automation::BrowserAutomationTool)
        .register(zhongshu_core::tool::webfetch::WebFetchTool)
        .register(zhongshu_core::tool::screenshot::ScreenshotTool)
        .register(zhongshu_core::tool::search_files::SearchFilesTool)
        .register(zhongshu_core::tool::fs::GrepTool)
        .register(zhongshu_core::tool::fs::GlobTool)
        .register(zhongshu_core::tool::fs::EditTool)
        .register(zhongshu_core::tool::automation::AutomationTool)
        .register(zhongshu_core::tool::self_test::SelfTestTool)
        .register(memory_tool.clone())
        .register(goal_tool.clone())
        .register(task_tool.clone())
        .register(suggestion_tool.clone())
        .register(memory_query_tool.clone());
    let mut worker_registry = base_worker_registry.clone();
    {
        let reports = r.block_on(async {
            let equipment = equipment.lock().unwrap();
            equipment.register_tools(&mut worker_registry);
            equipment.register_mcp_tools(&mut worker_registry).await
        });
        for report in reports {
            if let Some(error) = report.error {
                tracing::warn!(
                    "worker MCP server '{}' skipped: {}",
                    report.server_id,
                    error
                );
            }
        }
    }
    let mut worker_runtime_value = AgentRuntime::with_llm(
        provider.clone(),
        cfg.llm.model.clone(),
        worker_registry,
        AgentBudget {
            max_steps: 50,
            max_tool_calls: 100,
            per_tool_limit: 30,
            token_limit: 128_000,
            llm_timeout: Duration::from_secs(240),
            tool_timeout: Duration::from_secs(120),
        },
    );
    worker_runtime_value.ledger = Some(RunLedger::new(Database::new(core_db_path.clone())));
    worker_runtime_value.event_bus = Some((*eb).clone());
    let mut planner_runtime_value = worker_runtime_value.clone();
    let planner_model = if cfg.llm.model_routing.enabled {
        cfg.llm.model_routing.pro_model.clone()
    } else {
        cfg.llm.model.clone()
    };
    planner_runtime_value.provider = provider.change_model(&planner_model);
    planner_runtime_value.model = planner_model;
    planner_runtime_value.reasoning_effort = cfg
        .llm
        .model_routing
        .enabled
        .then(|| cfg.llm.model_routing.reasoning_agent.clone());
    let planner_runtime = Arc::new(tokio::sync::RwLock::new(planner_runtime_value));
    let worker_runtime = Arc::new(tokio::sync::RwLock::new(worker_runtime_value));

    let review_tools = vec![
        "read_file".into(),
        "list_dir".into(),
        "search_files".into(),
        "grep".into(),
        "glob".into(),
    ];
    let mut analyst_profile = AgentProfile::new(
        "analysis-employee",
        "你是 review_pipeline 中的分析员工。只读审查用户目标和当前项目，引用具体事实，提交风险与建议；不得修改文件，不得运行测试或其他验证，不得声称已经验证。验证由后续测试员工负责。",
        review_tools.clone(),
        AgentBudget::assistant_default(),
    )
    .with_specialty(
        EmployeeRole::architect(),
        vec![
            EmployeeCapability::architecture_review(),
            EmployeeCapability::contract_review(),
        ],
        "只读分析与风险定位，不负责执行验证",
    )
    .with_verification_policy(VerificationPolicy::NotRequired);
    analyst_profile.llm_model = Some(cfg.llm.model_routing.flash_model.clone());
    let mut verifier_tools = review_tools;
    verifier_tools.extend(["shell".into(), "self_test".into()]);
    let mut verifier_profile = AgentProfile::new(
        "verification-employee",
        "你是 review_pipeline 中的测试员工。基于分析员工的报告独立复核，只读检查并运行与目标直接相关的验证；不得修改文件。没有新鲜成功证据时必须明确未验证。",
        verifier_tools,
        AgentBudget::assistant_default(),
    )
    .with_specialty(
        EmployeeRole::tester(),
        vec![
            EmployeeCapability::test_design(),
            EmployeeCapability::test_execution(),
        ],
        "独立复核与新鲜验证证据",
    )
    .with_verification_policy(VerificationPolicy::Required);
    verifier_profile.llm_model = Some(cfg.llm.model_routing.flash_model.clone());
    let delegation = Arc::new(DelegationController::new(
        worker_runtime.clone(),
        analyst_profile.clone(),
        verifier_profile.clone(),
        llm_registry.clone(),
        eb.clone(),
        response_tx.clone(),
        run_controller.clone(),
    ));
    let mut organization_verifier_profile = verifier_profile.clone();
    organization_verifier_profile
        .tool_names
        .retain(|tool| tool != "shell" && tool != "self_test");
    organization_verifier_profile.verification_policy = VerificationPolicy::NotRequired;
    organization_verifier_profile.specialty.focus = "只读复核；通用组织入口不运行命令验证".into();
    let sandbox_employee_budget = || AgentBudget {
        max_steps: 12,
        max_tool_calls: 64,
        per_tool_limit: 32,
        token_limit: 16_000,
        llm_timeout: Duration::from_secs(120),
        tool_timeout: Duration::from_secs(120),
    };
    let mut implementation_profile = AgentProfile::new(
        "implementation-employee",
        "你是通用实施员工。只在用户明确开启 Mutation mode 并分配文件 scope 后修改文件；先读取相关上下文，在隔离沙箱中完成最小改动，运行与任务直接相关的轻量验证，再通过 submit_patch_proposal 提交实际差异。不得修改 scope 外文件，不得声称未执行的验证已经通过。",
        vec![
            "read_file".into(),
            "list_dir".into(),
            "search_files".into(),
            "grep".into(),
            "glob".into(),
            "write_file".into(),
            "edit".into(),
            "shell".into(),
        ],
        sandbox_employee_budget(),
    )
    .with_specialty(
        EmployeeRole::generalist(),
        Vec::new(),
        "限定 scope 的实现、修改与轻量验证",
    )
    .with_verification_policy(VerificationPolicy::Required);
    implementation_profile.llm_model = Some(cfg.llm.model_routing.flash_model.clone());
    let mut integration_profile = AgentProfile::new(
        "integration-employee",
        "你是通用集成员工。只在用户明确开启 Mutation mode 并分配文件 scope 后工作；读取共享上下文，但只修改自己的 scope。在隔离沙箱中完成最小改动和轻量验证，通过 submit_patch_proposal 提交实际差异，并清楚汇报与其他员工输出的接口假设。不得修改 scope 外文件，不得声称未执行的验证已经通过。",
        vec![
            "read_file".into(),
            "list_dir".into(),
            "search_files".into(),
            "grep".into(),
            "glob".into(),
            "write_file".into(),
            "edit".into(),
            "shell".into(),
        ],
        sandbox_employee_budget(),
    )
    .with_specialty(
        EmployeeRole::generalist(),
        Vec::new(),
        "独立 scope 的实现、集成检查与轻量验证",
    )
    .with_verification_policy(VerificationPolicy::Required);
    integration_profile.llm_model = Some(cfg.llm.model_routing.flash_model.clone());

    services::spawn_task_executor(
        eb.clone(),
        core_db_path.clone(),
        worker_runtime.clone(),
        AgentProfile::new(
            "background-task-executor",
            "你是中书的后台任务执行 worker。按给定步骤执行任务，优先使用工具获得事实证据；需要修改、验证或访问敏感资源时遵守权限和 harness 反馈。不要声称完成未验证的工作。",
            vec![],
            AgentBudget {
                max_steps: 80,
                max_tool_calls: 256,
                per_tool_limit: 128,
                token_limit: 128_000,
                llm_timeout: Duration::from_secs(240),
                tool_timeout: Duration::from_secs(120),
            },
        ),
    );

    let profile_dir = config::config_dir().join("profiles");
    let _ = std::fs::create_dir_all(&profile_dir);
    let mut worker_profiles = AgentProfile::load_dir(&profile_dir);
    if worker_profiles.is_empty() {
        tracing::info!(
            "no worker profiles in {:?}, using default task-handler",
            profile_dir
        );
        worker_profiles.push(AgentProfile::new(
            "task-handler",
            "你是一个后台任务处理助手。收到定时任务或事件后，分析任务内容并执行必要的操作。",
            vec![],
            AgentBudget::default(),
        ));
    } else {
        tracing::info!(count = worker_profiles.len(), "loaded worker profiles");
    }
    let mut organization_roster = worker_profiles.clone();
    for built_in in [
        analyst_profile,
        organization_verifier_profile,
        implementation_profile,
        integration_profile,
    ] {
        if !organization_roster
            .iter()
            .any(|profile| profile.name == built_in.name)
        {
            organization_roster.push(built_in);
        }
    }
    let org_checkpoint_store = Some(
        zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(
            zhongshu_core::core::Database::new(core_db_path.clone()),
        ),
    );
    let organization = Arc::new(OrganizationController::new(
        worker_runtime.clone(),
        planner_runtime,
        organization_roster,
        eb.clone(),
        response_tx.clone(),
        run_controller.clone(),
        org_checkpoint_store,
        std::env::current_dir().unwrap_or_default(),
        proxy.clone(),
    ));
    let auto_delegation = Arc::new(AutoDelegationController::new(
        organization.clone(),
        inbox.clone(),
        eb.clone(),
    ));

    let attention_mgr = AttentionManager::new((*eb).clone());
    let (digest_queue, _attention_handle) = attention_mgr.spawn();

    let mut source_mgr = SourceManager::new((*eb).clone());
    source_mgr.register(TimerSource::new("heartbeat", Duration::from_secs(300)));
    #[cfg(target_os = "windows")]
    source_mgr.register(DiskUsageSource::new(
        "disk-root",
        "C:\\",
        0.90,
        Duration::from_secs(3600),
    ));
    #[cfg(not(target_os = "windows"))]
    source_mgr.register(DiskUsageSource::new(
        "disk-root",
        "/",
        0.90,
        Duration::from_secs(3600),
    ));
    source_mgr.register(BatterySource::new("battery", 20, Duration::from_secs(3600)));
    let _source_handle = source_mgr.spawn();

    for profile in worker_profiles {
        TaskWorkerDispatcher::spawn(
            task_queue.clone(),
            worker_runtime.clone(),
            profile,
            eb.clone(),
        );
    }

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

    if cfg.agent.background.enabled {
        let _heartbeat_handle = Heartbeat::default().spawn();
    }

    {
        let digest_eb = (*eb).clone();
        let dq = digest_queue.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(86400));
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

    let event_log_path = config::config_dir().join("event_log.jsonl");
    EventLogger::replay(&event_log_path, &eb);
    let _event_logger = EventLogger::new(event_log_path).unwrap().spawn(&eb);

    let task_repo = TaskRepository::new(Database::new(core_db_path.clone()));
    {
        let recovered = task_repo.recover_stale_inflight(0).unwrap_or_default();
        if !recovered.is_empty() {
            tracing::info!(
                "startup: recovered {} stale inflight tasks",
                recovered.len()
            );
        }
    }
    // Recover unfinished organization tasks from crash.
    {
        let org_store = zhongshu_core::core::checkpoint::OrganizationCheckpointStore::new(
            Database::new(core_db_path.clone()),
        );
        let graph_task_ids = match org_store.list_unfinished_graphs() {
            Ok(task_ids) => task_ids,
            Err(error) => {
                tracing::error!(%error, "failed to inspect unfinished organization graphs");
                Vec::new()
            }
        };
        for task_id in &graph_task_ids {
            let recovery =
                r.block_on(DurableExecutionRunner::new(org_store.clone()).recover(task_id));
            match recovery {
                Ok(Some(recovered)) => {
                    let reason = if recovered.report.recovery_required_nodes.is_empty() {
                        "组织任务在进程退出前未完成，等待显式恢复".to_string()
                    } else {
                        format!(
                            "以下节点的外部效果未知，必须先核对：{}",
                            recovered.report.recovery_required_nodes.join(", ")
                        )
                    };
                    tracing::warn!(
                        task_id = %task_id,
                        store_version = recovered.store_version,
                        recovery_required = ?recovered.report.recovery_required_nodes,
                        "startup: retained unfinished organization graph for reconciliation"
                    );
                    eb.publish(Event::Organization(
                        zhongshu_core::event::OrganizationEvent::TaskFinished {
                            task_id: task_id.clone(),
                            status: "recovery_required".into(),
                            reason: Some(reason),
                        },
                    ));
                }
                Ok(None) => {
                    tracing::warn!(task_id = %task_id, "unfinished graph disappeared during recovery scan");
                }
                Err(error) => {
                    tracing::error!(task_id = %task_id, %error, "failed to recover organization graph");
                    eb.publish(Event::Organization(
                        zhongshu_core::event::OrganizationEvent::TaskFinished {
                            task_id: task_id.clone(),
                            status: "recovery_required".into(),
                            reason: Some(format!("恢复 checkpoint 失败：{error}")),
                        },
                    ));
                }
            }
        }
        if let Ok(unfinished) = org_store.list_unfinished() {
            for task_id in &unfinished {
                if graph_task_ids.contains(task_id) {
                    if let Err(error) = org_store.delete(task_id) {
                        tracing::warn!(%error, task_id = %task_id, "failed to remove superseded legacy organization checkpoint");
                    }
                    continue;
                }
                tracing::warn!(
                    task_id = %task_id,
                    "startup: found unfinished organization task from crash — marking as failed"
                );
                eb.publish(Event::Organization(
                    zhongshu_core::event::OrganizationEvent::TaskFinished {
                        task_id: task_id.clone(),
                        status: "cancelled".into(),
                        reason: Some("进程崩溃，组织任务未完成".into()),
                    },
                ));
                if let Err(e) = org_store.delete(task_id) {
                    tracing::warn!(error = %e, "failed to delete stale organization checkpoint");
                }
            }
        }
    }
    let runbook_store = RunbookStore::new(Database::new(core_db_path.clone()));
    let mut app = match ZhongshuApp::new(
        cfg,
        controller,
        inbox.clone(),
        delegation,
        organization,
        auto_delegation,
        eb,
        event_rx,
        response_tx,
        response_rx,
        proxy,
        r,
        task_repo,
        runbook_store,
        observer.clone(),
        equipment.clone(),
        worker_runtime.clone(),
        base_worker_registry,
        run_controller,
    ) {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("init: {e:#}");
            return;
        }
    };

    EventLoop::new().unwrap().run_app(&mut app).unwrap();
}
