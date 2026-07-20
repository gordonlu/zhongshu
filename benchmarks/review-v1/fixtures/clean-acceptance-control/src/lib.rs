#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    CompletedVerified,
    CompletedUnverified,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadDecision {
    Accepted,
    Submitted,
    Rejected,
}

pub fn decide(outcome: RunOutcome) -> LeadDecision {
    match outcome {
        RunOutcome::CompletedVerified => LeadDecision::Accepted,
        RunOutcome::CompletedUnverified => LeadDecision::Submitted,
        RunOutcome::Failed => LeadDecision::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_work_is_accepted() {
        assert_eq!(decide(RunOutcome::CompletedVerified), LeadDecision::Accepted);
        assert_eq!(
            decide(RunOutcome::CompletedUnverified),
            LeadDecision::Submitted
        );
        assert_eq!(decide(RunOutcome::Failed), LeadDecision::Rejected);
    }
}
