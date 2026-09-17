use std::{io::Cursor, marker::PhantomData};

use anyhow::{bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use binrw_derive::binread;
use uuid::Uuid;
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor, peek_four_cc, rfrm::FormDescriptor, room::SAtlasLookup, CColor4f,
    CTransform4f, FourCC, TaggedVec,
};

mod decal;
pub use decal::{Decal, DecalData};

// Model container
pub const K_FORM_MCON: FourCC = FourCC(*b"MCON");

const K_CHUNK_MCVD: FourCC = FourCC(*b"MCVD");
const K_CHUNK_MCHD: FourCC = FourCC(*b"MCHD");
const K_CHUNK_MCCD: FourCC = FourCC(*b"MCCD");
const K_CHUNK_MCDD: FourCC = FourCC(*b"MCDD");

#[binrw]
#[derive(Clone, Debug)]
#[allow(unused)]
struct SModConHeader {
    unk: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct WorldModelInstance {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub model: Uuid,
    pub transform: CTransform4f,
}

#[binread]
#[derive(Clone, Debug, Default)]
pub struct SModConVisualData {
    #[br(map = |v: TaggedVec<u32, uuid::Bytes>| v.data.into_iter().map(Uuid::from_bytes_le).collect())]
    pub models: Vec<Uuid>,
    #[br(map = |v: TaggedVec<u32, uuid::Bytes>| v.data.into_iter().map(Uuid::from_bytes_le).collect())]
    /// WMDL resources preloaded by CModConData, separate from CMDL resources.
    pub world_models: Vec<Uuid>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub colors: Vec<CColor4f>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub transforms: Vec<CTransform4f>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Placements identify WMDLs directly, not by an index into `world_models`.
    pub world_instances: Vec<WorldModelInstance>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_1: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Per-visual index into `models` (CModConRenderData::Allocate).
    pub model_indices: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Per-visual index into `colors`, used by both ordinary and instanced draws.
    pub color_indices: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Per-visual flags appended by the native deferred G-buffer draw path.
    pub gbuffer_flags: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_3: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Empty disables atlas lookup; otherwise indexed in visual-instance order.
    pub visual_atlas_lookups: Vec<SAtlasLookup>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    /// Empty disables atlas lookup; otherwise indexed in world-instance order.
    pub world_atlas_lookups: Vec<SAtlasLookup>,
    /// Remaining MCVD bytes: presence flags followed by the optional render
    /// octree. Preserved until spatial queries are implemented.
    #[br(parse_with = binrw::until_eof)]
    pub spatial_data: Vec<u8>,
}

/// One placed visual. Model resources and colors are shared through indexed
/// tables; transforms are stored directly in visual order.
#[derive(Clone, Copy, Debug)]
pub struct VisualInstance<'a> {
    pub model_index: usize,
    pub transform: &'a CTransform4f,
    pub color: &'a CColor4f,
    pub gbuffer_flags: u8,
    pub atlas_lookup: Option<&'a SAtlasLookup>,
}

impl SModConVisualData {
    /// Resolve instance relationships without assuming the tables are sorted.
    /// Invalid parallel-array lengths or indices are rejected before iteration.
    pub fn instances(&self) -> Result<impl ExactSizeIterator<Item = VisualInstance<'_>>> {
        self.validate_instances()?;
        Ok(self.transforms.iter().enumerate().map(|(index, transform)| VisualInstance {
            model_index: usize::from(self.model_indices[index]),
            transform,
            color: &self.colors[usize::from(self.color_indices[index])],
            gbuffer_flags: self.gbuffer_flags[index],
            atlas_lookup: self.visual_atlas_lookups.get(index),
        }))
    }

    fn validate_instances(&self) -> Result<()> {
        let count = self.transforms.len();
        ensure!(
            self.model_indices.len() == count,
            "MCON model-index count differs from transform count"
        );
        ensure!(
            self.color_indices.len() == count,
            "MCON color-index count differs from transform count"
        );
        ensure!(
            self.gbuffer_flags.len() == count,
            "MCON G-buffer flag count differs from transform count"
        );
        ensure!(
            self.visual_atlas_lookups.is_empty() || self.visual_atlas_lookups.len() == count,
            "MCON visual atlas count differs from transform count"
        );
        ensure!(
            self.world_atlas_lookups.is_empty()
                || self.world_atlas_lookups.len() == self.world_instances.len(),
            "MCON world atlas count differs from world-instance count"
        );
        for (instance, (&model, &color)) in
            self.model_indices.iter().zip(&self.color_indices).enumerate()
        {
            ensure!(
                usize::from(model) < self.models.len(),
                "MCON instance {instance} model index {model} is out of range"
            );
            ensure!(
                usize::from(color) < self.colors.len(),
                "MCON instance {instance} color index {color} is out of range"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ModConData<O: ByteOrder> {
    pub visual_data: Option<SModConVisualData>,
    pub decal_data: Option<DecalData>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> ModConData<O> {
    pub fn slice(data: &[u8]) -> Result<Self> {
        let (mcon_desc, mut mcon_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(mcon_desc.id == K_FORM_MCON);
        ensure!(mcon_desc.reader_version.get() == 41);
        ensure!(mcon_desc.writer_version.get() == 44);

        let mut data = Self { visual_data: None, decal_data: None, _marker: PhantomData };
        while !mcon_data.is_empty() {
            if peek_four_cc(mcon_data) == *b"PEEK" {
                break;
            }
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(mcon_data)?;
            match chunk_desc.id {
                K_CHUNK_MCVD => {
                    let visual: SModConVisualData =
                        Cursor::new(chunk_data).read_type(Endian::Little)?;
                    visual.validate_instances()?;
                    data.visual_data = Some(visual);
                }
                K_CHUNK_MCHD => { /* TODO */ }
                K_CHUNK_MCCD => { /* TODO */ }
                K_CHUNK_MCDD => {
                    ensure!(data.decal_data.is_none(), "Duplicate MCON decal chunk");
                    data.decal_data = Some(DecalData::slice(chunk_data)?);
                }
                id => bail!("Unknown MCON chunk ID {id:?}"),
            }
            mcon_data = remain;
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual_data() -> SModConVisualData {
        SModConVisualData {
            models: vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            colors: vec![[1., 0., 0., 1.].into(), [0., 1., 0., 0.5].into()],
            transforms: vec![CTransform4f::default(); 3],
            model_indices: vec![1, 0, 1],
            color_indices: vec![0, 1, 1],
            gbuffer_flags: vec![2, 4, 2],
            ..Default::default()
        }
    }

    #[test]
    fn instances_resolve_shared_tables_in_visual_order() {
        let visual = visual_data();
        let instances: Vec<_> = visual.instances().unwrap().collect();
        assert_eq!(instances.iter().map(|v| v.model_index).collect::<Vec<_>>(), [1, 0, 1]);
        assert!(std::ptr::eq(instances[0].color, &visual.colors[0]));
        assert!(std::ptr::eq(instances[2].color, &visual.colors[1]));
        assert!(std::ptr::eq(instances[2].transform, &visual.transforms[2]));
        assert_eq!(instances[1].gbuffer_flags, 4);
        assert_eq!(SModConVisualData::default().instances().unwrap().len(), 0);
    }

    #[test]
    fn invalid_instance_tables_return_errors() {
        let mut visual = visual_data();
        visual.model_indices.pop();
        assert!(visual.instances().err().unwrap().to_string().contains("model-index count"));
        let mut visual = visual_data();
        visual.color_indices.push(0);
        assert!(visual.instances().err().unwrap().to_string().contains("color-index count"));
        let mut visual = visual_data();
        visual.gbuffer_flags.pop();
        assert!(visual.instances().err().unwrap().to_string().contains("G-buffer flag count"));
        let mut visual = visual_data();
        visual.model_indices[1] = 2;
        assert!(visual.instances().err().unwrap().to_string().contains("instance 1 model index 2"));
        let mut visual = visual_data();
        visual.color_indices[2] = 2;
        assert!(visual.instances().err().unwrap().to_string().contains("instance 2 color index 2"));
    }

    #[test]
    fn atlas_tables_follow_placements_not_resource_indices() {
        let mut visual = visual_data();
        visual.visual_atlas_lookups =
            (0..3).map(|index| SAtlasLookup([index as f32, 0., 0.25, 0.].into())).collect();
        let instances: Vec<_> = visual.instances().unwrap().collect();
        assert_eq!(instances[0].model_index, instances[2].model_index);
        assert_eq!(instances[0].atlas_lookup.unwrap().transform([0.; 2]), [0., 0.]);
        assert_eq!(instances[2].atlas_lookup.unwrap().transform([0.; 2]), [2., 0.]);
        visual.visual_atlas_lookups.pop();
        assert!(visual.instances().is_err());
        visual.visual_atlas_lookups.clear();
        visual.world_instances.push(WorldModelInstance {
            model: Uuid::from_u128(5),
            transform: CTransform4f::default(),
        });
        assert!(visual.instances().unwrap().all(|instance| instance.atlas_lookup.is_none()));
        visual.world_atlas_lookups = vec![SAtlasLookup::default(); 2];
        assert!(visual.instances().is_err());
    }
}
