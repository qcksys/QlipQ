//! A custom iced `shader` widget that renders raw RGBA video frames through a single, **persistent**
//! `wgpu::Texture` updated in place via `queue.write_texture`.
//!
//! Why this exists: feeding live frames through `image::Handle::from_rgba` mints a brand-new image
//! id every frame, and iced's wgpu image cache evicts ids not seen in the current frame — so each
//! frame allocates then frees a slot in the shared texture atlas, which repacks and **flickers**. A
//! shader primitive owns its own texture and bypasses the atlas entirely. This mirrors how
//! `iced_video_player` renders on iced 0.14.
//!
//! iced sets the render-pass viewport to the widget's bounds before [`Primitive::draw`], so a
//! fullscreen triangle with UV 0..1 maps the whole frame into the widget — no transform needed.
//! Size the widget to the frame's aspect ratio to avoid stretching.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;

/// The latest decoded frame, shared between the app (writer) and the shader primitive (reader).
#[derive(Default)]
pub struct FrameSlot {
    width: u32,
    height: u32,
    data: Vec<u8>,
    /// Bumped on every new frame so the GPU knows to re-upload.
    generation: u64,
}

pub type SharedFrame = Arc<Mutex<FrameSlot>>;

pub fn new_shared_frame() -> SharedFrame {
    Arc::new(Mutex::new(FrameSlot::default()))
}

/// Store a new RGBA frame (`width * height * 4` bytes); bumps the generation to trigger re-upload.
pub fn push_frame(slot: &SharedFrame, width: u32, height: u32, data: Vec<u8>) {
    if let Ok(mut s) = slot.lock() {
        s.width = width;
        s.height = height;
        s.data = data;
        s.generation = s.generation.wrapping_add(1);
    }
}

/// `shader::Program` that renders the current frame from a [`SharedFrame`].
pub struct VideoProgram {
    frame: SharedFrame,
}

impl VideoProgram {
    pub fn new(frame: SharedFrame) -> Self {
        Self { frame }
    }
}

impl<Message> shader::Program<Message> for VideoProgram {
    type State = ();
    type Primitive = VideoPrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> VideoPrimitive {
        VideoPrimitive { frame: self.frame.clone() }
    }
}

pub struct VideoPrimitive {
    frame: SharedFrame,
}

impl std::fmt::Debug for VideoPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPrimitive").finish_non_exhaustive()
    }
}

impl shader::Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        pipeline: &mut VideoPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        if let Ok(slot) = self.frame.lock() {
            let expected = (slot.width as usize) * (slot.height as usize) * 4;
            if slot.width > 0 && slot.height > 0 && slot.data.len() >= expected {
                pipeline.upload(
                    device,
                    queue,
                    slot.generation,
                    slot.width,
                    slot.height,
                    &slot.data[..expected],
                );
            }
        }
    }

    fn draw(&self, pipeline: &VideoPipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        pipeline.draw(render_pass)
    }
}

/// Long-lived GPU state, created once and stored by iced for all [`VideoPrimitive`]s.
pub struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    texture: Option<wgpu::Texture>,
    bind_group: Option<wgpu::BindGroup>,
    size: (u32, u32),
    uploaded: Option<u64>,
}

impl VideoPipeline {
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
        width: u32,
        height: u32,
        data: &[u8],
    ) {
        // Allocate (or reallocate on a resolution change) the persistent texture + bind group.
        if self.texture.is_none() || self.size != (width, height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("qlipq video frame"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("qlipq video bind group"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.texture = Some(texture);
            self.bind_group = Some(bind_group);
            self.size = (width, height);
            self.uploaded = None;
        }

        // Re-upload only when the frame actually changed. `write_texture` has no row-alignment
        // requirement, so any `width * 4` stride is fine.
        if self.uploaded != Some(generation) {
            if let Some(texture) = &self.texture {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                );
                self.uploaded = Some(generation);
            }
        }
    }

    fn draw(&self, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(bind_group) = &self.bind_group else {
            return false;
        };
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

impl shader::Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("qlipq video texture layout"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("qlipq video pipeline layout"),
            bind_group_layouts: &[&texture_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("qlipq video shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("qlipq video pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            sampler,
            texture_layout,
            texture: None,
            bind_group: None,
            size: (0, 0),
            uploaded: None,
        }
    }
}

/// Fullscreen-triangle vertex + texture-sampling fragment. The triangle covers clip space; with the
/// render-pass viewport set to the widget bounds, UV 0..1 maps the whole frame into the widget.
const SHADER: &str = r#"
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var out: VsOut;
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    out.uv = vec2<f32>(x, y);
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(frame_texture, frame_sampler, in.uv);
}
"#;
