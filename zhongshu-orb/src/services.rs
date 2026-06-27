use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zhongshu_core::agent::llm::{ChatCompletionRequest, LlmProvider};
use zhongshu_core::agent::llm_registry::LlmRegistry;
use zhongshu_core::agent::{AgentProfile, AgentRuntime, Worker};
use zhongshu_core::equipment::{
    parse_proposal_response, EquipmentObserver, EquipmentRegistry, EquipmentType, Manifest,
};

use crate::app::AgentController;
use zhongshu_core::core::{
    ArtifactRepository, ArtifactType, Database, GoalRepository, GoalType, MemoryCandidateStore,
    MemoryPolicy, ObservationStore, ObservationType, RunbookStore, Scheduler, StepStatus,
    SuggestionEngine, SuggestionStatus, TaskPlanner, TaskRepository, TaskStatus, TaskStep,
};
use zhongshu_core::event::{Event, EventBus, GoalEvent, SuggestionEvent, TaskEvent};
use zhongshu_core::task::Task as WorkerTask;

/// Spawn the scheduler to scan active goals and create tasks every hour.
pub fn spawn_scheduler(scheduler: Scheduler) {
    scheduler.spawn(3600);
}

/// Evaluate memory candidates every 10 minutes.
pub fn spawn_memory_evaluation(memory_policy: MemoryPolicy, registry: Arc<LlmRegistry>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let client = match registry.client_for_role("memory") {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("memory: no LLM provider: {e}");
                    continue;
                }
            };
            let m = memory_policy.clone();
            match m.evaluate_with(&*client.provider).await {
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
pub fn spawn_task_executor(
    eb: Arc<EventBus>,
    core_db_path: PathBuf,
    worker_runtime: Arc<tokio::sync::RwLock<AgentRuntime>>,
    worker_profile: AgentProfile,
) {
    tokio::spawn(async move {
        let mut rx = eb.subscribe();
        let task_repo = TaskRepository::new(Database::new(core_db_path.clone()));
        let planner = TaskPlanner::new(Database::new(core_db_path.clone()));
        let artifact_repo = ArtifactRepository::new(Database::new(core_db_path.clone()));
        let db_path = core_db_path.clone();
        let memory_candidates = MemoryCandidateStore::new(Database::new(core_db_path));
        while let Ok(event) = rx.recv().await {
            let (task_id, title) = match &event {
                Event::Task(TaskEvent::Triggered { task_id, title }) => {
                    (task_id.clone(), title.clone())
                }
                _ => continue,
            };
            let plnr = planner.clone();

            // 1. Plan the task (async LLM call)
            let provider = { worker_runtime.read().await.provider.clone() };
            let plan_steps = match plnr.plan(&task_id, &*provider).await {
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
                // Mark step running and store input
                let prompt = if all_output.is_empty() {
                    format!("任务：{title}\n当前步骤：{}\n请执行此步骤。", step.action)
                } else {
                    format!(
                        "任务：{title}\n已完成步骤：\n{}\n当前步骤：{}\n请根据已完成的结果执行此步骤。",
                        all_output, step.action,
                    )
                };
                let _ = tokio::task::spawn_blocking({
                    let trepo = trepo.clone();
                    let sid = step.id.clone();
                    let p = prompt.clone();
                    move || {
                        let _ = trepo.update_step_status(&sid, StepStatus::Running);
                        let _ = trepo.set_step_input(&sid, &p);
                    }
                })
                .await;

                let worker_task = task_step_to_worker_task(&task_id, &title, step, &prompt);
                let runtime_snapshot = { worker_runtime.read().await.clone() };
                let step_output = match Worker::execute(
                    &runtime_snapshot,
                    &worker_profile,
                    worker_task,
                    None,
                )
                .await
                {
                    Ok(report) => {
                        // Extract tool summary and verification from trace events
                        let tool_names: Vec<String> = report
                            .trace_events
                            .iter()
                            .filter_map(|e| {
                                if let zhongshu_core::harness::trace::event::HarnessEvent::ToolCall {
                                    tool_name, ..
                                } = e
                                {
                                    Some(tool_name.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        let tool_summary = if tool_names.is_empty() {
                            String::new()
                        } else {
                            tool_names.join(", ")
                        };

                        let verification_text: String = report
                            .trace_events
                            .iter()
                            .filter_map(|e| {
                                if let zhongshu_core::harness::trace::event::HarnessEvent::Verification {
                                    success, command, exit_code, ..
                                } = e
                                {
                                    let status = if *success { "通过" } else { "失败" };
                                    Some(format!(
                                        "{status} (cmd: {command}, exit: {})",
                                        exit_code.map_or("?".to_string(), |c| c.to_string())
                                    ))
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("; ");

                        let step_output = if report.findings.trim().is_empty() {
                            report.summary
                        } else {
                            report.findings
                        };

                        // Persist step output, tool summary, and verification
                        let _ = tokio::task::spawn_blocking({
                            let trepo = trepo.clone();
                            let sid = step.id.clone();
                            let out = step_output.clone();
                            let ts = tool_summary.clone();
                            let vf = verification_text.clone();
                            move || {
                                let _ = trepo.set_step_output(&sid, &out);
                                if !ts.is_empty() {
                                    let _ = trepo.set_step_tool_summary(&sid, &ts);
                                }
                                if !vf.is_empty() {
                                    let _ = trepo.set_step_verification(&sid, &vf);
                                }
                                let _ = trepo.update_step_status(&sid, StepStatus::Completed);
                            }
                        })
                        .await;

                        step_output
                    }
                    Err(e) => {
                        tracing::warn!("executor: step '{}' worker failed: {e}", step.action);
                        let err_msg = format!("Worker execution error: {e}");
                        let _ = tokio::task::spawn_blocking({
                            let trepo = trepo.clone();
                            let sid = step.id.clone();
                            let em = err_msg.clone();
                            move || {
                                let _ = trepo.set_step_error(&sid, &em);
                            }
                        })
                        .await;
                        failed = true;
                        break;
                    }
                };

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
                let _dbp = db_path.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = trepo.set_output(&tid, &out, Some("execution failed"));
                    let _ = trepo.update_status(&tid, TaskStatus::Failed);
                    let _ = _dbp;
                    tracing::warn!("executor: task '{}' failed", ttl);
                });
            } else {
                let steps_for_runbook = plan_steps.clone();
                let _dbp = db_path.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = trepo.set_output(&tid, &out, None);
                    let _ = trepo.update_status(&tid, TaskStatus::Completed);
                    // Save runbook
                    let rstore = RunbookStore::new(Database::new(_dbp));
                    let _ = rstore.save(&zhongshu_core::core::Runbook {
                        id: format!("rb-{tid}"),
                        goal: ttl.clone(),
                        conversation_id: None,
                        created_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs().to_string())
                            .unwrap_or_default(),
                        total_steps: steps_for_runbook.len(),
                        passed: steps_for_runbook
                            .len()
                            .saturating_sub(if failed { 0 } else { 0 }),
                        failed: if failed { 1 } else { 0 },
                        steps: steps_for_runbook
                            .iter()
                            .enumerate()
                            .map(|(i, s)| zhongshu_core::core::RunbookStep {
                                action: format!("step_{}", i + 1),
                                tool: "task".into(),
                                input: s.action.clone(),
                                output_status: if failed { "failed" } else { "completed" }.into(),
                                output_preview: out.chars().take(200).collect(),
                                verification: "".into(),
                            })
                            .collect(),
                    });
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

fn task_step_to_worker_task(
    task_id: &str,
    title: &str,
    step: &TaskStep,
    prompt: &str,
) -> WorkerTask {
    WorkerTask {
        id: step.id.clone(),
        source: format!("core-task:{task_id}"),
        tool: "agent".into(),
        arguments: serde_json::json!({
            "task_id": task_id,
            "title": title,
            "step_id": step.id.clone(),
            "step_order": step.step_order,
            "step_action": step.action.clone(),
            "prompt": prompt,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_step_to_worker_task_maps_step_context() {
        let step = TaskStep {
            id: "step-1".into(),
            task_id: "task-1".into(),
            step_order: 2,
            action: "collect evidence".into(),
            status: StepStatus::Pending,
            input: None,
            output: None,
            created_at: 123,
        };

        let task = task_step_to_worker_task("task-1", "Review harness", &step, "do it");

        assert_eq!(task.id, "step-1");
        assert_eq!(task.source, "core-task:task-1");
        assert_eq!(task.tool, "agent");
        assert_eq!(task.arguments["task_id"], "task-1");
        assert_eq!(task.arguments["title"], "Review harness");
        assert_eq!(task.arguments["step_id"], "step-1");
        assert_eq!(task.arguments["step_order"], 2);
        assert_eq!(task.arguments["step_action"], "collect evidence");
        assert_eq!(task.arguments["prompt"], "do it");
    }
}

/// LLM-based suggestion analysis: read recent observations every 30 min.
pub fn spawn_llm_suggestion_engine(registry: Arc<LlmRegistry>, core_db_path: PathBuf) {
    tokio::spawn(async move {
        let observation_store = ObservationStore::new(Database::new(core_db_path.clone()));
        let suggestion_engine = SuggestionEngine::new(Database::new(core_db_path));
        loop {
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
            let client = match registry.client_for_role("suggestion") {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("suggestion: no LLM provider: {e}");
                    continue;
                }
            };
            let req = ChatCompletionRequest {
                model: client.model.clone(),
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
            let response = match client.provider.chat(req).await {
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

/// Periodically check observations and ask LLM to propose new equipment.
/// Runs only when `controller.auto_evolve_enabled` is true.
pub fn spawn_auto_evolution(
    observer: Arc<Mutex<EquipmentObserver>>,
    controller: Arc<AgentController>,
    equipment: Arc<Mutex<EquipmentRegistry>>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            if !controller
                .auto_evolve_enabled
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                continue;
            }
            let prompt = match observer.lock().unwrap().equipment_proposal_prompt() {
                Some(p) => p,
                None => continue,
            };
            tracing::info!("auto_evolve: requesting equipment proposal from LLM");
            let provider = controller.provider_snapshot();
            let req = ChatCompletionRequest {
                model: provider.model_name().to_string(),
                messages: vec![zhongshu_core::agent::llm::Message::user(&prompt)],
                stream: false,
                temperature: Some(0.3),
                max_tokens: Some(2000),
                reasoning_effort: None,
                tools: None,
                tool_choice: None,
            };
            let response = match provider.chat(req).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("auto_evolve: LLM call failed: {e}");
                    continue;
                }
            };
            let text = response
                .choices
                .first()
                .map(|c| c.message.content.as_str())
                .unwrap_or("");
            let manifest = match parse_proposal_response(text) {
                Some(m) => m,
                None => {
                    tracing::info!("auto_evolve: LLM declined or returned invalid proposal");
                    continue;
                }
            };
            if !matches!(manifest.equipment_type, EquipmentType::Skill) {
                tracing::warn!(
                    "auto_evolve: unsupported equipment type for '{}'; only skill is currently installable",
                    manifest.name
                );
                continue;
            }
            // Write manifest to a temp directory for installation.
            let tmp = std::env::temp_dir().join(format!("zhongshu_evolve_{}", manifest.name));
            if let Err(e) = std::fs::create_dir_all(&tmp) {
                tracing::warn!("auto_evolve: failed to create temp dir: {e}");
                continue;
            }
            if let Err(e) = write_auto_evolve_package(
                &tmp,
                &manifest,
                &serde_json::to_string_pretty(&manifest).unwrap(),
            ) {
                tracing::warn!("auto_evolve: failed to write temp package: {e}");
                let _ = std::fs::remove_dir_all(&tmp);
                continue;
            }
            // Install via registry.
            let name = manifest.name.clone();
            match equipment.lock().unwrap().install_from(&tmp) {
                Ok(id) => {
                    tracing::info!("auto_evolve: installed '{}'", id);
                    controller.refresh_skill_prompts();
                }
                Err(e) => {
                    tracing::warn!("auto_evolve: install failed for '{name}': {e}");
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
    });
}

fn write_auto_evolve_package(
    dir: &std::path::Path,
    manifest: &Manifest,
    manifest_json: &str,
) -> std::io::Result<()> {
    std::fs::write(dir.join("manifest.json"), manifest_json)?;
    if matches!(manifest.equipment_type, EquipmentType::Skill) {
        std::fs::write(dir.join("prompt.md"), auto_evolve_prompt_md(manifest))?;
    }
    Ok(())
}

/// Extract reusable skills from completed runbooks.
/// Listens for TaskEvent::Completed, loads the runbook, and asks the LLM
/// to generate a skill manifest for installation.
pub fn spawn_runbook_to_skill(
    eb: Arc<EventBus>,
    registry: Arc<LlmRegistry>,
    core_db_path: PathBuf,
    equipment: Arc<Mutex<EquipmentRegistry>>,
    controller: Arc<AgentController>,
) {
    tokio::spawn(async move {
        let mut rx = eb.subscribe();
        while let Ok(event) = rx.recv().await {
            let task_id = match &event {
                Event::Task(TaskEvent::Completed { task_id, .. }) => task_id.clone(),
                _ => continue,
            };
            let rb_id = format!("rb-{task_id}");
            let rb_store = RunbookStore::new(Database::new(core_db_path.clone()));
            let runbooks = match rb_store.list() {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("runbook2skill: list failed: {e}");
                    continue;
                }
            };
            let runbook = match runbooks.into_iter().find(|rb| rb.id == rb_id) {
                Some(rb) => rb,
                None => continue,
            };
            // Skip trivial runbooks (too few steps to extract meaningful skill).
            if runbook.steps.len() < 2 {
                continue;
            }
            // Skip if any step failed.
            if runbook.failed > 0 {
                continue;
            }
            let client = match registry.client_for_role("worker") {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("runbook2skill: no LLM provider: {e}");
                    continue;
                }
            };
            let steps_text: String = runbook
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. [{}] {} — {}", i + 1, s.tool, s.action, s.input))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
                r#"根据以下 Runbook（任务执行记录），提取一个可复用的技能。

Runbook 目标：{goal}

执行步骤：
{steps}

请分析这些步骤，生成一个技能 Manifest JSON。要求：
- name：简短英文技能名（kebab-case，如 "data-analysis"）
- version："1.0.0"
- description：中文描述技能用途
- type："skill"
- tools：用到的工具列表（如 ["shell", "grep"]）
- entry：使用默认值

只返回 JSON，放在 ```json 代码块中，不要包含其他文字。
"#,
                goal = runbook.goal,
                steps = steps_text,
            );
            let req = ChatCompletionRequest {
                model: client.model.clone(),
                messages: vec![zhongshu_core::agent::llm::Message::user(&prompt)],
                stream: false,
                temperature: Some(0.3),
                max_tokens: Some(1500),
                reasoning_effort: None,
                tools: None,
                tool_choice: None,
            };
            let response = match client.provider.chat(req).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("runbook2skill: LLM call failed: {e}");
                    continue;
                }
            };
            let text = response
                .choices
                .first()
                .map(|c| c.message.content.as_str())
                .unwrap_or("");
            let manifest = match parse_proposal_response(text) {
                Some(m) => m,
                None => {
                    tracing::info!("runbook2skill: LLM declined or invalid proposal");
                    continue;
                }
            };
            if !matches!(manifest.equipment_type, EquipmentType::Skill) {
                tracing::warn!("runbook2skill: non-skill type, skipping");
                continue;
            }
            // Write package to temp dir and install.
            let tmp = std::env::temp_dir().join(format!("zhongshu_rb2skill_{}", manifest.name));
            if let Err(e) = std::fs::create_dir_all(&tmp) {
                tracing::warn!("runbook2skill: failed to create temp dir: {e}");
                continue;
            }
            let manifest_json = match serde_json::to_string_pretty(&manifest) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!("runbook2skill: serialize failed: {e}");
                    let _ = std::fs::remove_dir_all(&tmp);
                    continue;
                }
            };
            if let Err(e) = write_auto_evolve_package(&tmp, &manifest, &manifest_json) {
                tracing::warn!("runbook2skill: write package failed: {e}");
                let _ = std::fs::remove_dir_all(&tmp);
                continue;
            }
            let name = manifest.name.clone();
            match equipment.lock().unwrap().install_from(&tmp) {
                Ok(id) => {
                    tracing::info!("runbook2skill: installed skill '{}' from runbook", id);
                    controller.refresh_skill_prompts();
                }
                Err(e) => {
                    tracing::warn!("runbook2skill: install failed for '{name}': {e}");
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
    });
}

fn auto_evolve_prompt_md(manifest: &Manifest) -> String {
    let tools = if manifest.tools.is_empty() {
        "无特定工具".to_string()
    } else {
        manifest.tools.join(", ")
    };
    format!(
        r#"# 装备：{name}

{description}

当用户请求与该装备能力匹配时，优先按本装备处理。使用工具前遵守中书的权限关卡；不要自行扩大权限。

可用/相关工具：{tools}
"#,
        name = manifest.name,
        description = manifest.description,
        tools = tools
    )
}
