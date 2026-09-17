use std::collections::BTreeMap;

use crate::{
    ir::tests::bytes,
    maxwell::MaxwellCode,
    program::{
        FloatModel, Inputs, InterpretOptions, Program, Register, Signature, SpecialFunctionModel,
        Stage, Value,
    },
    resources::ConstantMemory,
};

pub(in crate::ir::tests) const IDENTITY_ROWS: [[f32; 4]; 3] =
    [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];

pub(in crate::ir::tests) struct Vertex {
    pub position: [f32; 3],
    pub instance_rows: [[f32; 4]; 3],
    pub probe_index: u32,
}

impl Vertex {
    pub fn attributes(&self) -> BTreeMap<u16, u32> {
        let mut attributes = BTreeMap::from([(0x90, self.probe_index)]);
        for (component, value) in self.position.into_iter().enumerate() {
            attributes.insert(0x80 + component as u16 * 4, value.to_bits());
        }
        for (row, values) in self.instance_rows.iter().enumerate() {
            for (component, value) in values.iter().enumerate() {
                let offset = 0xa0 + row as u16 * 16 + component as u16 * 4;
                attributes.insert(offset, value.to_bits());
            }
        }
        attributes
    }
}

pub(in crate::ir::tests) struct Transforms {
    pub clip_columns: [[f32; 4]; 4],
    pub normal_rows: [[f32; 4]; 3],
}

impl Transforms {
    pub fn constants(&self) -> ConstantMemory {
        // This shader reads only the first 112 bytes of uc_globalTransforms.
        // Leave all other constant addresses unavailable, rather than inventing
        // values for the rest of the reflected 544-byte buffer.
        let bytes = self
            .clip_columns
            .iter()
            .chain(&self.normal_rows)
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        ConstantMemory::new(&[(3, &bytes)]).unwrap()
    }
}

struct Reference {
    clip: [f64; 4],
    normal: [f64; 3],
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left.into_iter().zip(right).map(|(a, b)| a * b).sum()
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn reference(vertex: &Vertex, transforms: &Transforms) -> Reference {
    let position = vertex.position.map(f64::from);
    let instance: [[f64; 3]; 3] =
        vertex.instance_rows.map(|row| [f64::from(row[0]), f64::from(row[1]), f64::from(row[2])]);
    let world: [f64; 3] = std::array::from_fn(|row| {
        dot(instance[row], position) + f64::from(vertex.instance_rows[row][3])
    });
    let clip = std::array::from_fn(|row| {
        world
            .into_iter()
            .chain([1.])
            .zip(transforms.clip_columns)
            .map(|(value, column)| value * f64::from(column[row]))
            .sum()
    });

    // The shader derives a sphere normal from the local position. Use a
    // mathematical inverse transpose in f64, independently of its FFMA sequence.
    let length = dot(position, position).sqrt();
    let local_normal = position.map(|value| value / length);
    let cofactors = [
        cross(instance[1], instance[2]),
        cross(instance[2], instance[0]),
        cross(instance[0], instance[1]),
    ];
    let determinant = dot(instance[0], cofactors[0]);
    let world_normal = cofactors.map(|row| dot(row, local_normal) / determinant);
    let normal = transforms
        .normal_rows
        .map(|row| dot([f64::from(row[0]), f64::from(row[1]), f64::from(row[2])], world_normal));
    Reference { clip, normal }
}

#[test]
fn original_probe_vertex_transforms_position_normal_and_record_index() {
    let program = Program::lift(MaxwellCode::parse(&bytes(super::VERTEX)).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Vertex,
        outputs: [0x70, 0x74, 0x78, 0x7c, 0x80, 0x84, 0x88, 0x90]
            .map(Register::OutputAttribute)
            .to_vec(),
        ..Default::default()
    })
    .unwrap();
    let transforms = Transforms {
        clip_columns: [
            [1.25, 0.125, 0.25, 0.],
            [0.25, 0.75, -0.125, 0.],
            [0.125, -0.25, 0.5, 1.],
            [2., -1., 0.25, 0.5],
        ],
        normal_rows: [[0.5, 0.25, -0.125, 0.], [0.125, 1.5, 0.25, 0.], [-0.25, 0.125, 0.75, 0.]],
    };
    let nonuniform_scale = [[2., 0., 0., 1.], [0., 0.5, 0., -2.], [0., 0., 4., 0.5]];
    let mirrored_shear = [[-2., 0.25, 0.5, -1.], [0.5, 1.5, 0.25, 2.], [0.125, -0.25, 0.75, 3.]];
    for instance_rows in [IDENTITY_ROWS, nonuniform_scale, mirrored_shear] {
        for position in [[1., 2., 3.], [-4., 2., -1.], [0., -1., 0.], [0.25, 0.5, -0.75]] {
            for probe_index in [0, 0x10001, 0x7fc12345, u32::MAX] {
                let vertex = Vertex { position, instance_rows, probe_index };
                let expected = reference(&vertex, &transforms);
                let inputs = Inputs {
                    attributes: vertex.attributes(),
                    constants: transforms.constants(),
                    ..Default::default()
                };
                let output = program
                    .interpret_with_options(&inputs, InterpretOptions {
                        step_budget: 10000,
                        float_model: FloatModel::SoftwareIeee,
                        special_functions: SpecialFunctionModel::HostF32,
                    })
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation");
                for (component, expected) in
                    expected.clip.into_iter().chain(expected.normal).enumerate()
                {
                    let Value::Word(bits) = output[component] else { panic!("non-word output") };
                    let actual = f64::from(f32::from_bits(bits));
                    assert!(
                        (actual - expected).abs() < expected.abs().max(1.) * 3e-5,
                        "component {component}, position {position:?}, instance {instance_rows:?}: {actual} != {expected}"
                    );
                }
                assert_eq!(output[7], Value::Word(probe_index));
            }
        }
    }
}
