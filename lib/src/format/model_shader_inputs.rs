//! Model vertex data supplied to the generic Maxwell shader wrapper.
//!
//! Reflection maps model components to hardware attribute locations. Each
//! location occupies four words beginning at0x80+16*location. Preserve all stored
//! components; UV swizzles and color transforms belong to the original shader.
//! Absent consumed components are rejected instead of assuming fetch defaults.

use anyhow::{bail, ensure, Context, Result};
use half::f16;
use zerocopy::LittleEndian;

use super::{
    cmdl::{EBufferType, EVertexDataFormat, ModelData},
    shader_nvn::VertexLayoutEntry,
};

/// Decode the stored components into raw shader-register words. Normalized
/// values become f32, integer formats retain integer bits, and f32 retains its
/// original bits. This does not manufacture absent vector components.
pub fn vertex_component_words(format: EVertexDataFormat, bytes: &[u8]) -> Result<Vec<u32>> {
    use EVertexDataFormat::*;
    ensure!(bytes.len() == format.byte_size() as usize, "Vertex component byte length mismatch");
    #[derive(Clone, Copy)]
    enum Kind {
        Uint,
        Sint,
        Unorm,
        Snorm,
        Float,
    }
    let (kind, size, count) = match format {
        R8Unorm => (Kind::Unorm, 1, 1),
        R8Uint => (Kind::Uint, 1, 1),
        R8Snorm => (Kind::Snorm, 1, 1),
        R8Sint => (Kind::Sint, 1, 1),
        R16Unorm => (Kind::Unorm, 2, 1),
        R16Uint => (Kind::Uint, 2, 1),
        R16Snorm => (Kind::Snorm, 2, 1),
        R16Sint => (Kind::Sint, 2, 1),
        R16Float => (Kind::Float, 2, 1),
        Rg8Unorm => (Kind::Unorm, 1, 2),
        Rg8Uint => (Kind::Uint, 1, 2),
        Rg8Snorm => (Kind::Snorm, 1, 2),
        Rg8Sint => (Kind::Sint, 1, 2),
        R32Uint => (Kind::Uint, 4, 1),
        R32Sint => (Kind::Sint, 4, 1),
        R32Float => (Kind::Float, 4, 1),
        Rg16Unorm => (Kind::Unorm, 2, 2),
        Rg16Uint => (Kind::Uint, 2, 2),
        Rg16Snorm => (Kind::Snorm, 2, 2),
        Rg16Sint => (Kind::Sint, 2, 2),
        Rg16Float => (Kind::Float, 2, 2),
        Rgba8Unorm => (Kind::Unorm, 1, 4),
        Rgba8Uint => (Kind::Uint, 1, 4),
        Rgba8Snorm => (Kind::Snorm, 1, 4),
        Rgba8Sint => (Kind::Sint, 1, 4),
        Rg32Uint => (Kind::Uint, 4, 2),
        Rg32Sint => (Kind::Sint, 4, 2),
        Rg32Float => (Kind::Float, 4, 2),
        Rgba16Unorm => (Kind::Unorm, 2, 4),
        Rgba16Uint => (Kind::Uint, 2, 4),
        Rgba16Snorm => (Kind::Snorm, 2, 4),
        Rgba16Sint => (Kind::Sint, 2, 4),
        Rgba16Float => (Kind::Float, 2, 4),
        Rgb32Uint => (Kind::Uint, 4, 3),
        Rgb32Sint => (Kind::Sint, 4, 3),
        Rgb32Float => (Kind::Float, 4, 3),
        Rgba32Uint => (Kind::Uint, 4, 4),
        Rgba32Sint => (Kind::Sint, 4, 4),
        Rgba32Float => (Kind::Float, 4, 4),
        Rgb10a2Unorm | Rgb10a2Uint => {
            let word = u32::from_le_bytes(bytes.try_into()?);
            let values = [word & 1023, (word >> 10) & 1023, (word >> 20) & 1023, word >> 30];
            return Ok(values
                .into_iter()
                .enumerate()
                .map(|(i, v)| {
                    if matches!(format, Rgb10a2Unorm) {
                        (v as f32 / if i == 3 { 3.0 } else { 1023.0 }).to_bits()
                    } else {
                        v
                    }
                })
                .collect());
        }
        Unknown => bail!("Unknown model vertex format"),
    };
    let mut out = Vec::with_capacity(count);
    for bytes in bytes.chunks_exact(size) {
        let raw = match size {
            1 => u32::from(bytes[0]),
            2 => u32::from(u16::from_le_bytes(bytes.try_into()?)),
            4 => u32::from_le_bytes(bytes.try_into()?),
            _ => unreachable!(),
        };
        let signed = match size {
            1 => raw as i8 as i32,
            2 => raw as i16 as i32,
            _ => raw as i32,
        };
        out.push(match kind {
            Kind::Uint => raw,
            Kind::Sint => signed as u32,
            Kind::Float if size == 2 => f16::from_bits(raw as u16).to_f32().to_bits(),
            Kind::Float => raw,
            Kind::Unorm => (raw as f32 / ((1u32 << (size * 8)) - 1) as f32).to_bits(),
            Kind::Snorm => {
                (signed as f32 / ((1u32 << (size * 8 - 1)) - 1) as f32).max(-1.0).to_bits()
            }
        });
    }
    Ok(out)
}

pub fn model_vertex_inputs(
    model: &ModelData<LittleEndian>,
    vertex_buffer: usize,
    layout: &[VertexLayoutEntry],
    required: &[u16],
) -> Result<Vec<[u32; 256]>> {
    let info = model.vbuf.info.get(vertex_buffer).context("Model vertex buffer missing")?;
    let first_buffer =
        model.vbuf.info[..vertex_buffer].iter().map(|i| usize::from(i.num_buffers)).sum::<usize>();
    let mut out = vec![[0; 256]; info.vertex_count as usize];
    let mut supplied = [false; 256];
    for entry in layout {
        ensure!((0..32).contains(&entry.location), "Unsupported vertex input location");
        let start = 32 + entry.location as usize * 4;
        if !required.iter().any(|&byte| (start * 4..(start + 4) * 4).contains(&(byte as usize))) {
            continue;
        }
        let component = info
            .components
            .iter()
            .find(|c| c.component as u32 == entry.component)
            .with_context(|| format!("Missing model vertex component {}", entry.component))?;
        ensure!(
            component.buffer_index < u32::from(info.num_buffers),
            "Vertex component buffer outside group"
        );
        let input = model
            .vtx_buffers
            .get(first_buffer + component.buffer_index as usize)
            .context("Vertex data missing")?;
        let size = component.format.byte_size() as usize;
        ensure!(size != 0 && component.stride != 0, "Invalid vertex format/stride");
        for (i, vertex) in out.iter_mut().enumerate() {
            let offset = i
                .checked_mul(component.stride as usize)
                .and_then(|n| n.checked_add(component.offset as usize))
                .context("Vertex address overflow")?;
            let bytes = input
                .get(offset..offset.checked_add(size).context("Vertex span overflow")?)
                .context("Truncated vertex data")?;
            let words = vertex_component_words(component.format, bytes)?;
            vertex[start..start + words.len()].copy_from_slice(&words);
            supplied[start..start + words.len()].fill(true);
        }
    }
    for &byte in required {
        ensure!(
            byte % 4 == 0 && supplied.get(byte as usize / 4) == Some(&true),
            "Missing consumed vertex word {byte:#x}"
        );
    }
    Ok(out)
}

pub fn model_mesh_indices(model: &ModelData<LittleEndian>, mesh: usize) -> Result<Vec<u32>> {
    let mesh = model.mesh.meshes.get(mesh).context("Model mesh missing")?;
    let buffer = usize::from(mesh.idx_buf_idx);
    let kind = *model.ibuf.info.get(buffer).context("Index buffer format missing")?;
    let data = model.idx_buffers.get(buffer).context("Index buffer missing")?;
    let size = match kind {
        EBufferType::U8 => 1,
        EBufferType::U16 => 2,
        EBufferType::U32 => 4,
    };
    let start = (mesh.index_start as usize).checked_mul(size).context("Index start overflow")?;
    let length = (mesh.index_count as usize).checked_mul(size).context("Index count overflow")?;
    let data = data
        .get(start..start.checked_add(length).context("Index span overflow")?)
        .context("Truncated mesh indices")?;
    let count = model
        .vbuf
        .info
        .get(usize::from(mesh.vtx_buf_idx))
        .context("Vertex buffer missing")?
        .vertex_count;
    data.chunks_exact(size)
        .map(|bytes| {
            let index = match kind {
                EBufferType::U8 => u32::from(bytes[0]),
                EBufferType::U16 => u32::from(u16::from_le_bytes(bytes.try_into()?)),
                EBufferType::U32 => u32::from_le_bytes(bytes.try_into()?),
            };
            ensure!(index < count, "Mesh index exceeds vertex count");
            Ok(index)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_formats_keep_integer_and_float_semantics_distinct() {
        use EVertexDataFormat::*;
        assert_eq!(vertex_component_words(Rgba8Sint, &[0, 127, 128, 255]).unwrap(), [
            0, 127, 0xffffff80, 0xffffffff
        ]);
        assert_eq!(vertex_component_words(Rgba8Snorm, &[0, 127, 128, 255]).unwrap(), [
            0.0f32.to_bits(),
            1.0f32.to_bits(),
            (-1.0f32).to_bits(),
            (-1.0f32 / 127.0).to_bits()
        ]);
        assert_eq!(vertex_component_words(Rg16Float, &[0, 128, 0, 60]).unwrap(), [
            (-0.0f32).to_bits(),
            1.0f32.to_bits()
        ]);
        let packed = 0xc00807ffu32.to_le_bytes();
        assert_eq!(vertex_component_words(Rgb10a2Uint, &packed).unwrap(), [1023, 513, 0, 3]);
        assert_eq!(vertex_component_words(Rgb10a2Unorm, &packed).unwrap(), [
            1.0f32.to_bits(),
            (513.0f32 / 1023.0).to_bits(),
            0.0f32.to_bits(),
            1.0f32.to_bits()
        ]);
        let nan = 0x7fa12345u32.to_le_bytes();
        assert_eq!(vertex_component_words(R32Float, &nan).unwrap(), [0x7fa12345]);
        assert!(vertex_component_words(Rgb32Float, &[0; 11]).is_err());
        assert!(vertex_component_words(Unknown, &[]).is_err());
    }
}
