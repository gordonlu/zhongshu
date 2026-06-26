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
