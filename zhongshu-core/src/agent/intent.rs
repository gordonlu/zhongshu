/// Lightweight keyword-based intent classification for user interjections.
/// No LLM dependency — pure pattern matching on normalized text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionIntent {
    /// "停", "别继续", "先到这里"
    Stop,
    /// "不是这个意思", "换个方向", "先别写代码"
    Redirect,
    /// "不要用数据库", "必须兼容 Windows"
    Constraint,
    /// "这个文件不能改", "只允许读"
    ApprovalCorrection,
    /// "你现在做到哪了", "查到哪一步了"
    ProgressAsk,
    /// "继续", "接着来"
    Continue,
    /// 普通补充
    Other,
}

pub fn intent_classify(text: &str) -> InterruptionIntent {
    let t = text.trim().to_lowercase();

    // Stop
    if matches_keywords(&t, &["停", "停止", "停下", "别继续", "先到这里", "不要再做了", "stop", "cancel", "halt", "abort", "别说了", "别讲了", "够了"])
        || t.len() <= 2 && matches_keywords(&t, &["停", "不"])
    {
        return InterruptionIntent::Stop;
    }

    // Continue
    if matches_keywords(&t, &["继续", "接着来", "继续吧", "接着说", "接着讲", "请继续", "continue", "go on", "carry on", "resume", "proceed"])
    {
        return InterruptionIntent::Continue;
    }

    // Progress ask
    if matches_keywords(&t, &["做到哪", "查到哪", "到什么", "进度", "进展", "怎么样了", "什么情况", "status", "progress", "where are you", "how far", "完成多少"])
    {
        return InterruptionIntent::ProgressAsk;
    }

    // Redirect
    if matches_keywords(&t, &["不是这个意思", "换个方向", "你理解错了", "不对", "不是这样", "方向不对", "换个思路", "先别写代码", "先讲方案", "先不说", "另一个方向", "换一个", "换种方式", "换个角度"])
    {
        return InterruptionIntent::Redirect;
    }

    // Constraint
    if matches_keywords(&t, &["不要用", "不能用", "必须", "不允许", "禁止", "别用", "限制", "约束", "条件", "注意", "要求", "改成", "改为", "改用", "用不了", "不支持"])
    {
        return InterruptionIntent::Constraint;
    }

    // Approval correction
    if matches_keywords(&t, &["不能改", "不能写", "只能读", "不可以", "不允许改", "这个文件", "那个文件", "不能动", "只允许", "不能删除", "不能修改"])
    {
        return InterruptionIntent::ApprovalCorrection;
    }

    InterruptionIntent::Other
}

fn matches_keywords(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|kw| text.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stop_intents() {
        assert_eq!(intent_classify("停"), InterruptionIntent::Stop);
        assert_eq!(intent_classify("stop"), InterruptionIntent::Stop);
        assert_eq!(intent_classify("别继续了"), InterruptionIntent::Stop);
        assert_eq!(intent_classify("先到这里吧"), InterruptionIntent::Stop);
    }

    #[test]
    fn test_continue_intents() {
        assert_eq!(intent_classify("继续"), InterruptionIntent::Continue);
        assert_eq!(intent_classify("接着来"), InterruptionIntent::Continue);
        assert_eq!(intent_classify("continue"), InterruptionIntent::Continue);
    }

    #[test]
    fn test_progress_ask() {
        assert_eq!(intent_classify("你现在做到哪了"), InterruptionIntent::ProgressAsk);
        assert_eq!(intent_classify("进度怎么样了"), InterruptionIntent::ProgressAsk);
        assert_eq!(intent_classify("status"), InterruptionIntent::ProgressAsk);
    }

    #[test]
    fn test_redirect_intents() {
        assert_eq!(intent_classify("不是这个意思"), InterruptionIntent::Redirect);
        assert_eq!(intent_classify("换个方向"), InterruptionIntent::Redirect);
        assert_eq!(intent_classify("先别写代码"), InterruptionIntent::Redirect);
    }

    #[test]
    fn test_constraint_intents() {
        assert_eq!(intent_classify("不要用数据库"), InterruptionIntent::Constraint);
        assert_eq!(intent_classify("必须兼容 Windows"), InterruptionIntent::Constraint);
    }

    #[test]
    fn test_approval_correction() {
        assert_eq!(intent_classify("这个文件不能改"), InterruptionIntent::ApprovalCorrection);
        assert_eq!(intent_classify("只能读不能写"), InterruptionIntent::ApprovalCorrection);
    }

    #[test]
    fn test_other_fallback() {
        assert_eq!(intent_classify("你好"), InterruptionIntent::Other);
        assert_eq!(intent_classify("我想问个问题"), InterruptionIntent::Other);
    }
}
