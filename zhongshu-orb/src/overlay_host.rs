use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use wry::WebViewBuilder;

use http;
use serde_json::json;

use crate::overlay_assets::{
    legacy_chat_html, react_protocol_name, react_protocol_url, serve_react_protocol_asset,
    OverlayAsset,
};
use crate::overlay_contract::{
    parse_ui_command, AuthRequest, ChatEntry, OrganizationRecoveryCommand, OrganizationTaskCommand,
    SettingsConfig, SettingsUpdate, UiToOverlayCommand,
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

// === Shared overlay state ===

pub struct IpcClones {
    pub pi: Arc<Mutex<VecDeque<String>>>,
    pub pdr: Arc<Mutex<VecDeque<String>>>,
    pub pdo: Arc<Mutex<VecDeque<OrganizationTaskCommand>>>,
    pub plo: Arc<Mutex<bool>>,
    pub plog: Arc<Mutex<bool>>,
    pub pror: Arc<Mutex<VecDeque<OrganizationRecoveryCommand>>>,
    pub pa: Arc<Mutex<Option<String>>>,
    pub pd: Arc<Mutex<Option<String>>>,
    pub pp: Arc<Mutex<Option<String>>>,
    pub ps: Arc<Mutex<Option<SettingsUpdate>>>,
    pub rnc: Arc<Mutex<bool>>,
    pub rs: Arc<Mutex<bool>>,
    pub pos: Arc<Mutex<bool>>,
    pub plm: Arc<Mutex<bool>>,
    pub plt: Arc<Mutex<bool>>,
    pub plr: Arc<Mutex<bool>>,
    pub ple: Arc<Mutex<bool>>,
    pub pte: Arc<Mutex<Option<String>>>,
    pub ptz: Arc<Mutex<bool>>,
    pub host_commands: OverlayHostCommandQueue,
    pub pct: Arc<Mutex<Option<String>>>,
    pub pcmt: Arc<Mutex<Option<String>>>,
}

pub struct OverlayState {
    #[allow(dead_code)]
    pub request_quit: bool,
    #[allow(dead_code)]
    pub personality_selected: bool,
    pub pending_input: Arc<Mutex<VecDeque<String>>>,
    pub pending_delegate_review: Arc<Mutex<VecDeque<String>>>,
    pub pending_delegate_organization: Arc<Mutex<VecDeque<OrganizationTaskCommand>>>,
    pub pending_list_organization: Arc<Mutex<bool>>,
    pub pending_list_organization_graphs: Arc<Mutex<bool>>,
    pub pending_organization_recovery: Arc<Mutex<VecDeque<OrganizationRecoveryCommand>>>,
    pub pending_approve: Arc<Mutex<Option<String>>>,
    pub pending_deny: Arc<Mutex<Option<String>>>,
    pub pending_personality: Arc<Mutex<Option<String>>>,
    pub pending_settings: Arc<Mutex<Option<SettingsUpdate>>>,
    pub request_new_conversation: Arc<Mutex<bool>>,
    pub request_stop: Arc<Mutex<bool>>,
    pub pending_open_settings: Arc<Mutex<bool>>,
    pub pending_load_more: Arc<Mutex<bool>>,
    pub pending_list_tasks: Arc<Mutex<bool>>,
    pub pending_list_runbooks: Arc<Mutex<bool>>,
    pub pending_list_equipment: Arc<Mutex<bool>>,
    pub pending_toggle_equipment: Arc<Mutex<Option<String>>>,
    pub pending_toggle_zoom: Arc<Mutex<bool>>,
    pub host_commands: OverlayHostCommandQueue,
    pub pending_cancel_task: Arc<Mutex<Option<String>>>,
    pub pending_complete_task: Arc<Mutex<Option<String>>>,
}

impl OverlayState {
    pub fn new() -> Self {
        Self {
            request_quit: false,
            personality_selected: false,
            pending_input: Default::default(),
            pending_delegate_review: Default::default(),
            pending_delegate_organization: Default::default(),
            pending_list_organization: Default::default(),
            pending_list_organization_graphs: Default::default(),
            pending_organization_recovery: Default::default(),
            pending_approve: Default::default(),
            pending_deny: Default::default(),
            pending_personality: Default::default(),
            pending_settings: Default::default(),
            request_new_conversation: Default::default(),
            request_stop: Default::default(),
            pending_open_settings: Default::default(),
            pending_load_more: Default::default(),
            pending_list_tasks: Default::default(),
            pending_list_runbooks: Default::default(),
            pending_list_equipment: Default::default(),
            pending_toggle_equipment: Default::default(),
            pending_toggle_zoom: Default::default(),
            host_commands: Default::default(),
            pending_cancel_task: Default::default(),
            pending_complete_task: Default::default(),
        }
    }

    pub fn clone_for_ipc(&self) -> IpcClones {
        IpcClones {
            pi: self.pending_input.clone(),
            pdr: self.pending_delegate_review.clone(),
            pdo: self.pending_delegate_organization.clone(),
            plo: self.pending_list_organization.clone(),
            plog: self.pending_list_organization_graphs.clone(),
            pror: self.pending_organization_recovery.clone(),
            pa: self.pending_approve.clone(),
            pd: self.pending_deny.clone(),
            pp: self.pending_personality.clone(),
            ps: self.pending_settings.clone(),
            rnc: self.request_new_conversation.clone(),
            rs: self.request_stop.clone(),
            pos: self.pending_open_settings.clone(),
            plm: self.pending_load_more.clone(),
            plt: self.pending_list_tasks.clone(),
            plr: self.pending_list_runbooks.clone(),
            ple: self.pending_list_equipment.clone(),
            pte: self.pending_toggle_equipment.clone(),
            ptz: self.pending_toggle_zoom.clone(),
            host_commands: self.host_commands.clone(),
            pct: self.pending_cancel_task.clone(),
            pcmt: self.pending_complete_task.clone(),
        }
    }

    pub fn take_input(&self) -> Option<String> {
        self.pending_input.lock().unwrap().pop_front()
    }
    pub fn take_delegate_review(&self) -> Option<String> {
        self.pending_delegate_review.lock().unwrap().pop_front()
    }
    pub fn take_delegate_organization(&self) -> Option<OrganizationTaskCommand> {
        self.pending_delegate_organization
            .lock()
            .unwrap()
            .pop_front()
    }
    pub fn take_list_organization(&self) -> bool {
        std::mem::take(&mut *self.pending_list_organization.lock().unwrap())
    }
    pub fn take_list_organization_graphs(&self) -> bool {
        std::mem::take(&mut *self.pending_list_organization_graphs.lock().unwrap())
    }
    pub fn take_organization_recovery(&self) -> Option<OrganizationRecoveryCommand> {
        self.pending_organization_recovery
            .lock()
            .unwrap()
            .pop_front()
    }
    pub fn take_approve(&self) -> Option<String> {
        self.pending_approve.lock().unwrap().take()
    }
    pub fn take_deny(&self) -> Option<String> {
        self.pending_deny.lock().unwrap().take()
    }
    pub fn take_personality(&self) -> Option<String> {
        self.pending_personality.lock().unwrap().take()
    }
    pub fn take_settings(&self) -> Option<SettingsUpdate> {
        self.pending_settings.lock().unwrap().take()
    }
    pub fn take_new_conversation(&self) -> bool {
        std::mem::take(&mut *self.request_new_conversation.lock().unwrap())
    }
    pub fn take_stop(&self) -> bool {
        std::mem::take(&mut *self.request_stop.lock().unwrap())
    }
    pub fn take_open_settings(&self) -> bool {
        std::mem::take(&mut *self.pending_open_settings.lock().unwrap())
    }
    pub fn take_load_more(&self) -> bool {
        std::mem::take(&mut *self.pending_load_more.lock().unwrap())
    }
    pub fn take_list_tasks(&self) -> bool {
        std::mem::take(&mut *self.pending_list_tasks.lock().unwrap())
    }
    pub fn take_list_runbooks(&self) -> bool {
        std::mem::take(&mut *self.pending_list_runbooks.lock().unwrap())
    }
    pub fn take_list_equipment(&self) -> bool {
        std::mem::take(&mut *self.pending_list_equipment.lock().unwrap())
    }
    pub fn take_toggle_equipment(&self) -> Option<String> {
        self.pending_toggle_equipment.lock().unwrap().take()
    }
    pub fn take_toggle_zoom(&self) -> bool {
        std::mem::take(&mut *self.pending_toggle_zoom.lock().unwrap())
    }
    pub fn take_cancel_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_cancel_task.lock().unwrap())
    }
    pub fn take_complete_task(&self) -> Option<String> {
        std::mem::take(&mut *self.pending_complete_task.lock().unwrap())
    }
}

pub fn make_ipc_handler(clones: IpcClones) -> impl Fn(http::Request<String>) + 'static + Send {
    move |request: http::Request<String>| match parse_ui_command(request.body()) {
        UiToOverlayCommand::Submit(text) => {
            clones.pi.lock().unwrap().push_back(text);
        }
        UiToOverlayCommand::DelegateReview(text) => {
            clones.pdr.lock().unwrap().push_back(text);
        }
        UiToOverlayCommand::DelegateOrganization(task) => {
            clones.pdo.lock().unwrap().push_back(task);
        }
        UiToOverlayCommand::ListOrganizationEmployees => {
            *clones.plo.lock().unwrap() = true;
        }
        UiToOverlayCommand::ListOrganizationGraphs => {
            *clones.plog.lock().unwrap() = true;
        }
        UiToOverlayCommand::RecoverOrganization(command) => {
            clones.pror.lock().unwrap().push_back(command);
        }
        UiToOverlayCommand::Stop => {
            *clones.rs.lock().unwrap() = true;
        }
        UiToOverlayCommand::NewConversation | UiToOverlayCommand::DeleteHistory => {
            *clones.rnc.lock().unwrap() = true;
        }
        UiToOverlayCommand::Approve(rid) => {
            *clones.pa.lock().unwrap() = Some(rid);
        }
        UiToOverlayCommand::Deny(rid) => {
            *clones.pd.lock().unwrap() = Some(rid);
        }
        UiToOverlayCommand::PickPersonality(personality) => {
            *clones.pp.lock().unwrap() = Some(personality);
        }
        UiToOverlayCommand::SaveSettings(settings) => {
            *clones.ps.lock().unwrap() = Some(settings);
        }
        UiToOverlayCommand::OpenSettings => {
            *clones.pos.lock().unwrap() = true;
        }
        UiToOverlayCommand::LoadMore => {
            *clones.plm.lock().unwrap() = true;
        }
        UiToOverlayCommand::ListTasks => {
            *clones.plt.lock().unwrap() = true;
        }
        UiToOverlayCommand::ListRunbooks => {
            *clones.plr.lock().unwrap() = true;
        }
        UiToOverlayCommand::ListEquipment => {
            *clones.ple.lock().unwrap() = true;
        }
        UiToOverlayCommand::ToggleEquipment(id) => {
            *clones.pte.lock().unwrap() = Some(id);
        }
        UiToOverlayCommand::ToggleZoom => {
            *clones.ptz.lock().unwrap() = true;
        }
        UiToOverlayCommand::StartDrag => {
            clones.host_commands.push(OverlayHostCommand::StartDrag);
        }
        UiToOverlayCommand::Minimize => {
            clones.host_commands.push(OverlayHostCommand::Minimize);
        }
        UiToOverlayCommand::MaximizeRestore => {
            clones
                .host_commands
                .push(OverlayHostCommand::MaximizeRestore);
        }
        UiToOverlayCommand::CloseWindow => {
            clones.host_commands.push(OverlayHostCommand::CloseWindow);
        }
        UiToOverlayCommand::CancelTask(id) => {
            *clones.pct.lock().unwrap() = Some(id);
        }
        UiToOverlayCommand::CompleteTask(id) => {
            *clones.pcmt.lock().unwrap() = Some(id);
        }
        UiToOverlayCommand::Unknown => {}
    }
}

/// Trait shared by all platform overlay handles, providing JSON event sending.
pub trait OverlayHandleExt {
    fn webview_eval(&self, js: &str);

    fn send(&self, msg: &serde_json::Value) {
        let js = format!(
            "window.handleIpc({})",
            serde_json::to_string(msg).expect("serde_json::Value serialization is infallible")
        );
        self.webview_eval(&js);
    }

    fn push_delta(&self, content: &str) {
        self.send(&json!({ "type": "delta", "content": content }));
    }

    fn complete_message(&self) {
        self.send(&json!({ "type": "complete" }));
    }

    fn set_history(&self, entries: &[ChatEntry], has_more: bool) {
        self.send(&json!({ "type": "history", "entries": entries, "has_more": has_more }));
    }

    fn prepend_history(&self, entries: &[ChatEntry], has_more: bool) {
        self.send(&json!({ "type": "prepend_history", "entries": entries, "has_more": has_more }));
    }

    fn show_auth(&self, req: &AuthRequest) {
        self.send(&json!({ "type": "auth", "request": req }));
    }

    fn show_settings(&self, config: &SettingsConfig) {
        self.send(&json!({ "type": "settings", "config": config }));
    }

    #[allow(dead_code)] // only called from macOS test (not compiled on Linux)
    fn show_personality_picker(&self) {
        self.send(&json!({ "type": "show_personality" }));
    }

    fn clear_chat(&self) {
        self.send(&json!({ "type": "clear" }));
    }

    fn toast(&self, text: &str) {
        self.send(&json!({ "type": "toast", "text": text }));
    }

    fn set_state(&self, state: &str) {
        self.send(&json!({ "type": "state_change", "state": state }));
    }

    fn show_tasks(&self, tasks: &[serde_json::Value]) {
        self.send(&json!({ "type": "tasks", "tasks": tasks }));
    }

    fn show_runbooks(&self, runbooks: &[serde_json::Value]) {
        self.send(&json!({ "type": "runbooks", "runbooks": runbooks }));
    }

    fn show_equipment(&self, items: &[serde_json::Value]) {
        self.send(&json!({ "type": "equipment", "items": items }));
    }
}

/// Build an `OverlayHostDiagnostics` for a given platform.
pub fn overlay_diagnostics(
    platform: &str,
    webview_available: bool,
    startup_error: Option<String>,
) -> OverlayHostDiagnostics {
    OverlayHostDiagnostics {
        platform: platform.to_string(),
        webview_available,
        startup_error,
    }
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
