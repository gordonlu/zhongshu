/// Positive indicator patterns — output contains a claim of verification.
const POSITIVE_CLAIMS: &[&str] = &[
    "测试通过",
    "验证通过",
    "测试成功",
    "验证成功",
    "全部通过",
    "运行测试",
    "测试全部通过",
    "已通过测试",
    "通过所有测试",
    "已测试通过",
    "编译通过",
    "构建通过",
    "tests passed",
    "test passed",
    "all tests pass",
    "verification passed",
    "verified",
];

/// Negative indicator patterns — output explicitly denies verification.
/// If any of these appear, the output is NOT a claim of verification.
const NEGATIVE_CLAIMS: &[&str] = &[
    "未验证",
    "无法验证",
    "未测试",
    "没有验证",
    "没有测试",
    "测试还没",
    "还没有测试",
    "无法确认是否通过",
    "不能确认",
];

/// Strip negated claims: if "没有" appears within 20 chars before a
/// positive claim keyword, the sentence is negated.
fn has_negated(output: &str, positive_keyword: &str) -> bool {
    let idx = match output.find(positive_keyword) {
        Some(i) => i,
        None => return false,
    };
    let before = &output[..idx];
    // Check for negation keywords in the preceding 20 characters
    let window = if before.len() > 20 { &before[before.len() - 20..] } else { before };
    window.contains("没有") || window.contains("没") || window.contains("未")
}

pub fn has_verification_claim(output: &str) -> bool {
    let lower = output.to_lowercase();

    // Explicit negative patterns override everything
    if NEGATIVE_CLAIMS.iter().any(|n| lower.contains(n)) {
        return false;
    }

    // Positive patterns must not be negated
    POSITIVE_CLAIMS.iter().any(|c| {
        lower.contains(c) && !has_negated(&lower, c)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Positive cases ─────────────────────────────────────────────

    #[test]
    fn detects_chinese_claim() {
        assert!(has_verification_claim("修改完成，测试通过。"));
        assert!(has_verification_claim("验证通过，可以提交。"));
        assert!(has_verification_claim("编译通过"));
        assert!(has_verification_claim("构建通过，可以提交"));
        assert!(has_verification_claim("已通过所有测试"));
        assert!(has_verification_claim("我跑过测试，全部通过"));
    }

    #[test]
    fn detects_english_claim() {
        assert!(has_verification_claim("All tests passed."));
        assert!(has_verification_claim("verification passed"));
        assert!(has_verification_claim("Fix verified"));
    }

    // ── Negative cases ─────────────────────────────────────────────

    #[test]
    fn explicit_denial_not_claim() {
        assert!(!has_verification_claim("未验证"));
        assert!(!has_verification_claim("无法验证是否通过"));
        assert!(!has_verification_claim("未测试"));
    }

    #[test]
    fn negated_claim_not_false_positive() {
        // The critical case: "测试通过" appears but is negated
        assert!(!has_verification_claim("我没有声称测试通过"));
        assert!(!has_verification_claim("没有测试通过的证据"));
    }

    #[test]
    fn no_false_positive() {
        assert!(!has_verification_claim("正在分析代码结构"));
        assert!(!has_verification_claim("测试还没跑"));
        assert!(!has_verification_claim("无法确认是否通过"));
        assert!(!has_verification_claim("还没有测试"));
    }
}
