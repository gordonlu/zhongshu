use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::action::journal::ActionJournal;
use crate::action::lifecycle::{ActionRequest, ActionResult, ActionStatus};
use crate::action::policy::ActionPolicy;
use crate::agent::{AgentCallbacks, ToolCompletionStatus};
use crate::tool::{
    infer_side_effect, SideEffect, ToolExecutionPolicy, ToolExecutor, ToolOutput, ToolRegistry,
    ToolStatus, ToolTermination,
};

/// Run the full per-action lifecycle: idempotency check → start callback →
/// tool execution → interruption/unknown-outcome → auth wait → done callback.
///
/// Returns an `ActionResult` the caller uses to update messages, traces, etc.
pub async fn dispatch(
    request: ActionRequest,
    registry: &ToolRegistry,
    journal: &ActionJournal,
    tool_timeout: std::time::Duration,
    cancel_token: &CancellationToken,
    callbacks: Option<&AgentCallbacks>,
) -> ActionResult {
    let executor = ToolExecutor::with_policy(
        registry,
        ToolExecutionPolicy {
            timeout: tool_timeout,
            ..Default::default()
        },
    );
    dispatch_with(request, &executor, journal, cancel_token, callbacks).await
}

/// Core dispatch implementation that accepts an already-configured executor.
/// Useful when the caller needs to set up the executor specially.
pub async fn dispatch_with(
    request: ActionRequest,
    executor: &ToolExecutor<'_>,
    journal: &ActionJournal,
    cancel_token: &CancellationToken,
    callbacks: Option<&AgentCallbacks>,
) -> ActionResult {
    let idempotency_key = make_idempotency_key(&request.tool_name, &request.arguments);
    let policy = ActionPolicy::from_tool(&request.tool_name);

    // ── Idempotency check ──────────────────────────────────────────────
    if policy.should_skip_if_completed() {
        if journal.is_tool_completed(&idempotency_key) {
            info!(tool = %request.tool_name, "tool already completed, skipping");
            return ActionResult {
                status: ActionStatus::Completed,
                observation: format!(
                    "<observation tool=\"{}\" status=\"completed\">此操作用于上次中断前已完成，跳过重复执行。</observation>",
                    request.tool_name
                ),
                tool_calls_made: request.tool_calls_made,
                tool_termination: ToolTermination::Completed,
                output_status: ToolStatus::Success,
                output_error: None,
                output_request_id: None,
                tool_output: None,
                was_idempotent_skip: true,
            };
        }
    }

    // ── Record tool start ──────────────────────────────────────────────
    if let Some(ref cb) = callbacks {
        (cb.on_tool_start)(&request.tool_name, &request.arguments);
    }
    journal.record_start(&request.tool_name, &request.arguments, &idempotency_key);

    // ── Execute ────────────────────────────────────────────────────────
    let execution = executor
        .execute(
            &request.tool_name,
            &request.arguments,
            Some(cancel_token.clone()),
        )
        .await;
    let termination = execution.termination;
    let output = execution.output;

    // ── Interruption handling ──────────────────────────────────────────
    if termination != ToolTermination::Completed {
        let side_effect = execution
            .spec
            .as_ref()
            .map(|s| s.side_effect)
            .unwrap_or_else(|| infer_side_effect(&request.tool_name));

        match side_effect {
            SideEffect::ReadOnly => {}
            SideEffect::LocalWrite
            | SideEffect::SystemChange
            | SideEffect::ExternalAction
            | SideEffect::Irreversible => {
                let original = output.error.as_deref().unwrap_or("未知原因");
                let interrupted = ToolOutput::error(format!(
                    "{}。实际执行状态未知，此操作可能已部分或完全执行，请核实系统状态。",
                    original,
                ));
                let observation = interrupted.render_observation(&request.tool_name);

                if let Some(ref cb) = callbacks {
                    (cb.on_tool_done)(
                        &request.tool_name,
                        &request.arguments,
                        ToolCompletionStatus::UnknownEffect,
                    );
                }
                journal.record_completion(
                    &request.tool_name,
                    &request.arguments,
                    &idempotency_key,
                    "unknown_effect",
                );

                return ActionResult {
                    status: ActionStatus::UnknownOutcome,
                    observation,
                    tool_calls_made: request.tool_calls_made,
                    tool_termination: termination,
                    output_status: ToolStatus::Error,
                    output_error: Some(original.to_string()),
                    output_request_id: None,
                    was_idempotent_skip: false,
                    tool_output: Some(output.clone()),
                };
            }
        }
    }

    // ── AuthRequired handling ──────────────────────────────────────────
    if output.status == ToolStatus::AuthRequired {
        if let Some(ref cb) = callbacks {
            (cb.on_tool_done)(
                &request.tool_name,
                &request.arguments,
                ToolCompletionStatus::AwaitingApproval,
            );
        }
        info!(tool = %request.tool_name, status = "auth_required");

        let rid = output.request_id.clone();
        let mut approval_outcome = "cancelled";
        if let Some(ref rid_val) = rid {
            let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
            if crate::authority::register_waiter_if_pending(rid_val, outcome_tx).is_ok() {
                let outcome = tokio::select! {
                    result = outcome_rx => {
                        match result {
                            Ok(crate::authority::AuthOutcome::Approved) => "approved",
                            Ok(crate::authority::AuthOutcome::Denied) => "denied",
                            _ => "cancelled",
                        }
                    }
                    _ = cancel_token.cancelled() => {
                        crate::authority::take_pending(rid_val);
                        "cancelled"
                    }
                };
                approval_outcome = outcome;
            }
        }

        let observation = match approval_outcome {
            "approved" => format!(
                "<observation tool=\"{}\" status=\"approved\">用户已授权，可以执行此工具。</observation>",
                request.tool_name
            ),
            "denied" => format!(
                "<observation tool=\"{}\" status=\"denied\">用户已拒绝此工具执行，请换其他方法。</observation>",
                request.tool_name
            ),
            _ => output.render_observation(&request.tool_name),
        };

        if let Some(ref cb) = callbacks {
            (cb.on_tool_done)(
                &request.tool_name,
                &request.arguments,
                match approval_outcome {
                    "approved" => ToolCompletionStatus::Completed,
                    "denied" => ToolCompletionStatus::Failed,
                    _ => ToolCompletionStatus::Cancelled,
                },
            );
        }
        journal.record_completion(
            &request.tool_name,
            &request.arguments,
            &idempotency_key,
            match approval_outcome {
                "approved" => "approved",
                "denied" => "denied",
                _ => "cancelled",
            },
        );

        return ActionResult {
            status: match approval_outcome {
                "approved" => ActionStatus::Completed,
                "denied" => ActionStatus::Failed,
                _ => ActionStatus::Cancelled,
            },
            observation,
            tool_calls_made: request.tool_calls_made,
            tool_termination: termination,
            output_status: output.status,
            output_error: output.error.clone(),
            output_request_id: output.request_id.clone(),
            tool_output: Some(output.clone()),
            was_idempotent_skip: false,
        };
    }

    // ── Normal completion ──────────────────────────────────────────────
    let tool_success = matches!(output.status, ToolStatus::Success);
    let completion_status = match (tool_success, termination) {
        (true, _) => ToolCompletionStatus::Completed,
        (false, ToolTermination::TimedOut) => ToolCompletionStatus::TimedOut,
        (false, ToolTermination::Cancelled) => ToolCompletionStatus::Cancelled,
        (false, _) => ToolCompletionStatus::Failed,
    };

    if let Some(ref cb) = callbacks {
        (cb.on_tool_done)(&request.tool_name, &request.arguments, completion_status);
    }
    journal.record_completion(
        &request.tool_name,
        &request.arguments,
        &idempotency_key,
        completion_status.as_ledger_status(),
    );

    let action_status = match completion_status {
        ToolCompletionStatus::Completed => ActionStatus::Completed,
        ToolCompletionStatus::Failed => ActionStatus::Failed,
        ToolCompletionStatus::TimedOut => ActionStatus::TimedOut,
        ToolCompletionStatus::Cancelled => ActionStatus::Cancelled,
        _ => ActionStatus::Completed,
    };

    ActionResult {
        status: action_status,
        observation: output.render_observation(&request.tool_name),
        tool_calls_made: request.tool_calls_made + 1,
        tool_termination: termination,
        output_status: output.status,
        output_error: output.error.clone(),
        output_request_id: output.request_id.clone(),
        tool_output: Some(output.clone()),
        was_idempotent_skip: false,
    }
}

fn make_idempotency_key(tool_name: &str, arguments: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    arguments.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

impl ActionResult {
    pub fn success(&self) -> bool {
        matches!(self.status, ActionStatus::Completed) && !self.was_idempotent_skip
    }

    pub fn should_count_tool_call(&self) -> bool {
        !self.was_idempotent_skip
    }
}
