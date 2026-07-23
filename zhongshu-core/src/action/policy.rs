use crate::tool::{infer_replay_policy, ReplayPolicy};

#[derive(Debug, Clone)]
pub struct ActionPolicy {
    pub replay_policy: ReplayPolicy,
    pub requires_approval: bool,
}

impl ActionPolicy {
    pub fn from_tool(tool_name: &str) -> Self {
        Self {
            replay_policy: infer_replay_policy(tool_name),
            requires_approval: false,
        }
    }

    pub fn always_require_approval(mut self) -> Self {
        self.requires_approval = true;
        self
    }

    pub fn should_skip_if_completed(&self) -> bool {
        !matches!(self.replay_policy, ReplayPolicy::AlwaysExecute)
    }
}
