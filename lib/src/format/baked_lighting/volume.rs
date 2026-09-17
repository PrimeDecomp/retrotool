//! Sparse CPU view of the six probe volumes. Tiles retain their deswizzled BCn
//! bytes; only the blocks needed by a sample are decoded. Missing interior tiles
//! are errors, separate from the sampler's established black exterior border.

use std::collections::{btree_map::Entry, BTreeMap};

use anyhow::{ensure, Context, Result};
use zerocopy::ByteOrder;

use super::{probe::ProbeSamples, ProbeLayerLayout};
use crate::format::{texture_sampler::TextureSamplerParams, txtr::TextureData};

/// Borrowed, explicitly selected physical tiles. This does not resolve scene
/// residency or priority between overlapping LTPB resources.
pub struct ProbeVolume<'a> {
    grid_origin: [i32; 3],
    dimensions: [u32; 3],
    layers: [ProbeLayerLayout; 6],
    tiles: [BTreeMap<[u32; 3], &'a [u8]>; 6],
}

impl<'a> ProbeVolume<'a> {
    /// Use the actual aligned grid origin and dimensions, e.g. from the native
    /// `ProbeVolumeLayout`. No dense texture allocation is required.
    pub fn new(grid_origin: [i32; 3], dimensions: [u32; 3]) -> Result<Self> {
        let mut layers = Vec::with_capacity(6);
        for layer in 0..6 {
            layers.push(ProbeLayerLayout::for_volume(grid_origin, dimensions, layer)?);
        }
        Ok(Self {
            grid_origin,
            dimensions,
            layers: layers.try_into().unwrap(),
            tiles: std::array::from_fn(|_| BTreeMap::new()),
        })
    }

    /// An isolated-resource view enclosing an inclusive probe grid. This uses
    /// the same common tile alignment as the runtime allocator, but does not
    /// claim that one LTPB's bounds equal a complete scene's volume bounds.
    pub fn enclosing_grid(minimum: [i32; 3], maximum: [i32; 3]) -> Result<Self> {
        let mut origin = [0; 3];
        let mut dimensions = [0; 3];
        for (axis, alignment) in [128i64, 64, 16].into_iter().enumerate() {
            ensure!(minimum[axis] <= maximum[axis], "Reversed probe grid bounds");
            let lo = i64::from(minimum[axis]).div_euclid(alignment) * alignment;
            let hi = (i64::from(maximum[axis]).div_euclid(alignment) + 1) * alignment;
            origin[axis] = i32::try_from(lo)?;
            dimensions[axis] = u32::try_from(hi - lo)?;
        }
        Self::new(origin, dimensions)
    }

    pub fn grid_origin(&self) -> [i32; 3] { self.grid_origin }

    pub fn dimensions(&self) -> [u32; 3] { self.dimensions }

    /// Initialize71001c8cbc uses linear filtering, border clamping and no
    /// anisotropy. The store at71001c93fc overrides Simple's border with 1:
    /// opaque black, not the SDK's default transparent black.
    pub fn sampler_params() -> TextureSamplerParams { TextureSamplerParams::simple(1, [4; 3], 1) }

    /// Add one selected LTPB tile. Returns false for tiles outside this volume.
    /// Duplicate mappings are rejected without replacement: callers must choose
    /// the owner before insertion, including when identical data is repeated.
    pub fn insert_tile<O: ByteOrder>(
        &mut self,
        layer: u32,
        position: [i32; 3],
        texture: &'a TextureData<O>,
    ) -> Result<bool> {
        let layout = self.layers.get(layer as usize).context("Unknown probe volume layer")?;
        let Some(mapping) = layout.map_tile(position)? else { return Ok(false) };
        layout.validate_texture(texture)?;
        match self.tiles[layer as usize].entry(mapping.tile) {
            Entry::Vacant(entry) => {
                entry.insert(&texture.data);
            }
            Entry::Occupied(_) => {
                anyhow::bail!("Duplicate probe tile in layer {layer} at {position:?}")
            }
        }
        Ok(true)
    }

    /// Sample all six volumes at normalized texture coordinates, at LOD zero.
    /// Uses mathematical trilinear weights and the native black border. This is
    /// a host reference sampler: BC1 decoding and texture-unit weight precision
    /// can differ from the GPU. It does not emulate unbound sparse-page contents.
    pub fn sample(&self, coordinate: [f32; 3]) -> Result<ProbeSamples> {
        Ok(ProbeSamples {
            midpoint_rgb: self.sample_layer(0, coordinate)?,
            range_rgb: self.sample_layer(1, coordinate)?,
            exponent_rgb: self.sample_layer(2, coordinate)?,
            gradient_directions: [
                self.sample_layer(3, coordinate)?,
                self.sample_layer(4, coordinate)?,
                self.sample_layer(5, coordinate)?,
            ],
        })
    }

    pub fn sample_layer(&self, layer: u32, coordinate: [f32; 3]) -> Result<[f32; 3]> {
        let layout = self.layers.get(layer as usize).context("Unknown probe volume layer")?;
        ensure!(coordinate.into_iter().all(f32::is_finite), "Nonfinite probe sample coordinate");
        // Wider host arithmetic keeps finite far-outside coordinates from
        // overflowing before the border test. Centers are at (index+0.5)/size.
        let texel: [f64; 3] =
            std::array::from_fn(|i| f64::from(coordinate[i]) * f64::from(self.dimensions[i]) - 0.5);
        if (0..3).any(|i| texel[i] <= -1.0 || texel[i] >= f64::from(self.dimensions[i])) {
            return Ok([0.0; 3]);
        }
        let base = texel.map(|v| v.floor() as i64);
        let fraction: [f64; 3] = std::array::from_fn(|i| texel[i] - base[i] as f64);
        // Accumulate in f64 so rounding the weights cannot turn a constant
        // UNORM 1 into an out-of-range f32 result rejected by probe decoding.
        let mut result = [0.0f64; 3];
        for corner in 0..8 {
            let high: [bool; 3] = std::array::from_fn(|i| corner & (1 << i) != 0);
            let weight: f64 =
                (0..3).map(|i| if high[i] { fraction[i] } else { 1.0 - fraction[i] }).product();
            if weight == 0.0 {
                continue;
            }
            let position: [i64; 3] = std::array::from_fn(|i| base[i] + i64::from(high[i]));
            if (0..3).any(|i| position[i] < 0 || position[i] >= i64::from(self.dimensions[i])) {
                continue;
            }
            let position = position.map(|v| v as u32);
            let tile: [u32; 3] = std::array::from_fn(|i| position[i] / layout.tile_dimensions[i]);
            let bytes = self.tiles[layer as usize].get(&tile).with_context(|| {
                let absolute: [i64; 3] =
                    std::array::from_fn(|i| i64::from(layout.min_tile[i]) + i64::from(tile[i]));
                format!("Missing probe tile in layer {layer} at {absolute:?}")
            })?;
            let local: [u32; 3] = std::array::from_fn(|i| position[i] % layout.tile_dimensions[i]);
            let rgb = decode_texel(layer, layout, bytes, local);
            for channel in 0..3 {
                result[channel] += f64::from(rgb[channel]) * weight;
            }
        }
        Ok(result.map(|v| v as f32))
    }

    /// Convert a continuous probe-grid index to normalized texture coordinates.
    /// For an isolated LTPB, divide a storage-space position by grid_spacing to
    /// get this index. Scene renderers can instead use their bound BPTX matrices.
    pub fn grid_coordinate(&self, index: [f32; 3]) -> Result<[f32; 3]> {
        ensure!(index.into_iter().all(f32::is_finite), "Nonfinite probe grid index");
        Ok(std::array::from_fn(|i| {
            ((f64::from(index[i]) - f64::from(self.grid_origin[i]) + 0.5)
                / f64::from(self.dimensions[i])) as f32
        }))
    }
}

fn decode_texel(
    layer: u32,
    layout: &ProbeLayerLayout,
    bytes: &[u8],
    position: [u32; 3],
) -> [f32; 3] {
    let [x, y, z] = position;
    let blocks_x = layout.tile_dimensions[0] / 4;
    let blocks_y = layout.tile_dimensions[1] / 4;
    let block = ((z * blocks_y + y / 4) * blocks_x + x / 4) as usize;
    let texel = ((y % 4) * 4 + x % 4) as usize;
    if layer < 2 {
        let mut rgb = [0.0; 48];
        bcdec_rs::bc6h_float(&bytes[block * 16..block * 16 + 16], &mut rgb, 12, false);
        [rgb[texel * 3], rgb[texel * 3 + 1], rgb[texel * 3 + 2]]
    } else {
        let mut rgba = [0; 64];
        bcdec_rs::bc1(&bytes[block * 8..block * 8 + 8], &mut rgba, 16);
        std::array::from_fn(|i| f32::from(rgba[texel * 4 + i]) / 255.0)
    }
}

#[cfg(test)]
mod tests;
