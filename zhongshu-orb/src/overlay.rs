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
    show_settings: bool,
    enlarged: bool,
    normal_size: (f32, f32),
    settings_pending_save: bool,
    settings_api_key: String,
    settings_api_key_original: String,
    settings_api_key_masked: bool,
    settings_api_base: String,
    settings_model: String,
    settings_bg_enabled: bool,
    settings_bg_interval: String,
    settings_bg_prompt: String,
    settings_personality: String,
    settings_auto_evolve: bool,
    settings_proxy_port: String,
    personality_selected: bool,
    personality: String,
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

        let cfg = crate::config::load();
        let api_key = cfg.llm.api_key_env.clone();
        let key_masked = !api_key.is_empty();

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
            show_settings: false,
            enlarged: false,
            normal_size: (width, height),
            settings_pending_save: false,
            settings_api_key: if key_masked { "********".into() } else { api_key },
            settings_api_key_original: cfg.llm.api_key_env.clone(),
            settings_api_key_masked: key_masked,
            settings_api_base: cfg.llm.api_base.clone(),
            settings_model: cfg.llm.model.clone(),
            settings_bg_enabled: cfg.agent.background.enabled,
            settings_bg_interval: cfg.agent.background.interval_secs.to_string(),
            settings_bg_prompt: cfg.agent.background.prompt.clone(),
            settings_personality: cfg.agent.personality.clone(),
            settings_auto_evolve: cfg.agent.auto_evolve,
            settings_proxy_port: cfg.deeplossless.proxy_port.to_string(),
            personality_selected: cfg.agent.personality_selected,
            personality: cfg.agent.personality.clone(),
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
            // ── Toolbar ───────────────────────────────────────────────
            egui::TopBottomPanel::top("toolbar").show(cx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.heading("中书");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🗑 清空").clicked() {
                            self.entries.clear();
                            self.streaming = None;
                        }
                        ui.add_space(8.0);
                        let enlarge_label = if self.enlarged { "⊟ 还原" } else { "⊞ 放大" };
                        if ui.button(enlarge_label).clicked() {
                            self.enlarged = !self.enlarged;
                            let (w, h) = self.normal_size;
                            if self.enlarged {
                                let _ = self.window.request_inner_size(
                                    winit::dpi::LogicalSize::new(w * 2.0, (h * 1.5).min(1600.0))
                                );
                            } else {
                                let _ = self.window.request_inner_size(
                                    winit::dpi::LogicalSize::new(w, h)
                                );
                            }
                        }
                        ui.add_space(8.0);
                        let label = if self.show_settings { "✕ 关闭" } else { "⚙ 设置" };
                        if ui.button(label).clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        ui.add_space(8.0);
                    });
                });
            });

            // ── Input area ────────────────────────────────────────────
            if !self.show_settings {
                egui::TopBottomPanel::bottom("input").show(cx, |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(34, 34, 38))
                            .corner_radius(8)
                            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52)))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.input)
                                        .hint_text("给 中书 发消息...")
                                        .desired_width(f32::INFINITY)
                                        .id("chat_input".into())
                                );

                                if cx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                                    let msg = self.input.trim().to_string();
                                    if !msg.is_empty() {
                                        send = Some(msg);
                                        self.input.clear();
                                        cx.memory_mut(|mem| mem.request_focus("chat_input".into()));
                                        self.window.set_ime_allowed(false);
                                        self.window.set_ime_allowed(true);
                                    }
                                }
                            });
                        // Stop button
                        if self.streaming.is_some() {
                            if ui.button("⏹ 停止").clicked() {
                                self.request_stop = true;
                            }
                        }
                    });
                });
            }

            // ── Approval ──────────────────────────────────────────────
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
                            if !req.source.is_empty() {
                                ui.label(format!("来源：{}", req.source));
                            }
                            ui.label(format!("工具：{}", req.tool));
                            ui.label(format!("指令：{}", req.command));
                        });
                });
            }

            // ── Central area ──────────────────────────────────────────
            egui::CentralPanel::default().show(cx, |ui| {
                if self.show_settings {
                    // ── Settings panel ────────────────────────────────
                    egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(8.0);
                    let accent = Color32::from_rgb(208, 74, 26);
                    let card_bg = Color32::from_rgb(34, 34, 38);

                    // Model settings card
                    egui::Frame::new()
                        .fill(card_bg)
                        .corner_radius(8)
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52)))
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| { ui.colored_label(accent, "模型设置"); ui.add_space(8.0); ui.separator(); ui.add_space(8.0);
                            egui::Grid::new("settings_model").num_columns(2).spacing([12.0, 8.0]).min_col_width(80.0).show(ui, |ui| {
                                ui.label("API Key"); let resp = ui.add_sized([ui.available_width(), 28.0],
                                    egui::TextEdit::singleline(&mut self.settings_api_key).password(self.settings_api_key_masked).hint_text("sk-..."));
                                if resp.has_focus() && self.settings_api_key_masked { self.settings_api_key = std::mem::take(&mut self.settings_api_key_original); self.settings_api_key_masked = false; } ui.end_row();
                                ui.label("API 地址"); ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut self.settings_api_base).hint_text("https://api.openai.com/v1")); ui.end_row();
                                ui.label("模型"); ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut self.settings_model).hint_text("gpt-4o")); ui.end_row();
                            });
                        });
                    ui.add_space(12.0);

                    // Personality card
                    egui::Frame::new()
                        .fill(card_bg).corner_radius(8).stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52))).inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| { ui.colored_label(accent, "个性风格"); ui.add_space(4.0); ui.colored_label(Color32::from_rgb(128, 124, 118), "频繁更换会降低 AI 缓存命中率"); ui.add_space(8.0); ui.separator(); ui.add_space(8.0);
                            let keys = ["古典", "极客", "温度"]; let names = ["古典 · 精炼幕僚", "极客 · 技术直白", "温度 · 友好同事"];
                            ui.horizontal(|ui| { for (i, key) in keys.iter().enumerate() {
                                let selected = self.settings_personality == *key;
                                if ui.selectable_label(selected, names[i]).clicked() { self.settings_personality = key.to_string(); self.settings_pending_save = true; }
                            }});
                        });
                    ui.add_space(12.0);

                    // Auto-evolve card
                    egui::Frame::new()
                        .fill(card_bg).corner_radius(8).stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52))).inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| { ui.colored_label(accent, "自我进化"); ui.add_space(6.0);
                            ui.horizontal(|ui| { ui.checkbox(&mut self.settings_auto_evolve, "启用自动进化"); });
                            if self.settings_auto_evolve { ui.add_space(8.0); ui.separator(); ui.add_space(6.0);
                                ui.colored_label(Color32::from_rgb(160, 156, 150),
                                    "观察你与中书交互的模式，研判是否值得制作或升级装备。\n\n可见范围（仅限与中书交互时）：\n• 工具调用类型和成功率\n• 你的提问内容（存储前自动脱敏）\n\n不可见（除非你显式授权）：\n• 浏览器历史\n• 系统进程\n• 键盘记录\n• 屏幕内容\n• 其他应用行为");
                            }
                        });
                    ui.add_space(12.0);

                    // Proxy port card
                    egui::Frame::new()
                        .fill(card_bg).corner_radius(8).stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52))).inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| { ui.colored_label(accent, "代理端口"); ui.add_space(4.0); ui.colored_label(Color32::from_rgb(128, 124, 118), "deeplossless 本地代理端口，冲突时自动 +1"); ui.add_space(8.0); ui.separator(); ui.add_space(8.0);
                            ui.horizontal(|ui| { ui.label("端口"); ui.add_sized([100.0, 28.0], egui::TextEdit::singleline(&mut self.settings_proxy_port).desired_width(80.0)); });
                        });
                    ui.add_space(12.0);

                    // Background card
                    egui::Frame::new()
                        .fill(card_bg).corner_radius(8).stroke(egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 52))).inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| { ui.colored_label(accent, "后台检查"); ui.add_space(8.0); ui.separator(); ui.add_space(8.0);
                            ui.horizontal(|ui| { ui.checkbox(&mut self.settings_bg_enabled, "启用定时系统检查"); });
                            if self.settings_bg_enabled { ui.add_space(8.0);
                                egui::Grid::new("settings_bg").num_columns(2).spacing([12.0, 8.0]).min_col_width(80.0).show(ui, |ui| {
                                    ui.label("间隔（秒）"); ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut self.settings_bg_interval)); ui.end_row();
                                    ui.label("提示词"); ui.add_sized([ui.available_width(), 60.0], egui::TextEdit::multiline(&mut self.settings_bg_prompt)); ui.end_row();
                                });
                            }
                        });
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if ui.button("保存").clicked() { self.settings_pending_save = true; }
                        ui.add_space(8.0);
                        if ui.button("取消").clicked() { self.show_settings = false; }
                    });
                    });
                } else if !self.personality_selected {
                    render_personality_picker(ui, &mut self.pending_personality);
                    if self.pending_personality.is_some() { self.personality_selected = true; }
                } else {
                    if self.entries.is_empty() {
                        ui.add_space(80.0);
                        ui.vertical_centered(|ui| {
                            ui.colored_label(Color32::from_rgb(128, 124, 118), "开始对话");
                            ui.add_space(4.0);
                            ui.colored_label(Color32::from_rgb(100, 96, 90), "在下方输入消息或发送系统指令");
                        });
                    } else {
                        egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                            ui.add_space(4.0);
                            render_chat(ui, &mut self.entries, self.streaming.as_mut());
                            ui.add_space(4.0);
                        });
                    }
                }
            });
        });

        // ── Settings save ────────────────────────────────────────────
        if self.settings_pending_save {
            self.settings_pending_save = false;
            let mut cfg = crate::config::load();
            cfg.llm.api_key_env = if self.settings_api_key_masked {
                self.settings_api_key_original.clone()
            } else { self.settings_api_key.clone() };
            cfg.llm.api_base = self.settings_api_base.clone();
            cfg.llm.model = self.settings_model.clone();
            cfg.agent.background.enabled = self.settings_bg_enabled;
            cfg.agent.background.interval_secs = self.settings_bg_interval.parse().unwrap_or(600);
            cfg.agent.background.prompt = self.settings_bg_prompt.clone();
            if self.settings_personality != cfg.agent.personality {
                cfg.agent.personality = self.settings_personality.clone();
                self.pending_personality = Some(self.settings_personality.clone());
                self.personality = self.settings_personality.clone();
            }
            cfg.agent.personality_selected = true;
            cfg.agent.auto_evolve = self.settings_auto_evolve;
            cfg.deeplossless.proxy_port = self.settings_proxy_port.parse().unwrap_or(8081);
            crate::config::save(&cfg);
            self.show_settings = false;
        }

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

fn render_personality_picker(ui: &mut egui::Ui, pending: &mut Option<String>) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.heading("选择个性风格");
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(128, 124, 118), "选一个你喜欢的，之后可以在设置里更改");
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(90, 86, 82), "提示：频繁更换个性风格会降低 AI 缓存命中率");
    });
    ui.add_space(16.0);
    let options = [
        ("古典", "用语干练，有古风但不酸\n像唐代中书省的干练幕僚\n不卑不亢，不赘言"),
        ("极客", "说话直接，不寒暄\n用技术人的方式表达\n可以带一点冷幽默"),
        ("温度", "像好的 coworker\n友好但不啰嗦\n该严肃时严肃，该轻松时轻松"),
    ];
    for (key, desc) in options {
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(45, 45, 50))
            .corner_radius(10)
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(80, 80, 85)))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_min_width(280.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| { ui.strong(key); ui.add_space(4.0); ui.colored_label(Color32::from_rgb(160, 156, 150), desc); });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.button("使用").clicked() { *pending = Some(key.to_string()); } });
                });
            });
        ui.add_space(6.0);
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
    let (role_color, role_label, accent_color) = match entry.role {
        EntryRole::User => (Color32::from_rgb(208, 96, 50), "你", Color32::from_rgb(208, 74, 26)),
        EntryRole::Assistant => (Color32::from_rgb(190, 186, 180), "中书", Color32::from_rgb(70, 130, 200)),
        EntryRole::System => (Color32::from_rgb(140, 136, 130), "系统", Color32::from_rgb(100, 100, 100)),
    };
    let card_bg = match entry.role {
        EntryRole::User => Color32::from_rgba_premultiplied(208, 74, 26, 40),
        EntryRole::Assistant => Color32::from_rgba_premultiplied(44, 44, 50, 200),
        EntryRole::System => Color32::from_rgba_premultiplied(50, 50, 55, 180),
    };

    let is_user = matches!(entry.role, EntryRole::User);
    let mut render_card = |ui: &mut egui::Ui| {
        // Left accent border
        egui::Frame::new()
            .fill(accent_color)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(3, 16))
.show(ui, |_| {});
            ui.add_space(6.0);
        egui::Frame::new()
            .fill(card_bg)
            .corner_radius(8)
            .stroke(egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 12)))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(role_color, egui::RichText::new(role_label).strong());
                    ui.add_space(6.0);
                    ui.colored_label(Color32::from_rgb(80, 76, 70), "·");
                    ui.colored_label(Color32::from_rgb(80, 76, 70), "刚刚");
                });
                if !entry.tool_calls.is_empty() {
                    ui.add_space(4.0);
                    render_tool_timeline(ui, &entry.tool_calls);
                }
                ui.add_space(4.0);
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
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            render_card(ui);
        });
    }
    ui.add_space(8.0);
}

fn render_streaming(ui: &mut egui::Ui, stream: &mut StreamingState) {
    let (role_color, role_label, accent_color) = match stream.role {
        EntryRole::User => (Color32::from_rgb(208, 96, 50), "你", Color32::from_rgb(208, 74, 26)),
        EntryRole::Assistant => (Color32::from_rgb(190, 186, 180), "中书", Color32::from_rgb(70, 130, 200)),
        EntryRole::System => (Color32::from_rgb(140, 136, 130), "系统", Color32::from_rgb(100, 100, 100)),
    };

    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            // Left accent border
            egui::Frame::new()
                .fill(accent_color)
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(3, 16))
                .show(ui, |_| {});
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(Color32::from_rgba_premultiplied(44, 44, 50, 200))
                .corner_radius(8)
                .stroke(egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 12)))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(role_color, egui::RichText::new(role_label).strong());
                        ui.add_space(6.0);
                        ui.colored_label(Color32::from_rgb(80, 76, 70), "·");
                        ui.colored_label(Color32::from_rgb(80, 76, 70), "思考中...");
                    });
                    if !stream.tool_calls.is_empty() {
                        ui.add_space(4.0);
                        render_tool_timeline(ui, &stream.tool_calls);
                    }
                    if stream.content.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.colored_label(Color32::from_rgb(128, 124, 118), "等待回复...");
                        });
                    } else {
                        ui.add_space(4.0);
                        render_markdown_cached(ui, &stream.content, &mut stream.cached_job);
                    }
                });
        });
    });
    ui.add_space(8.0);
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

    for event in parser {
        let top = fmt_stack.last().unwrap().clone();

        match event {
            Event::Start(tag) => match tag {
                Tag::CodeBlock(_) => {
                    code_depth += 1;
                    fmt_stack.push(code.clone());
                }
                Tag::Item => {
                    job.append("\n  • ", 0.0, top);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::CodeBlock | TagEnd::Heading(..) | TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link => {
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
    let (data, index) = match find_noto_mono_sc() {
        Some((d, idx)) => (Some(d), idx),
        None => {
            // Direct fallback: known path + TTC index 7 for Noto Sans Mono CJK SC
            let direct = std::fs::read("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc").ok();
            if let Some(data) = direct {
                tracing::info!("loaded NotoSansCJK-Regular.ttc directly (index 7)");
                (Some(data), 7u32)
            } else {
                let d = search_paths.iter().find_map(|p| {
                    let result = std::fs::read(p);
                    match &result {
                        Ok(bytes) => tracing::info!("loaded font: {} ({} bytes)", p, bytes.len()),
                        Err(e) => tracing::debug!("font unreadable: {} ({})", p, e),
                    }
                    result.ok()
                });
                (d, 0u32)
            }
        }
    };

    match data {
        Some(data) => {
            let mut fonts = egui::FontDefinitions::default();
            let mut font_data = egui::FontData::from_owned(data);
            font_data.index = index;
            fonts.font_data.insert("cjk".into(), Arc::new(font_data));
            for (_, family_fonts) in fonts.families.iter_mut() {
                family_fonts.insert(0, "cjk".into());
            }
            ctx.set_fonts(fonts);
            tracing::info!("CJK font applied (index {})", index);
        }
        None => {
            tracing::warn!("no CJK font found; Chinese text will show as squares");
            tracing::warn!("searched paths: {:?}", search_paths);
        }
    }
}

/// Try `fc-match` to find "Noto Sans Mono CJK SC" with correct TTC index.
#[cfg(target_os = "linux")]
fn find_noto_mono_sc() -> Option<(Vec<u8>, u32)> {
    use std::process::Command;
    let output = Command::new("fc-match")
        .args(["-f", "%{file[0]}:%{index}", "Noto Sans Mono CJK SC"])
        .output().ok()?;
    let s = String::from_utf8(output.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() || s == ":" { return None; }
    let parts: Vec<&str> = s.split(':').collect();
    let path = parts[0];
    let index: u32 = parts.get(1).and_then(|i| i.parse().ok()).unwrap_or(0);
    let data = std::fs::read(path).ok()?;
    tracing::info!("fontconfig: {} (index {})", path, index);
    Some((data, index))
}

#[cfg(not(target_os = "linux"))]
fn find_noto_mono_sc() -> Option<(Vec<u8>, u32)> {
    None
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    let accent = Color32::from_rgb(208, 74, 26);
    let accent_dim = Color32::from_rgb(160, 56, 20);
    let bg = Color32::from_rgb(20, 20, 22);
    let surface = Color32::from_rgb(30, 30, 33);
    let surface_raised = Color32::from_rgb(38, 38, 42);
    let border = Color32::from_rgb(48, 48, 52);
    let text = Color32::from_rgb(228, 224, 218);
    let text_muted = Color32::from_rgb(128, 124, 118);

    v.window_corner_radius = egui::CornerRadius::same(12);
    v.panel_fill = bg;
    v.window_fill = bg;
    v.extreme_bg_color = Color32::from_rgb(14, 14, 16);
    v.faint_bg_color = surface;

    v.widgets.noninteractive.bg_fill = surface;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, border);
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text_muted);

    v.widgets.inactive.bg_fill = surface_raised;
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text_muted);
    v.widgets.inactive.expansion = 0.0;

    v.widgets.active.bg_fill = accent_dim;
    v.widgets.active.corner_radius = egui::CornerRadius::same(8);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5, accent);
    v.widgets.active.expansion = 0.0;

    v.widgets.hovered.bg_fill = Color32::from_rgb(50, 50, 54);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, accent);
    v.widgets.hovered.expansion = 0.0;

    v.selection.bg_fill = Color32::from_rgba_premultiplied(208, 74, 26, 80);
    v.selection.stroke = egui::Stroke::NONE;

    v.button_frame = true;
    v.collapsing_header_frame = false;

    v.override_text_color = Some(text);

    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, border);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
    ctx.set_style(style);
}
