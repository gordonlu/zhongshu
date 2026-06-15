use crate::core::db::Database;
use crate::core::task::TaskRepository;
use crate::core::models::*;

/// Breaks a task into executable steps.
/// In the future this will use the LLM to generate plans.
/// For now, uses simple heuristics.
#[derive(Clone)]
pub struct TaskPlanner {
    repo: TaskRepository,
}

impl TaskPlanner {
    pub fn new(db: Database) -> Self {
        TaskPlanner { repo: TaskRepository::new(db) }
    }

    /// Generate a plan for the given task based on its title.
    pub fn plan(&self, task_id: &str) -> rusqlite::Result<Vec<TaskStep>> {
        let task = match self.repo.get(task_id)? {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let steps = self.infer_steps(&task.title);
        let mut created = Vec::new();
        for (i, action) in steps.iter().enumerate() {
            let step = self.repo.add_step(task_id, i as i32, action)?;
            created.push(step);
        }
        self.repo.update_status(task_id, TaskStatus::Planning)?;
        Ok(created)
    }

    fn infer_steps(&self, title: &str) -> Vec<String> {
        let lower = title.to_lowercase();
        if lower.contains("总结") || lower.contains("report") || lower.contains("汇总") {
            vec![
                "收集相关资料".into(),
                "整理和分析数据".into(),
                "生成报告".into(),
            ]
        } else if lower.contains("搜索") || lower.contains("search") || lower.contains("找") {
            vec![
                "确定搜索范围和关键词".into(),
                "执行搜索".into(),
                "整理结果".into(),
            ]
        } else if lower.contains("清理") || lower.contains("clean") || lower.contains("整理") {
            vec![
                "检查当前状态".into(),
                "清理不需要的文件或数据".into(),
                "确认清理结果".into(),
            ]
        } else {
            vec![
                "分析任务需求".into(),
                "执行主要工作".into(),
                "验证结果".into(),
            ]
        }
    }
}
