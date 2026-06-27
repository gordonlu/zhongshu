use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::tool::spec::{ObservableToolInput, ToolReplayKey, ToolResultSummary, ToolSpec};
use crate::tool::{ToolOutput, ToolRegistry, ToolStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionPolicy {
    pub timeout: Duration,
    pub result_preview_chars: usize,
}

impl Default for ToolExecutionPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            result_preview_chars: 2_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub tool_name: String,
    pub spec: Option<ToolSpec>,
    pub observable_input: ObservableToolInput,
    pub replay_key: ToolReplayKey,
    pub output: ToolOutput,
    pub summary: ToolResultSummary,
    pub elapsed_ms: u128,
    pub timed_out: bool,
}

pub struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    policy: ToolExecutionPolicy,
}

impl<'a> ToolExecutor<'a> {
    pub fn new(registry: &'a ToolRegistry) -> Self {
        Self {
            registry,
            policy: ToolExecutionPolicy::default(),
        }
    }

    pub fn with_policy(registry: &'a ToolRegistry, policy: ToolExecutionPolicy) -> Self {
        Self { registry, policy }
    }

    pub async fn execute(&self, name: &str, arguments: &str) -> ToolExecution {
        let args = parse_arguments(arguments);
        let observable_input = ObservableToolInput::new(name, args.clone());
        let replay_key = ToolReplayKey::from_observable(&observable_input);
        let spec = self.registry.get(name).map(|tool| tool.spec());
        let started = Instant::now();

        let result = tokio::time::timeout(
            self.policy.timeout,
            self.registry.execute(name, &args.to_string()),
        )
        .await;

        let (output, timed_out) = match result {
            Ok(output) => (output, false),
            Err(_) => (
                ToolOutput::error(format!(
                    "tool '{name}' timed out after {:?}",
                    self.policy.timeout
                )),
                true,
            ),
        };

        let summary = ToolResultSummary::from_output(&output, self.policy.result_preview_chars);

        ToolExecution {
            tool_name: name.to_string(),
            spec,
            observable_input,
            replay_key,
            output,
            summary,
            elapsed_ms: started.elapsed().as_millis(),
            timed_out,
        }
    }

    pub async fn execute_plan(&self, calls: Vec<ToolCallRequest>) -> ToolExecutionPlan {
        let mut results = Vec::with_capacity(calls.len());
        let mut saw_mutation = false;
        for call in calls {
            let execution = self.execute(&call.name, &call.arguments).await;
            if execution
                .spec
                .as_ref()
                .map(|spec| !spec.read_only || spec.destructive)
                .unwrap_or(true)
            {
                saw_mutation = true;
            }
            let should_stop = execution.output.status != ToolStatus::Success
                || execution.timed_out
                || saw_mutation && call.stop_after_mutation;
            results.push(execution);
            if should_stop {
                break;
            }
        }

        ToolExecutionPlan {
            executions: results,
            scheduling: ToolScheduling::Serial,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments: String,
    pub stop_after_mutation: bool,
}

impl ToolCallRequest {
    pub fn new(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
            stop_after_mutation: false,
        }
    }

    pub fn stop_after_mutation(mut self, stop: bool) -> Self {
        self.stop_after_mutation = stop;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionPlan {
    pub executions: Vec<ToolExecution>,
    pub scheduling: ToolScheduling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolScheduling {
    Serial,
}

fn parse_arguments(arguments: &str) -> serde_json::Value {
    if arguments.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments).unwrap_or_else(|e| {
            serde_json::json!({
                "__parse_error": e.to_string(),
                "__raw": arguments,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use async_trait::async_trait;

    struct EchoTool;
    struct WriteTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "echo"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(arguments.clone())
        }
    }

    #[async_trait]
    impl Tool for WriteTool {
        fn name(&self) -> &str {
            "fs"
        }

        fn description(&self) -> &str {
            "write"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(arguments.clone())
        }
    }

    #[tokio::test]
    async fn executor_returns_structured_execution_record() {
        let registry = ToolRegistry::new().register(EchoTool);
        let execution = ToolExecutor::new(&registry)
            .execute("echo", r#"{"b":1,"a":2}"#)
            .await;

        assert_eq!(execution.tool_name, "echo");
        assert_eq!(execution.output.status, crate::tool::ToolStatus::Success);
        assert!(execution.spec.is_some());
        assert!(!execution.timed_out);
        assert_eq!(
            execution.observable_input.arguments,
            serde_json::json!({"a": 2, "b": 1})
        );
    }

    #[tokio::test]
    async fn executor_records_parse_error_as_observable_input() {
        let registry = ToolRegistry::new().register(EchoTool);
        let execution = ToolExecutor::new(&registry).execute("echo", "{").await;

        assert!(execution
            .observable_input
            .arguments
            .get("__parse_error")
            .is_some());
    }

    #[tokio::test]
    async fn execution_plan_stops_after_mutation_when_requested() {
        let registry = ToolRegistry::new().register(WriteTool).register(EchoTool);
        let plan = ToolExecutor::new(&registry)
            .execute_plan(vec![
                ToolCallRequest::new("fs", r#"{"path":"a.txt"}"#).stop_after_mutation(true),
                ToolCallRequest::new("echo", r#"{"a":1}"#),
            ])
            .await;

        assert_eq!(plan.scheduling, ToolScheduling::Serial);
        assert_eq!(plan.executions.len(), 1);
        assert_eq!(plan.executions[0].tool_name, "fs");
    }
}
