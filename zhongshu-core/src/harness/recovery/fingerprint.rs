use crate::harness::state::{FailureSignature, RecoveryState};

pub fn fingerprint(command: &str, output: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    command.hash(&mut hasher);
    output
        .chars()
        .take(200)
        .collect::<String>()
        .hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub fn record(state: &mut RecoveryState, _tool_name: &str, command: &str, output: &str, step: u32) {
    let fp = fingerprint(command, output);
    if let Some(existing) = state.failures.iter_mut().find(|f| f.command_hash == fp) {
        existing.count += 1;
    } else {
        state.failures.push(FailureSignature {
            command_hash: fp.clone(),
            error_fingerprint: fp,
            count: 1,
            first_seen_step: step,
        });
    }
}

pub fn is_repeated_failure(state: &RecoveryState, command: &str, output: &str) -> bool {
    let fp = fingerprint(command, output);
    state
        .failures
        .iter()
        .any(|f| f.command_hash == fp && f.count >= 3)
}
