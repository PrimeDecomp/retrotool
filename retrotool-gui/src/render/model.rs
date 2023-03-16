use std::{borrow::Cow, collections::HashMap, num::NonZeroU8, ops::Range};

use anyhow::Result;
use bevy::{
    prelude::*,
    render::{
        mesh::{Indices, MeshVertexAttribute, VertexAttributeValues},
        primitives::Aabb,
        render_resource::SamplerDescriptor,
        texture::ImageSampler,
    },
};
use bit_set::BitSet;
use half::f16;
use retrolib::format::{
    cmdl::{
        CMaterialDataInner, EBufferType, EMaterialDataId, EVertexComponent, EVertexDataFormat,
        ModelData, STextureUsageInfo, SVertexDataComponent,
    },
    txtr::{
        ETextureAnisotropicRatio, ETextureFilter, ETextureMipFilter, ETextureWrap,
        STextureSamplerData,
    },
    CAABox, CTransform4f,
};
use uuid::Uuid;
use wgpu_types::{AddressMode, FilterMode, PrimitiveTopology};

use crate::{
    loaders::{model::ModelAsset, texture::TextureAsset},
    material::CustomMaterial,
};

pub struct BuiltMesh {
    pub mesh: Handle<Mesh>,
    pub material: Handle<CustomMaterial>,
    pub material_name: String,
    pub visible: bool,
}

pub struct ModelLod {
    pub meshes: BitSet,
    pub distance: Option<f32>,
}

pub struct BuiltModel {
    pub meshes: Vec<BuiltMesh>,
    pub lod: Vec<ModelLod>,
    pub aabb: Aabb,
}

pub fn load_model(
    asset: &ModelAsset,
    _commands: &mut Commands,
    // server: &AssetServer,
    texture_assets: &Assets<TextureAsset>,
    images: &mut Assets<Image>,
    materials: &mut Assets<CustomMaterial>,
    meshes: &mut Assets<Mesh>,
    // center: bool,
) -> Result<BuiltModel> {
    let ModelAsset {
        inner: ModelData { head, mtrl, mesh, vbuf, ibuf, vtx_buffers, idx_buffers },
        textures,
    } = asset;

    // Build sampler descriptors
    let mut sampler_descriptors = HashMap::<Uuid, SamplerDescriptor>::new();
    for mat in &mtrl.materials {
        for data in &mat.data {
            match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    if let Some(usage) = &texture.usage {
                        let sampler_data = textures
                            .get(&texture.id)
                            .and_then(|handle| texture_assets.get(handle))
                            .map(|txtr| &txtr.inner.head.sampler_data);
                        sampler_descriptors
                            .insert(texture.id, sampler_descriptor_from_usage(usage, sampler_data));
                    }
                }
                CMaterialDataInner::LayeredTexture(layers) => {
                    for texture in &layers.textures {
                        if let Some(usage) = &texture.usage {
                            let sampler_data = textures
                                .get(&texture.id)
                                .and_then(|handle| texture_assets.get(handle))
                                .map(|txtr| &txtr.inner.head.sampler_data);
                            sampler_descriptors.insert(
                                texture.id,
                                sampler_descriptor_from_usage(usage, sampler_data),
                            );
                        }
                    }
                }
                _ => continue,
            }
        }
    }

    // Build texture images
    let mut texture_handles = HashMap::<Uuid, Handle<Image>>::new();
    for (id, handle) in textures {
        let asset = texture_assets.get(handle).unwrap();
        let mut image = asset.texture.clone();
        if let Some(desc) = sampler_descriptors.get(id) {
            image.sampler_descriptor = ImageSampler::Descriptor(desc.clone());
        }
        texture_handles.insert(*id, images.add(image));
    }

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

    // Build materials
    let mut material_handles = Vec::with_capacity(mtrl.materials.len());
    for mat in &mtrl.materials {
        let mut out_mat = CustomMaterial::default();
        // log::info!("Shader {}, unk {}", mat.shader_id, mat.unk_guid);
        for data in &mat.data {
            match data.data_id {
                EMaterialDataId::DIFT | EMaterialDataId::BCLR => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        out_mat.base_color_l0 = Color::WHITE;
                        out_mat.base_color_texture_0 = texture_handles.get(&texture.id).cloned();
                        out_mat.base_color_uv_0 =
                            texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    }
                    _ => {
                        log::warn!("Unsupported material data type for DIFT {:?}", data.data_type);
                    }
                },
                EMaterialDataId::BCRL => match &data.data {
                    CMaterialDataInner::LayeredTexture(layers) => {
                        out_mat.base_color_l0 = layers.base.colors[0].to_array().into();
                        out_mat.base_color_l1 = layers.base.colors[1].to_array().into();
                        out_mat.base_color_l2 = layers.base.colors[2].to_array().into();
                        out_mat.base_color_texture_0 =
                            texture_handles.get(&layers.textures[0].id).cloned();
                        out_mat.base_color_texture_1 =
                            texture_handles.get(&layers.textures[1].id).cloned();
                        out_mat.base_color_texture_2 =
                            texture_handles.get(&layers.textures[2].id).cloned();
                        out_mat.base_color_uv_0 = layers.textures[0]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.base_color_uv_1 = layers.textures[1]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.base_color_uv_2 = layers.textures[2]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                    }
                    _ => {
                        log::warn!("Unsupported material data type for BCRL {:?}", data.data_type);
                    }
                },
                EMaterialDataId::DIFC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        out_mat.base_color = Color::rgba(color.r, color.g, color.b, color.a);
                    }
                    _ => log::warn!("Unsupported material data type for DIFC {:?}", data.data_type),
                },
                EMaterialDataId::ICAN => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        out_mat.emissive_texture = texture_handles.get(&texture.id).cloned();
                        out_mat.emissive_uv =
                            texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    }
                    _ => log::warn!("Unsupported material data type for ICAN {:?}", data.data_type),
                },
                EMaterialDataId::ICNC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        out_mat.emissive_color = Color::rgba(color.r, color.g, color.b, color.a);
                    }
                    _ => log::warn!("Unsupported material data type for ICNC {:?}", data.data_type),
                },
                EMaterialDataId::NMAP => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        out_mat.normal_map_l0 = Color::WHITE;
                        out_mat.normal_map_texture_0 = texture_handles.get(&texture.id).cloned();
                        out_mat.normal_map_uv_0 = texture.usage.as_ref().unwrap().tex_coord;
                    }
                    _ => {
                        log::warn!("Unsupported material data type for NMAP {:?}", data.data_type);
                    }
                },
                EMaterialDataId::NRML => match &data.data {
                    CMaterialDataInner::LayeredTexture(layers) => {
                        out_mat.normal_map_l0 = layers.base.colors[0].to_array().into();
                        out_mat.normal_map_l1 = layers.base.colors[1].to_array().into();
                        out_mat.normal_map_l2 = layers.base.colors[2].to_array().into();
                        out_mat.normal_map_texture_0 =
                            texture_handles.get(&layers.textures[0].id).cloned();
                        out_mat.normal_map_texture_1 =
                            texture_handles.get(&layers.textures[1].id).cloned();
                        out_mat.normal_map_texture_2 =
                            texture_handles.get(&layers.textures[2].id).cloned();
                        out_mat.normal_map_uv_0 = layers.textures[0]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.normal_map_uv_1 = layers.textures[1]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.normal_map_uv_2 = layers.textures[2]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                    }
                    _ => {
                        log::warn!("Unsupported material data type for NRML {:?}", data.data_type);
                    }
                },
                EMaterialDataId::METL => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        out_mat.metallic_map_l0 = Color::WHITE;
                        out_mat.metallic_map_texture_0 = texture_handles.get(&texture.id).cloned();
                        out_mat.metallic_map_uv_0 = texture.usage.as_ref().unwrap().tex_coord;
                    }
                    _ => {
                        log::warn!("Unsupported material data type for METL {:?}", data.data_type);
                    }
                },
                EMaterialDataId::MTLL => match &data.data {
                    CMaterialDataInner::LayeredTexture(layers) => {
                        out_mat.metallic_map_l0 = layers.base.colors[0].to_array().into();
                        out_mat.metallic_map_l1 = layers.base.colors[1].to_array().into();
                        out_mat.metallic_map_l2 = layers.base.colors[2].to_array().into();
                        out_mat.metallic_map_texture_0 =
                            texture_handles.get(&layers.textures[0].id).cloned();
                        out_mat.metallic_map_texture_1 =
                            texture_handles.get(&layers.textures[1].id).cloned();
                        out_mat.metallic_map_texture_2 =
                            texture_handles.get(&layers.textures[2].id).cloned();
                        out_mat.metallic_map_uv_0 = layers.textures[0]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.metallic_map_uv_1 = layers.textures[1]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                        out_mat.metallic_map_uv_2 = layers.textures[2]
                            .usage
                            .as_ref()
                            .map(|u| u.tex_coord)
                            .unwrap_or_default();
                    }
                    _ => {
                        log::warn!("Unsupported material data type for MTLL {:?}", data.data_type);
                    }
                },
                id => {
                    log::warn!("Unsupported material data ID {id:?}");
                }
            }
        }
        material_handles.push(materials.add(out_mat));
    }

    // Process meshes
    let aabb = Aabb::from_min_max(
        Vec3::new(head.bounds.min.x, head.bounds.min.y, head.bounds.min.z),
        Vec3::new(head.bounds.max.x, head.bounds.max.y, head.bounds.max.z),
    );
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
            material: material_handles[in_mesh.material_idx as usize].clone(),
            material_name: mtrl.materials[in_mesh.material_idx as usize].name.clone(),
            visible: true,
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

    Ok(BuiltModel { meshes: out_meshes, lod, aabb })
}

pub fn convert_aabb(aabb: &CAABox) -> Aabb {
    Aabb::from_min_max(
        Vec3::new(aabb.min.x, aabb.min.y, aabb.min.z),
        Vec3::new(aabb.max.x, aabb.max.y, aabb.max.z),
    )
}

pub fn convert_transform(xf: &CTransform4f) -> Transform {
    Transform::from_matrix(Mat4::from_cols_array(&xf.to_matrix_array()))
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
        TexCoord0 => Mesh::ATTRIBUTE_UV_0,
        Color => Mesh::ATTRIBUTE_COLOR,
        // BoneIndices => Mesh::ATTRIBUTE_JOINT_INDEX,
        // BoneWeights => Mesh::ATTRIBUTE_JOINT_WEIGHT,
        _ => return None,
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
        Rg16Float => Float32x2(copy_converting(input, component, |v: [u16; 2]| {
            v.map(|u| f16::from_bits(u).to_f32())
        })),
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
            Position | Normal => Float32x3(copy_converting(input, component, |v: [u16; 3]| {
                v.map(|u| f16::from_bits(u).to_f32())
            })),
            TexCoord0 => Float32x2(copy_converting(input, component, |v: [u16; 2]| {
                v.map(|u| f16::from_bits(u).to_f32())
            })),
            _ => Float32x4(copy_converting(input, component, |v: [u16; 4]| {
                v.map(|u| f16::from_bits(u).to_f32())
            })),
        },
        Rgb32Uint => Uint32x3(copy_direct(input, component)),
        Rgb32Sint => Sint32x3(copy_direct(input, component)),
        Rgb32Float => Float32x3(copy_direct(input, component)),
        Rgba32Uint => Uint32x4(copy_direct(input, component)),
        Rgba32Sint => Sint32x4(copy_direct(input, component)),
        Rgba32Float => match component.component {
            Position | Normal => Float32x3(copy_direct(input, component)),
            TexCoord0 => Float32x2(copy_direct(input, component)),
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

fn texture_wrap(wrap: ETextureWrap) -> AddressMode {
    match wrap {
        ETextureWrap::ClampToEdge => AddressMode::ClampToEdge,
        ETextureWrap::Repeat => AddressMode::Repeat,
        ETextureWrap::MirroredRepeat => AddressMode::MirrorRepeat,
        ETextureWrap::MirrorClamp => todo!("Mirror clamp"),
        ETextureWrap::ClampToBorder => AddressMode::ClampToBorder,
        ETextureWrap::Clamp => todo!("Clamp"),
    }
}

fn sampler_descriptor_from_usage<'a>(
    usage: &STextureUsageInfo,
    data: Option<&STextureSamplerData>,
) -> SamplerDescriptor<'a> {
    SamplerDescriptor {
        label: None,
        address_mode_u: match usage.wrap_x {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            u32::MAX => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_x)),
            n => todo!("wrap {n}"),
        },
        address_mode_v: match usage.wrap_y {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            u32::MAX => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_y)),
            n => todo!("wrap {n}"),
        },
        address_mode_w: match usage.wrap_z {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            u32::MAX => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_z)),
            n => todo!("wrap {n}"),
        },
        mag_filter: match usage.filter {
            0 => FilterMode::Nearest,
            1 => FilterMode::Linear,
            u32::MAX => data.map_or(FilterMode::Nearest, |d| match d.filter {
                ETextureFilter::Nearest => FilterMode::Nearest,
                ETextureFilter::Linear => FilterMode::Linear,
            }),
            n => todo!("Filter {n}"),
        },
        min_filter: match usage.filter {
            0 => FilterMode::Nearest,
            1 => FilterMode::Linear,
            u32::MAX => data.map_or(FilterMode::Nearest, |d| match d.filter {
                ETextureFilter::Nearest => FilterMode::Nearest,
                ETextureFilter::Linear => FilterMode::Linear,
            }),
            n => todo!("Filter {n}"),
        },
        mipmap_filter: data.map_or(FilterMode::Nearest, |d| match d.mip_filter {
            ETextureMipFilter::Nearest => FilterMode::Nearest,
            ETextureMipFilter::Linear => FilterMode::Linear,
        }),
        lod_min_clamp: 0.0,
        lod_max_clamp: f32::MAX,
        compare: None,
        anisotropy_clamp: NonZeroU8::new(
            data.map(|d| match d.aniso {
                ETextureAnisotropicRatio::None => 0,
                ETextureAnisotropicRatio::Ratio1 => 1,
                ETextureAnisotropicRatio::Ratio2 => 2,
                ETextureAnisotropicRatio::Ratio4 => 4,
                ETextureAnisotropicRatio::Ratio8 => 8,
                ETextureAnisotropicRatio::Ratio16 => 16,
            })
            .unwrap_or_default(),
        ),
        // anisotropy_clamp: NonZeroU8::new(8),
        border_color: None,
    }
}
