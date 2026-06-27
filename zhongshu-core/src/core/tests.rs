#[cfg(test)]
mod tests {
    use crate::agent::llm::{
        ChatCompletionRequest, ChatCompletionResponse, FinalChoice, LlmProvider, Message, Role,
        StreamEvent,
    };
    use crate::core::db::Database;
    use crate::core::goal::GoalRepository;
    use crate::core::memory::MemoryCandidateStore;
    use crate::core::memory::MemoryPolicy;
    use crate::core::models::*;
    use crate::core::observation::ObservationStore;
    use crate::core::scheduler::Scheduler;
    use crate::core::suggestion::SuggestionEngine;
    use crate::core::task::TaskPlanner;
    use crate::core::task::TaskRepository;
    use async_trait::async_trait;

    struct MockPlannerProvider;
    #[async_trait]
    impl LlmProvider for MockPlannerProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message {
                        role: Role::Assistant,
                        content: r#"["收集项目数据","撰写报告大纲","编写详细内容","审阅和修改"]"#
                            .into(),
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }
        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn change_model(&self, _model: &str) -> std::sync::Arc<dyn LlmProvider> {
            std::sync::Arc::new(MockPlannerProvider)
        }
        fn model_name(&self) -> &str {
            "mock"
        }
    }

    fn test_db() -> Database {
        let dir = std::env::temp_dir();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!("zhongshu_core_test_{ts}.db"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path.with_extension("db-wal"));
        let _ = std::fs::remove_file(&path.with_extension("db-shm"));
        let db = Database::new(path);
        db.migrate().expect("migration");
        db
    }

    // ── Database ─────────────────────────────────────────────────────

    #[test]
    fn migration_creates_all_tables() {
        let db = test_db();
        let conn = db.conn().unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for t in &[
            "observations",
            "suggestions",
            "goals",
            "tasks",
            "task_steps",
            "task_runs",
            "artifacts",
            "task_artifacts",
            "memory_candidates",
            "memories",
            "events",
        ] {
            assert!(tables.contains(&t.to_string()), "table {t} not found");
        }
    }

    // ── Goal Repository ──────────────────────────────────────────────

    #[test]
    fn goal_create_and_list() {
        let db = test_db();
        let repo = GoalRepository::new(db);
        let g = repo
            .create(
                "learn rust",
                Some("study rust this month"),
                GoalType::Ongoing,
            )
            .unwrap();
        assert!(g.id.starts_with("goal-"));
        assert_eq!(g.title, "learn rust");
        assert_eq!(g.status, GoalStatus::Active);

        let goals = repo.list_active().unwrap();
        assert_eq!(goals.len(), 1);
    }

    #[test]
    fn goal_create_twice_both_active() {
        let db = test_db();
        let repo = GoalRepository::new(db);
        repo.create("g1", None, GoalType::OneShot).unwrap();
        repo.create("g2", None, GoalType::Recurring).unwrap();
        assert_eq!(repo.list_active().unwrap().len(), 2);
        assert_eq!(repo.list_all().unwrap().len(), 2);
    }

    #[test]
    fn goal_pause_and_complete() {
        let db = test_db();
        let repo = GoalRepository::new(db);
        let g = repo.create("test", None, GoalType::OneShot).unwrap();

        assert!(repo.update_status(&g.id, GoalStatus::Paused).unwrap());
        assert_eq!(repo.get(&g.id).unwrap().unwrap().status, GoalStatus::Paused);

        assert!(repo.update_status(&g.id, GoalStatus::Completed).unwrap());
        assert_eq!(
            repo.get(&g.id).unwrap().unwrap().status,
            GoalStatus::Completed
        );

        assert_eq!(repo.list_active().unwrap().len(), 0);
    }

    #[test]
    fn goal_get_nonexistent() {
        let db = test_db();
        let repo = GoalRepository::new(db);
        assert!(repo.get("goal-nope").unwrap().is_none());
    }

    // ── Task Repository ──────────────────────────────────────────────

    #[test]
    fn task_create_and_list() {
        let db = test_db();
        let repo = TaskRepository::new(db);
        let t = repo.create(None, "write docs").unwrap();
        assert!(t.id.starts_with("task-"));
        assert_eq!(t.status, TaskStatus::Pending);

        let pending = repo.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn task_create_with_goal() {
        let db = test_db();
        let grepo = GoalRepository::new(db.clone());
        let trepo = TaskRepository::new(db);
        let g = grepo.create("release v2", None, GoalType::OneShot).unwrap();
        let t = trepo.create(Some(&g.id), "prepare release notes").unwrap();
        assert_eq!(t.goal_id, Some(g.id));
    }

    #[test]
    fn task_status_transitions() {
        let db = test_db();
        let repo = TaskRepository::new(db);
        let t = repo.create(None, "test").unwrap();

        assert!(repo.update_status(&t.id, TaskStatus::Running).unwrap());
        assert_eq!(
            repo.get(&t.id).unwrap().unwrap().status,
            TaskStatus::Running
        );

        assert!(repo.update_status(&t.id, TaskStatus::Completed).unwrap());
        assert_eq!(
            repo.get(&t.id).unwrap().unwrap().status,
            TaskStatus::Completed
        );
    }

    #[test]
    fn task_list_open_includes_running_excludes_finished() {
        let db = test_db();
        let repo = TaskRepository::new(db);
        let pending = repo.create(None, "pending").unwrap();
        let running = repo.create(None, "running").unwrap();
        let completed = repo.create(None, "completed").unwrap();

        assert!(repo
            .update_status(&running.id, TaskStatus::Running)
            .unwrap());
        assert!(repo
            .update_status(&completed.id, TaskStatus::Completed)
            .unwrap());

        let open = repo.list_open().unwrap();
        let ids: Vec<&str> = open.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&pending.id.as_str()));
        assert!(ids.contains(&running.id.as_str()));
        assert!(!ids.contains(&completed.id.as_str()));
    }

    #[test]
    fn task_steps() {
        let db = test_db();
        let repo = TaskRepository::new(db);
        let t = repo.create(None, "step test").unwrap();

        let s1 = repo.add_step(&t.id, 0, "step one").unwrap();
        let s2 = repo.add_step(&t.id, 1, "step two").unwrap();
        assert_eq!(s1.step_order, 0);
        assert_eq!(s2.step_order, 1);

        let steps = repo.list_steps(&t.id).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].action, "step one");
    }

    // ── Task Planner ─────────────────────────────────────────────────

    #[tokio::test]
    async fn planner_generates_steps() {
        let db = test_db();
        let repo = TaskRepository::new(db.clone());
        let planner = TaskPlanner::new(db);
        let t = repo.create(None, "写项目总结报告").unwrap();
        let steps = planner.plan(&t.id, &MockPlannerProvider).await.unwrap();
        assert!(steps.len() >= 3, "expected >=3 steps, got {}", steps.len());
        assert_eq!(
            repo.get(&t.id).unwrap().unwrap().status,
            TaskStatus::Planning
        );
    }

    // ── Observation Store ────────────────────────────────────────────

    #[test]
    fn observation_insert_and_recent() {
        let db = test_db();
        let store = ObservationStore::new(db);
        store
            .insert(ObservationType::UserMessage, "hello", Some("test"), None)
            .unwrap();
        store
            .insert(ObservationType::ToolResult, "ok", None, None)
            .unwrap();

        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        // Both inserted same second; order depends on insert timing.  Just check both exist.
        let types: Vec<_> = recent.iter().map(|o| o.type_).collect();
        assert!(types.contains(&ObservationType::UserMessage));
        assert!(types.contains(&ObservationType::ToolResult));
    }

    #[test]
    fn observation_by_type() {
        let db = test_db();
        let store = ObservationStore::new(db);
        store
            .insert(ObservationType::UserMessage, "hi", None, None)
            .unwrap();
        store
            .insert(ObservationType::AgentAction, "thinking", None, None)
            .unwrap();

        let msgs = store.by_type("user_message", 10).unwrap();
        assert_eq!(msgs.len(), 1);
    }

    // ── Suggestion Engine ────────────────────────────────────────────

    #[test]
    fn suggestion_insert_and_list() {
        let db = test_db();
        let engine = SuggestionEngine::new(db);
        engine
            .insert("suggest something", Some("goal"), 0.8, None)
            .unwrap();
        let pending = engine.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].confidence, 0.8);
    }

    #[test]
    fn suggestion_accept_reject() {
        let db = test_db();
        let engine = SuggestionEngine::new(db);
        let s = engine.insert("test", None, 0.5, None).unwrap();
        assert!(engine
            .update_status(&s.id, &SuggestionStatus::Accepted)
            .unwrap());
        assert!(engine.list_pending().unwrap().is_empty());
    }

    // ── Memory Pipeline ──────────────────────────────────────────────

    #[test]
    fn memory_candidate_promoted_to_memory() {
        let db = test_db();
        let candidates = MemoryCandidateStore::new(db.clone());
        let policy = MemoryPolicy::new(db);

        candidates
            .insert(
                "user likes dark mode",
                Some("preference"),
                0.9,
                Some("agent"),
                None,
            )
            .unwrap();
        let accepted = policy.evaluate().unwrap();
        assert_eq!(
            accepted.len(),
            1,
            "should promote high-confidence candidate"
        );
        assert_eq!(accepted[0].content, "user likes dark mode");
    }

    #[test]
    fn memory_low_confidence_not_promoted() {
        let db = test_db();
        let candidates = MemoryCandidateStore::new(db.clone());
        let policy = MemoryPolicy::new(db);

        candidates
            .insert(
                "maybe something",
                Some("preference"),
                0.3,
                Some("agent"),
                None,
            )
            .unwrap();
        let accepted = policy.evaluate().unwrap();
        assert_eq!(accepted.len(), 0, "low confidence should not be promoted");
    }

    #[test]
    fn memory_search() {
        let db = test_db();
        let candidates = MemoryCandidateStore::new(db.clone());
        let policy = MemoryPolicy::new(db);

        candidates
            .insert("user prefers terminal", Some("preference"), 0.9, None, None)
            .unwrap();
        candidates
            .insert("user likes vscode", Some("preference"), 0.9, None, None)
            .unwrap();
        policy.evaluate().unwrap();

        let results = policy.search("terminal", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("terminal"));

        let all = policy.list_memories(10).unwrap();
        assert_eq!(all.len(), 2);
    }

    // ── Scheduler ────────────────────────────────────────────────────

    #[test]
    fn scheduler_creates_tasks_for_one_shot_goal() {
        let db = test_db();
        let grepo = GoalRepository::new(db.clone());
        let _trepo = TaskRepository::new(db.clone());
        let scheduler = Scheduler::new(db);

        grepo
            .create("write a script", None, GoalType::OneShot)
            .unwrap();
        let ids = scheduler.tick();
        assert_eq!(ids.len(), 1, "should create one task for one-shot goal");
    }

    #[test]
    fn scheduler_does_not_duplicate_tasks() {
        let db = test_db();
        let grepo = GoalRepository::new(db.clone());
        let trepo = TaskRepository::new(db.clone());
        let scheduler = Scheduler::new(db);

        grepo.create("fix bug", None, GoalType::OneShot).unwrap();
        assert_eq!(scheduler.tick().len(), 1);
        // Second tick should not create another task
        assert_eq!(scheduler.tick().len(), 0);
        // Getting the task and marking it completed should allow a new one
        let tasks = trepo.list_pending().unwrap();
        assert_eq!(tasks.len(), 1);
    }

    // ── Task Step Lifecycle (Phase 4) ─────────────────────────────────

    #[test]
    fn step_lifecycle_full() {
        let db = test_db();
        let trepo = TaskRepository::new(db);

        let task = trepo.create(None, "test task").unwrap();
        let step = trepo.add_step(&task.id, 0, "first step").unwrap();

        assert_eq!(step.status, StepStatus::Pending);
        assert!(step.started_at.is_none());
        assert!(step.finished_at.is_none());
        assert!(step.error.is_none());

        // Mark running — sets started_at
        trepo.update_step_status(&step.id, StepStatus::Running).unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].status, StepStatus::Running);
        assert!(steps[0].started_at.is_some());
        assert!(steps[0].finished_at.is_none());

        // Set input
        trepo.set_step_input(&step.id, "do the thing").unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].input.as_deref(), Some("do the thing"));

        // Complete — sets finished_at
        trepo.update_step_status(&step.id, StepStatus::Completed).unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].status, StepStatus::Completed);
        assert!(steps[0].finished_at.is_some());

        // Tool summary
        trepo.set_step_tool_summary(&step.id, "read, grep, edit").unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].tool_summary.as_deref(), Some("read, grep, edit"));

        // Verification
        trepo.set_step_verification(&step.id, "通过: all tests pass").unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].verification.as_deref(), Some("通过: all tests pass"));
    }

    #[test]
    fn step_error_sets_failed_status() {
        let db = test_db();
        let trepo = TaskRepository::new(db);

        let task = trepo.create(None, "fail task").unwrap();
        let step = trepo.add_step(&task.id, 0, "risky step").unwrap();

        // Running
        trepo.update_step_status(&step.id, StepStatus::Running).unwrap();

        // Error — set_step_error sets status to Failed automatically
        trepo.set_step_error(&step.id, "worker crashed").unwrap();
        let steps = trepo.list_steps(&task.id).unwrap();
        assert_eq!(steps[0].status, StepStatus::Failed);
        assert_eq!(steps[0].error.as_deref(), Some("worker crashed"));
        assert!(steps[0].finished_at.is_some());
    }

    #[test]
    fn step_new_statuses_roundtrip() {
        let db = test_db();
        let trepo = TaskRepository::new(db);

        let task = trepo.create(None, "statuses").unwrap();

        for (order, status) in [StepStatus::ToolBlocked, StepStatus::VerificationFailed].iter().enumerate() {
            let step = trepo.add_step(&task.id, order as i32, "test").unwrap();
            trepo.update_step_status(&step.id, *status).unwrap();
            let steps = trepo.list_steps(&task.id).unwrap();
            assert_eq!(steps[order].status, *status);
        }
    }
}
