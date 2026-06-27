#[derive(Debug)]
pub struct SuccessfulRun {
    pub goal: String,
    pub total_steps: u32,
    pub tools_used: Vec<String>,
    pub key_changes: Vec<String>,
    pub verification_passed: bool,
}

pub fn extract_successful_run(
    events: &[crate::harness::trace::event::HarnessEvent],
) -> Option<SuccessfulRun> {
    let goal = events.iter().find_map(|e| {
        if let crate::harness::trace::event::HarnessEvent::RunStarted { ref input, .. } = e {
            Some(input.clone())
        } else {
            None
        }
    })?;

    let tools_used: Vec<String> = events
        .iter()
        .filter_map(|e| {
            if let crate::harness::trace::event::HarnessEvent::ToolCall {
                ref tool_name,
                success,
                ..
            } = e
            {
                if *success {
                    Some(tool_name.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    let verification_passed = events.iter().any(|e| {
        matches!(
            e,
            crate::harness::trace::event::HarnessEvent::Verification { success: true, .. }
        )
    });

    Some(SuccessfulRun {
        goal,
        total_steps: events.len() as u32,
        tools_used,
        key_changes: Vec::new(),
        verification_passed,
    })
}

pub fn events_to_runbook(
    events: &[crate::harness::trace::event::HarnessEvent],
    fallback_goal: &str,
) -> Option<crate::core::Runbook> {
    if events.is_empty() {
        return None;
    }

    let goal = events
        .iter()
        .find_map(|event| {
            if let crate::harness::trace::event::HarnessEvent::RunStarted { input, .. } = event {
                Some(input.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| fallback_goal.to_string());

    let created_at = events
        .iter()
        .find_map(|event| {
            if let crate::harness::trace::event::HarnessEvent::RunStarted { timestamp, .. } = event
            {
                Some(timestamp.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(current_timestamp);

    let mut steps = Vec::new();
    for event in events {
        if let crate::harness::trace::event::HarnessEvent::ToolCall {
            step,
            tool_name,
            args_hash,
            success,
        } = event
        {
            let verification = verification_for_step(events, *step);
            steps.push(crate::core::RunbookStep {
                action: format!("tool call step {step}"),
                tool: tool_name.clone(),
                input: format!("args_hash={args_hash}"),
                output_status: if *success { "passed" } else { "failed" }.into(),
                output_preview: String::new(),
                verification,
            });
        }
    }

    let passed = steps
        .iter()
        .filter(|step| step.output_status == "passed")
        .count();
    let failed = steps
        .iter()
        .filter(|step| step.output_status == "failed")
        .count();

    Some(crate::core::Runbook {
        id: crate::core::models::id("runbook"),
        goal,
        conversation_id: None,
        total_steps: steps.len(),
        passed,
        failed,
        steps,
        created_at,
    })
}

fn verification_for_step(
    events: &[crate::harness::trace::event::HarnessEvent],
    target_step: u32,
) -> String {
    events
        .iter()
        .filter_map(|event| {
            if let crate::harness::trace::event::HarnessEvent::Verification {
                command,
                success,
                exit_code,
                step,
            } = event
            {
                if *step == target_step {
                    Some(format!(
                        "{} ({}, exit={})",
                        command,
                        if *success { "passed" } else { "failed" },
                        exit_code
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "none".into())
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn current_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::event::HarnessEvent;

    #[test]
    fn converts_trace_events_to_runbook_steps() {
        let events = vec![
            HarnessEvent::RunStarted {
                timestamp: 42,
                input: "fix bug".into(),
                mode: "react".into(),
            },
            HarnessEvent::ToolCall {
                step: 1,
                tool_name: "self_test".into(),
                args_hash: "abc".into(),
                success: true,
            },
            HarnessEvent::Verification {
                command: "{}".into(),
                success: true,
                exit_code: Some(0),
                step: 1,
            },
            HarnessEvent::RunCompleted {
                timestamp: 43,
                total_steps: 1,
                outcome: "Finished".into(),
            },
        ];

        let runbook = events_to_runbook(&events, "fallback").unwrap();

        assert_eq!(runbook.goal, "fix bug");
        assert_eq!(runbook.created_at, "42");
        assert_eq!(runbook.total_steps, 1);
        assert_eq!(runbook.passed, 1);
        assert_eq!(runbook.failed, 0);
        assert_eq!(runbook.steps[0].tool, "self_test");
        assert_eq!(runbook.steps[0].input, "args_hash=abc");
        assert!(runbook.steps[0].verification.contains("passed"));
    }

    #[test]
    fn empty_trace_does_not_create_runbook() {
        assert!(events_to_runbook(&[], "fallback").is_none());
    }
}
