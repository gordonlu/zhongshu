use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agent::loop_::{LoopResult, RunOutcome, StopReason};
use crate::agent::llm::Message;
use crate::harness::trace::event::HarnessEvent;

/// A structured, exportable receipt for a single agent run.
///
/// Aggregates outcome, tool calls, verification results, file changes,
/// approval decisions, budget usage, and any recovery/inflight state
/// into one serializable document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReceipt {
    pub run_id: String,
    pub model: String,
    pub started_at: u64,
    pub duration_ms: u128,

    pub outcome: RunOutcome,
    pub stop_reason: String,

    pub tool_calls_made: usize,
    pub estimated_tokens: usize,
    pub messages: Vec<Message>,

    pub budget: ReceiptBudget,
    pub tools: Vec<ToolReceipt>,
    pub verifications: Vec<VerificationReceipt>,
    pub patches: Vec<PatchReceipt>,
    pub approvals: Vec<ApprovalReceipt>,
    pub inflight: Vec<String>,
    pub recovered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptBudget {
    pub token_limit: usize,
    pub max_steps: u32,
    pub max_tool_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReceipt {
    pub name: String,
    pub args_hash: String,
    pub success: bool,
    pub step: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReceipt {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub step: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchReceipt {
    pub path: PathBuf,
    pub operation: String,
    pub changed: bool,
    pub diff_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalReceipt {
    pub tool: String,
    pub decision: String,
}

impl RunReceipt {
    pub fn from_loop_result(
        result: &LoopResult,
        run_id: &str,
        model: &str,
        budget: &crate::agent::AgentBudget,
        started_at: u64,
        approvals: Vec<ApprovalReceipt>,
        inflight: Vec<String>,
        recovered: bool,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let started_ms = started_at as u128 * 1000;
        let duration_ms = now.saturating_sub(started_ms);

        let mut tools = Vec::new();
        let mut verifications = Vec::new();
        let mut patches = Vec::new();

        for event in &result.trace_events {
            match event {
                HarnessEvent::ToolCall {
                    step,
                    tool_name,
                    args_hash,
                    success,
                } => tools.push(ToolReceipt {
                    name: tool_name.clone(),
                    args_hash: args_hash.clone(),
                    success: *success,
                    step: *step,
                }),
                HarnessEvent::Verification {
                    command,
                    success,
                    exit_code,
                    step,
                } => verifications.push(VerificationReceipt {
                    command: command.clone(),
                    success: *success,
                    exit_code: *exit_code,
                    step: *step,
                }),
                HarnessEvent::PatchApplied {
                    path,
                    operation,
                    changed,
                    ..
                } => patches.push(PatchReceipt {
                    path: path.clone(),
                    operation: operation.clone(),
                    changed: *changed,
                    diff_summary: String::new(),
                }),
                HarnessEvent::PatchPreview {
                    path,
                    operation,
                    diff_summary,
                    ..
                } => {
                    if !patches.iter().any(|p| p.path == *path) {
                        patches.push(PatchReceipt {
                            path: path.clone(),
                            operation: operation.clone(),
                            changed: false,
                            diff_summary: diff_summary.clone(),
                        });
                    }
                }
                _ => {}
            }
        }

        RunReceipt {
            run_id: run_id.to_string(),
            model: model.to_string(),
            started_at,
            duration_ms,
            outcome: result.outcome,
            stop_reason: format!("{:?}", result.stop_reason),
            tool_calls_made: result.tool_calls_made,
            estimated_tokens: result.estimated_tokens,
            messages: result.messages.clone(),
            budget: ReceiptBudget {
                token_limit: budget.token_limit,
                max_steps: budget.max_steps,
                max_tool_calls: budget.max_tool_calls,
            },
            tools,
            verifications,
            patches,
            approvals,
            inflight,
            recovered,
        }
    }

    pub fn summary_text(&self) -> String {
        let outcome = match self.outcome {
            RunOutcome::CompletedVerified => "✓ 已验证完成",
            RunOutcome::CompletedUnverified => "✓ 完成（未验证）",
            RunOutcome::Blocked => "✗ 被阻塞",
            RunOutcome::Interrupted => "⚠ 被中断",
            RunOutcome::BudgetExhausted => "✗ 预算耗尽",
            RunOutcome::Failed => "✗ 执行失败",
        };
        let tool_count = self.tools.len();
        let passed = self.verifications.iter().filter(|v| v.success).count();
        let failed = self.verifications.iter().filter(|v| !v.success).count();
        let patches_count = self.patches.len();

        format!(
            "## Run Receipt\n\
             - ID: {}\n\
             - 模型: {}\n\
             - 结果: {} ({})\n\
             - 工具调用: {} 次\n\
             - 验证: {} 通过 / {} 失败\n\
             - 文件修改: {} 处\n\
             - Token 估计: {}\n\
             - 耗时: {}ms\n\
             {}",
            self.run_id,
            self.model,
            outcome,
            self.stop_reason,
            tool_count,
            passed,
            failed,
            patches_count,
            self.estimated_tokens,
            self.duration_ms,
            if self.recovered {
                "* 本次运行从崩溃检查点恢复\n"
            } else {
                ""
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
use crate::agent::loop_::{LoopResult, RunOutcome, StopReason};
    use crate::agent::AgentBudget;
    use crate::harness::trace::event::HarnessEvent;

    #[test]
    fn run_receipt_from_loop_result() {
        let result = LoopResult {
            messages: vec![],
            stop_reason: StopReason::Finished,
            outcome: RunOutcome::CompletedUnverified,
            tool_calls_made: 3,
            estimated_tokens: 1500,
            trace_events: vec![
                HarnessEvent::ToolCall {
                    step: 1,
                    tool_name: "read_file".into(),
                    args_hash: "abc".into(),
                    success: true,
                },
                HarnessEvent::Verification {
                    command: "cargo test".into(),
                    success: true,
                    exit_code: Some(0),
                    step: 2,
                },
                HarnessEvent::PatchApplied {
                    session_id: None,
                    path: PathBuf::from("src/main.rs"),
                    operation: "edit".into(),
                    changed: true,
                },
            ],
        };
        let budget = AgentBudget::assistant_default();
        let receipt = RunReceipt::from_loop_result(
            &result,
            "test-run",
            "gpt-4",
            &budget,
            1000,
            vec![],
            vec![],
            false,
        );

        assert_eq!(receipt.run_id, "test-run");
        assert_eq!(receipt.model, "gpt-4");
        assert_eq!(receipt.outcome, RunOutcome::CompletedUnverified);
        assert_eq!(receipt.tool_calls_made, 3);
        assert_eq!(receipt.tools.len(), 1);
        assert_eq!(receipt.tools[0].name, "read_file");
        assert_eq!(receipt.verifications.len(), 1);
        assert_eq!(receipt.verifications[0].command, "cargo test");
        assert_eq!(receipt.patches.len(), 1);
        assert_eq!(receipt.patches[0].path, PathBuf::from("src/main.rs"));

        let summary = receipt.summary_text();
        assert!(summary.contains("完成（未验证）"));
        assert!(summary.contains("工具调用"));
    }
}
