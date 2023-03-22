use std::{num::NonZeroU8, path::PathBuf};

use anyhow::{Error, Result};
use bevy::{
    asset::{AssetLoader, AssetPath, BoxedFuture, LoadContext, LoadState, LoadedAsset},
    prelude::*,
    render::{render_resource::SamplerDescriptor, texture::ImageSampler},
    utils::{hashbrown::hash_map::Entry, HashMap},
};
use binrw::Endian;
use retrolib::format::{
    cmdl::{
        CMaterialCache, CMaterialDataInner, EMaterialDataId, ModelData, STextureUsageInfo,
        K_FORM_CMDL,
    },
    foot::{locate_asset_id, locate_meta},
    txtr::{
        ETextureAnisotropicRatio, ETextureFilter, ETextureMipFilter, ETextureWrap,
        STextureSamplerData,
    },
};
use uuid::Uuid;
use wgpu_types::{AddressMode, Face, FilterMode};

use crate::{
    loaders::texture::TextureAsset,
    material::CustomMaterial,
    render::{convert_color, model::MESH_FLAG_OPAQUE},
    AssetRef,
};

#[derive(Copy, Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct MaterialKey {
    pub material_idx: usize,
    pub mesh_flags: u16,
    pub mesh_mirrored: bool,
}

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e829"]
pub struct ModelAsset {
    pub asset_ref: AssetRef,
    pub inner: ModelData,
    pub textures: HashMap<Uuid, Handle<TextureAsset>>,
    pub texture_images: HashMap<Uuid, Handle<Image>>,
    pub materials: HashMap<MaterialKey, Handle<CustomMaterial>>,
}

pub struct ModelAssetLoader;

impl FromWorld for ModelAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for ModelAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let id = locate_asset_id(bytes, Endian::Little)?;
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = ModelData::slice(bytes, meta, Endian::Little)?;
            // log::info!("Loaded model {:?}", data.head);
            // log::info!("Loaded meshes {:#?}", data.mesh);
            let mut dependencies = HashMap::<Uuid, AssetPath>::new();
            for mat in &data.mtrl.materials {
                for data in &mat.data {
                    match &data.data {
                        CMaterialDataInner::Texture(texture) => {
                            dependencies.insert(
                                texture.id,
                                AssetPath::new(PathBuf::from(format!("{}.TXTR", texture.id)), None),
                            );
                        }
                        CMaterialDataInner::LayeredTexture(texture) => {
                            for texture in &texture.textures {
                                if texture.id.is_nil() {
                                    continue;
                                }
                                dependencies.insert(
                                    texture.id,
                                    AssetPath::new(
                                        PathBuf::from(format!("{}.TXTR", texture.id)),
                                        None,
                                    ),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let textures = dependencies
                .iter()
                .map(|(u, p)| (*u, load_context.get_handle(p.clone())))
                .collect();
            load_context.set_default_asset(
                LoadedAsset::new(ModelAsset {
                    asset_ref: AssetRef { id, kind: K_FORM_CMDL },
                    inner: data,
                    textures,
                    texture_images: default(),
                    materials: default(),
                })
                .with_dependencies(dependencies.into_values().collect()),
            );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["cmdl", "smdl", "wmdl"] }
}

impl ModelAsset {
    pub fn get_load_state(&self, server: &AssetServer) -> LoadState {
        server.get_group_load_state(self.textures.values().map(|h| h.id()))
    }

    pub fn sampler_data<'asset>(
        &self,
        texture_id: &Uuid,
        texture_assets: &'asset Assets<TextureAsset>,
    ) -> Option<&'asset STextureSamplerData> {
        self.textures
            .get(texture_id)
            .and_then(|handle| texture_assets.get(handle))
            .map(|txtr| &txtr.inner.head.sampler_data)
    }

    pub fn build_texture_images(
        &mut self,
        texture_assets: &Assets<TextureAsset>,
        images: &mut Assets<Image>,
    ) {
        // Build sampler descriptors
        let mut sampler_descriptors = HashMap::<Uuid, SamplerDescriptor>::new();
        for mat in &self.inner.mtrl.materials {
            for data in &mat.data {
                match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        if let Some(usage) = &texture.usage {
                            let sampler_data = self.sampler_data(&texture.id, texture_assets);
                            sampler_descriptors.insert(
                                texture.id,
                                sampler_descriptor_from_usage(usage, sampler_data),
                            );
                        }
                    }
                    CMaterialDataInner::LayeredTexture(layers) => {
                        for texture in &layers.textures {
                            if let Some(usage) = &texture.usage {
                                let sampler_data = self.sampler_data(&texture.id, texture_assets);
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
        for (id, handle) in &self.textures {
            let asset = texture_assets.get(handle).unwrap();
            let mut image = asset.texture.clone();
            if let Some(desc) = sampler_descriptors.get(id) {
                image.sampler_descriptor = ImageSampler::Descriptor(desc.clone());
            }
            self.texture_images.insert(*id, images.add(image));
        }
    }

    pub fn material(
        &mut self,
        key: &MaterialKey,
        assets: &mut Assets<CustomMaterial>,
    ) -> Result<Handle<CustomMaterial>> {
        Ok(match self.materials.entry(*key) {
            Entry::Occupied(e) => e.get().clone(),
            Entry::Vacant(e) => {
                let material =
                    build_material(key, &self.inner.mtrl.materials, &self.texture_images)?;
                let handle = assets.add(material);
                e.insert(handle.clone());
                handle
            }
        })
    }
}

fn build_material(
    key: &MaterialKey,
    materials: &[CMaterialCache],
    texture_images: &HashMap<Uuid, Handle<Image>>,
) -> Result<CustomMaterial> {
    let mut out_mat = CustomMaterial {
        alpha_mode: if key.mesh_flags & MESH_FLAG_OPAQUE != 0 {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        },
        cull_mode: Some(if key.mesh_mirrored { Face::Front } else { Face::Back }),
        ..default()
    };
    for data in &materials[key.material_idx].data {
        match data.data_id {
            EMaterialDataId::DIFT | EMaterialDataId::BCLR => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.base_color_l0 = Color::WHITE;
                    out_mat.base_color_texture_0 = texture_images.get(&texture.id).cloned();
                    out_mat.base_color_uv_0 =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for DIFT {:?}", data.data_type);
                }
            },
            EMaterialDataId::BCRL => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.base_color_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.base_color_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.base_color_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.base_color_texture_0 =
                        texture_images.get(&layers.textures[0].id).cloned();
                    out_mat.base_color_texture_1 =
                        texture_images.get(&layers.textures[1].id).cloned();
                    out_mat.base_color_texture_2 =
                        texture_images.get(&layers.textures[2].id).cloned();
                    out_mat.base_color_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.base_color_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.base_color_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for BCRL {:?}", data.data_type);
                }
            },
            EMaterialDataId::DIFC => match &data.data {
                CMaterialDataInner::Color(color) => {
                    out_mat.base_color = convert_color(color);
                }
                _ => log::warn!("Unsupported material data type for DIFC {:?}", data.data_type),
            },
            EMaterialDataId::ICAN => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.emissive_texture = texture_images.get(&texture.id).cloned();
                    out_mat.emissive_uv =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => log::warn!("Unsupported material data type for ICAN {:?}", data.data_type),
            },
            EMaterialDataId::ICNC => match &data.data {
                CMaterialDataInner::Color(color) => {
                    out_mat.emissive_color = convert_color(color);
                }
                _ => log::warn!("Unsupported material data type for ICNC {:?}", data.data_type),
            },
            EMaterialDataId::NMAP => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.normal_map_l0 = Color::WHITE;
                    out_mat.normal_map_texture_0 = texture_images.get(&texture.id).cloned();
                    out_mat.normal_map_uv_0 = texture.usage.as_ref().unwrap().tex_coord;
                }
                _ => {
                    log::warn!("Unsupported material data type for NMAP {:?}", data.data_type);
                }
            },
            EMaterialDataId::NRML => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.normal_map_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.normal_map_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.normal_map_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.normal_map_texture_0 =
                        texture_images.get(&layers.textures[0].id).cloned();
                    out_mat.normal_map_texture_1 =
                        texture_images.get(&layers.textures[1].id).cloned();
                    out_mat.normal_map_texture_2 =
                        texture_images.get(&layers.textures[2].id).cloned();
                    out_mat.normal_map_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.normal_map_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.normal_map_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for NRML {:?}", data.data_type);
                }
            },
            EMaterialDataId::METL => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.metallic_map_l0 = Color::WHITE;
                    out_mat.metallic_map_texture_0 = texture_images.get(&texture.id).cloned();
                    out_mat.metallic_map_uv_0 = texture.usage.as_ref().unwrap().tex_coord;
                }
                _ => {
                    log::warn!("Unsupported material data type for METL {:?}", data.data_type);
                }
            },
            EMaterialDataId::MTLL => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.metallic_map_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.metallic_map_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.metallic_map_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.metallic_map_texture_0 =
                        texture_images.get(&layers.textures[0].id).cloned();
                    out_mat.metallic_map_texture_1 =
                        texture_images.get(&layers.textures[1].id).cloned();
                    out_mat.metallic_map_texture_2 =
                        texture_images.get(&layers.textures[2].id).cloned();
                    out_mat.metallic_map_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.metallic_map_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.metallic_map_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
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
    Ok(out_mat)
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

fn sampler_descriptor_from_usage<'desc>(
    usage: &STextureUsageInfo,
    data: Option<&STextureSamplerData>,
) -> SamplerDescriptor<'desc> {
    SamplerDescriptor {
        label: None,
        address_mode_u: match usage.wrap_x {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            -1 => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_x)),
            n => todo!("wrap {n}"),
        },
        address_mode_v: match usage.wrap_y {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            -1 => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_y)),
            n => todo!("wrap {n}"),
        },
        address_mode_w: match usage.wrap_z {
            0 => AddressMode::ClampToEdge,
            1 => AddressMode::Repeat,
            2 => AddressMode::MirrorRepeat,
            3 => todo!("Mirror clamp"),
            4 => AddressMode::ClampToBorder,
            5 => todo!("Clamp"),
            -1 => data.map_or(AddressMode::Repeat, |d| texture_wrap(d.wrap_z)),
            n => todo!("wrap {n}"),
        },
        mag_filter: match usage.filter {
            0 => FilterMode::Nearest,
            1 => FilterMode::Linear,
            -1 => data.map_or(FilterMode::Nearest, |d| match d.filter {
                ETextureFilter::Nearest => FilterMode::Nearest,
                ETextureFilter::Linear => FilterMode::Linear,
            }),
            n => todo!("Filter {n}"),
        },
        min_filter: match usage.filter {
            0 => FilterMode::Nearest,
            1 => FilterMode::Linear,
            -1 => data.map_or(FilterMode::Nearest, |d| match d.filter {
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
