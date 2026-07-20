use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use zhongshu_core::event::AgentState;

// ── State color palette (shared by orb + tray) ───────────────────────

fn state_color(state: AgentState) -> (u8, u8, u8) {
    match state {
        AgentState::Idle => (57, 100, 254),      // Zhongshu primary blue
        AgentState::Thinking => (68, 152, 255),  // blue-cyan flow
        AgentState::Executing => (35, 205, 220), // active tool energy
        AgentState::Submitted => (234, 179, 8),  // awaiting verification
        AgentState::Done { success: true } => (76, 210, 135),
        AgentState::Done { success: false } => (235, 76, 86),
    }
}

// ── Windows: transparent orb ────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod orb {
    use crate::render;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Instant;
    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event_loop::ActiveEventLoop;
    #[cfg(target_os = "windows")]
    use winit::platform::windows::WindowAttributesExtWindows;
    use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
    use zhongshu_core::event::AgentState;

    use super::state_color;

    fn to_orb_mode(state: AgentState) -> crate::render::OrbMode {
        match state {
            AgentState::Idle => crate::render::OrbMode::Idle,
            AgentState::Thinking => crate::render::OrbMode::Thinking,
            AgentState::Executing => crate::render::OrbMode::Executing,
            AgentState::Submitted => crate::render::OrbMode::DoneSuccess,
            AgentState::Done { success: true } => crate::render::OrbMode::DoneSuccess,
            AgentState::Done { success: false } => crate::render::OrbMode::DoneFailure,
        }
    }

    /// Smooth color interpolator for state transitions.
    struct ColorLerp {
        current: (f32, f32, f32),
        target: (f32, f32, f32),
        transition_t0: f64,
    }

    impl ColorLerp {
        fn new(r: u8, g: u8, b: u8) -> Self {
            ColorLerp {
                current: (r as f32, g as f32, b as f32),
                target: (r as f32, g as f32, b as f32),
                transition_t0: 0.0,
            }
        }

        fn set_target(&mut self, r: u8, g: u8, b: u8, t: f64) {
            let prev = self.get(t);
            self.current = (prev.0 as f32, prev.1 as f32, prev.2 as f32);
            self.target = (r as f32, g as f32, b as f32);
            self.transition_t0 = t;
        }

        fn get(&self, t: f64) -> (u8, u8, u8) {
            let dur = 0.3;
            let frac = ((t - self.transition_t0) / dur).min(1.0).max(0.0);
            let ease = (frac * frac * (3.0 - 2.0 * frac)) as f32;
            (
                (self.current.0 + (self.target.0 - self.current.0) * ease) as u8,
                (self.current.1 + (self.target.1 - self.current.1) * ease) as u8,
                (self.current.2 + (self.target.2 - self.current.2) * ease) as u8,
            )
        }
    }

    pub struct OrbIndicator {
        window: Arc<Window>,
        surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
        state: AgentState,
        color: ColorLerp,
        start_time: Instant,
        /// Visual Done state held at least this long before decaying to Idle.
        done_until: Instant,
    }

    impl OrbIndicator {
        pub fn create(el: &ActiveEventLoop, size: u32) -> Self {
            let mut attrs = WindowAttributes::default()
                .with_title("zhongshu")
                .with_inner_size(LogicalSize::new(size, size))
                .with_resizable(false)
                .with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_transparent(true)
                .with_active(false);
            #[cfg(target_os = "windows")]
            {
                attrs = attrs.with_skip_taskbar(true);
            }
            let w = Arc::new(el.create_window(attrs).unwrap());

            // Default: bottom-right, 20% inset from edges.
            if let Some(m) = el.primary_monitor() {
                let p = m.position();
                let s = m.size();
                let x = p.x + s.width as i32 - size as i32 - (s.width as f64 * 0.2) as i32;
                let y = p.y + s.height as i32 - size as i32 - (s.height as f64 * 0.2) as i32;
                let _ = w.set_outer_position(PhysicalPosition::new(x.max(0), y.max(0)));
            }

            // Override with saved position from last drag.
            let pos_path = crate::config::config_dir().join("orb_pos.json");
            if let Ok(data) = std::fs::read_to_string(&pos_path) {
                if let Ok((x, y)) = serde_json::from_str::<(i32, i32)>(&data) {
                    let _ = w.set_outer_position(PhysicalPosition::new(x, y));
                }
            }

            let ctx = softbuffer::Context::new(w.clone()).unwrap();
            let surface = softbuffer::Surface::new(&ctx, w.clone()).unwrap();
            let c = state_color(AgentState::Idle);
            w.request_redraw();
            OrbIndicator {
                window: w.clone(),
                surface,
                state: AgentState::Idle,
                color: ColorLerp::new(c.0, c.1, c.2),
                start_time: Instant::now(),
                done_until: Instant::now(),
            }
        }

        pub fn set_state(&mut self, state: AgentState) {
            self.state = state;
            let c = state_color(state);
            let t = self.start_time.elapsed().as_secs_f64();
            self.color.set_target(c.0, c.1, c.2, t);
            // Ensure Done state is visible for at least 900ms.
            if matches!(state, AgentState::Done { .. } | AgentState::Submitted) {
                self.done_until = Instant::now() + std::time::Duration::from_millis(1100);
            }
            self.window.request_redraw();
        }

        pub fn window(&self) -> &Arc<Window> {
            &self.window
        }
        pub fn window_id(&self) -> WindowId {
            self.window.id()
        }

        pub fn save_position(&self) {
            if let Ok(pos) = self.window.outer_position() {
                let path = crate::config::config_dir().join("orb_pos.json");
                if let Ok(data) = serde_json::to_string(&(pos.x, pos.y)) {
                    let _ = std::fs::write(path, data);
                }
            }
        }

        pub fn render(&mut self) {
            // Decay Done → Idle after minimum display time.
            let effective = if matches!(self.state, AgentState::Done { .. } | AgentState::Submitted)
                && Instant::now() >= self.done_until
            {
                AgentState::Idle
            } else {
                self.state
            };

            let sz = self.window.inner_size();
            let (ww, hh) = (sz.width, sz.height);
            if ww == 0 || hh == 0 {
                return;
            }
            self.surface
                .resize(NonZeroU32::new(ww).unwrap(), NonZeroU32::new(hh).unwrap())
                .ok();
            let mut buf = match self.surface.buffer_mut() {
                Ok(b) => b,
                Err(_) => return,
            };

            let t = self.start_time.elapsed().as_secs_f64();
            let (cr, cg, cb) = self.color.get(t);
            let mode = to_orb_mode(effective);
            render::draw_orb(&mut buf, ww, hh, cr, cg, cb, t, mode);

            buf.present().unwrap();

            self.window.request_redraw();
        }
    }
}

// ── Linux: system tray with breathing animation ─────────────────────

#[cfg(target_os = "linux")]
pub mod tray {
    use crossbeam_channel::{self, Receiver, Sender};
    use ksni::TrayMethods;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::WindowId;
    use zhongshu_core::event::AgentState;

    use super::state_color;

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
        breath_phase: Arc<AtomicU32>,
        active: Arc<AtomicBool>,
    }

    impl ksni::Tray for KsniTray {
        fn id(&self) -> String {
            "zhongshu".into()
        }
        fn title(&self) -> String {
            let s = *self.state.lock().unwrap();
            match s {
                AgentState::Idle => "中书".into(),
                AgentState::Thinking => "中书（思考中）".into(),
                AgentState::Executing => "中书（执行中）".into(),
                AgentState::Submitted => "中书（已提交，未验证）".into(),
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
                AgentState::Submitted => "结果已提交，尚未验证",
                AgentState::Done { success: true } => "任务完成",
                AgentState::Done { success: false } => "任务失败",
            };
            let phase = self.breath_phase.load(Ordering::Relaxed) as f64;
            ksni::ToolTip {
                icon_name: "".into(),
                icon_pixmap: icon_pixmap(s, phase),
                title: "中书".into(),
                description: desc.into(),
            }
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let phase = self.breath_phase.load(Ordering::Relaxed) as f64;
            icon_pixmap(*self.state.lock().unwrap(), phase)
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: "打开".into(),
                    icon_name: "window-new".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::OpenOverlay);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "清空对话".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::NewConversation);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "退出".into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(TrayEvent::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    fn icon_pixmap(state: AgentState, _phase: f64) -> Vec<ksni::Icon> {
        // Smaller size set — tray is for status indication, not animation.
        let sizes: &[i32] = &[16, 22, 32];
        let (r, g, b) = state_color(state);

        sizes
            .iter()
            .map(|size| {
                let mut data = vec![0u8; (size * size * 4) as usize];
                let cx = *size as f32 / 2.0;
                let cy = *size as f32 / 2.0;
                let outer_r = *size as f32 / 2.0 - 1.0;
                let core_r = (outer_r * 0.5).max(2.0);
                let outer_r2 = outer_r.powi(2);
                let core_r2 = core_r.powi(2);

                for y in 0..*size {
                    for x in 0..*size {
                        let idx = ((y * size + x) * 4) as usize;
                        let dx = x as f32 - cx;
                        let dy = y as f32 - cy;
                        let dist2 = dx * dx + dy * dy;
                        if dist2 > outer_r2 {
                            continue;
                        }
                        let dist = dist2.sqrt();
                        let frac = 1.0 - (dist / outer_r);
                        let alpha = if dist2 <= core_r2 {
                            255
                        } else {
                            (255.0 * frac * frac) as u8
                        };
                        data[idx] = alpha;
                        data[idx + 1] = r;
                        data[idx + 2] = g;
                        data[idx + 3] = b;
                    }
                }
                ksni::Icon {
                    width: *size,
                    height: *size,
                    data,
                }
            })
            .collect()
    }

    impl TrayIndicator {
        pub fn create(_el: &ActiveEventLoop) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            let state = Arc::new(std::sync::Mutex::new(AgentState::Idle));
            let breath_phase = Arc::new(AtomicU32::new(0));
            let active = Arc::new(AtomicBool::new(false));
            let tray = KsniTray {
                state: state.clone(),
                tx,
                breath_phase: breath_phase.clone(),
                active: active.clone(),
            };

            let handle = tokio::runtime::Handle::current()
                .block_on(async { tray.spawn().await })
                .expect("ksni tray spawn");

            // Breathing timer: low frequency — tray pixmap is not an animation canvas.
            let bp = breath_phase.clone();
            let h = handle.clone();
            let act = active.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let start = tokio::time::Instant::now();
                loop {
                    let is_active = act.load(Ordering::Relaxed);
                    // Idle: 1 Hz, active: 10 Hz (reduced from 20 Hz).
                    let ms = if is_active { 100 } else { 1000 };
                    tokio::time::sleep(Duration::from_millis(ms)).await;

                    let elapsed = start.elapsed().as_secs_f64();
                    bp.store((elapsed * 10.0) as u32, Ordering::Relaxed);
                    let _ = h.update(|_: &mut KsniTray| {}).await;
                }
            });

            tracing::info!("ksni tray created");
            TrayIndicator {
                rx,
                handle: Some(handle),
            }
        }

        pub fn set_state(&mut self, state: AgentState) {
            if let Some(ref handle) = self.handle {
                if handle.is_closed() {
                    return;
                }
                let _ = tokio::runtime::Handle::current().block_on(async {
                    handle
                        .update(|tray: &mut KsniTray| {
                            *tray.state.lock().unwrap() = state;
                            tray.active
                                .store(!matches!(state, AgentState::Idle), Ordering::Relaxed);
                        })
                        .await
                });
            }
        }

        pub fn window_id(&self) -> Option<WindowId> {
            None
        }
        pub fn render(&mut self) {}
    }

    impl Drop for TrayIndicator {
        fn drop(&mut self) {
            // Forget our handle clone; the background task keeps its own clone
            // which will be dropped by tokio runtime shutdown.
            if let Some(handle) = self.handle.take() {
                std::mem::forget(handle);
            }
        }
    }
}

// ── Shared Indicator enum ───────────────────────────────────────────

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
            #[cfg(not(target_os = "linux"))]
            Indicator::Orb(o) => o.set_state(state),
            #[cfg(target_os = "linux")]
            Indicator::Tray(t) => t.set_state(state),
        }
    }

    pub fn window(&self) -> Option<&Arc<Window>> {
        match self {
            #[cfg(not(target_os = "linux"))]
            Indicator::Orb(o) => Some(o.window()),
            #[cfg(target_os = "linux")]
            Indicator::Tray(_) => None,
        }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        match self {
            #[cfg(not(target_os = "linux"))]
            Indicator::Orb(o) => Some(o.window_id()),
            #[cfg(target_os = "linux")]
            Indicator::Tray(t) => t.window_id(),
        }
    }

    pub fn render(&mut self) {
        match self {
            #[cfg(not(target_os = "linux"))]
            Indicator::Orb(o) => o.render(),
            #[cfg(target_os = "linux")]
            Indicator::Tray(t) => t.render(),
        }
    }

    pub fn save_position(&self) {
        #[cfg(not(target_os = "linux"))]
        match self {
            Indicator::Orb(o) => o.save_position(),
        }
    }
}
