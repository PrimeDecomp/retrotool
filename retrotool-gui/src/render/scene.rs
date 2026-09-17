//! Shared geometry creation for container and ROOM inspection.
//! Script activity, scene placement and lighting remain the caller's state.

use anyhow::Result;
use bevy::{camera::primitives::Aabb, ecs::system::SystemParam, math::Vec3A, prelude::*};
use retrolib::format::room::SAtlasLookup;
use uuid::Uuid;

use super::{
    convert_transform, model::load_model, scene_lighting::BakedAtlasLighting,
    scene_preview::PreviewMesh, transform_aabb,
};
use crate::{
    loaders::{
        modcon::ModConAsset,
        model::{MaterialKey, ModelAsset},
        texture::TextureAsset,
    },
    AssetRef,
};

pub struct LoadedInstance {
    pub entity: Entity,
    pub visible: bool,
}

/// Geometry shared by several placements. The loaded roots correspond to
/// `placements` in order; each root owns its mesh children and draw bindings.
pub struct ModelGroup {
    pub handle: Handle<ModelAsset>,
    pub loaded: Vec<LoadedInstance>,
    pub placements: Vec<ModelPlacement>,
    pub aabb: Aabb,
    pub error: Option<String>,
}

pub struct ModelPlacement {
    pub transform: Transform,
    pub atlas_lookup: Option<SAtlasLookup>,
}

impl ModelPlacement {
    fn lighting(&self, texture: Option<&Handle<TextureAsset>>) -> Option<BakedAtlasLighting> {
        let lookup = self.atlas_lookup.filter(SAtlasLookup::is_valid)?;
        Some(BakedAtlasLighting { texture: texture?.clone(), lookup })
    }
}

impl ModelGroup {
    fn new(handle: Handle<ModelAsset>) -> Self {
        Self {
            handle,
            loaded: Vec::new(),
            placements: Vec::new(),
            aabb: Aabb::default(),
            error: None,
        }
    }
}

#[derive(Component, Clone, Debug)]
pub struct ModelLabel {
    pub asset_ref: AssetRef,
    pub tab_id: Uuid,
}

/// Retain each placement, including repeated WMDL resource IDs. The caller
/// chooses the container's scene transform; these are container-local values.
pub fn container_models(container: &ModConAsset) -> Result<Vec<ModelGroup>> {
    let Some(data) = &container.inner.visual_data else { return Ok(Vec::new()) };
    let mut models: Vec<_> = container.models.iter().cloned().map(ModelGroup::new).collect();
    for instance in data.instances()? {
        models[instance.model_index].placements.push(ModelPlacement {
            transform: convert_transform(instance.transform),
            atlas_lookup: instance.atlas_lookup.copied(),
        });
    }
    for (ordinal, instance) in data.world_instances.iter().enumerate() {
        let handle = &container.world_models[&instance.model];
        let index = models.iter().position(|model| model.handle == *handle).unwrap_or_else(|| {
            models.push(ModelGroup::new(handle.clone()));
            models.len() - 1
        });
        models[index].placements.push(ModelPlacement {
            transform: convert_transform(&instance.transform),
            atlas_lookup: data.world_atlas_lookups.get(ordinal).copied(),
        });
    }
    models.retain(|model| !model.placements.is_empty());
    Ok(models)
}

/// Bounds are available only after all nonempty groups have loaded.
pub fn model_bounds<'a>(models: impl IntoIterator<Item = &'a ModelGroup>) -> Option<Aabb> {
    let mut min = Vec3A::splat(f32::INFINITY);
    let mut max = Vec3A::splat(f32::NEG_INFINITY);
    let mut any = false;
    for model in models {
        if model.loaded.len() != model.placements.len() {
            return None;
        }
        for placement in &model.placements {
            let bounds = transform_aabb(&model.aabb, &placement.transform.compute_affine());
            min = min.min(bounds.min());
            max = max.max(bounds.max());
            any = true;
        }
    }
    any.then(|| Aabb::from_min_max(min.into(), max.into()))
}

#[derive(SystemParam)]
pub struct SceneModelLoader<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub server: Res<'w, AssetServer>,
    meshes: ResMut<'w, Assets<Mesh>>,
    models: ResMut<'w, Assets<ModelAsset>>,
    textures: Res<'w, Assets<TextureAsset>>,
    images: ResMut<'w, Assets<Image>>,
    children: Query<'w, 's, &'static Children>,
    previews: Query<'w, 's, &'static mut PreviewMesh>,
}

impl SceneModelLoader<'_, '_> {
    /// Hide existing roots until their owning tab is shown, then create groups
    /// whose dependencies are ready. Returns whether any geometry was added.
    pub fn load(
        &mut self,
        groups: &mut [ModelGroup],
        tab_id: Uuid,
        lightmap: Option<&Handle<TextureAsset>>,
    ) -> bool {
        let mut changed = false;
        for group in groups {
            if !group.loaded.is_empty() {
                for (instance, placement) in group.loaded.iter().zip(&group.placements) {
                    if let Ok(children) = self.children.get(instance.entity) {
                        for entity in children.iter() {
                            if let Ok(mut preview) = self.previews.get_mut(entity) {
                                preview.set_baked_atlas(placement.lighting(lightmap));
                            }
                        }
                    }
                    if let Ok(mut entity) = self.commands.get_entity(instance.entity) {
                        entity.insert(Visibility::Hidden);
                    }
                }
                continue;
            }
            if group.error.is_some() {
                continue;
            }
            let Some(mut asset) = self.models.get_mut(&group.handle) else { continue };
            if !asset.get_load_state(&self.server).is_loaded() {
                continue;
            }
            asset.build_texture_images(&self.textures, &mut self.images);
            let built = match load_model(&asset, &mut self.meshes) {
                Ok(built) => built,
                Err(error) => {
                    group.error = Some(format!("{error:#}"));
                    log::error!("Failed to build {}: {error:#}", asset.asset_ref.id);
                    continue;
                }
            };
            for placement in &group.placements {
                let transform = placement.transform;
                let mirrored = transform.scale.x.is_sign_negative()
                    ^ transform.scale.y.is_sign_negative()
                    ^ transform.scale.z.is_sign_negative();
                let entity = self
                    .commands
                    .spawn((transform, Visibility::Hidden, ModelLabel {
                        asset_ref: asset.asset_ref,
                        tab_id,
                    }))
                    .with_children(|builder| {
                        for index in built.lod[0].meshes.iter() {
                            let mesh = &built.meshes[index];
                            let mut preview =
                                PreviewMesh::new(group.handle.clone(), index, MaterialKey {
                                    material_idx: mesh.material_idx,
                                    mesh_class: mesh.mesh_class,
                                    mesh_mirrored: mirrored,
                                    mesh_two_sided: mesh.two_sided,
                                });
                            preview.set_baked_atlas(placement.lighting(lightmap));
                            builder.spawn((Mesh3d(mesh.mesh.clone()), preview));
                        }
                    })
                    .id();
                group.loaded.push(LoadedInstance { entity, visible: true });
            }
            group.aabb = built.aabb;
            changed = true;
        }
        changed
    }

    pub fn close(&mut self, groups: &[ModelGroup]) {
        for instance in groups.iter().flat_map(|group| &group.loaded) {
            if let Ok(mut entity) = self.commands.get_entity(instance.entity) {
                entity.despawn();
            }
        }
    }
}
