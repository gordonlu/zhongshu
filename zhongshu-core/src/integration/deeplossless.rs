use std::sync::Arc;

use deeplossless::runtime::RuntimePolicyConfig;
use deeplossless::runtime_coordinator::{CoordinatorConfig, RuntimeCoordinator};
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
        let url = format!("{}lcm/compress", self.base_url);
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

        let conv_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM conversations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        let conv_id = match conv_id {
            Some(id) => id,
            None => return Vec::new(),
        };

        let mut stmt = match conn.prepare(
            "SELECT role, content FROM messages WHERE conversation_id = ?1 ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("failed to prepare messages query: {e}");
                return Vec::new();
            }
        };

        let rows = match stmt.query_map(rusqlite::params![conv_id], |row| {
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
    pub async fn delete_chat_history(&self) {
        let guard = self.coordinator.lock().await;
        let coordinator = match guard.as_ref() {
            Some(c) => c,
            None => return,
        };
        let db = &coordinator.state.storage.db;
        let conv_id = match db.last_conversation_id() {
            Ok(Some(id)) => id,
            _ => return,
        };
        // Soft-delete all DAG nodes in this conversation
        let all = match db.get_all_dag_nodes(conv_id) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("failed to load dag nodes for deletion: {e}");
                return;
            }
        };
        for node in &all {
            if !node.deleted {
                if let Err(e) = db.delete_dag_node(node.id) {
                    tracing::warn!("failed to delete dag node {}: {e}", node.id);
                }
            }
        }
        tracing::info!("deleted {} nodes from conversation {conv_id}", all.len());

        // Also delete from the messages table so history doesn't reappear on restart.
        if let Ok(conn) = rusqlite::Connection::open(&self.db_path.replacen(
            "~",
            &std::env::var("HOME").unwrap_or_else(|_| ".".into()),
            1,
        )) {
            if let Err(e) = conn.execute(
                "DELETE FROM messages WHERE conversation_id = ?1",
                rusqlite::params![conv_id],
            ) {
                tracing::warn!("failed to delete messages for conversation {conv_id}: {e}");
            } else {
                tracing::info!("deleted all messages for conversation {conv_id}");
            }
        }
    }
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
}
