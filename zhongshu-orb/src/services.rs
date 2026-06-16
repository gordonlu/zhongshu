use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zhongshu_core::agent::llm::{ChatCompletionRequest, LlmProvider, OpenAiProvider};
use zhongshu_core::core::{
    ArtifactRepository, ArtifactType, Database, GoalRepository, GoalType, MemoryCandidateStore,
    MemoryPolicy, ObservationStore, ObservationType, Scheduler, StepStatus, SuggestionEngine,
    SuggestionStatus, TaskPlanner, TaskRepository, TaskStatus,
};
use zhongshu_core::event::{Event, EventBus, GoalEvent, SuggestionEvent, TaskEvent};

/// Spawn the scheduler to scan active goals and create tasks every hour.
pub fn spawn_scheduler(scheduler: Scheduler) {
    scheduler.spawn(3600);
}

/// Evaluate memory candidates every 10 minutes.
pub fn spawn_memory_evaluation(memory_policy: MemoryPolicy, provider: OpenAiProvider) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let m = memory_policy.clone();
            match m.evaluate_with(&provider).await {
                Ok(accepted) => {
                    if !accepted.is_empty() {
                        tracing::info!(
                            "memory: promoted {} candidates to memories",
                            accepted.len()
                        );
                    }
                }
                Err(e) => tracing::warn!("memory evaluate: {e}"),
            }
        }
    });
}

/// Run observation pattern analysis and cleanup every 30 minutes.
pub fn spawn_suggestion_analysis(
    observation_store: ObservationStore,
    suggestion_engine: SuggestionEngine,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1800)).await;
            let e = suggestion_engine.clone();
            let o = observation_store.clone();
            tokio::task::spawn_blocking(move || {
                if let Ok(n) = o.cleanup_expired() {
                    if n > 0 {
                        tracing::info!("observations: cleaned {n} expired");
                    }
                }
                match e.analyze() {
                    Ok(sugs) => {
                        if !sugs.is_empty() {
                            tracing::info!(
                                "suggestions: generated {} from pattern analysis",
                                sugs.len()
                            );
                        }
                    }
                    Err(err) => tracing::warn!("suggestion analyze: {err}"),
                }
            });
        }
    });
}

/// Feed EventBus Agent/Tool events into the observation store.
pub fn spawn_event_observation_feed(eb: Arc<EventBus>, observation_store: ObservationStore) {
    tokio::spawn(async move {
        let mut rx = eb.subscribe();
        let obs = observation_store;
        while let Ok(event) = rx.recv().await {
            let (type_, content) = match &event {
                Event::Agent(e) => (ObservationType::AgentAction, format!("{:?}", e)),
                Event::Tool(e) => (ObservationType::ToolResult, format!("{:?}", e)),
                _ => continue,
            };
            let obs = obs.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = obs.insert(type_, &content, Some("eventbus"), None) {
                    tracing::debug!("observation insert: {e}");
                }
            });
        }
    });
}

/// Event-driven workflow: suggestion accepted → create goal → create task.
pub fn spawn_event_workflow(eb: Arc<EventBus>, core_db_path: PathBuf) {
    tokio::spawn(async move {
        let mut rx = eb.subscribe();
        let goal_repo = GoalRepository::new(Database::new(core_db_path.clone()));
        let task_repo = TaskRepository::new(Database::new(core_db_path));
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Suggestion(SuggestionEvent::Accepted { content, .. }) => {
                    let repo = goal_repo.clone();
                    let trepo = task_repo.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Ok(goal) = repo.create(&content, None, GoalType::OneShot) {
                            tracing::info!(
                                "event: created goal '{}' from accepted suggestion",
                                goal.title
                            );
                            if let Ok(ref t) = trepo.create(Some(&goal.id), &goal.title) {
                                tracing::info!("event: created task '{}' from new goal", t.title);
                            }
                        }
                    });
                }
                Event::Goal(GoalEvent::Created { goal_id, .. }) => {
                    let trepo = task_repo.clone();
                    tokio::task::spawn_blocking(move || {
                        if trepo.create(Some(&goal_id), "执行目标").is_ok() {
                            tracing::info!("event: created task from goal {}", goal_id);
                        }
                    });
                }
                _ => {}
            }
        }
    });
}

/// Listen for Task::Triggered → plan steps → run LLM per step → save output + artifact + memory.
pub fn spawn_task_executor(eb: Arc<EventBus>, provider: OpenAiProvider, core_db_path: PathBuf) {
    tokio::spawn(async move {
        let mut rx = eb.subscribe();
        let task_repo = TaskRepository::new(Database::new(core_db_path.clone()));
        let planner = TaskPlanner::new(Database::new(core_db_path.clone()));
        let artifact_repo = ArtifactRepository::new(Database::new(core_db_path.clone()));
        let memory_candidates = MemoryCandidateStore::new(Database::new(core_db_path));
        let p = provider;
        while let Ok(event) = rx.recv().await {
            let (task_id, title) = match &event {
                Event::Task(TaskEvent::Triggered { task_id, title }) => {
                    (task_id.clone(), title.clone())
                }
                _ => continue,
            };
            let plnr = planner.clone();

            // 1. Plan the task (async LLM call)
            let plan_steps = match plnr.plan(&task_id, &p).await {
                Ok(steps) if !steps.is_empty() => steps,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("executor: plan failed: {e}");
                    continue;
                }
            };

            // 2. Execute steps one by one
            let trepo = task_repo.clone();
            let r#in = tokio::task::spawn_blocking({
                let trepo = trepo.clone();
                let tid = task_id.clone();
                move || trepo.update_status(&tid, TaskStatus::Running)
            })
            .await;
            if let Err(e) = r#in {
                tracing::warn!("executor: update_status Running failed: {e}");
                continue;
            }
            if let Ok(Err(e)) = r#in {
                tracing::warn!("executor: set Running failed: {e}");
                continue;
            }

            let mut all_output = String::new();
            let mut failed = false;

            for step in &plan_steps {
                // Mark step running
                let _ = tokio::task::spawn_blocking({
                    let trepo = trepo.clone();
                    let sid = step.id.clone();
                    move || trepo.update_step_status(&sid, StepStatus::Running)
                })
                .await;

                let prompt = if all_output.is_empty() {
                    format!("任务：{title}\n当前步骤：{}\n请执行此步骤。", step.action)
                } else {
                    format!(
                        "任务：{title}\n已完成步骤：\n{}\n当前步骤：{}\n请根据已完成的结果执行此步骤。",
                        all_output, step.action,
                    )
                };

                let req = ChatCompletionRequest {
                    model: "deepseek-chat".into(),
                    messages: vec![zhongshu_core::agent::llm::Message {
                        role: zhongshu_core::agent::llm::Role::User,
                        content: prompt,
                        tool_calls: None,
                        tool_call_id: None,
                    }],
                    tools: None,
                    tool_choice: None,
                    stream: false,
                    temperature: None,
                    max_tokens: Some(2000),
                    reasoning_effort: None,
                };
                let step_output = match p.chat(req).await {
                    Ok(r) => r
                        .choices
                        .into_iter()
                        .next()
                        .map(|c| c.message.content)
                        .unwrap_or_default(),
                    Err(e) => {
                        tracing::warn!("executor: step '{}' LLM call failed: {e}", step.action);
                        let _ = tokio::task::spawn_blocking({
                            let trepo = trepo.clone();
                            let sid = step.id.clone();
                            move || trepo.update_step_status(&sid, StepStatus::Failed)
                        })
                        .await;
                        failed = true;
                        break;
                    }
                };

                // Save step output
                let _ = tokio::task::spawn_blocking({
                    let trepo = trepo.clone();
                    let sid = step.id.clone();
                    let out = step_output.clone();
                    move || {
                        let _ = trepo.set_step_output(&sid, &out);
                        let _ = trepo.update_step_status(&sid, StepStatus::Completed);
                    }
                })
                .await;

                if !all_output.is_empty() {
                    all_output.push('\n');
                }
                all_output.push_str(&format!("步骤 {}: {}", step.step_order + 1, step_output));
            }

            let trepo = task_repo.clone();
            let arepo = artifact_repo.clone();
            let mc = memory_candidates.clone();
            let ebus = eb.clone();
            let tid = task_id.clone();
            let ttl = title.clone();
            let out = all_output.clone();

            if failed {
                tokio::task::spawn_blocking(move || {
                    let _ = trepo.set_output(&tid, &out, Some("execution failed"));
                    let _ = trepo.update_status(&tid, TaskStatus::Failed);
                    tracing::warn!("executor: task '{}' failed", ttl);
                });
            } else {
                tokio::task::spawn_blocking(move || {
                    let _ = trepo.set_output(&tid, &out, None);
                    let _ = trepo.update_status(&tid, TaskStatus::Completed);
                    if let Ok(a) = arepo.insert(
                        ArtifactType::Report,
                        Some(&ttl),
                        None,
                        Some(&out.chars().take(500).collect::<String>()),
                    ) {
                        let _ = arepo.link(&tid, &a.id, "output");
                    }
                    let summary = format!(
                        "完成任务「{}」: {}",
                        ttl,
                        out.chars().take(200).collect::<String>()
                    );
                    let _ = mc.insert(&summary, Some("procedure"), 0.6, Some("task"), Some(&tid));
                    ebus.publish(Event::Task(TaskEvent::Completed {
                        task_id: tid.clone(),
                        title: ttl.clone(),
                        output: out.clone(),
                    }));
                    tracing::info!("executor: completed task '{}'", ttl);
                });
            }
        }
    });
}

/// LLM-based suggestion analysis: read recent observations every 30 min.
pub fn spawn_llm_suggestion_engine(provider: OpenAiProvider, core_db_path: PathBuf) {
    tokio::spawn(async move {
        let observation_store = ObservationStore::new(Database::new(core_db_path.clone()));
        let suggestion_engine = SuggestionEngine::new(Database::new(core_db_path));
        let p = provider;
        loop {
            use zhongshu_core::agent::llm::LlmProvider;
            tokio::time::sleep(Duration::from_secs(1800)).await;
            let recent_obs = tokio::task::spawn_blocking({
                let o = observation_store.clone();
                move || o.recent(30).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            if recent_obs.is_empty() {
                continue;
            }
            let obs_text: Vec<String> = recent_obs
                .iter()
                .map(|o| {
                    format!(
                        "[{}] {}",
                        o.type_.as_str(),
                        o.content.chars().take(200).collect::<String>()
                    )
                })
                .collect();
            let prompt = format!(
                "根据以下观察记录，判断是否有值得关注的事情。\
                 如果有，用 JSON 数组格式返回建议，每个包含 type 和 content 字段。\
                 如果没有值得关注的，返回空数组 [].\n\n观察:\n{}",
                obs_text.join("\n"),
            );
            let req = ChatCompletionRequest {
                model: "deepseek-chat".into(),
                messages: vec![zhongshu_core::agent::llm::Message {
                    role: zhongshu_core::agent::llm::Role::User,
                    content: prompt,
                    tool_calls: None,
                    tool_call_id: None,
                }],
                tools: None,
                tool_choice: None,
                stream: false,
                temperature: Some(0.3),
                max_tokens: Some(1000),
                reasoning_effort: None,
            };
            let response = match p.chat(req).await {
                Ok(r) => r
                    .choices
                    .into_iter()
                    .next()
                    .map(|c| c.message.content)
                    .unwrap_or_default(),
                Err(e) => {
                    tracing::warn!("suggestion LLM: {e}");
                    continue;
                }
            };
            let suggestions: Vec<serde_json::Value> = match serde_json::from_str(&response) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "suggestion LLM: JSON parse failed: {e}. response={}",
                        response.chars().take(300).collect::<String>()
                    );
                    Vec::new()
                }
            };
            let e = suggestion_engine.clone();
            tokio::task::spawn_blocking(move || {
                for s in &suggestions {
                    let type_ = s.get("type").and_then(|v| v.as_str());
                    let content = s.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    if !content.is_empty() {
                        if let Err(err) = e.insert(content, type_, 0.5, None) {
                            tracing::warn!("suggestion insert: {err}");
                        }
                    }
                }
            });
        }
    });
}

/// Compensate for EventBus lag/drop: periodically scan DB for events
/// that should have triggered workflow actions but may have been lost.
///
/// - Accepted suggestions without a matching goal → create goal + task.
/// - Stale pending tasks (never picked up by executor) → re-publish Triggered.
pub fn spawn_compensation(eb: Arc<EventBus>, core_db_path: PathBuf) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            let path = core_db_path.clone();
            let bus = eb.clone();

            tokio::task::spawn_blocking(move || {
                let suggestion_engine = SuggestionEngine::new(Database::new(path.clone()));
                let goal_repo = GoalRepository::new(Database::new(path.clone()));
                let task_repo = TaskRepository::new(Database::new(path.clone()));

                // 1. Accepted suggestions → create missing goals/tasks.
                if let Ok(accepted) = suggestion_engine.list_by_status(&SuggestionStatus::Accepted)
                {
                    for sug in &accepted {
                        // Skip if a goal with the same title already exists.
                        if let Ok(Some(_)) = goal_repo.find_by_title(&sug.content) {
                            continue;
                        }
                        if let Ok(goal) = goal_repo.create(&sug.content, None, GoalType::OneShot) {
                            tracing::info!(
                                "compensation: created goal '{}' from accepted suggestion",
                                goal.title
                            );
                            if let Ok(ref t) = task_repo.create(Some(&goal.id), &goal.title) {
                                tracing::info!(
                                    "compensation: created task '{}' from new goal",
                                    t.title
                                );
                                let _ = task_repo.update_status(&t.id, TaskStatus::Pending);
                                // Publish Triggered so the executor picks it up.
                                bus.publish(Event::Task(TaskEvent::Triggered {
                                    task_id: t.id.clone(),
                                    title: t.title.clone(),
                                }));
                            }
                        }
                    }
                }

                // 2. Stale pending tasks → re-publish Triggered.
                if let Ok(stale) = task_repo.list_stale_pending(300) {
                    for task in &stale {
                        tracing::info!(
                            "compensation: re-publishing Triggered for stale task '{}'",
                            task.title
                        );
                        bus.publish(Event::Task(TaskEvent::Triggered {
                            task_id: task.id.clone(),
                            title: task.title.clone(),
                        }));
                    }
                }
            });
        }
    });
}
