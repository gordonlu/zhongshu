use crate::agent::llm::{ChatCompletionRequest, LlmProvider, Message, Role};
use crate::core::db::Database;
use crate::core::models::*;
use crate::core::task::TaskRepository;

/// Breaks a task into executable steps using the LLM.
#[derive(Clone)]
pub struct TaskPlanner {
    repo: TaskRepository,
}

impl TaskPlanner {
    pub fn new(db: Database) -> Self {
        TaskPlanner {
            repo: TaskRepository::new(db),
        }
    }

    /// Generate a plan for the given task using the LLM.
    /// Steps are saved to the database. Returns the created steps.
    pub async fn plan(
        &self,
        task_id: &str,
        provider: &dyn LlmProvider,
    ) -> anyhow::Result<Vec<TaskStep>> {
        let task = match self.repo.get(task_id)? {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let prompt = format!(
            "你是任务规划专家。为以下任务生成详细的执行步骤，\
             每个步骤应是一个具体的、可执行的动作描述。\
             以 JSON 字符串数组格式返回，每个字符串是一个步骤。\
             只返回 JSON 数组，不要包含其他文字、代码块标记或解释。\
             步骤数控制在 3-6 步。\n\n任务：{}",
            task.title,
        );

        let req = ChatCompletionRequest {
            model: provider.model_name().into(),
            messages: vec![Message {
                role: Role::User,
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.3),
            max_tokens: Some(1500),
            reasoning_effort: None,
        };

        let response = provider.chat(req).await?;
        let text = response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        let steps: Vec<String> = serde_json::from_str(&text).unwrap_or_else(|_| {
            tracing::warn!("planner: LLM response was not valid JSON, using defaults");
            vec![
                "分析任务需求".into(),
                "执行主要工作".into(),
                "验证结果".into(),
            ]
        });

        let mut created = Vec::new();
        for (i, action) in steps.iter().enumerate() {
            let step = self.repo.add_step(task_id, i as i32, action)?;
            created.push(step);
        }
        self.repo.update_status(task_id, TaskStatus::Planning)?;
        tracing::info!(task = %task.title, steps = steps.len(), "planner: generated plan");
        Ok(created)
    }
}
