use crate::harness::architecture::config::ArchitectureRule;
use crate::harness::architecture::index::ProjectIndex;
use crate::harness::architecture::layer::LayerGraph;
use crate::harness::architecture::diff::AstChange;
use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
use crate::harness::state::{OpenViolation, ViolationKey};

/// Evaluate rules against the current index and recent changes.
/// Returns feedback for immediate injection and violation records for state tracking.
pub fn evaluate_rules(
    rules: &[ArchitectureRule],
    index: &ProjectIndex,
    layers: &LayerGraph,
    changes: &[AstChange],
    existing_violations: &[OpenViolation],
) -> (Vec<HarnessFeedback>, Vec<OpenViolation>) {
    let mut feedback = Vec::new();
    let mut new_violations = Vec::new();

    for rule in rules {
        match rule {
            ArchitectureRule::ForbidDependency { name, from_layer, to_layer, severity } => {
                check_forbid_dependency(name, from_layer, to_layer, *severity, index, layers, &mut feedback, &mut new_violations, existing_violations);
            }
            ArchitectureRule::RequireSymbolWhenTouching { name, file_globs, required_symbols, severity } => {
                check_require_symbol(name, file_globs, required_symbols, *severity, changes, &mut feedback, &mut new_violations, existing_violations);
            }
            _ => {}
        }
    }

    (feedback, new_violations)
}

fn check_forbid_dependency(
    name: &str, from: &str, to: &str, severity: Severity,
    index: &ProjectIndex, layers: &LayerGraph,
    feedback: &mut Vec<HarnessFeedback>,
    new_violations: &mut Vec<OpenViolation>,
    existing: &[OpenViolation],
) {
    for (path, file_index) in &index.files {
        let file_layer = match layers.layer_for(path) {
            Some(l) => l,
            None => continue,
        };
        if file_layer != from {
            continue;
        }
        for import in &file_index.imports {
            let import_lower = import.to_lowercase();
            let to_layer_lower = to.replace('-', "_").to_lowercase();
            let to_as_path = to.replace('_', "::").to_lowercase();
            if import_lower.contains(&to_layer_lower) || import_lower.contains(&to_as_path) {
                let key = ViolationKey {
                    rule_id: name.into(),
                    file_path: path.clone(),
                    symbol_id: import.clone(),
                };
                if crate::harness::architecture::violation::dedup_violations(existing, &key.clone().into()) {
                    continue;
                }
                feedback.push(HarnessFeedback {
                    source: FeedbackSource::Architecture,
                    severity,
                    rule_id: name.into(),
                    message: format!("{} 层导入了 {} 层的符号：{}", from, to, import),
                    suggestion: format!("{} 层不应直接依赖 {} 层。应该通过核心 EventBus 或接口暴露。", from, to),
                    evidence: Some(format!("文件：{}", path.display())),
                });
                new_violations.push(OpenViolation {
                    key,
                    status: crate::harness::state::ViolationStatus::Open,
                    severity,
                    message: format!("forbidden dependency: {} → {}", from, to),
                    raised_step: 0,
                });
            }
        }
    }
}

fn check_require_symbol(
    name: &str, file_globs: &[globset::GlobMatcher], required_symbols: &[String],
    severity: Severity, changes: &[AstChange],
    feedback: &mut Vec<HarnessFeedback>,
    new_violations: &mut Vec<OpenViolation>,
    existing: &[OpenViolation],
) {
    for change in changes {
        let changed_file = match change {
            AstChange::FunctionAdded { .. } | AstChange::FunctionRemoved { .. } | AstChange::FunctionBodyChanged { .. } => continue,
            AstChange::ImportAdded { file, .. } | AstChange::ImportRemoved { file, .. } => file,
            AstChange::FunctionSignatureChanged { .. } => continue,
        };
        if !file_globs.iter().any(|g| g.is_match(changed_file)) {
            continue;
        }
        let content = match std::fs::read_to_string(changed_file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let missing: Vec<&String> = required_symbols.iter().filter(|sym| !content.contains(sym.as_str())).collect();
        if missing.is_empty() {
            continue;
        }
        let key = ViolationKey {
            rule_id: name.into(),
            file_path: changed_file.clone(),
            symbol_id: required_symbols.join(","),
        };
        if crate::harness::architecture::violation::dedup_violations(existing, &key.clone().into()) {
            continue;
        }
        feedback.push(HarnessFeedback {
            source: FeedbackSource::Architecture,
            severity,
            rule_id: name.into(),
            message: format!("修改了需要 EventBus 的文件，但缺少 {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
            suggestion: "任务/目标状态变化应通过 EventBus 发布事件，而非直接操作数据库。".into(),
            evidence: Some(format!("文件：{}", changed_file.display())),
        });
        new_violations.push(OpenViolation {
            key,
            status: crate::harness::state::ViolationStatus::Open,
            severity,
            message: format!("missing required symbols: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
            raised_step: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::harness::architecture::index::FileIndex;

    #[test]
    fn forbid_dependency_detects_violation() {
        let mut index = ProjectIndex::new(PathBuf::from("."));
        let path = PathBuf::from("zhongshu-orb/src/app.rs");
        index.files.insert(path.clone(), FileIndex {
            path: path.clone(),
            imports: vec!["zhongshu_core::db::TaskRepo".into()],
            items: vec![],
            parse_error: None,
        });

        let mut layers = LayerGraph::default();
        layers.add_layer("core_db", "zhongshu-core/src/db/**/*.rs");

        let rules = vec![
            ArchitectureRule::ForbidDependency {
                name: "test".into(),
                from_layer: "orb".into(),
                to_layer: "core_db".into(),
                severity: Severity::Fatal,
            },
        ];

        let (fb, violations) = evaluate_rules(&rules, &index, &layers, &[], &[]);
        assert!(!fb.is_empty(), "should detect orb importing core_db");
        assert!(!violations.is_empty());
    }

    #[test]
    fn forbid_dependency_skips_same_layer() {
        let mut index = ProjectIndex::new(PathBuf::from("."));
        let path = PathBuf::from("zhongshu-core/src/app.rs");
        index.files.insert(path.clone(), FileIndex {
            path,
            imports: vec!["zhongshu_orb::Overlay".into()],
            items: vec![],
            parse_error: None,
        });

        let mut layers = LayerGraph::default();
        layers.add_layer("core_db", "zhongshu-core/src/db/**/*.rs");

        let rules = vec![
            ArchitectureRule::ForbidDependency {
                name: "test".into(),
                from_layer: "orb".into(),
                to_layer: "core_db".into(),
                severity: Severity::Fatal,
            },
        ];

        let (fb, _) = evaluate_rules(&rules, &index, &layers, &[], &[]);
        assert!(fb.is_empty(), "orb rule should not match core files");
    }
}
