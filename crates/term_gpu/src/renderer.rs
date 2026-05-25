//! `GpuRenderer` owns the wgpu surface, device, queue, pipeline, and per-frame
//! buffers. The renderer re-creates the instance buffer each frame (matching
//! Warp's pattern — see `docs/analysis/warp-rendering-research.md` §3.1).

use std::sync::Arc;

use winit::window::Window;

use crate::atlas::GlyphAtlas;
use crate::instances::{GlyphInstance, RectInstance, Uniforms};
use crate::pipeline::{
    create_atlas_bind_group_layout, create_prim_pipeline, create_text_pipeline,
    create_uniform_bind_group_layout,
};

pub struct GpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    prim_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    uniform_bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    atlas: GlyphAtlas,
    atlas_bind_group: wgpu::BindGroup,
    size: winit::dpi::PhysicalSize<u32>,
    scale_factor: f32,
}

impl GpuRenderer {
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .expect("create_surface failed");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no compatible GPU adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("term_gpu/device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .expect("request_device failed");

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let uniform_bgl = create_uniform_bind_group_layout(&device);
        let atlas_bgl = create_atlas_bind_group_layout(&device);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("term_gpu/uniform_buffer"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("term_gpu/uniform_bind_group"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let atlas = GlyphAtlas::new(&device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("term_gpu/atlas_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("term_gpu/atlas_bind_group"),
            layout: &atlas_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let prim_pipeline = create_prim_pipeline(&device, format, &uniform_bgl);
        let text_pipeline = create_text_pipeline(&device, format, &uniform_bgl, &atlas_bgl);

        Self {
            surface,
            device,
            queue,
            config,
            prim_pipeline,
            text_pipeline,
            uniform_bind_group,
            uniform_buffer,
            atlas,
            atlas_bind_group,
            size,
            scale_factor,
        }
    }

    /// Mutable access to the glyph atlas. Use this from the per-frame text
    /// path: shape glyphs, then `atlas_mut().get_or_insert(cache_key, …)`
    /// for each one to obtain its `PlacedGlyph` for `GlyphInstance`.
    pub fn atlas_mut(&mut self) -> &mut GlyphAtlas {
        &mut self.atlas
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> winit::dpi::PhysicalSize<u32> {
        self.size
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.scale_factor = scale_factor;
    }

    pub fn render(
        &mut self,
        rects: &[RectInstance],
        glyphs: &[GlyphInstance],
        scroll_offset_y: f32,
    ) {
        // Flush any pending atlas updates from this frame's get_or_insert calls.
        self.atlas.upload(&self.queue);

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                eprintln!("term_gpu: surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let uniforms = Uniforms {
            screen_size: [self.size.width as f32, self.size.height as f32],
            scroll_offset: [0.0, scroll_offset_y],
            scale_factor: self.scale_factor,
            _pad: [0.0; 3],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, uniforms.as_bytes());

        let rect_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("term_gpu/rect_instance_buffer"),
            size: (std::mem::size_of::<RectInstance>() * rects.len().max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !rects.is_empty() {
            self.queue
                .write_buffer(&rect_buffer, 0, RectInstance::as_bytes(rects));
        }

        let glyph_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("term_gpu/glyph_instance_buffer"),
            size: (std::mem::size_of::<GlyphInstance>() * glyphs.len().max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !glyphs.is_empty() {
            self.queue
                .write_buffer(&glyph_buffer, 0, GlyphInstance::as_bytes(glyphs));
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("term_gpu/encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("term_gpu/main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.04,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            // Rect pass first — backgrounds, ruler, etc.
            pass.set_pipeline(&self.prim_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, rect_buffer.slice(..));
            if !rects.is_empty() {
                pass.draw(0..6, 0..rects.len() as u32);
            }
            // Glyph pass — text on top. Uniform bind group at @group(0) is
            // still bound; only @group(1) needs to change.
            pass.set_pipeline(&self.text_pipeline);
            pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            pass.set_vertex_buffer(0, glyph_buffer.slice(..));
            if !glyphs.is_empty() {
                pass.draw(0..6, 0..glyphs.len() as u32);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // Age unused atlas entries; entries reused this frame stay fresh.
        self.atlas.end_frame();
    }
}
