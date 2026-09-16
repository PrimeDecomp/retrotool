//! Lift bounded compiler-generated AL2P/ALD and AL2P/IPA pairs.
//!
//! This is whole-operation translation, not an AL2P ISA emulator. The logical
//! address must resolve from supplied constants. Its physical bits and predicate
//! remain opaque. The paired MPR DataView_Model DX/NVN programs establish the
//! three-UV selection pattern; uam's NVC0 lowering introduces AFETCH for indirect
//! VFETCH/PINTERP. See docs/shaders.md for evidence and limitations.

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

use super::*;
use crate::maxwell::attribute::AttributeAddress;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConstantAssumption {
    pub read: ConstantRead,
    pub value: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttributeAddressUse {
    pub producer_offset: usize,
    pub consumer_offset: usize,
    pub logical_byte_offset: u16,
    pub encoded_consumer_offset: u16,
    pub interpolation: bool,
}

#[derive(Clone)]
pub(super) struct KnownWord {
    value: u32,
    assumptions: Vec<ConstantAssumption>,
}

#[derive(Clone)]
pub(super) struct AddressPair {
    pub register: u8,
    producer: usize,
    predicate: u8,
    displacement: i16,
    logical: u16,
}

impl Emitter {
    pub(super) fn known_logic_predicate(
        &self,
        operation: Operation,
        always: bool,
    ) -> Option<(u8, KnownWord)> {
        if !always || self.in_control_region || self.attribute_constants.is_none() {
            return None;
        }
        let Operation::Integer(IntegerInstruction {
            sources,
            operation:
                IntegerOperation::Logic { operation, invert, predicate_mode, predicate_destination },
            ..
        }) = operation
        else {
            return None;
        };
        if predicate_destination == 7 || !(2..=3).contains(&predicate_mode) {
            return None;
        }
        let mut a = if operation == LogicOperation::PassB {
            KnownWord { value: 0, assumptions: Vec::new() }
        } else {
            self.known_word(sources[0])?
        };
        let b = self.known_word(sources[1])?;
        let av = if invert[0] { !a.value } else { a.value };
        let bv = if invert[1] { !b.value } else { b.value };
        let value = match operation {
            LogicOperation::And => av & bv,
            LogicOperation::Or => av | bv,
            LogicOperation::Xor => av ^ bv,
            LogicOperation::PassB => bv,
        };
        a.value = u32::from((value != 0) == (predicate_mode == 3));
        for assumption in b.assumptions {
            if !a.assumptions.contains(&assumption) {
                a.assumptions.push(assumption);
            }
        }
        Some((predicate_destination, a))
    }

    pub(super) fn known_attribute_guard(&mut self, predicate: PredicateOperand) -> Option<bool> {
        if self.in_control_region {
            return None;
        }
        let (_, value) = self.known_predicates.iter().find(|(p, _)| *p == predicate.register)?;
        for &assumption in &value.assumptions {
            if !self.constant_assumptions.contains(&assumption) {
                self.constant_assumptions.push(assumption);
            }
        }
        Some((value.value != 0) != predicate.inverted)
    }

    fn known_word(&self, source: ValueSource) -> Option<KnownWord> {
        let immediate = |value| KnownWord { value, assumptions: Vec::new() };
        match source {
            ValueSource::Immediate(value) => Some(immediate(value)),
            ValueSource::Register(255) => Some(immediate(0)),
            ValueSource::Register(r) => {
                self.known_words.iter().find(|(v, _)| *v == r).map(|(_, v)| v.clone())
            }
            ValueSource::Constant { buffer, byte_offset } => {
                let read = ConstantRead { buffer, byte_offset: byte_offset.try_into().ok()? };
                let value = self.attribute_constants.as_ref()?.compact_reads(&[read]).ok()?[0];
                Some(KnownWord { value, assumptions: vec![ConstantAssumption { read, value }] })
            }
        }
    }

    pub(super) fn known_result(
        &self,
        operation: Operation,
        always: bool,
    ) -> Option<(u8, KnownWord)> {
        if !always || self.in_control_region || self.attribute_constants.is_none() {
            return None;
        }
        match operation {
            Operation::Alu(AluInstruction {
                destination,
                operation: AluOperation::Move { source, lane_mask: 15 },
                ..
            }) => Some((destination, self.known_word(source)?)),
            Operation::Integer(IntegerInstruction {
                destination,
                sources,
                operation: IntegerOperation::Shift { right, signed, wrap },
                ..
            }) => {
                let mut a = self.known_word(sources[0])?;
                let b = self.known_word(sources[1])?;
                let shift = b.value & 31;
                a.value = if !wrap && b.value >= 32 {
                    if right && signed {
                        ((a.value as i32) >> 31) as u32
                    } else {
                        0
                    }
                } else if right && signed {
                    ((a.value as i32) >> shift) as u32
                } else if right {
                    a.value >> shift
                } else {
                    a.value << shift
                };
                for assumption in b.assumptions {
                    if !a.assumptions.contains(&assumption) {
                        a.assumptions.push(assumption);
                    }
                }
                Some((destination, a))
            }
            _ => None,
        }
    }

    pub(super) fn replace_predicate(&mut self, r: u8, always: bool) -> Result<()> {
        if r == 7 {
            return Ok(());
        }
        self.known_predicates.retain(|(p, _)| *p != r);
        ensure!(
            always || !self.address_predicates.iter().any(|(p, _)| *p == r),
            "predicated write would mix ordinary and opaque AL2P predicates"
        );
        if always {
            self.address_predicates.retain(|(p, _)| *p != r);
            if self.in_control_region && !self.replaced_predicates.contains(&r) {
                self.replaced_predicates.push(r);
            }
        }
        Ok(())
    }

    pub(super) fn attribute_address(
        &mut self,
        a: AttributeAddress,
        always: bool,
        offset: usize,
    ) -> Result<()> {
        ensure!(
            self.attribute_constants.is_some(),
            "AL2P physical attribute addresses require bound constants and a graphics I/O provider"
        );
        ensure!(
            always && !self.in_control_region,
            "AL2P requires an unconditional straight-line producer"
        );
        ensure!(
            a.unrecognized_bits == 0 && !a.output && a.components == 1 && a.destination != 255,
            "unsupported AL2P compiler pattern"
        );
        // Resolve the logical operand before invalidating an aliased destination.
        let value = self
            .known_word(ValueSource::Register(a.address_register))
            .context("AL2P logical address is not resolved from known MOV/SHL/SHR inputs")?;
        let logical = i64::from(value.value) + i64::from(a.byte_offset);
        ensure!(
            (0x80..0x280).contains(&logical) && logical % 4 == 0,
            "AL2P logical input is outside the supported aligned generic attributes"
        );
        ensure!((0x80..0x280).contains(&a.byte_offset), "unsupported AL2P logical base");
        // These are compile-time assumptions, checked again by the generated
        // kernel and the draw binder. Unknown material bytes never become zero.
        for assumption in value.assumptions {
            if !self.constant_assumptions.contains(&assumption) {
                self.constant_assumptions.push(assumption);
            }
        }
        self.known_words.retain(|(r, _)| *r != a.destination);
        self.address_pairs.retain(|p| p.register != a.destination);
        self.range_values.retain(|(r, _)| *r != a.destination);
        self.definite.retain(|r| *r != a.destination);
        if !self.written.contains(&a.destination) {
            self.written.push(a.destination);
        }
        self.replace_predicate(a.predicate_destination, true)?;
        if a.predicate_destination != 7 {
            self.definite_predicates.retain(|r| *r != a.predicate_destination);
            self.address_predicates.push((a.predicate_destination, Some(offset)));
            if !self.written_predicates.contains(&a.predicate_destination) {
                self.written_predicates.push(a.predicate_destination);
            }
        }
        self.address_pairs.push(AddressPair {
            register: a.destination,
            producer: offset,
            predicate: a.predicate_destination,
            displacement: a.byte_offset,
            logical: logical as u16,
        });
        Ok(())
    }

    pub(super) fn lift_attribute_load(
        &mut self,
        mut access: AttributeAccess,
        load: bool,
        offset: usize,
    ) -> Result<AttributeAccess> {
        if access.address_register == 255 {
            return Ok(access);
        }
        let pair = self
            .address_pairs
            .iter()
            .find(|p| p.register == access.address_register)
            .context("physical attribute access lacks a matching local AL2P producer")?;
        ensure!(
            load && !access.patch
                && !access.output_flag
                && access.byte_offset == 0
                && access.vertex_register == 255
                && access.components == 1
                && pair.predicate == 7,
            "unsupported AL2P/ALD compiler pattern"
        );
        self.attribute_address_uses.push(AttributeAddressUse {
            producer_offset: pair.producer,
            consumer_offset: offset,
            logical_byte_offset: pair.logical,
            encoded_consumer_offset: access.byte_offset,
            interpolation: false,
        });
        access.address_register = 255;
        access.byte_offset = pair.logical;
        Ok(access)
    }

    pub(super) fn lift_interpolation(
        &mut self,
        mut ipa: Interpolation,
        always: bool,
        offset: usize,
    ) -> Result<Interpolation> {
        if !ipa.indexed {
            return Ok(ipa);
        }
        let pair = self
            .address_pairs
            .iter()
            .find(|p| p.register == ipa.address_register)
            .context("indexed interpolation lacks a matching local AL2P producer")?;
        ensure!(
            always
                && !ipa.predicate.inverted
                && pair.predicate != 7
                && ipa.predicate.register == pair.predicate
                && self.address_predicates.contains(&(pair.predicate, Some(pair.producer))),
            "indexed IPA requires its unmodified AL2P address/predicate pair"
        );
        // MPR's matched DX/NVN compiler form retains the vector base in these
        // bits. Do not extrapolate to arbitrary immediates or predicate inputs.
        ensure!(
            ipa.byte_offset == (pair.displacement as u16 & !15)
                && ipa.mode == InterpolationMode::Perspective
                && ipa.location == Some(InterpolationLocation::Offset),
            "unsupported AL2P/IPA compiler pattern"
        );
        self.attribute_address_uses.push(AttributeAddressUse {
            producer_offset: pair.producer,
            consumer_offset: offset,
            logical_byte_offset: pair.logical,
            encoded_consumer_offset: ipa.byte_offset,
            interpolation: true,
        });
        ipa.indexed = false;
        ipa.address_register = 255;
        ipa.byte_offset = pair.logical;
        ipa.predicate = PredicateOperand { register: 7, inverted: false };
        Ok(ipa)
    }
}

impl ProgramKernel {
    /// Changing an address-selection constant requires translating the stage
    /// again. The GPU repeats this check and reports host fault bit32.
    pub fn validate_constant_assumptions(
        &self,
        memory: &crate::resources::ConstantMemory,
    ) -> Result<()> {
        for assumption in &self.constant_assumptions {
            ensure!(
                memory.compact_reads(&[assumption.read])?[0] == assumption.value,
                "AL2P address-selection constant changed; prepare the shader draw again"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXIT: u64 = 0xe30000000007000f;
    const NOP: u64 = 0x50b0000000070f00;
    const CLEAR: u64 = 0x5c9807800ff70000;
    const PREFIX: [u64; 3] = [0x4c98078c00070005, 0x3848000000470505, 0xefa0000008070506];
    const IPA: u64 = 0xe060024800970600;

    fn bytes(words: &[u64]) -> Vec<u8> {
        let mut result = Vec::new();
        for chunk in words.chunks(3) {
            result.extend_from_slice(&0u64.to_le_bytes());
            for i in 0..3 {
                result.extend_from_slice(&chunk.get(i).copied().unwrap_or(NOP).to_le_bytes());
            }
        }
        result
    }

    fn memory(selector: u32) -> ConstantMemory {
        ConstantMemory::new(&[(3, &selector.to_le_bytes())]).unwrap()
    }

    fn run(words: &[u64], selector: u32) -> Result<ProgramKernel> {
        lower_graphics_stage(
            MaxwellCode::parse(&bytes(words))?,
            false,
            LoweringOptions::default(),
            &[],
            Some(&memory(selector)),
            None,
        )
    }

    #[test]
    fn indexed_uv_pairs_retain_and_enforce_assumptions() {
        let mut words = PREFIX.to_vec();
        words.extend([IPA, CLEAR | 1, CLEAR | 2, CLEAR | 3, EXIT]);
        for selector in 0..3 {
            let k = run(&words, selector).unwrap();
            assert_eq!(k.interpolant_reads[0].byte_offset, 0x80 + selector as u16 * 16);
            assert_eq!(k.attribute_address_uses.len(), 1);
            assert_eq!(k.unavailable_registers_on_exit, [6]);
            assert_eq!(k.unavailable_predicates_on_exit, [0]);
            assert_eq!(k.constant_assumptions.len(), 1);
            assert!(k.uses_memory_status && k.wgsl.contains("*memory_status |= 32u"));
            k.validate_constant_assumptions(&memory(selector)).unwrap();
            assert!(k.validate_constant_assumptions(&memory(selector + 1)).is_err());
        }
        assert!(run(&words, 32).is_err());
        assert!(lower_fragment(MaxwellCode::parse(&bytes(&words)).unwrap()).is_err());
        assert!(lower_graphics_stage(
            MaxwellCode::parse(&bytes(&words)).unwrap(),
            false,
            LoweringOptions::default(),
            &[],
            Some(&ConstantMemory::new(&[]).unwrap()),
            None
        )
        .is_err());
    }

    #[test]
    fn opaque_address_and_predicate_cannot_escape_or_mismatch() {
        for invalid in [
            vec![0x5c98078000670000],      // raw AL2P address read
            vec![CLEAR & !0xf0000],        // ordinary @P0 guard
            vec![IPA ^ (1 << 50)],         // inverted pair predicate
            vec![IPA ^ (1 << 47)],         // another predicate
            vec![IPA ^ (1 << 32)],         // unverified retained immediate
            vec![0x38403000001705ff, IPA], // overwrite P0 before IPA
            vec![CLEAR | 6, IPA],          // overwrite address before IPA
        ] {
            let mut words = PREFIX.to_vec();
            words.extend(invalid);
            words.push(EXIT);
            assert!(run(&words, 0).is_err(), "{words:x?}");
        }
        // A conditional write invalidates a compile-time MOV value even if its
        // runtime predicate might be false. No stale address specialization.
        let mut words = PREFIX[..1].to_vec();
        words.extend([(CLEAR | 5) & !0xf0000, PREFIX[1], PREFIX[2], IPA, EXIT]);
        assert!(run(&words, 0).is_err());
    }

    fn target(base: u64, from: usize, to: usize) -> u64 {
        let offset = |i: usize| (i / 3 * 32 + (i % 3 + 1) * 8) as i64;
        base | (((offset(to) - offset(from) - 8) as u64 & 0xffffff) << 20)
    }

    #[test]
    fn control_flow_preserves_opaque_taint_until_replacement() {
        let ssy = |from, to| target(0xe290000000000000, from, to);
        let sync = 0xf0f800000007000f;
        let clear_p0 = 0x38403000001705ff;
        let guard_p0 = CLEAR & !0xf0000;
        for replacement in [false, true] {
            let words = [
                PREFIX[0],
                PREFIX[1],
                PREFIX[2],
                IPA,
                ssy(4, 8),
                if replacement { clear_p0 } else { NOP },
                guard_p0,
                sync,
                EXIT,
            ];
            assert_eq!(run(&words, 0).is_ok(), replacement);
            let words = [
                PREFIX[0],
                PREFIX[1],
                PREFIX[2],
                IPA,
                ssy(4, 7),
                if replacement { clear_p0 } else { NOP },
                sync,
                guard_p0,
                EXIT,
            ];
            assert_eq!(run(&words, 0).is_ok(), replacement);
        }
        // A diamond has one still-opaque path. The join must not treat the
        // predicate's replacement in the other block as unconditional.
        for other in [NOP, clear_p0] {
            let words = [
                PREFIX[0],
                PREFIX[1],
                PREFIX[2],
                IPA,
                ssy(4, 11),
                target(0xe24000000001000f, 5, 8),
                clear_p0,
                target(0xe24000000007000f, 7, 9),
                other,
                guard_p0,
                sync,
                EXIT,
            ];
            assert_eq!(run(&words, 0).is_ok(), other == clear_p0);
        }
        let words =
            [PREFIX[0], PREFIX[1], PREFIX[2], IPA, ssy(4, 7), 0x5c98078000670000, sync, EXIT];
        assert!(run(&words, 0).is_err());
    }

    #[test]
    fn vertex_pairs_support_guarded_loads_without_exposing_addresses() {
        let words = [
            PREFIX[0],
            PREFIX[1],
            PREFIX[2] | (7 << 44),
            0xefd87f8000000600,
            0xefd87f8000070601,
            EXIT,
        ];
        let k = lower_graphics_stage(
            MaxwellCode::parse(&bytes(&words)).unwrap(),
            true,
            LoweringOptions::default(),
            &[],
            Some(&memory(1)),
            None,
        )
        .unwrap();
        assert_eq!(k.input_attributes, [0x90]);
        assert_eq!(k.attribute_address_uses.len(), 2);
        assert_eq!(k.unavailable_registers_on_exit, [6]);
        assert!(k.unavailable_predicates_on_exit.is_empty());
    }

    #[test]
    fn packed_selector_guards_choose_one_aliasing_load() {
        // A known LOP predicate selects one of two complementary ALD guards.
        // Both loads alias the address register. The skipped one cannot require
        // an address token that the executed one already replaced.
        let words = [
            PREFIX[0],
            0x38403000001705ff,
            0x3829008000170505,
            PREFIX[1],
            PREFIX[2] | (7 << 44),
            0xefd87f8000000606,
            0xefd87f8000080606,
            0xeff07f800807ff06,
            EXIT,
        ];
        for selector in 0..6 {
            let k = lower_graphics_stage(
                MaxwellCode::parse(&bytes(&words)).unwrap(),
                true,
                LoweringOptions::default(),
                &[],
                Some(&memory(selector)),
                None,
            )
            .unwrap();
            assert!(k.input_registers.is_empty() && k.unavailable_registers_on_exit.is_empty());
            assert_eq!(k.input_attributes, [0x80 + (selector as u16 / 2) * 16]);
            assert_eq!(k.attribute_address_uses.len(), 1);
            assert_eq!(k.constant_assumptions[0].value, selector);
        }
        // An ordinary conditional predicate overwrite destroys that assumption.
        let mut changed = words.to_vec();
        changed.insert(5, 0x38403000001105ff);
        assert!(lower_graphics_stage(
            MaxwellCode::parse(&bytes(&changed)).unwrap(),
            true,
            LoweringOptions::default(),
            &[],
            Some(&memory(1)),
            None
        )
        .is_err());
    }

    #[test]
    fn unused_attribute_stores_do_not_invent_scratch_registers() {
        let raw = bytes(&[0xeff07f800807ff06, EXIT]);
        let code = MaxwellCode::parse(&raw).unwrap();
        let k = lower_graphics_stage(code, true, LoweringOptions::default(), &[], None, Some(&[]))
            .unwrap();
        assert!(k.input_registers.is_empty());
        assert_eq!(k.unavailable_attributes_on_exit, [0x80]);
        assert!(k.written_attributes.is_empty());
        let k =
            lower_graphics_stage(code, true, LoweringOptions::default(), &[], None, Some(&[0x80]))
                .unwrap();
        assert_eq!(k.input_registers, [6]);
        assert!(k.unavailable_attributes_on_exit.is_empty());
        // An output later read by ALD.O cannot be omitted silently.
        let raw = bytes(&[0xeff07f800807ff06, 0xefd87f810807ff00, EXIT]);
        assert!(lower_graphics_stage(
            MaxwellCode::parse(&raw).unwrap(),
            true,
            LoweringOptions::default(),
            &[],
            None,
            Some(&[])
        )
        .is_err());
    }
}
