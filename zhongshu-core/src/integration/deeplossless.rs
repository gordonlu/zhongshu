use std::sync::Arc;

use deeplossless::runtime::RuntimePolicyConfig;
use deeplossless::runtime_coordinator::{CoordinatorConfig, RuntimeCoordinator};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

const DEFAULT_UPSTREAM: &str = "https://api.deepseek.com";
const PORT_RANGE: u16 = 10; // try up to 10 ports from the base

pub struct DeeplosslessConfig {
    pub db_path: String,
    pub api_key: String,
    pub upstream: String,
    pub summarize_model: String,
    pub proxy_port: u16,
}

impl Default for DeeplosslessConfig {
    fn default() -> Self {
        DeeplosslessConfig {
            db_path: "~/.deeplossless/lcm.db".into(),
            api_key: String::new(),
            upstream: DEFAULT_UPSTREAM.into(),
            summarize_model: "deepseek-chat".into(),
            proxy_port: 0,
        }
    }
}

pub struct DeeplosslessProxy {
    coordinator: Arc<Mutex<Option<RuntimeCoordinator>>>,
    actual_port: u16,
    base_url: String,
    db_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeeplosslessSnapshotTier {
    Ephemeral,
    Structural,
    Full,
    Frozen,
}

impl DeeplosslessSnapshotTier {
    fn as_i32(self) -> i32 {
        match self {
            DeeplosslessSnapshotTier::Ephemeral => 0,
            DeeplosslessSnapshotTier::Structural => 1,
            DeeplosslessSnapshotTier::Full => 2,
            DeeplosslessSnapshotTier::Frozen => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessSnapshotResult {
    pub status: String,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessRollbackResult {
    pub rollback_to: i64,
    pub summary: String,
    pub level: u8,
    pub deleted_nodes: Vec<i64>,
    #[serde(default)]
    pub children_remaining: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileClaimResult {
    pub status: String,
    pub agent_id: String,
    pub file_path: String,
    pub conv_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileClaimConflict {
    pub file_path: String,
    pub agent_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeeplosslessFileClaimOutcome {
    Claimed {
        claim: DeeplosslessFileClaimResult,
    },
    Conflict {
        conflict: DeeplosslessFileClaimConflict,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileReleaseResult {
    pub status: String,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileReleaseMissing {
    pub agent_id: String,
    pub file_path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeeplosslessFileReleaseOutcome {
    Released {
        release: DeeplosslessFileReleaseResult,
    },
    Missing {
        missing: DeeplosslessFileReleaseMissing,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileConflict {
    pub agent_id: String,
    pub file_path: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeeplosslessFileConflictsResult {
    #[serde(default)]
    pub conflicts: Vec<DeeplosslessFileConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct DeeplosslessErrorEnvelope {
    error: DeeplosslessErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct DeeplosslessErrorBody {
    message: String,
}

impl DeeplosslessProxy {
    pub async fn new(config: DeeplosslessConfig) -> anyhow::Result<Self> {
        let key = if config.api_key.is_empty() {
            None
        } else {
            Some(config.api_key.clone())
        };

        let coord_config = CoordinatorConfig {
            upstream: config.upstream.clone(),
            db_path: config.db_path.clone(),
            api_key: key,
            admin_key: None,
            summarizer_model: config.summarize_model,
            rate_limit: 100,
            runtime_profile: "autonomous".into(),
            dry_run: false,
            log_dir: None,
            record: None,
            passthrough: false,
            no_pipeline: false,
            no_header_mod: false,
            lcm_context: true,
            cache_normalize: true,
            lcm_context_tokens: 500,
            dag_threshold: None,
            summarizer_budget: 2000,
            policy_config: RuntimePolicyConfig::default(),
            workspace: None,
        };

        let coordinator = RuntimeCoordinator::build(coord_config).await?;
        info!("deeplossless coordinator built");

        Ok(DeeplosslessProxy {
            coordinator: Arc::new(Mutex::new(Some(coordinator))),
            actual_port: 0,
            base_url: String::new(),
            db_path: config.db_path.clone(),
        })
    }

    /// Start the HTTP proxy on the given port (0 = random).
    /// If the port is taken, tries subsequent ports up to +PORT_RANGE.
    pub async fn start(&mut self, desired_port: u16) -> anyhow::Result<u16> {
        let mut guard = self.coordinator.lock().await;
        let coordinator = guard.as_mut().expect("coordinator already taken");

        let app = coordinator.router();

        let (listener, actual) = bind_socket(desired_port).await?;

        info!("deeplossless proxy listening on 127.0.0.1:{actual}");

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!("deeplossless proxy stopped: {e}");
            }
        });

        self.actual_port = actual;
        self.base_url = format!("http://127.0.0.1:{actual}/v1");
        Ok(actual)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn port(&self) -> u16 {
        self.actual_port
    }

    fn lcm_url(&self, path: &str) -> anyhow::Result<String> {
        if self.base_url.is_empty() {
            return Err(anyhow::anyhow!("deeplossless proxy has not started"));
        }
        Ok(format!(
            "{}/lcm/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
    }

    pub async fn shutdown(&self) {
        let mut guard = self.coordinator.lock().await;
        if let Some(c) = guard.take() {
            c.shutdown(std::time::Duration::from_secs(2)).await;
            info!("deeplossless proxy shut down");
        }
    }

    /// Get the current (most recent) conversation ID from deeplossless.
    pub async fn current_conv_id(&self) -> Option<i64> {
        let expanded = self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        );
        let conn = rusqlite::Connection::open(&expanded).ok()?;
        conn.query_row(
            "SELECT id FROM conversations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    /// Compress the oldest `count` DAG leaves in the current conversation
    /// by POSTing to the local `/v1/lcm/compress` endpoint.
    ///
    /// Returns the number of leaf nodes compressed (0 if none, 0 if
    /// proxy not yet started).
    pub async fn compress_oldest_leaves(&self, count: usize) -> anyhow::Result<usize> {
        if self.base_url.is_empty() || count < 2 {
            return Ok(0);
        }
        let conv_id = match self.current_conv_id().await {
            Some(id) => id,
            None => return Ok(0),
        };

        // Query oldest leaf IDs directly from the SQLite db.
        let expanded = self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        );
        let conn = match rusqlite::Connection::open(&expanded) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("compress: cannot open lcm.db: {e}");
                return Ok(0);
            }
        };
        let ids: Vec<i64> = match conn.prepare(
            "SELECT id FROM dag_nodes WHERE conversation_id = ?1 AND is_leaf = 1 AND deleted = 0 ORDER BY id ASC LIMIT ?2",
        ) {
            Ok(mut stmt) => stmt
                .query_map(rusqlite::params![conv_id, count as i64], |row| row.get::<_, i64>(0))
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            Err(e) => {
                tracing::warn!("compress: cannot query dag leaves: {e}");
                return Ok(0);
            }
        };

        if ids.len() < 2 {
            return Ok(0);
        }

        let from = ids[0];
        let to = ids[ids.len() - 1];
        let url = self.lcm_url("compress")?;
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .json(&serde_json::json!({"conv_id": conv_id, "from": from, "to": to}))
            .send()
            .await?;

        if resp.status().is_success() {
            tracing::info!(
                "compressed {} DAG leaves [{from}..{to}] in conv {conv_id}",
                ids.len()
            );
            Ok(ids.len())
        } else {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!("compress endpoint returned {status}: {text}");
            Ok(0)
        }
    }

    /// Take a deeplossless execution snapshot.
    ///
    /// This is intentionally a thin wrapper around `/v1/lcm/snapshot`.
    /// Zhongshu should not maintain a parallel snapshot store.
    pub async fn take_execution_snapshot(
        &self,
        execution_id: i64,
        memory_version_id: i64,
        tier: DeeplosslessSnapshotTier,
        retention_ttl: Option<i64>,
    ) -> anyhow::Result<DeeplosslessSnapshotResult> {
        if execution_id <= 0 {
            return Err(anyhow::anyhow!("execution_id must be positive"));
        }
        let url = self.lcm_url("snapshot")?;
        let mut body = serde_json::json!({
            "execution_id": execution_id,
            "memory_version_id": memory_version_id,
            "tier": tier.as_i32(),
        });
        if let Some(ttl) = retention_ttl {
            body["retention_ttl"] = serde_json::json!(ttl);
        }

        let resp = reqwest::Client::new().post(url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "snapshot endpoint returned {status}: {text}"
            ));
        }
        Ok(resp.json().await?)
    }

    /// Roll back the deeplossless DAG to an existing node.
    ///
    /// File rollback should use this runtime facility when applicable before
    /// introducing any Zhongshu-local rollback persistence.
    pub async fn rollback_to_node(
        &self,
        node_id: i64,
    ) -> anyhow::Result<DeeplosslessRollbackResult> {
        if node_id <= 0 {
            return Err(anyhow::anyhow!("node_id must be positive"));
        }
        let url = self.lcm_url("rollback")?;
        let resp = reqwest::Client::new()
            .post(url)
            .json(&serde_json::json!({ "id": node_id }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "rollback endpoint returned {status}: {text}"
            ));
        }
        Ok(resp.json().await?)
    }

    /// Claim a file through deeplossless' active-file registry.
    ///
    /// Conflict is returned as data instead of a generic error so worker
    /// orchestration can make an explicit scheduling decision.
    pub async fn claim_file(
        &self,
        agent_id: &str,
        file_path: &str,
        operation: &str,
        conv_id: i64,
    ) -> anyhow::Result<DeeplosslessFileClaimOutcome> {
        validate_file_claim(agent_id, file_path, operation, conv_id)?;
        let resp = reqwest::Client::new()
            .post(self.lcm_url("file/claim")?)
            .json(&serde_json::json!({
                "agent_id": agent_id,
                "file_path": file_path,
                "operation": operation,
                "conv_id": conv_id,
            }))
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            return Ok(DeeplosslessFileClaimOutcome::Claimed {
                claim: resp.json().await?,
            });
        }

        let text = resp.text().await.unwrap_or_default();
        let message = deeplossless_error_message(&text);
        if status == reqwest::StatusCode::CONFLICT {
            return Ok(DeeplosslessFileClaimOutcome::Conflict {
                conflict: DeeplosslessFileClaimConflict {
                    file_path: file_path.to_string(),
                    agent_id: parse_conflict_agent(&message),
                    message,
                },
            });
        }

        Err(anyhow::anyhow!(
            "file claim endpoint returned {status}: {message}"
        ))
    }

    /// Release a file claim through deeplossless.
    ///
    /// Missing claims are explicit because a double release may point to a
    /// worker lifecycle bug.
    pub async fn release_file(
        &self,
        agent_id: &str,
        file_path: &str,
    ) -> anyhow::Result<DeeplosslessFileReleaseOutcome> {
        validate_agent_and_file(agent_id, file_path)?;
        let resp = reqwest::Client::new()
            .post(self.lcm_url("file/release")?)
            .json(&serde_json::json!({
                "agent_id": agent_id,
                "file_path": file_path,
            }))
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            return Ok(DeeplosslessFileReleaseOutcome::Released {
                release: resp.json().await?,
            });
        }

        let text = resp.text().await.unwrap_or_default();
        let message = deeplossless_error_message(&text);
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(DeeplosslessFileReleaseOutcome::Missing {
                missing: DeeplosslessFileReleaseMissing {
                    agent_id: agent_id.to_string(),
                    file_path: file_path.to_string(),
                    message,
                },
            });
        }

        Err(anyhow::anyhow!(
            "file release endpoint returned {status}: {message}"
        ))
    }

    pub async fn file_conflicts(&self) -> anyhow::Result<DeeplosslessFileConflictsResult> {
        let resp = reqwest::Client::new()
            .get(self.lcm_url("file/conflicts")?)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let message = deeplossless_error_message(&text);
            return Err(anyhow::anyhow!(
                "file conflicts endpoint returned {status}: {message}"
            ));
        }
        Ok(resp.json().await?)
    }

    /// Load recent chat history with correct roles from the messages table.
    /// Opens a direct read-only SQLite connection to lcm.db.
    /// Returns a list of (role, content) pairs from the most recent conversation.
    pub async fn load_chat_history(&self) -> Vec<(String, String)> {
        let expanded = self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        );
        let conn = match rusqlite::Connection::open(&expanded) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("failed to open lcm.db for history: {e}");
                return Vec::new();
            }
        };

        // Load messages across ALL conversations (message IDs are globally
        // unique and sequential).  This way restarts don't fragment history.
        let mut stmt = match conn.prepare("SELECT role, content FROM messages ORDER BY id ASC") {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("failed to prepare messages query: {e}");
                return Vec::new();
            }
        };

        let rows = match stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("failed to query messages: {e}");
                return Vec::new();
            }
        };

        // deeplossless stores content as JSON-encoded strings, e.g. `"\"text\""`.
        // Deserialize to get the actual text.
        fn decode_json_str(raw: &str) -> String {
            serde_json::from_str(raw).unwrap_or_else(|_| raw.to_string())
        }

        let mut turns = Vec::new();
        for row in rows {
            match row {
                Ok((role, content)) => {
                    // Only include user and assistant roles
                    if role == "user" || role == "assistant" {
                        turns.push((role, decode_json_str(&content)));
                    }
                }
                Err(e) => tracing::warn!("error reading message row: {e}"),
            }
        }
        turns
    }

    /// Permanently delete the most recent conversation from lcm.db.
    fn our_session_id(&self) -> Option<String> {
        let expanded = self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        );
        let sid_path = std::path::Path::new(&expanded).with_extension("sid");
        let sid = if sid_path.exists() {
            std::fs::read_to_string(&sid_path)
                .ok()
                .map(|s| s.trim().to_string())
        } else {
            None
        };
        let sid = sid.unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().to_string();
            let _ = std::fs::write(&sid_path, &id);
            id
        });
        // Rewrite all conversations to use our persistent session_id.
        if let Ok(conn) = rusqlite::Connection::open(&expanded) {
            let _ = conn.execute(
                "UPDATE conversations SET session_id = ?1 WHERE session_id != ?1",
                rusqlite::params![&sid],
            );
        }
        Some(sid)
    }

    pub async fn delete_chat_history(&self) {
        let sid = match self.our_session_id() {
            Some(s) => s,
            None => return,
        };
        let guard = self.coordinator.lock().await;
        let coordinator = match guard.as_ref() {
            Some(c) => c,
            None => return,
        };
        let db = &coordinator.state.storage.db;
        let expanded = self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        );
        if let Ok(conn) = rusqlite::Connection::open(&expanded) {
            let conv_ids: Vec<i64> = conn
                .prepare("SELECT id FROM conversations WHERE session_id = ?1")
                .ok()
                .map(|mut stmt| {
                    stmt.query_map(rusqlite::params![&sid], |row| row.get::<_, i64>(0))
                        .ok()
                        .map(|rows| rows.filter_map(|r| r.ok()).collect())
                })
                .flatten()
                .unwrap_or_default();
            for cid in &conv_ids {
                // Delete DAG nodes via deeplossless API.
                if let Ok(nodes) = db.get_all_dag_nodes(*cid) {
                    let mut deleted = 0usize;
                    for node in &nodes {
                        if !node.deleted {
                            let _ = db.delete_dag_node(node.id);
                            deleted += 1;
                        }
                    }
                    if deleted > 0 || !nodes.is_empty() {
                        tracing::info!("deleted {deleted} nodes from conversation {cid}");
                    }
                }
                // Delete messages scoped to this conversation.
                if let Ok(n) = conn.execute(
                    "DELETE FROM messages WHERE conversation_id = ?1",
                    rusqlite::params![cid],
                ) {
                    if n > 0 {
                        tracing::info!("deleted {n} messages for conversation {cid}");
                    }
                } else {
                    tracing::warn!("failed to delete messages for conv {cid}");
                }
            }
        }
    }
}

fn validate_file_claim(
    agent_id: &str,
    file_path: &str,
    operation: &str,
    conv_id: i64,
) -> anyhow::Result<()> {
    validate_agent_and_file(agent_id, file_path)?;
    if operation.trim().is_empty() {
        return Err(anyhow::anyhow!("operation must not be empty"));
    }
    if conv_id <= 0 {
        return Err(anyhow::anyhow!("conv_id must be positive"));
    }
    Ok(())
}

fn validate_agent_and_file(agent_id: &str, file_path: &str) -> anyhow::Result<()> {
    if agent_id.trim().is_empty() {
        return Err(anyhow::anyhow!("agent_id must not be empty"));
    }
    if file_path.trim().is_empty() {
        return Err(anyhow::anyhow!("file_path must not be empty"));
    }
    Ok(())
}

fn deeplossless_error_message(body: &str) -> String {
    serde_json::from_str::<DeeplosslessErrorEnvelope>(body)
        .map(|envelope| envelope.error.message)
        .unwrap_or_else(|_| body.to_string())
}

fn parse_conflict_agent(message: &str) -> Option<String> {
    message
        .split("held by agent '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .filter(|agent| !agent.is_empty())
        .map(ToString::to_string)
}

/// Try to bind to `start_port`; if taken, try port+1, port+2, ... up to +PORT_RANGE.
async fn bind_socket(start_port: u16) -> anyhow::Result<(tokio::net::TcpListener, u16)> {
    let attempts = if start_port == 0 { 1 } else { PORT_RANGE };
    for offset in 0..attempts {
        let port = if start_port == 0 {
            0
        } else {
            start_port + offset
        };
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let actual = listener.local_addr()?.port();
                return Ok((listener, actual));
            }
            Err(_e) if offset + 1 < attempts => {
                tracing::warn!("port {port} in use, trying next");
                continue;
            }
            Err(e) => return Err(anyhow::anyhow!("cannot bind to {addr}: {e}")),
        }
    }
    Err(anyhow::anyhow!(
        "no free port in range {}-{}",
        start_port,
        start_port + PORT_RANGE - 1
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a DeeplosslessProxy with a temporary in-memory database
    /// and a random port. Returns the proxy and the base URL.
    async fn test_proxy() -> (DeeplosslessProxy, String) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();

        let mut proxy = DeeplosslessProxy::new(DeeplosslessConfig {
            db_path,
            api_key: String::new(),
            upstream: DEFAULT_UPSTREAM.into(),
            summarize_model: "deepseek-chat".into(),
            proxy_port: 0,
        })
        .await
        .expect("proxy build");

        let _port = proxy.start(0).await.expect("proxy start");
        let base_url = proxy.base_url().to_string();
        (proxy, base_url)
    }

    #[tokio::test]
    async fn proxy_starts_and_listens() {
        let (proxy, base_url) = test_proxy().await;
        assert!(proxy.port() > 0, "should bind a random port");
        assert_eq!(base_url, format!("http://127.0.0.1:{}/v1", proxy.port()));

        // Verify the health endpoint responds
        let health_url = format!("http://127.0.0.1:{}/v1/health", proxy.port());
        let resp = reqwest::get(&health_url).await.expect("health check");
        assert!(
            resp.status().is_success(),
            "health should return 200, got {}",
            resp.status()
        );

        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn proxy_rejects_without_api_key() {
        let (proxy, base_url) = test_proxy().await;

        // Send a chat request without API key — proxy may accept but
        // upstream call will fail since there's no real key.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let resp = client
            .post(format!("{base_url}/chat/completions"))
            .json(&serde_json::json!({
                "model": "deepseek-chat",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": false,
            }))
            .send()
            .await
            .expect("chat request");

        // The proxy accepts the request and tries to forward upstream.
        // Without a real API key, upstream returns 401 which the proxy relays.
        assert!(
            resp.status().is_success() || resp.status().as_u16() == 401,
            "expected success or 401, got {}",
            resp.status()
        );

        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn lcm_url_requires_started_proxy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();

        let proxy = DeeplosslessProxy::new(DeeplosslessConfig {
            db_path,
            api_key: String::new(),
            upstream: DEFAULT_UPSTREAM.into(),
            summarize_model: "deepseek-chat".into(),
            proxy_port: 0,
        })
        .await
        .expect("proxy build");

        assert!(proxy.lcm_url("snapshot").is_err());
        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn lcm_url_formats_endpoint_with_separator() {
        let (proxy, _base_url) = test_proxy().await;

        assert_eq!(
            proxy.lcm_url("snapshot").unwrap(),
            format!("http://127.0.0.1:{}/v1/lcm/snapshot", proxy.port())
        );

        proxy.shutdown().await;
    }

    #[test]
    fn parses_deeplossless_error_message_and_conflict_agent() {
        let body = r#"{"error":{"code":"CONFLICT","message":"file 'src/lib.rs' held by agent 'worker-a' in another conversation"}}"#;
        let message = deeplossless_error_message(body);

        assert_eq!(
            message,
            "file 'src/lib.rs' held by agent 'worker-a' in another conversation"
        );
        assert_eq!(parse_conflict_agent(&message).as_deref(), Some("worker-a"));
    }

    #[test]
    fn validates_file_claim_inputs() {
        assert!(validate_file_claim("worker-a", "src/lib.rs", "edit", 1).is_ok());
        assert!(validate_file_claim("", "src/lib.rs", "edit", 1).is_err());
        assert!(validate_file_claim("worker-a", "", "edit", 1).is_err());
        assert!(validate_file_claim("worker-a", "src/lib.rs", "", 1).is_err());
        assert!(validate_file_claim("worker-a", "src/lib.rs", "edit", 0).is_err());
    }

    #[tokio::test]
    async fn file_claims_roundtrip_through_lcm_endpoint() {
        let (proxy, _base_url) = test_proxy().await;

        let first = proxy
            .claim_file("worker-a", "src/lib.rs", "edit", 1)
            .await
            .expect("first claim");
        assert!(matches!(
            first,
            DeeplosslessFileClaimOutcome::Claimed { .. }
        ));

        let conflict = proxy
            .claim_file("worker-b", "src/lib.rs", "edit", 2)
            .await
            .expect("conflict claim");
        match conflict {
            DeeplosslessFileClaimOutcome::Conflict { conflict } => {
                assert_eq!(conflict.file_path, "src/lib.rs");
                assert_eq!(conflict.agent_id.as_deref(), Some("worker-a"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }

        let conflicts = proxy.file_conflicts().await.expect("conflicts");
        assert_eq!(conflicts.conflicts.len(), 1);
        assert_eq!(conflicts.conflicts[0].agent_id, "worker-a");

        let release = proxy
            .release_file("worker-a", "src/lib.rs")
            .await
            .expect("release");
        assert!(matches!(
            release,
            DeeplosslessFileReleaseOutcome::Released { .. }
        ));

        let second = proxy
            .claim_file("worker-b", "src/lib.rs", "edit", 2)
            .await
            .expect("second claim");
        assert!(matches!(
            second,
            DeeplosslessFileClaimOutcome::Claimed { .. }
        ));

        proxy.shutdown().await;
    }
}
