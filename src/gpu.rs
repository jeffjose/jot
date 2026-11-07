//! Putting frames on the screen with the GPU, through wgpu: Vulkan where
//! there is a driver for it (NVIDIA, AMD and Intel alike), OpenGL where
//! not. What is drawn is kept in a texture on the card; each frame only
//! the parts that changed are sent to it, and it is drawn to the window
//! at the screen's pace. Without a GPU to use, jterm puts its frames up
//! with the CPU as before.

use winit::window::Window;

use crate::canvas::Canvas;
use crate::layout::Rect;

const SHADER: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle over the whole window.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(frame, vec2<i32>(at.xy), 0);
    return vec4<f32>(c.rgb, 1.0);
}
"#;

pub struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    /// What the card has, and a name for it, for --debug.
    pub name: String,
}

impl Gpu {
    /// A GPU to draw `window` with, or None when there is none to use.
    ///
    /// # Safety
    /// The window must outlive what this returns.
    pub unsafe fn new(window: &Window, display: winit::event_loop::OwnedDisplayHandle, width: u32, height: u32) -> Option<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)));
        let fail = |at: &str| {
            if crate::debug() {
                eprintln!("no GPU: {at}");
            }
        };
        let Ok(target) = wgpu::SurfaceTargetUnsafe::from_window(window) else { fail("no window handle"); return None };
        let surface = match instance.create_surface_unsafe(target) {
            Ok(s) => s,
            Err(e) => {
                fail(&format!("surface: {e}"));
                return None;
            }
        };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| fail(&format!("adapter: {e}")))
        .ok()?;
        let info = adapter.get_info();
        // A GPU done in software is slower than jterm's own drawing.
        if info.device_type == wgpu::DeviceType::Cpu && std::env::var_os("JTERM_GPU_ANY").is_none() {
            fail(&format!("{} is the CPU", info.name));
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| fail(&format!("device: {e}"))).ok()?;
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1))?;
        // The frame's colors go to the screen as they are, with no sRGB
        // conversion on the way.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("present"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("present"), bind_group_layouts: &[Some(&layout)], ..Default::default() });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: config.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let (texture, bind) = Self::frame(&device, &layout, config.width, config.height);
        let name = format!("{} ({:?})", info.name, info.backend);
        Some(Gpu { surface, device, queue, config, pipeline, layout, texture, bind, name })
    }

    fn frame(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, width: u32, height: u32) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The canvas's 0x00RRGGBB, little-endian, is B, G, R, then 0.
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("frame"), layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }] });
        (texture, bind)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        (self.texture, self.bind) = Self::frame(&self.device, &self.layout, width, height);
    }

    /// The parts of `canvas` that changed sent to the card, and the whole
    /// of it drawn to the window. False when the window could not be drawn
    /// to this time.
    pub fn present(&mut self, canvas: &Canvas, damage: &[Rect]) -> bool {
        if (canvas.width as u32, canvas.height as u32) != (self.config.width, self.config.height) {
            self.resize(canvas.width as u32, canvas.height as u32);
        }
        let bytes = unsafe { std::slice::from_raw_parts(canvas.pixels.as_ptr() as *const u8, canvas.pixels.len() * 4) };
        let row = canvas.width as u32 * 4;
        for d in damage {
            let x0 = d.x.clamp(0, canvas.width) as u32;
            let x1 = (d.x + d.w).clamp(0, canvas.width) as u32;
            let y0 = d.y.clamp(0, canvas.height) as u32;
            let y1 = (d.y + d.h).clamp(0, canvas.height) as u32;
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
                bytes,
                wgpu::TexelCopyBufferLayout { offset: (y0 * row + x0 * 4) as u64, bytes_per_row: Some(row), rows_per_image: None },
                wgpu::Extent3d { width: x1 - x0, height: y1 - y0, depth_or_array_layers: 1 },
            );
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        true
    }
}
