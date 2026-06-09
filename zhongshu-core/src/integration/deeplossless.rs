use std::sync::Arc;
use tokio::sync::Mutex;

use anyhow::Context;
use deeplossless::compactor::{CompactCommand, Compactor, CompactorConfig};
use deeplossless::dag::{DagConfig, DagEngine, DagNode};
use deeplossless::db::{Database, DatabaseBuilder};
use deeplossless::summarizer::{SummarizerConfig};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

const DEFAULT_CONTEXT_WINDOW: usize = 100_000;
const DEFAULT_GROUP_SIZE: usize = 16;

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub db_path: String,
    pub token_budget: usize,
    pub api_key: String,
    pub upstream: String,
    pub summarize_model: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        ContextConfig {
            db_path: "~/.zhongshu/context.db".into(),
            token_budget: DEFAULT_CONTEXT_WINDOW,
            api_key: String::new(),
            upstream: "https://api.deepseek.com".into(),
            summarize_model: "deepseek-chat".into(),
        }
    }
}

pub struct ContextEngine {
    db: Arc<Database>,
    dag: Arc<DagEngine>,
    compactor: Mutex<Compactor>,
    config: ContextConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressReason {
    HardLimit,
    SoftLimit,
    TaskBoundary,
    TopicShift,
}

pub struct CompressDecision {
    pub should_compress: bool,
    pub reason: Option<CompressReason>,
}

impl ContextEngine {
    pub async fn new(config: ContextConfig) -> anyhow::Result<Self> {
        let db_path = expand_tilde(&config.db_path);
        let db = Arc::new(
            DatabaseBuilder::new()
                .path(&db_path)
                .build()
                .await
                .context("failed to open zhongshu context database")?,
        );

        let dag_config = DagConfig {
            soft_threshold_ratio: 0.70,
            hard_threshold_ratio: 0.90,
            max_level: 3,
            max_fanout: 100,
            max_expand_depth: 10,
            recent_message_count: 20,
            token_correction_factor: 1.0,
            token_overhead: 12,
            embedding_api_key: String::new(),
        };

        let dag = Arc::new(DagEngine::builder().build(db.clone()));

        let compactor_config = CompactorConfig {
            dag: dag_config,
            summarizer: SummarizerConfig {
                model: config.summarize_model.clone(),
                upstream: config.upstream.clone(),
                api_key: config.api_key.clone(),
                ..SummarizerConfig::default()
            },
            soft_threshold_pct: 0.70,
            hard_threshold_pct: 0.90,
            group_size: DEFAULT_GROUP_SIZE,
            age_weight: 0.4,
            token_density_weight: 0.2,
            novelty_weight: 0.4,
        };

        let compactor = Compactor::new(db.clone(), compactor_config, None);

        Ok(ContextEngine {
            db,
            dag,
            compactor: Mutex::new(compactor),
            config,
        })
    }

    pub fn find_or_create_conv(&self, system_prompt: &str, model: &str) -> anyhow::Result<i64> {
        let mut hasher = Sha256::new();
        hasher.update(system_prompt.as_bytes());
        hasher.update(model.as_bytes());
        let fingerprint = hex::encode(&hasher.finalize()[..8]);

        self.db
            .find_or_create_conversation(&fingerprint, model)
            .context("failed to find or create conversation")
    }

    pub fn build_context(&self, conv_id: i64, token_budget: usize, query: &str) -> anyhow::Result<String> {
        let nodes = self
            .dag
            .assemble_context(conv_id, token_budget, Some(query))
            .context("failed to assemble context")?;

        if nodes.is_empty() {
            return Ok(String::new());
        }

        Ok(render_nodes(&nodes))
    }

    pub fn append_turn(
        &self,
        conv_id: i64,
        user_msg: &str,
        assistant_msg: &str,
    ) -> anyhow::Result<()> {
        self.dag.insert_leaf(conv_id, user_msg, estimate_tokens(user_msg))?;
        self.dag.insert_leaf(conv_id, assistant_msg, estimate_tokens(assistant_msg))?;
        Ok(())
    }

    pub fn check_compression(&self, conv_id: i64) -> CompressDecision {
        let total = match self.dag.total_tokens(conv_id) {
            Ok(t) => t as usize,
            Err(e) => {
                warn!("context: failed to get total tokens: {e}");
                return CompressDecision { should_compress: false, reason: None };
            }
        };

        let hard = (self.config.token_budget as f64 * 0.90) as usize;
        let soft = (self.config.token_budget as f64 * 0.70) as usize;

        if total > hard {
            debug!(total, hard, "hard limit exceeded");
            return CompressDecision { should_compress: true, reason: Some(CompressReason::HardLimit) };
        }

        if total > soft {
            debug!(total, soft, "soft limit exceeded");
            return CompressDecision { should_compress: true, reason: Some(CompressReason::SoftLimit) };
        }

        CompressDecision { should_compress: false, reason: None }
    }

    pub async fn trigger_compaction(&self, conv_id: i64) -> anyhow::Result<()> {
        let mut compactor = self.compactor.lock().await;
        compactor
            .send_command(CompactCommand::ReviewAndCompact {
                conv_id,
                context_window: self.config.token_budget,
            })
            .await
            .map_err(|_| anyhow::anyhow!("compactor: failed to send command"))?;
        info!(
            conv_id,
            context_window = self.config.token_budget,
            "compaction triggered"
        );
        Ok(())
    }

    pub fn conv_token_count(&self, conv_id: i64) -> anyhow::Result<i64> {
        self.dag.total_tokens(conv_id)
    }

    pub fn conv_leaf_count(&self, conv_id: i64) -> anyhow::Result<usize> {
        self.dag.get_leaves(conv_id).map(|v| v.len())
    }
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), &path[2..]);
        }
    }
    path.to_string()
}

fn estimate_tokens(text: &str) -> i64 {
    (text.len() as f64 / 3.5).ceil() as i64
}

fn render_nodes(nodes: &[DagNode]) -> String {
    let mut parts = Vec::new();

    let summaries: Vec<_> = nodes.iter().filter(|n| n.level > 0).collect();
    let leaves: Vec<_> = nodes.iter().filter(|n| n.level == 0).collect();

    if !summaries.is_empty() {
        parts.push("## 历史摘要".to_string());
        for node in summaries {
            parts.push(format!("[L{}] {}", node.level, node.summary));
        }
    }

    if !leaves.is_empty() {
        parts.push("## 近期对话".to_string());
        for node in leaves.iter().rev().take(10) {
            let truncated = if node.summary.len() > 500 {
                format!("{}...", &node.summary[..500])
            } else {
                node.summary.clone()
            };
            parts.push(truncated);
        }
    }

    parts.join("\n\n")
}
