use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{bail, ensure, Context, Result};
use argh::FromArgs;
use remax::wgsl::{texture::TextureBinding, F2iNanBehavior, LoweringOptions};
use retrolib::{
    format::{
        foot::locate_meta,
        material_archive::MaterialArchive,
        material_features::FeaturedMaterial,
        mtrl::MaterialData,
        rfrm::FormDescriptor,
        shader_dxbc::Dxbc,
        shader_nvn::{NvnReflection, NvnShader},
        shader_nvn_samples::NvnSampleShading,
        shader_sdx::SdxShader,
    },
    util::file::map_file,
};
use serde_json::json;
use zerocopy::LittleEndian;

#[derive(FromArgs, PartialEq, Debug)]
/// inspect and extract material shader archives
#[argh(subcommand, name = "mtrl")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Extract(ExtractArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
/// extract shader records, metadata, and a JSON index
#[argh(subcommand, name = "extract")]
struct ExtractArgs {
    #[argh(switch)]
    /// attempt experimental vertex, fragment and scalar WGSL kernel lowering
    wgsl: bool,
    #[argh(switch)]
    /// also attempt experimental scalar WGSL lowering (register kernel only)
    scalar_wgsl: bool,
    #[argh(option, default = "String::from(\"zero\")")]
    /// F2I NaN host policy: zero or indefinite; original NVN setting is unresolved
    f2i_nan_behavior: String,
    #[argh(option, default = "String::from(\"shader\")")]
    /// graphics sample-shading mode: shader (file default), sample, or fragment
    sample_shading: String,
    #[argh(option)]
    /// JSON array of explicit texture types per backend/source/stage for WGSL
    texture_bindings: Option<PathBuf>,
    #[argh(positional)]
    /// cooked MTRL with retrotool footer, or standalone MaterialArchive.arc
    input: PathBuf,
    #[argh(positional)]
    /// output directory
    output: PathBuf,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StageTextureBindings {
    backend: usize,
    source: usize,
    stage: u32,
    bindings: Vec<TextureBinding>,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Extract(args) => extract(args),
    }
}

fn extract(args: ExtractArgs) -> Result<()> {
    let sample_shading = match args.sample_shading.as_str() {
        "shader" => NvnSampleShading::ShaderDefault,
        "sample" => NvnSampleShading::PerSample,
        "fragment" => NvnSampleShading::PerFragment,
        _ => bail!("--sample-shading must be shader, sample, or fragment"),
    };
    let lowering_options = LoweringOptions {
        f2i_nan_behavior: match args.f2i_nan_behavior.as_str() {
            "zero" => F2iNanBehavior::Zero,
            "indefinite" => F2iNanBehavior::Indefinite,
            _ => bail!("--f2i-nan-behavior must be zero or indefinite"),
        },
    };
    let data = map_file(&args.input)?;
    let (desc, _, _) = FormDescriptor::<LittleEndian>::slice(&data)?;
    let cooked;
    let archive;
    let mut export_data = None;
    let mut runtime = None;
    let mut padding = &[][..];
    match desc.reader_version.get() {
        168 => {
            cooked =
                MaterialData::<LittleEndian>::slice(&data, locate_meta::<LittleEndian>(&data)?)?;
            let payload = cooked.payload()?;
            export_data = Some(payload.export_data);
            runtime = Some(payload.runtime);
            padding = payload.padding;
            archive = payload.archive;
        }
        22 => {
            let (parsed, remain) = MaterialArchive::slice(&data)?;
            ensure!(remain.is_empty(), "Unexpected trailing data after standalone archive");
            archive = parsed;
        }
        version => bail!("Unsupported MTRL reader version {version}"),
    }
    let mut texture_bindings = BTreeMap::new();
    if let Some(path) = &args.texture_bindings {
        ensure!(args.wgsl || args.scalar_wgsl, "--texture-bindings requires WGSL lowering");
        let declarations: Vec<StageTextureBindings> = serde_json::from_slice(&fs::read(path)?)
            .with_context(|| format!("invalid texture declarations in {}", path.display()))?;
        for entry in declarations {
            let backend =
                archive.backends.get(entry.backend).context("texture backend out of range")?;
            ensure!(backend.id == *b"SNVN", "texture declaration requires SNVN backend");
            let source =
                backend.sources.get(entry.source).context("texture source out of range")?;
            let shader = NvnShader::parse(source.binary)?;
            ensure!(
                shader.programs()?.iter().any(|p| p.stage.kind == entry.stage),
                "texture declaration names an absent stage"
            );
            let mut indices = std::collections::BTreeSet::new();
            for binding in &entry.bindings {
                ensure!(binding.texture_index <= 0x1fff, "texture word index out of range");
                ensure!(indices.insert(binding.texture_index), "duplicate texture word index");
            }
            ensure!(
                texture_bindings
                    .insert((entry.backend, entry.source, entry.stage), entry.bindings)
                    .is_none(),
                "duplicate texture stage declaration"
            );
        }
    }
    let stage_textures = |backend, source, stage| {
        texture_bindings.get(&(backend, source, stage)).map(Vec::as_slice).unwrap_or(&[])
    };
    fs::create_dir_all(&args.output)?;
    if let Some(export) = export_data {
        fs::write(args.output.join("export.bin"), export)?;
    }
    if let Some(runtime) = &runtime {
        let permutations = |values: &[[u64; 2]]| {
            values.iter().map(|p| p.map(|v| format!("{v:016x}"))).collect::<Vec<_>>()
        };
        let techniques: Vec<_> = runtime
            .techniques
            .iter()
            .enumerate()
            .map(|(slot, t)| {
                json!({"slot":slot, "unique_count":t.unique_count,
                "runtime_offset":t.runtime_offset, "permutation_offset":t.permutation_offset,
                "runtime_ids":t.runtime_ids.iter().map(|v|format!("{v:016x}")).collect::<Vec<_>>(),
                "permutation_indices":t.permutation_indices,
                "permutations":permutations(&t.permutations)})
            })
            .collect();
        let decoded = json!({"id":runtime.id.to_string(), "hash":format!("{:08x}",runtime.hash),
            "dynamic_feature_skip_masks":runtime.dynamic_feature_skip_masks, "runtime_id_size":runtime.runtime_id_size,
            "permutation_size":runtime.permutation_size, "unique_offset":runtime.unique_offset,
            "allocation_size":runtime.allocation_size, "allocation_alignment":runtime.allocation_alignment,
            "techniques":techniques, "unique_permutations":permutations(&runtime.unique_permutations)});
        fs::write(args.output.join("runtime.json"), serde_json::to_vec_pretty(&decoded)?)?;
    }
    if !padding.is_empty() {
        fs::write(args.output.join("padding.bin"), padding)?;
    }
    let mut backends = Vec::new();
    for (bi, backend) in archive.backends.iter().enumerate() {
        let mut sources = Vec::new();
        for (si, source) in backend.sources.iter().enumerate() {
            let binary_path = format!("backend-{bi}-source-{si}.bin");
            fs::write(args.output.join(&binary_path), source.binary)?;
            let source_path = if source.source.is_empty() {
                None
            } else {
                let path = format!("backend-{bi}-source-{si}.txt");
                fs::write(args.output.join(&path), source.source)?;
                Some(path)
            };
            let nvn = if backend.id == *b"SNVN" && !source.binary.is_empty() {
                let decoded =
                    NvnShader::parse(source.binary).with_context(|| format!("SNVN source {si}"))?;
                let gpu_path = format!("backend-{bi}-source-{si}-gpu.bin");
                let cpu_path = format!("backend-{bi}-source-{si}-cpu.bin");
                fs::write(args.output.join(&gpu_path), &decoded.gpu_pool)?;
                fs::write(args.output.join(&cpu_path), &decoded.cpu_pool)?;
                let decoded_programs = decoded.programs()?;
                let mut programs = Vec::new();
                for (pi, program) in decoded_programs.iter().enumerate() {
                    let prefix = format!("backend-{bi}-source-{si}-program-{pi}");
                    let code_path = format!("{prefix}-code.bin");
                    let instructions_path = format!("{prefix}-instructions.json");
                    fs::write(args.output.join(&code_path), program.code.bytes)?;
                    let instructions: Vec<_> = program
                        .code
                        .instructions()
                        .map(|i| {
                            json!({
                                "offset":i.offset, "word":format!("{:016x}",i.word),
                                "schedule":i.schedule, "predicate":i.predicate,
                                "predicate_inverted":i.predicate_inverted,
                                "texture_sample":i.texture_sample(),
                                "texture_load":i.texture_load(),
                                "texture_operands":i.texture_sample().and_then(|v|v.operand_registers())
                                    .or_else(||i.texture_load().and_then(|v|v.operand_registers())),
                                "constant_load":i.constant_load(),
                                "global_load":i.global_load(),
                                "alu":i.alu(),
                                "attribute":i.attribute(),
                                "float_comparison":i.float_comparison(),
                                "conversion":i.conversion(),
                                "integer":i.integer(),
                                "pixel_load":i.pixel_load(),
                                "xmad":i.xmad(),
                            })
                        })
                        .collect();
                    fs::write(
                        args.output.join(&instructions_path),
                        serde_json::to_vec_pretty(&instructions)?,
                    )?;
                    let vertex_kernel = args.wgsl && program.stage.kind == 0;
                    let fragment_kernel = args.wgsl && program.stage.kind == 1;
                    let generated_wgsl = if args.scalar_wgsl || args.wgsl {
                        let lower = if vertex_kernel {
                            remax::wgsl::lower_vertex_with_textures
                        } else if fragment_kernel {
                            remax::wgsl::lower_fragment_with_textures
                        } else {
                            remax::wgsl::lower_scalar_with_textures
                        };
                        match lower(
                            program.code,
                            lowering_options,
                            stage_textures(bi, si, program.stage.kind),
                        ) {
                            Ok(kernel) => {
                                let kind = if vertex_kernel {
                                    "vertex"
                                } else if fragment_kernel {
                                    "fragment"
                                } else {
                                    "scalar"
                                };
                                let path = format!("{prefix}-{kind}.wgsl");
                                fs::write(args.output.join(&path), &kernel.wgsl)?;
                                Some(json!({"code":path,"constant_reads":kernel.constant_reads,
                                    "indexed_constant_buffers":kernel.indexed_constant_buffers,
                                    "textures":kernel.textures,
                                    "texture_provider":"explicit caller texture views, samplers and LOD bias; descriptor result types are not inferred from shader words",
                                    "texture_fetch_bounds_policy":"zero for out-of-range coordinates, layers, levels or samples; host policy, not native NVN behavior",
                                    "constant_memory_binding":kernel.constant_memory_binding,
                                    "uses_sample_index":kernel.uses_sample_index,"uses_memory_status":kernel.uses_memory_status,
                                    "uses_fragment_kill":kernel.uses_fragment_kill,
                                    "fragment_kill_abi":"when used, append fragment_killed:ptr<function,u32> after all other arguments; initialize to zero; KIL sets one while execution continues; suppress fragment outputs afterward",
                                    "control_flow_regions":kernel.control_flow_regions,"control_flow_block_limit":kernel.control_flow_block_limit,
                                    "control_flow_fault_policy":"status bit16 on host block transition limit; caller must reject the invocation",
                                    "interpolation_gradient_slots":kernel.interpolation_gradient_slots,"interpolation_offset_model":kernel.interpolation_offset_model,
                                    "interpolation_gradient_abi":"when nonempty, append array<vec2<f32>,N> after sample/memory arguments; slots index interpolant_reads; supply pixel-center values and screen-space (dx,dy) numerator gradients",
                                    "constant_memory_abi":"32 (word_offset,byte_length) pairs then data; use ConstantMemory; invalid reads return zero and set status bits 1=bank,2=range,4=alignment",
                                    "input_registers":kernel.input_registers,"input_predicates":kernel.input_predicates,
                                    "written_registers":kernel.written_registers,"exit_offset":kernel.exit_offset,
                                    "written_predicates":kernel.written_predicates,"interpolant_reads":kernel.interpolant_reads,
                                    "defined_registers_on_exit":kernel.defined_registers_on_exit,
                                    "unavailable_registers_on_exit":kernel.unavailable_registers_on_exit,
                                    "defined_attributes_on_exit":kernel.defined_attributes_on_exit,
                                    "instruction_count":kernel.instruction_count,
                                    "input_attributes":kernel.input_attributes,"initial_output_attributes":kernel.initial_output_attributes,
                                    "written_attributes":kernel.written_attributes,"exponentiation_pairs":kernel.exponentiation_pairs,
                                    "entry_function":if vertex_kernel {"mpr_vertex"} else if fragment_kernel {"mpr_fragment"} else {"mpr_scalar"},
                                    "state_type":if vertex_kernel {"MprVertexState"} else if fragment_kernel {"MprFragmentState"} else {"MprRegisterState"},
                                    "numeric_model":"host f32; not Maxwell bit equivalence",
                                    "f2i_nan_behavior":kernel.f2i_nan_behavior,
                                    "f2i_nan_policy_note":"declared host policy; original NVN SET_SHADER_CONTROL not yet verified",
                                    "scope":if vertex_kernel {"direct vertex-attribute kernel; semantic and pipeline mapping require a wrapper"}
                                        else if fragment_kernel {"fragment register kernel; interpolation and output/resource mapping require a provider and wrapper"}
                                        else {"register kernel; graphics inputs, outputs and resources require a wrapper"}}))
                            }
                            Err(error) => Some(json!({"unsupported":format!("{error:#}")})),
                        }
                    } else {
                        None
                    };
                    let additional_path = if program.additional_gpu_data.is_empty() {
                        None
                    } else {
                        let path = format!("{prefix}-additional.bin");
                        fs::write(args.output.join(&path), program.additional_gpu_data)?;
                        Some(path)
                    };
                    let pixel_io = program.header.as_ref().and_then(|header| header.pixel_io());
                    let constant_path = if program.constant_data.is_empty() {
                        None
                    } else {
                        let path = format!("{prefix}-constants.bin");
                        fs::write(args.output.join(&path), program.constant_data)?;
                        Some(path)
                    };
                    programs.push(json!({"stage":program.stage.kind, "control":program.control,
                        "pixel_io":pixel_io,
                        "header":program.header, "code":code_path, "code_offset":program.code_offset,
                        "code_size":program.code.bytes.len(), "instruction_count":instructions.len(),
                        "instructions":instructions_path, "additional_gpu_data":additional_path,
                        "compiler_constants":{"data":constant_path,
                            "graphics_bank":(program.stage.kind != 3).then_some(1),
                            "gpu_pool_offset":program.constant_data_offset,
                            "byte_size":program.constant_data.len(),
                            "scope":"selected stage only; excludes allocation padding"},
                        "scalar_wgsl":if args.scalar_wgsl {generated_wgsl.clone()} else {None},
                        "wgsl":if args.wgsl {generated_wgsl} else {None}}));
                }
                let graphics_wgsl = if args.wgsl {
                    let graphics = (|| -> anyhow::Result<_> {
                        ensure!(
                            decoded_programs.len() == 2,
                            "graphics wrapper requires exactly vertex and pixel programs"
                        );
                        let vertex = decoded_programs
                            .iter()
                            .find(|p| p.stage.kind == 0)
                            .context("no vertex stage")?;
                        let fragment = decoded_programs
                            .iter()
                            .find(|p| p.stage.kind == 1)
                            .context("no pixel stage")?;
                        retrolib::format::shader_nvn_graphics::lower_graphics_with_textures(
                            vertex,
                            fragment,
                            retrolib::format::shader_nvn_graphics::GraphicsOptions {
                                lowering: lowering_options,
                                sample_shading,
                            },
                            stage_textures(bi, si, 0),
                            stage_textures(bi, si, 1),
                        )
                    })();
                    Some(match graphics {
                        Ok(kernel) => {
                            let path = format!("backend-{bi}-source-{si}-graphics.wgsl");
                            fs::write(args.output.join(&path), &kernel.wgsl)?;
                            json!({"code":path,"vertex_entry":"mpr_vertex_main","fragment_entry":"mpr_fragment_main",
                                "vertex_attribute_words":256,"vertex_attributes":kernel.vertex.input_attributes,
                                "vertex_constant_reads":kernel.vertex.constant_reads,"fragment_constant_reads":kernel.fragment.constant_reads,
                                "unavailable_scratch_registers":{"vertex":kernel.vertex.unavailable_registers_on_exit,"fragment":kernel.fragment.unavailable_registers_on_exit},
                                "unavailable_scratch_policy":"opaque RRO bits are unspecified; graphics observes only declared outputs",
                                "indexed_constant_buffers":{"vertex":kernel.vertex.indexed_constant_buffers,"fragment":kernel.fragment.indexed_constant_buffers},
                                "textures":{"vertex":kernel.vertex.textures,"fragment":kernel.fragment.textures},
                                "texture_provider":"explicit caller texture views, samplers and LOD bias; group2 vertex/group3 fragment",
                                "texture_fetch_bounds_policy":"zero for out-of-range coordinates, layers, levels or samples; host policy, not native NVN behavior",
                                "constant_memory_bindings":{"vertex":kernel.vertex.constant_memory_binding,"fragment":kernel.fragment.constant_memory_binding},
                                "constant_memory_fault_policy":"invalid loads clip affected vertices or discard affected fragments; host policy, not native invalid-access semantics",
                                "interpolant_reads":kernel.fragment.interpolant_reads,
                                "interpolation_gradient_slots":kernel.fragment.interpolation_gradient_slots,"interpolation_offset_model":kernel.fragment.interpolation_offset_model,
                                "interpolation_provider":"offset planes use center linear varyings and screen derivatives; sample-frequency system depth/reciprocal W are recentered from host sample positions",
                                "fragment_frequency":kernel.fragment_frequency,
                                "uses_fragment_kill":kernel.fragment.uses_fragment_kill,
                                "fragment_kill_policy":"KIL records output suppression while the kernel continues; discard occurs after translated shader execution",
                                "sample_shading_mode":sample_shading,
                                "f2i_nan_behavior":{"vertex":kernel.vertex.f2i_nan_behavior,"fragment":kernel.fragment.f2i_nan_behavior},
                                "f2i_nan_policy_note":"declared host policy; original NVN SET_SHADER_CONTROL not yet verified",
                                "storage_bindings":{"group":0,"vertex_inputs":0,"vertex_constants":1,"fragment_constants":2},
                                "clip_space":"caller supplies WGSL-compatible positions; depth0..1; no implicit Y/depth conversion",
                                "numeric_model":"host f32 and rasterizer; not Maxwell bit equivalence"})
                        }
                        Err(error) => json!({"unsupported":format!("{error:#}")}),
                    })
                } else {
                    None
                };
                Some(json!({"unknown":decoded.unknown, "stages":decoded.stages,
                    "gpu_pool":gpu_path, "gpu_pool_size":decoded.gpu_pool.len(),
                    "cpu_pool":cpu_path, "cpu_pool_size":decoded.cpu_pool.len(), "programs":programs,
                    "graphics_wgsl":graphics_wgsl}))
            } else {
                None
            };
            let sdx = if backend.id == *b"SDX " && !source.binary.is_empty() {
                let decoded =
                    SdxShader::parse(source.binary).with_context(|| format!("SDX source {si}"))?;
                let dxbc = Dxbc::parse(&decoded.dxbc)?;
                let dxbc_path = format!("backend-{bi}-source-{si}-dxbc.bin");
                let reflection_path = format!("backend-{bi}-source-{si}-dxbc.json");
                fs::write(args.output.join(&dxbc_path), &decoded.dxbc)?;
                fs::write(args.output.join(&reflection_path), serde_json::to_vec_pretty(&dxbc)?)?;
                Some(json!({"unknown":decoded.unknown, "dxbc":dxbc_path,
                    "decoded":reflection_path, "dxbc_size":decoded.dxbc.len()}))
            } else {
                None
            };
            sources.push(json!({"state":source.state, "binary":binary_path,
                "binary_size":source.binary.len(), "source":source_path, "nvn":nvn, "sdx":sdx}));
        }
        let mut materials = Vec::new();
        for (mi, material) in backend.materials.iter().enumerate() {
            let data_path = format!("backend-{bi}-material-{mi}.bin");
            fs::write(args.output.join(&data_path), material.data)?;
            let featured = FeaturedMaterial::parse(material.data)
                .with_context(|| format!("{} material {mi} ({})", backend.id, material.name))?;
            let features_path = format!("backend-{bi}-material-{mi}-features.json");
            fs::write(args.output.join(&features_path), serde_json::to_vec_pretty(&featured)?)?;
            let runtime_features_path = if let Some(runtime) =
                runtime.as_ref().filter(|r| r.id == material.id && r.hash == material.hash)
            {
                let mut layouts = Vec::new();
                for (slot, technique) in featured.techniques.iter().enumerate() {
                    if let Some(technique) = technique {
                        let skip_mask = *runtime
                            .dynamic_feature_skip_masks
                            .get(slot)
                            .context("Missing EXPT dynamic-feature mask for technique")?;
                        layouts.push(json!({"slot":slot, "skip_mask":format!("{skip_mask:08x}"),
                            "features":technique.features.dynamic_features(skip_mask)?}));
                    }
                }
                let path = format!("backend-{bi}-material-{mi}-runtime-features.json");
                fs::write(args.output.join(&path), serde_json::to_vec_pretty(&layouts)?)?;
                Some(path)
            } else {
                None
            };
            let mut shaders = Vec::new();
            for (si, shader) in material.shaders.iter().enumerate() {
                let metadata_path = format!("backend-{bi}-material-{mi}-shader-{si}.bin");
                fs::write(args.output.join(&metadata_path), shader.metadata)?;
                let nvn = if backend.id == *b"SNVN" && !shader.metadata.is_empty() {
                    let reflection = NvnReflection::parse(shader.metadata)
                        .with_context(|| format!("SNVN material {mi}, shader {si}"))?;
                    let technique = featured
                        .technique(shader.identifier.technique)
                        .context("Shader refers to an absent or unknown material technique")?;
                    let variables = technique
                        .features
                        .bind_variables(
                            shader.identifier.permutation,
                            &reflection.featured_variables()?,
                        )
                        .with_context(|| {
                            format!("SNVN material {mi}, shader {si} feature bindings")
                        })?;
                    let bindings = variables
                        .iter()
                        .map(|v| {
                            let location = reflection.variable(v.packed)?;
                            let graphics_location =
                                location.as_ref().and_then(|v| v.graphics_location());
                            let material_texture = v.material_texture_binding()?;
                            let scene_data = v.scene_binding_request()?;
                            let material_uniform = v.material_uniform_binding()?;
                            Ok(json!({"feature":v, "location":location,
                                "graphics_location":graphics_location,
                                "material_texture":material_texture,"scene_data":scene_data,
                                "material_uniform":material_uniform}))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    // A nonempty stream is dispatched by shader class. Preserve
                    // it independently, without assuming a featured-shader class.
                    let shader_data_path =
                        format!("backend-{bi}-material-{mi}-shader-{si}-data.bin");
                    fs::write(args.output.join(&shader_data_path), reflection.shader_data)?;
                    Some(json!({"reflection":reflection, "shader_data":shader_data_path,
                        "bindings":bindings, "technique_state":technique.state,
                        "override_state":technique.override_state(shader.identifier.permutation)?}))
                } else {
                    None
                };
                shaders.push(json!({
                    "technique":char::from(shader.identifier.technique).to_string(),
                    "permutation":shader.identifier.permutation.map(|v| format!("{v:016x}")),
                    "hash":format!("{:016x}",shader.hash),
                    "vertex_program":shader.vertex_program, "pixel_program":shader.pixel_program,
                    "geometry_program":shader.geometry_program, "compute_program":shader.compute_program,
                    "unknown":shader.unknown, "defines":shader.defines,
                    "defines_unknown":shader.defines_unknown, "metadata":metadata_path, "nvn":nvn,
                }));
            }
            materials.push(
                json!({"id":material.id.to_string(), "hash":format!("{:08x}",material.hash),
                "name":material.name, "unknown":material.unknown, "data":data_path,
                "features":features_path, "runtime_features":runtime_features_path, "shaders":shaders}),
            );
        }
        log::info!(
            "{}: {} sources, {} materials (TOC {})",
            backend.id,
            sources.len(),
            materials.len(),
            if backend.toc.is_some() { "validated" } else { "absent" }
        );
        backends.push(json!({"id":backend.id.to_string(), "flags":backend.flags,
            "has_toc":backend.toc.is_some(), "sources":sources, "materials":materials}));
    }
    let index = json!({"archive_version":22, "resource_version":desc.reader_version.get(),
        "export":export_data.map(|_| "export.bin"), "runtime":runtime.map(|_| "runtime.json"),
        "padding_size":padding.len(), "backends":backends});
    fs::write(args.output.join("index.json"), serde_json::to_vec_pretty(&index)?)?;
    Ok(())
}
