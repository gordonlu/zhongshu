use crate::harness::action::{FeedbackSource, HarnessAction, HarnessFeedback, Severity};
use crate::harness::state::ToolCallFingerprint;

const MAX_HISTORY: usize = 20;
const CONSECUTIVE_LIMIT: u32 = 3;
const TOTAL_LIMIT: u32 = 5;

pub fn check_duplicate(
    state: &mut crate::harness::state::ToolLoopState,
    tool_name: &str,
    args_hash: &str,
) -> HarnessAction {
    let fp = ToolCallFingerprint {
        tool_name: tool_name.to_string(),
        args_hash: args_hash.to_string(),
    };

    state.recent_calls.push_back(fp.clone());
    if state.recent_calls.len() > MAX_HISTORY {
        state.recent_calls.pop_front();
    }

    let count = state.counts.entry(fp.clone()).or_insert(0);
    *count += 1;

    let consecutive = state
        .recent_calls
        .iter()
        .rev()
        .take_while(|c| **c == fp)
        .count() as u32;

    if consecutive >= CONSECUTIVE_LIMIT {
        return HarnessAction::BlockTool {
            feedback: HarnessFeedback {
                source: FeedbackSource::ToolLoop,
                severity: Severity::BlockTool,
                rule_id: "tool/loop_consecutive".into(),
                message: format!(
                    "连续 {} 次调用同一工具 ({}), 参数相同。",
                    consecutive, tool_name
                ),
                suggestion: "先检查之前的调用结果，或者换一种方式获取信息。".into(),
                evidence: None,
            },
        };
    }

    if *count >= TOTAL_LIMIT {
        return HarnessAction::BlockTool {
            feedback: HarnessFeedback {
                source: FeedbackSource::ToolLoop,
                severity: Severity::BlockTool,
                rule_id: "tool/loop_total".into(),
                message: format!("{} 已调用 {} 次，已达上限。", tool_name, *count),
                suggestion: "请先阅读已有的信息，不要重复查询。".into(),
                evidence: None,
            },
        };
    }

    HarnessAction::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::state::ToolLoopState;
    use std::collections::{HashMap, VecDeque};

    #[test]
    fn blocks_after_3_consecutive() {
        let mut state = ToolLoopState {
            recent_calls: VecDeque::new(),
            counts: HashMap::new(),
        };
        for _ in 0..2 {
            let result = check_duplicate(&mut state, "grep", "hash1");
            assert!(matches!(result, HarnessAction::None));
        }
        let result = check_duplicate(&mut state, "grep", "hash1");
        assert!(matches!(result, HarnessAction::BlockTool { .. }));
    }

    #[test]
    fn allows_different_args() {
        let mut state = ToolLoopState {
            recent_calls: VecDeque::new(),
            counts: HashMap::new(),
        };
        for i in 0..5 {
            let result = check_duplicate(&mut state, "grep", &format!("hash{}", i));
            assert!(matches!(result, HarnessAction::None));
        }
    }
}
