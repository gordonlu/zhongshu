use crate::harness::tool::effect::EffectRisk;

pub fn requires_approval(risk: EffectRisk) -> bool {
    matches!(risk, EffectRisk::DestructiveMutation | EffectRisk::ExternalSideEffect | EffectRisk::SensitiveDataAccess)
}

pub fn check_and_set_pending(
    tool_name: &str,
    command: &str,
    risk: EffectRisk,
) -> bool {
    if !requires_approval(risk) {
        return true;
    }
    let result = crate::authority::check(tool_name, command);
    match result {
        crate::authority::CheckResult::Allow => true,
        crate::authority::CheckResult::Deny { reason: _ } => false,
        crate::authority::CheckResult::RequireAuth { request } => {
            crate::authority::set_pending(&request.tool, &request.program, &request.command, "harness");
            false
        }
    }
}
