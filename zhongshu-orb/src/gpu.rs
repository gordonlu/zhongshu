use std::sync::Arc;
use anyhow::Context;
use winit::window::Window;

// ── Shared GPU root (singleton) ─────────────────────────────────────

/// Holds the global GPU instance, adapter, device, and queue.
/// Created once at startup before the event loop.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    instance: wgpu::Instance,
}

impl GpuContext {
    /// Blocking initialisation — must be called before the winit event
    /// loop starts.  `pollster::block_on` is acceptable here because no
    /// other async work is in-flight yet.
    pub fn new() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }
        )).context("no suitable GPU adapter found")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(), None,
        )).context("failed to create GPU device")?;
        Ok(GpuContext { device, queue, adapter, instance })
    }

    fn create_surface(&self, window: &Arc<Window>) -> wgpu::Surface<'static> {
        self.instance
            .create_surface(window.clone())
            .expect("failed to create wgpu surface")
    }

    fn best_format(caps: &SurfaceCaps) -> wgpu::TextureFormat {
        let pref = [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Bgra8Unorm];
        for p in &pref {
            if caps.formats.contains(p) { return *p; }
        }
        caps.formats[0]
    }

    fn surface_caps(&self, surface: &wgpu::Surface) -> SurfaceCaps {
        SurfaceCaps {
            formats: surface.get_capabilities(&self.adapter).formats,
            present_modes: surface.get_capabilities(&self.adapter).present_modes,
        }
    }
}

// ── Per-window surface ──────────────────────────────────────────────

struct SurfaceCaps {
    formats: Vec<wgpu::TextureFormat>,
    #[allow(dead_code)]
    present_modes: Vec<wgpu::PresentMode>,
}

/// Owns a `wgpu::Surface` and its configuration, tied to one window.
/// The surface must not outlive its window — `WindowSurface` ensures
/// this by being stored alongside the `Arc<Window>` in `EguiOverlay`.
pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
}

impl WindowSurface {
    pub fn new(gpu: &GpuContext, window: &Arc<Window>, width: u32, height: u32) -> Self {
        let surface = gpu.create_surface(window);
        let caps = gpu.surface_caps(&surface);
        let format = GpuContext::best_format(&caps);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&gpu.device, &config);
        WindowSurface { surface, config, format }
    }

    /// Reconfigure the swapchain after a window resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 { return; }
        if self.config.width == width && self.config.height == height { return; }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn get_current_texture(&self) -> Result<wgpu::SurfaceTexture, wgpu::SurfaceError> {
        self.surface.get_current_texture()
    }
}
