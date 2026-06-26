pub fn has_verification_claim(output: &str) -> bool {
    let claims = [
        "测试通过", "验证通过", "测试成功", "验证成功",
        "全部通过", "运行测试", "测试全部通过",
        "tests passed", "test passed", "all tests pass",
        "verification passed", "verified",
    ];
    let lower = output.to_lowercase();
    claims.iter().any(|c| lower.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_chinese_claim() {
        assert!(has_verification_claim("修改完成，测试通过。"));
        assert!(has_verification_claim("验证通过，可以提交。"));
    }

    #[test]
    fn detects_english_claim() {
        assert!(has_verification_claim("All tests passed."));
        assert!(has_verification_claim("verification passed"));
    }

    #[test]
    fn no_false_positive() {
        assert!(!has_verification_claim("正在分析代码结构"));
    }
}
