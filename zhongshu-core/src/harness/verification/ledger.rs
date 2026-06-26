use super::classify::VerificationType;
use crate::harness::state::VerificationState;

pub fn record(
    state: &mut VerificationState,
    tool_name: &str,
    command: &str,
    exit_code: Option<i32>,
    step: u32,
) {
    let vtype = if tool_name == "self_test" {
        VerificationType::Test
    } else if tool_name == "shell" {
        super::classify::classify_command(command)
    } else {
        return;
    };

    if vtype == VerificationType::Unknown {
        return;
    }

    let record = crate::harness::state::VerificationRecord {
        command: command.to_string(),
        command_hash: simple_hash(command),
        success: exit_code.map(|c| c == 0).unwrap_or(false),
        exit_code,
        step,
    };

    state.records.push(record.clone());
    if record.success {
        state.last_success = Some(record);
    } else {
        state.last_failure = Some(record);
    }
    state.last_verify_step = step;
}

fn simple_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_test_success() {
        let mut state = VerificationState {
            required: false,
            records: Vec::new(),
            last_success: None,
            last_failure: None,
            last_edit_step: 0,
            last_verify_step: 0,
            unavailable_reason: None,
        };
        record(&mut state, "shell", "cargo test", Some(0), 1);
        assert!(state.last_success.is_some());
        assert_eq!(state.last_verify_step, 1);
    }

    #[test]
    fn ignores_non_verification_tool() {
        let mut state = VerificationState {
            required: false,
            records: Vec::new(),
            last_success: None,
            last_failure: None,
            last_edit_step: 0,
            last_verify_step: 0,
            unavailable_reason: None,
        };
        record(&mut state, "shell", "ls -la", Some(0), 1);
        assert!(state.records.is_empty());
    }
}
