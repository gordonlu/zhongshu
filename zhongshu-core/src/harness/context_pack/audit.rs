use crate::harness::trace::event::HarnessEvent;

pub fn build_report(events: &[HarnessEvent]) -> String {
    let total = events.len();
    let tool_calls = events
        .iter()
        .filter(|e| matches!(e, HarnessEvent::ToolCall { .. }))
        .count();
    let verifications = events
        .iter()
        .filter(|e| matches!(e, HarnessEvent::Verification { .. }))
        .count();
    format!(
        "Run audit: {} events, {} tool calls, {} verifications",
        total, tool_calls, verifications
    )
}
