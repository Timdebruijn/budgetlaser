//! wgpu pipeline: capsules (segments/dots) additively into an HDR target,
//! optional glow by decaying the previous target, then tonemap to the surface.

use crate::config::{Mapping, Output as Cfg};
use crate::point::Segment;
use std::sync::Arc;
use winit::window::Window;

const MAX_SEGMENTS: usize = 8192; // ponytail: fixed cap; grow the buffer when ILDA frames exceed it
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Uniforms {
    scale: [f32; 2],
    res: [f32; 2],
    half_width: f32,
    decay: f32,
    exposure: f32,
    gamma_out: f32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    cfg: Cfg,
    uniforms: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    line_pipeline: wgpu::RenderPipeline,
    decay_pipeline: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,
    segments: wgpu::Buffer,
    hdr: [(wgpu::TextureView, wgpu::BindGroup); 2],
    hdr_size: (u32, u32),
    frame: usize,
    first: bool,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        display: winit::event_loop::OwnedDisplayHandle,
        cfg: Cfg,
    ) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)),
        );
        let surface = instance.create_surface(window).expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no GPU adapter");
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("budgetlaser"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .unwrap();
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo; // vsync, no tearing
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });
        let fs_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&uniform_layout), Some(&tex_layout)],
            immediate_size: 0,
        });

        let seg_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Segment>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x3],
        };
        let pipeline =
            |label, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout>], layout, format, blend| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(blend),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let line_pipeline = pipeline(
            "line",
            "vs_line",
            "fs_line",
            &[Some(seg_layout)],
            &line_layout,
            HDR,
            wgpu::BlendState::ADDITIVE,
        );
        let decay_pipeline = pipeline(
            "decay",
            "vs_fullscreen",
            "fs_decay",
            &[],
            &fs_layout,
            HDR,
            wgpu::BlendState::REPLACE,
        );
        let tonemap_pipeline = pipeline(
            "tonemap",
            "vs_fullscreen",
            "fs_tonemap",
            &[],
            &fs_layout,
            format,
            wgpu::BlendState::REPLACE,
        );

        let segments = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segments"),
            size: (MAX_SEGMENTS * std::mem::size_of::<Segment>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let hdr_size = target_size(&config, cfg.internal_scale);
        let hdr = make_targets(&device, &tex_layout, &sampler, hdr_size);
        Self {
            surface,
            device,
            queue,
            config,
            cfg,
            uniforms,
            uniform_bg,
            tex_layout,
            sampler,
            line_pipeline,
            decay_pipeline,
            tonemap_pipeline,
            segments,
            hdr,
            hdr_size,
            frame: 0,
            first: true,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.recreate_targets();
    }

    fn recreate_targets(&mut self) {
        self.hdr_size = target_size(&self.config, self.cfg.internal_scale);
        log::info!(
            "surface {}x{}, internal {}x{}",
            self.config.width,
            self.config.height,
            self.hdr_size.0,
            self.hdr_size.1
        );
        self.hdr = make_targets(&self.device, &self.tex_layout, &self.sampler, self.hdr_size);
        self.first = true;
    }

    /// Draws and presents one frame; false when nothing was shown (window hidden).
    pub fn render(&mut self, segs: &[Segment]) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // No vsync to block on while hidden or minimised; without this the
                // loop spins at 100% CPU.
                std::thread::sleep(std::time::Duration::from_millis(16));
                return false;
            }
            _ => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
        };
        let (w, h) = (self.hdr_size.0 as f32, self.hdr_size.1 as f32);
        let scale = match self.cfg.mapping {
            Mapping::Stretch => [1.0, 1.0],
            Mapping::Fit if w > h => [h / w, 1.0],
            Mapping::Fit => [1.0, w / h],
        };
        self.queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                scale,
                res: [w, h],
                half_width: (self.cfg.line_width_px * self.cfg.internal_scale * 0.5).max(0.5),
                decay: if self.cfg.glow && !self.first {
                    self.cfg.glow_decay
                } else {
                    0.0
                },
                exposure: self.cfg.exposure,
                gamma_out: if self.config.format.is_srgb() {
                    0.0
                } else {
                    1.0
                },
            }),
        );
        let n = segs.len().min(MAX_SEGMENTS);
        if n > 0 {
            self.queue
                .write_buffer(&self.segments, 0, bytemuck::cast_slice(&segs[..n]));
        }

        let cur = self.frame % 2;
        let prev = 1 - cur;
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hdr"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr[cur].0,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.uniform_bg, &[]);
            if self.cfg.glow && !self.first {
                pass.set_pipeline(&self.decay_pipeline);
                pass.set_bind_group(1, &self.hdr[prev].1, &[]);
                pass.draw(0..3, 0..1);
            }
            if n > 0 {
                // Clip to the scan field: points outside it are simply not drawn.
                let (sw, sh) = (w * scale[0], h * scale[1]);
                pass.set_scissor_rect(
                    ((w - sw) / 2.0) as u32,
                    ((h - sh) / 2.0) as u32,
                    sw.round().max(1.0) as u32,
                    sh.round().max(1.0) as u32,
                );
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, self.segments.slice(..));
                pass.draw(0..6, 0..n as u32);
            }
        }
        {
            let view = frame.texture.create_view(&Default::default());
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &self.uniform_bg, &[]);
            pass.set_bind_group(1, &self.hdr[cur].1, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        self.frame += 1;
        self.first = false;
        true
    }
}

fn target_size(config: &wgpu::SurfaceConfiguration, scale: f32) -> (u32, u32) {
    let f = |v: u32| ((v as f32 * scale).round() as u32).max(1);
    (f(config.width), f(config.height))
}

fn make_targets(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    (w, h): (u32, u32),
) -> [(wgpu::TextureView, wgpu::BindGroup); 2] {
    let make = |label| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        (view, bg)
    };
    [make("hdr0"), make("hdr1")]
}
