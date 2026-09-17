//! Isolated model/ROOM inspection inputs. Recovered providers use explicit
//! preview values, optionally with an authored atlas. Native scene registration,
//! script modulation and probe lighting are not reconstructed here.

use std::{fs::File, io::Read, path::PathBuf};

use anyhow::{ensure, Context, Result};
use bevy::{
    camera::visibility::NoFrustumCulling,
    prelude::*,
    render::renderer::{RenderAdapterInfo, RenderDevice},
};
use retrolib::format::{
    cmdl::{CMaterialData, CMaterialDataInner, EMaterialDataId, EMaterialDataType},
    material_ambient::AmbientSource,
    material_scene_bindings::{
        DrawUniform, ReflectionProbeSnapshot, SceneMaterialInputs, SceneMaterialSnapshot,
        SceneTextureView,
    },
    model_data::wind::WindEnvironment,
    reflection_probe::{ReflectionBrdfLut, ReflectionProbeParams},
    texture_sampler::TextureSamplerParams,
    txtr::{ETextureFormat, ETextureType},
};

use super::{
    original_shader::{
        prepare_mesh_with_scene, Compiler, OriginalShaderInfo, OriginalShaderMesh,
        OriginalShaderTime, ShaderOptions,
    },
    scene_lighting::{BakedAtlasLighting, ATLAS_TOKEN},
};
use crate::{
    loaders::{
        material::MaterialAsset,
        model::{MaterialKey, ModelAsset},
        texture::TextureAsset,
    },
    material::CustomMaterial,
};

// Host-local token identity, not an executable address or serialized asset ID.
const BRDF_TOKEN: u64 = 1;

#[derive(Resource)]
pub struct ShaderPreview {
    enabled: bool,
    compiler: Compiler,
    revision: u64,
    brdf: Option<Handle<Image>>,
    reflection: [u8; 0x30],
    path_input: String,
    loaded_path: Option<PathBuf>,
    pending_path: Option<PathBuf>,
    error: Option<String>,
    animation_seconds: f64,
    animation_playing: bool,
    preference_path: Option<PathBuf>,
}

impl Default for ShaderPreview {
    fn default() -> Self {
        let params = ReflectionProbeParams::default();
        let mut reflection = [0; 0x30];
        reflection[0x10..0x18].copy_from_slice(&BRDF_TOKEN.to_le_bytes());
        for (i, v) in [
            params.max_valid_mip_index,
            params.min_intensity,
            params.max_intensity,
            params.probe_intensity,
        ]
        .into_iter()
        .enumerate()
        {
            reflection[0x20 + 4 * i..0x24 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        Self {
            enabled: true,
            compiler: Compiler::Legacy,
            revision: 0,
            brdf: None,
            reflection,
            path_input: String::new(),
            loaded_path: None,
            pending_path: None,
            error: None,
            animation_seconds: 0.0,
            animation_playing: false,
            preference_path: None,
        }
    }
}

impl ShaderPreview {
    pub fn animation_time(&self) -> f64 { self.animation_seconds }

    pub fn set_animation_playing(&mut self, playing: bool) { self.animation_playing = playing; }

    pub fn set_animation_time(&mut self, seconds: f64) -> Result<()> {
        ensure!(
            seconds.is_finite() && seconds >= 0.0,
            "Material preview time must be finite and nonnegative"
        );
        self.animation_seconds = seconds;
        Ok(())
    }

    fn advance_uv(&mut self, seconds: f64) {
        if self.enabled && self.animation_playing && seconds.is_finite() && seconds >= 0.0 {
            let next = self.animation_seconds + seconds;
            if next.is_finite() {
                self.animation_seconds = next;
            }
        }
    }

    /// A single local resource preference. Default preview instances do no disk
    /// I/O; the application opts in, and tests can provide an isolated path.
    pub fn restore_brdf_preference(&mut self, path: PathBuf) {
        self.preference_path = Some(path.clone());
        let result = (|| -> Result<Option<PathBuf>> {
            let mut bytes = Vec::new();
            match File::open(&path) {
                Ok(file) => {
                    file.take(16_385).read_to_end(&mut bytes)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            ensure!(bytes.len() <= 16_384, "Oversized BRDF path preference");
            Ok(Some(serde_json::from_slice(&bytes)?))
        })();
        match result {
            Ok(Some(path)) => self.request_brdf(path),
            Ok(None) => (),
            Err(e) => self.error = Some(format!("Could not restore BRDF preference: {e:#}")),
        }
    }

    fn save_brdf_preference(&self, table: &std::path::Path) -> Result<()> {
        let Some(path) = &self.preference_path else {
            return Ok(());
        };
        std::fs::create_dir_all(path.parent().context("BRDF preference directory missing")?)?;
        std::fs::write(path, serde_json::to_vec(table)?)?;
        Ok(())
    }

    pub fn set_compiler(&mut self, compiler: Compiler) {
        if self.compiler != compiler {
            self.compiler = compiler;
            self.revision += 1;
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.revision += 1;
        }
    }

    pub fn request_brdf(&mut self, path: PathBuf) {
        self.path_input = path.to_string_lossy().into_owned();
        self.pending_path = Some(path);
        self.error = None;
    }

    pub fn clear_brdf(&mut self) {
        self.pending_path = None;
        self.error = None;
        self.loaded_path = None;
        self.path_input.clear();
        if let Some(path) = &self.preference_path {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    self.error = Some(format!("Could not clear saved BRDF preference: {e}"));
                }
            }
        }
        if self.brdf.take().is_some() {
            self.revision += 1;
        }
    }

    /// Pending and failed reads leave the last successfully loaded resource in use.
    fn load_pending(&mut self, images: &mut Assets<Image>) {
        let Some(path) = self.pending_path.take() else {
            return;
        };
        let result = (|| -> Result<_> {
            let path = path.canonicalize()?;
            let mut bytes = Vec::new();
            File::open(&path)?
                .take(ReflectionBrdfLut::BYTE_LEN as u64 + 1)
                .read_to_end(&mut bytes)?;
            ReflectionBrdfLut::parse(&bytes)?;
            Ok((
                path,
                images.add(Image::new_fill(
                    wgpu::Extent3d {
                        width: ReflectionBrdfLut::WIDTH as u32,
                        height: ReflectionBrdfLut::HEIGHT as u32,
                        depth_or_array_layers: 1,
                    },
                    wgpu::TextureDimension::D2,
                    &bytes,
                    wgpu::TextureFormat::Rg8Unorm,
                    bevy::asset::RenderAssetUsages::default(),
                )),
            ))
        })();
        match result {
            Ok((path, handle)) => {
                self.brdf = Some(handle);
                self.error = self
                    .save_brdf_preference(&path)
                    .err()
                    .map(|e| format!("Table loaded, but its path could not be saved: {e:#}"));
                self.loaded_path = Some(path);
                self.revision += 1;
            }
            Err(error) => {
                self.error = Some(format!("Could not load {}: {error:#}", path.display()))
            }
        }
    }

    fn inputs_with_reflection<'a>(
        &self,
        reflection: &'a [u8; 0x30],
    ) -> Result<SceneMaterialInputs<'a>> {
        let mut scene = SceneMaterialInputs::default();
        // Isolated previews start at system time zero with unit exposure.
        // These are explicit host inputs, not inferred game scene state.
        scene.global_system_values = Some(Default::default());
        scene.wind_environment = Some(WindEnvironment::native_fallback());
        scene.set_draw_uniform(DrawUniform::LodParameters);
        scene.set(
            EMaterialDataId::ALLD,
            Some(SceneMaterialSnapshot::AmbientLight(AmbientSource::full_bright())),
        )?;
        scene.set(EMaterialDataId::CLLD, None)?;
        // CGameUIRenderJob::RenderPrimaryPass (0x7100b359b4) supplies the
        // default back/top backlight intensities, then two zero components.
        // World rendering can modulate these through backlight hints; this is
        // the standalone preview's explicit lighting policy.
        scene.set_uniform(CMaterialData {
            data_id: EMaterialDataId::BKGL,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color([4.0, 2.0, 0.0, 0.0].into()),
        })?;
        if self.brdf.is_some() {
            scene.set(
                EMaterialDataId::REFP,
                Some(SceneMaterialSnapshot::ReflectionProbe(ReflectionProbeSnapshot::parse(
                    reflection,
                )?)),
            )?;
            scene.set_texture_view(BRDF_TOKEN, None, SceneTextureView {
                kind: ETextureType::D2,
                format: ETextureFormat::Rg8Unorm,
                sampler: TextureSamplerParams::simple(1, [0; 3], 0),
                mip_level_count: Some(1),
            });
        }
        Ok(scene)
    }

    /// Pair preview REFP with the material's actual REFL view, then apply an
    /// optional authored atlas. This does not resolve spatial reflection probes.
    /// Native resolve (7100217170) uses the texture token's mip count minus one.
    /// The viewer binds complete textures, so the TXTR mip count is that range.
    /// Scene renderers supplying their own probe snapshots use the lower-level
    /// prepare_mesh_with_scene API instead.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_mesh(
        &self,
        model: &ModelAsset,
        index: usize,
        mirrored: bool,
        materials: &Assets<MaterialAsset>,
        textures: &Assets<TextureAsset>,
        options: ShaderOptions,
        atlas: Option<&BakedAtlasLighting>,
    ) -> Result<super::original_shader::OriginalShaderMesh> {
        let mut reflection = self.reflection;
        if self.brdf.is_some() {
            let mesh = model.inner.mesh.meshes.get(index).context("Missing mesh")?;
            for data in model.inner.mtrl.material_values(mesh.material_idx as usize)? {
                if data.data_id != EMaterialDataId::REFL {
                    continue;
                }
                let CMaterialDataInner::Texture(token) = &data.data else {
                    anyhow::bail!("Reflection material value is not a texture");
                };
                let texture = model
                    .textures
                    .get(&token.id)
                    .and_then(|handle| textures.get(handle))
                    .context("Reflection texture unavailable")?;
                let header = &texture.inner.head;
                ensure!(header.kind == ETextureType::Cube, "Reflection texture is not a cube");
                let mip_count = header.mip_sizes.len();
                ensure!((1..=255).contains(&mip_count), "Invalid reflection mip count");
                let max_mip = (mip_count - 1) as f32;
                reflection[0x20..0x24].copy_from_slice(&max_mip.to_le_bytes());
            }
        }
        let mut scene = self.inputs_with_reflection(&reflection)?;
        let atlas_texture = atlas
            .map(|atlas| {
                let texture = textures.get(&atlas.texture).context("ROOM lightmap unavailable")?;
                atlas.bind(texture, &mut scene)?;
                Ok::<_, anyhow::Error>(texture)
            })
            .transpose()?;
        prepare_mesh_with_scene(
            model,
            index,
            mirrored,
            materials,
            textures,
            &scene,
            options,
            |token| {
                if token == ATLAS_TOKEN {
                    return Ok(atlas_texture.context("ROOM lightmap unavailable")?.texture.clone());
                }
                self.image(token)
            },
        )
    }

    pub fn image(&self, token: u64) -> Result<Handle<Image>> {
        ensure!(token == BRDF_TOKEN, "Unknown preview scene texture token");
        self.brdf.clone().context("Load the game's reflection BRDF table in View > Shader preview")
    }

    /// Report the selected meshes' actual preparation results, not just the toggle.
    pub fn description<'a>(
        &self,
        meshes: impl IntoIterator<Item = Option<&'a PreviewMesh>>,
    ) -> String {
        let (mut total, mut original, mut fallback, mut preparing, mut unavailable) =
            (0, 0, 0, 0, 0);
        let mut baked = 0;
        for mesh in meshes {
            if mesh.is_some_and(|mesh| mesh.original.as_ref().is_some_and(|info| info.baked_atlas))
            {
                baked += 1;
            }
            total += 1;
            match mesh.map(|m| m.status(self)) {
                Some(PreviewMeshStatus::Original) => original += 1,
                Some(PreviewMeshStatus::Fallback | PreviewMeshStatus::Preview) => fallback += 1,
                Some(PreviewMeshStatus::Unavailable) => unavailable += 1,
                Some(PreviewMeshStatus::Preparing) | None => preparing += 1,
            }
        }
        let mut text = if self.enabled {
            format!("Original shaders: {original}/{total} visible meshes; {fallback} fallback")
        } else {
            format!("Preview materials: {fallback}/{total} visible meshes; Bevy lighting")
        };
        if preparing != 0 {
            text.push_str(&format!("; {preparing} preparing"));
        }
        if unavailable != 0 {
            text.push_str(&format!("; {unavailable} unavailable"));
        }
        if original != 0 {
            if baked == 0 {
                text.push_str("\nOriginal shaders use full-bright preview lighting");
            } else {
                text.push_str(&format!("\nBaked lightmap: {baked} draws; white modulation"));
            }
        }
        text
    }

    pub fn menu(&mut self, ui: &mut egui::Ui) {
        let mut enabled = self.enabled;
        ui.checkbox(&mut enabled, "Use original shaders (experimental)");
        self.set_enabled(enabled);
        ui.label("Models use full-bright ambient lighting. ROOMs can use their baked lightmaps; probes and dynamic lighting are not yet applied.");
        ui.separator();
        ui.label("Compiler");
        let mut compiler = self.compiler;
        for option in [
            Compiler::Legacy,
            Compiler::Remax(remax::program::wgsl::Profile::Rendering),
            Compiler::Remax(remax::program::wgsl::Profile::Checked),
        ] {
            ui.radio_value(&mut compiler, option, option.label());
        }
        self.set_compiler(compiler);
        if matches!(compiler, Compiler::Remax(_)) {
            ui.small("Experimental semantic compiler. GPU faults are checked asynchronously; a failure switches the affected instance to preview shading.");
        }
        ui.label("Material animation");
        ui.horizontal(|ui| {
            if ui.button(if self.animation_playing { "Pause" } else { "Play" }).clicked() {
                self.set_animation_playing(!self.animation_playing);
            }
            if ui.button("Reset").clicked() {
                self.set_animation_time(0.0).unwrap();
                self.set_animation_playing(false);
            }
            let mut seconds = self.animation_time();
            let response = ui.add(
                egui::DragValue::new(&mut seconds)
                    .speed(0.05)
                    .range(0.0..=1_000_000.0)
                    .suffix(" s"),
            );
            if response.changed() {
                let _ = self.set_animation_time(seconds);
            }
        });
        ui.small("Controls playback for supported material animations.");
        ui.separator();
        ui.label("Reflection BRDF table");
        ui.label("Some materials require the extracted 256-byte game table. Its location is remembered for the next launch.");
        ui.text_edit_singleline(&mut self.path_input);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.path_input.is_empty(), egui::Button::new("Load table"))
                .clicked()
            {
                self.request_brdf(PathBuf::from(&self.path_input));
            }
            if ui
                .add_enabled(
                    self.brdf.is_some() || self.pending_path.is_some(),
                    egui::Button::new("Clear table"),
                )
                .clicked()
            {
                self.clear_brdf();
            }
        });
        if let Some(path) = &self.loaded_path {
            ui.label(format!("Using {}", path.display()));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        ui.small("Open model and area tabs update automatically; unsupported materials use preview shading.");
    }
}

/// Renderer selection is attached to the existing geometry entity so changing
/// preview resources preserves visibility, LOD, instance transforms and cameras.
#[derive(Component)]
pub struct PreviewMesh {
    model: Handle<ModelAsset>,
    index: usize,
    key: MaterialKey,
    revision: Option<u64>,
    fallback_available: bool,
    pub original: Option<OriginalShaderInfo>,
    description: String,
    atlas: Option<BakedAtlasLighting>,
}

impl PreviewMesh {
    pub fn set_baked_atlas(&mut self, atlas: Option<BakedAtlasLighting>) {
        let unchanged = match (&self.atlas, &atlas) {
            (None, None) => true,
            (Some(old), Some(new)) => old.same_binding(new),
            _ => false,
        };
        if !unchanged {
            self.atlas = atlas;
            self.revision = None;
        }
    }

    pub fn description(&self) -> String {
        let mut text = self.description.clone();
        if let Some(info) = &self.original {
            if matches!(info.compiler, Compiler::Remax(_)) {
                text.push_str(&format!(
                    "\nAsynchronous GPU checks: {} completed",
                    info.diagnostics.completed()
                ));
            }
        }
        text
    }

    pub fn new(model: Handle<ModelAsset>, index: usize, key: MaterialKey) -> Self {
        Self {
            model,
            index,
            key,
            revision: None,
            fallback_available: false,
            original: None,
            description: "Preparing renderer".into(),
            atlas: None,
        }
    }

    pub fn status(&self, preview: &ShaderPreview) -> PreviewMeshStatus {
        if self.revision != Some(preview.revision)
            || self.original.as_ref().is_some_and(|info| info.diagnostics.pending())
        {
            PreviewMeshStatus::Preparing
        } else if self.original.is_some() {
            PreviewMeshStatus::Original
        } else if !self.fallback_available {
            PreviewMeshStatus::Unavailable
        } else if preview.enabled {
            PreviewMeshStatus::Fallback
        } else {
            PreviewMeshStatus::Preview
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewMeshStatus {
    Preparing,
    Original,
    Fallback,
    Preview,
    Unavailable,
}

impl PreviewMeshStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing",
            Self::Original => "Original shaders",
            Self::Fallback => "Preview fallback",
            Self::Preview => "Preview",
            Self::Unavailable => "Renderer unavailable",
        }
    }
}

pub struct ShaderPreviewPlugin;
impl Plugin for ShaderPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShaderPreview>().add_systems(PostUpdate, update_preview_meshes);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_preview_meshes(
    mut commands: Commands,
    device: Res<RenderDevice>,
    adapter: Res<RenderAdapterInfo>,
    server: Res<AssetServer>,
    mut preview: ResMut<ShaderPreview>,
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut models: ResMut<Assets<ModelAsset>>,
    material_resources: Res<Assets<MaterialAsset>>,
    textures: Res<Assets<TextureAsset>>,
    mut materials: ResMut<Assets<CustomMaterial>>,
    mut meshes: Query<(Entity, &mut PreviewMesh, Option<&mut OriginalShaderTime>)>,
) {
    preview.load_pending(&mut images);
    preview.advance_uv(time.delta_secs_f64());
    // Area instances share each prepared draw for the same model and mesh state.
    // A per-update cache avoids retaining closed models or stale image bindings.
    let mut selections = std::collections::HashMap::new();
    let options = ShaderOptions {
        compiler: preview.compiler,
        features: device.features(),
        backend: adapter.backend,
    };
    for (entity, mut mesh, clock) in &mut meshes {
        if let Some(mut clock) = clock {
            if clock.0 != preview.animation_seconds {
                clock.0 = preview.animation_seconds;
            }
        } else {
            commands.entity(entity).insert(OriginalShaderTime(preview.animation_seconds));
        }
        if mesh.revision == Some(preview.revision) {
            if let Some(error) = mesh.original.as_ref().and_then(|info| info.diagnostics.failure())
            {
                if let Some(mut model) = models.get_mut(&mesh.model) {
                    let fallback =
                        model.material(&mesh.key, &mut materials, &material_resources, &textures);
                    let mut instance = commands.entity(entity);
                    instance.remove::<(OriginalShaderMesh, NoFrustumCulling)>();
                    mesh.original = None;
                    mesh.description = format!("Preview fallback; {error}");
                    match fallback {
                        Ok(material) => {
                            instance.insert(MeshMaterial3d(material));
                            mesh.fallback_available = true;
                        }
                        Err(error) => {
                            mesh.fallback_available = false;
                            mesh.description
                                .push_str(&format!("\nPreview also unavailable: {error:#}"));
                        }
                    }
                }
            }
            continue;
        }
        if preview.enabled
            && mesh.atlas.as_ref().is_some_and(|atlas| {
                textures.get(&atlas.texture).is_none()
                    && !server.load_state(&atlas.texture).is_failed()
            })
        {
            continue;
        }
        let Some(mut model) = models.get_mut(&mesh.model) else {
            continue;
        };
        let (original, fallback, description) = selections
            .entry((
                mesh.model.id(),
                mesh.index,
                mesh.key,
                mesh.atlas.as_ref().map(|atlas| atlas.texture.id()),
            ))
            .or_insert_with(|| {
                let original = if preview.enabled {
                    preview.prepare_mesh(
                        &model,
                        mesh.index,
                        mesh.key.mesh_mirrored,
                        &material_resources,
                        &textures,
                        options,
                        mesh.atlas.as_ref(),
                    )
                } else {
                    Err(anyhow::anyhow!("Original shaders disabled in View > Shader preview"))
                };
                match original {
                    Ok(original) => {
                        let info = original.info();
                        let mut description = format!(
                            "{}; {}\n\
                             VS source {}, PS source {}\n\
                             Permutation {:016X}{:016X}",
                            info.compiler.label(),
                            if mesh.atlas.is_some() {
                                "baked atlas; white modulation"
                            } else {
                                "full-bright preview lighting"
                            },
                            info.sources[0],
                            info.sources[1],
                            info.permutation[1],
                            info.permutation[0],
                        );
                        if info.animated_uv || info.wind {
                            description
                                .push_str("\nMaterial animation controls: View > Shader preview");
                        }
                        (Some(original), None, description)
                    }
                    Err(error) => {
                        let mut description = if preview.enabled {
                            format!("Preview material; original shader unavailable: {error:#}")
                        } else {
                            "Preview material; original shaders disabled".into()
                        };
                        if preview.enabled
                            && preview.brdf.is_none()
                            && format!("{error:#}").contains("Unresolved scene material REFP")
                        {
                            description.push_str(
                                "\nLoad the game's reflection BRDF table in View > Shader preview.",
                            );
                        }
                        let fallback = match model.material(
                            &mesh.key,
                            &mut materials,
                            &material_resources,
                            &textures,
                        ) {
                            Ok(material) => Some(material),
                            Err(error) => {
                                description
                                    .push_str(&format!("\nPreview also unavailable: {error:#}"));
                                None
                            }
                        };
                        (None, fallback, description)
                    }
                }
            });
        let mut entity = commands.entity(entity);
        // Required components outlive their owner in Bevy. Restore ordinary
        // mesh culling when returning to preview materials.
        entity.remove::<(OriginalShaderMesh, NoFrustumCulling, MeshMaterial3d<CustomMaterial>)>();
        let original = original.as_ref().map(|original| {
            let mut instance = original.instance();
            instance.baked_atlas_lookup = mesh.atlas.as_ref().map(|atlas| atlas.lookup);
            instance
        });
        if let Some(original) = &original {
            entity.insert(original.clone());
        }
        if let Some(fallback) = fallback {
            entity.insert(MeshMaterial3d(fallback.clone()));
        }
        mesh.original = original.as_ref().map(OriginalShaderMesh::info);
        mesh.fallback_available = fallback.is_some();
        mesh.description = description.clone();
        mesh.revision = Some(preview.revision);
    }
}

/// Conventional per-user location; no workspace paths are built into the app.
pub fn brdf_preference_path() -> Option<PathBuf> {
    let directory = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(directory.join("retrotool/brdf-table-path.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_clock_pauses_seeks_and_preserves_shader_revision() {
        let mut preview = ShaderPreview::default();
        let revision = preview.revision;
        preview.advance_uv(1.0);
        assert_eq!(preview.animation_time(), 0.0);
        preview.set_animation_playing(true);
        preview.advance_uv(0.25);
        assert_eq!(preview.animation_time(), 0.25);
        preview.set_animation_time(3.0).unwrap();
        preview.advance_uv(0.25);
        assert_eq!(preview.animation_time(), 3.25);
        preview.set_animation_playing(false);
        preview.advance_uv(100.0);
        assert_eq!(preview.animation_time(), 3.25);
        assert!(preview.set_animation_time(f64::NAN).is_err());
        assert!(preview.set_animation_time(-1.0).is_err());
        assert_eq!(preview.revision, revision);
        preview.set_enabled(false);
        preview.set_animation_playing(true);
        preview.advance_uv(100.0);
        assert_eq!(preview.animation_time(), 3.25);
    }

    #[test]
    fn brdf_preference_restores_only_valid_loads_and_clear_removes_it() {
        let directory = std::env::temp_dir().join(format!(
            "retrotool-brdf-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let table = directory.join("table with spaces.bin");
        let invalid = directory.join("invalid.bin");
        let preference = directory.join("settings/brdf-table-path.json");
        std::fs::write(&table, [0u8; ReflectionBrdfLut::BYTE_LEN]).unwrap();
        std::fs::write(&invalid, [0u8; 255]).unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(AssetPlugin::default()).init_asset::<Image>();
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut preview = ShaderPreview::default();
        preview.restore_brdf_preference(preference.clone());
        assert!(preview.error.is_none() && preview.brdf.is_none());
        preview.request_brdf(table.clone());
        preview.load_pending(&mut images);
        assert!(preview.error.is_none() && preview.brdf.is_some());
        let saved = std::fs::read(&preference).unwrap();
        assert_eq!(
            serde_json::from_slice::<PathBuf>(&saved).unwrap(),
            table.canonicalize().unwrap()
        );
        let handle = preview.brdf.clone();
        preview.request_brdf(invalid);
        preview.load_pending(&mut images);
        assert!(preview.error.is_some());
        assert_eq!(preview.brdf, handle);
        assert_eq!(std::fs::read(&preference).unwrap(), saved);
        let mut restored = ShaderPreview::default();
        restored.restore_brdf_preference(preference.clone());
        restored.load_pending(&mut images);
        assert!(restored.error.is_none() && restored.brdf.is_some());
        assert_eq!(restored.loaded_path, Some(table.canonicalize().unwrap()));
        restored.clear_brdf();
        assert!(restored.brdf.is_none() && !preference.exists());
        let mut cleared = ShaderPreview::default();
        cleared.restore_brdf_preference(preference.clone());
        assert!(cleared.pending_path.is_none());
        std::fs::write(&preference, b"malformed preference").unwrap();
        cleared.restore_brdf_preference(preference);
        assert!(cleared.error.is_some() && cleared.pending_path.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
