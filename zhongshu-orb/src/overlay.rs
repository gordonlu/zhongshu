use std::ops::Deref;
use std::sync::Mutex;

use glib;
use gtk::gdk::prelude::MonitorExt;
use gtk::prelude::*;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use wry::WebViewBuilderExtUnix;

use crate::overlay_assets::select_overlay_asset;
use crate::overlay_host::{
    log_selected_asset, make_ipc_handler, overlay_diagnostics, webview_builder_for_asset,
    OverlayHandleExt, OverlayHostCommand, OverlayHostDiagnostics, OverlayState,
};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, SettingsUpdate,
    ToolCallEntry, ToolStatus,
};

// ── Message types ────────────────────────────────────────────────────

// ── Global GTK thread state ─────────────────────────────────────────

pub(crate) enum GtkCommand {
    Eval(String),
    Show(f32, f32),
    Hide,
    Minimize,
    MaximizeRestore,
    CloseWindow,
    StartDrag,
}

pub(crate) static GTK_TX: once_cell::sync::Lazy<crossbeam_channel::Sender<GtkCommand>> =
    once_cell::sync::Lazy::new(|| {
        let (tx_i, rx_i) = crossbeam_channel::unbounded::<GtkCommand>();
        std::thread::spawn(move || {
            gtk::init().expect("GTK init failed");
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.set_title("Zhongshu");
            window.set_default_size(520, 800);
            window.set_default_size(520, 800);
            window.set_decorated(false);
            window.set_resizable(true);
            window.connect_delete_event(|w, _| {
                w.hide();
                glib::Propagation::Stop
            });

            let asset = select_overlay_asset();
            log_selected_asset("gtk", &asset);
            let builder = webview_builder_for_asset(asset);

            let webview = builder
                .with_user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.7827.102 Safari/537.36")
                .with_ipc_handler(move |request: http::Request<String>| {
                    IPC_HANDLER.lock().unwrap().as_ref().map(|h| h(request));
                })
                .build_gtk(&window)
                .expect("wry WebView build_gtk failed");

            window.show_all();

            glib::idle_add_local(move || {
                while let Ok(cmd) = rx_i.try_recv() {
                    match cmd {
                        GtkCommand::Eval(js) => {
                            if let Err(e) = webview.evaluate_script(&js) {
                                tracing::warn!("webview eval error: {e}");
                            }
                        }
                        GtkCommand::Show(w, h) => {
                            let (screen_w, screen_h) = gtk::gdk::Display::default()
                                .and_then(|display| display.primary_monitor())
                                .map(|monitor| {
                                    let area = monitor.workarea();
                                    (area.width() as f32, area.height() as f32)
                                })
                                .unwrap_or((1280.0, 900.0));
                            let max_w = (screen_w * 0.96).max(320.0);
                            let max_h = (screen_h * 0.92).max(480.0);
                            let clamped_w = w.min(max_w).max(360.0) as i32;
                            let clamped_h = h.min(max_h).max(520.0) as i32;
                            window.resize(clamped_w, clamped_h);
                            window.set_default_size(clamped_w, clamped_h);
                            window.show_all();
                        }
                        GtkCommand::Hide => {
                            window.hide();
                        }
                        GtkCommand::Minimize => {
                            window.iconify();
                        }
                        GtkCommand::MaximizeRestore => {
                            if window.is_maximized() {
                                window.unmaximize();
                            } else {
                                window.maximize();
                            }
                        }
                        GtkCommand::CloseWindow => {
                            window.close();
                        }
                        GtkCommand::StartDrag => {
                            window.begin_move_drag(1, 0, 0, 0);
                        }
                    }
                }
                glib::ControlFlow::Continue
            });

            gtk::main();
        });
        tx_i
    });

/// Thread-safe IPC handler set by the current OverlayHandle.
static IPC_HANDLER: once_cell::sync::Lazy<
    Mutex<Option<Box<dyn Fn(http::Request<String>) + Send>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(None));

// ── Overlay handle ───────────────────────────────────────────────────

pub struct OverlayHandle {
    pub state: OverlayState,
}

impl Deref for OverlayHandle {
    type Target = OverlayState;
    fn deref(&self) -> &OverlayState {
        &self.state
    }
}

impl OverlayHandleExt for OverlayHandle {
    fn webview_eval(&self, js: &str) {
        if let Err(e) = GTK_TX.send(GtkCommand::Eval(js.to_string())) {
            tracing::warn!("gtk tx send error: {e}");
        }
    }
}

impl OverlayHandle {
    pub fn eval(&self, js: &str) {
        self.webview_eval(js);
    }

    pub fn show_window(&self, width: f32, height: f32) {
        let _ = GTK_TX.send(GtkCommand::Show(width, height));
    }

    pub fn window_id(&self) -> Option<WindowId> {
        None
    }

    pub fn host_diagnostics(&self) -> OverlayHostDiagnostics {
        overlay_diagnostics("gtk", true, None)
    }

    pub fn handle_window_event(&self, _event: &WindowEvent) -> bool {
        false
    }

    pub fn start_drag_window(&self) {
        let _ = GTK_TX.send(GtkCommand::StartDrag);
    }

    pub fn minimize_window(&self) {
        let _ = GTK_TX.send(GtkCommand::Minimize);
    }

    pub fn maximize_restore_window(&self) {
        let _ = GTK_TX.send(GtkCommand::MaximizeRestore);
    }

    pub fn close_window(&self) {
        let _ = GTK_TX.send(GtkCommand::CloseWindow);
    }

    pub fn take_start_drag(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::StartDrag)
    }

    pub fn take_minimize(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::Minimize)
    }

    pub fn take_maximize_restore(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::MaximizeRestore)
    }

    pub fn take_close_window(&self) -> bool {
        self.host_commands.take(OverlayHostCommand::CloseWindow)
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        let _ = GTK_TX.send(GtkCommand::Hide);
    }
}

/// Show the overlay window and return a handle for IPC.
pub fn show(_event_loop: &ActiveEventLoop, width: f32, height: f32) -> OverlayHandle {
    let _ = *GTK_TX;

    let state = OverlayState::new();
    let clones = state.clone_for_ipc();

    *IPC_HANDLER.lock().unwrap() = Some(Box::new(make_ipc_handler(clones)));

    let _ = GTK_TX.send(GtkCommand::Show(width, height));

    OverlayHandle { state }
}
