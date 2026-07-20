#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    CompletedVerified,
    CompletedUnverified,
    Failed,
}

pub fn accepted_for_merge(outcome: RunOutcome) -> bool {
    matches!(
        outcome,
        RunOutcome::CompletedVerified | RunOutcome::CompletedUnverified
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_work_is_accepted() {
        assert!(accepted_for_merge(RunOutcome::CompletedVerified));
    }

    #[test]
    fn failed_work_is_rejected() {
        assert!(!accepted_for_merge(RunOutcome::Failed));
    }
}
