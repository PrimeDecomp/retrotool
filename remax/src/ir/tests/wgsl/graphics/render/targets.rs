use std::collections::BTreeMap;

use super::RenderState;
use crate::maxwell::graphics::{ColorOutputRouting, GraphicsProgram};

const IMAGE_BYTES: u64 = 8 * 256;
const DEPTH_OFFSET: u64 = 8 * IMAGE_BYTES;
pub(super) const FAULT_OFFSET: u64 = DEPTH_OFFSET + IMAGE_BYTES;
pub(super) const READBACK_BYTES: u64 = FAULT_OFFSET + 16;

struct Attachment {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl Attachment {
    fn new(device: &wgpu::Device, samples: u32, format: wgpu::TextureFormat) -> Self {
        let texture = |sample_count| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | if sample_count == 1 {
                        wgpu::TextureUsages::COPY_SRC
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                view_formats: &[],
            })
        };
        let target = texture(samples);
        let view = target.create_view(&Default::default());
        let resolve = (samples > 1).then(|| {
            let target = texture(1);
            let view = target.create_view(&Default::default());
            (target, view)
        });
        Self { texture: target, view, resolve }
    }

    fn copy_to(&self, encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer, offset: u64) {
        let texture = self.resolve.as_ref().map(|(texture, _)| texture).unwrap_or(&self.texture);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
        );
    }
}

pub(super) struct Targets {
    colors: Vec<Option<Attachment>>,
    depth: Option<Attachment>,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    clear: wgpu::Color,
}

impl Targets {
    pub fn new(device: &wgpu::Device, program: &GraphicsProgram, state: &RenderState) -> Self {
        let samples = state.samples;
        // RGBA32F blending is not portable; use UNORM for blend/MSAA probes.
        let format = if samples == 1 && state.blend.is_none() {
            wgpu::TextureFormat::Rgba32Float
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let mut colors: Vec<_> = program
            .fragment_outputs
            .colors
            .iter()
            .map(|registers| {
                registers
                    .iter()
                    .any(Option::is_some)
                    .then(|| Attachment::new(device, samples, format))
            })
            .collect();
        if program.color_routing == ColorOutputRouting::DualSource {
            colors.truncate(1);
        }
        while colors.last().is_some_and(Option::is_none) {
            colors.pop();
        }
        // A pass with no colors still needs an attachment to establish its
        // extent and sample count. It can test fixed-function depth writes too.
        let depth = if program.fragment_outputs.depth.is_some() || colors.is_empty() {
            assert_eq!(samples, 1, "this harness only reads single-sample depth");
            Some(Attachment::new(device, 1, wgpu::TextureFormat::Depth32Float))
        } else {
            None
        };
        Self { colors, depth, format, blend: state.blend, clear: state.clear }
    }

    pub fn states(&self) -> Vec<Option<wgpu::ColorTargetState>> {
        self.colors
            .iter()
            .map(|attachment| {
                attachment.as_ref().map(|_| wgpu::ColorTargetState {
                    format: self.format,
                    blend: self.blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect()
    }

    pub fn depth_state(&self) -> Option<wgpu::DepthStencilState> {
        self.depth.as_ref().map(|_| wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        })
    }

    pub fn color_attachments(&self) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        self.colors
            .iter()
            .map(|attachment| {
                attachment.as_ref().map(|attachment| wgpu::RenderPassColorAttachment {
                    view: &attachment.view,
                    resolve_target: attachment.resolve.as_ref().map(|(_, view)| view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect()
    }

    pub fn depth_attachment(&self) -> Option<wgpu::RenderPassDepthStencilAttachment<'_>> {
        self.depth.as_ref().map(|attachment| wgpu::RenderPassDepthStencilAttachment {
            view: &attachment.view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        })
    }

    pub fn copy_to(&self, encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer) {
        for (target, attachment) in self.colors.iter().enumerate() {
            if let Some(attachment) = attachment {
                attachment.copy_to(encoder, buffer, target as u64 * IMAGE_BYTES);
            }
        }
        if let Some(depth) = &self.depth {
            depth.copy_to(encoder, buffer, DEPTH_OFFSET);
        }
    }

    pub fn read_colors(&self, data: &[u8]) -> BTreeMap<u8, Vec<[f32; 4]>> {
        let float_pixels = self.format == wgpu::TextureFormat::Rgba32Float;
        let pixel_bytes = if float_pixels { 16 } else { 4 };
        self.colors
            .iter()
            .enumerate()
            .filter_map(|(target, attachment)| {
                attachment.as_ref()?;
                let image = image_bytes(data, target as u64 * IMAGE_BYTES);
                let pixels = image
                    .chunks_exact(256)
                    .flat_map(|row| {
                        row[..8 * pixel_bytes].chunks_exact(pixel_bytes).map(|pixel| {
                            std::array::from_fn(|component| {
                                if float_pixels {
                                    read_float(pixel, component * 4)
                                } else {
                                    f32::from(pixel[component]) / 255.
                                }
                            })
                        })
                    })
                    .collect();
                Some((target as u8, pixels))
            })
            .collect()
    }

    pub fn read_depth(&self, data: &[u8]) -> Option<Vec<f32>> {
        self.depth.as_ref()?;
        Some(
            image_bytes(data, DEPTH_OFFSET)
                .chunks_exact(256)
                .flat_map(|row| (0..8).map(|column| read_float(row, column * 4)))
                .collect(),
        )
    }
}

fn image_bytes(data: &[u8], offset: u64) -> &[u8] {
    &data[offset as usize..(offset + IMAGE_BYTES) as usize]
}

fn read_float(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}
