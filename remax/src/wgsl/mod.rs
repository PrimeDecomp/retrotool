//! Experimental scalar, vertex and fragment Maxwell-to-WGSL lowering.
//!
//! The generated function consumes/returns register state; it is not a complete
//! graphics pipeline. Callers supply the listed constant reads and any incoming
//! registers/predicates. Unsupported instructions fail the whole translation.
//! Only padding after an unconditional EXIT is ignored.
//!
//! Numeric contract: host f32 arithmetic and transcendental functions, explicit
//! FTZ bit handling, nearest rounding for float ALU operations. F2I/I2F use
//! integer bit algorithms for all four rounding modes, with an explicit F2I NaN
//! policy (the original NVN state value is not yet verified). This does not promise Maxwell MUFU,
//! NaN, signed-zero, subnormal, or fused-operation bit equivalence. Directed
//! rounding, DNZ/FMZ and multiply scaling remain rejected. An unconditional
//! RRO.EX2 followed by an unmodified MUFU.EX2 consumer becomes host
//! exp2, following Mesa NAK builder.rs fexp2. The intermediate is opaque:
//! ordinary reads and predicated consumption are rejected. Control-flow analysis
//! tracks possible opaque values until unconditional replacement. Standalone
//! kernels cannot expose them at EXIT; graphics kernels explicitly identify
//! unobserved scratch entries as unavailable in their returned register state.

use std::fmt::Write;

use anyhow::{bail, ensure, Context, Result};
use serde_derive::Serialize;

use crate::{
    maxwell::{
        alu::{
            AluInstruction, AluOperation, FloatOperand, FloatRounding, PredicateOperand,
            RangeReduction, SpecialFunction, ValueSource,
        },
        attribute::{
            AttributeAccess, AttributeInstruction, Interpolation, InterpolationLocation,
            InterpolationMode,
        },
        compare::{
            FloatComparison, FloatComparisonInstruction, FloatComparisonOperands, PredicateLogic,
        },
        convert::{ConversionInstruction, ConversionOperation},
        integer::{IntegerInstruction, IntegerOperation, LogicOperation},
        integer_compare::{
            IntegerComparison, IntegerComparisonInstruction, IntegerComparisonOperands,
        },
        memory::{ConstantLoad, LdcMode, LdcType},
        raster::{PixelLoad, PixelValue},
        texture::{TextureLod, TextureOperands},
        xmad::{XmadAccumulator, XmadInstruction},
        Instruction, MaxwellCode, Operation,
    },
    resources::ConstantMemoryBinding,
    wgsl::texture::{self as texture_wgsl, TextureArguments, TextureBinding, TextureResource},
};

pub mod graphics;
pub mod texture;

mod attributes;
mod control;
use attributes::{AddressPair, KnownWord};
pub use attributes::{AttributeAddressUse, ConstantAssumption};

use crate::resources::{ConstantMemory, ConstantRead};

/// Host execution guard per closed control region. Exceeding it sets status bit16;
/// graphics wrappers discard/clip that invocation instead of returning a valid draw.
pub const CONTROL_FLOW_BLOCK_LIMIT: u32 = 65_536;

pub use crate::program::F2iNanBehavior;

#[derive(Clone, Copy, Debug, Default)]
pub struct LoweringOptions {
    pub f2i_nan_behavior: F2iNanBehavior,
}

/// One value supplied by a fragment interpolation provider. Perspective values
/// are the rasterizer's numerator (attribute / clip W, interpolated linearly);
/// the IPA instruction applies its multiplier inside the generated kernel.
/// Constant values preserve raw bits. Location is explicit, including centroid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct InterpolantRead {
    pub byte_offset: u16,
    pub mode: InterpolationMode,
    pub location: InterpolationLocation,
}

/// Experimental offset model inferred from MPR's sample-table packing,
/// uam/NAK's4096-scale encoding, NVK's four fractional bits, and SM75 GL probes.
/// Raw SM53 execution and arbitrary high/low ignored bits remain unverified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum InterpolationOffsetModel {
    /// X/Y are signed nibbles in bits8..11/24..27, measured in sixteenths of a
    /// pixel from its center. Other bits do not affect this host model.
    SignedSixteenths,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProgramKernel {
    pub wgsl: String,
    /// Selected NaN policy when F2I is used; None means the kernel does not use it.
    pub f2i_nan_behavior: Option<F2iNanBehavior>,
    /// Entries in the generated function's constants array, in order.
    pub constant_reads: Vec<ConstantRead>,
    /// Bound address selectors used to lift compiler-generated indirect inputs.
    /// Both the caller and generated kernel must retain these exact values.
    pub constant_assumptions: Vec<ConstantAssumption>,
    pub attribute_address_uses: Vec<AttributeAddressUse>,
    /// Opaque AL2P predicate results left in unobserved graphics scratch state.
    pub unavailable_predicates_on_exit: Vec<u8>,
    /// Hardware banks accessed by LDC. Bind ConstantMemory at the reported
    /// storage binding; compact ALU reads must refer to the same bank bytes.
    pub indexed_constant_buffers: Vec<u8>,
    pub constant_memory_binding: Option<ConstantMemoryBinding>,
    /// Adds a `sample_index: u32` argument after compact constants. This is the
    /// actual invocation index, not a request to change shading frequency.
    pub uses_sample_index: bool,
    /// LDC/control-flow kernels append `memory_status: ptr<function,u32>` after
    /// any sample argument. Bits1/2/4/8 report bank/range/alignment/unknown-byte
    /// faults; bit16 reports the host control-transition limit and bit32 a changed
    /// address-selection constant. Failed reads
    /// return zero. These host faults do not emulate invalid Maxwell execution.
    pub uses_memory_status: bool,
    /// Fragment KIL appends `fragment_killed: ptr<function,u32>` after all other
    /// optional arguments. Initialize it to zero; KIL sets it to one without
    /// ending execution. The graphics caller suppresses output after the kernel.
    /// This preserves later derivative sampling for the supported pure operations;
    /// it does not emulate arbitrary helper-lane observations or side effects.
    pub uses_fragment_kill: bool,
    /// Number of closed SSY regions lowered with per-invocation control flow.
    pub control_flow_regions: usize,
    /// Explicit host transition limit, reported only for control-flow kernels.
    pub control_flow_block_limit: Option<u32>,
    /// Register sources used before an unconditional assignment in this kernel.
    pub input_registers: Vec<u8>,
    pub input_predicates: Vec<u8>,
    pub written_registers: Vec<u8>,
    pub written_predicates: Vec<u8>,
    /// Values assigned on every possible EXIT path. A graphics wrapper must
    /// not silently initialize missing shader outputs to zero.
    pub defined_registers_on_exit: Vec<u8>,
    /// Graphics-only kernels can leave opaque scratch registers unobserved.
    /// These returned register entries are unspecified and must not be read.
    /// Standalone scalar/vertex/fragment kernels require all raw entries valid.
    pub unavailable_registers_on_exit: Vec<u8>,
    pub defined_attributes_on_exit: Vec<u16>,
    /// Graphics-only output stores omitted because the pixel stage does not
    /// consume them. Their returned raw state must not be observed.
    pub unavailable_attributes_on_exit: Vec<u16>,
    pub instruction_count: usize,
    pub exit_offset: usize,
    /// Direct input/output attribute byte offsets used by vertex kernels.
    pub input_attributes: Vec<u16>,
    pub initial_output_attributes: Vec<u16>,
    pub written_attributes: Vec<u16>,
    pub exponentiation_pairs: usize,
    /// Entries in a fragment kernel's interpolants array, in order.
    pub interpolant_reads: Vec<InterpolantRead>,
    /// Indices into interpolant_reads needing screen gradients. Their scalar
    /// interpolant is the pixel-center value; append array<vec2<f32>,N> to the
    /// kernel arguments (after sample/memory arguments), with (dx,dy) per pixel
    /// in the caller's window coordinate system. Perspective values and their
    /// gradients describe the numerator, before the original IPA multiplier.
    pub interpolation_gradient_slots: Vec<usize>,
    pub interpolation_offset_model: Option<InterpolationOffsetModel>,
    /// Explicit typed host textures. Group2 for vertex/scalar, group3 for pixel.
    /// The caller supplies matching views and samplers; TEXS output types cannot
    /// be inferred from instruction words alone. TLDS out-of-range fetches
    /// return zero as an explicit host policy, without claiming native behavior.
    pub textures: Vec<TextureResource>,
}

pub type ScalarKernel = ProgramKernel;

#[derive(Default)]
struct Emitter {
    options: LoweringOptions,
    attribute_constants: Option<ConstantMemory>,
    known_words: Vec<(u8, KnownWord)>,
    known_predicates: Vec<(u8, KnownWord)>,
    address_pairs: Vec<AddressPair>,
    address_predicates: Vec<(u8, Option<usize>)>,
    replaced_predicates: Vec<u8>,
    exit_unavailable_predicates: Vec<u8>,
    constant_assumptions: Vec<ConstantAssumption>,
    attribute_address_uses: Vec<AttributeAddressUse>,
    uses_f2i: bool,
    constants: Vec<ConstantRead>,
    indexed_constants: Vec<u8>,
    uses_sample_index: bool,
    uses_fragment_kill: bool,
    inputs: Vec<u8>,
    predicates: Vec<u8>,
    definite_predicates: Vec<u8>,
    written_predicates: Vec<u8>,
    written: Vec<u8>,
    definite: Vec<u8>,
    exit_registers: Option<Vec<u8>>,
    exit_attributes: Option<Vec<u16>>,
    observed_registers: Option<Vec<u8>>,
    observed_attributes: Option<Vec<u16>>,
    omitted_output_attributes: Vec<u16>,
    exit_unavailable: Vec<u8>,
    body: String,
    vertex: bool,
    fragment: bool,
    interpolants: Vec<InterpolantRead>,
    interpolation_gradient_slots: Vec<usize>,
    input_attributes: Vec<u16>,
    initial_output_attributes: Vec<u16>,
    written_attributes: Vec<u16>,
    definite_attributes: Vec<u16>,
    /// Captured inputs for opaque RRO results. Matching MUFU consumers can
    /// write another register; the intermediate remains opaque until an
    /// unconditional overwrite. None marks a possibly opaque value inherited
    /// across control flow, whose producer cannot be used in this block.
    range_values: Vec<(u8, Option<String>)>,
    replaced_registers: Vec<u8>,
    exponentiation_pairs: usize,
    texture_bindings: Vec<TextureBinding>,
    textures: Vec<TextureResource>,
    conditional_exit: bool,
    in_control_region: bool,
    control_flow_regions: usize,
}

impl Emitter {
    fn texture_arguments(&mut self, operands: TextureOperands) -> Result<TextureArguments> {
        let mut result = TextureArguments::default();
        for r in operands.coordinates.into_iter().flatten() {
            result.coordinates.push(self.source(ValueSource::Register(r))?);
        }
        let mut read = |r: Option<u8>| r.map(|r| self.source(ValueSource::Register(r))).transpose();
        result.layer = read(operands.layer)?;
        result.lod = read(operands.lod)?;
        result.comparison = read(operands.comparison)?;
        result.offset = read(operands.texel_offset)?;
        result.sample = read(operands.sample_index)?;
        Ok(result)
    }

    fn texture(&mut self, operation: Operation, always: bool, guard: &str) -> Result<()> {
        let (index, dimension, multisample, implicit, operands, outputs) = match operation {
            Operation::TextureSample(t) => {
                let mode = t.mode.context("unknown TEXS mode")?;
                (
                    t.texture_index,
                    mode.dimension,
                    false,
                    mode.lod == TextureLod::Implicit,
                    t.operand_registers(),
                    t.component_registers(),
                )
            }
            Operation::TextureLoad(t) => {
                let mode = t.mode.context("unknown TLDS mode")?;
                (
                    t.texture_index,
                    mode.dimension,
                    mode.multisample,
                    false,
                    t.operand_registers(),
                    t.component_registers(),
                )
            }
            _ => unreachable!(),
        };
        let operands = operands.context("unsupported texture operand register span")?;
        let outputs = outputs.context("unsupported texture result mask")?;
        let slot = texture_wgsl::resource(
            &self.texture_bindings,
            &mut self.textures,
            index,
            dimension,
            multisample,
            matches!(operation, Operation::TextureSample(_)),
            implicit,
            self.fragment,
        )?;
        ensure!(!implicit || self.fragment, "implicit texture sampling requires a fragment kernel");
        let derivatives = implicit && !self.textures[slot].binding.implicit_lod_is_fixed();
        ensure!(
            !derivatives || (always && !self.conditional_exit && !self.in_control_region),
            "implicit texture derivatives require straight-line unconditional fragment flow without earlier conditional EXIT"
        );
        let args = self.texture_arguments(operands)?;
        let body = match operation {
            Operation::TextureSample(t) => format!(
                "let result={};\n",
                texture_wgsl::sample_call(t, &args, slot, &self.textures[slot])?
            ),
            Operation::TextureLoad(t) => {
                texture_wgsl::load_call(t, &args, slot, &self.textures[slot])?
            }
            _ => unreachable!(),
        };
        // All arguments and all result channels are captured before any aliased
        // destination is written. Predication covers the resource access itself.
        if always {
            writeln!(self.body, "{{")?;
        } else {
            writeln!(self.body, "if ({guard}) {{")?;
        }
        self.body.push_str(&body);
        for (component, r) in outputs.into_iter().enumerate() {
            if let Some(r) = r {
                self.write_register(r, &format!("result[{component}]"), always, "true")?;
            }
        }
        writeln!(self.body, "}}")?;
        Ok(())
    }

    fn pixel_load(&mut self, p: PixelLoad, always: bool, guard: &str) -> Result<()> {
        ensure!(self.fragment, "PIXLD requires a fragment kernel");
        ensure!(p.unrecognized_bits == 0, "unrecognized PIXLD bits");
        ensure!(
            p.value == Some(PixelValue::MyIndex)
                && p.address_register == 255
                && p.predicate_destination == 7,
            "PIXLD mode/address/predicate requires additional raster state"
        );
        self.uses_sample_index = true;
        self.write_register(p.destination, "sample_index", always, guard)?;
        Ok(())
    }

    fn xmad(&mut self, x: XmadInstruction) -> Result<String> {
        ensure!(x.unrecognized_bits == 0, "unrecognized XMAD bits");
        ensure!(
            !x.extended && !x.write_condition_code,
            "XMAD carry/condition-code state is not lowered"
        );
        let accumulator = x.accumulator.context("unrecognized XMAD accumulator mode")?;
        ensure!(accumulator != XmadAccumulator::SignCorrection, "XMAD.CSFU is not lowered");
        let raw =
            [self.source(x.sources[0])?, self.source(x.sources[1])?, self.source(x.sources[2])?];
        let parts: [String; 2] = std::array::from_fn(|i| {
            let half = if x.high_half[i] {
                format!("({} >> 16u)", raw[i])
            } else {
                format!("({} & 65535u)", raw[i])
            };
            if x.signed[i] {
                format!("bitcast<u32>(bitcast<i32>({half} << 16u) >> 16u)")
            } else {
                half
            }
        });
        let mut product = format!("({} * {})", parts[0], parts[1]);
        if x.product_shift_left {
            product = format!("({product} << 16u)");
        }
        let c = match accumulator {
            XmadAccumulator::Low => format!("({} & 65535u)", raw[2]),
            XmadAccumulator::High => format!("({} >> 16u)", raw[2]),
            _ => raw[2].clone(),
        };
        let mut value = format!("({product} + {c})");
        if accumulator == XmadAccumulator::MergeCarry {
            value = format!("({value} + ({} << 16u))", raw[1]);
        }
        if x.merge {
            value = format!("(({value} & 65535u) | ({} << 16u))", raw[1]);
        }
        Ok(value)
    }

    fn constant_load(&mut self, load: ConstantLoad, always: bool, guard: &str) -> Result<()> {
        ensure!(load.unrecognized_bits == 0, "unrecognized LDC bits");
        ensure!(load.mode == LdcMode::Indexed, "segmented/linear LDC addressing is not lowered");
        let data_type = load.data_type.context("unknown LDC size")?;
        let size = data_type.byte_len();
        // A vector result must remain in R0..R254. RZ is a discard destination.
        ensure!(
            size != 8 || load.destination == 255 || load.destination < 254,
            "LDC register span crosses RZ"
        );
        let address = self.source(ValueSource::Register(load.address_register))?;
        if !self.indexed_constants.contains(&load.buffer) {
            self.indexed_constants.push(load.buffer);
        }
        // Evaluate the address and both loaded words before either destination
        // write. The address register may alias the first destination.
        writeln!(self.body, "{}{{", if always { String::new() } else { format!("if ({guard}) ") })?;
        writeln!(
            self.body,
            "let loaded = mpr_ldc({}u, ({address} + 0x{:08x}u), {size}u, memory_status);",
            load.buffer,
            i32::from(load.displacement) as u32
        )?;
        let value = if matches!(data_type, LdcType::I8 | LdcType::I16) {
            let shift = 32 - u32::from(size) * 8;
            format!("bitcast<u32>(bitcast<i32>(loaded.x << {shift}u) >> {shift}u)")
        } else {
            "loaded.x".into()
        };
        self.write_register(load.destination, &value, always, "true")?;
        if size == 8 && load.destination != 255 {
            self.write_register(load.destination + 1, "loaded.y", always, "true")?;
        }
        writeln!(self.body, "}}")?;
        Ok(())
    }

    fn conversion(&mut self, c: ConversionInstruction) -> Result<String> {
        ensure!(
            c.unrecognized_bits == 0,
            "unrecognized conversion bits {:#x}",
            c.unrecognized_bits
        );
        ensure!(!c.write_condition_code, "conversion condition-code writes are not lowered");
        ensure!(c.selector == 0, "conversion source selection is not lowered");
        let rounding = rounding_code(c.rounding);
        match c.operation {
            ConversionOperation::FloatToFloat => {
                bail!("F2F requires the semantic compiler's floating-rounding model")
            }
            ConversionOperation::FloatToInteger => {
                ensure!(
                    c.source_bits == 32 && c.destination_bits == 32,
                    "F2I requires F32 and a 32-bit destination"
                );
                let bits = self.float_bits(
                    FloatOperand { source: c.source, absolute: c.absolute, negate: c.negate },
                    c.flush_to_zero,
                )?;
                self.uses_f2i = true;
                let nan = if self.options.f2i_nan_behavior == F2iNanBehavior::Indefinite {
                    0x80000000u32
                } else {
                    0
                };
                Ok(format!("mpr_f2i({bits}, {}, {rounding}u, 0x{nan:08x}u)", c.signed))
            }
            ConversionOperation::IntegerToFloat => {
                ensure!(
                    c.destination_bits == 32 && c.source_bits <= 32,
                    "I2F requires an integer of at most32 bits and F32 destination"
                );
                ensure!(!c.absolute && !c.negate, "I2F integer source modifiers are not lowered");
                let mut bits = self.source(c.source)?;
                if c.source_bits < 32 {
                    let shift = 32 - c.source_bits;
                    bits = if c.signed {
                        format!("bitcast<u32>(bitcast<i32>({bits} << {shift}u) >> {shift}u)")
                    } else {
                        format!("({bits} & {}u)", (1u32 << c.source_bits) - 1)
                    };
                }
                Ok(format!("mpr_i2f({bits}, {}, {rounding}u)", c.signed))
            }
        }
    }

    fn integer(&mut self, i: IntegerInstruction, always: bool, guard: &str) -> Result<()> {
        ensure!(i.unrecognized_bits == 0, "unrecognized integer bits {:#x}", i.unrecognized_bits);
        ensure!(
            !i.write_condition_code && !i.extended,
            "integer carry/condition-code state is not lowered"
        );
        // PASS_B does not consume its first source.
        let a = if matches!(i.operation, IntegerOperation::Logic {
            operation: LogicOperation::PassB,
            ..
        }) {
            "0u".to_string()
        } else {
            self.source(i.sources[0])?
        };
        let b = self.source(i.sources[1])?;
        let mut predicate = None;
        let expression = match i.operation {
            IntegerOperation::BitFieldExtract { signed, reverse } => {
                if reverse {
                    ensure!(
                        matches!(i.sources[1],ValueSource::Immediate(v) if v&65535==0x2000),
                        "partial or dynamic BFE.BREV needs independent validation"
                    );
                }
                let a = if reverse { format!("reverseBits({a})") } else { a };
                format!("mpr_bfe({a},{b},{signed})")
            }
            IntegerOperation::FindLeading { signed, shift_amount, invert } => {
                let b = if invert { format!("~({b})") } else { b };
                format!("mpr_flo({b},{signed},{shift_amount})")
            }
            IntegerOperation::Add { negate, saturate } => {
                ensure!(!saturate, "saturated integer addition is not lowered");
                ensure!(negate != [true, true], "IADD.PO is not lowered");
                let a = if negate[0] { format!("(0u - {a})") } else { a };
                let b = if negate[1] { format!("(0u - {b})") } else { b };
                format!("({a} + {b})")
            }
            IntegerOperation::ScaledAdd { .. } => {
                anyhow::bail!("ISCADD requires the semantic compiler")
            }
            IntegerOperation::Logic {
                operation,
                invert,
                predicate_mode,
                predicate_destination,
            } => {
                ensure!(predicate_mode != 1, "reserved LOP predicate mode is not lowered");
                ensure!(
                    predicate_mode != 0 || predicate_destination == 7,
                    "LOP predicate mode0 writes are not lowered"
                );
                if predicate_mode >= 2 && predicate_destination != 7 {
                    predicate = Some((predicate_destination, predicate_mode == 3));
                }
                let a = if invert[0] { format!("(~{a})") } else { a };
                let b = if invert[1] { format!("(~{b})") } else { b };
                match operation {
                    LogicOperation::PassB => b,
                    _ => format!("({a} {} {b})", match operation {
                        LogicOperation::And => "&",
                        LogicOperation::Or => "|",
                        _ => "^",
                    }),
                }
            }
            IntegerOperation::Shift { right, signed, wrap } => {
                let shift = format!("({b} & 31u)");
                let value = if right && signed {
                    format!("bitcast<u32>(bitcast<i32>({a}) >> {shift})")
                } else {
                    format!("({a} {} {shift})", if right { ">>" } else { "<<" })
                };
                if wrap {
                    value
                } else {
                    let overflow = if right && signed {
                        format!("bitcast<u32>(bitcast<i32>({a}) >> 31u)")
                    } else {
                        "0u".into()
                    };
                    format!("select({value}, {overflow}, {b} >= 32u)")
                }
            }
        };
        if let Some((r, nonzero)) = predicate {
            // Snapshot once: destination GPR can alias a source and destination
            // predicate can also be the execution guard.
            writeln!(
                self.body,
                "{}{{\nlet result = {expression};",
                if always { String::new() } else { format!("if ({guard}) ") }
            )?;
            self.write_register(i.destination, "result", always, "true")?;
            writeln!(
                self.body,
                "s.predicates[{r}] = select(0u, 1u, result {} 0u);\n}}",
                if nonzero { "!=" } else { "==" }
            )?;
            self.replace_predicate(r, always)?;
            if always && !self.definite_predicates.contains(&r) {
                self.definite_predicates.push(r);
            }
            if !self.written_predicates.contains(&r) {
                self.written_predicates.push(r);
            }
        } else {
            self.write_register(i.destination, &expression, always, guard)?;
        }
        Ok(())
    }

    fn source(&mut self, value: ValueSource) -> Result<String> {
        Ok(match value {
            ValueSource::Register(255) => "0u".into(),
            ValueSource::Register(r) => {
                ensure!(
                    !self.range_values.iter().any(|(register, _)| *register == r)
                        && !self.address_pairs.iter().any(|p| p.register == r),
                    "opaque RRO/AL2P intermediate is read outside a matching consumer"
                );
                if !self.definite.contains(&r) && !self.inputs.contains(&r) {
                    self.inputs.push(r);
                }
                format!("s.registers[{r}]")
            }
            ValueSource::Immediate(bits) => format!("0x{bits:08x}u"),
            ValueSource::Constant { buffer, byte_offset } => {
                ensure!(byte_offset >= 0, "negative constant-buffer address");
                let value = ConstantRead { buffer, byte_offset: byte_offset as u16 };
                let slot = match self.constants.iter().position(|v| *v == value) {
                    Some(slot) => slot,
                    None => {
                        self.constants.push(value);
                        self.constants.len() - 1
                    }
                };
                format!("constants[{slot}]")
            }
        })
    }

    fn float_bits(&mut self, value: FloatOperand, ftz: bool) -> Result<String> {
        let mut bits = self.source(value.source)?;
        if value.absolute {
            bits = format!("({bits} & 0x7fffffffu)");
        }
        if value.negate {
            bits = format!("({bits} ^ 0x80000000u)");
        }
        if ftz {
            bits = format!("mpr_flush({bits})");
        }
        Ok(bits)
    }

    fn float(&mut self, value: FloatOperand, ftz: bool) -> Result<String> {
        Ok(format!("bitcast<f32>({})", self.float_bits(value, ftz)?))
    }

    fn predicate(&mut self, value: PredicateOperand) -> Result<String> {
        ensure!(
            !self.address_predicates.iter().any(|(p, _)| *p == value.register),
            "opaque AL2P predicate is read outside its paired IPA"
        );
        let base = if value.register == 7 {
            "true".into()
        } else {
            if !self.definite_predicates.contains(&value.register)
                && !self.predicates.contains(&value.register)
            {
                self.predicates.push(value.register);
            }
            format!("(s.predicates[{}] != 0u)", value.register)
        };
        Ok(if value.inverted { format!("!{base}") } else { base })
    }

    fn alu(&mut self, alu: AluInstruction) -> Result<String> {
        ensure!(
            alu.unrecognized_bits == 0,
            "unrecognized ALU bits {:#018x}",
            alu.unrecognized_bits
        );
        ensure!(!alu.write_condition_code, "condition-code writes are not lowered");
        let (mut expression, saturate, ftz) = match alu.operation {
            AluOperation::Move { source, lane_mask } => {
                ensure!(lane_mask == 15, "partial MOV lane mask {lane_mask:#x}");
                return self.source(source);
            }
            AluOperation::FAdd { sources: [a, b], rounding, flush_to_zero, saturate } => {
                nearest(rounding)?;
                (
                    format!(
                        "({} + {})",
                        self.float(a, flush_to_zero)?,
                        self.float(b, flush_to_zero)?
                    ),
                    saturate,
                    flush_to_zero,
                )
            }
            AluOperation::FMul {
                sources: [a, b],
                rounding,
                flush_to_zero,
                dnz,
                saturate,
                scale_code,
                ..
            } => {
                nearest(rounding)?;
                ensure!(!dnz, "DNZ/FMZ multiplication is not lowered");
                ensure!(scale_code == 0, "scaled multiplication is not lowered");
                (
                    format!(
                        "({} * {})",
                        self.float(a, flush_to_zero)?,
                        self.float(b, flush_to_zero)?
                    ),
                    saturate,
                    flush_to_zero,
                )
            }
            AluOperation::FFma { sources: [a, b, c], rounding, flush_to_zero, dnz, saturate } => {
                nearest(rounding)?;
                ensure!(!dnz, "DNZ/FMZ fused multiplication is not lowered");
                (
                    format!(
                        "fma({}, {}, {})",
                        self.float(a, flush_to_zero)?,
                        self.float(b, flush_to_zero)?,
                        self.float(c, flush_to_zero)?
                    ),
                    saturate,
                    flush_to_zero,
                )
            }
            AluOperation::FMinMax { sources: [a, b], min, flush_to_zero } => {
                let a = self.float(a, flush_to_zero)?;
                let b = self.float(b, flush_to_zero)?;
                (
                    format!("select(max({a}, {b}), min({a}, {b}), {})", self.predicate(min)?),
                    false,
                    flush_to_zero,
                )
            }
            AluOperation::Mufu { source, function, saturate, .. } => {
                let value = self.float(source, false)?;
                let expression = match function {
                    Some(SpecialFunction::Exp2) => format!("exp2({value})"),
                    Some(SpecialFunction::Log2) => format!("log2({value})"),
                    Some(SpecialFunction::Reciprocal) => format!("(1.0 / {value})"),
                    Some(SpecialFunction::ReciprocalSqrt) => format!("inverseSqrt({value})"),
                    Some(SpecialFunction::Sqrt) => format!("sqrt({value})"),
                    _ => bail!("MUFU {function:?} is not lowered"),
                };
                (expression, saturate, false)
            }
            AluOperation::Rro { .. } => bail!("range reduction is not lowered"),
        };
        if saturate {
            expression = format!("clamp({expression}, 0.0, 1.0)");
        }
        expression = format!("bitcast<u32>({expression})");
        if ftz {
            expression = format!("mpr_flush({expression})");
        }
        Ok(expression)
    }

    fn comparison_relation(&mut self, op: FloatComparisonOperands) -> Result<String> {
        ensure!(
            op.unrecognized_bits == 0,
            "unrecognized comparison bits {:#x}",
            op.unrecognized_bits
        );
        let a = self.float_bits(op.sources[0], op.flush_to_zero)?;
        let b = self.float_bits(op.sources[1], op.flush_to_zero)?;
        let nan = format!("(mpr_bits_is_nan({a}) || mpr_bits_is_nan({b}))");
        let equal = format!("mpr_float_equal_bits({a}, {b})");
        // Native host comparisons can flush subnormal inputs even without FTZ.
        // Ordered integer keys preserve them and keep +/-zero equal explicitly.
        let less = format!("(!{equal} && mpr_float_order_key({a}) < mpr_float_order_key({b}))");
        let greater = format!("(!{equal} && mpr_float_order_key({a}) > mpr_float_order_key({b}))");
        use FloatComparison::*;
        Ok(match op.comparison {
            False => "false".into(),
            True => "true".into(),
            IsNumber => format!("!{nan}"),
            IsNan => nan.clone(),
            compare => {
                let relation = match compare {
                    Less | LessUnordered => less,
                    Equal | EqualUnordered => equal,
                    LessEqual | LessEqualUnordered => format!("({equal} || {less})"),
                    Greater | GreaterUnordered => greater,
                    NotEqual | NotEqualUnordered => format!("!{equal}"),
                    GreaterEqual | GreaterEqualUnordered => format!("({equal} || {greater})"),
                    _ => unreachable!(),
                };
                let unordered = matches!(
                    compare,
                    LessUnordered
                        | EqualUnordered
                        | LessEqualUnordered
                        | GreaterUnordered
                        | NotEqualUnordered
                        | GreaterEqualUnordered
                );
                if unordered {
                    format!("({nan} || {relation})")
                } else {
                    format!("(!{nan} && {relation})")
                }
            }
        })
    }

    fn comparison(&mut self, op: FloatComparisonOperands) -> Result<String> {
        let relation = self.comparison_relation(op)?;
        let accumulator = self.predicate(op.accumulator)?;
        let operator = predicate_operator(op.logic)?;
        Ok(format!("({relation} {operator} {accumulator})"))
    }

    fn set_predicates(
        &mut self,
        destination: u8,
        secondary: u8,
        operands: FloatComparisonOperands,
        always: bool,
        guard: &str,
    ) -> Result<()> {
        ensure!(
            destination == 7 || destination != secondary,
            "aliased comparison predicate destinations are not lowered"
        );
        let relation = self.comparison_relation(operands)?;
        let accumulator = self.predicate(operands.accumulator)?;
        let operator = predicate_operator(operands.logic)?;
        self.write_predicates(
            destination,
            secondary,
            &relation,
            &accumulator,
            operator,
            always,
            guard,
        )
    }

    #[allow(clippy::too_many_arguments)] // Explicit predicate operands and execution guard.
    fn write_predicates(
        &mut self,
        destination: u8,
        secondary: u8,
        relation: &str,
        accumulator: &str,
        operator: &str,
        always: bool,
        guard: &str,
    ) -> Result<()> {
        ensure!(
            destination == 7 || destination != secondary,
            "aliased comparison predicate destinations are not lowered"
        );
        // Both destinations, the accumulator and the execution guard can alias.
        // Capture the inputs before either write and evaluate the guard once.
        // The second output combines !comparison with accumulator, not !output0.
        if always {
            writeln!(self.body, "{{")?;
        } else {
            writeln!(self.body, "if ({guard}) {{")?;
        }
        writeln!(self.body, "let comparison = {relation};\nlet accumulator = {accumulator};")?;
        for (r, value) in [(destination, "comparison"), (secondary, "!comparison")] {
            if r == 7 {
                continue;
            }
            writeln!(
                self.body,
                "s.predicates[{r}] = select(0u, 1u, {value} {operator} accumulator);"
            )?;
            self.replace_predicate(r, always)?;
            if always && !self.definite_predicates.contains(&r) {
                self.definite_predicates.push(r);
            }
            if !self.written_predicates.contains(&r) {
                self.written_predicates.push(r);
            }
        }
        writeln!(self.body, "}}")?;
        Ok(())
    }

    fn integer_relation(&mut self, op: IntegerComparisonOperands) -> Result<String> {
        ensure!(op.unrecognized_bits == 0, "unrecognized integer comparison bits");
        ensure!(!op.extended, "extended integer comparison carry state is not lowered");
        let mut a = self.source(op.sources[0])?;
        let mut b = self.source(op.sources[1])?;
        if op.signed {
            a = format!("bitcast<i32>({a})");
            b = format!("bitcast<i32>({b})");
        }
        let cmp = match op.comparison {
            IntegerComparison::False => return Ok("false".into()),
            IntegerComparison::True => return Ok("true".into()),
            IntegerComparison::Less => "<",
            IntegerComparison::Equal => "==",
            IntegerComparison::LessEqual => "<=",
            IntegerComparison::Greater => ">",
            IntegerComparison::NotEqual => "!=",
            IntegerComparison::GreaterEqual => ">=",
        };
        Ok(format!("({a} {cmp} {b})"))
    }

    fn write_register(&mut self, r: u8, expression: &str, always: bool, guard: &str) -> Result<()> {
        if r == 255 {
            return Ok(());
        }
        ensure!(
            always
                || (!self.range_values.iter().any(|(register, _)| *register == r)
                    && !self.address_pairs.iter().any(|p| p.register == r)),
            "predicated write would mix ordinary and opaque RRO/AL2P values"
        );
        self.known_words.retain(|(register, _)| *register != r);
        if always {
            self.address_pairs.retain(|p| p.register != r);
            self.range_values.retain(|(register, _)| *register != r);
            if self.in_control_region && !self.replaced_registers.contains(&r) {
                self.replaced_registers.push(r);
            }
            writeln!(self.body, "s.registers[{r}] = {expression};")?;
            if !self.definite.contains(&r) {
                self.definite.push(r);
            }
        } else {
            writeln!(self.body, "if ({guard}) {{ s.registers[{r}] = {expression}; }}")?;
        }
        if !self.written.contains(&r) {
            self.written.push(r);
        }
        Ok(())
    }

    fn attribute(
        &mut self,
        access: AttributeAccess,
        load: bool,
        always: bool,
        guard: &str,
    ) -> Result<()> {
        ensure!(self.vertex, "attribute access requires a vertex kernel");
        ensure!(
            access.unrecognized_bits == 0,
            "unrecognized attribute bits {:#x}",
            access.unrecognized_bits
        );
        ensure!(!access.patch, "patch attributes are not lowered");
        ensure!(access.address_register == 255, "physical attribute addressing is not lowered");
        ensure!(access.vertex_register == 255, "cross-vertex attribute addressing is not lowered");
        ensure!(
            access.byte_offset.is_multiple_of(4)
                && u32::from(access.byte_offset) + u32::from(access.components) * 4 <= 0x400,
            "attribute access exceeds or misaligns the direct attribute space"
        );
        ensure!(
            u16::from(access.data_register) + u16::from(access.components) <= 256,
            "attribute register group wraps past RZ"
        );
        for component in 0..access.components {
            let offset = access.byte_offset + u16::from(component) * 4;
            let slot = offset / 4;
            let register = access.data_register + component;
            if !load && self.observed_attributes.as_ref().is_some_and(|v| !v.contains(&offset)) {
                if !self.omitted_output_attributes.contains(&offset) {
                    self.omitted_output_attributes.push(offset);
                }
                continue;
            }
            if load {
                let expression = if access.output_flag {
                    ensure!(
                        !self.omitted_output_attributes.contains(&offset),
                        "ALD.O reads an output omitted by graphics liveness analysis"
                    );
                    if !self.definite_attributes.contains(&offset)
                        && !self.initial_output_attributes.contains(&offset)
                    {
                        self.initial_output_attributes.push(offset);
                    }
                    format!("s.attributes[{slot}]")
                } else {
                    if !self.input_attributes.contains(&offset) {
                        self.input_attributes.push(offset);
                    }
                    format!("input_attributes[{slot}]")
                };
                self.write_register(register, &expression, always, guard)?;
            } else {
                let expression = self.source(ValueSource::Register(register))?;
                if always {
                    writeln!(self.body, "s.attributes[{slot}] = {expression};")?;
                    if !self.definite_attributes.contains(&offset) {
                        self.definite_attributes.push(offset);
                    }
                } else {
                    writeln!(self.body, "if ({guard}) {{ s.attributes[{slot}] = {expression}; }}")?;
                }
                if !self.written_attributes.contains(&offset) {
                    self.written_attributes.push(offset);
                }
            }
        }
        Ok(())
    }

    fn interpolate(&mut self, ipa: Interpolation, always: bool, guard: &str) -> Result<()> {
        ensure!(self.fragment, "interpolation requires a fragment kernel");
        ensure!(ipa.unrecognized_bits == 0, "unrecognized interpolation bits");
        ensure!(!ipa.indexed, "indexed interpolation is not lowered");
        ensure!(
            ipa.predicate.register == 7 && !ipa.predicate.inverted,
            "interpolation with a nontrivial trailing predicate is not verified"
        );
        ensure!(
            ipa.address_register == 255,
            "direct interpolation requires an RZ address register"
        );
        ensure!(ipa.byte_offset.is_multiple_of(4), "unaligned interpolation attribute");
        ensure!(
            ipa.mode != InterpolationMode::State,
            "state-selected interpolation is not lowered"
        );
        let location = ipa.location.context("unknown interpolation location")?;
        ensure!(
            location != InterpolationLocation::Offset || ipa.mode != InterpolationMode::Constant,
            "constant-mode offset interpolation is not verified"
        );
        // Direct IPA encodes RZ. nvdisasm rejects a non-RZ address without IDX.
        let read = InterpolantRead { byte_offset: ipa.byte_offset, mode: ipa.mode, location };
        let slot = match self.interpolants.iter().position(|v| *v == read) {
            Some(slot) => slot,
            None => {
                self.interpolants.push(read);
                self.interpolants.len() - 1
            }
        };
        let bits = if location == InterpolationLocation::Offset {
            let gradient = match self.interpolation_gradient_slots.iter().position(|v| *v == slot) {
                Some(index) => index,
                None => {
                    self.interpolation_gradient_slots.push(slot);
                    self.interpolation_gradient_slots.len() - 1
                }
            };
            let offset = self.source(ValueSource::Register(ipa.offset_register))?;
            format!("bitcast<u32>(mpr_interpolate_offset(bitcast<f32>(interpolants[{slot}]), interpolation_gradients[{gradient}], {offset}))")
        } else {
            format!("interpolants[{slot}]")
        };
        let mut value = format!("bitcast<f32>({bits})");
        if ipa.mode == InterpolationMode::Perspective {
            let multiplier = self.source(ValueSource::Register(ipa.multiplier_register))?;
            value = format!("({value} * bitcast<f32>({multiplier}))");
        }
        if ipa.saturate {
            value = format!("clamp({value}, 0.0, 1.0)");
        }
        let expression = if ipa.mode == InterpolationMode::Perspective || ipa.saturate {
            format!("bitcast<u32>({value})")
        } else {
            bits
        };
        self.write_register(ipa.destination, &expression, always, guard)?;
        Ok(())
    }

    fn instruction(&mut self, i: Instruction) -> Result<bool> {
        let predicate = PredicateOperand { register: i.predicate, inverted: i.predicate_inverted };
        let mut always = predicate.register == 7 && !predicate.inverted;
        let operation = i.operation();
        // ANUV's packed selector chooses XY versus ZW with a LOP predicate.
        // Fold only a physical ALD guard with a known, still-valid definition.
        if !always
            && matches!(operation, Operation::Attribute(AttributeInstruction::Load(a)) if a.physical())
        {
            if let Some(execute) = self.known_attribute_guard(predicate) {
                if !execute {
                    return Ok(false);
                }
                always = true;
            }
        }
        let guard = if always { "true".into() } else { self.predicate(predicate)? };
        let known_predicate = self.known_logic_predicate(operation, always);
        let known_result = self.known_result(operation, always);
        if operation == Operation::Exit {
            for register in self
                .range_values
                .iter()
                .map(|(r, _)| *r)
                .chain(self.address_pairs.iter().map(|p| p.register))
            {
                ensure!(
                    self.observed_registers.as_ref().is_some_and(|r| !r.contains(&register)),
                    "EXIT could expose an opaque RRO/AL2P intermediate in R{register}"
                );
                if !self.exit_unavailable.contains(&register) {
                    self.exit_unavailable.push(register);
                }
            }
            for &(p, _) in &self.address_predicates {
                ensure!(
                    self.observed_registers.is_some(),
                    "EXIT could expose an opaque AL2P predicate"
                );
                if !self.exit_unavailable_predicates.contains(&p) {
                    self.exit_unavailable_predicates.push(p);
                }
            }
            if predicate.register != 7 || !predicate.inverted {
                intersect_exits(&mut self.exit_registers, &self.definite);
                intersect_exits(&mut self.exit_attributes, &self.definite_attributes);
            }
            if always {
                writeln!(self.body, "return s;")?;
            } else {
                self.conditional_exit = true;
                writeln!(self.body, "if ({guard}) {{ return s; }}")?;
            }
            return Ok(always);
        }
        match operation {
            Operation::Nop => (),
            Operation::FragmentKill(kill) => {
                ensure!(self.fragment, "KIL requires a fragment-stage kernel");
                ensure!(
                    kill.condition == 15 && kill.unrecognized_bits == 0,
                    "unsupported KIL condition or modifier bits"
                );
                self.uses_fragment_kill = true;
                // Keep evaluating subsequent pure shader operations, including
                // texture derivatives. Suppress outputs at the graphics boundary.
                writeln!(self.body, "if ({guard}) {{ *fragment_killed = 1u; }}")?;
            }
            Operation::DependencyBarrier(barrier) => {
                ensure!(
                    barrier.unrecognized_bits == 0
                        && (!barrier.less_equal || barrier.scoreboard < 6),
                    "unsupported dependency barrier"
                );
                // All currently translated loads/samples produce their value
                // synchronously. Waiting on their scoreboard has no extra effect.
            }
            Operation::Conversion(c) => {
                let value = self.conversion(c)?;
                self.write_register(c.destination, &value, always, &guard)?;
            }
            Operation::Integer(i) => self.integer(i, always, &guard)?,
            Operation::IntegerComparison(compare) => {
                let operands = match compare {
                    IntegerComparisonInstruction::Set { operands, .. }
                    | IntegerComparisonInstruction::SetPredicates { operands, .. } => operands,
                };
                let relation = self.integer_relation(operands)?;
                let accumulator = self.predicate(operands.accumulator)?;
                let operator = predicate_operator(operands.logic)?;
                match compare {
                    IntegerComparisonInstruction::Set {
                        destination,
                        boolean_float,
                        write_condition_code,
                        ..
                    } => {
                        ensure!(
                            !write_condition_code,
                            "comparison condition-code writes are not lowered"
                        );
                        let true_bits = if boolean_float { "0x3f800000u" } else { "0xffffffffu" };
                        self.write_register(
                            destination,
                            &format!("select(0u,{true_bits},{relation} {operator} {accumulator})"),
                            always,
                            &guard,
                        )?;
                    }
                    IntegerComparisonInstruction::SetPredicates {
                        destination,
                        secondary_destination,
                        ..
                    } => self.write_predicates(
                        destination,
                        secondary_destination,
                        &relation,
                        &accumulator,
                        operator,
                        always,
                        &guard,
                    )?,
                }
            }
            Operation::PixelLoad(p) => self.pixel_load(p, always, &guard)?,
            Operation::Xmad(x) => {
                let value = self.xmad(x)?;
                self.write_register(x.destination, &value, always, &guard)?;
            }
            Operation::ConstantLoad(load) => self.constant_load(load, always, &guard)?,
            Operation::TextureSample(_) | Operation::TextureLoad(_) => {
                self.texture(operation, always, &guard)?
            }
            Operation::Alu(alu) => {
                if let AluOperation::Rro { source, reduction: RangeReduction::Exp2 } = alu.operation
                {
                    ensure!(
                        always && alu.destination != 255,
                        "predicated/discarded RRO is not lowered"
                    );
                    ensure!(alu.unrecognized_bits == 0, "unrecognized RRO bits");
                    let source = self.float(source, false)?;
                    let name = format!("mpr_exp2_input_{:x}", i.offset);
                    writeln!(self.body, "let {name} = {source};")?;
                    self.known_words.retain(|(r, _)| *r != alu.destination);
                    self.address_pairs.retain(|p| p.register != alu.destination);
                    self.range_values.retain(|(r, _)| *r != alu.destination);
                    self.range_values.push((alu.destination, Some(name)));
                    self.definite.retain(|r| *r != alu.destination);
                    if self.in_control_region && !self.replaced_registers.contains(&alu.destination)
                    {
                        self.replaced_registers.push(alu.destination);
                    }
                    if !self.written.contains(&alu.destination) {
                        self.written.push(alu.destination);
                    }
                    return Ok(false);
                }
                if let AluOperation::Mufu {
                    source:
                        FloatOperand { source: ValueSource::Register(register), absolute, negate },
                    function: Some(SpecialFunction::Exp2),
                    saturate,
                    ..
                } = alu.operation
                {
                    if let Some(index) = self.range_values.iter().position(|(r, _)| *r == register)
                    {
                        ensure!(
                            always && !absolute && !negate && alu.unrecognized_bits == 0,
                            "RRO requires an unconditional, unmodified MUFU.EX2 consumer"
                        );
                        let name = self.range_values[index]
                            .1
                            .as_ref()
                            .context("MUFU consumes an opaque RRO value across control flow")?
                            .clone();
                        let value = if saturate {
                            format!("clamp(exp2({name}), 0.0, 1.0)")
                        } else {
                            format!("exp2({name})")
                        };
                        self.write_register(
                            alu.destination,
                            &format!("bitcast<u32>({value})"),
                            always,
                            &guard,
                        )?;
                        self.exponentiation_pairs += 1;
                        return Ok(false);
                    }
                }
                let expression = self.alu(alu)?;
                self.write_register(alu.destination, &expression, always, &guard)?;
            }
            Operation::FloatComparison(FloatComparisonInstruction::Set {
                destination,
                boolean_float,
                write_condition_code,
                operands,
            }) => {
                ensure!(!write_condition_code, "comparison condition-code writes are not lowered");
                let condition = self.comparison(operands)?;
                let true_bits = if boolean_float { "0x3f800000u" } else { "0xffffffffu" };
                self.write_register(
                    destination,
                    &format!("select(0u, {true_bits}, {condition})"),
                    always,
                    &guard,
                )?;
            }
            Operation::FloatComparison(FloatComparisonInstruction::SetPredicates {
                destination,
                secondary_destination,
                operands,
            }) => {
                self.set_predicates(destination, secondary_destination, operands, always, &guard)?
            }
            Operation::Attribute(AttributeInstruction::Load(access)) => {
                let access = self.lift_attribute_load(access, true, i.offset)?;
                self.attribute(access, true, always, &guard)?
            }
            Operation::Attribute(AttributeInstruction::Store(access)) => {
                let access = self.lift_attribute_load(access, false, i.offset)?;
                self.attribute(access, false, always, &guard)?
            }
            Operation::Attribute(AttributeInstruction::Interpolate(ipa)) => {
                let ipa = self.lift_interpolation(ipa, always, i.offset)?;
                self.interpolate(ipa, always, &guard)?
            }
            Operation::Attribute(AttributeInstruction::Address(a)) => {
                self.attribute_address(a, always, i.offset)?;
            }
            _ => bail!("unsupported instruction family"),
        }
        if let Some((p, value)) = known_predicate {
            self.known_predicates.push((p, value));
        }
        if let Some((r, value)) = known_result {
            if r != 255 {
                self.known_words.push((r, value));
            }
        }
        Ok(false)
    }
}

fn predicate_operator(logic: Option<PredicateLogic>) -> Result<&'static str> {
    Ok(match logic.context("unknown comparison predicate combiner")? {
        PredicateLogic::And => "&&",
        PredicateLogic::Or => "||",
        PredicateLogic::Xor => "!=",
    })
}

fn intersect_exits<T: Clone + PartialEq>(exits: &mut Option<Vec<T>>, current: &[T]) {
    if let Some(exits) = exits {
        exits.retain(|value| current.contains(value));
    } else {
        *exits = Some(current.to_vec());
    }
}

fn nearest(rounding: FloatRounding) -> Result<()> {
    ensure!(
        rounding == FloatRounding::NearestEven,
        "directed rounding {rounding:?} is not lowered"
    );
    Ok(())
}

fn rounding_code(rounding: FloatRounding) -> u8 {
    match rounding {
        FloatRounding::NearestEven => 0,
        FloatRounding::NegativeInfinity => 1,
        FloatRounding::PositiveInfinity => 2,
        FloatRounding::Zero => 3,
    }
}

/// Lower a whole scalar instruction stream up to its unconditional EXIT.
/// Function names start with mpr_; callers may append graphics/compute wrappers.
/// The constants argument has max(1, constant_reads.len()) elements.
/// LDC adds the manifest's storage binding and a final memory-status pointer.
pub fn lower_scalar(code: MaxwellCode<'_>) -> Result<ScalarKernel> {
    lower_scalar_with_options(code, LoweringOptions::default())
}

pub fn lower_scalar_with_options(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
) -> Result<ScalarKernel> {
    lower_scalar_with_textures(code, options, &[])
}

pub fn lower_scalar_with_textures(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
    textures: &[TextureBinding],
) -> Result<ScalarKernel> {
    lower(code, false, false, options, textures, None, None, None)
}

/// Direct vertex attribute access plus the scalar operation subset. The caller
/// supplies256 input attribute words; the returned state includes256 output
/// attribute words. Semantic/buffer/vertex-layout mapping remains the caller's
/// responsibility. ALD.O dependencies on prior output values are reported.
pub fn lower_vertex(code: MaxwellCode<'_>) -> Result<ProgramKernel> {
    lower_vertex_with_options(code, LoweringOptions::default())
}

pub fn lower_vertex_with_options(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
) -> Result<ProgramKernel> {
    lower_vertex_with_textures(code, options, &[])
}

pub fn lower_vertex_with_textures(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
    textures: &[TextureBinding],
) -> Result<ProgramKernel> {
    lower(code, true, false, options, textures, None, None, None)
}

/// Fragment register kernel with explicit interpolation inputs. The caller
/// supplies center/centroid/flat values according to interpolant_reads, including
/// raw perspective numerators and fragment reciprocal W. The instruction stream
/// performs its own reciprocal and perspective multiplication. Interpolants and
/// constants each have max(1, manifest.len()) elements. Output register mapping,
/// textures and graphics wrappers remain separate work. Offset inputs provide
/// their center value here and append screen gradients to the kernel arguments.
/// PIXLD.MY_INDEX adds a sample_index argument after constants. LDC appends a
/// memory_status pointer after it (or directly after constants if PIXLD is not
/// used). See ProgramKernel and ConstantMemory for resource layout/fault rules.
pub fn lower_fragment(code: MaxwellCode<'_>) -> Result<ProgramKernel> {
    lower_fragment_with_options(code, LoweringOptions::default())
}

pub fn lower_fragment_with_options(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
) -> Result<ProgramKernel> {
    lower_fragment_with_textures(code, options, &[])
}

pub fn lower_fragment_with_textures(
    code: MaxwellCode<'_>,
    options: LoweringOptions,
    textures: &[TextureBinding],
) -> Result<ProgramKernel> {
    lower(code, false, true, options, textures, None, None, None)
}

/// Internal graphics wrapper contract: vertex returns attributes, pixel returns
/// the single RGBA target verified by wgsl::graphics (R0..R3).
pub(super) fn lower_graphics_stage(
    code: MaxwellCode<'_>,
    vertex: bool,
    options: LoweringOptions,
    textures: &[TextureBinding],
    attribute_constants: Option<&ConstantMemory>,
    observed_attributes: Option<&[u16]>,
) -> Result<ProgramKernel> {
    lower(
        code,
        vertex,
        !vertex,
        options,
        textures,
        Some(if vertex { &[] } else { &[0, 1, 2, 3] }),
        attribute_constants,
        observed_attributes,
    )
}

#[allow(clippy::too_many_arguments)] // Existing register-kernel backend contract.
fn lower(
    code: MaxwellCode<'_>,
    vertex: bool,
    fragment: bool,
    options: LoweringOptions,
    textures: &[TextureBinding],
    observed_registers: Option<&[u8]>,
    attribute_constants: Option<&ConstantMemory>,
    observed_attributes: Option<&[u16]>,
) -> Result<ProgramKernel> {
    let mut emitter = Emitter {
        vertex,
        fragment,
        options,
        texture_bindings: textures.to_vec(),
        attribute_constants: attribute_constants.cloned(),
        observed_attributes: observed_attributes.map(<[u16]>::to_vec),
        observed_registers: observed_registers.map(<[u8]>::to_vec),
        ..Default::default()
    };
    let mut skip_until = 0;
    for (index, i) in code.instructions().enumerate() {
        if i.offset < skip_until {
            continue;
        }
        if matches!(
            i.control_flow().map(|f| f.operation),
            Some(crate::maxwell::control::ControlOperation::Push {
                kind: crate::maxwell::control::StackKind::Sync,
                ..
            })
        ) {
            let region = crate::maxwell::control::ReconvergenceRegion::build(code, i)?;
            emitter.control_region(&region)?;
            skip_until = region.end_offset;
            continue;
        }
        let done = emitter.instruction(i).with_context(|| {
            format!("Maxwell instruction at {:#x}, word {:#018x}", i.offset, i.word)
        })?;
        if !done {
            continue;
        }
        let state = if vertex {
            "MprVertexState"
        } else if fragment {
            "MprFragmentState"
        } else {
            "MprRegisterState"
        };
        let attributes = if vertex { ", attributes: array<u32, 256>" } else { "" };
        let function = if vertex {
            "mpr_vertex"
        } else if fragment {
            "mpr_fragment"
        } else {
            "mpr_scalar"
        };
        let input_attributes = if vertex {
            "input_attributes: array<u32, 256>, ".into()
        } else if fragment {
            format!("interpolants: array<u32, {}>, ", emitter.interpolants.len().max(1))
        } else {
            String::new()
        };
        let constant_memory_binding = (!emitter.indexed_constants.is_empty())
            .then_some(ConstantMemoryBinding { group: 1, binding: u32::from(fragment) });
        let resource_helpers = constant_memory_binding
            .map(|b| include_str!("resources.wgsl").replace("__BINDING__", &b.binding.to_string()))
            .unwrap_or_default();
        let mut resource_arguments = String::new();
        if emitter.uses_sample_index {
            resource_arguments.push_str(", sample_index: u32");
        }
        let uses_memory_status = constant_memory_binding.is_some()
            || emitter.control_flow_regions != 0
            || !emitter.constant_assumptions.is_empty();
        if uses_memory_status {
            resource_arguments.push_str(", memory_status: ptr<function,u32>");
        }
        let gradient_count = emitter.interpolation_gradient_slots.len();
        if gradient_count != 0 {
            write!(
                resource_arguments,
                ", interpolation_gradients: array<vec2<f32>,{gradient_count}>"
            )?;
        }
        if emitter.uses_fragment_kill {
            resource_arguments.push_str(", fragment_killed: ptr<function,u32>");
        }
        let mut assumption_checks = String::new();
        for assumption in &emitter.constant_assumptions {
            let slot = emitter
                .constants
                .iter()
                .position(|r| *r == assumption.read)
                .context("address assumption was not recorded as a constant read")?;
            writeln!(
                assumption_checks,
                "if (constants[{slot}] != {}u) {{ *memory_status |= 32u; }}",
                assumption.value
            )?;
        }
        let wgsl = format!(
            r#"struct {state} {{ registers: array<u32, 255>, predicates: array<u32, 7>{attributes} }};
fn mpr_flush(bits: u32) -> u32 {{
    return select(bits, bits & 0x80000000u, (bits & 0x7f800000u) == 0u);
}}
fn mpr_bits_is_nan(bits: u32) -> bool {{
    return (bits & 0x7f800000u) == 0x7f800000u && (bits & 0x007fffffu) != 0u;
}}
fn mpr_float_equal_bits(a: u32, b: u32) -> bool {{
    return a == b || ((a | b) & 0x7fffffffu) == 0u;
}}
fn mpr_float_order_key(bits: u32) -> u32 {{
    return select(bits ^ 0x80000000u, ~bits, (bits & 0x80000000u) != 0u);
}}
{integer_helpers}
{resource_helpers}
{texture_declarations}
fn mpr_interpolate_offset(center:f32, gradient:vec2<f32>, packed:u32)->f32 {{
    // SignedSixteenths host model; see InterpolationOffsetModel provenance.
    let x=bitcast<i32>(packed << 20u) >> 28u;
    let y=bitcast<i32>(packed << 4u) >> 28u;
    let offset=vec2<f32>(f32(x),f32(y))*0.0625;
    return center + gradient.x*offset.x + gradient.y*offset.y;
}}
fn {function}(initial: {state}, {input_attributes}constants: array<u32, {}>{resource_arguments}) -> {state} {{
var s = initial;
{assumption_checks}
{}
}}
"#,
            emitter.constants.len().max(1),
            emitter.body,
            integer_helpers = include_str!("integer.wgsl"),
            texture_declarations = emitter
                .textures
                .iter()
                .enumerate()
                .map(|(slot, t)| t.declarations(slot))
                .collect::<String>(),
        );
        return Ok(ProgramKernel {
            wgsl,
            f2i_nan_behavior: emitter.uses_f2i.then_some(emitter.options.f2i_nan_behavior),
            constant_reads: emitter.constants,
            constant_assumptions: emitter.constant_assumptions,
            attribute_address_uses: emitter.attribute_address_uses,
            unavailable_predicates_on_exit: emitter.exit_unavailable_predicates,
            indexed_constant_buffers: emitter.indexed_constants,
            constant_memory_binding,
            uses_sample_index: emitter.uses_sample_index,
            uses_memory_status,
            uses_fragment_kill: emitter.uses_fragment_kill,
            control_flow_regions: emitter.control_flow_regions,
            control_flow_block_limit: (emitter.control_flow_regions != 0)
                .then_some(CONTROL_FLOW_BLOCK_LIMIT),
            input_registers: emitter.inputs,
            input_predicates: emitter.predicates,
            written_registers: emitter.written,
            written_predicates: emitter.written_predicates,
            defined_registers_on_exit: emitter.exit_registers.unwrap_or_default(),
            unavailable_registers_on_exit: emitter.exit_unavailable,
            defined_attributes_on_exit: emitter.exit_attributes.unwrap_or_default(),
            unavailable_attributes_on_exit: emitter.omitted_output_attributes,
            instruction_count: index + 1,
            exit_offset: i.offset,
            input_attributes: emitter.input_attributes,
            initial_output_attributes: emitter.initial_output_attributes,
            written_attributes: emitter.written_attributes,
            exponentiation_pairs: emitter.exponentiation_pairs,
            interpolant_reads: emitter.interpolants,
            interpolation_gradient_slots: emitter.interpolation_gradient_slots,
            interpolation_offset_model: (gradient_count != 0)
                .then_some(InterpolationOffsetModel::SignedSixteenths),
            textures: emitter.textures,
        });
    }
    bail!("program has no unconditional EXIT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_kill_preserves_later_execution_and_validates_its_modes() {
        for predicate in 0..16u64 {
            let raw = bytes(&[
                0xe33000000000000f | (predicate << 16),
                0x5c9807800ff70003, // MOV R3,RZ after KIL
                0xe30000000007000f,
            ]);
            let code = MaxwellCode::parse(&raw).unwrap();
            let kernel = lower_fragment(code).unwrap();
            assert!(kernel.uses_fragment_kill);
            assert_eq!(kernel.defined_registers_on_exit, [3]);
            assert_eq!(kernel.exit_offset, 0x18);
            assert!(!kernel.wgsl.contains("discard;"));
            assert!(lower_scalar(code).is_err());
            assert!(lower_vertex(code).is_err());
        }
        for condition in 0..32u64 {
            let raw = bytes(&[0xe330000000070000 | condition, 0xe30000000007000f]);
            assert_eq!(lower_fragment(MaxwellCode::parse(&raw).unwrap()).is_ok(), condition == 15);
        }
        for bit in (5..16).chain(20..48) {
            let raw = bytes(&[0xe33000000007000f | (1 << bit), 0xe30000000007000f]);
            assert!(lower_fragment(MaxwellCode::parse(&raw).unwrap()).is_err());
        }
        // KIL retains helper execution, so subsequent unconditional implicit
        // sampling does not become a divergent early-exit operation.
        let raw = bytes(&[0xe33000000000000f, 0xd830008061070804, 0xe30000000007000f]);
        let binding = TextureBinding {
            texture_index: 8,
            dimension: crate::maxwell::texture::TextureDimension::D2,
            value_type: crate::wgsl::texture::TextureValueType::Float,
            multisample: false,
            lod_bias: 0.0,
            sampling: None,
        };
        let kernel = lower_fragment_with_textures(
            MaxwellCode::parse(&raw).unwrap(),
            LoweringOptions::default(),
            &[binding],
        )
        .unwrap();
        assert!(kernel.uses_fragment_kill);
        assert!(kernel.wgsl.contains("textureSampleBias"));
    }

    #[test]
    fn indexed_resources_and_invocation_inputs_are_explicit() {
        let raw = bytes(&[0xefe8e0028007ff00, 0xef95003000070404, 0xe30000000007000f]);
        let k = lower_fragment(MaxwellCode::parse(&raw).unwrap()).unwrap();
        assert!(k.uses_sample_index && k.uses_memory_status);
        assert_eq!(k.indexed_constant_buffers, [3]);
        assert_eq!(k.constant_memory_binding, Some(ConstantMemoryBinding { group: 1, binding: 1 }));
        assert_eq!(k.input_registers, [4]); // LDC address read before R4/R5 write
        assert!(k.defined_registers_on_exit.contains(&4));
        assert!(k.defined_registers_on_exit.contains(&5));
        assert!(lower_vertex(MaxwellCode::parse(&raw).unwrap()).is_err());
        let predicated = bytes(&[0xef95003000000404, 0xe30000000007000f]);
        let k = lower_fragment(MaxwellCode::parse(&predicated).unwrap()).unwrap();
        assert_eq!(k.input_predicates, [0]);
        assert!(k.defined_registers_on_exit.is_empty());
    }

    #[test]
    fn textures_require_unambiguous_typed_bindings() {
        use crate::{maxwell::texture::TextureDimension, wgsl::texture::TextureValueType};
        let raw = bytes(&[0xd850008061070804, 0xe30000000007000f]); // TEXS.2D.LZ RGBA
        let code = MaxwellCode::parse(&raw).unwrap();
        let binding = TextureBinding {
            texture_index: 8,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            lod_bias: 0.0,
            sampling: None,
        };
        let options = LoweringOptions::default();
        assert!(lower_scalar(code).is_err());
        let k = lower_scalar_with_textures(code, options, &[binding]).unwrap();
        assert_eq!(k.textures.len(), 1);
        assert_eq!(k.textures[0].binding, binding);
        assert_eq!(k.textures[0].group, 2);
        assert_eq!(k.textures[0].sampler_binding, Some(1));
        assert!(!k.textures[0].uses_implicit_lod);
        assert!(lower_scalar_with_textures(code, options, &[binding, binding]).is_err());
        for wrong in [
            TextureBinding { texture_index: 10, ..binding },
            TextureBinding { dimension: TextureDimension::D2Array, ..binding },
            TextureBinding { value_type: TextureValueType::Uint, ..binding },
            TextureBinding { value_type: TextureValueType::Depth, ..binding },
            TextureBinding { multisample: true, ..binding },
            TextureBinding { lod_bias: f32::NAN, ..binding },
        ] {
            assert!(lower_scalar_with_textures(code, options, &[wrong]).is_err());
        }
    }

    #[test]
    fn implicit_texture_sampling_requires_uniform_fragment_flow() {
        use crate::{maxwell::texture::TextureDimension, wgsl::texture::TextureValueType};
        let sample = 0xd830008061070804; // TEXS.2D RGBA
        let exit = 0xe30000000007000f;
        let raw = bytes(&[sample, exit]);
        let code = MaxwellCode::parse(&raw).unwrap();
        let bindings = [TextureBinding {
            texture_index: 8,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            lod_bias: -1.0,
            sampling: None,
        }];
        let options = LoweringOptions::default();
        let k = lower_fragment_with_textures(code, options, &bindings).unwrap();
        assert!(k.textures[0].uses_implicit_lod);
        assert_eq!(k.textures[0].group, 3);
        assert!(lower_scalar_with_textures(code, options, &bindings).is_err());
        assert!(lower_vertex_with_textures(code, options, &bindings).is_err());
        for words in [vec![sample & !0xf0000, exit], vec![exit & !0xf0000, sample, exit]] {
            let raw = bytes(&words);
            assert!(lower_fragment_with_textures(
                MaxwellCode::parse(&raw).unwrap(),
                options,
                &bindings
            )
            .is_err());
        }
    }

    #[test]
    fn fixed_mip_sampling_requires_complete_view_and_sampler_evidence() {
        use crate::{
            maxwell::texture::TextureDimension,
            wgsl::texture::{TextureFilter, TextureSampling, TextureValueType},
        };
        let sampling = TextureSampling {
            mip_level_count: 1,
            min_filter: TextureFilter::Linear,
            mag_filter: TextureFilter::Linear,
            max_anisotropy: 1,
        };
        let binding = TextureBinding {
            texture_index: 8,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            lod_bias: -1.0,
            sampling: Some(sampling),
        };
        let sample = 0xd830008061070804;
        let exit = 0xe30000000007000f;
        for words in [vec![exit & !0xf0000, sample, exit], vec![sample & !0xf0000, exit], vec![
            0xe290000000000000 | (24 << 20),
            sample,
            0xf0f800000007000f,
            exit,
        ]] {
            let raw = bytes(&words);
            let code = MaxwellCode::parse(&raw).unwrap();
            let options = LoweringOptions::default();
            let k = lower_fragment_with_textures(code, options, &[binding]).unwrap();
            assert!(k.textures[0].uses_implicit_lod);
            assert!(k.textures[0].uses_fixed_implicit_lod);
            assert!(k.wgsl.contains("textureSampleLevel"));
            assert!(!k.wgsl.contains("textureSampleBias"));
            assert!(lower_vertex_with_textures(code, options, &[binding]).is_err());
            for wrong in [
                None,
                Some(TextureSampling { mip_level_count: 2, ..sampling }),
                Some(TextureSampling { min_filter: TextureFilter::Nearest, ..sampling }),
                Some(TextureSampling { max_anisotropy: 2, ..sampling }),
                Some(TextureSampling { mip_level_count: 0, ..sampling }),
                Some(TextureSampling { max_anisotropy: 0, ..sampling }),
            ] {
                let binding = TextureBinding { sampling: wrong, ..binding };
                assert!(lower_fragment_with_textures(code, options, &[binding]).is_err());
            }
        }
        // Proving one resource does not waive derivatives for another.
        let raw = bytes(&[exit & !0xf0000, sample, sample + (2 << 36), exit]);
        let other = TextureBinding { texture_index: 10, sampling: None, ..binding };
        assert!(lower_fragment_with_textures(
            MaxwellCode::parse(&raw).unwrap(),
            LoweringOptions::default(),
            &[binding, other],
        )
        .is_err());
    }

    #[test]
    fn unsupported_resource_modes_and_carry_do_not_execute() {
        for word in [
            0x5b640b800ff70007,              // ISETP.X: incoming carry
            0x5b6403800ff70007 | (3 << 45),  // invalid predicate combiner
            0x5b5403800ff70002 | (1 << 47),  // ISET.CC
            0x3800010200070002 & !(1 << 33), // partial BFE.BREV range
            0xf0f0000024070000 | (6 << 26),  // invalid DEPBAR.LE scoreboard
            0xef90303ffff70408,              // LDC.U8.ISL
            0xef96003000070408,              // invalid LDC size 6
            0xef90023000070408,              // unexplained LDC bit 41
            0xef950030000704fe,              // B64 span crosses RZ
            0xefe8e0038007ff04,              // invalid PIXLD mode
            0xefe800028007ff04,              // PIXLD predicate output
            0x360403cffff70504,              // XMAD.X carry state
            0x360c038ffff70504,              // XMAD.CSFU
        ] {
            let raw = bytes(&[word, 0xe30000000007000f]);
            assert!(lower_fragment(MaxwellCode::parse(&raw).unwrap()).is_err(), "{word:016x}");
        }
    }

    #[test]
    fn control_regions_reject_derivatives_and_opaque_range_values() {
        use crate::{maxwell::texture::TextureDimension, wgsl::texture::TextureValueType};
        let raw = bytes(&[
            0xe290000000000000 | (24 << 20), // SSY to index3
            0xd830008061070804,              // implicit TEXS.2D
            0xf0f800000007000f,
            0xe30000000007000f,
        ]);
        let binding = TextureBinding {
            texture_index: 8,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            lod_bias: 0.0,
            sampling: None,
        };
        let error = lower_fragment_with_textures(
            MaxwellCode::parse(&raw).unwrap(),
            LoweringOptions::default(),
            &[binding],
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("implicit texture"));
        let raw = bytes(&[
            0x5c90008000070001,              // RRO.EX2 R1,R0
            0xe290000000000000 | (16 << 20), // SSY to index3
            0xf0f800000007000f,
            0x5080000000270101, // would consume R1 after the region
            0xe30000000007000f,
        ]);
        let error = lower_scalar(MaxwellCode::parse(&raw).unwrap()).unwrap_err();
        assert!(format!("{error:#}").contains("opaque RRO"));
    }

    #[test]
    fn conversion_nan_policy_is_explicit_and_nonexecuted_flags_reject() {
        let raw = bytes(&[0x5cb0108000070a00, 0xe30000000007000f]);
        let k = lower_scalar(MaxwellCode::parse(&raw).unwrap()).unwrap();
        assert_eq!(k.f2i_nan_behavior, Some(F2iNanBehavior::Zero));
        let k = lower_scalar_with_options(MaxwellCode::parse(&raw).unwrap(), LoweringOptions {
            f2i_nan_behavior: F2iNanBehavior::Indefinite,
        })
        .unwrap();
        assert_eq!(k.f2i_nan_behavior, Some(F2iNanBehavior::Indefinite));
        assert!(k.wgsl.contains("mpr_f2i(mpr_flush(s.registers[0]), false, 1u, 0x80000000u)"));
        for word in [
            0x4c10800004470000,
            0x4c1008000457ff01,
            0x3840000000170000,
            0x5cb8000000070606 | (2 << 41),
            0x5cb0108000070a00 | (1 << 42),
        ] {
            let raw = bytes(&[word, 0xe30000000007000f]);
            assert!(lower_scalar(MaxwellCode::parse(&raw).unwrap()).is_err(), "{word:016x}");
        }
    }

    fn bytes(words: &[u64]) -> Vec<u8> {
        words
            .chunks(3)
            .flat_map(|group| {
                [
                    0x001f8000fc0007e0,
                    group[0],
                    *group.get(1).unwrap_or(&0x50b0000000070f00),
                    *group.get(2).unwrap_or(&0x50b0000000070f00),
                ]
                .into_iter()
                .flat_map(u64::to_le_bytes)
            })
            .collect()
    }

    #[test]
    fn predicate_writes_resolve_dependencies_without_losing_accumulator_inputs() {
        // FSETP.LT.AND P0,P1,R0,R1,PT, then FSET using P1 as accumulator.
        let setp = 0x5bb1038000170001;
        let set = 0x5801008000170002;
        let data = bytes(&[setp, set, 0xe30000000007000f]);
        let kernel = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(kernel.written_predicates, [0, 1]);
        assert!(kernel.input_predicates.is_empty());
        assert_eq!(kernel.input_registers, [0, 1]);
        let data = bytes(&[setp & !(7 << 39), set, 0xe30000000007000f]);
        let kernel = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(kernel.input_predicates, [0]);
        let data = bytes(&[setp & !0xf0000, set, 0xe30000000007000f]);
        let kernel = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(kernel.input_predicates, [0, 1]);
        let data = bytes(&[setp & !7, 0xe30000000007000f]);
        assert!(lower_scalar(MaxwellCode::parse(&data).unwrap()).is_err());
    }

    #[test]
    fn graphics_outputs_must_be_defined_on_early_exit_paths() {
        let data = bytes(&[
            0x0103f8000007f000, // MOV32I R0,1
            0x5bb1038000170001, // FSETP writes P0/P1
            0xe30000000000000f, // @P0 EXIT
            0x0103f8000007f002, // MOV32I R2,1
            0xe30000000007000f,
        ]);
        let k = lower_fragment(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.defined_registers_on_exit, [0]);
        assert_eq!(k.written_registers, [0, 2]);
        let data = bytes(&[
            0x0103f8000007f000,
            0xeff07f800807ff00, // AST a[0x80],R0
            0xe30000000000000f,
            0xeff07f800847ff00, // AST a[0x84],R0
            0xe30000000007000f,
        ]);
        let k = lower_vertex(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.defined_attributes_on_exit, [0x80]);
        assert_eq!(k.written_attributes, [0x80, 0x84]);
    }

    #[test]
    fn fragment_interpolants_preserve_modes_locations_and_multiplier_dependencies() {
        let ipa = |destination: u64, mode: u64, location: u64| {
            0xe003ff880ff7ff00 | destination | mode << 54 | location << 52
        };
        let data = bytes(&[
            ipa(0, 0, 0),
            ipa(1, 0, 1),
            ipa(2, 2, 0),
            ipa(3, 0, 0),
            (ipa(5, 1, 0) & !(255 << 20)) | 4 << 20,
            0xe30000000007000f,
        ]);
        let k = lower_fragment(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.interpolant_reads.len(), 4);
        assert_eq!(k.interpolant_reads[1].location, InterpolationLocation::Centroid);
        assert_eq!(k.interpolant_reads[2].mode, InterpolationMode::Constant);
        assert_eq!(k.input_registers, [4]);
        for word in [ipa(0, 3, 0), ipa(0, 2, 2), ipa(0, 0, 3), ipa(0, 0, 0) | 1 << 38] {
            let data = bytes(&[word, 0xe30000000007000f]);
            assert!(lower_fragment(MaxwellCode::parse(&data).unwrap()).is_err());
        }
        for predicate in 0..16u64 {
            let word = (ipa(0, 0, 0) & !(15 << 47)) | predicate << 47;
            let data = bytes(&[word, 0xe30000000007000f]);
            let result = lower_fragment(MaxwellCode::parse(&data).unwrap());
            assert_eq!(result.is_ok(), predicate == 7);
            if let Err(error) = result {
                assert!(format!("{error:#}").contains("nontrivial trailing predicate"));
            }
        }
        let word = (ipa(0, 0, 0) & !(255 << 8)) | 4 << 8;
        let invalid_direct = bytes(&[word, 0xe30000000007000f]);
        let error = lower_fragment(MaxwellCode::parse(&invalid_direct).unwrap()).unwrap_err();
        assert!(format!("{error:#}").contains("RZ address register"));
        assert!(lower_scalar(MaxwellCode::parse(&data).unwrap()).is_err());
    }

    #[test]
    fn offset_planes_share_inputs_but_preserve_register_dependencies() {
        let data = bytes(&[
            0xe003ff880ff7ff03, // IPA.PASS R3,a[0x80],RZ (center)
            0xe02380080017ff00, // IPA.PASS.OFFSET R0,a[0x80],R1,R0
            0xe06380080017ff02, // IPA.OFFSET R2,a[0x80],R1,R0
            0xe06380080017ff04, // Same perspective plane, current R0 offset
            0xe30000000007000f,
        ]);
        let k = lower_fragment(MaxwellCode::parse(&data).unwrap()).unwrap();
        // Center, offset PASS, and offset perspective have distinct manifests;
        // repeated perspective reads share one center/gradient input pair.
        assert_eq!(k.interpolant_reads.len(), 3);
        assert_eq!(k.interpolation_gradient_slots, [1, 2]);
        assert_eq!(k.interpolation_offset_model, Some(InterpolationOffsetModel::SignedSixteenths));
        assert_eq!(k.input_registers, [0, 1]);
        assert_eq!(k.defined_registers_on_exit, [3, 0, 2, 4]);

        let center = bytes(&[0xe003ff880ff7ff03, 0xe30000000007000f]);
        let k = lower_fragment(MaxwellCode::parse(&center).unwrap()).unwrap();
        assert!(k.interpolation_gradient_slots.is_empty());
        assert_eq!(k.interpolation_offset_model, None);
    }

    #[test]
    fn vertex_outputs_distinguish_initial_values_and_prior_stores() {
        let data = bytes(&[0xefd87f810807ff01, 0xeff07f800907ff01, 0xe30000000007000f]);
        let k = lower_vertex(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.initial_output_attributes, [0x80]);
        assert!(k.input_attributes.is_empty());
        assert_eq!(k.written_attributes, [0x90]);
        let data = bytes(&[
            0xefd8ff800807ff00,
            0xeff0ff810907ff00,
            0xefd8ff810907ff02,
            0xeff0ff800a07ff02,
            0xe30000000007000f,
        ]);
        let k = lower_vertex(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.input_attributes, [0x80, 0x84]);
        assert!(k.initial_output_attributes.is_empty());
        assert_eq!(k.written_attributes, [0x90, 0x94, 0xa0, 0xa4]);
    }

    #[test]
    fn rro_pair_must_not_expose_the_opaque_intermediate() {
        let rro = 0x5c90008000070001;
        let mufu = 0x5080000000270101;
        let exit = 0xe30000000007000f;
        let data = bytes(&[rro, 0x4c98078c00070000, mufu, exit]);
        let k = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.exponentiation_pairs, 1);
        assert_eq!(k.input_registers, [0]);
        for words in [
            vec![rro, exit],
            vec![rro, mufu & !0xf0000, exit],
            vec![rro, (mufu & !255) | 2, exit],
            vec![rro, 0x5c5800000ff70102, mufu, exit],
        ] {
            let data = bytes(&words);
            assert!(lower_vertex(MaxwellCode::parse(&data).unwrap()).is_err());
        }
    }

    #[test]
    fn rro_consumers_preserve_the_temporary_until_it_is_replaced() {
        let rro = 0x5c90008000070001; // RRO.EX2 R1,R0
        let mufu = 0x5080000000270102; // MUFU.EX2 R2,R1
        let replace = 0x4c98078c00070001; // MOV R1,c[3][0]
        let exit = 0xe30000000007000f;
        let data = bytes(&[rro, mufu, (mufu & !255) | 3, replace, exit]);
        let k = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.exponentiation_pairs, 2);
        assert_eq!(k.input_registers, [0]);
        assert_eq!(k.defined_registers_on_exit, [2, 3, 1]);
        // The original float is captured before its register is overwritten.
        let data = bytes(&[rro, replace & !255, mufu, replace, exit]);
        let k = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(k.exponentiation_pairs, 1);
        assert_eq!(k.input_registers, [0]);
        // Replacing an unobserved temporary is legal, including an attribute load.
        for words in [vec![rro, replace, exit], vec![rro, 0xefd87f800807ff01, exit]] {
            let data = bytes(&words);
            assert!(lower_vertex(MaxwellCode::parse(&data).unwrap()).is_ok());
        }
        // Consuming into R2 does not turn R1 into float bits. The destination
        // replacement must not erase the dependency before checking its RHS.
        for words in [
            vec![rro, mufu, exit],
            vec![rro, mufu, replace & !0xf0000, exit],
            vec![rro, mufu, 0x5c5800000ff70101, exit], // FADD R1,R1,RZ
            vec![rro, mufu | (1 << 48), replace, exit], // modified MUFU source
        ] {
            let data = bytes(&words);
            assert!(lower_scalar(MaxwellCode::parse(&data).unwrap()).is_err());
        }
    }

    #[test]
    fn opaque_temporaries_require_replacement_on_every_control_path() {
        let rro = 0x5c90008000070001; // RRO.EX2 R1,R0
        let mufu = 0x5080000000270102; // MUFU.EX2 R2,R1
        let clear = 0x5c9807800ff70001; // MOV R1,RZ
        let read = 0x5c98078000170003; // MOV R3,R1
        let nop = 0x50b0000000070f00;
        let sync = 0xf0f800000007000f;
        let exit = 0xe30000000007000f;
        let offset = |index: i64| (index / 3) * 32 + 8 + (index % 3) * 8;
        let target = |word: u64, from, to| {
            word | ((((offset(to) - offset(from) - 8) as u64) & 0xffffff) << 20)
        };
        let ssy = |from, to| target(0xe290000000000000, from, to);
        let branch = |from, to| target(0xe24000000000000f, from, to); // @P0
        for first in [clear, nop] {
            let data = bytes(&[ssy(0, 7), first, rro, mufu, branch(4, 1), sync, nop, clear, exit]);
            let k = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
            assert!(k.unavailable_registers_on_exit.is_empty());
            assert_eq!(k.input_registers, [0]);
        }
        // The first iteration can read R1, but the loop back edge carries its
        // opaque value. An optional replacement cannot make that read valid.
        for words in [vec![ssy(0, 7), read, rro, mufu, branch(4, 1), sync, nop, clear, exit], vec![
            ssy(0, 8),
            clear & !0xf0000,
            read,
            rro,
            mufu,
            branch(5, 1),
            sync,
            nop,
            clear,
            exit,
        ]] {
            let data = bytes(&words);
            assert!(format!("{:#}", lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap_err())
                .contains("reads opaque RRO/AL2P register R1"));
        }
        // A diamond joins one overwritten path with one still-opaque path.
        for (other, valid) in [(clear, true), (nop, false), (clear & !0xf0000, false)] {
            let data = bytes(&[
                rro,
                ssy(1, 8),
                branch(2, 5),
                clear,
                target(0xe24000000007000f, 4, 6),
                other,
                read,
                sync,
                clear,
                exit,
            ]);
            assert_eq!(lower_scalar(MaxwellCode::parse(&data).unwrap()).is_ok(), valid);
        }
    }

    #[test]
    fn graphics_outputs_exclude_only_unobserved_opaque_scratch() {
        let clear5 = 0x5c9807800ff70005;
        let rro5 = 0x5c90008000470005;
        let mufu0 = 0x5080000000270500;
        let clear = 0x5c9807800ff70000;
        let exit = 0xe30000000007000f;
        let raw = bytes(&[clear5, rro5, mufu0, clear | 1, clear | 2, clear | 3, exit]);
        let code = MaxwellCode::parse(&raw).unwrap();
        assert!(lower_fragment(code).is_err());
        let k =
            lower_graphics_stage(code, false, LoweringOptions::default(), &[], None, None).unwrap();
        assert_eq!(k.unavailable_registers_on_exit, [5]);
        assert!(!k.defined_registers_on_exit.contains(&5));
        assert!((0..4).all(|r| k.defined_registers_on_exit.contains(&r)));
        let raw = bytes(&[clear | 3, (rro5 & !255) | 3, exit]);
        assert!(lower_graphics_stage(
            MaxwellCode::parse(&raw).unwrap(),
            false,
            LoweringOptions::default(),
            &[],
            None,
            None
        )
        .is_err());
        let k = lower_graphics_stage(
            MaxwellCode::parse(&raw).unwrap(),
            true,
            LoweringOptions::default(),
            &[],
            None,
            None,
        )
        .unwrap();
        assert_eq!(k.unavailable_registers_on_exit, [3]);
        assert!(!k.defined_registers_on_exit.contains(&3));
    }

    #[test]
    fn constant_shader_and_unreachable_padding() {
        // Original startup source18 PS: constant color, followed by EXIT/BRA.
        let data = bytes(&[
            0x4c98078c00070000,
            0x4c98078c00170001,
            0x4c98078c00270002,
            0x4c98078c00370003,
            0xe30000000007000f,
            0xe2400fffff87000f,
        ]);
        let result = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(
            result.constant_reads,
            (0..4).map(|i| ConstantRead { buffer: 3, byte_offset: i * 4 }).collect::<Vec<_>>()
        );
        assert_eq!(result.written_registers, [0, 1, 2, 3]);
        assert!(result.input_registers.is_empty());
        assert_eq!(result.exit_offset, 0x30);
    }

    #[test]
    fn unsupported_program_does_not_produce_a_partial_kernel() {
        let data = bytes(&[0x4c98078c00070000, 0xefd87f800807ff00, 0xe30000000007000f]); // ALD
        let error = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap_err();
        assert!(format!("{error:#}").contains("at 0x10"));
        let data = bytes(&[0x5c68118000270102, 0xe30000000007000f]); // FMUL.RZ
        assert!(format!("{:#}", lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap_err())
            .contains("directed rounding"));
        let data = bytes(&[0x4c98078c00070000]);
        assert!(lower_scalar(MaxwellCode::parse(&data).unwrap()).is_err());
    }

    #[test]
    fn predicated_write_retains_input_dependency_and_rz_is_zero() {
        let data = bytes(&[0x5c9807800ff00000, 0x5c98078000070001, 0xe30000000007000f]);
        let result = lower_scalar(MaxwellCode::parse(&data).unwrap()).unwrap();
        assert_eq!(result.input_registers, [0]); // @P0 MOV R0,RZ may not execute
        assert_eq!(result.input_predicates, [0]);
        assert!(result.wgsl.contains("s.registers[0] = 0u"));
        assert!(!result.wgsl.contains("registers[255]"));
    }
}
