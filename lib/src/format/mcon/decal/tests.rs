use std::io::{Cursor, Write};

use zerocopy::LittleEndian;

use super::*;
use crate::format::{
    chunk::ChunkDescriptor,
    mcon::{ModConData, K_CHUNK_MCDD, K_FORM_MCON},
    rfrm::FormDescriptor,
};

fn chunk() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(2u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(0xabcd_u16.to_le_bytes());
    bytes.extend(0x12345678u32.to_le_bytes());
    bytes.extend(0x87654321u32.to_le_bytes());
    bytes.extend(Uuid::from_u128(1).to_bytes_le());
    bytes.extend(Uuid::from_u128(2).to_bytes_le());
    for (id, material, angle, order, unknown) in
        [(3, 1u16, 0.5f32, -3i16, 0x80), (4, 0, -1.25, 7, 0x42)]
    {
        bytes.extend(Uuid::from_u128(id).to_bytes_le());
        for word in 0..12 {
            bytes.extend((word as f32 + id as f32).to_le_bytes());
        }
        bytes.extend(material.to_le_bytes());
        bytes.extend(angle.to_le_bytes());
        bytes.extend(order.to_le_bytes());
        bytes.push(unknown);
    }
    bytes
}

fn container(chunks: &[&[u8]]) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let form = FormDescriptor::<LittleEndian> {
        id: K_FORM_MCON,
        reader_version: 41.into(),
        writer_version: 44.into(),
        ..Default::default()
    };
    form.write(&mut out, |out| {
        for &bytes in chunks {
            ChunkDescriptor::<LittleEndian> { id: K_CHUNK_MCDD, ..Default::default() }
                .write(out, |out| Ok(out.write_all(bytes)?))?;
        }
        Ok(())
    })
    .unwrap();
    out.into_inner()
}

#[test]
fn packed_records_preserve_unknowns_and_resolve_material_indices() {
    let data = ModConData::<LittleEndian>::slice(&container(&[&chunk()])).unwrap();
    let decals = data.decal_data.unwrap();
    assert_eq!(decals.unknown_short, 0xabcd);
    assert_eq!(decals.unknown_words, [0x12345678, 0x87654321]);
    assert_eq!(decals.decals.len(), 2);
    for (index, decal) in decals.decals.iter().enumerate() {
        assert_eq!(decal.id, Uuid::from_u128(index as u128 + 3));
        let matrix: mint::RowMatrix3x4<f32> = decal.transform.into();
        let rows: [[f32; 4]; 3] = matrix.into();
        assert_eq!(rows.concat(), (0..12).map(|v| (v + index + 3) as f32).collect::<Vec<_>>());
        assert_eq!(
            decals.materials[usize::from(decal.material_index)],
            Uuid::from_u128(2 - index as u128)
        );
    }
    assert_eq!(decals.decals[0].angle_radians, 0.5);
    assert_eq!(decals.decals[0].sort_order, -3);
    assert_eq!(decals.decals[0].unknown, 0x80);
    assert_eq!(decals.decals[1].angle_radians, -1.25);
    assert_eq!(decals.decals[1].sort_order, 7);
    assert_eq!(decals.decals[1].unknown, 0x42);
}

#[test]
fn malformed_decal_chunks_are_rejected_before_indexing_or_allocation() {
    let bytes = chunk();
    for length in 0..bytes.len() {
        assert!(DecalData::slice(&bytes[..length]).is_err(), "accepted length {length}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(DecalData::slice(&extra).is_err());
    let mut oversized = bytes.clone();
    oversized[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(DecalData::slice(&oversized).unwrap_err().to_string().contains("size differs"));
    let mut invalid = bytes.clone();
    invalid[48 + 73 + 64..48 + 73 + 66].copy_from_slice(&2u16.to_le_bytes());
    assert!(DecalData::slice(&invalid)
        .unwrap_err()
        .to_string()
        .contains("decal 1 material index 2"));
    assert!(ModConData::<LittleEndian>::slice(&container(&[&bytes, &bytes])).is_err());
}

#[test]
fn absent_and_empty_decal_chunks_remain_distinct() {
    assert!(ModConData::<LittleEndian>::slice(&container(&[])).unwrap().decal_data.is_none());
    let empty = ModConData::<LittleEndian>::slice(&container(&[&[0; 16]])).unwrap();
    let decals = empty.decal_data.unwrap();
    assert!(decals.decals.is_empty());
    assert!(decals.materials.is_empty());
}

#[test]
#[ignore = "requires MPR_ROOT containing the original game PAKs"]
fn native_mcon_decal_assets_preserve_placements() -> Result<()> {
    use crate::{format::pack::Package, util::file::map_file};

    let root = std::path::PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let fixtures = [
        ("!Intro_Master/!Intro_Master.pak", "7bfe75de-a6a5-4d9c-b9d3-cdffe59e3ebc", 1, 1),
        ("!Intro_Master/00d_Intro_connect.pak", "413707c5-c102-4bf1-b592-6c0bf504e73c", 5, 3),
        ("!Intro_Master/08b_Intro_ventshaft.pak", "0156e2f5-9314-469d-a3e7-ae4859db33e1", 15, 2),
        ("!Mines_Master/00h_mines_connect.pak", "73ff7899-7ed3-4b36-b662-9d4111dd943c", 2, 2),
        ("universeRoomMP1.pak", "a3cb9dbf-a2b8-4336-8603-65338cfd9953", 18, 5),
        ("universeRoomMP1.pak", "de1fbc84-42a5-4d04-84ab-2df0b1303481", 13, 4),
        ("universeRoomMP1.pak", "fed49441-63f8-4b5c-86ca-3ca74887645c", 13, 4),
    ];
    for (pak, id, count, materials) in fixtures {
        let package = map_file(root.join("romfs/Worlds/MP1").join(pak))?;
        let bytes = Package::<LittleEndian>::read_asset(&package, Uuid::parse_str(id)?)?;
        let data = ModConData::<LittleEndian>::slice(&bytes)?;
        let decals = data.decal_data.context("Missing native MCDD")?;
        assert_eq!(decals.decals.len(), count, "{id}");
        assert_eq!(decals.materials.len(), materials, "{id}");
        assert_eq!(decals.unknown_short, 0);
        assert_eq!(decals.unknown_words, [0; 2]);
        for decal in &decals.decals {
            assert_eq!(decal.angle_radians.to_bits(), std::f32::consts::FRAC_PI_4.to_bits());
            assert_eq!(decal.sort_order, 0);
            assert_eq!(decal.unknown, 0);
        }
    }
    Ok(())
}
