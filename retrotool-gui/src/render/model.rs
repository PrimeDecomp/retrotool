use std::{borrow::Cow, ops::Range};

use anyhow::Result;
use bevy::{
    prelude::*,
    render::{
        mesh::{Indices, MeshVertexAttribute, VertexAttributeValues},
        primitives::Aabb,
    },
};
use bit_set::BitSet;
use half::prelude::*;
use retrolib::{
    array_ref,
    format::{
        cmdl::{
            CMaterialCache, EBufferType, EVertexComponent, EVertexDataFormat, ModelData,
            SVertexDataComponent,
        },
        CAABox, CTransform4f,
    },
};
use wgpu_types::PrimitiveTopology;

use crate::{
    loaders::model::ModelAsset,
    material::{
        ATTRIBUTE_TANGENT_1, ATTRIBUTE_TANGENT_2, ATTRIBUTE_UV_1, ATTRIBUTE_UV_2, ATTRIBUTE_UV_3,
    },
};

pub const MESH_FLAG_OPAQUE: u16 = 1;

pub struct BuiltMesh {
    pub mesh: Handle<Mesh>,
    pub material_idx: usize,
    pub visible: bool,
    pub flags: u16,
    pub unk_e: u16,
}

pub struct ModelLod {
    pub meshes: BitSet,
    pub distance: Option<f32>,
}

pub struct BuiltModel {
    pub meshes: Vec<BuiltMesh>,
    pub lod: Vec<ModelLod>,
    pub materials: Vec<CMaterialCache>,
    pub aabb: Aabb,
}

pub fn load_model(asset: &ModelAsset, meshes: &mut Assets<Mesh>) -> Result<BuiltModel> {
    let ModelAsset {
        inner: ModelData { head, mtrl, mesh, vbuf, ibuf, vtx_buffers, idx_buffers },
        ..
    } = asset;

    // Build vertex buffers
    let mut buf_infos: Vec<VertexBufferInfo> = Vec::with_capacity(vtx_buffers.len());
    let mut cur_buf = 0usize;
    for info in &vbuf.info {
        let num_buffers = info.num_buffers as usize;
        let mut attributes = Vec::with_capacity(info.components.len());
        for component in &info.components {
            let input = &*vtx_buffers[cur_buf + component.buffer_index as usize];
            if let Some((attribute, values)) = convert_component(input, component) {
                attributes.push((attribute, values));
            }
        }
        buf_infos.push(VertexBufferInfo { attributes });
        cur_buf += num_buffers;
    }

    // Process index buffers
    let mut index_buffers = Vec::<IndicesSlice>::new();
    for (idx, &index_type) in ibuf.info.iter().enumerate() {
        let in_buf = &*idx_buffers[idx];
        let out = match index_type {
            EBufferType::U8 => {
                IndicesSlice::U16(Cow::Owned(in_buf.iter().map(|&u| u as u16).collect()))
            }
            EBufferType::U16 => IndicesSlice::U16(Cow::Borrowed(bytemuck::cast_slice(in_buf))),
            EBufferType::U32 => IndicesSlice::U32(Cow::Borrowed(bytemuck::cast_slice(in_buf))),
        };
        index_buffers.push(out);
    }

    // Process meshes
    let mut out_meshes = vec![];
    for (_idx, in_mesh) in mesh.meshes.iter().enumerate() {
        let (indices, vert_range) = match &index_buffers[in_mesh.idx_buf_idx as usize] {
            IndicesSlice::U16(indices) => {
                let (values, range) =
                    slice_indices(indices, in_mesh.index_start, in_mesh.index_count);
                (Indices::U16(values), range)
            }
            IndicesSlice::U32(indices) => {
                let (values, range) =
                    slice_indices(indices, in_mesh.index_start, in_mesh.index_count);
                (Indices::U32(values), range)
            }
        };
        let mut out_mesh = Mesh::new(PrimitiveTopology::TriangleList);
        out_mesh.set_indices(Some(indices));
        for (component, values) in &buf_infos[in_mesh.vtx_buf_idx as usize].attributes {
            out_mesh
                .insert_attribute(component.clone(), slice_vertices(values, vert_range.clone()));
        }
        out_meshes.push(BuiltMesh {
            mesh: meshes.add(out_mesh),
            material_idx: in_mesh.material_idx as usize,
            visible: true,
            flags: in_mesh.unk_c,
            unk_e: in_mesh.unk_e,
        });
    }

    let mut lod = Vec::with_capacity(mesh.lod_count as usize);
    for (idx, outer) in mesh.lod_info.iter().enumerate() {
        let mut visible = BitSet::with_capacity(mesh.meshes.len());
        for inner in &outer.inner {
            for &idx in &mesh.shorts[inner.offset as usize..(inner.offset + inner.count) as usize] {
                visible.insert(idx as usize);
            }
        }
        lod.push(ModelLod { meshes: visible, distance: mesh.lod_rules.get(idx).map(|r| r.value) });
    }

    Ok(BuiltModel {
        meshes: out_meshes,
        lod,
        materials: mtrl.materials.clone(),
        aabb: convert_aabb(&head.bounds),
    })
}

#[inline]
pub fn convert_aabb(aabb: &CAABox) -> Aabb {
    let min = mint::Vector3::from(aabb.min);
    let max = mint::Vector3::from(aabb.max);
    Aabb::from_min_max(min.into(), max.into())
}

#[inline]
pub fn convert_transform(xf: &CTransform4f) -> Transform {
    let mtx = mint::ColumnMatrix4::from(*xf);
    Transform::from_matrix(mtx.into())
}

#[derive(Debug, Clone, Default)]
struct VertexBufferInfo {
    pub attributes: Vec<(MeshVertexAttribute, VertexAttributeValues)>,
}

#[inline]
fn copy_direct<T>(input: &[u8], component: &SVertexDataComponent) -> Vec<T>
where T: bytemuck::AnyBitPattern {
    let stride = component.stride as usize;
    let mut out = bytemuck::zeroed_vec(input.len() / stride);
    let mut offset = component.offset as usize;
    for v in &mut out {
        *v = *bytemuck::from_bytes(&input[offset..offset + std::mem::size_of::<T>()]);
        offset += stride;
    }
    out
}

#[inline]
fn copy_converting<T, R, C>(input: &[u8], component: &SVertexDataComponent, convert: C) -> Vec<R>
where
    T: bytemuck::AnyBitPattern,
    R: bytemuck::AnyBitPattern,
    C: Fn(T) -> R,
{
    let stride = component.stride as usize;
    let mut out = bytemuck::zeroed_vec(input.len() / stride);
    let mut offset = component.offset as usize;
    for v in &mut out {
        *v = convert(*bytemuck::from_bytes(&input[offset..offset + std::mem::size_of::<T>()]));
        offset += stride;
    }
    out
}

trait HalfArray<const N: usize> {
    fn to_f32_array(self) -> [f32; N];
}

impl<const N: usize> HalfArray<N> for [u16; N] {
    fn to_f32_array(self) -> [f32; N] {
        let mut dst = [0f32; N];
        self.reinterpret_cast::<f16>().convert_to_f32_slice(&mut dst);
        dst
    }
}

fn convert_component(
    input: &[u8],
    component: &SVertexDataComponent,
) -> Option<(MeshVertexAttribute, VertexAttributeValues)> {
    use EVertexComponent::*;
    use EVertexDataFormat::*;
    use VertexAttributeValues::*;
    let attribute = match component.component {
        Position => Mesh::ATTRIBUTE_POSITION,
        Normal => Mesh::ATTRIBUTE_NORMAL,
        Tangent0 => Mesh::ATTRIBUTE_TANGENT,
        Tangent1 => ATTRIBUTE_TANGENT_1,
        Tangent2 => ATTRIBUTE_TANGENT_2,
        TexCoord0 => Mesh::ATTRIBUTE_UV_0,
        TexCoord1 => ATTRIBUTE_UV_1,
        TexCoord2 => ATTRIBUTE_UV_2,
        TexCoord3 => ATTRIBUTE_UV_3,
        Color => Mesh::ATTRIBUTE_COLOR,
        // BoneIndices => Mesh::ATTRIBUTE_JOINT_INDEX,
        // BoneWeights => Mesh::ATTRIBUTE_JOINT_WEIGHT,
        _ => {
            log::info!("Skipping attribute {:?}", component.component);
            return None;
        }
    };
    let values = match component.format {
        Rg8Unorm => Unorm8x2(copy_direct(input, component)),
        Rg8Uint => Uint8x2(copy_direct(input, component)),
        Rg8Snorm => Snorm8x2(copy_direct(input, component)),
        Rg8Sint => Sint8x2(copy_direct(input, component)),
        R32Uint => Uint32(copy_direct(input, component)),
        R32Sint => Sint32(copy_direct(input, component)),
        R32Float => Float32(copy_direct(input, component)),
        Rg16Unorm => Unorm16x2(copy_direct(input, component)),
        Rg16Uint => Uint16x2(copy_direct(input, component)),
        Rg16Snorm => Snorm16x2(copy_direct(input, component)),
        Rg16Sint => Sint16x2(copy_direct(input, component)),
        Rg16Float => Float32x2(copy_converting(input, component, |v: [u16; 2]| v.to_f32_array())),
        Rgba8Unorm => match component.component {
            Color => Float32x4(copy_converting(input, component, |v: [u8; 4]| {
                v.map(|u| u as f32 * 255.0)
            })),
            _ => Unorm8x4(copy_direct(input, component)),
        },
        Rgba8Uint => Uint16x4(copy_converting(input, component, |v: [u8; 4]| v.map(|n| n as u16))),
        Rgba8Snorm => Snorm8x4(copy_direct(input, component)),
        Rgba8Sint => Sint8x4(copy_direct(input, component)),
        Rg32Uint => Uint32x2(copy_direct(input, component)),
        Rg32Sint => Sint32x2(copy_direct(input, component)),
        Rg32Float => Float32x2(copy_direct(input, component)),
        Rgba16Unorm => Unorm16x4(copy_direct(input, component)),
        Rgba16Uint => Uint16x4(copy_direct(input, component)),
        Rgba16Snorm => Snorm16x4(copy_direct(input, component)),
        Rgba16Sint => Sint16x4(copy_direct(input, component)),
        Rgba16Float => match component.component {
            Position | Normal => Float32x3(copy_converting(input, component, |v: [u16; 4]| {
                debug_assert_eq!(v[3], 1);
                *array_ref!(v.to_f32_array(), 0, 3)
            })),
            TexCoord0 | TexCoord1 | TexCoord2 | TexCoord3 => {
                Float32x2(copy_converting(input, component, |v: [u16; 4]| {
                    let dst = v.to_f32_array();
                    if component.component == TexCoord1 {
                        // println!("UV 1: {:?}", values);
                        // ???
                        *array_ref!(dst, 2, 2)
                    } else {
                        *array_ref!(dst, 0, 2)
                    }
                }))
            }
            _ => Float32x4(copy_converting(input, component, |v: [u16; 4]| v.to_f32_array())),
        },
        Rgb32Uint => Uint32x3(copy_direct(input, component)),
        Rgb32Sint => Sint32x3(copy_direct(input, component)),
        Rgb32Float => Float32x3(copy_direct(input, component)),
        Rgba32Uint => Uint32x4(copy_direct(input, component)),
        Rgba32Sint => Sint32x4(copy_direct(input, component)),
        Rgba32Float => match component.component {
            Position | Normal => Float32x3(copy_direct(input, component)),
            TexCoord0 | TexCoord1 | TexCoord2 | TexCoord3 => {
                Float32x2(copy_direct(input, component))
            }
            _ => Float32x4(copy_direct(input, component)),
        },
        R16Uint => Uint32(copy_converting(input, component, |v: u16| v as u32)),
        R16Sint => Sint32(copy_converting(input, component, |v: i16| v as i32)),
        R16Float => Float32(copy_converting(input, component, |v: u16| f16::from_bits(v).to_f32())),
        _ => todo!(),
    };
    Some((attribute, values))
}

fn slice_vertices(values: &VertexAttributeValues, range: Range<usize>) -> VertexAttributeValues {
    use VertexAttributeValues::*;
    match values {
        Float32(vec) => Float32(vec[range].to_vec()),
        Sint32(vec) => Sint32(vec[range].to_vec()),
        Uint32(vec) => Uint32(vec[range].to_vec()),
        Float32x2(vec) => Float32x2(vec[range].to_vec()),
        Sint32x2(vec) => Sint32x2(vec[range].to_vec()),
        Uint32x2(vec) => Uint32x2(vec[range].to_vec()),
        Float32x3(vec) => Float32x3(vec[range].to_vec()),
        Sint32x3(vec) => Sint32x3(vec[range].to_vec()),
        Uint32x3(vec) => Uint32x3(vec[range].to_vec()),
        Float32x4(vec) => Float32x4(vec[range].to_vec()),
        Sint32x4(vec) => Sint32x4(vec[range].to_vec()),
        Uint32x4(vec) => Uint32x4(vec[range].to_vec()),
        Sint16x2(vec) => Sint16x2(vec[range].to_vec()),
        Snorm16x2(vec) => Snorm16x2(vec[range].to_vec()),
        Uint16x2(vec) => Uint16x2(vec[range].to_vec()),
        Unorm16x2(vec) => Unorm16x2(vec[range].to_vec()),
        Sint16x4(vec) => Sint16x4(vec[range].to_vec()),
        Snorm16x4(vec) => Snorm16x4(vec[range].to_vec()),
        Uint16x4(vec) => Uint16x4(vec[range].to_vec()),
        Unorm16x4(vec) => Unorm16x4(vec[range].to_vec()),
        Sint8x2(vec) => Sint8x2(vec[range].to_vec()),
        Snorm8x2(vec) => Snorm8x2(vec[range].to_vec()),
        Uint8x2(vec) => Uint8x2(vec[range].to_vec()),
        Unorm8x2(vec) => Unorm8x2(vec[range].to_vec()),
        Sint8x4(vec) => Sint8x4(vec[range].to_vec()),
        Snorm8x4(vec) => Snorm8x4(vec[range].to_vec()),
        Uint8x4(vec) => Uint8x4(vec[range].to_vec()),
        Unorm8x4(vec) => Unorm8x4(vec[range].to_vec()),
    }
}

#[inline]
fn slice_indices<T>(input: &[T], start: u32, count: u32) -> (Vec<T>, Range<usize>)
where T: num_traits::Num + num_traits::Bounded + Copy + PartialOrd + TryInto<usize> {
    let slice = &input[start as usize..(start + count) as usize];
    let mut min = T::max_value();
    let mut max = T::min_value();
    for &v in slice {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    let values =
        if min.is_zero() { slice.to_vec() } else { slice.iter().map(|&v| v - min).collect() };
    (values, min.try_into().unwrap_or(usize::MAX)..max.try_into().unwrap_or(usize::MAX) + 1)
}

#[derive(Clone)]
enum IndicesSlice<'a> {
    U16(Cow<'a, [u16]>),
    U32(Cow<'a, [u32]>),
}
