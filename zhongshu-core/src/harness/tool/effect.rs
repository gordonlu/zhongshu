#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolEffect {
    ReadOnly,
    WriteFile,
    DeleteFile,
    RunProcess,
    NetworkAccess,
    BrowserSideEffect,
    SystemMutation,
    SensitiveDataAccess,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectRisk {
    ReadOnly,
    LocalMutation,
    DestructiveMutation,
    ExternalSideEffect,
    SensitiveDataAccess,
}

pub fn classify_effects(tool_name: &str) -> Vec<ToolEffect> {
    match tool_name {
        "read_file" | "grep" | "glob" | "search_files" | "webfetch" | "web_search" => {
            vec![ToolEffect::ReadOnly]
        }
        "write_file" | "edit" => vec![ToolEffect::WriteFile],
        "shell" => vec![ToolEffect::RunProcess],
        "browser" | "browser_automation" => {
            vec![ToolEffect::NetworkAccess, ToolEffect::BrowserSideEffect]
        }
        "screenshot" | "automation" => vec![ToolEffect::SystemMutation],
        "memory_query" => vec![ToolEffect::ReadOnly],
        "goal" | "task" | "suggestion" => vec![ToolEffect::ReadOnly],
        _ => vec![ToolEffect::ReadOnly],
    }
}

pub fn risk_from_effects(effects: &[ToolEffect]) -> EffectRisk {
    if effects.iter().any(|e| matches!(e, ToolEffect::DeleteFile)) {
        return EffectRisk::DestructiveMutation;
    }
    if effects
        .iter()
        .any(|e| matches!(e, ToolEffect::SystemMutation))
    {
        return EffectRisk::DestructiveMutation;
    }
    if effects.iter().any(|e| matches!(e, ToolEffect::WriteFile)) {
        return EffectRisk::LocalMutation;
    }
    if effects
        .iter()
        .any(|e| matches!(e, ToolEffect::NetworkAccess | ToolEffect::BrowserSideEffect))
    {
        return EffectRisk::ExternalSideEffect;
    }
    EffectRisk::ReadOnly
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tools_are_readonly() {
        assert_eq!(classify_effects("read_file"), vec![ToolEffect::ReadOnly]);
    }

    #[test]
    fn edit_is_mutation() {
        assert!(classify_effects("edit").contains(&ToolEffect::WriteFile));
    }

    #[test]
    fn risk_classification() {
        assert_eq!(
            risk_from_effects(&[ToolEffect::WriteFile]),
            EffectRisk::LocalMutation
        );
        assert_eq!(
            risk_from_effects(&[ToolEffect::ReadOnly]),
            EffectRisk::ReadOnly
        );
        assert_eq!(
            risk_from_effects(&[ToolEffect::NetworkAccess]),
            EffectRisk::ExternalSideEffect
        );
    }
}
