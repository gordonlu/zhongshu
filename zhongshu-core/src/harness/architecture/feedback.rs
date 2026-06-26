use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
use crate::harness::architecture::config::ArchitectureRule;
use crate::harness::architecture::layer::LayerGraph;

/// Generate architecture boundary hints for pre_turn injection.
pub fn generate_hints(rules: &[ArchitectureRule], _layers: &LayerGraph) -> Vec<HarnessFeedback> {
    let mut hints = Vec::new();
    for rule in rules {
        match rule {
            ArchitectureRule::ForbidDependency {
                name,
                from_layer,
                to_layer,
                ..
            } => {
                hints.push(HarnessFeedback {
                    source: FeedbackSource::Architecture,
                    severity: Severity::Info,
                    rule_id: name.clone(),
                    message: format!("架构约束：{} 层不应依赖 {} 层。", from_layer, to_layer),
                    suggestion: "如需从跨层访问数据，请使用 EventBus 或定义公共接口。".into(),
                    evidence: None,
                });
            }
            _ => {}
        }
    }
    hints
}
