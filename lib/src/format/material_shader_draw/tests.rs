//! Replay the bounded real-material fixture kept outside the source repository.

use std::{fs, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use remax::{
    maxwell::graphics::{self, FragmentFrequency, GraphicsProgram, QuadFaultMethod, StageSource},
    program::{
        interpolation::OffsetModel,
        wgsl::{texture::Binding, ControlFlow, FaultScope},
        Program, Register, Signature, Stage,
    },
};
use zerocopy::LittleEndian;

use super::{
    ModelShaderBindings, ModelShaderDraw, ModelShaderFrame, ModelShaderOptions,
    ModelShaderTextureSource,
};
use crate::format::{
    cmdl::{EMaterialDataId, ModelData},
    foot::locate_meta,
    material_ambient::AmbientSource,
    material_scene_bindings::{
        ReflectionProbeSnapshot, SceneMaterialInputs, SceneMaterialSnapshot, SceneTexture,
        SceneTextureView,
    },
    model_shader_inputs::{model_mesh_indices, model_vertex_inputs},
    mtrl::MaterialData,
    reflection_probe::ReflectionBrdfLut,
    shader_nvn_globals::NvnGlobalTransforms,
    shader_nvn_samples::{NvnSampleConstants, NvnSampleShading},
    texture_sampler::TextureSamplerParams,
    txtr::{ETextureFormat, ETextureType, TextureData},
};

fn words(bytes: &[u8]) -> Vec<u32> {
    assert!(bytes.len().is_multiple_of(4));
    bytes.chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect()
}

#[test]
#[ignore = "requires MPR_MATERIAL_FIXTURE pointing to the captured vine draw"]
fn prepared_vine_bindings_match_native_programs_and_requirements() -> Result<()> {
    let directory = PathBuf::from(
        std::env::var_os("MPR_MATERIAL_FIXTURE")
            .context("Set MPR_MATERIAL_FIXTURE to research/material-integration/vine")?,
    );
    let read = |name: &str| fs::read(directory.join(name));
    let model_bytes = read("model.cmdl")?;
    let model =
        ModelData::<LittleEndian>::slice(&model_bytes, locate_meta::<LittleEndian>(&model_bytes)?)?;
    let material_bytes = read("material.mtrl")?;
    let material = MaterialData::<LittleEndian>::slice(
        &material_bytes,
        locate_meta::<LittleEndian>(&material_bytes)?,
    )?;
    let payload = material.payload()?;
    let reflection = read("reflection.bin")?;
    let mut scene = SceneMaterialInputs::default();
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
    let options =
        ModelShaderOptions { technique: b'f', sample_shading: NvnSampleShading::ShaderDefault };
    let bindings = ModelShaderBindings::prepare_mesh_with_scene(
        &model,
        0,
        &payload,
        options,
        |id| {
            let bytes = read(&format!("{id}.txtr"))?;
            Ok(Some(
                TextureData::<LittleEndian>::slice(&bytes, locate_meta::<LittleEndian>(&bytes)?)?
                    .head,
            ))
        },
        &scene,
        0.,
    )?;
    assert_eq!(bindings.source_indices, [34, 34, -1, -1]);
    assert_eq!(bindings.permutation, [0xc00b10, 0]);
    assert_eq!(bindings.textures.len(), 7);
    assert_eq!(
        bindings.textures.iter().map(|texture| texture.binding.texture_index).collect::<Vec<_>>(),
        [8, 10, 12, 14, 16, 18, 20]
    );
    assert_eq!(
        bindings.textures[5].source,
        ModelShaderTextureSource::Scene(SceneTexture::Token(1))
    );
    ReflectionBrdfLut::parse(&read("brdf-rg8.bin")?)?;
    assert_eq!(bindings.textures[6].source, ModelShaderTextureSource::Scene(SceneTexture::Zero));
    assert_eq!(bindings.textures[6].format, ETextureFormat::Rgba32Uint);
    assert_eq!(SceneTexture::Zero.cache_data().unwrap().pixels, read("cache-zero.bin")?);
    assert!(!bindings.has_animated_uv());
    for (name, program) in ["vertex", "fragment"].into_iter().zip(bindings.programs()?) {
        assert_eq!(program.code.bytes, read(&format!("{name}.maxwell"))?);
        assert_eq!(
            program.header.unwrap().words.as_slice(),
            words(&read(&format!("{name}.header"))?)
        );
        assert_eq!(program.constant_data, read(&format!("{name}.literals"))?);
    }
    let transforms = NvnGlobalTransforms::parse(&read("transforms.bin")?)?;
    let samples = NvnSampleConstants::default();
    assert_eq!(samples.to_bytes().as_slice(), read("samples.bin")?);
    let memory = bindings.constants(&ModelShaderFrame::new(&transforms, samples))?;
    assert_eq!(memory.vertex.words(), words(&read("vertex.constants")?));
    assert_eq!(memory.fragment.words(), words(&read("fragment.constants")?));

    let mut programs = Vec::new();
    for (native, (stage, constants)) in bindings
        .programs()?
        .into_iter()
        .zip([(Stage::Vertex, &memory.vertex), (Stage::Fragment, &memory.fragment)])
    {
        let header = native.header.context("missing native stage header")?;
        let outputs = if stage == Stage::Vertex {
            (0..0x400)
                .step_by(4)
                .filter(|&offset| header.vtg_attribute_enabled(offset, true) == Some(true))
                .map(Register::OutputAttribute)
                .collect()
        } else {
            header
                .pixel_io()
                .context("expected fragment interface")?
                .output_registers()
                .registers()
                .map(Register::Gpr)
                .collect()
        };
        let program = Program::lift(native.code, &Signature {
            local_memory_bytes: 0,
            indexed_attributes: vec![],
            stage,
            inputs: vec![],
            outputs,
        })?;
        let requirements = program.constant_requirements();
        let report = requirements.check(constants);
        assert!(report.failures.is_empty(), "{stage:?}: {:?}", report.failures);
        if stage == Stage::Vertex {
            assert!(report.is_complete());
            assert_eq!(report.validated_accesses, 59);
        } else {
            assert!(!report.is_complete());
            assert_eq!(report.validated_accesses, 51);
            assert_eq!(report.dynamic_accesses.len(), 10);
            assert_eq!(report.dynamic_accesses[0].pc, 0x98);
            assert_eq!(report.dynamic_accesses[0].bank, 0);
            assert!(report.dynamic_accesses[1..].iter().all(|access| access.bank == 6));
        }
        eprintln!(
            "vine {stage:?}: {} fixed constant reads validated, {} dynamic accesses",
            report.validated_accesses,
            report.dynamic_accesses.len()
        );
        programs.push(requirements);
    }

    let started = Instant::now();
    let linked = link(&bindings)?;
    eprintln!(
        "vine pair linked in {:?}: {} vertex bytes / {} fragment bytes, {} varying locations",
        started.elapsed(),
        linked.vertex.source.len(),
        linked.fragment.source.len(),
        linked.varying_locations
    );
    assert_eq!(linked.fragment.kernel.control_flow, ControlFlow::ScalarDispatcher);
    assert_eq!(linked.fragment.kernel.fault_scope, FaultScope::Quad);
    for (module, constants) in
        [(&linked.vertex, &memory.vertex), (&linked.fragment, &memory.fragment)]
    {
        assert!(module.kernel.constant_requirements.check(constants).failures.is_empty());
    }

    // Compiler-specific compact uniforms and assumptions remain the legacy
    // adapter's responsibility. The shared bindings already work above.
    let draw = ModelShaderDraw::compile(bindings)?;
    let constants = draw.constants(&ModelShaderFrame::new(&transforms, samples))?;
    assert_eq!(constants.vertex_memory.words(), memory.vertex.words());
    assert_eq!(constants.fragment_memory.words(), memory.fragment.words());
    let vertices = model_vertex_inputs(
        &model,
        model.mesh.meshes[0].vtx_buf_idx as usize,
        &draw.bindings.vertex_layout,
        &draw.kernel.vertex.input_attributes,
    )?;
    assert_eq!(vertices.len(), 3385);
    let mut legacy_attributes = draw.kernel.vertex.input_attributes.clone();
    legacy_attributes.sort_unstable();
    let mut semantic_attributes = linked.attributes;
    semantic_attributes.sort_unstable();
    assert_eq!(semantic_attributes, legacy_attributes);
    assert_eq!(
        vertices.iter().flatten().copied().collect::<Vec<_>>(),
        words(&read("vertices.bin")?)
    );
    assert_eq!(model_mesh_indices(&model, 0)?, words(&read("indices.bin")?));

    // A changing draw transform is runtime data, not a frozen specialization.
    let mut moved = transforms;
    moved.model_to_clip_columns[3][0] += 1.;
    let updated = draw.constants(&ModelShaderFrame::new(&moved, samples))?;
    assert_ne!(updated.vertex_memory.words(), constants.vertex_memory.words());
    assert_ne!(updated.vertex_compact, constants.vertex_compact);
    for (requirements, memory) in
        programs.iter().zip([&updated.vertex_memory, &updated.fragment_memory])
    {
        assert!(requirements.check(memory).failures.is_empty());
    }
    assert!(ModelShaderBindings::prepare_mesh_with_scene(
        &model,
        0,
        &payload,
        options,
        |_| Ok(None),
        &scene,
        0.,
    )
    .is_err());
    Ok(())
}

fn link(bindings: &ModelShaderBindings) -> Result<GraphicsProgram> {
    let [vertex, fragment] = bindings.programs()?;
    let frequency = bindings.sample_shading().frequency(
        fragment.control.fragment_sample_shading.context("missing native fragment frequency")?,
    );
    let frequency = match frequency {
        remax::wgsl::graphics::FragmentFrequency::PerFragment => FragmentFrequency::Fragment,
        remax::wgsl::graphics::FragmentFrequency::PerSample => FragmentFrequency::Sample,
    };
    let mut options = graphics::Options::new(frequency);
    options.vertex.approximate_arithmetic = true;
    options.vertex.approximate_special_functions = true;
    options.fragment.approximate_arithmetic = true;
    options.fragment.approximate_special_functions = true;
    options.fragment.offset_interpolation = Some(OffsetModel::SignedSixteenths);
    options.quad_faults = Some(QuadFaultMethod::QuadBroadcast);
    let textures = |stage| {
        bindings
            .textures
            .iter()
            .enumerate()
            .filter(|(_, texture)| texture.stage == stage)
            .map(|(index, texture)| Binding {
                view: texture.binding,
                group: 1,
                texture_binding: 2 * index as u32,
                sampler_binding: Some(2 * index as u32 + 1),
                depth_components: None,
            })
            .collect::<Vec<_>>()
    };
    Ok(graphics::translate(
        StageSource { code: vertex.code, header: vertex.header.as_ref().context("vertex SPH")? },
        StageSource {
            code: fragment.code,
            header: fragment.header.as_ref().context("fragment SPH")?,
        },
        options,
        &textures(0),
        &textures(1),
    )?)
}

mod wind;
