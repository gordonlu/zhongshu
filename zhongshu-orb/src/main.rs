mod app;
mod render;

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use render::{draw_orb, OrbState};
use app::{AgentRuntime, UiBridge, UiEvent, SessionState};
use zhongshu_core::agent::llm::{Message, OpenAiProvider};
use zhongshu_core::agent::loop_::{AgentBudget, AgentLoop};
use zhongshu_core::tool::default_registry;
use zhongshu_core::integration::{ContextConfig, ContextEngine};

const ORB_SIZE: u32 = 64;
const SYSTEM_PROMPT: &str = "\
你是「中书」(Zhongshu)，桌面 AI 助手。回复简洁，末尾加 <final_answer>。中文回复。";

struct EguiOverlay {
    window: Arc<Window>,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    input: String,
    messages: Vec<String>,
}

struct ZhongshuApp {
    orb_window: Option<Arc<Window>>,
    orb_surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    orb_state: OrbState,
    orb_positioned: bool,
    overlays: HashMap<WindowId, EguiOverlay>,
    start_time: Instant,
    bridge: UiBridge,
    runtime: AgentRuntime,
    session: SessionState,
    egui_ctx: Option<egui::Context>,
}

impl ZhongshuApp {
    fn new(bridge: UiBridge, runtime: AgentRuntime, session: SessionState) -> Self {
        ZhongshuApp {
            orb_window: None, orb_surface: None, orb_state: OrbState::Idle,
            orb_positioned: false, overlays: HashMap::new(),
            start_time: Instant::now(), bridge, runtime, session, egui_ctx: None,
        }
    }

    fn position_orb(&mut self) {
        if self.orb_positioned { return; }
        if let Some(w) = &self.orb_window {
            if let Some(m) = w.current_monitor() {
                let p = m.position(); let s = m.size();
                w.set_outer_position(PhysicalPosition::new(p.x + s.width as i32 - 80, p.y + s.height as i32 - 100));
                self.orb_positioned = true;
            }
        }
    }

    fn drain(&mut self) {
        while let Ok(ev) = self.bridge.rx.try_recv() {
            match ev {
                UiEvent::SetState(s) => { self.orb_state = s; if let Some(w) = &self.orb_window { w.request_redraw(); } }
                UiEvent::TextDelta(t) => {
                    for ov in self.overlays.values_mut() { ov.messages.push(t.clone()); ov.window.request_redraw(); }
                }
                UiEvent::ToolStart => {
                    self.orb_state = OrbState::Executing { pulse: 0.0 };
                    if let Some(w) = &self.orb_window { w.request_redraw(); }
                }
                UiEvent::ToolDone(ok) => {
                    let icon = if ok { "✓" } else { "✗" };
                    for ov in self.overlays.values_mut() { ov.messages.push(icon.to_string()); ov.window.request_redraw(); }
                }
            }
        }
    }

    fn run_agent(&mut self, input: String) {
        let tx = self.bridge.tx.clone();
        let p = self.runtime.provider.clone();
        let t = self.runtime.tools.clone();
        let m = self.runtime.model.clone();
        let e = self.session.engine.clone();
        let c = self.session.conv_id.clone();
        tx.send(UiEvent::SetState(OrbState::Thinking { progress: 0.0 })).ok();
        tx.send(UiEvent::TextDelta(format!("你: {}", input))).ok();
        tokio::spawn(async move {
            let eng = e.lock().await.clone();
            let cid = *c.lock().await;
            let mctx = eng.as_ref().map_or(String::new(), |x| x.build_context(cid, 5000, &input).unwrap_or_default());
            let mut msgs = vec![Message::system(SYSTEM_PROMPT)];
            if !mctx.is_empty() { msgs.push(Message::user(format!("<context>\n{mctx}\n</context>"))); }
            msgs.push(Message::user(input.clone()));
            let agent = AgentLoop::new(p, t, m).with_budget(AgentBudget::default()).with_messages(msgs);
            let r = agent.run_streaming("",
                { let tx=tx.clone(); move |x|{tx.send(UiEvent::TextDelta(x.to_string())).ok();} },
                { let tx=tx.clone(); move |_|{tx.send(UiEvent::ToolStart).ok();} },
                { let tx=tx.clone(); move |_,ok|{tx.send(UiEvent::ToolDone(ok)).ok();} },
            ).await;
            match r {
                Ok(rr) => {
                    if let Some(ref en) = eng { let _=en.append_turn(cid,&format!("[u]:{input}"),&format!("[a]:{}",rr.messages.last().map(|x|x.content.as_str()).unwrap_or(""))); if en.check_compression(cid).should_compress{let _=en.trigger_compaction(cid).await;} }
                    tx.send(UiEvent::SetState(OrbState::Done{success:true})).ok();
                }
                Err(e) => { tx.send(UiEvent::TextDelta(format!("错误:{e}"))).ok(); tx.send(UiEvent::SetState(OrbState::Done{success:false})).ok(); }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            tx.send(UiEvent::SetState(OrbState::Idle)).ok();
        });
    }

    fn init_engine(&mut self) {
        let ak = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
        if ak.is_empty() { return; }
        let m = self.runtime.model.clone();
        let ea = self.session.engine.clone(); let ca = self.session.conv_id.clone();
        tokio::spawn(async move {
            if let Ok(e) = ContextEngine::new(ContextConfig{api_key:ak,..ContextConfig::default()}).await {
                let cid = e.find_or_create_conv(SYSTEM_PROMPT,&m).unwrap_or(1);
                *ea.lock().await = Some(Arc::new(e)); *ca.lock().await = cid;
            }
        });
    }

    fn create_orb(&mut self, el: &ActiveEventLoop) {
        let attrs = WindowAttributes::default().with_title("zhongshu")
            .with_inner_size(LogicalSize::new(ORB_SIZE,ORB_SIZE)).with_resizable(false)
            .with_decorations(false).with_window_level(WindowLevel::AlwaysOnTop).with_active(false);
        let w = Arc::new(el.create_window(attrs).unwrap());
        let ctx = softbuffer::Context::new(w.clone()).unwrap();
        self.orb_surface = Some(softbuffer::Surface::new(&ctx,w.clone()).unwrap());
        self.orb_window = Some(w.clone()); w.request_redraw();
    }

    fn create_overlay(&mut self, el: &ActiveEventLoop) {
        let attrs = WindowAttributes::default().with_title("zhongshu 对话")
            .with_inner_size(LogicalSize::new(480.0,580.0)).with_resizable(true)
            .with_decorations(true).with_window_level(WindowLevel::Normal);
        let w = Arc::new(el.create_window(attrs).unwrap());
        let id = w.id();
        let vp_id = egui::viewport::ViewportId::from_hash_of(id);
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(w.clone()).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions{
            power_preference:wgpu::PowerPreference::LowPower,compatible_surface:Some(&surface),force_fallback_adapter:false,
        })).unwrap();
        let (device,queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(),None)).unwrap();
        let sz = w.inner_size();
        let config = wgpu::SurfaceConfiguration{
            usage:wgpu::TextureUsages::RENDER_ATTACHMENT,
            format:surface.get_capabilities(&adapter).formats[0],
            width:sz.width,height:sz.height,
            present_mode:wgpu::PresentMode::AutoVsync,
            alpha_mode:wgpu::CompositeAlphaMode::Auto,view_formats:vec![],
            desired_maximum_frame_latency:1,
        };
        surface.configure(&device,&config);
        let state = egui_winit::State::new(self.egui_ctx.clone().unwrap_or_default(),vp_id,&w,None,None,None);
        let renderer = egui_wgpu::Renderer::new(&device,config.format,None,1,false);
        self.overlays.insert(id,EguiOverlay{window:w.clone(),surface,device,queue,config,state,renderer,input:String::new(),messages:Vec::new()});
        w.focus_window(); w.request_redraw();
    }

    fn render_orb(&mut self) {
        self.position_orb();
        let (w,s) = match(&self.orb_window,&mut self.orb_surface){(Some(w),Some(s))=>(w,s),_=>return};
        let sz=w.inner_size();let(ww,hh)=(sz.width,sz.height);
        if ww==0||hh==0{return;}
        s.resize(NonZeroU32::new(ww).unwrap(),NonZeroU32::new(hh).unwrap()).ok();
        let mut buf=match s.buffer_mut(){Ok(b)=>b,Err(_)=>return};
        draw_orb(&mut buf,ww,hh,self.orb_state,self.start_time.elapsed().as_secs_f64());
        buf.present().unwrap();
        if !matches!(self.orb_state,OrbState::Idle){w.request_redraw();}
    }

    fn render_overlay(&mut self, id: WindowId) {
        let ov = match self.overlays.get_mut(&id) { Some(o) => o, None => return };
        let sz = ov.window.inner_size(); if sz.width==0||sz.height==0{return;}
        let raw = ov.state.take_egui_input(&ov.window);
        let ctx = self.egui_ctx.clone().unwrap_or_default();
        let mut send: Option<String> = None;
        let out = ctx.run(raw, |cx| {
            egui::CentralPanel::default().show(cx, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for m in &ov.messages { ui.label(m.as_str()); }
                });
            });
            egui::TopBottomPanel::bottom("input").show(cx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut ov.input).hint_text("输入，Enter 发送..."));
                if r.lost_focus()&&ui.input(|i|i.key_pressed(egui::Key::Enter)){send=Some(std::mem::take(&mut ov.input));}
                r.request_focus();
            });
        });
        self.egui_ctx = Some(ctx.clone());
        ov.state.handle_platform_output(&ov.window, out.platform_output);
        let inp = send.take().filter(|s|!s.trim().is_empty());

        let pj = ctx.tessellate(out.shapes, out.pixels_per_point);
        for (id, d) in out.textures_delta.set { ov.renderer.update_texture(&ov.device,&ov.queue,id,&d); }
        for id in out.textures_delta.free { ov.renderer.free_texture(&id); }

        if ov.config.width!=sz.width||ov.config.height!=sz.height {
            ov.config.width=sz.width; ov.config.height=sz.height;
            ov.surface.configure(&ov.device,&ov.config);
        }
        let sd = egui_wgpu::ScreenDescriptor{size_in_pixels:[ov.config.width,ov.config.height],pixels_per_point:ov.window.scale_factor() as f32};
        let frame = ov.surface.get_current_texture().unwrap();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = ov.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        ov.renderer.update_buffers(&ov.device,&ov.queue,&mut enc,&pj,&sd);
        {
            let rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor{
                label:Some("egui"),color_attachments:&[Some(wgpu::RenderPassColorAttachment{
                    view:&view,resolve_target:None,ops:wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store},
                })],depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,
            });
            let mut rp_static = rp.forget_lifetime();
            ov.renderer.render(&mut rp_static, &pj, &sd);
        }
        ov.queue.submit([enc.finish()]); frame.present();
        let _ = ov;

        if let Some(input) = inp { self.run_agent(input); }
    }
}

impl ApplicationHandler for ZhongshuApp {
    fn resumed(&mut self, el: &ActiveEventLoop) { if self.orb_window.is_none() { self.init_engine(); self.create_orb(el); } }
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.drain();
        let is_ol = self.overlays.contains_key(&id);
        if is_ol { if let Some(ov) = self.overlays.get_mut(&id) { let _ = ov.state.on_window_event(&ov.window, &event); } }
        match event {
            WindowEvent::CloseRequested => {
                if self.orb_window.as_ref().map(|w|w.id())==Some(id){el.exit();}else{self.overlays.remove(&id);}
            }
            WindowEvent::RedrawRequested => {
                if self.orb_window.as_ref().map(|w|w.id())==Some(id){self.render_orb();}else{self.render_overlay(id);}
            }
            WindowEvent::MouseInput{state:ElementState::Pressed,button:MouseButton::Left,..} => {
                if self.orb_window.as_ref().map(|w|w.id())==Some(id)&&self.overlays.is_empty() {
                    if self.session.engine.try_lock().map(|g|g.is_none()).unwrap_or(true){self.init_engine();}
                    self.create_overlay(el);
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain();
        el.set_control_flow(if matches!(self.orb_state,OrbState::Idle){ControlFlow::Wait}else{ControlFlow::Poll});
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"info".into())).init();
    let ak = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY");
    let model = std::env::var("ZHONGSHU_MODEL").unwrap_or_else(|_|"deepseek-v4-flash".into());
    let p = OpenAiProvider::new(&ak,&model);
    let t = default_registry().register(zhongshu_core::tool::search::WebSearchTool).register(zhongshu_core::tool::browser::BrowserTool).register(zhongshu_core::tool::screenshot::ScreenshotTool).register(zhongshu_core::tool::automation::AutomationTool);
    let b = UiBridge::new();
    let rt = AgentRuntime{provider:p,tools:t,model};
    let s = SessionState::new();
    let r = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let _g = r.enter();
    EventLoop::new().unwrap().run_app(&mut ZhongshuApp::new(b,rt,s)).unwrap();
}
