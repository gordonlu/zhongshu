use super::claim::{has_verification_claim, is_explicitly_unverified};
use crate::harness::action::{FeedbackSource, HarnessAction, HarnessFeedback, Severity};
use crate::harness::state::VerificationState;

pub fn check(state: &VerificationState, output: &str) -> Vec<HarnessAction> {
    let mut actions = Vec::new();
    let claims_verified = has_verification_claim(output);
    let explicitly_unverified = is_explicitly_unverified(output);

    if state.required && state.last_success.is_none() {
        actions.push(HarnessAction::BlockFinalize {
            feedback: HarnessFeedback {
                source: FeedbackSource::Verification,
                severity: Severity::Fatal,
                rule_id: "ver/required_not_run".into(),
                message: "用户要求验证，但没有运行任何测试。".into(),
                suggestion: "请先运行 cargo test 或相应的验证命令。".into(),
                evidence: None,
            },
        });
        return actions;
    }

    if claims_verified && state.last_success.is_none() {
        actions.push(HarnessAction::BlockFinalize {
            feedback: HarnessFeedback {
                source: FeedbackSource::Verification,
                severity: Severity::Fatal,
                rule_id: "ver/claim_without_evidence".into(),
                message: "输出声称了测试通过，但没有实际的测试执行记录。".into(),
                suggestion: "请先运行测试，获得真实的测试结果后再总结。".into(),
                evidence: None,
            },
        });
        return actions;
    }

    // If the output explicitly says "unverified", don't enforce stale check.
    // The agent is being honest about not having verified.
    if explicitly_unverified {
        return actions;
    }

    if state.last_verify_step <= state.last_edit_step && state.last_edit_step > 0 {
        actions.push(HarnessAction::BlockFinalize {
            feedback: HarnessFeedback {
                source: FeedbackSource::Verification,
                severity: Severity::Fatal,
                rule_id: "ver/stale_verification".into(),
                message: "最后一次修改后没有重新验证。".into(),
                suggestion: "代码已修改，需要重新运行测试确认修改正确。".into(),
                evidence: None,
            },
        });
        return actions;
    }

    if let Some(ref fail) = state.last_failure {
        if state.last_success.as_ref().map(|s| s.step).unwrap_or(0) < fail.step {
            actions.push(HarnessAction::BlockFinalize {
                feedback: HarnessFeedback {
                    source: FeedbackSource::Verification,
                    severity: Severity::Fatal,
                    rule_id: "ver/last_run_failed".into(),
                    message: "最后一次测试运行失败。".into(),
                    suggestion: format!("失败命令：{}。请修复问题后重新运行测试。", fail.command),
                    evidence: None,
                },
            });
            return actions;
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(
        last_success_step: u32,
        last_fail_step: Option<u32>,
        last_edit: u32,
        last_verify: u32,
    ) -> VerificationState {
        let mut state = VerificationState {
            required: false,
            records: Vec::new(),
            last_success: None,
            last_failure: None,
            last_edit_step: last_edit,
            last_verify_step: last_verify,
            unavailable_reason: None,
        };
        if last_success_step > 0 {
            state.last_success = Some(crate::harness::state::VerificationRecord {
                command: "cargo test".into(),
                command_hash: "abc".into(),
                success: true,
                exit_code: Some(0),
                step: last_success_step,
            });
        }
        if let Some(fs) = last_fail_step {
            state.last_failure = Some(crate::harness::state::VerificationRecord {
                command: "cargo test".into(),
                command_hash: "def".into(),
                success: false,
                exit_code: Some(1),
                step: fs,
            });
        }
        state
    }

    #[test]
    fn blocks_fake_claim() {
        let state = make_state(0, None, 0, 0);
        let actions = check(&state, "已完成修改，测试通过");
        assert!(actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })));
    }

    #[test]
    fn blocks_stale_verification() {
        let state = make_state(1, None, 3, 1);
        let actions = check(&state, "已完成修改");
        assert!(actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })));
    }

    #[test]
    fn blocks_first_edit_without_verification() {
        let state = make_state(0, None, 1, 0);
        let actions = check(&state, "done");
        assert!(actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })));
    }

    #[test]
    fn allows_fresh_verification() {
        let state = make_state(3, None, 2, 3);
        let actions = check(&state, "已完成修改");
        assert!(
            actions.is_empty()
                || actions
                    .iter()
                    .all(|a| !matches!(a, HarnessAction::BlockFinalize { .. }))
        );
    }

    #[test]
    fn blocks_failed_last_run() {
        let state = make_state(0, Some(3), 2, 3);
        let actions = check(&state, "已完成修改");
        assert!(actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })));
    }
}
