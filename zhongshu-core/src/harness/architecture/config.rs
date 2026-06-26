use crate::harness::action::Severity;

/// Architecture rules for the zhongshu project.
#[derive(Debug, Clone)]
pub enum ArchitectureRule {
    ForbidDependency {
        name: String,
        from_layer: String,
        to_layer: String,
        severity: Severity,
    },
    RequireSymbolWhenTouching {
        name: String,
        file_globs: Vec<globset::GlobMatcher>,
        required_symbols: Vec<String>,
        severity: Severity,
    },
    ForbidPublicApiBreak {
        name: String,
        severity: Severity,
    },
    SemanticRule {
        name: String,
        matcher: SemanticMatcher,
        severity: Severity,
    },
}

#[derive(Debug, Clone)]
pub enum SemanticMatcher {
    ForbidIgnoredResult {
        file_globs: Vec<globset::GlobMatcher>,
        callee_contains: Vec<String>,
    },
    AllowIgnoredResult {
        callee_contains: Vec<String>,
        reason: String,
    },
    ForbidPattern {
        file_globs: Vec<globset::GlobMatcher>,
        patterns: Vec<String>,
    },
}

/// Build the default rule set for the zhongshu project.
pub fn default_rules() -> Vec<ArchitectureRule> {
    vec![
        ArchitectureRule::ForbidDependency {
            name: "orb_must_not_depend_on_core_db".into(),
            from_layer: "orb".into(),
            to_layer: "core_db".into(),
            severity: Severity::Fatal,
        },
        ArchitectureRule::ForbidDependency {
            name: "core_must_not_depend_on_orb".into(),
            from_layer: "core".into(),
            to_layer: "orb".into(),
            severity: Severity::Fatal,
        },
    ]
}

pub fn glob_matcher(pattern: &str) -> globset::GlobMatcher {
    globset::Glob::new(pattern).expect("invalid architecture rule glob pattern").compile_matcher()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_has_both_layer_rules() {
        let rules = default_rules();
        assert!(rules.len() >= 2);
    }
}
