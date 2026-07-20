use std::time::{Duration, Instant};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

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

    pub async fn execute(
        &self,
        name: &str,
        arguments: &str,
        cancel_token: Option<CancellationToken>,
    ) -> ToolExecution {
        let args = parse_arguments(arguments);
        let args_string = args.to_string();
        let observable_input = ObservableToolInput::new(name, args);
        let replay_key = ToolReplayKey::from_observable(&observable_input);
        let spec = self.registry.get(name).map(|tool| tool.spec());
        let started = Instant::now();

        let (output, timed_out) = match cancel_token {
            Some(ct) => {
                tokio::select! {
                    result = tokio::time::timeout(
                        self.policy.timeout,
                        self.registry.execute(name, &args_string),
                    ) => {
                        match result {
                            Ok(output) => (output, false),
                            Err(_) => (
                                ToolOutput::error(format!(
                                    "tool '{name}' timed out after {:?}",
                                    self.policy.timeout
                                )),
                                true,
                            ),
                        }
                    }
                    _ = ct.cancelled() => {
                        (
                            ToolOutput::error(format!(
                                "tool '{name}' was cancelled",
                            )),
                            true,
                        )
                    }
                }
            }
            None => {
                let result = tokio::time::timeout(
                    self.policy.timeout,
                    self.registry.execute(name, &args_string),
                )
                .await;
                match result {
                    Ok(output) => (output, false),
                    Err(_) => (
                        ToolOutput::error(format!(
                            "tool '{name}' timed out after {:?}",
                            self.policy.timeout
                        )),
                        true,
                    ),
                }
            }
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

    pub async fn execute_plan(
        &self,
        calls: Vec<ToolCallRequest>,
        cancel_token: Option<CancellationToken>,
    ) -> ToolExecutionPlan {
        let mut results = Vec::with_capacity(calls.len());
        let mut read_group = Vec::new();
        let mut used_parallel_read_group = false;
        let mut used_serial_boundary = false;

        for call in calls {
            if self.can_execute_concurrently(&call.name) {
                read_group.push(call);
                continue;
            }

            let (flushed_parallel, should_stop) = self
                .flush_concurrent_read_group(&mut read_group, &mut results, cancel_token.clone())
                .await;
            used_parallel_read_group |= flushed_parallel;
            if should_stop {
                break;
            }

            let execution = self.execute(&call.name, &call.arguments, cancel_token.clone()).await;
            used_serial_boundary = true;
            let should_stop = execution.output.status != ToolStatus::Success
                || execution.timed_out
                || Self::is_mutating_execution(&execution) && call.stop_after_mutation;
            results.push(execution);
            if should_stop {
                break;
            }
        }

        if !read_group.is_empty() {
            let (flushed_parallel, _) = self
                .flush_concurrent_read_group(&mut read_group, &mut results, None)
                .await;
            used_parallel_read_group |= flushed_parallel;
        }

        ToolExecutionPlan {
            executions: results,
            scheduling: ToolScheduling::from_execution_shape(
                used_parallel_read_group,
                used_serial_boundary,
            ),
        }
    }

    fn can_execute_concurrently(&self, name: &str) -> bool {
        self.registry
            .get(name)
            .map(|tool| {
                let spec = tool.spec();
                spec.read_only && !spec.destructive && spec.supports_concurrent_execution
            })
            .unwrap_or(false)
    }

    async fn flush_concurrent_read_group(
        &self,
        read_group: &mut Vec<ToolCallRequest>,
        results: &mut Vec<ToolExecution>,
        cancel_token: Option<CancellationToken>,
    ) -> (bool, bool) {
        if read_group.is_empty() {
            return (false, false);
        }

        let calls = std::mem::take(read_group);
        let used_parallel = calls.len() > 1;
        let executions = join_all(
            calls
                .iter()
                .map(|call| self.execute(&call.name, &call.arguments, cancel_token.clone())),
        )
        .await;
        let should_stop = executions.iter().any(Self::is_failed_execution);
        results.extend(executions);
        (used_parallel, should_stop)
    }

    fn is_failed_execution(execution: &ToolExecution) -> bool {
        execution.output.status != ToolStatus::Success || execution.timed_out
    }

    fn is_mutating_execution(execution: &ToolExecution) -> bool {
        execution
            .spec
            .as_ref()
            .map(|spec| !spec.read_only || spec.destructive)
            .unwrap_or(true)
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
    ConcurrentReadOnly,
    Mixed,
}

impl ToolScheduling {
    fn from_execution_shape(used_parallel_read_group: bool, used_serial_boundary: bool) -> Self {
        match (used_parallel_read_group, used_serial_boundary) {
            (true, true) => Self::Mixed,
            (true, false) => Self::ConcurrentReadOnly,
            _ => Self::Serial,
        }
    }
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct EchoTool;
    struct WriteTool;
    struct ConcurrentSearchTool {
        current: Arc<AtomicUsize>,
        max: Arc<AtomicUsize>,
        writes_seen: Arc<AtomicUsize>,
    }

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

    #[async_trait]
    impl Tool for ConcurrentSearchTool {
        fn name(&self) -> &str {
            "search_files"
        }

        fn description(&self) -> &str {
            "search"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
            if arguments
                .get("fail")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return ToolOutput::error("search failed");
            }

            let active = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            record_max(&self.max, active);
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
            ToolOutput::success(serde_json::json!({
                "active": active,
                "writes_seen": self.writes_seen.load(Ordering::SeqCst),
            }))
        }
    }

    fn concurrent_search_tool() -> (
        ConcurrentSearchTool,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let current = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let writes_seen = Arc::new(AtomicUsize::new(0));
        (
            ConcurrentSearchTool {
                current: Arc::clone(&current),
                max: Arc::clone(&max),
                writes_seen: Arc::clone(&writes_seen),
            },
            current,
            max,
            writes_seen,
        )
    }

    fn record_max(max: &AtomicUsize, value: usize) {
        let mut observed = max.load(Ordering::SeqCst);
        while value > observed {
            match max.compare_exchange(observed, value, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
    }

    #[tokio::test]
    async fn executor_returns_structured_execution_record() {
        let registry = ToolRegistry::new().register(EchoTool);
        let execution = ToolExecutor::new(&registry)
            .execute("echo", r#"{"b":1,"a":2}"#, None)
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
        let execution = ToolExecutor::new(&registry).execute("echo", "{", None).await;

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
            .execute_plan(
                vec![
                    ToolCallRequest::new("fs", r#"{"path":"a.txt"}"#).stop_after_mutation(true),
                    ToolCallRequest::new("echo", r#"{"a":1}"#),
                ],
                None,
            )
            .await;

        assert_eq!(plan.scheduling, ToolScheduling::Serial);
        assert_eq!(plan.executions.len(), 1);
        assert_eq!(plan.executions[0].tool_name, "fs");
    }

    #[tokio::test]
    async fn execution_plan_runs_safe_read_tools_concurrently() {
        let (search, _current, max, _writes_seen) = concurrent_search_tool();
        let registry = ToolRegistry::new().register(search);
        let plan = ToolExecutor::new(&registry)
            .execute_plan(
                vec![
                    ToolCallRequest::new("search_files", r#"{"query":"a"}"#),
                    ToolCallRequest::new("search_files", r#"{"query":"b"}"#),
                ],
                None,
            )
            .await;

        assert_eq!(plan.scheduling, ToolScheduling::ConcurrentReadOnly);
        assert_eq!(plan.executions.len(), 2);
        assert_eq!(max.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn execution_plan_preserves_serial_boundaries_around_mutations() {
        let (search, _current, max, _writes_seen) = concurrent_search_tool();
        let registry = ToolRegistry::new().register(search).register(WriteTool);
        let plan = ToolExecutor::new(&registry)
            .execute_plan(
                vec![
                    ToolCallRequest::new("search_files", r#"{"query":"a"}"#),
                    ToolCallRequest::new("search_files", r#"{"query":"b"}"#),
                    ToolCallRequest::new("fs", r#"{"path":"a.txt"}"#),
                    ToolCallRequest::new("search_files", r#"{"query":"c"}"#),
                ],
                None,
            )
            .await;

        assert_eq!(plan.scheduling, ToolScheduling::Mixed);
        assert_eq!(plan.executions.len(), 4);
        assert_eq!(plan.executions[0].tool_name, "search_files");
        assert_eq!(plan.executions[1].tool_name, "search_files");
        assert_eq!(plan.executions[2].tool_name, "fs");
        assert_eq!(plan.executions[3].tool_name, "search_files");
        assert_eq!(max.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn execution_plan_stops_after_failed_read_group() {
        let (search, _current, _max, _writes_seen) = concurrent_search_tool();
        let registry = ToolRegistry::new().register(search).register(WriteTool);
        let plan = ToolExecutor::new(&registry)
            .execute_plan(
                vec![
                    ToolCallRequest::new("search_files", r#"{"query":"a"}"#),
                    ToolCallRequest::new("search_files", r#"{"fail":true}"#),
                    ToolCallRequest::new("fs", r#"{"path":"a.txt"}"#),
                ],
                None,
            )
            .await;

        assert_eq!(plan.scheduling, ToolScheduling::ConcurrentReadOnly);
        assert_eq!(plan.executions.len(), 2);
        assert_eq!(plan.executions[0].tool_name, "search_files");
        assert_eq!(plan.executions[1].output.status, ToolStatus::Error);
    }
}
