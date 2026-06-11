use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use egui::{self, Color32};
use pulldown_cmark::{Event, Tag, TagEnd, Options, Parser};
use winit::dpi::LogicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};
use crate::gpu::{GpuContext, WindowSurface};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolCallId(u64);

impl ToolCallId {
    pub fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        ToolCallId(N.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum EntryRole { User, Assistant, System }

impl std::fmt::Debug for EntryRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryRole::User => write!(f, "User"),
            EntryRole::Assistant => write!(f, "Assistant"),
            EntryRole::System => write!(f, "System"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToolStatus { Running, Done { success: bool, duration_ms: u64 } }

#[derive(Clone)]
pub struct ToolCallEntry {
    pub _id: ToolCallId,
    pub name: String,
    pub status: ToolStatus,
    pub started_at: std::time::Instant,
}

impl ToolCallEntry {
    pub fn new(name: String) -> Self {
        ToolCallEntry {
            _id: ToolCallId::new(),
            name,
            status: ToolStatus::Running,
            started_at: std::time::Instant::now()
        }
    }
}

pub struct ChatEntry {
    pub role: EntryRole,
    pub content: String,
    pub tool_calls: Vec<ToolCallEntry>,
    pub cached_job: Option<(u64, egui::text::LayoutJob)>,
}

pub struct StreamingState {
    pub role: EntryRole,
    pub content: String,
    pub tool_calls: Vec<ToolCallEntry>,
    cached_job: Option<(u64, egui::text::LayoutJob)>,
}

impl StreamingState {
    pub fn new(role: EntryRole) -> Self {
        StreamingState {
            role,
            content: String::new(),
            tool_calls: Vec::new(),
            cached_job: None
        }
    }

    pub fn finish(self) -> ChatEntry {
        ChatEntry {
            role: self.role,
            content: self.content,
            tool_calls: self.tool_calls,
            cached_job: self.cached_job
        }
    }
}

#[derive(Clone)]
pub struct ApprovalRequest {
    pub tool: String,
    pub program: String,
    pub command: String,
    pub source: String,
}

pub struct Overlay {
    pub window: Arc<Window>,
    pub state: egui_winit::State,
    pub gpu: Arc<GpuContext>,
    pub surface: WindowSurface,
    pub renderer: egui_wgpu::Renderer,
    pub input: String,
    pub entries: Vec<ChatEntry>,
    pub streaming: Option<StreamingState>,
    pub approval_request: Option<ApprovalRequest>,
    pub request_quit: bool,
    pub request_new_conversation: bool,
    pub pending_personality: Option<String>,
    pub request_stop: bool,
    ctx: egui::Context,
}

impl Overlay {
    pub fn new(
        el: &ActiveEventLoop,
        gpu: Arc<GpuContext>,
        width: f32,
        height: f32,
        font_paths: &[String],
    ) -> Self {
        let attrs = WindowAttributes::default()
            .with_title("Zhongshu")
            .with_inner_size(LogicalSize::new(width, height))
            .with_resizable(true)
            .with_decorations(true)
            .with_window_level(WindowLevel::Normal);

        let w = Arc::new(el.create_window(attrs).unwrap());
        let id = w.id();

        let ctx = egui::Context::default();
        configure_fonts(&ctx, font_paths);
        apply_theme(&ctx);

        let vp_id = egui::viewport::ViewportId::from_hash_of(id);

        let surface = WindowSurface::new(
            &gpu,
            &w,
            w.inner_size().width,
            w.inner_size().height
        );

        let format = surface.format();

        let state = egui_winit::State::new(ctx.clone(), vp_id, &w, None, None, None);

        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            format,
            None,
            1,
            false
        );

        let _ = w.focus_window();
        w.request_redraw();

        Self {
            window: w.clone(),
            state,
            gpu,
            surface,
            renderer,
            input: String::new(),
            entries: Vec::new(),
            streaming: None,
            approval_request: None,
            request_quit: false,
            request_new_conversation: false,
            pending_personality: None,
            request_stop: false,
            ctx,
        }
    }

    pub fn render(&mut self) -> Option<String> {
        let sz = self.window.inner_size();
        if sz.width == 0 || sz.height == 0 {
            return None;
        }

        let raw = self.state.take_egui_input(&self.window);
        let mut send: Option<String> = None;

        // Ensure IME is enabled and cursor area is near the bottom input field.
        self.window.set_ime_allowed(true);
        let input_h = 48.0; // approximate input field height
        self.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(0, sz.height.saturating_sub(input_h as u32)),
            winit::dpi::PhysicalSize::new(sz.width, input_h as u32),
        );

        let out = self.ctx.run(raw, |cx| {
            egui::TopBottomPanel::bottom("input").show(cx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(34, 34, 38))
                    .corner_radius(8)
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52)))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.input)
                                .hint_text("给 中书 发消息...")
                                .desired_width(f32::INFINITY)
                        );

                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let msg = self.input.trim().to_string();
                            if !msg.is_empty() {
                                send = Some(msg);
                                self.input.clear();
                            }
                        }
                    });
            });

            if let Some(ref req) = self.approval_request.clone() {
                egui::TopBottomPanel::top("approval").show(cx, |ui| {
                    let bg = Color32::from_rgba_premultiplied(180, 60, 20, 220);
                    egui::Frame::new()
                        .fill(bg)
                        .corner_radius(6)
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(208, 96, 50)))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(Color32::from_rgb(255, 200, 80), "[!]");
                                ui.strong(" 需要授权");
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let approve = ui.button("批准").clicked();
                                    let deny = ui.button("拒绝").clicked();
                                    if approve {
                                        zhongshu_core::authority::approve(&req.tool, &req.program);
                                        self.approval_request = None;
                                        self.window.request_redraw();
                                    }
                                    if deny {
                                        self.approval_request = None;
                                        self.window.request_redraw();
                                    }
                                });
                            });
                            ui.label(format!("工具：{}", req.tool));
                            ui.label(format!("指令：{}", req.command));
                        });
                });
            }

            egui::CentralPanel::default().show(cx, |ui| {
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        render_chat(ui, &mut self.entries, self.streaming.as_mut());
                        ui.add_space(4.0);
                    });
            });
        });

        self.state.handle_platform_output(&self.window, out.platform_output);

        let pj = self.ctx.tessellate(out.shapes, out.pixels_per_point);

        for (id, d) in out.textures_delta.set {
            self.renderer.update_texture(&self.gpu.device, &self.gpu.queue, id, &d);
        }
        for id in out.textures_delta.free {
            self.renderer.free_texture(&id);
        }

        self.surface.resize(&self.gpu.device, sz.width, sz.height);

        let sd = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [sz.width, sz.height],
            pixels_per_point: self.window.scale_factor() as f32
        };

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => return send.filter(|s| !s.is_empty()),
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut enc = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor::default()
        );

        self.renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut enc,
            &pj,
            &sd
        );

        {
            let rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut rp_static = rp.forget_lifetime();
            self.renderer.render(&mut rp_static, &pj, &sd);
        }

        self.gpu.queue.submit([enc.finish()]);
        frame.present();

        send
    }

    pub fn flush_streaming(&mut self, max_entries: usize) {
        if let Some(s) = self.streaming.take() {
            if !s.content.is_empty() || !s.tool_calls.is_empty() {
                let mut entry = s.finish();
                entry.content = strip_final_answer(&entry.content).trim().to_string();
                self.entries.push(entry);
            }
        }

        if self.entries.len() > max_entries {
            let remove = self.entries.len() - max_entries;
            self.entries.drain(0..remove);
        }
    }

}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn strip_final_answer(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '<' {
            let rest = &text[i..];
            let lower = rest.to_lowercase();
            if lower.starts_with("<final_answer") || lower.starts_with("</final_answer")
                || lower.starts_with("<final") || lower.starts_with("</final")
            {
                for (_, c) in chars.by_ref() {
                    if c == '>' { break; }
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

pub fn render_chat(
    ui: &mut egui::Ui,
    entries: &mut [ChatEntry],
    streaming: Option<&mut StreamingState>,
) {
    for entry in entries.iter_mut() {
        render_entry(ui, entry);
    }

    if let Some(stream) = streaming {
        render_streaming(ui, stream);
    }
}

fn render_entry(ui: &mut egui::Ui, entry: &mut ChatEntry) {
    let (role_color, role_label) = role_header(entry.role);
    let card_bg = match entry.role {
        EntryRole::User => Color32::from_rgba_premultiplied(208, 74, 26, 55),
        EntryRole::Assistant => Color32::from_rgba_premultiplied(44, 44, 50, 200),
        EntryRole::System => Color32::from_rgba_premultiplied(50, 50, 55, 180),
    };

    let is_user = matches!(entry.role, EntryRole::User);
    let mut render_card = |ui: &mut egui::Ui| {
        egui::Frame::new()
            .fill(card_bg)
            .corner_radius(8)
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52)))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(role_color, role_label);
                });
                if !entry.tool_calls.is_empty() {
                    render_tool_timeline(ui, &entry.tool_calls);
                }
                render_markdown_cached(ui, &entry.content, &mut entry.cached_job);
            });
    };

    if is_user {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_card(ui);
            });
        });
    } else {
        render_card(ui);
    }
    ui.add_space(6.0);
}

fn render_streaming(ui: &mut egui::Ui, stream: &mut StreamingState) {
    let (role_color, role_label) = role_header(stream.role);

    egui::Frame::new()
        .fill(Color32::from_rgba_premultiplied(44, 44, 50, 200))
        .corner_radius(8)
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52)))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(role_color, role_label);
            });
            if !stream.tool_calls.is_empty() {
                render_tool_timeline(ui, &stream.tool_calls);
            }
            render_markdown_cached(ui, &stream.content, &mut stream.cached_job);
        });
    ui.add_space(6.0);
}

fn role_header(role: EntryRole) -> (Color32, &'static str) {
    match role {
        EntryRole::User => (Color32::from_rgb(208, 96, 50), "你"),
        EntryRole::Assistant => (Color32::from_rgb(190, 186, 180), "中书"),
        EntryRole::System => (Color32::from_rgb(140, 136, 130), "系统"),
    }
}

fn render_markdown_cached(
    ui: &mut egui::Ui,
    text: &str,
    cache: &mut Option<(u64, egui::text::LayoutJob)>
) {
    if text.is_empty() {
        return;
    }

    let text = strip_final_answer(text).trim().to_string();

    let hash = hash_str(&text);

    if let Some((h, job)) = cache {
        if *h == hash {
            ui.label(job.clone());
            return;
        }
    }

    let job = build_markdown_job(ui, &text);
    ui.label(job.clone());
    *cache = Some((hash, job));
}

fn build_markdown_job(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
    let parser = Parser::new_ext(text, Options::all());
    let mut job = egui::text::LayoutJob::default();

    let base = base_format(ui);
    let code = code_format(ui);

    let mut fmt_stack = vec![base.clone()];
    let mut code_depth = 0u32;
    let list_depth = 0u32;

    for event in parser {
        let top = fmt_stack.last().unwrap().clone();

        match event {
            Event::Start(tag) => match tag {
                Tag::CodeBlock(_) => {
                    code_depth += 1;
                    fmt_stack.push(code.clone());
                }
                Tag::Item => {
                    if list_depth > 0 {
                        job.append("\n", 0.0, top.clone());
                    }
                    job.append("• ", 0.0, top.clone());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::CodeBlock => {
                    code_depth = code_depth.saturating_sub(1);
                    fmt_stack.pop();
                }
                _ => {}
            },
            Event::Text(t) => {
                job.append(&t, 0.0, top);
            }
            Event::SoftBreak => {
                job.append(" ", 0.0, base.clone());
            }
            Event::HardBreak => {
                job.append("\n", 0.0, base.clone());
            }
            _ => {}
        }
    }

    job
}

fn base_format(ui: &egui::Ui) -> egui::text::TextFormat {
    egui::text::TextFormat {
        font_id: egui::FontId::proportional(14.0),
        color: ui.style().visuals.text_color(),
        background: Color32::TRANSPARENT,
        italics: false,
        underline: egui::Stroke::NONE,
        strikethrough: egui::Stroke::NONE,
        valign: egui::Align::TOP,
        extra_letter_spacing: 0.0,
        line_height: None,
    }
}

fn code_format(_ui: &egui::Ui) -> egui::text::TextFormat {
    egui::text::TextFormat {
        font_id: egui::FontId::monospace(14.0),
        color: Color32::from_rgb(200, 200, 200),
        background: Color32::from_rgb(30, 30, 36),
        italics: false,
        underline: egui::Stroke::NONE,
        strikethrough: egui::Stroke::NONE,
        valign: egui::Align::TOP,
        extra_letter_spacing: 0.0,
        line_height: None,
    }
}

fn render_tool_timeline(ui: &mut egui::Ui, tools: &[ToolCallEntry]) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_premultiplied(40, 40, 50, 180))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            for tool in tools {
                ui.horizontal(|ui| {
                    let icon = match tool.status {
                        ToolStatus::Running => "◌",
                        ToolStatus::Done { success: true, .. } => "✓",
                        ToolStatus::Done { success: false, .. } => "✗",
                    };
                    ui.label(format!("{icon} {}", tool.name));
                });
            }
        });
}

pub fn configure_fonts(ctx: &egui::Context, search_paths: &[String]) {
    let data = search_paths.iter().find_map(|p| {
        let result = std::fs::read(p);
        match &result {
            Ok(bytes) => tracing::info!("loaded font: {} ({} bytes)", p, bytes.len()),
            Err(e) => tracing::debug!("font unreadable: {} ({})", p, e),
        }
        result.ok()
    });

    match data {
        Some(data) => {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("cjk".into(), Arc::new(egui::FontData::from_owned(data)));
            for (_, family_fonts) in fonts.families.iter_mut() {
                family_fonts.insert(0, "cjk".into());
            }
            ctx.set_fonts(fonts);
            tracing::info!("CJK font applied to all families");
        }
        None => {
            tracing::warn!("no CJK font found; Chinese text will show as squares");
            tracing::warn!("searched paths: {:?}", search_paths);
        }
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    let accent = Color32::from_rgb(208, 74, 26);
    let bg = Color32::from_rgb(24, 24, 26);
    let surface = Color32::from_rgb(34, 34, 38);
    let text = Color32::from_rgb(228, 224, 218);

    v.window_corner_radius = egui::CornerRadius::same(10);
    v.panel_fill = bg;
    v.window_fill = bg;
    v.extreme_bg_color = Color32::from_rgb(18, 18, 20);
    v.faint_bg_color = surface;

    v.widgets.noninteractive.bg_fill = surface;
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52));
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, Color32::from_rgb(160, 156, 150));

    v.widgets.inactive.bg_fill = surface;
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.active.bg_fill = Color32::from_rgb(176, 58, 16);
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5, accent);

    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 48, 52);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, accent);

    v.selection.bg_fill = Color32::from_rgba_premultiplied(208, 74, 26, 80);
    v.selection.stroke = egui::Stroke::NONE;
    v.override_text_color = Some(text);

    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    ctx.set_style(style);
}
