use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use zhongshu_core::event::AgentState;

// ── Windows: transparent orb ────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod orb {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Instant;
    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event_loop::ActiveEventLoop;
    use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
    #[cfg(target_os = "windows")]
    use winit::platform::windows::WindowAttributesExtWindows;
    use zhongshu_core::event::AgentState;
    use crate::render::{self, OrbState};

    pub struct OrbIndicator {
        window: Arc<Window>,
        surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
        state: AgentState,
        start_time: Instant,
    }

    fn to_orb_state(s: AgentState) -> OrbState {
        match s {
            AgentState::Idle => OrbState::Idle,
            AgentState::Thinking => OrbState::Thinking { progress: 0.0 },
            AgentState::Executing => OrbState::Executing { pulse: 0.0 },
            AgentState::Done { success } => OrbState::Done { success },
        }
    }

    impl OrbIndicator {
        pub fn create(el: &ActiveEventLoop, size: u32) -> Self {
            let mut attrs = WindowAttributes::default()
                .with_title("zhongshu")
                .with_inner_size(LogicalSize::new(size, size))
                .with_resizable(false).with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_transparent(true).with_active(false);
            #[cfg(target_os = "windows")]
            { attrs = attrs.with_skip_taskbar(true); }
            let w = Arc::new(el.create_window(attrs).unwrap());
        if let Some(m) = el.primary_monitor() {
            let p = m.position(); let s = m.size();
            let x = p.x + s.width as i32 / 2 - size as i32 / 2;
            let y = p.y + s.height as i32 / 2 - size as i32 / 2;
            let _ = w.set_outer_position(PhysicalPosition::new(x.max(0), y.max(0)));
        }
            let ctx = softbuffer::Context::new(w.clone()).unwrap();
            let surface = softbuffer::Surface::new(&ctx, w.clone()).unwrap();
            w.request_redraw();
            OrbIndicator { window: w.clone(), surface, state: AgentState::Idle, start_time: Instant::now() }
        }
        pub fn set_state(&mut self, state: AgentState) { self.state = state; self.window.request_redraw(); }
        pub fn window(&self) -> &Arc<Window> { &self.window }
        pub fn window_id(&self) -> WindowId { self.window.id() }
        pub fn render(&mut self) {
            let sz = self.window.inner_size(); let (ww, hh) = (sz.width, sz.height);
            if ww == 0 || hh == 0 { return; }
            self.surface.resize(NonZeroU32::new(ww).unwrap(), NonZeroU32::new(hh).unwrap()).ok();
            let mut buf = match self.surface.buffer_mut() { Ok(b) => b, Err(_) => return };
            render::draw_orb(&mut buf, ww, hh, to_orb_state(self.state), self.start_time.elapsed().as_secs_f64());
            buf.present().unwrap();
            if !matches!(self.state, AgentState::Idle) { self.window.request_redraw(); }
        }
    }
}

// ── Linux: system tray ──────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub mod tray {
    use std::sync::Arc;
    use crossbeam_channel::{self, Receiver, Sender};
    use ksni::TrayMethods;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::WindowId;
    use zhongshu_core::event::AgentState;

    #[derive(Debug, Clone)]
    pub enum TrayEvent {
        OpenOverlay,
        NewConversation,
        Quit,
    }

    pub struct TrayIndicator {
        pub rx: Receiver<TrayEvent>,
        handle: Option<ksni::Handle<KsniTray>>,
    }

    struct KsniTray {
        state: Arc<std::sync::Mutex<AgentState>>,
        tx: Sender<TrayEvent>,
    }

    impl ksni::Tray for KsniTray {
        fn id(&self) -> String { "zhongshu".into() }
        fn title(&self) -> String {
            let s = *self.state.lock().unwrap();
            match s {
                AgentState::Idle => "中书".into(),
                AgentState::Thinking => "中书（思考中）".into(),
                AgentState::Executing => "中书（执行中）".into(),
                AgentState::Done { success: true } => "中书（完成）".into(),
                AgentState::Done { success: false } => "中书（出错）".into(),
            }
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let s = *self.state.lock().unwrap();
            let desc = match s {
                AgentState::Idle => "就绪",
                AgentState::Thinking => "正在思考...",
                AgentState::Executing => "正在执行工具...",
                AgentState::Done { success: true } => "任务完成",
                AgentState::Done { success: false } => "任务失败",
            };
            ksni::ToolTip {
                icon_name: "".into(),
                icon_pixmap: icon_pixmap(s),
                title: "中书".into(),
                description: desc.into(),
            }
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            icon_pixmap(*self.state.lock().unwrap())
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: "打开".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::OpenOverlay);
                    }),
                    ..Default::default()
                }.into(),
                StandardItem {
                    label: "新对话".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::NewConversation);
                    }),
                    ..Default::default()
                }.into(),
                MenuItem::Separator,
                StandardItem {
                    label: "退出".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::Quit);
                    }),
                    ..Default::default()
                }.into(),
            ]
        }
    }

    fn icon_pixmap(state: AgentState) -> Vec<ksni::Icon> {
        let size: i32 = 32;
        let mut data = vec![0u8; (size * size * 4) as usize];
        let (r, g, b): (u8, u8, u8) = match state {
            AgentState::Idle => (60, 200, 60),
            AgentState::Thinking => (220, 180, 40),
            AgentState::Executing => (220, 60, 40),
            AgentState::Done { success: true } => (60, 200, 60),
            AgentState::Done { success: false } => (220, 40, 40),
        };
        let cx = size as f32 / 2.0;
        let cy = size as f32 / 2.0;
        let r2 = (size as f32 / 2.0 - 2.0).powi(2);
        for y in 0..size {
            for x in 0..size {
                let idx = ((y * size + x) * 4) as usize;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                if dx * dx + dy * dy <= r2 {
                    data[idx] = 255; data[idx + 1] = r; data[idx + 2] = g; data[idx + 3] = b;
                }
            }
        }
        vec![ksni::Icon { width: size, height: size, data }]
    }

    impl TrayIndicator {
        pub fn create(_el: &ActiveEventLoop) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            let state = Arc::new(std::sync::Mutex::new(AgentState::Idle));
            let tray = KsniTray { state: state.clone(), tx };

            let handle = tokio::runtime::Handle::current().block_on(async {
                tray.spawn().await
            }).expect("ksni tray spawn");

            tracing::info!("ksni tray created");
            TrayIndicator { rx, handle: Some(handle) }
        }

        pub fn set_state(&mut self, state: AgentState) {
            if let Some(ref handle) = self.handle {
                if handle.is_closed() { return; }
                let _ = tokio::runtime::Handle::current().block_on(async {
                    handle.update(|tray: &mut KsniTray| {
                        *tray.state.lock().unwrap() = state;
                    }).await
                });
            }
        }

        pub fn window_id(&self) -> Option<WindowId> { None }
        pub fn render(&mut self) {}
    }

    impl Drop for TrayIndicator {
        fn drop(&mut self) {
            // ksni::Handle::drop tries to shut down the D-Bus connection
            // synchronously, which segfaults at process exit when the tokio
            // runtime may already be gone.  Forget the handle to prevent Drop
            // from running — the OS will clean up the socket.
            if let Some(handle) = self.handle.take() {
                std::mem::forget(handle);
            }
        }
    }
}

pub enum Indicator {
    #[cfg(not(target_os = "linux"))]
    Orb(orb::OrbIndicator),
    #[cfg(target_os = "linux")]
    Tray(tray::TrayIndicator),
}

impl Indicator {
    #[cfg(not(target_os = "linux"))]
    pub fn create(el: &ActiveEventLoop, orb_size: u32) -> Self {
        Indicator::Orb(orb::OrbIndicator::create(el, orb_size))
    }
    #[cfg(target_os = "linux")]
    pub fn create(el: &ActiveEventLoop, _orb_size: u32) -> Self {
        Indicator::Tray(tray::TrayIndicator::create(el))
    }

    pub fn set_state(&mut self, state: AgentState) {
        match self {
            #[cfg(not(target_os = "linux"))] Indicator::Orb(o) => o.set_state(state),
            #[cfg(target_os = "linux")] Indicator::Tray(t) => t.set_state(state),
        }
    }

    pub fn window(&self) -> Option<&Arc<Window>> {
        match self {
            #[cfg(not(target_os = "linux"))] Indicator::Orb(o) => Some(o.window()),
            #[cfg(target_os = "linux")] Indicator::Tray(_) => None,
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        match self {
            #[cfg(not(target_os = "linux"))] Indicator::Orb(o) => Some(o.window_id()),
            #[cfg(target_os = "linux")] Indicator::Tray(t) => t.window_id(),
        }
    }

    pub fn render(&mut self) {
        match self {
            #[cfg(not(target_os = "linux"))] Indicator::Orb(o) => o.render(),
            #[cfg(target_os = "linux")] Indicator::Tray(t) => t.render(),
        }
    }
}
