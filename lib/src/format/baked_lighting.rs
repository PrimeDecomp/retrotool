//! Runtime layout of MPR's six sparse baked-probe textures.
//! This is derived scene state, not another serialized LTPB header. The caller
//! supplies the selected scene bounds and world-to-storage transform.

pub mod probe;
pub mod residency;
pub mod volume;

use anyhow::{ensure, Context, Result};
use zerocopy::ByteOrder;

use super::{
    txtr::{ETextureFormat, ETextureType, TextureData},
    CAABox,
};

#[derive(Debug, Clone)]
pub struct ProbeVolumeLayout {
    /// Inclusive minimum probe index after alignment to all six tile layouts.
    pub grid_origin: [i32; 3],
    pub dimensions: [u32; 3],
    pub probes_per_unit: f32,
    pub world_to_grid_rows: [[f32; 4]; 3],
    pub grid_to_texture_rows: [[f32; 4]; 3],
    pub world_to_texture_rows: [[f32; 4]; 3],
}

#[derive(Debug, Clone)]
pub struct ProbeLayerLayout {
    pub format: ETextureFormat,
    pub tile_dimensions: [u32; 3],
    pub min_tile: [i32; 3],
    pub max_tile: [i32; 3],
    pub tile_counts: [u32; 3],
    pub bytes_per_tile: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTileMapping {
    /// Coordinates relative to this virtual volume's first tile.
    pub tile: [u32; 3],
    pub texel_origin: [u32; 3],
    /// Native sparse page address, not an offset in a linear BCn image.
    pub virtual_byte_offset: u64,
    pub byte_len: u64,
}

/// The two BPTX uniform matrices built by UpdateBakedProbeTexMaterial71001c9f50.
#[derive(Debug, Clone)]
pub struct BakedProbeTransforms {
    pub view_to_texture_rows: [[f32; 4]; 3],
    /// Native semantic ViewToProbeSpaceInverseTranspose. The builder applies
    /// inverse-transpose to view-to-world alone, without the volume transform.
    pub view_to_probe_inverse_transpose_rows: [[f32; 4]; 3],
}

impl BakedProbeTransforms {
    /// Follow the engine's camera conversion, general inverse and BPTX builder.
    /// The camera is the engine's row-major 3x4 camera-to-world transform.
    /// Nonfinite inputs/results are rejected; native singular-camera behavior
    /// is not a usable rendering transform.
    pub fn from_camera(
        world_to_texture: [[f32; 4]; 3],
        camera_to_world: [[f32; 4]; 3],
    ) -> Result<Self> {
        ensure!(camera_to_world.iter().flatten().all(|v| v.is_finite()), "Nonfinite camera");
        let [[a, b, c, x], [d, e, f, y], [g, h, i, z]] = camera_to_world;
        // CreateNativeWorldToView710012df54: transpose/sign conversion and
        // explicitly ordered, unfused translation arithmetic.
        let world_to_view = [
            [-a, -d, -g, -((y * -d - a * x) - g * z)],
            [b, e, h, -((x * b + y * e) + z * h)],
            [-c, -f, -i, -((y * -f - c * x) - i * z)],
        ];
        Self::from_view_to_world(world_to_texture, inverse(world_to_view))
    }

    /// Accept an already resolved native/host view-to-world transform. This
    /// exposes the same builder products for callers with their own camera.
    pub fn from_view_to_world(
        world_to_texture: [[f32; 4]; 3],
        view_to_world: [[f32; 4]; 3],
    ) -> Result<Self> {
        ensure!(
            world_to_texture
                .iter()
                .flatten()
                .chain(view_to_world.iter().flatten())
                .all(|v| v.is_finite()),
            "Nonfinite baked probe transform"
        );
        let view_to_texture_rows = multiply(world_to_texture, view_to_world);
        let (determinant, cofactors) = linear_cofactors(view_to_world);
        let view_to_probe_inverse_transpose_rows =
            if determinant.abs() <= f32::from_bits(0x32000000) {
                [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
            } else {
                let reciprocal = 1.0 / determinant;
                cofactors
                    .map(|row| [row[0] * reciprocal, row[1] * reciprocal, row[2] * reciprocal, 0.0])
            };
        let result = Self { view_to_texture_rows, view_to_probe_inverse_transpose_rows };
        ensure!(
            result
                .view_to_texture_rows
                .iter()
                .flatten()
                .chain(result.view_to_probe_inverse_transpose_rows.iter().flatten())
                .all(|v| v.is_finite()),
            "Nonfinite baked probe matrix result"
        );
        Ok(result)
    }

    /// Raw 96-byte field span at CBakedLightingProbeTextureMaterialData+0x10.
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut bytes = [0; 96];
        let values = self
            .view_to_texture_rows
            .iter()
            .flatten()
            .chain(self.view_to_probe_inverse_transpose_rows.iter().flatten());
        for (word, value) in bytes.chunks_exact_mut(4).zip(values) {
            word.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

impl ProbeVolumeLayout {
    /// Reconstruct Initialize's CPU layout for finite bounds and density.
    /// Native callers currently supply 0.5 probes per unit. Bounds can include
    /// both zone bounds and several selected LTPB storages; a single PHDR alone
    /// does not determine the game's final scene volume.
    ///
    /// Normalization reproduces native FRECPE and two FRECPS refinements for
    /// positive u32 dimensions. Matrix multiplication preserves the native
    /// fused-operation order.
    pub fn from_bounds(
        bounds: CAABox,
        world_to_storage_rows: [[f32; 4]; 3],
        probes_per_unit: f32,
    ) -> Result<Self> {
        ensure!(probes_per_unit.is_finite() && probes_per_unit > 0.0, "Invalid probe density");
        ensure!(
            world_to_storage_rows.iter().flatten().all(|v| v.is_finite()),
            "Nonfinite probe transform"
        );
        let min = bounds.min.to_array();
        let max = bounds.max.to_array();
        let alignment = [128i32, 64, 16];
        let mut grid_origin = [0; 3];
        let mut dimensions = [0; 3];
        for i in 0..3 {
            ensure!(
                min[i].is_finite() && max[i].is_finite() && min[i] <= max[i],
                "Invalid probe bounds"
            );
            let index = |value: f32| -> Result<i32> {
                // Native FMUL, FADD 0.5, then FCVTMS (round toward -infinity).
                let value = value * probes_per_unit + 0.5;
                ensure!(
                    value.is_finite() && (-2147483648.0..2147483648.0).contains(&value),
                    "Probe index is out of range"
                );
                Ok(value.floor() as i32)
            };
            let lo = index(min[i])?.div_euclid(alignment[i]);
            let hi = index(next_down(max[i]))?.div_euclid(alignment[i]);
            grid_origin[i] = lo.checked_mul(alignment[i]).context("Probe origin overflow")?;
            let end = hi
                .checked_add(1)
                .and_then(|v| v.checked_mul(alignment[i]))
                .context("Probe extent overflow")?;
            dimensions[i] = u32::try_from(i64::from(end) - i64::from(grid_origin[i]))?;
            ensure!(dimensions[i] > 0, "Empty probe volume");
        }
        let scale = |values: [f32; 3]| {
            std::array::from_fn(|i| std::array::from_fn(|j| if i == j { values[i] } else { 0.0 }))
        };
        let world_to_grid_rows = multiply(scale([probes_per_unit; 3]), world_to_storage_rows);
        let mut translation = scale([1.0; 3]);
        for i in 0..3 {
            translation[i][3] = -(grid_origin[i] as f32 - 0.5);
        }
        let grid_to_texture_rows =
            multiply(scale(dimensions.map(dimension_reciprocal)), translation);
        let world_to_texture_rows = multiply(grid_to_texture_rows, world_to_grid_rows);
        Ok(Self {
            grid_origin,
            dimensions,
            probes_per_unit,
            world_to_grid_rows,
            grid_to_texture_rows,
            world_to_texture_rows,
        })
    }

    pub fn layer(&self, layer: u32) -> Result<ProbeLayerLayout> {
        ProbeLayerLayout::for_volume(self.grid_origin, self.dimensions, layer)
    }
}

impl ProbeLayerLayout {
    fn for_volume(grid_origin: [i32; 3], dimensions: [u32; 3], layer: u32) -> Result<Self> {
        ensure!(layer < 6, "Unknown probe texture layer {layer}");
        let (format, tile_dimensions) = if layer < 2 {
            (ETextureFormat::BptcUfloat, [64u32, 64, 16])
        } else {
            (ETextureFormat::RgbaBc1Unorm, [128u32, 64, 16])
        };
        let min_tile =
            std::array::from_fn(|i| grid_origin[i].div_euclid(tile_dimensions[i] as i32));
        let tile_counts = std::array::from_fn(|i| dimensions[i] / tile_dimensions[i]);
        let mut max_tile = [0; 3];
        for i in 0..3 {
            ensure!(
                grid_origin[i].rem_euclid(tile_dimensions[i] as i32) == 0
                    && dimensions[i].is_multiple_of(tile_dimensions[i])
                    && tile_counts[i] > 0,
                "Probe volume is not tile aligned"
            );
            max_tile[i] = i32::try_from(i64::from(min_tile[i]) + i64::from(tile_counts[i]) - 1)?;
        }
        Ok(ProbeLayerLayout {
            format,
            tile_dimensions,
            min_tile,
            max_tile,
            tile_counts,
            bytes_per_tile: 65536,
        })
    }

    pub fn linear_byte_len(&self) -> Result<usize> {
        let tiles = self
            .tile_counts
            .iter()
            .try_fold(1u64, |v, &n| v.checked_mul(u64::from(n)))
            .context("Probe volume size overflow")?;
        Ok(usize::try_from(
            tiles.checked_mul(self.bytes_per_tile).context("Probe volume size overflow")?,
        )?)
    }

    /// Copy one deswizzled LTPB tile into a caller-owned linear BCn volume.
    /// The caller initializes missing regions and chooses among overlapping
    /// scene tiles; this function does not infer their runtime priority.
    pub fn write_tile<O: ByteOrder>(
        &self,
        target: &mut [u8],
        position: [i32; 3],
        texture: &TextureData<O>,
    ) -> Result<Option<ProbeTileMapping>> {
        let Some(mapping) = self.map_tile(position)? else {
            return Ok(None);
        };
        ensure!(target.len() == self.linear_byte_len()?, "Wrong probe volume byte size");
        self.validate_texture(texture)?;
        let (bw, bh, bd) = self.format.block_size();
        ensure!(bd == 1, "Unsupported probe block depth");
        let (bw, bh) = (u32::from(bw), u32::from(bh));
        let bytes_per_block = self.format.bytes_per_pixel() as usize;
        let row_bytes = (self.tile_dimensions[0] / bw) as usize * bytes_per_block;
        let rows = (self.tile_dimensions[1] / bh) as usize;
        let target_row = row_bytes * self.tile_counts[0] as usize;
        let target_plane = target_row * rows * self.tile_counts[1] as usize;
        let x = (mapping.texel_origin[0] / bw) as usize * bytes_per_block;
        let y = (mapping.texel_origin[1] / bh) as usize;
        for z in 0..self.tile_dimensions[2] as usize {
            for row in 0..rows {
                let dest = (mapping.texel_origin[2] as usize + z) * target_plane
                    + (y + row) * target_row
                    + x;
                let src = (z * rows + row) * row_bytes;
                target[dest..dest + row_bytes].copy_from_slice(&texture.data[src..src + row_bytes]);
            }
        }
        Ok(Some(mapping))
    }

    fn validate_texture<O: ByteOrder>(&self, texture: &TextureData<O>) -> Result<()> {
        ensure!(
            texture.head.kind == ETextureType::D3
                && texture.head.format == self.format
                && [texture.head.width, texture.head.height, texture.head.layers]
                    == self.tile_dimensions
                && texture.head.mip_sizes.len() == 1
                && texture.data.len() as u64 == self.bytes_per_tile,
            "Probe tile texture does not match its layer layout"
        );
        Ok(())
    }

    /// Native PointInside is inclusive. Out-of-volume tiles produce no mapping.
    pub fn map_tile(&self, position: [i32; 3]) -> Result<Option<ProbeTileMapping>> {
        if (0..3).any(|i| position[i] < self.min_tile[i] || position[i] > self.max_tile[i]) {
            return Ok(None);
        }
        let tile =
            std::array::from_fn(|i| (i64::from(position[i]) - i64::from(self.min_tile[i])) as u32);
        let texel_origin = std::array::from_fn(|i| tile[i] * self.tile_dimensions[i]);
        let linear = u64::from(tile[2])
            .checked_mul(u64::from(self.tile_counts[1]))
            .and_then(|v| v.checked_add(u64::from(tile[1])))
            .and_then(|v| v.checked_mul(u64::from(self.tile_counts[0])))
            .and_then(|v| v.checked_add(u64::from(tile[0])))
            .context("Probe tile index overflow")?;
        Ok(Some(ProbeTileMapping {
            tile,
            texel_origin,
            virtual_byte_offset: linear
                .checked_mul(self.bytes_per_tile)
                .context("Probe page offset overflow")?,
            byte_len: self.bytes_per_tile,
        }))
    }
}

// FRECPE followed by two FRECPS/FMUL refinements at Initialize71001c8cbc.
// Restricted to positive u32 dimensions converted to f32, in [1, 2^32].
// The estimate's nine-bit significand uses the midpoint of an eight-bit bin.
fn dimension_reciprocal(dimension: u32) -> f32 {
    debug_assert!(dimension > 0);
    let value = dimension as f32;
    let raw = value.to_bits();
    let q = 256 + ((raw & 0x7fffff) >> 15);
    let divisor = 2 * q + 1;
    let rounded = (262144 + divisor / 2) / divisor;
    let mut reciprocal = f32::from_bits(((253 - (raw >> 23)) << 23) | ((rounded - 256) << 15));
    for _ in 0..2 {
        reciprocal *= (-value).mul_add(reciprocal, 2.0);
    }
    reciprocal
}

fn next_down(value: f32) -> f32 {
    // Initialize asks nextafterf(value, -FLT_MAX), rather than -infinity.
    if value == f32::MIN {
        return value;
    }
    if value == 0.0 {
        return f32::from_bits(0x80000001);
    }
    f32::from_bits(if value > 0.0 { value.to_bits() - 1 } else { value.to_bits() + 1 })
}

// Native GetInverse7100070de4/GetInverseTranspose71000706ac share these
// unfused cofactors and determinant order.
fn linear_cofactors(rows: [[f32; 4]; 3]) -> (f32, [[f32; 3]; 3]) {
    let [[a, b, c, _], [d, e, f, _], [g, h, i, _]] = rows;
    let first = [e * i - f * h, f * g - i * d, h * d - e * g];
    let determinant = c * first[2] + (a * first[0] + b * first[1]);
    (determinant, [first, [h * c - i * b, a * i - g * c, b * g - a * h], [
        f * b - e * c,
        d * c - a * f,
        a * e - b * d,
    ]])
}

// GetInverse7100070de4: translation expands cofactors with the original
// multiplication/addition grouping; -inverse(rotation)*translation can round
// differently. GetInverseTranspose71000706ac uses linear_cofactors directly.
fn inverse(rows: [[f32; 4]; 3]) -> [[f32; 4]; 3] {
    let [[a, b, c, x], [d, e, f, y], [g, h, i, z]] = rows;
    let (determinant, cofactors) = linear_cofactors(rows);
    let r = 1.0 / determinant;
    let translation = [
        (((e * c) * z + (i * (b * y) - (f * b) * z) - i * (e * x)) - (h * c) * y) + f * (h * x),
        ((g * c) * y + (i * (d * x) + ((a * f) * z - i * (a * y) - (d * c) * z))) - f * (g * x),
        e * (g * x) + ((((b * d) * z + (h * (a * y) - (a * e) * z)) - h * (d * x)) - (b * g) * y),
    ];
    std::array::from_fn(|row| {
        [cofactors[0][row] * r, cofactors[1][row] * r, cofactors[2][row] * r, r * translation[row]]
    })
}

// Native CTransform4f::operator*: FMUL, two FMLAs, then translation FADD.
fn multiply(left: [[f32; 4]; 3], right: [[f32; 4]; 3]) -> [[f32; 4]; 3] {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let first = right[0][j] * left[i][0];
            let second = right[1][j].mul_add(left[i][1], first);
            let third = right[2][j].mul_add(left[i][2], second);
            (if j == 3 { left[i][3] } else { 0.0 }) + third
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_direction_matrix_ignores_volume_scale_and_preserves_cutoff() -> Result<()> {
        let volume = [[0.125, 0.0, 0.0, 0.5], [0.0, 0.25, 0.0, 0.5], [0.0, 0.0, 0.5, 0.5]];
        let mut camera = [[1.0, 0.0, 0.0, 5.0], [0.0, 1.0, 0.0, 7.0], [0.0, 0.0, 1.0, 9.0]];
        let transforms = BakedProbeTransforms::from_camera(volume, camera)?;
        assert_eq!(transforms.view_to_probe_inverse_transpose_rows, [
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0]
        ]);
        camera[0][0] = 134217728.0; // Native view-to-world determinant is 2^-27.
        assert_eq!(
            BakedProbeTransforms::from_camera(volume, camera)?.view_to_probe_inverse_transpose_rows,
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
        );
        Ok(())
    }

    #[test]
    fn reciprocal_preserves_native_refinement_rounding() {
        // Captured Initialize outputs; ordinary division rounds one ULP higher.
        assert_eq!(dimension_reciprocal(208).to_bits(), 0x3b9d89d8);
        assert_eq!(dimension_reciprocal(960).to_bits(), 0x3a888888);
    }

    #[test]
    fn negative_boundary_uses_floor_before_tile_alignment() -> Result<()> {
        let layout = ProbeVolumeLayout::from_bounds(
            CAABox { min: [-109.0, -53.0, -549.0].into(), max: [-29.0, 27.0, -481.0].into() },
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            0.5,
        )?;
        // Native output for real resource4ff0af76: truncation incorrectly gives64 Z probes.
        ensure!(layout.grid_origin == [-128, -64, -288]);
        ensure!(layout.dimensions == [128, 128, 48]);
        let first = layout.layer(0)?;
        ensure!(first.min_tile == [-2, -1, -18] && first.max_tile == [-1, 0, -16]);
        let last = first.map_tile([-1, 0, -16])?.unwrap();
        ensure!(last.tile == [1, 1, 2] && last.texel_origin == [64, 64, 32]);
        ensure!(last.virtual_byte_offset == 11 * 65536);
        ensure!(first.map_tile([-3, -1, -18])?.is_none());
        ensure!(layout.layer(2)?.tile_counts == [1, 2, 3]);
        Ok(())
    }
}
