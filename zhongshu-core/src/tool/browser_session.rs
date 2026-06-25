use anyhow::{anyhow, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::info;

const CDP_PORT: u16 = 9222;

pub struct BrowserSession {
    browser: Arc<Browser>,
    #[allow(dead_code)]
    profile_dir: PathBuf,
    pub port: u16,
}

impl BrowserSession {
    pub async fn launch(profile_dir: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&profile_dir)?;

        let browser = match try_connect().await {
            Ok(b) => b,
            Err(_) => {
                info!("starting Chrome with CDP on port {CDP_PORT}");
                let config: BrowserConfig = BrowserConfig::builder()
                    .port(CDP_PORT)
                    .user_data_dir(&profile_dir)
                    .build()
                    .map_err(|e| anyhow!("Chrome config: {e}"))?;
                let (b, _handler) = Browser::launch(config).await?;
                b
            }
        };

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

async fn try_connect() -> Result<Browser> {
    let config: BrowserConfig = BrowserConfig::builder()
        .port(CDP_PORT)
        .build()
        .map_err(|e| anyhow!("connect: {e}"))?;
    let (b, _h) = Browser::launch(config).await?;
    Ok(b)
}

static SESSION: OnceLock<Arc<BrowserSession>> = OnceLock::new();

pub async fn global_session(profile_dir: PathBuf) -> Result<&'static Arc<BrowserSession>> {
    if let Some(s) = SESSION.get() {
        return Ok(s);
    }
    let session = BrowserSession::launch(profile_dir).await?;
    Ok(SESSION.get_or_init(|| session))
}
