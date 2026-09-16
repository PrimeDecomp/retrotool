use std::{io::Cursor, marker::PhantomData};

use anyhow::{ensure, Context, Result};
use binrw::{binrw, BinReaderExt, Endian};
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor,
    material_archive::{MaterialArchive, ShaderIdentifier},
    material_features::{FeatureChoice, FeatureVariable, FeaturedMaterial, GraphicsState},
    material_predicates::MaterialSelectionState,
    material_runtime::RuntimeMaterial,
    rfrm::FormDescriptor,
    shader_nvn::NvnReflection,
    FourCC,
};

// Cooked material resource (v168); its embedded shader archive uses MTRL v22.
pub const K_FORM_MTRL: FourCC = FourCC(*b"MTRL");

#[binrw]
#[derive(Clone, Debug)]
struct SMaterialMetaData {
    count: u32,
    version: u32,
    compressed_size: u32,
    decompressed_size: u32,
    file_offset: u32,
}

#[derive(Debug, Clone)]
pub struct MaterialData<O: ByteOrder> {
    pub decompressed: Vec<u8>,
    _marker: PhantomData<O>,
}

#[derive(Debug)]
pub struct MaterialPayload<'a> {
    /// Original EXPT bytes, retained alongside the parsed runtime lookup.
    pub export_data: &'a [u8],
    pub runtime: RuntimeMaterial,
    pub archive: MaterialArchive<'a>,
    pub padding: &'a [u8],
}

/// One cooked NVN program selected from an explicit, already merged draw state.
/// This identifies bytecode and feature bindings; it does not translate shaders.
#[derive(Debug)]
pub struct SelectedNvnShader<'a> {
    pub material_name: &'a str,
    pub runtime_id: u64,
    pub identifier: ShaderIdentifier,
    pub shader_hash: u64,
    /// Source-array indices in vertex/pixel/geometry/compute order (-1 is absent).
    pub programs: [i32; 4],
    pub features: Vec<FeatureChoice<'a>>,
    pub variables: Vec<FeatureVariable<'a>>,
    pub technique_state: GraphicsState,
    pub override_state: GraphicsState,
}

impl SelectedNvnShader<'_> {
    /// ApplyShaderPermutation (0x710018a830): technique defaults, then the
    /// current draw state, then the selected permutation's conditional state.
    pub fn graphics_state(&self, bound: &GraphicsState) -> Result<GraphicsState> {
        let mut state = GraphicsState::default();
        state.merge(&self.technique_state)?;
        state.merge(bound)?;
        state.merge(&self.override_state)?;
        Ok(state)
    }
}

impl<'a> MaterialPayload<'a> {
    /// Missing techniques or failed option selection return None. Unknown
    /// predicates, unresolved payloads and inconsistent archive data are errors.
    pub fn select_nvn_shader(
        &self,
        code: u8,
        state: &MaterialSelectionState,
    ) -> Result<Option<SelectedNvnShader<'a>>> {
        let slot = FeaturedMaterial::technique_index(code).context("Unknown technique code")?;
        let backend = self
            .archive
            .backends
            .iter()
            .find(|b| b.id == FourCC(*b"SNVN"))
            .context("Material has no NVN backend")?;
        let material = backend
            .materials
            .iter()
            .find(|m| m.id == self.runtime.id && m.hash == self.runtime.hash)
            .context("EXPT material identity does not match NVN archive")?;
        let featured = FeaturedMaterial::parse(material.data)?;
        let Some(technique) = featured.techniques[slot].as_ref() else {
            return Ok(None);
        };
        let mask = *self
            .runtime
            .dynamic_feature_skip_masks
            .get(slot)
            .context("Missing EXPT dynamic-feature skip mask")?;
        let Some(runtime_id) = technique
            .features
            .select_dynamic_runtime_id(mask, |f, allowed| state.select_feature(f, allowed))?
        else {
            return Ok(None);
        };
        let permutation = self
            .runtime
            .find_permutation(slot, runtime_id)
            .context("Selected runtime ID is absent from EXPT")?;
        let identifier = ShaderIdentifier { technique: code, permutation };
        let shader = material
            .shaders
            .iter()
            .find(|s| s.identifier == identifier)
            .context("EXPT permutation is absent from NVN archive")?;
        let reflection = NvnReflection::parse(shader.metadata)?;
        let variables =
            technique.features.bind_variables(permutation, &reflection.featured_variables()?)?;
        for variable in &variables {
            reflection.variable(variable.packed)?;
        }
        Ok(Some(SelectedNvnShader {
            material_name: material.name,
            runtime_id,
            identifier,
            shader_hash: shader.hash,
            features: technique.features.selected_features(permutation)?,
            programs: [
                shader.vertex_program,
                shader.pixel_program,
                shader.geometry_program,
                shader.compute_program,
            ],
            variables,
            technique_state: technique.state.clone(),
            override_state: technique.override_state(permutation)?,
        }))
    }
}

impl<O: ByteOrder> MaterialData<O> {
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (mtrl_desc, _, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(mtrl_desc.id == K_FORM_MTRL);
        ensure!(mtrl_desc.reader_version.get() == 168);
        ensure!(mtrl_desc.writer_version.get() == 168);

        let meta: SMaterialMetaData = Cursor::new(meta).read_type(Endian::Little)?;
        ensure!(meta.version == 168, "Unsupported MTRL metadata version");
        ensure!(meta.count == 1, "Unsupported MTRL metadata count");
        let start = meta.file_offset as usize;
        let end = start
            .checked_add(meta.compressed_size as usize)
            .ok_or_else(|| anyhow::anyhow!("MTRL compressed range overflow"))?;
        let form_end =
            std::mem::size_of::<FormDescriptor<O>>() + usize::try_from(mtrl_desc.size.get())?;
        ensure!(
            start >= std::mem::size_of::<FormDescriptor<O>>() && end <= form_end,
            "MTRL compressed range is outside the resource"
        );
        let compressed = &data[start..end];
        let decompressed = crate::util::compression::decompress_zlib_exact(
            compressed,
            meta.decompressed_size as usize,
        )?;

        Ok(Self { decompressed, _marker: PhantomData })
    }

    /// The decompressed v168 payload is EXPT + RFRM/MTRL v22 + alignment padding.
    pub fn payload(&self) -> Result<MaterialPayload<'_>> {
        let (chunk, export_data, remain) = ChunkDescriptor::<O>::slice(&self.decompressed)?;
        ensure!(chunk.id == FourCC(*b"EXPT"), "Expected material EXPT chunk");
        ensure!(chunk.unk.get() == 1, "Unsupported EXPT version");
        let runtime = RuntimeMaterial::parse(export_data)?;
        let (archive, padding) = MaterialArchive::slice(remain)?;
        Ok(MaterialPayload { export_data, runtime, archive, padding })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::ZlibEncoder, Compression};
    use zerocopy::LittleEndian;

    use super::*;

    fn resource(raw: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut data = b"RFRM".to_vec();
        data.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(b"MTRL");
        data.extend_from_slice(&168u32.to_le_bytes());
        data.extend_from_slice(&168u32.to_le_bytes());
        data.extend_from_slice(&compressed);
        let meta = [1, 168, compressed.len() as u32, raw.len() as u32, 32]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        (data, meta)
    }

    #[test]
    fn validates_compressed_resource_size_range_and_checksum() {
        let (data, meta) = resource(b"synthetic material payload");
        assert_eq!(
            MaterialData::<LittleEndian>::slice(&data, &meta).unwrap().decompressed,
            b"synthetic material payload"
        );
        for (offset, value) in [(12, 1u32), (12, 1000), (16, 0), (16, u32::MAX)] {
            let mut invalid_meta = meta.clone();
            invalid_meta[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(MaterialData::<LittleEndian>::slice(&data, &invalid_meta).is_err());
        }
        let mut broken = data.clone();
        *broken.last_mut().unwrap() ^= 1;
        assert!(MaterialData::<LittleEndian>::slice(&broken, &meta).is_err());
        let mut truncated = data.clone();
        truncated.pop();
        let size = (truncated.len() - 32) as u64;
        truncated[4..12].copy_from_slice(&size.to_le_bytes());
        let mut truncated_meta = meta;
        truncated_meta[8..12].copy_from_slice(&(size as u32).to_le_bytes());
        assert!(MaterialData::<LittleEndian>::slice(&truncated, &truncated_meta).is_err());
    }
}
