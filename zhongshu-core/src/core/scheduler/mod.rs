use std::sync::Arc;
use std::time::Duration;

use crate::core::db::Database;
use crate::core::goal::GoalRepository;
use crate::core::models::*;
use crate::core::task::TaskRepository;
use crate::event::{Event, EventBus, TaskEvent};

#[derive(Clone)]
pub struct Scheduler {
    goal_repo: GoalRepository,
    task_repo: TaskRepository,
    eb: Option<Arc<EventBus>>,
}

impl Scheduler {
    pub fn new(db: Database) -> Self {
        Scheduler {
            goal_repo: GoalRepository::new(db.clone()),
            task_repo: TaskRepository::new(db),
            eb: None,
        }
    }

    pub fn with_event_bus(mut self, eb: Arc<EventBus>) -> Self {
        self.eb = Some(eb);
        self
    }

    /// Returns IDs of newly created tasks.
    pub fn tick(&self) -> Vec<String> {
        let goals = match self.goal_repo.list_active() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("scheduler: failed to list goals: {e}");
                return vec![];
            }
        };

        let mut created = Vec::new();
        for goal in &goals {
            let task = match goal.goal_type {
                GoalType::OneShot => {
                    if self.has_any_task(&goal.id) {
                        continue;
                    }
                    self.task_repo.create(Some(&goal.id), &goal.title)
                }
                GoalType::Recurring => {
                    if self.has_active_or_pending_task(&goal.id) {
                        continue;
                    }
                    let title = format!(
                        "{} ({})",
                        goal.title,
                        chrono::Local::now().format("%Y-%m-%d")
                    );
                    self.task_repo.create(Some(&goal.id), &title)
                }
                GoalType::Ongoing => {
                    // Only create a new task if there is no task at all
                    // (pending, running OR completed) for this goal.
                    if self.has_any_task(&goal.id) {
                        continue;
                    }
                    self.task_repo.create(Some(&goal.id), &goal.title)
                }
            };
            if let Ok(task) = task {
                tracing::info!(
                    "scheduler: created task '{}' for goal '{}'",
                    task.title,
                    goal.id
                );
                if let Some(eb) = &self.eb {
                    eb.publish(Event::Task(TaskEvent::Triggered {
                        task_id: task.id.clone(),
                        title: task.title,
                    }));
                }
                created.push(task.id);
            }
        }
        created
    }

    fn has_any_task(&self, goal_id: &str) -> bool {
        self.task_repo
            .list_by_goal(goal_id)
            .ok()
            .map_or(false, |tasks| !tasks.is_empty())
    }

    fn has_active_or_pending_task(&self, goal_id: &str) -> bool {
        self.task_repo
            .list_by_goal(goal_id)
            .ok()
            .map_or(false, |tasks| {
                tasks.iter().any(|t| {
                    !matches!(
                        t.status,
                        TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Failed
                    )
                })
            })
    }

    pub fn spawn(self, interval_secs: u64) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval_secs)).await;
                let s = self.clone();
                if let Ok(ids) = tokio::task::spawn_blocking(move || s.tick()).await {
                    if !ids.is_empty() {
                        tracing::info!("scheduler: created {} tasks", ids.len());
                    }
                }
            }
        });
    }
}
