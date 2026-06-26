use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};

pub fn render_feedback(fb: &HarnessFeedback) -> String {
    let severity_tag = match fb.severity {
        Severity::Info => "[信息]",
        Severity::Warning => "[注意]",
        Severity::Fatal => "[约束]",
        Severity::BlockTool => "[约束]",
    };
    let source_tag = match fb.source {
        FeedbackSource::Architecture => "架构",
        FeedbackSource::Verification => "验证",
        FeedbackSource::Recovery => "纠偏",
        FeedbackSource::ToolLoop => "工具",
        FeedbackSource::Phase => "阶段",
    };
    let mut out = format!("{severity_tag} [{source_tag}] {}", fb.message);
    if !fb.suggestion.is_empty() {
        out.push_str(&format!("\n建议：{}", fb.suggestion));
    }
    if let Some(ref ev) = fb.evidence {
        out.push_str(&format!("\n证据：{}", ev));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};

    #[test]
    fn render_info_message() {
        let fb = HarnessFeedback {
            source: FeedbackSource::Architecture,
            severity: Severity::Warning,
            rule_id: "arch/test".into(),
            message: "测试消息".into(),
            suggestion: "测试建议".into(),
            evidence: None,
        };
        let rendered = render_feedback(&fb);
        assert!(rendered.contains("测试消息"));
        assert!(rendered.contains("测试建议"));
    }

    #[test]
    fn render_supports_fatal() {
        let fb = HarnessFeedback {
            source: FeedbackSource::Verification,
            severity: Severity::Fatal,
            rule_id: "ver/test".into(),
            message: "未验证".into(),
            suggestion: "请运行测试".into(),
            evidence: Some("exit code 1".into()),
        };
        let rendered = render_feedback(&fb);
        assert!(rendered.contains("[约束]"));
        assert!(rendered.contains("未验证"));
        assert!(rendered.contains("exit code 1"));
    }
}
