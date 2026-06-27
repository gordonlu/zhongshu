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
    // Take up to the last 20 characters before the keyword (char-based, not byte-based)
    let before = &output[..idx];
    let before_chars: Vec<char> = before.chars().collect();
    let window: String = if before_chars.len() > 20 {
        before_chars[before_chars.len() - 20..].iter().collect()
    } else {
        before_chars.iter().collect()
    };
    window.contains("没有") || window.contains("没") || window.contains("未")
}

/// Check whether the output explicitly states that verification was NOT done.
pub fn is_explicitly_unverified(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("未运行测试")
        || lower.contains("unverified")
        || lower.contains("not tested")
        || lower.contains("not verified")
}

pub fn has_verification_claim(output: &str) -> bool {
    let lower = output.to_lowercase();

    // Explicit negative patterns override everything
    if NEGATIVE_CLAIMS.iter().any(|n| lower.contains(n)) {
        return false;
    }

    // Positive patterns must not be negated
    POSITIVE_CLAIMS
        .iter()
        .any(|c| lower.contains(c) && !has_negated(&lower, c))
}

/// Check whether user input asks the agent to verify before completing.
pub fn requests_verification(input: &str) -> bool {
    let lower = input.to_lowercase();
    [
        "run test",
        "run tests",
        "test before",
        "make sure tests",
        "verify",
        "verification",
        "cargo test",
        "cargo check",
        "pytest",
        "npm test",
        "go test",
        "\u{6d4b}\u{8bd5}",
        "\u{9a8c}\u{8bc1}",
        "\u{8dd1}\u{6d4b}",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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

    #[test]
    fn detects_user_verification_request() {
        assert!(requests_verification("please run tests before finishing"));
        assert!(requests_verification("cargo test after the fix"));
        assert!(requests_verification(
            "\u{4fee}\u{590d}\u{540e}\u{8dd1}\u{6d4b}"
        ));
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
