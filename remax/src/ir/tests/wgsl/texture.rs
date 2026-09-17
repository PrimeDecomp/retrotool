use super::{bytes, EXIT};
mod array_views;
mod gpu;
use crate::{
    maxwell::{texture::TextureDimension as D, MaxwellCode},
    program::{
        wgsl::{
            texture::{
                Binding, DepthComponent, TextureBinding, TextureFilter, TextureSampling,
                TextureValueType as V,
            },
            FaultScope, Kernel, Options,
        },
        DiagnosticKind, Program, Register, Signature, Stage,
    },
};

pub(super) fn binding(dimension: D, value_type: V) -> Binding {
    Binding {
        view: TextureBinding {
            texture_index: 0x123,
            dimension,
            value_type,
            multisample: false,
            lod_bias: 0.0,
            sampling: None,
        },
        group: 2,
        texture_binding: 0,
        sampler_binding: Some(1),
        depth_components: None,
    }
}
fn word(load: bool, mode: u8) -> u64 {
    (if load { 0xda00000000000000 } else { 0xd800000000000000 })
        | (u64::from(mode) << 53)
        | (4 << 50)
        | (0x123 << 36)
        | (4 << 28)
        | (10 << 20)
        | (7 << 16)
        | (6 << 8)
}
fn program(words: &[u64], stage: Stage) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage,
        inputs: [0, 1, 4, 5, 6, 7, 10, 11]
            .map(Register::Gpr)
            .into_iter()
            .chain([Register::Predicate(0)])
            .collect(),
        outputs: [0, 1, 4, 5].map(Register::Gpr).to_vec(),
    })
    .unwrap()
}
pub(super) fn validate(k: &Kernel) -> Result<(), String> {
    let n = k.inputs.len().max(1);
    let fragment = k.requires_uniform_call;
    let wrapper = if fragment {
        let second = k.outputs.len().saturating_sub(1).min(1);
        format!("@fragment fn main(@builtin(position) p:vec4<f32>)->@location(0) vec4<u32>{{var words:array<u32,{n}>;var valid:array<u32,{n}>;for(var i=0u;i<{n}u;i+=1u){{words[i]=bitcast<u32>(p.x);valid[i]=select(0u,1u,p.y>0.0);}} let r=remax_main(words,valid);return vec4<u32>(r.words[0],r.words[{second}],r.status,r.pc); }}")
    } else {
        format!("@compute @workgroup_size(1) fn main(){{var words:array<u32,{n}>;var valid:array<u32,{n}>;let r=remax_main(words,valid);}}")
    };
    let source = format!("{}\n{wrapper}", k.source);
    // Old WGSL required a float LOD for ordinary depth sampling; current
    // WGSL requires an integer. The new backend follows the current contract.
    if !k
        .textures
        .iter()
        .any(|r| r.uses_sampler && !r.comparison_sampler && r.binding.view.value_type == V::Depth)
    {
        let m = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .map_err(|e| format!("{e:?}"))?;
    }
    let m = naga_next::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    naga_next::valid::Validator::new(
        naga_next::valid::ValidationFlags::all(),
        naga_next::valid::Capabilities::all(),
    )
    .validate(&m)
    .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[test]
fn supported_texture_shapes_validate_with_their_actual_wgsl_stage_requirements() {
    let samples = [
        (1, D::D2, false),
        (2, D::D2, false),
        (3, D::D2, false),
        (4, D::D2, true),
        (6, D::D2, true),
        (7, D::D2Array, false),
        (8, D::D2Array, false),
        (9, D::D2Array, true),
        (10, D::D3, false),
        (11, D::D3, false),
        (12, D::Cube, false),
        (13, D::Cube, false),
    ];
    for (mode, dim, compare) in samples {
        let b = binding(dim, if compare { V::Depth } else { V::Float });
        let p = program(&[word(false, mode), EXIT], Stage::Fragment);
        let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
        validate(&k).unwrap_or_else(|e| panic!("sample mode={mode}: {e}"));
        assert_eq!(
            k.fault_scope,
            if [1, 4, 7, 10, 12].contains(&mode) {
                FaultScope::Quad
            } else {
                FaultScope::Invocation
            }
        );
    }
    for (mode, dim, ms) in [
        (0, D::D1, false),
        (1, D::D1, false),
        (2, D::D2, false),
        (4, D::D2, false),
        (5, D::D2, false),
        (6, D::D2, true),
        (7, D::D3, false),
        (8, D::D2Array, false),
        (12, D::D2, false),
    ] {
        for ty in [V::Float, V::Uint, V::Sint] {
            let mut b = binding(dim, ty);
            b.view.multisample = ms;
            b.sampler_binding = None;
            let p = program(&[word(true, mode), EXIT], Stage::Vertex);
            let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
            validate(&k).unwrap_or_else(|e| panic!("load mode={mode} type={ty:?}: {e}"));
            assert_eq!(k.fault_scope, FaultScope::Invocation);
        }
    }
}

#[test]
fn scalar_sync_recovery_does_not_authorize_divergent_implicit_sampling() {
    use crate::ir::tests::control::{setup, sync};
    let p = program(&[setup(0, 4), sync(0), word(false, 1), sync(7), EXIT], Stage::Fragment);
    let error =
        p.emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Float)]).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Unsupported);
    assert!(error.message.contains("GPU execution analysis"));
}

#[test]
fn implicit_entry_samples_can_precede_invocation_local_branches_and_loops() {
    use crate::ir::tests::{add, branch, isetp, mov};
    for words in [vec![word(false, 1), mov(12, 1, 0), EXIT], vec![
        word(false, 1),
        mov(12, 0, 7),
        add(12, 12, 1),
        isetp(0, 7, 12, 11, 1, 7, 7),
        branch(4, 2, 0),
        EXIT,
    ]] {
        let program = program(&words, Stage::Fragment);
        let kernel = program
            .emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Float)])
            .unwrap();
        assert_eq!(kernel.fault_scope, FaultScope::Quad);
        validate(&kernel).unwrap();
        let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
        assert_eq!(body.matches("textureSampleBias(").count(), 1);
        assert!(body.find("textureSampleBias(").unwrap() < body.find("return ").unwrap());
        if let Some(dispatch) = body.find("switch block_id") {
            assert!(body.find("textureSampleBias(").unwrap() < dispatch);
        }
    }
    let repeating = program(&[word(false, 1), branch(1, 0, 0), EXIT], Stage::Fragment);
    let error = repeating
        .emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Float)])
        .unwrap_err();
    assert_eq!(error.pc, Some(8));
    assert!(error.message.contains("GPU execution analysis"));
}

#[test]
fn implicit_samples_keep_uniform_execution_in_the_presence_of_input_and_memory_faults() {
    // A direct constant read and F2I precede sampling. Their failure paths and
    // missing input flags must not make the sample non-uniform in the validator.
    let p =
        program(&[0x4c98078000070014, 0x5cb0000000670a15, word(false, 1), EXIT], Stage::Fragment);
    let k = p.emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Float)]).unwrap();
    assert!(k.requires_uniform_call);
    assert_eq!(k.fault_scope, FaultScope::Quad);
    validate(&k).unwrap();
    // Naga 29 explicitly disables fragment uniformity requirements. Check our
    // stronger no-early-return contract independently of validator acceptance.
    let module = naga_next::front::wgsl::parse_str(&k.source).unwrap();
    let (_, main) =
        module.functions.iter().find(|(_, f)| f.name.as_deref() == Some("remax_main")).unwrap();
    fn returns(block: &naga_next::Block) -> usize {
        use naga_next::Statement as S;
        block
            .iter()
            .map(|s| match s {
                S::Return { .. } => 1,
                S::Block(b) => returns(b),
                S::If { accept, reject, .. } => returns(accept) + returns(reject),
                S::Switch { cases, .. } => cases.iter().map(|c| returns(&c.body)).sum(),
                S::Loop { body, continuing, .. } => returns(body) + returns(continuing),
                _ => 0,
            })
            .sum()
    }
    assert!(returns(&main.body) >= 2);
    let body = k.source.split("fn remax_main(").nth(1).unwrap();
    let sample = body.rfind("textureSampleBias(").unwrap();
    assert!(!body[..sample].contains("return "), "diagnostics removed sampling participants");
    fn ends_in_return(block: &naga_next::Block) -> bool {
        match block.last() {
            Some(naga_next::Statement::Return { .. }) => true,
            Some(naga_next::Statement::Block(block)) => ends_in_return(block),
            _ => false,
        }
    }
    assert!(ends_in_return(&main.body));
}

#[test]
fn fixed_lod_proof_and_explicit_lod_permit_predication() {
    for fixed in [false, true] {
        let mut b = binding(D::D2, V::Float);
        if fixed {
            b.view.sampling = Some(TextureSampling {
                mip_level_count: 1,
                min_filter: TextureFilter::Nearest,
                mag_filter: TextureFilter::Nearest,
                max_anisotropy: 1,
            });
        }
        let p =
            program(&[word(false, if fixed { 1 } else { 2 }) & !(7 << 16), EXIT], Stage::Fragment);
        let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
        assert!(!k.requires_uniform_call);
        assert_eq!(k.fault_scope, FaultScope::Invocation);
        assert_eq!(k.textures[0].uses_fixed_implicit_lod, fixed);
        validate(&k).unwrap();
        if fixed {
            b.view.sampling.as_mut().unwrap().mag_filter = TextureFilter::Linear;
            assert!(p
                .emit_wgsl_with_textures(Options::default(), &[b])
                .unwrap_err()
                .message
                .contains("GPU execution analysis"));
        }
    }
}

#[test]
fn depth_reads_require_explicit_channel_mapping_and_comparisons_keep_rgba() {
    for load in [false, true] {
        let p = program(&[word(load, 2), EXIT], Stage::Fragment);
        let mut b = binding(D::D2, V::Depth);
        assert!(p
            .emit_wgsl_with_textures(Options::default(), &[b])
            .unwrap_err()
            .message
            .contains("component mapping"));
        b.depth_components = Some([
            DepthComponent::One,
            DepthComponent::Depth,
            DepthComponent::Zero,
            DepthComponent::Depth,
        ]);
        validate(&p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap()).unwrap();
    }
    let p = program(&[word(false, 6), EXIT], Stage::Fragment);
    let k = p.emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Depth)]).unwrap();
    assert!(k.textures[0].comparison_sampler);
    validate(&k).unwrap();
    let p = program(&[word(false, 5), EXIT], Stage::Fragment);
    assert!(p
        .emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Depth)])
        .unwrap_err()
        .message
        .contains("comparison LOD"));
    let mut b = binding(D::D2, V::Depth);
    b.depth_components = Some([DepthComponent::Depth; 4]);
    let implicit = program(&[word(false, 1), EXIT], Stage::Fragment);
    validate(&implicit.emit_wgsl_with_textures(Options::default(), &[b]).unwrap()).unwrap();
    let dynamic = program(&[word(false, 3), EXIT], Stage::Vertex);
    assert!(dynamic
        .emit_wgsl_with_textures(Options::default(), &[b])
        .unwrap_err()
        .message
        .contains("ordinary depth LOD"));
    b.view.lod_bias = 1.0;
    assert!(implicit
        .emit_wgsl_with_textures(Options::default(), &[b])
        .unwrap_err()
        .message
        .contains("depth sampling with bias"));
}

#[test]
fn constant_analysis_establishes_computed_depth_lod_without_freezing_inputs() {
    use crate::ir::tests::{add, mov};
    let mut binding = binding(D::D2, V::Depth);
    binding.depth_components = Some([DepthComponent::Depth; 4]);
    // TEXS.LL uses R10 as LOD. The zero here is an exact wrapped integer sum,
    // not a literal definition or a value captured from bound uniform data.
    let p = program(&[mov(10, u32::MAX, 7), add(10, 10, 1), word(false, 3), EXIT], Stage::Vertex);
    let kernel = p.emit_wgsl_with_textures(Options::default(), &[binding]).unwrap();
    validate(&kernel).unwrap();
    let dynamic = program(&[add(10, 10, 1), word(false, 3), EXIT], Stage::Vertex);
    assert!(dynamic
        .emit_wgsl_with_textures(Options::default(), &[binding])
        .unwrap_err()
        .message
        .contains("ordinary depth LOD"));
}

#[test]
fn invalid_or_ambiguous_bindings_fail_before_shader_creation() {
    let p = program(&[word(false, 2), EXIT], Stage::Vertex);
    let good = binding(D::D2, V::Float);
    assert!(p.emit_wgsl(Options::default()).is_err());
    assert!(p.emit_wgsl_with_textures(Options::default(), &[good, good]).is_err());
    for mutation in 0..6 {
        let mut b = good;
        match mutation {
            0 => b.view.dimension = D::D3,
            1 => b.view.multisample = true,
            2 => b.sampler_binding = None,
            3 => b.view.lod_bias = f32::NAN,
            4 => b.sampler_binding = Some(0),
            5 => {
                b.view.sampling = Some(TextureSampling {
                    mip_level_count: 0,
                    min_filter: TextureFilter::Linear,
                    mag_filter: TextureFilter::Linear,
                    max_anisotropy: 1,
                })
            }
            _ => unreachable!(),
        }
        let error = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::InvalidInput, "mutation={mutation}");
    }
    let p = program(&[0x4c98078000070014, word(false, 2), EXIT], Stage::Vertex);
    let mut b = good;
    b.group = 1;
    assert!(p
        .emit_wgsl_with_textures(Options::default(), &[b])
        .unwrap_err()
        .message
        .contains("share a WGSL binding"));
}
