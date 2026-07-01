use std::ops::Deref;
use std::sync::Arc;

use serde_json::json;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};
use wry::Rect;

use crate::overlay_assets::select_overlay_asset;
use crate::overlay_host::{
    log_selected_asset, make_ipc_handler, overlay_diagnostics, webview_builder_for_asset,
    OverlayHandleExt, OverlayHostDiagnostics, OverlayState,
};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, ToolCallEntry, ToolStatus,
};

pub struct OverlayHandle {
    pub state: OverlayState,
    window: Arc<Window>,
    webview: Option<wry::WebView>,
    startup_error: Option<String>,
}

impl Deref for OverlayHandle {
    type Target = OverlayState;
    fn deref(&self) -> &OverlayState {
        &self.state
    }
}

impl OverlayHandleExt for OverlayHandle {
    fn webview_eval(&self, js: &str) {
        if let Some(wv) = self.webview.as_ref() {
            if let Err(e) = wv.evaluate_script(js) {
                tracing::warn!("macos webview eval error: {e}");
            }
        }
    }
}

impl OverlayHandle {
    pub fn eval(&self, js: &str) {
        if let Some(webview) = self.webview.as_ref() {
            if let Err(e) = webview.evaluate_script(js) {
                tracing::warn!("macos webview eval error: {e}");
            }
        }
    }

    pub fn show_window(&self, width: f32, height: f32) {
        let (max_width, max_height) = self
            .window
            .current_monitor()
            .map(|monitor| {
                let size = monitor.size().to_logical::<f32>(self.window.scale_factor());
                (
                    (size.width * 0.96).max(360.0),
                    (size.height * 0.92).max(520.0),
                )
            })
            .unwrap_or((2400.0, 1600.0));
        let width = width.clamp(360.0, max_width);
        let height = height.clamp(520.0, max_height);
        let _ = self
            .window
            .request_inner_size(LogicalSize::new(width, height));
        self.window.set_visible(true);
        self.window.set_window_level(WindowLevel::AlwaysOnTop);
        self.window.focus_window();
        self.resize_webview();
        if self.startup_error.is_some() {
            self.window.request_user_attention(None);
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        Some(self.window.id())
    }

    pub fn host_diagnostics(&self) -> OverlayHostDiagnostics {
        overlay_diagnostics("macos", self.webview.is_some(), self.startup_error.clone())
    }

    pub fn handle_window_event(&self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => {
                self.window.set_visible(false);
                true
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.resize_webview();
                self.window.request_redraw();
                true
            }
            _ => false,
        }
    }

    pub fn resize_webview(&self) {
        let Some(webview) = self.webview.as_ref() else {
            return;
        };
        let size = self
            .window
            .inner_size()
            .to_logical::<u32>(self.window.scale_factor());
        if let Err(e) = webview.set_bounds(Rect {
            position: LogicalPosition::new(0, 0).into(),
            size: LogicalSize::new(size.width, size.height).into(),
        }) {
            tracing::warn!("macos webview resize error: {e}");
        }
    }

    pub fn start_drag_window(&self) {
        if let Err(e) = self.window.drag_window() {
            tracing::warn!("macos overlay drag_window failed: {e}");
        }
    }

    pub fn minimize_window(&self) {
        self.window.set_minimized(true);
    }

    pub fn maximize_restore_window(&self) {
        self.window.set_maximized(!self.window.is_maximized());
        self.resize_webview();
    }

    pub fn close_window(&self) {
        self.window.set_visible(false);
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        self.window.set_visible(false);
    }
}

pub fn show(event_loop: &ActiveEventLoop, width: f32, height: f32) -> OverlayHandle {
    let state = OverlayState::new();
    let clones = state.clone_for_ipc();
    let ipc_handler = make_ipc_handler(clones);

    let attrs = Window::default_attributes()
        .with_title("Zhongshu")
        .with_inner_size(LogicalSize::new(
            width.clamp(360.0, 2400.0),
            height.clamp(520.0, 1600.0),
        ))
        .with_min_inner_size(LogicalSize::new(360.0, 520.0))
        .with_decorations(false)
        .with_resizable(true)
        .with_visible(false)
        .with_window_level(WindowLevel::AlwaysOnTop);

    let window = Arc::new(
        event_loop
            .create_window(attrs)
            .expect("macos overlay window creation failed"),
    );

    let asset = select_overlay_asset();
    log_selected_asset("macos", &asset);
    let builder = webview_builder_for_asset(asset)
        .with_user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Zhongshu/1.0")
        .with_ipc_handler(ipc_handler);

    let (webview, startup_error) = match builder.build_as_child(window.as_ref()) {
        Ok(wv) => (Some(wv), None),
        Err(e) => {
            let message = format!("WebKit unavailable on macOS: {e}");
            tracing::error!("{message}");
            window.set_title(&format!("Zhongshu - {message}"));
            (None, Some(message))
        }
    };

    let handle = OverlayHandle {
        state,
        window,
        webview,
        startup_error,
    };
    handle.show_window(width, height);
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_host::OverlayHostCommand;
    use winit::application::ApplicationHandler;
    use winit::event_loop::EventLoop;

    struct TestContext {
        result: Option<Result<OverlayHandle, String>>,
    }

    impl ApplicationHandler for TestContext {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.result.is_some() {
                return;
            }
            let handle = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                show(event_loop, 700.0, 900.0)
            }));
            self.result = Some(match handle {
                Ok(h) => Ok(h),
                Err(e) => {
                    let msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    Err(msg)
                }
            });
            event_loop.exit();
        }
    }

    fn create_handle() -> OverlayHandle {
        let event_loop = EventLoop::new().expect("EventLoop::new() failed — no display server");
        let mut ctx = TestContext { result: None };
        event_loop.run_app(&mut ctx).expect("event loop failed");
        ctx.result
            .expect("TestContext never called resumed")
            .expect(
                "show() panicked during test — check for missing display server or macOS-only APIs",
            )
    }

    #[test]
    fn host_diagnostics_reports_macos_platform() {
        let handle = create_handle();
        let diag = handle.host_diagnostics();
        assert_eq!(diag.platform, "macos");
    }

    #[test]
    fn host_diagnostics_reports_webview_available() {
        let handle = create_handle();
        let diag = handle.host_diagnostics();
        assert!(
            diag.webview_available,
            "webview should be available on macOS"
        );
        assert!(diag.startup_error.is_none(), "no startup error expected");
    }

    #[test]
    fn take_input_happy_path() {
        let handle = create_handle();
        handle
            .pending_input
            .lock()
            .unwrap()
            .push_back("hello".into());
        assert_eq!(handle.take_input(), Some("hello".into()));
        assert_eq!(handle.take_input(), None);
    }

    #[test]
    fn take_input_empty() {
        let handle = create_handle();
        assert_eq!(handle.take_input(), None);
    }

    #[test]
    fn take_input_multiple_values() {
        let handle = create_handle();
        handle.pending_input.lock().unwrap().push_back("a".into());
        handle.pending_input.lock().unwrap().push_back("b".into());
        handle.pending_input.lock().unwrap().push_back("c".into());
        assert_eq!(handle.take_input(), Some("a".into()));
        assert_eq!(handle.take_input(), Some("b".into()));
        assert_eq!(handle.take_input(), Some("c".into()));
        assert_eq!(handle.take_input(), None);
    }

    #[test]
    fn take_approve_happy_path() {
        let handle = create_handle();
        *handle.pending_approve.lock().unwrap() = Some("req-1".into());
        assert_eq!(handle.take_approve(), Some("req-1".into()));
        assert_eq!(handle.take_approve(), None);
    }

    #[test]
    fn take_approve_empty() {
        let handle = create_handle();
        assert_eq!(handle.take_approve(), None);
    }

    #[test]
    fn take_deny_happy_path() {
        let handle = create_handle();
        *handle.pending_deny.lock().unwrap() = Some("req-2".into());
        assert_eq!(handle.take_deny(), Some("req-2".into()));
        assert_eq!(handle.take_deny(), None);
    }

    #[test]
    fn take_personality_happy_path() {
        let handle = create_handle();
        *handle.pending_personality.lock().unwrap() = Some("coder".into());
        assert_eq!(handle.take_personality(), Some("coder".into()));
        assert_eq!(handle.take_personality(), None);
    }

    #[test]
    fn take_settings_happy_path() {
        let handle = create_handle();
        let config = SettingsConfig {
            api_key: "sk-xxx".into(),
            api_key_saved: true,
            api_base: "https://api.example.com".into(),
            model: "gpt-4".into(),
            personality: "default".into(),
            proxy_port: None,
            bg_enabled: None,
            bg_interval: None,
            bg_prompt: None,
            auto_evolve: None,
            max_context_tokens: None,
            mode: None,
        };
        *handle.pending_settings.lock().unwrap() = Some(config.clone());
        assert_eq!(handle.take_settings(), Some(config));
        assert_eq!(handle.take_settings(), None);
    }

    #[test]
    fn take_new_conversation_default_false() {
        let handle = create_handle();
        assert!(!handle.take_new_conversation());
    }

    #[test]
    fn take_new_conversation_set_true_then_resets() {
        let handle = create_handle();
        *handle.request_new_conversation.lock().unwrap() = true;
        assert!(handle.take_new_conversation());
        assert!(!handle.take_new_conversation());
    }

    #[test]
    fn take_stop_default_false() {
        let handle = create_handle();
        assert!(!handle.take_stop());
    }

    #[test]
    fn take_stop_set_true_then_resets() {
        let handle = create_handle();
        *handle.request_stop.lock().unwrap() = true;
        assert!(handle.take_stop());
        assert!(!handle.take_stop());
    }

    #[test]
    fn take_open_settings_default_false() {
        let handle = create_handle();
        assert!(!handle.take_open_settings());
    }

    #[test]
    fn take_load_more_default_false() {
        let handle = create_handle();
        assert!(!handle.take_load_more());
    }

    #[test]
    fn take_list_tasks_default_false() {
        let handle = create_handle();
        assert!(!handle.take_list_tasks());
    }

    #[test]
    fn take_list_runbooks_default_false() {
        let handle = create_handle();
        assert!(!handle.take_list_runbooks());
    }

    #[test]
    fn take_list_equipment_default_false() {
        let handle = create_handle();
        assert!(!handle.take_list_equipment());
    }

    #[test]
    fn take_toggle_equipment_default_none() {
        let handle = create_handle();
        assert_eq!(handle.take_toggle_equipment(), None);
    }

    #[test]
    fn take_toggle_equipment_set_once() {
        let handle = create_handle();
        *handle.pending_toggle_equipment.lock().unwrap() = Some("eq-1".into());
        assert_eq!(handle.take_toggle_equipment(), Some("eq-1".into()));
        assert_eq!(handle.take_toggle_equipment(), None);
    }

    #[test]
    fn take_toggle_zoom_default_false() {
        let handle = create_handle();
        assert!(!handle.take_toggle_zoom());
    }

    #[test]
    fn take_toggle_zoom_set_true_then_resets() {
        let handle = create_handle();
        *handle.pending_toggle_zoom.lock().unwrap() = true;
        assert!(handle.take_toggle_zoom());
        assert!(!handle.take_toggle_zoom());
    }

    #[test]
    fn take_cancel_task_default_none() {
        let handle = create_handle();
        assert_eq!(handle.take_cancel_task(), None);
    }

    #[test]
    fn take_complete_task_default_none() {
        let handle = create_handle();
        assert_eq!(handle.take_complete_task(), None);
    }

    #[test]
    fn host_commands_start_drag() {
        let handle = create_handle();
        assert!(!handle.take_start_drag());
        handle.host_commands.push(OverlayHostCommand::StartDrag);
        assert!(handle.take_start_drag());
        assert!(!handle.take_start_drag());
    }

    #[test]
    fn host_commands_minimize() {
        let handle = create_handle();
        assert!(!handle.take_minimize());
        handle.host_commands.push(OverlayHostCommand::Minimize);
        assert!(handle.take_minimize());
        assert!(!handle.take_minimize());
    }

    #[test]
    fn host_commands_maximize_restore() {
        let handle = create_handle();
        assert!(!handle.take_maximize_restore());
        handle
            .host_commands
            .push(OverlayHostCommand::MaximizeRestore);
        assert!(handle.take_maximize_restore());
        assert!(!handle.take_maximize_restore());
    }

    #[test]
    fn host_commands_close_window() {
        let handle = create_handle();
        assert!(!handle.take_close_window());
        handle.host_commands.push(OverlayHostCommand::CloseWindow);
        assert!(handle.take_close_window());
        assert!(!handle.take_close_window());
    }

    #[test]
    fn host_commands_multiple_take_consumes_each() {
        let handle = create_handle();
        handle.host_commands.push(OverlayHostCommand::Minimize);
        handle.host_commands.push(OverlayHostCommand::CloseWindow);
        assert!(handle.take_minimize());
        assert!(!handle.take_minimize());
        assert!(handle.take_close_window());
        assert!(!handle.take_close_window());
    }

    #[test]
    fn window_id_returns_some() {
        let handle = create_handle();
        assert!(handle.window_id().is_some(), "window_id should return Some");
    }

    #[test]
    fn window_id_stable_across_calls() {
        let handle = create_handle();
        let id1 = handle.window_id();
        let id2 = handle.window_id();
        assert_eq!(id1, id2);
    }

    #[test]
    fn eval_no_webview_does_not_panic() {
        let handle = create_handle();
        let mut h = handle;
        h.webview = None;
        h.eval("console.log('no crash')");
    }

    #[test]
    fn send_no_webview_does_not_panic() {
        let handle = create_handle();
        let mut h = handle;
        h.webview = None;
        h.send(&json!({"type": "test"}));
    }

    #[test]
    fn show_window_clamps_size() {
        let handle = create_handle();
        handle.show_window(100.0, 100.0);
        let size = handle.window.inner_size();
        assert!(size.width >= 360u32);
        assert!(size.height >= 520u32);
    }

    #[test]
    fn show_window_accepts_reasonable_size() {
        let handle = create_handle();
        handle.show_window(800.0, 600.0);
        let size = handle.window.inner_size();
        assert!(size.width >= 700u32, "width should be close to 800");
        assert!(size.height >= 500u32, "height should be close to 600");
    }

    #[test]
    fn set_history_sends_json_no_crash() {
        let handle = create_handle();
        let entry = ChatEntry {
            role: EntryRole::User,
            content: "hello".into(),
            tool_calls: vec![],
        };
        handle.set_history(&[entry], false);
    }

    #[test]
    fn prepend_history_sends_json_no_crash() {
        let handle = create_handle();
        let entry = ChatEntry {
            role: EntryRole::Assistant,
            content: "world".into(),
            tool_calls: vec![],
        };
        handle.prepend_history(&[entry], true);
    }

    #[test]
    fn show_auth_sends_json_no_crash() {
        let handle = create_handle();
        let req = AuthRequest {
            request_id: "test-1".into(),
            source: "test".into(),
            tool: "bash".into(),
            command: "echo hello".into(),
        };
        handle.show_auth(&req);
    }

    #[test]
    fn show_settings_sends_json_no_crash() {
        let handle = create_handle();
        let config = SettingsConfig {
            api_key: "sk-test".into(),
            api_key_saved: true,
            api_base: "https://api.example.com".into(),
            model: "gpt-4".into(),
            personality: "default".into(),
            proxy_port: None,
            bg_enabled: None,
            bg_interval: None,
            bg_prompt: None,
            auto_evolve: None,
            max_context_tokens: None,
            mode: None,
        };
        handle.show_settings(&config);
    }

    #[test]
    fn show_tasks_sends_json_no_crash() {
        let handle = create_handle();
        handle.show_tasks(&[json!({"id": 1})]);
    }

    #[test]
    fn show_runbooks_sends_json_no_crash() {
        let handle = create_handle();
        handle.show_runbooks(&[json!({"name": "test"})]);
    }

    #[test]
    fn show_equipment_sends_json_no_crash() {
        let handle = create_handle();
        handle.show_equipment(&[json!({"item": "sword"})]);
    }

    #[test]
    fn complete_message_sends_json_no_crash() {
        let handle = create_handle();
        handle.complete_message();
    }

    #[test]
    fn toast_sends_json_no_crash() {
        let handle = create_handle();
        handle.toast("hello");
    }

    #[test]
    fn set_state_sends_json_no_crash() {
        let handle = create_handle();
        handle.set_state("idle");
    }

    #[test]
    fn clear_chat_sends_json_no_crash() {
        let handle = create_handle();
        handle.clear_chat();
    }

    #[test]
    fn push_delta_sends_json_no_crash() {
        let handle = create_handle();
        handle.push_delta("some delta content");
    }

    #[test]
    fn resize_webview_no_webview_does_not_panic() {
        let handle = create_handle();
        let mut h = handle;
        h.webview = None;
        h.resize_webview();
    }

    #[test]
    fn startup_error_diagnostics() {
        let handle = create_handle();
        let mut h = handle;
        h.startup_error = Some("simulated error".into());
        h.webview = None;
        let diag = h.host_diagnostics();
        assert_eq!(diag.platform, "macos");
        assert!(!diag.webview_available);
        assert_eq!(diag.startup_error.as_deref(), Some("simulated error"));
    }

    #[test]
    fn close_window_hides_then_show_restores() {
        let handle = create_handle();
        assert!(handle.window.is_visible());
        handle.close_window();
        assert!(!handle.window.is_visible());
        handle.show_window(600.0, 800.0);
        assert!(handle.window.is_visible());
    }

    #[test]
    fn minimize_window_sets_minimized() {
        let handle = create_handle();
        handle.minimize_window();
        assert!(handle.window.is_minimized());
    }

    #[test]
    fn handle_window_event_close_requested_hides() {
        let handle = create_handle();
        assert!(handle.window.is_visible());
        handle.handle_window_event(&WindowEvent::CloseRequested);
        assert!(!handle.window.is_visible());
    }

    #[test]
    fn handle_window_event_close_requested_returns_true() {
        let handle = create_handle();
        assert!(handle.handle_window_event(&WindowEvent::CloseRequested));
    }

    #[test]
    fn handle_window_event_resized_returns_true() {
        let handle = create_handle();
        // Use a dummy size — the actual resize happens in the event loop
        assert!(
            handle.handle_window_event(&WindowEvent::Resized(winit::dpi::PhysicalSize::new(
                800, 600
            )))
        );
    }

    #[test]
    fn handle_window_event_unknown_returns_false() {
        let handle = create_handle();
        // Focused is a no-op event that should return false
        assert!(!handle.handle_window_event(&WindowEvent::Focused(true)));
    }

    #[test]
    fn handle_window_event_scale_factor_changed_returns_true() {
        let handle = create_handle();
        assert!(
            handle.handle_window_event(&WindowEvent::ScaleFactorChanged {
                scale_factor: 2.0,
                inner_size: &mut winit::dpi::PhysicalSize::new(800, 600),
            })
        );
    }

    #[test]
    fn show_personality_picker_does_not_crash() {
        let handle = create_handle();
        handle.show_personality_picker();
    }
}
