//! Real grass material: native mesh providers, per-view inputs and linked translation.

use super::*;
use crate::{
    format::{
        cmdl::{CMaterialData, CMaterialDataInner, EMaterialDataType},
        material_scene_bindings::DrawUniform,
        model_data::{wind::WindEnvironment, ModelDataSource, ModelDataSources},
        pack::Package,
        reflection_probe::ReflectionProbeParams,
    },
    util::file::map_file,
};

#[test]
#[ignore = "requires MPR_ROOT with the original Overworld PAKs"]
fn grass_wind_uses_mesh_data_and_updates_without_retranslation() -> Result<()> {
    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let packages = [
        "romfs/Worlds/MP1/!Over_Master/!Over_Master.pak",
        "romfs/Preload/DuplicateData.pak",
        "romfs/Worlds/MP1/universeRoomMP1.pak",
    ]
    .map(|path| map_file(root.join(path)))
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    let asset = |id| {
        packages
            .iter()
            .find_map(|pak| Package::<LittleEndian>::read_asset(pak, id).ok())
            .with_context(|| format!("Missing fixture asset {id}"))
    };
    let model_bytes = asset(uuid::uuid!("c80f28dd-dab6-45ff-a3f6-0b7dd8026292"))?;
    let model =
        ModelData::<LittleEndian>::slice(&model_bytes, locate_meta::<LittleEndian>(&model_bytes)?)?;
    let material_bytes = asset(model.mtrl.materials[0].material_resource_id)?;
    let material = MaterialData::<LittleEndian>::slice(
        &material_bytes,
        locate_meta::<LittleEndian>(&material_bytes)?,
    )?;
    let payload = material.payload()?;
    let texture_header = |id| {
        let bytes = asset(id)?;
        Ok(Some(
            TextureData::<LittleEndian>::slice(&bytes, locate_meta::<LittleEndian>(&bytes)?)?.head,
        ))
    };
    let environment = WindEnvironment::native_fallback();
    let sources = ModelDataSources::parse(&model.head.data_sources)?;
    let ModelDataSource::Wind(data) = &sources.sources[0] else { panic!("Expected WIND source") };
    let set = data.mesh_set(0)?.unwrap();
    let mut reflection = [0; 0x30];
    reflection[0x10..0x18].copy_from_slice(&1u64.to_le_bytes());
    let params = ReflectionProbeParams::default();
    for (i, value) in [
        params.max_valid_mip_index,
        params.min_intensity,
        params.max_intensity,
        params.probe_intensity,
    ]
    .into_iter()
    .enumerate()
    {
        reflection[0x20 + i * 4..0x24 + i * 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut scene = SceneMaterialInputs::default();
    scene.global_system_values = Some(Default::default());
    scene.set(
        EMaterialDataId::ALLD,
        Some(SceneMaterialSnapshot::AmbientLight(AmbientSource::full_bright())),
    )?;
    scene.set(EMaterialDataId::CLLD, None)?;
    scene.set(
        EMaterialDataId::REFP,
        Some(SceneMaterialSnapshot::ReflectionProbe(ReflectionProbeSnapshot::parse(&reflection)?)),
    )?;
    scene.set_texture_view(1, None, SceneTextureView {
        kind: ETextureType::D2,
        format: ETextureFormat::Rg8Unorm,
        sampler: TextureSamplerParams::simple(1, [0; 3], 0),
        mip_level_count: Some(1),
    });
    for (id, value) in [
        (EMaterialDataId::BKGL, [4.0, 2.0, 0.0, 0.0]),
        (EMaterialDataId::LODP, [0.0, 0.0, 0.0, 1.0]),
    ] {
        scene.set_uniform(CMaterialData {
            data_id: id,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color(value.into()),
        })?;
    }
    let prepare = |scene: &SceneMaterialInputs<'_>| {
        ModelShaderBindings::prepare_mesh_with_scene(
            &model,
            0,
            &payload,
            ModelShaderOptions { technique: b'f', sample_shading: NvnSampleShading::ShaderDefault },
            texture_header,
            scene,
            0.0,
        )
    };
    assert!(prepare(&scene).unwrap_err().to_string().contains("explicit wind environment"));
    scene.wind_environment = Some(environment);
    let fixed_camera = prepare(&scene)?;
    scene.set_draw_uniform(DrawUniform::LodParameters);
    let bindings = prepare(&scene)?;
    assert_eq!(bindings.source_indices, [130, 130, -1, -1]);
    assert!(bindings.has_wind());
    assert_eq!(bindings.wind_bounds_padding(), set.at_time(&environment, 0.0)?.bounds_padding);
    let identity4 = std::array::from_fn(|row| {
        std::array::from_fn(|column| if row == column { 1.0 } else { 0.0 })
    });
    let identity3 = [identity4[0], identity4[1], identity4[2]];
    let transforms = NvnGlobalTransforms {
        model_to_clip_columns: identity4,
        model_view_inverse_transpose_rows: identity3,
        model_to_view_rows: identity3,
        model_to_world_rows: identity3,
        world_to_model_rows: identity3,
        world_to_view_rows: identity3,
        baked_to_view_rows: identity3,
        projection_columns: identity4,
        inverse_projection_columns: identity4,
        custom0_columns: identity4,
    };
    let samples = NvnSampleConstants::default();
    let frame = |time| ModelShaderFrame {
        animation_time: Some(time),
        cull_camera_position: Some([0.0; 3]),
        ..ModelShaderFrame::new(&transforms, samples)
    };
    assert!(bindings
        .constants(&ModelShaderFrame::new(&transforms, samples))
        .unwrap_err()
        .to_string()
        .contains("culling camera"));
    assert!(bindings
        .constants(&ModelShaderFrame { cull_camera_position: Some([f32::NAN; 3]), ..frame(0.0) })
        .is_err());
    let start = bindings.constants(&frame(0.0))?;
    let fixed_start = fixed_camera.constants(&ModelShaderFrame::new(&transforms, samples))?;
    assert_eq!(start.vertex.words(), fixed_start.vertex.words());
    assert_eq!(start.fragment.words(), fixed_start.fragment.words());
    let other_view = bindings.constants(&ModelShaderFrame {
        cull_camera_position: Some([2.0, 3.0, 4.0]),
        ..frame(0.0)
    })?;
    let camera_changes: Vec<_> = start
        .vertex
        .words()
        .iter()
        .zip(other_view.vertex.words())
        .filter(|(a, b)| a != b)
        .map(|(&a, &b)| (a, b))
        .collect();
    assert_eq!(camera_changes, [2.0f32, 3.0, 4.0].map(|v| (0, v.to_bits())));
    assert_eq!(start.fragment.words(), other_view.fragment.words());
    // Updating another view does not mutate the shared prepared material.
    assert_eq!(bindings.constants(&frame(0.0))?.vertex.words(), start.vertex.words());
    let later = bindings.constants(&frame(1.0))?;
    let changed: Vec<_> = start
        .vertex
        .words()
        .iter()
        .zip(later.vertex.words())
        .filter(|(a, b)| a != b)
        .map(|(&a, &b)| (a, b))
        .collect();
    assert_eq!(changed, [(0.0f32.to_bits(), 4.0f32.to_bits())]);
    assert_eq!(start.fragment.words(), later.fragment.words());
    scene.wind_environment = None;
    scene.set(
        EMaterialDataId::WIND,
        Some(SceneMaterialSnapshot::Wind(set.at_time(&environment, 1.0)?)),
    )?;
    let fixed = prepare(&scene)?;
    assert!(!fixed.has_wind());
    assert_eq!(fixed.wind_bounds_padding(), bindings.wind_bounds_padding());
    assert_eq!(fixed.constants(&frame(99.0))?.vertex.words(), later.vertex.words());
    for profile in
        [remax::program::wgsl::Profile::Checked, remax::program::wgsl::Profile::Rendering]
    {
        let options = remax::program::wgsl::Options {
            profile,
            approximate_arithmetic: true,
            approximate_special_functions: true,
            f2i_nan_behavior: Some(remax::program::F2iNanBehavior::Zero),
            offset_interpolation: Some(OffsetModel::SignedSixteenths),
            ..Default::default()
        };
        let program = crate::format::material_shader_draw::translate(
            &bindings,
            crate::format::material_shader_draw::TranslationOptions {
                vertex: options,
                fragment: options,
                quad_faults: Some(QuadFaultMethod::QuadBroadcast),
                ..Default::default()
            },
        )?;
        for memory in [&start, &later, &other_view] {
            program.vertex.kernel.constant_preconditions.bind(&memory.vertex)?;
            program.fragment.kernel.constant_preconditions.bind(&memory.fragment)?;
        }
    }
    // Actual raster coverage and playback are checked by the GUI's MCON gate.
    Ok(())
}
