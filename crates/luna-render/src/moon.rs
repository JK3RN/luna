//! Die Mondkugel: Karten laden, Bild zeichnen, Bild ohne Fenster erzeugen.

use anyhow::{anyhow, bail, Result};
use luna_data::{DataPaths, Lmap, LmapFormat, Manifest};

use crate::camera::FrameParams;
use crate::gpu::Gpu;

/// Darstellungsart des Shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// Das Bild: Relief, Schlagschatten, Streumodell.
    Full = 0,
    /// Prüfbild: Länge und Breite je Bildpunkt (nur für Tests).
    LonLat = 1,
    /// Relief ohne Schlagschatten.
    NoShadows = 2,
    /// Glatte Kugel ohne Relief.
    Smooth = 3,
    /// Prüfbild: einheitlich graue, glatte Kugel (nur für Tests des Streumodells).
    Gray = 4,
}

impl RenderMode {
    pub fn label(self) -> &'static str {
        match self {
            RenderMode::Full => "VOLL",
            RenderMode::LonLat => "PRÜFBILD",
            RenderMode::NoShadows => "OHNE SCHLAGSCHATTEN",
            RenderMode::Smooth => "GLATTE KUGEL",
            RenderMode::Gray => "GRAUE KUGEL",
        }
    }
    /// Nächste für Menschen gedachte Darstellungsart.
    pub fn next(self) -> Self {
        match self {
            RenderMode::Full => RenderMode::NoShadows,
            RenderMode::NoShadows => RenderMode::Smooth,
            _ => RenderMode::Full,
        }
    }
}

pub struct MoonRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    /// Grafikspeicher der Karten in Byte.
    pub texture_bytes: u64,
}

fn upload(gpu: &Gpu, map: &Lmap, label: &str) -> (wgpu::TextureView, u64) {
    let format = match map.format {
        LmapFormat::Rgba8Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        LmapFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        LmapFormat::Rg16Float => wgpu::TextureFormat::Rg16Float,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: map.width, height: map.height, depth_or_array_layers: 1 },
        mip_level_count: map.levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut bytes = 0u64;
    for level in 0..map.levels {
        let (w, h) = luna_data::lmap::level_size(map.width, map.height, level);
        let data = map.level(level);
        bytes += data.len() as u64;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
    (texture.create_view(&wgpu::TextureViewDescriptor::default()), bytes)
}

impl MoonRenderer {
    pub fn new(gpu: &Gpu, paths: &DataPaths, manifest: &Manifest, format: wgpu::TextureFormat) -> Result<Self> {
        if format.is_srgb() {
            bail!("Das Zielformat {format:?} kodiert selbst nach sRGB; LUNA kodiert im Shader und braucht ein lineares Format.");
        }
        if manifest.forge.horizon_directions != 8 {
            bail!("Die Horizontkarten haben {} Richtungen; dieser Renderer erwartet 8.", manifest.forge.horizon_directions);
        }
        let open = |name: &str| Lmap::open(&paths.forged().join(name));
        let (albedo, a) = upload(gpu, &open("albedo.lmap")?, "albedo");
        let (relief, b) = upload(gpu, &open("relief.lmap")?, "relief");
        let (hor_a, c) = upload(gpu, &open("horizon_a.lmap")?, "horizon_a");
        let (hor_b, d) = upload(gpu, &open("horizon_b.lmap")?, "horizon_b");

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("karten"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });

        let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mond"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
                tex_entry(3),
                tex_entry(4),
                tex_entry(5),
            ],
        });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mond"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&albedo) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&relief) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&hor_a) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&hor_b) },
            ],
        });

        let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mond"),
            source: wgpu::ShaderSource::Wgsl(include_str!("moon.wgsl").into()),
        });
        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mond"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mond"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self { pipeline, uniform, bind_group, format, texture_bytes: a + b + c + d })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Überträgt die Zahlen für das nächste Bild.
    pub fn prepare(&self, queue: &wgpu::Queue, params: &FrameParams) {
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(params));
    }

    /// Zeichnet die Mondkugel in einen laufenden Renderpass (deckt das ganze Bild).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Zeichenfläche ohne Fenster.
pub struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    padded_row: u32,
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Offscreen {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen-lesen"),
            size: padded_row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { texture, view, buffer, width, height, padded_row }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Schickt fertige Befehle ab und wartet, bis die Grafikkarte sie ausgeführt hat.
    pub fn submit_and_wait(&self, gpu: &Gpu, encoder: wgpu::CommandEncoder) -> Result<()> {
        let idx = gpu.queue.submit([encoder.finish()]);
        gpu.device
            .poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None })
            .map_err(|e| anyhow!("Grafikkarte antwortet nicht: {e}"))?;
        Ok(())
    }

    /// Zeichnet ein Bild und wartet, bis die Grafikkarte fertig ist. Liest nichts zurück.
    pub fn render(&self, gpu: &Gpu, moon: &MoonRenderer, params: &FrameParams) -> Result<()> {
        moon.prepare(&gpu.queue, params);
        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("offscreen") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            moon.draw(&mut pass);
        }
        let idx = gpu.queue.submit([enc.finish()]);
        gpu.device
            .poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None })
            .map_err(|e| anyhow!("Grafikkarte antwortet nicht: {e}"))?;
        Ok(())
    }

    /// Liest das zuletzt gezeichnete Bild als RGBA, 8 Bit je Kanal, Zeile 0 oben.
    pub fn read(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("lesen") });
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: None },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([enc.finish()]);
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("Grafikkarte antwortet nicht: {e}"))?;
        rx.recv()?.map_err(|e| anyhow!("Bild ließ sich nicht zurücklesen: {e}"))?;
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        {
            let data = slice.get_mapped_range();
            for row in 0..self.height as usize {
                let start = row * self.padded_row as usize;
                out.extend_from_slice(&data[start..start + self.width as usize * 4]);
            }
        }
        self.buffer.unmap();
        Ok(out)
    }
}
