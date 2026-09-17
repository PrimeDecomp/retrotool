//! Optional independent recompiler comparison. Fixtures and reference GPU
//! results are produced by research/local-memory without using Remax.

use std::{fs, path::Path};

use crate::{
    ir::tests::wgsl::gpu::Gpu,
    maxwell::{MaxwellCode, ShaderProgramHeader},
    program::{
        wgsl::{Options, Profile},
        Inputs, Program, Register, Signature, Stage, Value,
    },
};

#[test]
#[ignore = "requires REMAX_LOCAL_FIXTURES, independently generated Eden results and a native GPU"]
fn local_memory_matches_eden() {
    let directory = std::env::var_os("REMAX_LOCAL_FIXTURES").expect("REMAX_LOCAL_FIXTURES");
    let gpu = Gpu::new();
    let mut count = 0;
    for name in ["u8", "s8", "u16", "s16", "b32", "b64", "b128"] {
        for operation in ["load", "store"] {
            count +=
                compare_fixture(&gpu, &Path::new(&directory).join(format!("{operation}-{name}")));
        }
    }
    for name in ["fixed-b32", "fixed-b64", "fixed-b128", "sheegoth-spills"] {
        count += compare_fixture(&gpu, &Path::new(&directory).join(name));
    }
    println!("{count} local-memory output words per profile match Eden and independent byte expectations");
}

fn compare_fixture(gpu: &Gpu, directory: &Path) -> usize {
    let header =
        ShaderProgramHeader::parse(&fs::read(directory.join("vertex.header")).unwrap()).unwrap();
    assert!(header.load_or_store);
    assert_eq!(header.local_memory_high, 0);
    let outputs = (0x80..0x90)
        .step_by(4)
        .filter(|&offset| header.vtg_attribute_enabled(offset, true) == Some(true))
        .map(Register::OutputAttribute)
        .collect::<Vec<_>>();
    let code = fs::read(directory.join("vertex.maxwell")).unwrap();
    let program = Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature {
        stage: Stage::Vertex,
        local_memory_bytes: header.local_memory_low,
        outputs: outputs.clone(),
        ..Default::default()
    })
    .unwrap();
    let input_words = words(&directory.join("inputs.bin"));
    assert!(input_words.len().is_multiple_of(12));
    let inputs = input_words
        .chunks_exact(12)
        .map(|words| Inputs {
            attributes: words
                .iter()
                .enumerate()
                .map(|(index, &word)| (0x80 + index as u16 * 4, word))
                .collect(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let expected = words(&directory.join("expected.bin"));
    let reference = words(&directory.join("eden/results.bin"));
    assert_eq!(reference, expected, "independent compiler differs from byte expectations");
    assert_eq!(expected.len(), inputs.len() * outputs.len());
    for (input, expected) in inputs.iter().zip(expected.chunks_exact(outputs.len())) {
        assert_eq!(
            program.interpret(input, 1000).unwrap().into_outputs().expect("live invocation"),
            expected.iter().copied().map(Value::Word).collect::<Vec<_>>()
        );
    }
    for profile in [Profile::Checked, Profile::Rendering] {
        let kernel = program.emit_wgsl(Options { profile, ..Default::default() }).unwrap();
        let name = directory.file_name().unwrap().to_str().unwrap();
        if name.starts_with("fixed-") || name == "sheegoth-spills" {
            assert_eq!(kernel.source.contains("var local_words:"), profile == Profile::Checked);
        }
        for (index, result) in gpu.run(&kernel, &inputs).iter().enumerate() {
            assert_eq!(result[outputs.len()], 0, "GPU fault: {directory:?}, case {index}");
            assert_eq!(
                result[..outputs.len()],
                expected[index * outputs.len()..(index + 1) * outputs.len()],
                "{directory:?}, {profile:?}, case {index}"
            );
        }
    }
    expected.len()
}

fn words(path: &Path) -> Vec<u32> {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(bytes.len().is_multiple_of(4));
    bytes.chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect()
}
