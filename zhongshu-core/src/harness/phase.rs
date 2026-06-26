use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
use crate::harness::state::CodingPhase;

/// Infer phase from a tool call name and result.
pub fn infer_phase_from_event(tool_name: &str, success: bool) -> Option<CodingPhase> {
    match tool_name {
        "read_file" | "grep" | "glob" | "search_files" => Some(CodingPhase::Inspect),
        "edit" | "write_file" => Some(CodingPhase::Edit),
        "shell" if success && is_test_command(tool_name) => Some(CodingPhase::Verify),
        "self_test" => Some(CodingPhase::Verify),
        _ => None,
    }
}

fn is_test_command(tool_name: &str) -> bool {
    tool_name == "shell" // precise classification belongs to verification::classify
}

/// Phase transition rules.
pub fn validate_transition(
    current: CodingPhase,
    inferred: CodingPhase,
) -> Vec<HarnessFeedback> {
    let mut feedback = Vec::new();
    match (current, inferred) {
        (CodingPhase::Understand, CodingPhase::Edit) => {
            feedback.push(HarnessFeedback {
                source: FeedbackSource::Phase,
                severity: Severity::Warning,
                rule_id: "phase/edit_before_inspect".into(),
                message: "当前还处于 Understand 阶段，没有查阅任何文件就直接修改了。".into(),
                suggestion: "建议先使用 read_file / grep 了解相关代码再做修改。".into(),
                evidence: None,
            });
        }
        (CodingPhase::Understand, CodingPhase::Summarize) => {
            feedback.push(HarnessFeedback {
                source: FeedbackSource::Phase,
                severity: Severity::Fatal,
                rule_id: "phase/summarize_without_work".into(),
                message: "还没有做任何工作就直接总结完成了。".into(),
                suggestion: "请先理解任务需要做什么，然后逐步执行。".into(),
                evidence: None,
            });
        }
        _ => {}
    }
    feedback
}

/// Generate a phase hint for pre_turn injection.
pub fn phase_hint(phase: CodingPhase) -> Option<String> {
    match phase {
        CodingPhase::Understand => Some("先理解任务目标和现有代码结构。"),
        CodingPhase::Inspect => Some("查阅相关文件，了解当前状态。"),
        CodingPhase::Plan => None,
        CodingPhase::Edit => Some("修改文件，确保改动最小。"),
        CodingPhase::Verify => Some("运行测试或编译验证修改是否正确。"),
        CodingPhase::Repair => Some("基于失败原因调整修复策略。"),
        CodingPhase::Summarize => None,
    }
    .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_edit_phase() {
        assert_eq!(
            infer_phase_from_event("edit", true),
            Some(CodingPhase::Edit)
        );
        assert_eq!(
            infer_phase_from_event("write_file", true),
            Some(CodingPhase::Edit)
        );
    }

    #[test]
    fn infer_inspect_phase() {
        assert_eq!(
            infer_phase_from_event("read_file", true),
            Some(CodingPhase::Inspect)
        );
        assert_eq!(
            infer_phase_from_event("grep", true),
            Some(CodingPhase::Inspect)
        );
    }

    #[test]
    fn no_transition_warning_for_normal_flow() {
        let result = validate_transition(CodingPhase::Inspect, CodingPhase::Edit);
        assert!(result.is_empty());
    }

    #[test]
    fn warn_on_edit_before_inspect() {
        let result = validate_transition(CodingPhase::Understand, CodingPhase::Edit);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].severity, Severity::Warning);
    }

    #[test]
    fn block_summarize_without_work() {
        let result = validate_transition(CodingPhase::Understand, CodingPhase::Summarize);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].severity, Severity::Fatal);
    }
}
