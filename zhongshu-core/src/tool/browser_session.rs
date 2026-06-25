use anyhow::{Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tracing::info;

/// Default CDP port for zhongshu-managed Chrome.
const CDP_PORT: u16 = 9222;

/// Managed browser session: one Chrome process shared across operations.
pub struct BrowserSession {
    browser: Arc<Browser>,
    profile_dir: PathBuf,
    pub port: u16,
}

impl BrowserSession {
    /// Find or start Chrome, connect via CDP, using the given profile directory.
    pub async fn launch(profile_dir: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&profile_dir)?;

        let browser = match try_connect(CDP_PORT).await {
            Ok(b) => {
                info!("connected to existing Chrome on port {CDP_PORT}");
                b
            }
            Err(_) => {
                info!("starting Chrome with CDP on port {CDP_PORT}");
                let config = BrowserConfig::builder()
                    .port(CDP_PORT)
                    .user_data_dir(profile_dir.clone())
                    .build()
                    .context("failed to build Chrome config")?;
                Browser::launch(config).await.context("failed to launch Chrome")?
            }
        };

        info!("Chrome CDP ready on port {CDP_PORT}");
        Ok(Arc::new(BrowserSession {
            browser: Arc::new(browser),
            profile_dir,
            port: CDP_PORT,
        }))
    }

    pub fn handle(&self) -> &Arc<Browser> {
        &self.browser
    }
}

async fn try_connect(port: u16) -> Result<Browser> {
    let config = BrowserConfig::builder()
        .port(port)
        .build()
        .context("connect config")?;
    Browser::launch(config).await.map_err(Into::into)
}

/// Global browser session singleton.
static SESSION: OnceLock<Arc<BrowserSession>> = OnceLock::new();

/// Get or create the shared browser session.
/// `profile_dir` is only used on first call; subsequent calls ignore it.
pub async fn global_session(profile_dir: PathBuf) -> Result<&'static Arc<BrowserSession>> {
    if let Some(s) = SESSION.get() {
        return Ok(s);
    }
    let session = BrowserSession::launch(profile_dir).await?;
    Ok(SESSION.get_or_init(|| session))
}
