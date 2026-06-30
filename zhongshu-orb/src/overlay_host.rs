use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use wry::WebViewBuilder;

use crate::overlay_assets::{
    legacy_chat_html, react_protocol_name, react_protocol_url, serve_react_protocol_asset,
    OverlayAsset,
};

pub fn log_selected_asset(platform: &str, asset: &OverlayAsset) {
    match asset {
        OverlayAsset::ReactProtocol { index_path, .. } => {
            tracing::info!(
                "{platform} overlay loading react UI over custom protocol from {}",
                index_path.display()
            );
        }
        OverlayAsset::ReactInline { index_path, .. } => {
            tracing::info!(
                "{platform} overlay loading inlined react UI fallback from {}",
                index_path.display()
            );
        }
        OverlayAsset::LegacyHtml { reason } => {
            tracing::info!("{platform} overlay loading legacy UI: {reason}");
        }
    }
}

pub fn webview_builder_for_asset(asset: OverlayAsset) -> WebViewBuilder<'static> {
    match asset {
        OverlayAsset::ReactProtocol { dist_dir, .. } => WebViewBuilder::new()
            .with_custom_protocol(react_protocol_name().into(), move |_webview_id, request| {
                serve_react_protocol_asset(&dist_dir, request)
            })
            .with_url(react_protocol_url()),
        OverlayAsset::ReactInline { html, .. } => WebViewBuilder::new().with_html(html),
        OverlayAsset::LegacyHtml { .. } => WebViewBuilder::new().with_html(legacy_chat_html()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayHostCommand {
    StartDrag,
    Minimize,
    MaximizeRestore,
    CloseWindow,
}

#[derive(Debug, Clone, Default)]
pub struct OverlayHostCommandQueue {
    inner: Arc<Mutex<VecDeque<OverlayHostCommand>>>,
}

impl OverlayHostCommandQueue {
    pub fn push(&self, command: OverlayHostCommand) {
        self.inner.lock().unwrap().push_back(command);
    }

    pub fn take(&self, command: OverlayHostCommand) -> bool {
        let mut commands = self.inner.lock().unwrap();
        let Some(index) = commands.iter().position(|queued| *queued == command) else {
            return false;
        };
        commands.remove(index);
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayHostDiagnostics {
    pub platform: String,
    pub webview_available: bool,
    pub startup_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{OverlayHostCommand, OverlayHostCommandQueue};

    #[test]
    fn command_queue_preserves_each_window_command_once() {
        let queue = OverlayHostCommandQueue::default();

        queue.push(OverlayHostCommand::StartDrag);
        queue.push(OverlayHostCommand::CloseWindow);

        assert!(queue.take(OverlayHostCommand::CloseWindow));
        assert!(!queue.take(OverlayHostCommand::CloseWindow));
        assert!(queue.take(OverlayHostCommand::StartDrag));
    }

    #[test]
    fn diagnostics_serializes_startup_error() {
        let diagnostics = super::OverlayHostDiagnostics {
            platform: "windows".to_string(),
            webview_available: false,
            startup_error: Some("WebView2 unavailable".to_string()),
        };

        let json = serde_json::to_value(&diagnostics).unwrap();

        assert_eq!(json["platform"], "windows");
        assert_eq!(json["webview_available"], false);
        assert_eq!(json["startup_error"], "WebView2 unavailable");
    }
}
