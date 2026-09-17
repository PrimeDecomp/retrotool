//! Mesh source resolution and uniform-only animation updates.

use anyhow::{ensure, Context, Result};

use crate::format::{
    cmdl::EMaterialDataId,
    material_scene_bindings::SceneMaterialInputs,
    model_data::{
        animated_uv::{AnimatedUvSnapshot, ModelAnimatedUvData},
        wind::{WindEnvironment, WindSet, WindSnapshot},
        ModelDataSource, ModelDataSources,
    },
    shader_nvn_uniforms::{NvnUniformBuffers, NvnUniformValue},
};

#[derive(Clone, Debug, Default)]
pub(super) struct MeshAnimation {
    uv: Option<(ModelAnimatedUvData, usize)>,
    wind: Option<(WindSet, WindEnvironment)>,
    pub uv_bindings: Vec<UvBinding>,
    pub wind_bindings: Vec<u16>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct UvBinding {
    pub packed: u16,
    pub option: u32,
    pub flags: u32,
}

#[derive(Default)]
pub(super) struct MeshSnapshots {
    pub uv: Option<AnimatedUvSnapshot>,
    pub wind: Option<WindSnapshot>,
}

impl MeshAnimation {
    pub fn parse(bytes: &[u8], mesh: usize, scene: &SceneMaterialInputs<'_>) -> Result<Self> {
        let mut animation = Self::default();
        let mut seen_uv = false;
        let mut seen_wind = false;
        for source in ModelDataSources::parse(bytes)?.sources {
            match source {
                ModelDataSource::AnimatedUv(data) => {
                    ensure!(!seen_uv, "Duplicate ANUV sources are not supported");
                    seen_uv = true;
                    if data.mesh_sets.get(mesh) != Some(&255)
                        && !matches!(scene.get(EMaterialDataId::AUXF), Ok(Some(_)))
                    {
                        animation.uv = Some((data, mesh));
                    }
                }
                ModelDataSource::Wind(data) => {
                    ensure!(!seen_wind, "Duplicate WIND sources are not supported");
                    seen_wind = true;
                    if let Some(set) = data.mesh_set(mesh)? {
                        if !matches!(scene.get(EMaterialDataId::WIND), Ok(Some(_))) {
                            let environment = scene.wind_environment.context(
                                "Model WIND source requires an explicit wind environment",
                            )?;
                            animation.wind = Some((*set, environment));
                        }
                    }
                }
            }
        }
        Ok(animation)
    }

    pub fn snapshots(&self, seconds: f64) -> Result<MeshSnapshots> {
        ensure!(seconds.is_finite(), "Nonfinite material animation time");
        let uv = self
            .uv
            .as_ref()
            .map(|(data, mesh)| {
                let mut instance = data.instance()?;
                instance.set_time(seconds)?;
                data.mesh_snapshot(&instance, *mesh)
            })
            .transpose()?
            .flatten();
        let wind = self
            .wind
            .map(|(set, environment)| set.at_time(&environment, seconds as f32))
            .transpose()?;
        Ok(MeshSnapshots { uv, wind })
    }

    pub fn has_animated_uv(&self) -> bool { !self.uv_bindings.is_empty() }

    pub fn has_wind(&self) -> bool { !self.wind_bindings.is_empty() }

    pub fn write_at_time(&self, uniforms: &mut NvnUniformBuffers, seconds: f64) -> Result<()> {
        let snapshots = self.snapshots(seconds)?;
        if let Some(snapshot) = snapshots.uv {
            for binding in &self.uv_bindings {
                for value in snapshot.binding_writes(binding.option, binding.flags)? {
                    uniforms.write(binding.packed, u32::MAX, &value)?;
                }
            }
        }
        if let Some(snapshot) = snapshots.wind {
            let bytes = snapshot.to_bytes();
            for &packed in &self.wind_bindings {
                uniforms.write(packed, u32::MAX, &NvnUniformValue::Raw {
                    byte_offset: 0,
                    bytes: &bytes,
                })?;
            }
        }
        Ok(())
    }

    pub fn retain_bound_sources(&mut self) {
        if !self.has_animated_uv() {
            self.uv = None;
        }
        if !self.has_wind() {
            self.wind = None;
        }
    }
}
