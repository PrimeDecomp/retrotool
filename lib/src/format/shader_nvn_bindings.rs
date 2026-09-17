//! NVN graphics binding conventions recovered from MPR and its bundled SDK.
//! SDK build 2217a4a3c1f3c883278de72f8b008224ad252a24: BindUniformBuffer
//! +0x4825c0, BindStorageBuffer +0x4829d0, BindTextures +0x4839e0,
//! GetTextureHandle +0x466d00. Storage descriptors use MME macro16
//! (installed at +0x48b010; instruction variants at +0xad1354/+0xad1394).
//! Compute uses deferred command records and needs a separate lowering trace.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

use super::shader_nvn::{Resource, VariableBinding};

/// Hardware CB selector emitted for a graphics NVN uniform-buffer binding.
/// Reproduces the driver's five-bit masking; it does not validate API limits.
pub fn graphics_uniform_buffer_slot(binding: u32) -> u8 { (binding.wrapping_add(3) & 31) as u8 }

/// Byte offset where a graphics texture handle is uploaded into the stage's
/// driver constant table. Raw TEX/TEXS bound indices address 32-bit words here.
pub fn graphics_texture_handle_offset(binding: u32) -> u32 {
    binding.wrapping_mul(8).wrapping_add(0x20)
}

/// Recover an NVN graphics texture slot from a direct TEXS/TEX bound index.
/// Nonaligned indices and the reserved prefix do not name an ordinary slot.
pub fn graphics_texture_slot(index: u16) -> Option<u32> {
    let relative = u32::from(index).checked_sub(8)?;
    (relative & 1 == 0).then_some(relative / 2)
}

impl Resource {
    /// Direct texture descriptor-table word offsets, in vertex/pixel/geometry
    /// order. These are the indices consumed by TEXS/TLDS, not API slots or
    /// asset IDs. Validate the 13-bit instruction field before narrowing it;
    /// absent stages remain None. Compute uses a different driver path.
    pub fn graphics_texture_word_offsets(&self) -> Result<[Option<u16>; 3]> {
        ensure!(self.kind == 0, "expected an NVN texture resource");
        let mut offsets = [None; 3];
        for (offset, &slot) in offsets.iter_mut().zip(&self.bindings) {
            if slot == u16::MAX {
                continue;
            }
            let words = graphics_texture_handle_offset(u32::from(slot)) / 4;
            ensure!(words < 1 << 13, "NVN texture slot exceeds the direct descriptor field");
            *offset = Some(words as u16);
        }
        Ok(offsets)
    }
}

/// Exact SDK combined handle packing, including sign-extension of bit31 before
/// setting bit32. This handle identifies runtime descriptors, not asset UUIDs.
pub fn texture_handle(texture: u32, sampler: u32) -> Result<u64> {
    ensure!(texture < 1 << 20 && sampler < 1 << 12, "NVN descriptor index outside handle fields");
    Ok((i64::from((texture | (sampler << 20)) as i32) as u64) | (1u64 << 32))
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
pub enum GraphicsVariableBinding {
    Uniform {
        /// Hardware CB selectors, in vertex/pixel/geometry order.
        stage_buffers: [Vec<u8>; 3],
        offset: u16,
    },
    Texture {
        /// Byte offsets in the per-stage handle table; no compute claim.
        stage_handle_offsets: [Option<u32>; 3],
    },
    StorageBuffer {
        /// Byte offsets in c[0], in vertex/pixel/geometry order. Each descriptor
        /// holds a 64-bit GPU address followed by a 32-bit byte length. The
        /// fourth word is padding; single-buffer binding does not write it.
        /// The address identifies runtime memory, not an archive byte offset.
        stage_descriptor_offsets: [Option<u32>; 3],
    },
}

impl VariableBinding<'_> {
    /// Physical graphics locations, for joining reflected names to instructions.
    /// Unknown resource kinds return None. Compute is deliberately
    /// excluded: its NVN entrypoints emit a different deferred command stream.
    pub fn graphics_location(&self) -> Option<GraphicsVariableBinding> {
        match self {
            Self::Uniform { uniform, stage_bindings } => Some(GraphicsVariableBinding::Uniform {
                stage_buffers: std::array::from_fn(|stage| {
                    stage_bindings[stage]
                        .iter()
                        .copied()
                        .map(graphics_uniform_buffer_slot)
                        .collect()
                }),
                offset: uniform.offset,
            }),
            Self::Resource { resource } if resource.kind == 0 => {
                Some(GraphicsVariableBinding::Texture {
                    stage_handle_offsets: std::array::from_fn(|stage| {
                        (resource.bindings[stage] != u16::MAX).then(|| {
                            graphics_texture_handle_offset(u32::from(resource.bindings[stage]))
                        })
                    }),
                })
            }
            Self::Resource { resource } if resource.kind == 1 => {
                Some(GraphicsVariableBinding::StorageBuffer {
                    stage_descriptor_offsets: std::array::from_fn(|stage| {
                        (resource.bindings[stage] != u16::MAX).then(|| {
                            [0x110, 0x510, 0x410][stage] + u32::from(resource.bindings[stage]) * 16
                        })
                    }),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::shader_nvn::Resource;

    #[test]
    fn reflected_texture_slots_preserve_stages_and_validate_direct_indices() {
        // Original SimpleTexture/x/permutation8 reflection: texture0 is PS-only.
        let mut resource = Resource { kind: 0, bindings: [u16::MAX, 0, u16::MAX, u16::MAX] };
        assert_eq!(resource.graphics_texture_word_offsets().unwrap(), [None, Some(8), None]);
        resource.bindings = [3, 4091, 1, 65534];
        assert_eq!(resource.graphics_texture_word_offsets().unwrap(), [
            Some(14),
            Some(8190),
            Some(10)
        ]);
        // Compute is deliberately excluded; an oversized graphics slot fails.
        resource.bindings[1] = 4092;
        assert!(resource.graphics_texture_word_offsets().is_err());
        resource.bindings = [u16::MAX; 4];
        assert_eq!(resource.graphics_texture_word_offsets().unwrap(), [None; 3]);
        resource.kind = 1;
        assert!(resource.graphics_texture_word_offsets().is_err());
    }

    #[test]
    fn storage_binding_uses_stage_specific_driver_tables() {
        // LumaUpdate's original reflection binds storage buffer 0 to PS only.
        let resource = Resource { kind: 1, bindings: [u16::MAX, 0, u16::MAX, u16::MAX] };
        let Some(GraphicsVariableBinding::StorageBuffer { stage_descriptor_offsets }) =
            (VariableBinding::Resource { resource: &resource }).graphics_location()
        else {
            panic!("storage resource must remain distinct from texture handles");
        };
        assert_eq!(stage_descriptor_offsets, [None, Some(0x510), None]);

        let resource = Resource { kind: 1, bindings: [0, 15, 3, 7] };
        let Some(GraphicsVariableBinding::StorageBuffer { stage_descriptor_offsets }) =
            (VariableBinding::Resource { resource: &resource }).graphics_location()
        else {
            unreachable!();
        };
        assert_eq!(stage_descriptor_offsets, [Some(0x110), Some(0x600), Some(0x440)]);
        let unknown = Resource { kind: 2, ..resource };
        assert!((VariableBinding::Resource { resource: &unknown }).graphics_location().is_none());
    }
    #[test]
    fn binding_and_handle_boundaries_match_sdk_commands() {
        assert_eq!(graphics_uniform_buffer_slot(0), 3);
        assert_eq!(graphics_uniform_buffer_slot(29), 0); // Driver wrap, invalid API use retained.
        assert_eq!(graphics_texture_handle_offset(0), 0x20);
        assert_eq!(graphics_texture_slot(0x12), Some(5));
        assert_eq!(graphics_texture_slot(7), None);
        assert_eq!(graphics_texture_slot(9), None);
        assert_eq!(texture_handle(1, 2047).unwrap(), 0x17ff00001);
        assert_eq!(texture_handle(1, 2048).unwrap(), 0xffffffff80000001);
        assert!(texture_handle(1 << 20, 0).is_err());
        assert!(texture_handle(0, 4096).is_err());
    }
}
