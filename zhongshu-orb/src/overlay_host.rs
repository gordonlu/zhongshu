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
