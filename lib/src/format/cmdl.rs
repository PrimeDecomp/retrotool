#![allow(unused)]

use std::{io::Cursor, marker::PhantomData};

use anyhow::{bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use uuid::Uuid;
use zerocopy::ByteOrder;

// Retain the existing model-facing paths while shared values live under material.
pub use super::material::data::{
    CLayeredTextureBaseData, CLayeredTextureData, CMaterialData, CMaterialDataInner,
    CMaterialTextureTokenData, CMaterialVariableDesc, ELayeredTextureBlend, EMaterialDataId,
    EMaterialDataType, EMaterialFlag, SMaterialType, STextureUsageInfo,
};
use crate::{
    format::{rfrm::FormDescriptor, slice_chunks, CAABox, CColor4f, CStringFixed, FourCC},
    util::compression::decompress_buffer,
};

// Cooked model
pub const K_FORM_CMDL: FourCC = FourCC(*b"CMDL");
pub const K_FORM_SMDL: FourCC = FourCC(*b"SMDL");
pub const K_FORM_WMDL: FourCC = FourCC(*b"WMDL");

// Model header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// World header
pub const K_CHUNK_WDHD: FourCC = FourCC(*b"WDHD");
// Skinned header
pub const K_CHUNK_SKHD: FourCC = FourCC(*b"SKHD");
// Material data
pub const K_CHUNK_MTRL: FourCC = FourCC(*b"MTRL");
// Mesh data
pub const K_CHUNK_MESH: FourCC = FourCC(*b"MESH");
// Vertex buffer
pub const K_CHUNK_VBUF: FourCC = FourCC(*b"VBUF");
// Index buffer
pub const K_CHUNK_IBUF: FourCC = FourCC(*b"IBUF");
// GPU data
pub const K_CHUNK_GPU: FourCC = FourCC(*b"GPU ");

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelReadBufferInfo {
    pub size: u32,
    pub offset: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelBufferInfo {
    pub read_index: u32,
    pub offset: u32,
    pub size: u32,
    pub dest_size: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelMetaData {
    pub unk: u32,
    pub gpu_offset: u32,
    #[bw(try_calc = read_info.len().try_into())]
    pub read_info_count: u32,
    #[br(count = read_info_count)]
    pub read_info: Vec<SModelReadBufferInfo>,
    #[bw(try_calc = vtx_buffer_info.len().try_into())]
    pub vtx_buffer_count: u32,
    #[br(count = vtx_buffer_count)]
    pub vtx_buffer_info: Vec<SModelBufferInfo>,
    #[bw(try_calc = idx_buffer_info.len().try_into())]
    pub idx_info_count: u32,
    #[br(count = idx_info_count)]
    pub idx_buffer_info: Vec<SModelBufferInfo>,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug)]
pub enum EBufferType {
    U8 = 0, // unused, assumed
    U16 = 1,
    U32 = 2,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SIndexBufferInfoSection {
    #[bw(try_calc = info.len().try_into())]
    pub count: u32,
    #[br(count = count)]
    pub info: Vec<EBufferType>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexBufferInfoSection {
    #[bw(try_calc = info.len().try_into())]
    pub count: u32,
    #[br(count = count)]
    pub info: Vec<SVertexBufferInfo>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexBufferInfo {
    pub vertex_count: u32,
    #[bw(try_calc = components.len().try_into())]
    pub component_count: u32,
    #[br(count = component_count)]
    pub components: Vec<SVertexDataComponent>,
    pub num_buffers: u8, // correct?
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexDataComponent {
    pub buffer_index: u32,
    pub offset: u32,
    pub stride: u32,
    pub format: EVertexDataFormat,
    pub component: EVertexComponent,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EVertexDataFormat {
    Unknown = u32::MAX,
    R8Unorm = 0,
    R8Uint = 1,
    R8Snorm = 2,
    R8Sint = 3,
    R16Unorm = 4,
    R16Uint = 5,
    R16Snorm = 6,
    R16Sint = 7,
    R16Float = 8,
    Rg8Unorm = 9,
    Rg8Uint = 10,
    Rg8Snorm = 11,
    Rg8Sint = 12,
    R32Uint = 13,
    R32Sint = 14,
    R32Float = 15,
    Rg16Unorm = 16,
    Rg16Uint = 17,
    Rg16Snorm = 18,
    Rg16Sint = 19,
    Rg16Float = 20,
    Rgba8Unorm = 21,
    Rgba8Uint = 22,
    Rgba8Snorm = 23,
    Rgba8Sint = 24,
    Rgb10a2Unorm = 25,
    Rgb10a2Uint = 26,
    Rg32Uint = 27,
    Rg32Sint = 28,
    Rg32Float = 29,
    Rgba16Unorm = 30,
    Rgba16Uint = 31,
    Rgba16Snorm = 32,
    Rgba16Sint = 33,
    Rgba16Float = 34,
    Rgb32Uint = 35,
    Rgb32Sint = 36,
    Rgb32Float = 37,
    Rgba32Uint = 38,
    Rgba32Sint = 39,
    Rgba32Float = 40,
}

impl EVertexDataFormat {
    pub fn byte_size(self) -> u32 {
        match self {
            EVertexDataFormat::Unknown => 0,
            EVertexDataFormat::R8Unorm => 1,
            EVertexDataFormat::R8Uint => 1,
            EVertexDataFormat::R8Snorm => 1,
            EVertexDataFormat::R8Sint => 1,
            EVertexDataFormat::R16Unorm => 2,
            EVertexDataFormat::R16Uint => 2,
            EVertexDataFormat::R16Snorm => 2,
            EVertexDataFormat::R16Sint => 2,
            EVertexDataFormat::R16Float => 2,
            EVertexDataFormat::Rg8Unorm => 2,
            EVertexDataFormat::Rg8Uint => 2,
            EVertexDataFormat::Rg8Snorm => 2,
            EVertexDataFormat::Rg8Sint => 2,
            EVertexDataFormat::R32Uint => 4,
            EVertexDataFormat::R32Sint => 4,
            EVertexDataFormat::R32Float => 4,
            EVertexDataFormat::Rg16Unorm => 4,
            EVertexDataFormat::Rg16Uint => 4,
            EVertexDataFormat::Rg16Snorm => 4,
            EVertexDataFormat::Rg16Sint => 4,
            EVertexDataFormat::Rg16Float => 4,
            EVertexDataFormat::Rgba8Unorm => 4,
            EVertexDataFormat::Rgba8Uint => 4,
            EVertexDataFormat::Rgba8Snorm => 4,
            EVertexDataFormat::Rgba8Sint => 4,
            EVertexDataFormat::Rgb10a2Unorm => 4,
            EVertexDataFormat::Rgb10a2Uint => 4,
            EVertexDataFormat::Rg32Uint => 8,
            EVertexDataFormat::Rg32Sint => 8,
            EVertexDataFormat::Rg32Float => 8,
            EVertexDataFormat::Rgba16Unorm => 8,
            EVertexDataFormat::Rgba16Uint => 8,
            EVertexDataFormat::Rgba16Snorm => 8,
            EVertexDataFormat::Rgba16Sint => 8,
            EVertexDataFormat::Rgba16Float => 8,
            EVertexDataFormat::Rgb32Uint => 12,
            EVertexDataFormat::Rgb32Sint => 12,
            EVertexDataFormat::Rgb32Float => 12,
            EVertexDataFormat::Rgba32Uint => 16,
            EVertexDataFormat::Rgba32Sint => 16,
            EVertexDataFormat::Rgba32Float => 16,
        }
    }

    pub fn normalized(self) -> bool {
        matches!(
            self,
            EVertexDataFormat::R8Unorm
                | EVertexDataFormat::R8Snorm
                | EVertexDataFormat::R16Unorm
                | EVertexDataFormat::R16Snorm
                | EVertexDataFormat::Rg8Unorm
                | EVertexDataFormat::Rg8Snorm
                | EVertexDataFormat::Rg16Unorm
                | EVertexDataFormat::Rg16Snorm
                | EVertexDataFormat::Rgba8Unorm
                | EVertexDataFormat::Rgba8Snorm
                | EVertexDataFormat::Rgb10a2Unorm
                | EVertexDataFormat::Rgba16Unorm
                | EVertexDataFormat::Rgba16Snorm
        )
    }
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EVertexComponent {
    Position = 0,                           // in_position
    Normal = 1,                             // in_normal
    Tangent0 = 2,                           // in_tangent[0]
    Tangent1 = 3,                           // in_tangent[1]
    Tangent2 = 4,                           // in_tangent[2]
    TexCoord0 = 5,                          // in_texCoord[0]
    TexCoord1 = 6,                          // in_texCoord[1]
    TexCoord2 = 7,                          // in_texCoord[2]
    TexCoord3 = 8,                          // in_texCoord[3]
    Color = 9,                              // in_color
    BoneIndices = 10,                       // in_boneIndices
    BoneWeights = 11,                       // in_boneWeights
    BakedLightingCoord = 12,                // in_bakedLightingCoord
    BakedLightingTangent = 13,              // in_bakedLightingTangent
    VertInstanceParams = 14,                // in_vertInstanceParams
    VertInstanceColor = 15,                 // in_vertInstanceColor
    VertTransform0 = 16,                    // in_vertTransform[0]
    VertTransform1 = 17,                    // in_vertTransform[1]
    VertTransform2 = 18,                    // in_vertTransform[2]
    CurrentPosition = 19,                   // in_currentPosition
    VertInstanceOpacityParams = 20,         // in_vertInstanceOpacityParams
    VertInstanceColorIndexingParams = 21,   // in_vertInstanceColorIndexingParams
    VertInstanceOpacityIndexingParams = 22, // in_vertInstanceOpacityIndexingParams
    VertInstancePaintParams = 23,           // in_vertInstancePaintParams
    BakedLightingLookup = 24,               // in_bakedLightingLookup
    MaterialChoice0 = 25,                   // in_materialChoice[0]
    MaterialChoice1 = 26,                   // in_materialChoice[1]
    MaterialChoice2 = 27,                   // in_materialChoice[2]
    MaterialChoice3 = 28,                   // in_materialChoice[3]
}

#[binrw]
#[derive(Clone, Debug)]
// name?
pub struct SLodInfoInner {
    pub offset: u32,
    pub count: u32,
}

#[binrw]
#[derive(Clone, Debug)]
// name?
pub struct SLodInfoOuter {
    pub inner: [SLodInfoInner; 5],
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SRenderModelLODRule {
    pub value: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SMeshLoadInformation {
    #[bw(try_calc = meshes.len().try_into())]
    pub mesh_count: u32,
    #[br(count = mesh_count)]
    pub meshes: Vec<CRenderMesh>,
    /// Four EMeshClass values per byte, least-significant pair first.
    #[br(count = mesh_count.div_ceil(4))]
    pub mesh_classes: Vec<u8>,
    /// Eight two-sided flags per byte, least-significant bit first.
    #[br(count = mesh_count.div_ceil(8))]
    pub two_sided_flags: Vec<u8>,
    #[bw(try_calc = shorts.len().try_into())]
    pub short_count: u32,
    #[br(count = short_count)]
    pub shorts: Vec<u16>,
    // #[bw(try_calc = lod_info.len().try_into())]
    pub lod_count: u8,
    #[br(count = lod_count as usize)]
    pub lod_info: Vec<SLodInfoOuter>,
    #[bw(calc = if lod_rules.is_empty() { 0 } else { 1 })]
    pub has_lod_rules: u32,
    #[br(if(has_lod_rules == 1), count(lod_count as usize))]
    pub lod_rules: Vec<SRenderModelLODRule>,
}

impl SMeshLoadInformation {
    /// CModelRenderableDrawHelper::SetupStateForMesh_Unsorted, 0x71001f8870.
    pub fn mesh_class(&self, index: usize) -> Option<u8> {
        if index >= self.meshes.len() {
            return None;
        }
        self.mesh_classes.get(index / 4).map(|b| (b >> ((index % 4) * 2)) & 3)
    }

    /// The draw helper disables face culling for this mesh when the bit is set.
    pub fn is_two_sided(&self, index: usize) -> Option<bool> {
        if index >= self.meshes.len() {
            return None;
        }
        self.two_sided_flags.get(index / 8).map(|b| (b >> (index % 8)) & 1 != 0)
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CRenderMesh {
    pub material_idx: u16,
    pub vtx_buf_idx: u8,
    pub idx_buf_idx: u8,
    pub index_start: u32,
    pub index_count: u32,
    pub unk_c: u16,
    pub unk_e: u16, // 64
}

// #[binrw]
// #[derive(Clone, Debug)]
// pub struct DataSourceLoader {
// }

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelHeader {
    pub unk: u32,
    pub bounds: CAABox,
    /// Preserve the data-source count, records and optional custom-data tail.
    /// Their formats differ by model kind; this is not a new length-prefixed blob.
    #[br(parse_with = binrw::until_eof)]
    pub data_sources: Vec<u8>,
}

/// SMDL SKHD, loaded by NModelFormat::SSkinnedModelHeader at71001c370c.
/// HEAD remains the independent bounds/data-source header following SKHD.
#[binrw]
#[derive(Clone, Debug)]
pub struct SSkinnedModelHeader {
    pub unknown0: u32,
    /// Passed through CRenderSkinnedModel+100 to CSkinningMaterialData's initial
    /// identity palette constructor (UpdateModelReadiness710022bf88).
    pub bone_count: u32,
    pub unknown8: u32,
    pub mesh_bit_count: u32,
    #[bw(try_calc = mesh_bits.len().try_into())]
    pub mesh_word_count: u32,
    #[br(count = mesh_word_count)]
    pub mesh_bits: Vec<u32>,
    #[bw(try_calc = mesh_visibility_groups.len().try_into())]
    pub mesh_data_count: u32,
    #[br(count = mesh_data_count)]
    pub mesh_visibility_groups: Vec<u8>,
}

impl SSkinnedModelHeader {
    /// Native per-instance ShouldRenderMesh71002ea810: an unmarked mesh is visible;
    /// a marked mesh tests a supplied visibility mask at its byte group &31.
    /// The native lookup truncates the mesh index to16 bits.
    pub fn mesh_visible(&self, index: usize, visibility_mask: u32) -> Result<bool> {
        let index = index & 0xffff;
        ensure!(index < self.mesh_bit_count as usize, "Skinned mesh index out of bounds");
        let word = self
            .mesh_bits
            .get(index / 32)
            .ok_or_else(|| anyhow::anyhow!("Truncated skinned mesh bit vector"))?;
        if word & (1 << (index & 31)) == 0 {
            return Ok(true);
        }
        let group = self
            .mesh_visibility_groups
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Missing skinned mesh visibility group"))?;
        Ok(visibility_mask & (1 << (group & 31)) != 0)
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SMaterialChunk {
    /// LoadMaterialCacheHeader runs once for shared data, then once per material.
    #[bw(try_calc = shared_data.len().try_into())]
    pub shared_data_count: u32,
    #[br(count = shared_data_count)]
    pub shared_data_types: Vec<SMaterialType>,
    #[br(count = shared_data_count)]
    pub shared_data: Vec<CMaterialData>,
    #[bw(try_calc = materials.len().try_into())]
    pub material_count: u32,
    #[br(count = material_count)]
    pub materials: Vec<CMaterialCache>,
}

impl SMaterialChunk {
    /// Shared color provider created by LoadMaterialCache (0x7100348fe8), after
    /// the serialized shared values, when any material has the BKLT trait.
    /// Bounds use the model's native Y axis, before viewer coordinate conversion.
    pub fn backlight_bounds(&self, bounds: &CAABox) -> Option<CMaterialData> {
        if !self.materials.iter().any(|m| m.traits.iter().any(|t| t.0 == *b"BKLT")) {
            return None;
        }
        let height = bounds.max.y - bounds.min.y;
        let reciprocal = if height <= f32::EPSILON { 1.0 } else { height.recip() };
        Some(CMaterialData {
            data_id: EMaterialDataId::BKLB,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color(CColor4f {
                r: bounds.min.y,
                g: bounds.max.y,
                b: reciprocal,
                a: -bounds.min.y * reciprocal,
            }),
        })
    }

    /// Static model data in lookup order: per-material values, then shared data.
    /// CRenderModel::SetupMaterialForRenderMesh appends these after draw-context
    /// providers. Duplicate IDs in the shared block cannot override local values.
    pub fn material_values(&self, index: usize) -> Result<Vec<&CMaterialData>> {
        let material = self
            .materials
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Invalid material index {index}"))?;
        let mut seen = [false; 116];
        Ok(material
            .data
            .iter()
            .chain(&self.shared_data)
            .filter(|data| {
                let first = !seen[data.data_id as usize];
                seen[data.data_id as usize] = true;
                first
            })
            .collect())
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CMaterialCache {
    #[br(try_map = CStringFixed::into_string)]
    #[bw(map = CStringFixed::from_string)]
    pub name: String,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    /// Cooked MTRL resource GUID (NMaterialFormat::build_tag).
    pub material_resource_id: Uuid,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    /// Material implementation GUID, matching the embedded archive's material ID.
    pub material_id: Uuid,
    /// EMaterialFlag bit mask supplied to CMaterialInstanceDataBuilder::Finalize.
    pub flags: u32,
    /// External variable binding-key material-set hash; runtime resource +0x48.
    pub material_set_hash: u32,
    #[bw(try_calc = traits.len().try_into())]
    pub trait_count: u32,
    #[br(count = trait_count)]
    pub traits: Vec<FourCC>,
    #[bw(try_calc = variable_descs.len().try_into())]
    pub variable_desc_count: u32,
    #[br(count = variable_desc_count)]
    pub variable_descs: Vec<CMaterialVariableDesc>,
    #[bw(try_calc = data_types.len().try_into())]
    pub data_count: u32,
    #[br(count = data_count)]
    pub data_types: Vec<SMaterialType>,
    #[br(count = data_count)]
    pub data: Vec<CMaterialData>,
}

impl CMaterialCache {
    /// CVariableBindingKey::Generate (0x71001fd318): high32 is material-set
    /// hash, low32 is the variable's FourCC in conventional display order.
    pub fn variable_binding_key(&self, name: FourCC) -> u64 {
        (u64::from(self.material_set_hash) << 32) | u64::from(u32::from_be_bytes(name.0))
    }

    /// CMaterialInstanceResource::GetVariableDesc(key), 0x71001fd0cc.
    pub fn variable_desc(&self, key: u64) -> Option<&CMaterialVariableDesc> {
        if key >> 32 != u64::from(self.material_set_hash) {
            return None;
        }
        self.variable_descs.iter().find(|d| u32::from_be_bytes(d.name.0) == key as u32)
    }
}

fn decompress_gpu_buffers(
    file_data: &[u8],
    read_info: &[SModelReadBufferInfo],
    buffer_info: &[SModelBufferInfo],
) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::with_capacity(buffer_info.len());
    for info in buffer_info {
        let read_info = &read_info[info.read_index as usize];
        let read_buffer =
            &file_data[read_info.offset as usize..(read_info.offset + read_info.size) as usize];
        let comp_buf = &read_buffer[info.offset as usize..(info.offset + info.size) as usize];
        let (_, buf) = decompress_buffer(comp_buf, info.dest_size as u64)?;
        out.push(buf.into_owned());
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ModelData<O: ByteOrder> {
    pub head: SModelHeader,
    pub skinned_header: Option<SSkinnedModelHeader>,
    pub mtrl: SMaterialChunk,
    pub mesh: SMeshLoadInformation,
    pub vbuf: SVertexBufferInfoSection,
    pub ibuf: SIndexBufferInfoSection,
    pub vtx_buffers: Vec<Vec<u8>>,
    pub idx_buffers: Vec<Vec<u8>>,
    _marker: PhantomData<O>,
}

impl<O> ModelData<O>
where O: ByteOrder + 'static
{
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (cmdl_desc, cmdl_data, _) = FormDescriptor::<O>::slice(data)?;
        match cmdl_desc.id {
            K_FORM_CMDL => {
                ensure!(cmdl_desc.reader_version.get() == 114);
                ensure!(cmdl_desc.writer_version.get() == 125);
            }
            K_FORM_SMDL => {
                ensure!(cmdl_desc.reader_version.get() == 127);
                ensure!(cmdl_desc.writer_version.get() == 133);
            }
            K_FORM_WMDL => {
                ensure!(cmdl_desc.reader_version.get() == 118);
                ensure!(cmdl_desc.writer_version.get() == 124);
            }
            id => bail!("Unknown FourCC {:?}", id),
        }

        let meta: SModelMetaData = Cursor::new(meta).read_type(Endian::Little)?;
        let vtx_buffers = decompress_gpu_buffers(data, &meta.read_info, &meta.vtx_buffer_info)?;
        let idx_buffers = decompress_gpu_buffers(data, &meta.read_info, &meta.idx_buffer_info)?;

        let mut head: Option<SModelHeader> = None;
        let mut skinned_header = None;
        let mut mtrl: Option<SMaterialChunk> = None;
        let mut mesh: Option<SMeshLoadInformation> = None;
        let mut vbuf: Option<SVertexBufferInfoSection> = None;
        let mut ibuf: Option<SIndexBufferInfoSection> = None;
        slice_chunks::<O, _, _>(
            cmdl_data,
            |desc, data| {
                match desc.id {
                    K_CHUNK_HEAD | K_CHUNK_WDHD => {
                        head = Some(Cursor::new(data).read_type(Endian::Little)?)
                    }
                    K_CHUNK_SKHD => {
                        let mut cursor = Cursor::new(data);
                        skinned_header =
                            Some(cursor.read_type::<SSkinnedModelHeader>(Endian::Little)?);
                        ensure!(cursor.position() as usize == data.len(), "Trailing SKHD data");
                    }
                    K_CHUNK_MTRL => mtrl = Some(Cursor::new(data).read_type(Endian::Little)?),
                    K_CHUNK_MESH => mesh = Some(Cursor::new(data).read_type(Endian::Little)?),
                    K_CHUNK_VBUF => vbuf = Some(Cursor::new(data).read_type(Endian::Little)?),
                    K_CHUNK_IBUF => ibuf = Some(Cursor::new(data).read_type(Endian::Little)?),
                    // GPU data decompressed via META
                    K_CHUNK_GPU => {}
                    id => bail!("Unknown {} chunk {id:?}", cmdl_desc.id),
                }
                Ok(())
            },
            |form, _data| bail!("Unknown {} form {:?}", cmdl_desc.id, form.id),
        )?;
        let Some(head) = head else { bail!("Failed to locate HEAD") };
        let Some(mtrl) = mtrl else { bail!("Failed to locate MTRL") };
        let Some(mesh) = mesh else { bail!("Failed to locate MESH") };
        let Some(vbuf) = vbuf else { bail!("Failed to locate VBUF") };
        let Some(ibuf) = ibuf else { bail!("Failed to locate IBUF") };

        // log::debug!("HEAD: {head:#?}");
        // log::debug!("MTRL: {mtrl:#?}");
        // log::debug!("MESH: {mesh:#?}");
        // log::debug!("VBUF: {vbuf:#?}");
        // log::debug!("IBUF: {ibuf:#?}");

        ensure!(cmdl_desc.id != K_FORM_SMDL || skinned_header.is_some(), "SMDL missing SKHD");
        Ok(Self {
            head,
            skinned_header,
            mtrl,
            mesh,
            vbuf,
            ibuf,
            vtx_buffers,
            idx_buffers,
            _marker: PhantomData,
        })
    }
}

#[cfg(test)]
mod material_tests {
    use binrw::BinWriterExt;

    #[test]
    fn packed_mesh_class_and_two_sided_fields_cross_byte_boundaries() {
        let mut bytes = 9u32.to_le_bytes().to_vec();
        bytes.extend([0; 9 * 16]); // Nine serialized CRenderMesh records.
        bytes.extend([0xe4, 0x1b, 0x02]);
        bytes.extend([0xa5, 0x01]);
        bytes.extend([0; 9]); // No mesh-index list, LODs, or LOD rules.
        let mut cursor = Cursor::new(bytes);
        let info: SMeshLoadInformation = cursor.read_le().unwrap();
        assert_eq!(cursor.position(), cursor.get_ref().len() as u64);
        assert_eq!((0..9).map(|i| info.mesh_class(i).unwrap()).collect::<Vec<_>>(), [
            0, 1, 2, 3, 3, 2, 1, 0, 2
        ]);
        assert_eq!((0..9).map(|i| info.is_two_sided(i).unwrap()).collect::<Vec<_>>(), [
            true, false, true, false, false, true, false, true, true
        ]);
        assert_eq!(info.mesh_class(9), None);
        assert_eq!(info.is_two_sided(9), None);
    }

    use super::*;
    use crate::format::material_predicates::{MaterialSelectionState, SelectionValue};

    fn word(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
    fn color(out: &mut Vec<u8>, id: &[u8; 4], value: f32) {
        out.extend_from_slice(id);
        out.extend_from_slice(b"COLR");
        for _ in 0..4 {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    fn fixture() -> Vec<u8> {
        let mut out = Vec::new();
        word(&mut out, 2); // Shared header: two declarations, then two values.
        out.extend_from_slice(b"DIFCCOLRBSAOCOLR");
        color(&mut out, b"DIFC", 0.25);
        color(&mut out, b"BSAO", 0.5);
        word(&mut out, 1); // One material.
        word(&mut out, 7);
        out.extend_from_slice(b"fixture");
        out.extend_from_slice(&[0; 32]); // Resource/implementation GUIDs.
        word(&mut out, 1 << 7);
        word(&mut out, 0x12345678);
        word(&mut out, 1);
        out.extend_from_slice(b"BKLT");
        word(&mut out, 1);
        out.extend_from_slice(b"TIMEBSAO");
        out.extend_from_slice(&[1, 3]);
        word(&mut out, 1);
        out.extend_from_slice(b"BSAOCOLR");
        color(&mut out, b"BSAO", 0.75);
        out
    }

    #[test]
    fn nonempty_shared_material_block_preserves_wire_order_and_local_precedence() {
        let raw = fixture();
        let mut reader = Cursor::new(&raw);
        let chunk: SMaterialChunk = reader.read_le().unwrap();
        assert_eq!(reader.position(), raw.len() as u64);
        assert_eq!(chunk.shared_data.len(), 2);
        let values = chunk.material_values(0).unwrap();
        assert_eq!(values.iter().map(|d| d.data_id).collect::<Vec<_>>(), [
            EMaterialDataId::BSAO,
            EMaterialDataId::DIFC
        ]);
        let CMaterialDataInner::Color(value) = &values[0].data else { panic!() };
        assert_eq!(value.r, 0.75);
        let mut writer = Cursor::new(Vec::new());
        writer.write_le(&chunk).unwrap();
        assert_eq!(writer.into_inner(), raw);
        assert!(chunk.material_values(1).is_err());
    }

    #[test]
    fn backlight_bounds_use_native_y_and_handle_flat_models() {
        let mut chunk: SMaterialChunk = Cursor::new(fixture()).read_le().unwrap();
        for (min, max, expected) in [
            (-2.0, 6.0, [-2.0, 6.0, 0.125, 0.25]),
            (4.0, 4.0, [4.0, 4.0, 1.0, -4.0]),
            (0.0, f32::EPSILON, [0.0, f32::EPSILON, 1.0, -0.0]),
            (0.0, 2.0 * f32::EPSILON, [0.0, 2.0 * f32::EPSILON, 4194304.0, -0.0]),
        ] {
            let bounds = CAABox {
                min: crate::format::CVector3f { x: -100.0, y: min, z: -50.0 },
                max: crate::format::CVector3f { x: 100.0, y: max, z: 50.0 },
            };
            let data = chunk.backlight_bounds(&bounds).unwrap();
            assert_eq!(data.data_id, EMaterialDataId::BKLB);
            let CMaterialDataInner::Color(color) = data.data else { panic!() };
            assert_eq!(color.to_array(), expected);
        }
        chunk.materials[0].traits.clear();
        assert!(chunk.backlight_bounds(&CAABox::default()).is_none());
    }

    #[test]
    fn external_variable_keys_use_set_hash_and_fourcc_byte_order() {
        let chunk: SMaterialChunk = Cursor::new(fixture()).read_le().unwrap();
        let material = &chunk.materials[0];
        let key = material.variable_binding_key(FourCC(*b"TIME"));
        assert_eq!(key, 0x12345678_54494d45);
        let desc = material.variable_desc(key).unwrap();
        assert_eq!(desc.data_id, FourCC(*b"BSAO"));
        assert_eq!((desc.data_type, desc.packing_rule), (1, 3));
        assert!(material.variable_desc(key ^ (1 << 32)).is_none());
        assert!(material.variable_desc(key ^ 1).is_none());
    }

    #[test]
    fn standalone_nil_texture_stores_usage_but_layer_slot_omits_it() {
        let mut layer = Cursor::new(vec![0; 16]);
        let token: CMaterialTextureTokenData = layer.read_le().unwrap();
        assert!(token.usage.is_none());
        assert_eq!(layer.position(), 16);
        let mut raw = b"BCLRTXTR".to_vec();
        raw.extend_from_slice(&[0; 16]);
        word(&mut raw, 2);
        for _ in 0..4 {
            word(&mut raw, u32::MAX);
        }
        let mut reader = Cursor::new(&raw);
        let data: CMaterialData = reader.read_le().unwrap();
        assert_eq!(reader.position(), 44);
        let CMaterialDataInner::Texture(token) = &data.data else { panic!() };
        assert!(token.id.is_nil());
        assert_eq!(token.usage.as_ref().unwrap().tex_coord, 2);
        let mut writer = Cursor::new(Vec::new());
        writer.write_le(&data).unwrap();
        assert_eq!(writer.into_inner(), raw);
    }

    #[test]
    fn model_selection_state_preserves_flags_shared_data_and_generated_bounds_presence() {
        let chunk: SMaterialChunk = Cursor::new(fixture()).read_le().unwrap();
        let model =
            MaterialSelectionState::from_model_material(&chunk, 0, |_| unreachable!()).unwrap();
        assert_eq!(model.flags, 1 << 7);
        assert!(model.contains(EMaterialDataId::DIFC));
        assert!(model.contains(EMaterialDataId::BSAO));
        assert!(model.contains(EMaterialDataId::BKLB));
        assert_eq!(model.select_definition("MFIN", &[], 3).unwrap(), Some(1));
        let mut context = MaterialSelectionState::default();
        context.flags = 1;
        context.set(EMaterialDataId::PMOD, SelectionValue::Int(3));
        context.append_fallback(&model);
        assert_eq!(context.flags, 129);
        assert_eq!(context.select_definition("PMOD", &["PMOD"], 15).unwrap(), Some(3));
    }
}
