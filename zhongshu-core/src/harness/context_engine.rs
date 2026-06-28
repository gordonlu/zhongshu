use serde::{Deserialize, Serialize};

use crate::coding::CodingRuntimeLink;
use crate::core::context::{
    ContextBuildError, ContextPack, ContextPackBuilder, ContextPackReport, EvidenceBlock,
    RecentUnit, StateBlock,
};
use crate::harness::recovery::policy::RecoverySignal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEngineConfig {
    pub max_context_tokens: usize,
    pub pressure_threshold_percent: u8,
}

impl Default for ContextEngineConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 64_000,
            pressure_threshold_percent: 85,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRuntimeLink {
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
}

impl From<CodingRuntimeLink> for ContextRuntimeLink {
    fn from(value: CodingRuntimeLink) -> Self {
        Self {
            deeplossless_conversation_id: value.deeplossless_conversation_id,
            deeplossless_replay_execution_id: value.deeplossless_replay_execution_id,
        }
    }
}

impl From<&CodingRuntimeLink> for ContextRuntimeLink {
    fn from(value: &CodingRuntimeLink) -> Self {
        Self {
            deeplossless_conversation_id: value.deeplossless_conversation_id,
            deeplossless_replay_execution_id: value.deeplossless_replay_execution_id.clone(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ContextPipelineInput {
    pub stable_system: String,
    pub state: Option<StateBlock>,
    pub evidence: Vec<EvidenceBlock>,
    pub recent: Vec<RecentUnit>,
    pub user_input: String,
    pub runtime_link: ContextRuntimeLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPipelineReport {
    pub max_context_tokens: usize,
    pub pressure_threshold_percent: u8,
    pub pressure_percent: u8,
    pub under_pressure: bool,
    pub stable_prefix_hash: String,
    pub dropped_evidence_ids: Vec<String>,
    pub dropped_recent_units: usize,
    pub warning_count: usize,
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
}

impl ContextPipelineReport {
    pub fn recovery_signal(&self) -> Option<RecoverySignal> {
        if !self.under_pressure {
            return None;
        }

        Some(RecoverySignal::context_pressure(format!(
            "context pressure {}% of {} tokens; dropped {} evidence blocks and {} recent units",
            self.pressure_percent,
            self.max_context_tokens,
            self.dropped_evidence_ids.len(),
            self.dropped_recent_units
        )))
    }
}

#[derive(Debug, Clone)]
pub struct ContextPipelineOutput {
    pub pack: ContextPack,
    pub report: ContextPipelineReport,
    pub pack_report: ContextPackReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextEngineError {
    BuildFailed(String),
}

#[derive(Debug, Clone)]
pub struct ContextEngine {
    config: ContextEngineConfig,
}

impl ContextEngine {
    pub fn new(config: ContextEngineConfig) -> Self {
        Self { config }
    }

    pub fn build(
        &self,
        input: ContextPipelineInput,
    ) -> Result<ContextPipelineOutput, ContextEngineError> {
        let builder = ContextPackBuilder::new()
            .stable_system(input.stable_system)
            .with_evidence(input.evidence)
            .with_recent(input.recent)
            .input(input.user_input);
        let builder = if let Some(state) = input.state {
            builder.state(state)
        } else {
            builder
        };

        let (pack, pack_report) = builder
            .build(self.config.max_context_tokens)
            .map_err(ContextEngineError::from)?;
        let pressure_percent =
            pressure_percent(pack_report.total_tokens, self.config.max_context_tokens);
        let report = ContextPipelineReport {
            max_context_tokens: self.config.max_context_tokens,
            pressure_threshold_percent: self.config.pressure_threshold_percent,
            pressure_percent,
            under_pressure: pressure_percent >= self.config.pressure_threshold_percent,
            stable_prefix_hash: pack_report.stable_prefix_hash.clone(),
            dropped_evidence_ids: pack_report.dropped_evidence_ids.clone(),
            dropped_recent_units: pack_report.dropped_recent_units,
            warning_count: pack_report.warnings.len(),
            deeplossless_conversation_id: input.runtime_link.deeplossless_conversation_id,
            deeplossless_replay_execution_id: input.runtime_link.deeplossless_replay_execution_id,
        };

        Ok(ContextPipelineOutput {
            pack,
            report,
            pack_report,
        })
    }
}

impl Default for ContextEngine {
    fn default() -> Self {
        Self::new(ContextEngineConfig::default())
    }
}

impl From<ContextBuildError> for ContextEngineError {
    fn from(value: ContextBuildError) -> Self {
        match value {
            ContextBuildError::StableSystemTooLong(tokens) => Self::BuildFailed(format!(
                "stable system exceeds context cap: {tokens} tokens"
            )),
            ContextBuildError::ContextTooLong { total, limit } => {
                Self::BuildFailed(format!("context too long: {total}/{limit} tokens"))
            }
        }
    }
}

fn pressure_percent(total_tokens: usize, max_context_tokens: usize) -> u8 {
    if max_context_tokens == 0 {
        return 100;
    }
    ((total_tokens.saturating_mul(100)) / max_context_tokens).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::{
        ContextMessage, ContextRole, EvidenceSource, MemorySnippet, TextSpan, TrustLevel,
    };

    fn evidence(id: &str, content: &str, relevance: f32) -> EvidenceBlock {
        EvidenceBlock {
            id: id.to_string(),
            source: EvidenceSource::FileRead,
            source_id: Some("src/lib.rs".into()),
            locator: Some("src/lib.rs:1".into()),
            chunk_id: None,
            span: Some(TextSpan::Lines { start: 1, end: 1 }),
            content: content.to_string(),
            confidence: 1.0,
            relevance,
            trust: TrustLevel::Trusted,
        }
    }

    fn recent(content: &str) -> RecentUnit {
        RecentUnit::Single(ContextMessage {
            role: ContextRole::User,
            content: content.to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        })
    }

    #[test]
    fn build_reuses_core_builder_and_preserves_runtime_link() {
        let engine = ContextEngine::new(ContextEngineConfig {
            max_context_tokens: 1024,
            pressure_threshold_percent: 90,
        });
        let input = ContextPipelineInput {
            stable_system: "stable coding instructions".into(),
            state: Some(StateBlock {
                goals: vec!["finish phase 9".into()],
                todos: vec!["wire context engine".into()],
                memories: vec![MemorySnippet {
                    content: "deeplossless owns replay".into(),
                    confidence: 0.9,
                }],
            }),
            evidence: vec![evidence("file-a", "important implementation detail", 0.9)],
            recent: vec![recent("previous user turn")],
            user_input: "continue".into(),
            runtime_link: ContextRuntimeLink {
                deeplossless_conversation_id: Some(42),
                deeplossless_replay_execution_id: Some("replay-7".into()),
            },
        };

        let output = engine.build(input).expect("context output");

        assert_eq!(output.pack.evidence.len(), 1);
        assert_eq!(output.report.deeplossless_conversation_id, Some(42));
        assert_eq!(
            output.report.deeplossless_replay_execution_id.as_deref(),
            Some("replay-7")
        );
        assert_eq!(
            output.report.stable_prefix_hash,
            ContextPackReport::compute_hash("stable coding instructions")
        );
    }

    #[test]
    fn pressure_report_can_emit_recovery_signal() {
        let engine = ContextEngine::new(ContextEngineConfig {
            max_context_tokens: 100,
            pressure_threshold_percent: 20,
        });
        let input = ContextPipelineInput {
            stable_system: "system".into(),
            evidence: vec![
                evidence("high", &"a".repeat(120), 1.0),
                evidence("low", &"b".repeat(120), 0.1),
            ],
            recent: vec![recent(&"old recent".repeat(20))],
            user_input: "input".into(),
            ..ContextPipelineInput::default()
        };

        let output = engine.build(input).expect("context output");
        let signal = output
            .report
            .recovery_signal()
            .expect("pressure recovery signal");

        assert!(output.report.under_pressure);
        assert!(output.report.pressure_percent >= 20);
        assert!(signal
            .evidence
            .as_deref()
            .unwrap_or_default()
            .contains("context pressure"));
    }

    #[test]
    fn build_error_is_structured() {
        let engine = ContextEngine::new(ContextEngineConfig {
            max_context_tokens: 1,
            pressure_threshold_percent: 90,
        });
        let err = engine
            .build(ContextPipelineInput {
                stable_system: "system".into(),
                user_input: "user input that cannot fit".into(),
                ..ContextPipelineInput::default()
            })
            .expect_err("context should not fit");

        assert!(matches!(err, ContextEngineError::BuildFailed(_)));
    }
}
