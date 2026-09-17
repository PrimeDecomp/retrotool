use super::Gpu;

pub(super) const LEVELS: u32 = 5;

fn texel(level: u32, x: u32, y: u32) -> [u8; 4] {
    [
        (x * 255 / ((32 >> level) - 1)) as u8,
        ((3 * x + 17 * y + 13 * level) % 256) as u8,
        (50 * level) as u8,
        (255 - 3 * x - 2 * y) as u8,
    ]
}

pub(super) fn upload(gpu: &Gpu) -> wgpu::Texture {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SimpleTexture mip fixture"),
        size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
        mip_level_count: LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for level in 0..LEVELS {
        let size = 32 >> level;
        let bytes = (0..size)
            .flat_map(|y| (0..size).flat_map(move |x| texel(level, x, y)))
            .collect::<Vec<_>>();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
    }
    texture
}

/// Independent bilinear/trilinear reference over the deliberately authored mip
/// images. LOD is relative to the bound view, not the original texture's mip0.
pub(super) fn sample(uv: [f64; 2], lod: f64, base_mip: u32) -> [f64; 4] {
    let lod = lod.clamp(0., f64::from(LEVELS - base_mip - 1));
    let lower = lod.floor() as u32;
    let upper = (lower + 1).min(LEVELS - base_mip - 1);
    let low = bilinear(uv, base_mip + lower);
    let high = bilinear(uv, base_mip + upper);
    std::array::from_fn(|channel| low[channel] + (high[channel] - low[channel]) * lod.fract())
}

fn bilinear(uv: [f64; 2], level: u32) -> [f64; 4] {
    let size = 32 >> level;
    let [x, y] = uv.map(|coordinate| coordinate * f64::from(size) - 0.5);
    let fraction = [x - x.floor(), y - y.floor()];
    let coordinates = [x, y].map(|coordinate| {
        [coordinate.floor(), coordinate.floor() + 1.]
            .map(|value| value.clamp(0., f64::from(size - 1)) as u32)
    });
    let mut result = [0.; 4];
    for row in 0..2 {
        for column in 0..2 {
            let weight_x = if column == 0 { 1. - fraction[0] } else { fraction[0] };
            let weight_y = if row == 0 { 1. - fraction[1] } else { fraction[1] };
            let texel = texel(level, coordinates[0][column], coordinates[1][row]);
            for channel in 0..4 {
                result[channel] += f64::from(texel[channel]) / 255. * weight_x * weight_y;
            }
        }
    }
    result
}
