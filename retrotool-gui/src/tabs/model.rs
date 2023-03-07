use std::{borrow::Cow, ops::Range};

use bevy::{
    asset::LoadState,
    core_pipeline::clear_color::ClearColorConfig,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::{
        camera::Viewport,
        mesh::*,
        primitives::Aabb,
        render_resource::{
            AddressMode, Extent3d, FilterMode, SamplerDescriptor, TextureDescriptor,
            TextureDimension, TextureFormat, TextureUsages,
        },
        texture::ImageSampler,
        view::RenderLayers,
    },
    utils::HashMap,
};
use bevy_egui::EguiContext;
use egui::{Id, PointerButton, Sense};
use half::f16;
use retrolib::format::{
    cmdl::{
        CMaterialDataInner, EBufferType, EMaterialDataId, EVertexComponent, EVertexDataFormat,
        ModelData, STextureUsageInfo, SVertexDataComponent,
    },
    txtr::{ETextureFormat, ETextureType},
};
use uuid::Uuid;

use crate::{
    icon,
    loaders::{ModelAsset, TextureAsset},
    tabs::SystemTab,
    AssetRef, TabState,
};

pub struct LoadedModel {
    pub entities: Vec<(Entity, bool)>,
    pub camera_xf: Transform,
    pub upside_down: bool,
    pub radius: f32,
    pub origin: Vec3,
    pub projection: Projection,
}

pub struct ModelTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<ModelAsset>,
    pub loaded: Option<LoadedModel>,
}

#[derive(Debug, Clone, Default)]
struct VertexBufferInfo {
    // pub vertex_count: u32,
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
        // Color => Mesh::ATTRIBUTE_COLOR,
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
        Rgba8Uint => Uint8x4(copy_direct(input, component)),
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
        if min == T::zero() { slice.to_vec() } else { slice.iter().map(|&v| v - min).collect() };
    (values, min.try_into().unwrap_or(usize::MAX)..max.try_into().unwrap_or(usize::MAX) + 1)
}

#[derive(Clone)]
enum IndicesSlice<'a> {
    U16(Cow<'a, [u16]>),
    U32(Cow<'a, [u32]>),
}

#[derive(Component)]
pub struct TemporaryTag;

fn sampler_descriptor_from_usage<'a>(usage: &STextureUsageInfo) -> SamplerDescriptor<'a> {
    SamplerDescriptor {
        label: None,
        address_mode_u: match usage.wrap_x {
            0 | u32::MAX => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            n => todo!("wrap {n}"),
        },
        address_mode_v: match usage.wrap_y {
            0 | u32::MAX => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            n => todo!("wrap {n}"),
        },
        address_mode_w: match usage.wrap_z {
            0 | u32::MAX => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            n => todo!("wrap {n}"),
        },
        mag_filter: match usage.filter {
            0 | u32::MAX => FilterMode::Nearest,
            1 => FilterMode::Linear,
            n => todo!("Filter {n}"),
        },
        min_filter: match usage.filter {
            0 | u32::MAX => FilterMode::Nearest,
            1 => FilterMode::Linear,
            n => todo!("Filter {n}"),
        },
        mipmap_filter: FilterMode::Linear, // TODO?
        lod_min_clamp: 0.0,
        lod_max_clamp: 0.0,
        compare: None,
        anisotropy_clamp: None,
        border_color: None,
    }
}

impl SystemTab for ModelTab {
    type LoadParam = (
        SCommands,
        SResMut<Assets<Mesh>>,
        SResMut<Assets<StandardMaterial>>,
        SResMut<Assets<ModelAsset>>,
        SResMut<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<AssetServer>,
    );
    type UiParam = SCommands;

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (
            mut commands,
            mut meshes,
            mut materials,
            mut models,
            texture_assets,
            mut images,
            server,
        ) = query;
        if let Some(loaded) = &self.loaded {
            for (entity, _) in &loaded.entities {
                if let Some(mut commands) = commands.get_entity(*entity) {
                    commands.insert(Visibility::INVISIBLE);
                }
            }
            return;
        }

        let ModelAsset {
            inner: ModelData { head, mtrl, mesh, vbuf, ibuf, vtx_buffers, idx_buffers },
            textures,
        } = match models.get_mut(&self.handle) {
            Some(v) => v,
            None => return,
        };
        // Ensure all dependencies loaded
        match server.get_group_load_state(textures.iter().map(|(_, h)| h.id())) {
            LoadState::Loaded => {}
            _ => return,
        }

        // Build sampler descriptors
        let mut sampler_descriptors = HashMap::<Uuid, SamplerDescriptor>::new();
        for mat in &mtrl.materials {
            for data in &mat.data {
                match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        if let Some(usage) = &texture.usage {
                            sampler_descriptors
                                .insert(texture.id, sampler_descriptor_from_usage(usage));
                        }
                    }
                    CMaterialDataInner::LayeredTexture(textures) => {
                        for texture in &textures.textures {
                            if let Some(usage) = &texture.usage {
                                sampler_descriptors
                                    .insert(texture.id, sampler_descriptor_from_usage(usage));
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
            let txtr = texture_assets.get(handle).unwrap();
            let image_handle = images.add(Image {
                data: txtr
                    .decompressed
                    .as_ref()
                    .map(|i| i.to_rgba8().into_raw())
                    .unwrap_or_else(|| txtr.inner.data.clone()),
                texture_descriptor: TextureDescriptor {
                    label: None,
                    size: Extent3d {
                        width: txtr.inner.head.width,
                        height: txtr.inner.head.height,
                        depth_or_array_layers: if txtr.decompressed.is_some() {
                            1 // FIXME
                        } else {
                            txtr.inner.head.layers
                        },
                    },
                    mip_level_count: if txtr.decompressed.is_some() {
                        1
                    } else {
                        txtr.inner.head.mip_sizes.len() as u32
                    },
                    sample_count: 1,
                    dimension: match txtr.inner.head.kind {
                        ETextureType::_1D => TextureDimension::D1,
                        ETextureType::_2D => TextureDimension::D2,
                        ETextureType::_3D => TextureDimension::D3,
                        ETextureType::Cube => TextureDimension::D2,
                        ETextureType::_1DArray => TextureDimension::D1,
                        ETextureType::_2DArray => TextureDimension::D2,
                        ETextureType::_2DMultisample => TextureDimension::D2,
                        ETextureType::_2DMultisampleArray => TextureDimension::D2,
                        ETextureType::CubeArray => TextureDimension::D2,
                    },
                    format: if txtr.decompressed.is_some() {
                        if txtr.inner.head.format.is_srgb() {
                            TextureFormat::Rgba8UnormSrgb
                        } else {
                            TextureFormat::Rgba8Unorm
                        }
                    } else {
                        match txtr.inner.head.format {
                            ETextureFormat::R8Unorm => TextureFormat::Rgba8Unorm,
                            ETextureFormat::R8Snorm => TextureFormat::R8Snorm,
                            ETextureFormat::R8Uint => TextureFormat::R8Uint,
                            ETextureFormat::R8Sint => TextureFormat::R8Sint,
                            ETextureFormat::R16Unorm => TextureFormat::R16Unorm,
                            ETextureFormat::R16Snorm => TextureFormat::R16Snorm,
                            ETextureFormat::R16Uint => TextureFormat::R16Uint,
                            ETextureFormat::R16Sint => TextureFormat::R16Sint,
                            ETextureFormat::R16Float => TextureFormat::R16Float,
                            ETextureFormat::R32Uint => TextureFormat::R32Uint,
                            ETextureFormat::R32Sint => TextureFormat::R32Sint,
                            ETextureFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
                            ETextureFormat::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
                            ETextureFormat::Rgba16Float => TextureFormat::Rgba16Float,
                            ETextureFormat::Rgba32Float => TextureFormat::Rgba32Float,
                            ETextureFormat::Depth16Unorm => TextureFormat::Depth16Unorm,
                            ETextureFormat::Depth16Unorm2 => TextureFormat::Depth16Unorm,
                            ETextureFormat::Depth24S8Unorm => TextureFormat::Depth24PlusStencil8,
                            ETextureFormat::Depth32Float => TextureFormat::Depth32Float,
                            ETextureFormat::RgbaBc1Unorm => TextureFormat::Bc1RgbaUnorm,
                            ETextureFormat::RgbaBc1Srgb => TextureFormat::Bc1RgbaUnormSrgb,
                            ETextureFormat::RgbaBc2Unorm => TextureFormat::Bc2RgbaUnorm,
                            ETextureFormat::RgbaBc2Srgb => TextureFormat::Bc2RgbaUnormSrgb,
                            ETextureFormat::RgbaBc3Unorm => TextureFormat::Bc3RgbaUnorm,
                            ETextureFormat::RgbaBc3Srgb => TextureFormat::Bc3RgbaUnormSrgb,
                            ETextureFormat::RgbaBc4Unorm => TextureFormat::Bc4RUnorm,
                            ETextureFormat::RgbaBc4Snorm => TextureFormat::Bc4RSnorm,
                            ETextureFormat::RgbaBc5Unorm => TextureFormat::Bc5RgUnorm,
                            ETextureFormat::RgbaBc5Snorm => TextureFormat::Bc5RgSnorm,
                            ETextureFormat::Rg11B10Float => TextureFormat::Rg11b10Float,
                            ETextureFormat::R32Float => TextureFormat::R32Float,
                            ETextureFormat::Rg8Unorm => TextureFormat::Rg8Unorm,
                            ETextureFormat::Rg8Snorm => TextureFormat::Rg8Snorm,
                            ETextureFormat::Rg8Uint => TextureFormat::Rg8Uint,
                            ETextureFormat::Rg8Sint => TextureFormat::Rg8Sint,
                            ETextureFormat::Rg16Float => TextureFormat::Rg16Float,
                            ETextureFormat::Rg16Unorm => TextureFormat::Rg16Unorm,
                            ETextureFormat::Rg16Snorm => TextureFormat::Rg16Snorm,
                            ETextureFormat::Rg16Uint => TextureFormat::Rg16Uint,
                            ETextureFormat::Rg16Sint => TextureFormat::Rg16Sint,
                            ETextureFormat::Rgb10A2Unorm => TextureFormat::Rgb10a2Unorm,
                            ETextureFormat::Rg32Uint => TextureFormat::Rg32Uint,
                            ETextureFormat::Rg32Sint => TextureFormat::Rg32Sint,
                            ETextureFormat::Rg32Float => TextureFormat::Rg32Float,
                            ETextureFormat::Rgba16Unorm => TextureFormat::Rgba16Unorm,
                            ETextureFormat::Rgba16Snorm => TextureFormat::Rgba16Snorm,
                            ETextureFormat::Rgba16Uint => TextureFormat::Rgba16Uint,
                            ETextureFormat::Rgba16Sint => TextureFormat::Rgba16Sint,
                            ETextureFormat::Rgba32Uint => TextureFormat::Rgba32Uint,
                            ETextureFormat::Rgba32Sint => TextureFormat::Rgba32Sint,
                            ETextureFormat::BptcUfloat => TextureFormat::Bc6hRgbUfloat,
                            ETextureFormat::BptcSfloat => TextureFormat::Bc6hRgbSfloat,
                            ETextureFormat::BptcUnorm => TextureFormat::Bc7RgbaUnorm,
                            ETextureFormat::BptcUnormSrgb => TextureFormat::Bc7RgbaUnormSrgb,
                            format => todo!("format {format:?}"),
                        }
                    },
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                },
                sampler_descriptor: sampler_descriptors
                    .get(id)
                    .map(|desc| ImageSampler::Descriptor(desc.clone()))
                    .unwrap_or_default(),
                texture_view_descriptor: None,
            });
            texture_handles.insert(*id, image_handle);
        }

        // Build vertex buffers
        let mut buf_infos: Vec<VertexBufferInfo> = Vec::with_capacity(vtx_buffers.len());
        let mut cur_buf = 0usize;
        for info in &vbuf.info {
            let num_buffers = info.unk as usize; // guess
            let mut attributes = Vec::with_capacity(info.components.len());
            for component in &info.components {
                let input = &*vtx_buffers[cur_buf + component.buffer_index as usize];
                if let Some((attribute, values)) = convert_component(input, component) {
                    attributes.push((attribute, values));
                }
            }
            buf_infos.push(VertexBufferInfo {
                // vertex_count: info.vertex_count,
                attributes,
            });
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
            let mut out_mat = StandardMaterial::default();
            println!("Shader {}, unk {}", mat.shader_id, mat.unk_guid);
            let _ = server.load_untyped(format!("{}.MTRL", mat.shader_id));
            for data in &mat.data {
                match data.data_id {
                    EMaterialDataId::DIFT | EMaterialDataId::BCLR => match &data.data {
                        CMaterialDataInner::Texture(texture) => {
                            out_mat.base_color_texture = texture_handles.get(&texture.id).cloned();
                        }
                        _ => {
                            log::warn!(
                                "Unsupported material data type for DIFT {:?}",
                                data.data_type
                            );
                        }
                    },
                    EMaterialDataId::DIFC => match &data.data {
                        CMaterialDataInner::Color(color) => {
                            out_mat.base_color = Color::rgba(color.r, color.g, color.b, color.a);
                        }
                        _ => log::warn!(
                            "Unsupported material data type for DIFC {:?}",
                            data.data_type
                        ),
                    },
                    EMaterialDataId::ICAN => match &data.data {
                        CMaterialDataInner::Texture(_texture) => {
                            // out_mat.emissive_texture = texture_handles.get(&texture.id).cloned();
                        }
                        _ => log::warn!(
                            "Unsupported material data type for ICAN {:?}",
                            data.data_type
                        ),
                    },
                    EMaterialDataId::ICNC => match &data.data {
                        CMaterialDataInner::Color(_color) => {
                            // out_mat.emissive = Color::rgba(color.r, color.g, color.b, color.a);
                        }
                        _ => log::warn!(
                            "Unsupported material data type for ICNC {:?}",
                            data.data_type
                        ),
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
        let mut entities = vec![];
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
                out_mesh.insert_attribute(
                    component.clone(),
                    slice_vertices(values, vert_range.clone()),
                );
            }
            entities.push((
                commands
                    .spawn(PbrBundle {
                        mesh: meshes.add(out_mesh),
                        material: material_handles[in_mesh.material_idx as usize].clone(),
                        transform: Transform::from_translation((-aabb.center).into()),
                        ..default()
                    })
                    .id(),
                true,
            ));
        }

        let radius = (aabb.max() - aabb.min()).max_element() * 1.25;
        let mut camera_xf =
            Transform::from_xyz(-radius, 5.0, radius).looking_at(Vec3::ZERO, Vec3::Y);
        let rot_matrix = Mat3::from_quat(camera_xf.rotation);
        camera_xf.translation = rot_matrix.mul_vec3(Vec3::new(0.0, 0.0, radius));
        self.loaded = Some(LoadedModel {
            entities,
            camera_xf,
            upside_down: false,
            radius,
            origin: Vec3::ZERO,
            projection: Projection::Perspective(default()),
        });
    }

    fn close(&mut self, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (mut commands, _, _, _, _, _, _) = query;
        if let Some(loaded) = &self.loaded {
            for (entity, _) in &loaded.entities {
                if let Some(mut commands) = commands.get_entity(*entity) {
                    commands.despawn();
                }
            }
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        state: &mut TabState,
    ) {
        let rect = ui.available_rect_before_wrap();
        let left_top = rect.left_top();
        let size = rect.size();
        let viewport = Viewport {
            physical_position: UVec2 { x: left_top.x as u32, y: left_top.y as u32 },
            physical_size: UVec2 { x: size.x as u32, y: size.y as u32 },
            depth: 0.0..1.0,
        };

        let response = ui.interact(rect, Id::new("background"), Sense::click_and_drag());

        let mut commands = query;
        if let Some(loaded) = &mut self.loaded {
            let mut transform = &mut loaded.camera_xf;
            let mut any = false;
            let mut rotation_move = Vec2::ZERO;
            let mut pan = Vec2::ZERO;
            let scroll = {
                if response.hovered() {
                    let delta = ui.input(|i| i.scroll_delta);
                    Vec2::new(delta.x, delta.y)
                } else {
                    Vec2::ZERO
                }
            };
            if response.drag_started_by(PointerButton::Primary)
                || response.drag_released_by(PointerButton::Primary)
            {
                // only check for upside down when orbiting started or ended this frame
                // if the camera is "upside" down, panning horizontally would be inverted, so invert the input to make it correct
                let up = transform.rotation * Vec3::Y;
                loaded.upside_down = up.y <= 0.0;
            }
            if response.dragged_by(PointerButton::Primary) {
                let delta = response.drag_delta();
                rotation_move = Vec2::new(delta.x, delta.y);
            } else if response.dragged_by(PointerButton::Middle) {
                let delta = response.drag_delta();
                pan = Vec2::new(delta.x, delta.y);
            }
            if rotation_move.length_squared() > 0.0 {
                any = true;
                let delta_x = {
                    let delta = rotation_move.x / rect.width() * std::f32::consts::PI * 2.0;
                    if loaded.upside_down {
                        -delta
                    } else {
                        delta
                    }
                };
                let delta_y = rotation_move.y / rect.height() * std::f32::consts::PI;
                let yaw = Quat::from_rotation_y(-delta_x);
                let pitch = Quat::from_rotation_x(-delta_y);
                transform.rotation = yaw * transform.rotation; // rotate around global y axis
                transform.rotation *= pitch; // rotate around local x axis
            } else if pan.length_squared() > 0.0 {
                any = true;
                if let Projection::Perspective(projection) = &loaded.projection {
                    pan *= Vec2::new(projection.fov * projection.aspect_ratio, projection.fov)
                        / Vec2::new(rect.width(), rect.height());
                }
                // translate by local axes
                let right = transform.rotation * Vec3::X * -pan.x;
                let up = transform.rotation * Vec3::Y * pan.y;
                // make panning proportional to distance away from focus point
                let translation = (right + up) * loaded.radius;
                loaded.origin += translation;
            } else if scroll.y.abs() > 0.0 {
                any = true;
                loaded.radius -= (scroll.y / 50.0/* TODO ? */) * loaded.radius * 0.2;
                // dont allow zoom to reach zero or you get stuck
                loaded.radius = f32::max(loaded.radius, 0.05);
            }
            if any {
                // emulating parent/child to make the yaw/y-axis rotation behave like a turntable
                // parent = x and y rotation
                // child = z-offset
                let rot_matrix = Mat3::from_quat(transform.rotation);
                transform.translation =
                    loaded.origin + rot_matrix.mul_vec3(Vec3::new(0.0, 0.0, loaded.radius));
            }

            commands.spawn((
                Camera3dBundle {
                    camera_3d: Camera3d {
                        clear_color: if state.render_layer == 0 {
                            ClearColorConfig::Default
                        } else {
                            ClearColorConfig::None
                        },
                        ..default()
                    },
                    camera: Camera {
                        viewport: Some(viewport),
                        priority: state.render_layer as isize,
                        ..default()
                    },
                    transform: loaded.camera_xf,
                    ..default()
                },
                RenderLayers::layer(state.render_layer),
                TemporaryTag,
            ));
            commands.spawn((
                DirectionalLightBundle {
                    directional_light: DirectionalLight { ..default() },
                    transform: Transform::from_xyz(-30.0, 5.0, 20.0)
                        .looking_at(Vec3::ZERO, Vec3::Y),
                    ..default()
                },
                RenderLayers::layer(state.render_layer),
                TemporaryTag,
            ));

            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(rect.height() * 0.25).show(ui, |ui| {
                    for (idx, (entity, visible)) in loaded.entities.iter_mut().enumerate() {
                        ui.checkbox(visible, format!("Mesh {idx}"));
                        if let Some(mut commands) = commands.get_entity(*entity) {
                            commands.insert((
                                if *visible { Visibility::VISIBLE } else { Visibility::INVISIBLE },
                                RenderLayers::layer(state.render_layer),
                            ));
                        }
                    }
                });
            });
            state.render_layer += 1;
        }
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::FILE_3D, self.asset_ref.kind, self.asset_ref.id).into()
    }
}
