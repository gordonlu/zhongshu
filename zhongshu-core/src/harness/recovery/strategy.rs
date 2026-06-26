pub fn hint(repeated_patch: bool, repeated_failure: bool, no_progress: bool) -> Option<String> {
    if repeated_failure && repeated_patch {
        Some("同一个错误反复出现且 patch 高度相似，建议：先分析错误的根本原因，而不是继续修改同一个函数。".into())
    } else if no_progress {
        Some("连续多轮没有取得进展，建议：重新梳理任务目标，确认当前的修改方向是否正确。".into())
    } else if repeated_failure {
        Some(
            "同一个测试持续失败，建议：先检查测试预期和测试环境，而不是继续修改被测试的代码。"
                .into(),
        )
    } else {
        None
    }
}
