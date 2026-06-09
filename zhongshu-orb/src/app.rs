use std::sync::Arc;
use tokio::sync::mpsc;
use zhongshu_core::agent::llm::OpenAiProvider;
use zhongshu_core::tool::ToolRegistry;
use zhongshu_core::integration::ContextEngine;

use crate::render::OrbState;

pub enum UiEvent {
    SetState(OrbState),
    TextDelta(String),
    ToolStart,
    ToolDone(bool),
}

pub struct UiBridge {
    pub tx: mpsc::UnboundedSender<UiEvent>,
    pub rx: mpsc::UnboundedReceiver<UiEvent>,
}

impl UiBridge {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        UiBridge { tx, rx }
    }
}

pub struct AgentRuntime {
    pub provider: OpenAiProvider,
    pub tools: ToolRegistry,
    pub model: String,
}

#[derive(Clone)]
pub struct SessionState {
    pub engine: Arc<tokio::sync::Mutex<Option<Arc<ContextEngine>>>>,
    pub conv_id: Arc<tokio::sync::Mutex<i64>>,
}

impl SessionState {
    pub fn new() -> Self {
        SessionState {
            engine: Arc::new(tokio::sync::Mutex::new(None)),
            conv_id: Arc::new(tokio::sync::Mutex::new(1)),
        }
    }
}
