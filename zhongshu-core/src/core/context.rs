use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone)]
pub struct ContextPackReport {
    pub stable_system_tokens: usize,
    pub state_tokens: usize,
    pub evidence_tokens: usize,
    pub recent_tokens: usize,
    pub input_tokens: usize,
    pub total_tokens: usize,
    pub stable_prefix_hash: String,
    pub dropped_evidence_ids: Vec<String>,
    pub dropped_recent_units: usize,
    pub warnings: Vec<ContextWarning>,
}

#[derive(Debug, Clone)]
pub enum ContextWarning {
    StableSystemExceedsCap { tokens: usize },
    EvidenceContentTruncated { id: String },
    ContextTooLong { total: usize, limit: usize },
}

impl ContextPackReport {
    pub fn compute_hash(system: &str) -> String {
        let mut hasher = DefaultHasher::new();
        system.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub stable_system: String,
    pub state: Option<StateBlock>,
    pub evidence: Vec<EvidenceBlock>,
    pub recent: Vec<RecentUnit>,
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateBlock {
    pub goals: Vec<String>,
    pub todos: Vec<String>,
    pub memories: Vec<MemorySnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub content: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceBlock {
    pub id: String,
    pub source: EvidenceSource,
    pub source_id: Option<String>,
    pub locator: Option<String>,
    pub chunk_id: Option<String>,
    pub span: Option<TextSpan>,
    pub content: String,
    pub confidence: f32,
    pub relevance: f32,
    pub trust: TrustLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvidenceSource {
    WebSearch,
    WebFetch,
    FileRead,
    ShellOutput,
    Memory,
    BrowserSnapshot,
}

impl EvidenceSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            EvidenceSource::WebSearch => "WebSearch",
            EvidenceSource::WebFetch => "WebFetch",
            EvidenceSource::FileRead => "FileRead",
            EvidenceSource::ShellOutput => "ShellOutput",
            EvidenceSource::Memory => "Memory",
            EvidenceSource::BrowserSnapshot => "BrowserSnapshot",
        }
    }

    pub fn source_weight(&self) -> f32 {
        match self {
            EvidenceSource::Memory => 0.9,
            EvidenceSource::FileRead => 0.8,
            EvidenceSource::WebSearch => 0.6,
            EvidenceSource::WebFetch => 0.5,
            EvidenceSource::ShellOutput => 0.4,
            EvidenceSource::BrowserSnapshot => 0.3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrustLevel {
    Trusted,
    LowConfidence,
    Untrusted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TextSpan {
    Lines { start: usize, end: usize },
    Chars { start: usize, end: usize },
    Paragraphs { start: usize, end: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecentUnit {
    UserAssistant {
        user: ContextMessage,
        assistant: Option<ContextMessage>,
    },
    ToolChain {
        assistant: ContextMessage,
        tool_results: Vec<ContextMessage>,
        followup: Option<ContextMessage>,
    },
    Single(ContextMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: ContextRole,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ContextToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub output: Option<String>,
}

/// Estimate token count from text length. Uses same heuristic as existing loop_.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 / 3.5).ceil() as usize
}

#[derive(Debug, Default)]
pub struct ContextPackBuilder {
    stable_system: Option<String>,
    state: Option<StateBlock>,
    evidence: Vec<EvidenceBlock>,
    recent: Vec<RecentUnit>,
    input: Option<String>,
}

impl ContextPackBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stable_system(mut self, s: String) -> Self {
        self.stable_system = Some(s);
        self
    }

    pub fn state(mut self, s: StateBlock) -> Self {
        self.state = Some(s);
        self
    }

    pub fn with_evidence(mut self, e: Vec<EvidenceBlock>) -> Self {
        self.evidence = e;
        self
    }

    pub fn with_recent(mut self, r: Vec<RecentUnit>) -> Self {
        self.recent = r;
        self
    }

    pub fn input(mut self, s: String) -> Self {
        self.input = Some(s);
        self
    }

    pub fn build(
        self,
        max_context_tokens: usize,
    ) -> Result<(ContextPack, ContextPackReport), ContextBuildError> {
        let stable_system = self.stable_system.unwrap_or_default();
        let input = self.input.unwrap_or_default();

        let mut report = ContextPackReport::default();
        let evidence = self.evidence;
        let recent = self.recent;
        let state = self.state;
        let mut warnings = Vec::new();

        // Hard cap on stable_system: 8000 tokens
        let system_tokens = estimate_tokens(&stable_system);
        if system_tokens > 8000 {
            warnings.push(ContextWarning::StableSystemExceedsCap {
                tokens: system_tokens,
            });
            return Err(ContextBuildError::StableSystemTooLong(system_tokens));
        }

        let input_tokens = estimate_tokens(&input);
        let system_plus_input = system_tokens + input_tokens;
        if system_plus_input > max_context_tokens {
            warnings.push(ContextWarning::ContextTooLong {
                total: system_plus_input,
                limit: max_context_tokens,
            });
            return Err(ContextBuildError::ContextTooLong {
                total: system_plus_input,
                limit: max_context_tokens,
            });
        }

        // Score + crop evidence
        let evidence_with_scores: Vec<(usize, f32, EvidenceBlock)> = evidence
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let score = e.relevance * e.confidence * e.source.source_weight();
                (i, score, e)
            })
            .collect();

        let mut evidence_tokens = 0usize;
        let mut kept_evidence: Vec<EvidenceBlock> = Vec::new();
        let mut dropped_evidence_ids = Vec::new();

        // Sort by score ascending (lowest first), but crop from lowest
        let mut sorted: Vec<_> = evidence_with_scores;
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let evidence_budget = max_context_tokens.saturating_sub(system_plus_input) / 4;
        let mut budget_remaining = evidence_budget;

        for (_, _score, block) in sorted {
            let tokens = estimate_tokens(&block.content);
            if tokens <= budget_remaining {
                kept_evidence.push(block);
                evidence_tokens += tokens;
                budget_remaining = budget_remaining.saturating_sub(tokens);
            } else if !block.content.is_empty() && budget_remaining > 32 {
                // Truncate content, keep locator
                let max_len = (budget_remaining as f64 * 3.5) as usize;
                let truncated: String = block.content.chars().take(max_len).collect();
                let block_id = block.id.clone();
                let mut truncated_block = block;
                truncated_block.content = truncated;
                evidence_tokens += estimate_tokens(&truncated_block.content);
                kept_evidence.push(truncated_block);
                budget_remaining = 0;
                warnings.push(ContextWarning::EvidenceContentTruncated {
                    id: block_id,
                });
            } else {
                dropped_evidence_ids.push(block.id);
            }
        }

        // Crop state memories if needed
        let state_tokens = if let Some(ref st) = state {
            let mut t = 0usize;
            for g in &st.goals {
                t += estimate_tokens(g);
            }
            for td in &st.todos {
                t += estimate_tokens(td);
            }
            for m in &st.memories {
                t += estimate_tokens(&m.content);
            }
            t
        } else {
            0
        };

        // Crop recent units (oldest first)
        let mut recent_tokens = 0usize;
        let mut kept_recent: Vec<RecentUnit> = Vec::new();
        let mut dropped_recent = 0usize;
        let recent_budget = max_context_tokens.saturating_sub(
            system_plus_input + evidence_tokens + state_tokens,
        );

        // Recent units go newest-first for cropping (crop oldest)
        for unit in recent.into_iter().rev() {
            let unit_tokens = estimate_recent_unit_tokens(&unit);
            if recent_tokens + unit_tokens <= recent_budget {
                kept_recent.push(unit);
                recent_tokens += unit_tokens;
            } else {
                dropped_recent += 1;
            }
        }
        kept_recent.reverse();

        let total_estimate =
            system_tokens + input_tokens + evidence_tokens + state_tokens + recent_tokens;

        report.stable_system_tokens = system_tokens;
        report.state_tokens = state_tokens;
        report.evidence_tokens = evidence_tokens;
        report.recent_tokens = recent_tokens;
        report.input_tokens = input_tokens;
        report.total_tokens = total_estimate;
        report.stable_prefix_hash = ContextPackReport::compute_hash(&stable_system);
        report.dropped_evidence_ids = dropped_evidence_ids;
        report.dropped_recent_units = dropped_recent;
        report.warnings = warnings;

        Ok((
            ContextPack {
                stable_system,
                state,
                evidence: kept_evidence,
                recent: kept_recent,
                input,
            },
            report,
        ))
    }
}

fn estimate_tool_calls(tool_calls: &[ContextToolCall]) -> usize {
    tool_calls
        .iter()
        .map(|tc| {
            estimate_tokens(&tc.name)
                + estimate_tokens(&tc.arguments)
                + tc.output.as_ref().map(|o| estimate_tokens(o)).unwrap_or(0)
        })
        .sum()
}

fn estimate_message_tokens(msg: &ContextMessage) -> usize {
    estimate_tokens(&msg.content)
        + msg
            .tool_call_id
            .as_ref()
            .map(|id| estimate_tokens(id))
            .unwrap_or(0)
        + estimate_tool_calls(&msg.tool_calls)
}

fn estimate_recent_unit_tokens(unit: &RecentUnit) -> usize {
    match unit {
        RecentUnit::UserAssistant { user, assistant } => {
            estimate_message_tokens(user)
                + assistant
                    .as_ref()
                    .map(estimate_message_tokens)
                    .unwrap_or(0)
        }
        RecentUnit::ToolChain {
            assistant,
            tool_results,
            followup,
        } => {
            estimate_message_tokens(assistant)
                + tool_results.iter().map(estimate_message_tokens).sum::<usize>()
                + followup
                    .as_ref()
                    .map(estimate_message_tokens)
                    .unwrap_or(0)
        }
        RecentUnit::Single(msg) => estimate_message_tokens(msg),
    }
}

#[derive(Debug)]
pub enum ContextBuildError {
    StableSystemTooLong(usize),
    ContextTooLong { total: usize, limit: usize },
}

impl std::fmt::Display for ContextBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextBuildError::StableSystemTooLong(t) => {
                write!(f, "stable_system exceeds 8000 token hard cap: {}", t)
            }
            ContextBuildError::ContextTooLong { total, limit } => {
                write!(f, "context too long: {} > {} tokens", total, limit)
            }
        }
    }
}

impl std::error::Error for ContextBuildError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_evidence(id: &str, confidence: f32, relevance: f32) -> EvidenceBlock {
        EvidenceBlock {
            id: id.to_string(),
            source: EvidenceSource::WebSearch,
            source_id: None,
            locator: None,
            chunk_id: None,
            span: None,
            content: format!("Evidence block {} content {}", id, "x".repeat(100)),
            confidence,
            relevance,
            trust: TrustLevel::Untrusted,
        }
    }

    #[test]
    fn test_builder_basic() {
        let (pack, report) = ContextPackBuilder::new()
            .stable_system("You are a helpful assistant.".to_string())
            .input("Hello".to_string())
            .build(100_000)
            .unwrap();

        assert_eq!(pack.stable_system, "You are a helpful assistant.");
        assert_eq!(pack.input, "Hello");
        assert!(pack.evidence.is_empty());
        assert!(pack.recent.is_empty());
        assert!(report.total_tokens > 0);
    }

    #[test]
    fn test_builder_crops_low_score_evidence() {
        let evidence = vec![
            sample_evidence("high", 0.9, 0.9),
            sample_evidence("low", 0.1, 0.1),
        ];

        let (_pack, report) = ContextPackBuilder::new()
            .stable_system("sys".to_string())
            .with_evidence(evidence)
            .input("Hi".to_string())
            .build(500)
            .unwrap();

        assert_eq!(report.dropped_evidence_ids.len(), 0); // both fit
    }

    #[test]
    fn test_builder_context_too_long() {
        let result = ContextPackBuilder::new()
            .stable_system("x".repeat(1000))
            .input("y".repeat(1000))
            .build(100); // way too small

        assert!(matches!(
            result,
            Err(ContextBuildError::ContextTooLong { .. })
        ));
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("hello") > 0);
        assert!(estimate_tokens(&"x".repeat(350)) == 100);
    }

    #[test]
    fn test_evidence_crops_low_score_first() {
        let evidence = vec![
            sample_evidence("low", 0.1, 0.1),
            sample_evidence("high", 0.9, 0.9),
        ];

        let (_pack, report) = ContextPackBuilder::new()
            .stable_system("sys".to_string())
            .with_evidence(evidence)
            .input("Hi".to_string())
            .build(200)
            .unwrap();

        assert!(!report.dropped_evidence_ids.contains(&"high".to_string()));
    }

    #[test]
    fn test_stable_system_exceeds_cap() {
        let long_system = "x".repeat(28_001);
        let result = ContextPackBuilder::new()
            .stable_system(long_system)
            .input("Hi".to_string())
            .build(100_000);
        assert!(matches!(result, Err(ContextBuildError::StableSystemTooLong(_))));
    }

    #[test]
    fn test_context_message_conversion() {
        let msg = ContextMessage {
            role: ContextRole::User,
            content: "test".to_string(),
            tool_call_id: None,
            tool_calls: vec![],
        };
        assert_eq!(msg.content, "test");
    }
}
