use std::num::NonZeroU32;
use std::ops::Deref;
use std::sync::Arc;

use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};
use wry::Rect;

use crate::overlay_assets::select_overlay_asset;
use crate::overlay_host::{
    log_selected_asset, make_ipc_handler, overlay_diagnostics, webview_builder_for_asset,
    OverlayHandleExt, OverlayHostCommand, OverlayHostDiagnostics, OverlayState,
};

#[allow(unused_imports)]
pub use crate::overlay_contract::{
    AuthRequest, ChatEntry, EntryRole, OverlayToUiEvent, SettingsConfig, ToolCallEntry, ToolStatus,
};

pub struct OverlayHandle {
    pub state: OverlayState,
    window: Arc<Window>,
    webview: Option<wry::WebView>,
    fallback_surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
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
                tracing::warn!("windows webview eval error: {e}");
            }
        }
    }
}

impl OverlayHandle {
    pub fn eval(&self, js: &str) {
        if let Some(webview) = self.webview.as_ref() {
            if let Err(e) = webview.evaluate_script(js) {
                tracing::warn!("windows webview eval error: {e}");
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
        overlay_diagnostics("windows", self.webview.is_some(), self.startup_error.clone())
    }

    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
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
            WindowEvent::RedrawRequested if self.startup_error.is_some() => {
                self.render_startup_error();
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
            tracing::warn!("windows webview resize error: {e}");
        }
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

    pub fn start_drag_window(&self) {
        if let Err(e) = self.window.drag_window() {
            tracing::warn!("windows overlay drag_window failed: {e}");
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

    fn render_startup_error(&mut self) {
        let Some(surface) = self.fallback_surface.as_mut() else {
            return;
        };
        let Some(error) = self.startup_error.as_deref() else {
            return;
        };
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let Some(width) = NonZeroU32::new(size.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        draw_startup_error(&mut buffer, size.width, size.height, error);
        if let Err(e) = buffer.present() {
            tracing::warn!("windows startup error fallback present failed: {e}");
        }
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
            .expect("windows overlay window creation failed"),
    );

    let asset = select_overlay_asset();
    log_selected_asset("windows", &asset);
    let builder = webview_builder_for_asset(asset);

    let mut startup_error = None;
    let force_startup_error = std::env::var("ZHONGSHU_ORB_FORCE_WEBVIEW2_ERROR")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let builder = builder
        .with_user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Zhongshu/1.0")
        .with_ipc_handler(ipc_handler);

    let (webview, fallback_surface) = if force_startup_error {
        let message = "WebView2 unavailable: forced startup smoke".to_string();
        tracing::error!("{message}");
        window.set_title(&format!("Zhongshu - {message}"));
        startup_error = Some(message);
        window.request_redraw();
        (None, create_startup_error_surface(&window))
    } else {
        match builder.build_as_child(window.as_ref()) {
            Ok(webview) => (Some(webview), None),
            Err(e) => {
                let message = format!("WebView2 unavailable: {e}");
                tracing::error!("{message}");
                window.set_title(&format!("Zhongshu - {message}"));
                startup_error = Some(message);
                window.request_redraw();
                (None, create_startup_error_surface(&window))
            }
        }
    };

    let handle = OverlayHandle {
        state,
        window,
        webview,
        fallback_surface,
        startup_error,
    };
    handle.show_window(width, height);
    handle
}

fn create_startup_error_surface(
    window: &Arc<Window>,
) -> Option<softbuffer::Surface<Arc<Window>, Arc<Window>>> {
    match softbuffer::Context::new(window.clone())
        .and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()))
    {
        Ok(surface) => Some(surface),
        Err(surface_error) => {
            tracing::warn!("windows startup error fallback unavailable: {surface_error}");
            None
        }
    }
}

fn draw_startup_error(buffer: &mut [u32], width: u32, height: u32, error: &str) {
    let width = width as usize;
    let height = height as usize;
    buffer.fill(argb(255, 10, 16, 28));
    fill_rect(
        buffer,
        width,
        height,
        0,
        0,
        width,
        56,
        argb(255, 15, 24, 40),
    );
    fill_rect(
        buffer,
        width,
        height,
        0,
        55,
        width,
        1,
        argb(255, 43, 58, 83),
    );

    let panel_x = 36;
    let panel_y = 92;
    let panel_w = width.saturating_sub(72).max(1);
    fill_rect(
        buffer,
        width,
        height,
        panel_x,
        panel_y,
        panel_w,
        220,
        argb(255, 17, 24, 39),
    );
    fill_rect(
        buffer,
        width,
        height,
        panel_x,
        panel_y,
        6,
        220,
        argb(255, 248, 113, 113),
    );

    draw_text(
        buffer,
        width,
        height,
        38,
        20,
        "ZHONGSHU",
        3,
        argb(255, 238, 245, 255),
    );
    draw_text(
        buffer,
        width,
        height,
        panel_x + 26,
        panel_y + 30,
        "STARTUP ERROR",
        3,
        argb(255, 248, 113, 113),
    );
    draw_text(
        buffer,
        width,
        height,
        panel_x + 26,
        panel_y + 76,
        "WEBVIEW2 IS UNAVAILABLE",
        2,
        argb(255, 238, 245, 255),
    );
    draw_text(
        buffer,
        width,
        height,
        panel_x + 26,
        panel_y + 112,
        "INSTALL MICROSOFT EDGE WEBVIEW2 RUNTIME",
        2,
        argb(255, 166, 184, 212),
    );
    draw_text(
        buffer,
        width,
        height,
        panel_x + 26,
        panel_y + 140,
        "THEN RESTART ZHONGSHU",
        2,
        argb(255, 166, 184, 212),
    );
    let detail: String = error
        .chars()
        .map(|ch| if ch.is_ascii() { ch } else { '?' })
        .take(96)
        .collect();
    draw_text(
        buffer,
        width,
        height,
        panel_x + 26,
        panel_y + 178,
        &detail.to_uppercase(),
        1,
        argb(255, 113, 131, 160),
    );
}

fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    let end_y = (y + h).min(height);
    let end_x = (x + w).min(width);
    for row in y.min(height)..end_y {
        let offset = row * width;
        for col in x.min(width)..end_x {
            buffer[offset + col] = color;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
    color: u32,
) {
    let mut cursor_x = x;
    let scale = scale.max(1);
    for ch in text.chars() {
        draw_glyph(buffer, width, height, cursor_x, y, ch, scale, color);
        cursor_x += 6 * scale;
        if cursor_x >= width.saturating_sub(6 * scale) {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    ch: char,
    scale: usize,
    color: u32,
) {
    let glyph = glyph_rows(ch);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) == 0 {
                continue;
            }
            fill_rect(
                buffer,
                width,
                height,
                x + col * scale,
                y + row * scale,
                scale,
                scale,
                color,
            );
        }
    }
}

fn glyph_rows(ch: char) -> [u8; 7] {
    match ch {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0F, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0F],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0F, 0x10, 0x10, 0x13, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0E],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x0A, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x10, 0x1E, 0x01, 0x01, 0x1E],
        '6' => [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],
        ':' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        ' ' => [0; 7],
        _ => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
    }
}
