use std::io::Write;

use super::*;
use crate::format::{material::data::EMaterialDataId, CColor4f};

fn payload(properties: &[(u8, &[u8; 4], Vec<u8>)], flags: &[&[u8; 4]]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(Uuid::from_u128(0x12345678_9012_3456_7890_123456789abc).to_bytes_le());
    data.extend(Uuid::from_u128(0xabcdef01_2345_6789_abcd_ef0123456789).to_bytes_le());
    data.extend(0x12345678u32.to_le_bytes());
    data.push(0x30); // BKLT | RLTG, not a trait count.
    data.extend((-1i32).to_le_bytes());
    data.extend(10i32.to_le_bytes());
    data.extend(1u32.to_le_bytes());
    data.extend(b"TESTCCH0");
    data.extend([1, 3]);
    data.extend((properties.len() as u32).to_le_bytes());
    for (kind, id, value) in properties {
        data.push(*kind);
        data.extend(*id);
        data.extend(value);
    }
    data.extend((flags.len() as u32).to_le_bytes());
    for id in flags {
        data.extend(*id);
    }
    data
}

fn container(data: &[u8]) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    FormDescriptor::<LittleEndian> {
        id: K_FORM_MATI,
        reader_version: 11.into(),
        writer_version: 13.into(),
        ..Default::default()
    }
    .write(&mut out, |out| {
        ChunkDescriptor::<LittleEndian> { id: K_CHUNK_MDAT, ..Default::default() }
            .write(out, |out| Ok(out.write_all(data)?))
    })
    .unwrap();
    out.into_inner()
}

fn texture(id: Uuid) -> Vec<u8> {
    let mut data = id.to_bytes_le().to_vec();
    for word in [3i32, -1, 4, 1, -1] {
        data.extend(word.to_le_bytes());
    }
    data
}

#[test]
fn native_field_order_preserves_state_and_common_typed_values() {
    let color = [0.25f32, -1.0, 4.0, 0.5];
    let int4 = [1i32, -2, 3, -4];
    let bytes = payload(
        &[
            (2, b"ICH0", (-23i32).to_le_bytes().to_vec()),
            (3, b"CCH0", color.into_iter().flat_map(f32::to_le_bytes).collect()),
            (4, b"CCH1", color.into_iter().flat_map(f32::to_le_bytes).collect()),
            (10, b"ECH0", int4.into_iter().flat_map(i32::to_le_bytes).collect()),
        ],
        &[b"MFVC", b"MFC0", b"MFVC"],
    );
    let material = MaterialInstance::slice(&container(&bytes)).unwrap();
    assert_eq!(material.material_id.to_string(), "12345678-9012-3456-7890-123456789abc");
    assert_eq!(material.material_resource_id.to_string(), "abcdef01-2345-6789-abcd-ef0123456789");
    assert_eq!(material.material_set_hash, 0x12345678);
    assert_eq!((material.traits, material.cull_mode, material.blend_mode), (0x30, -1, 10));
    let variable = &material.variable_descs[0];
    assert_eq!(variable.name, *b"TEST");
    assert_eq!(variable.data_id, *b"CCH0");
    assert_eq!((variable.data_type, variable.packing_rule), (1, 3));
    assert_eq!(material.flags(), (1 << 4) | (1 << 22));
    let properties = &material.properties;
    assert!(matches!(properties[0].data().data, CMaterialDataInner::Int1(-23)));
    assert_eq!(properties[1].data().data_id, EMaterialDataId::CCH0);
    assert_eq!(properties[1].kind(), PropertyKind::Color);
    assert_eq!(properties[2].kind(), PropertyKind::Float4);
    for property in &properties[1..3] {
        assert_eq!(property.data().data_type, EMaterialDataType::Color);
        let CMaterialDataInner::Color(value) = property.data().data else { panic!("not color") };
        assert_eq!(value.to_array(), color);
    }
    let CMaterialDataInner::Int4(value) = properties[3].data().data else { panic!("not int4") };
    assert_eq!(value.to_array(), int4);
}

#[test]
fn nil_textures_remain_present_and_keep_their_fallback_dimensions() {
    let properties: Vec<_> = (5..=8).map(|tag| (tag, b"TCH0", texture(Uuid::nil()))).collect();
    let material = MaterialInstance::slice(&container(&payload(&properties, &[]))).unwrap();
    for (property, expected) in material.properties.iter().zip([
        ETextureType::D2,
        ETextureType::D2Array,
        ETextureType::Cube,
        ETextureType::D3,
    ]) {
        assert_eq!(property.texture_fallback(), Some(expected));
        let CMaterialDataInner::Texture(value) = &property.data().data else {
            panic!("not texture")
        };
        let usage = value.usage.as_ref().unwrap();
        assert_eq!(usage.tex_coord, 3);
        assert_eq!([usage.filter, usage.wrap_x, usage.wrap_y, usage.wrap_z], [-1, 4, 1, -1]);
    }
    let bytes = payload(&[(6, b"TCH0", texture(Uuid::from_u128(1)))], &[]);
    let referenced = MaterialInstance::slice(&container(&bytes)).unwrap();
    assert_eq!(referenced.properties[0].texture_fallback(), None);
}

#[test]
fn unknown_flag_stops_runtime_mask_without_dropping_serialized_names() {
    let bytes = payload(&[], &[b"MFVC", b"????", b"MFC0"]);
    let material = MaterialInstance::slice(&container(&bytes)).unwrap();
    assert_eq!(material.flags(), 1 << 4);
    assert_eq!(material.material_flags, [FourCC(*b"MFVC"), FourCC(*b"????"), FourCC(*b"MFC0")]);
}

#[test]
fn truncated_counts_unknown_tags_and_unconsumed_data_are_rejected() {
    let bytes = payload(&[(6, b"TCH0", texture(Uuid::nil()))], &[b"MFVC"]);
    for length in 0..bytes.len() {
        assert!(MaterialInstance::slice(&container(&bytes[..length])).is_err(), "length {length}");
    }
    for count_offset in [45, 59, bytes.len() - 8] {
        let mut invalid = bytes.clone();
        invalid[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(MaterialInstance::slice(&container(&invalid)).is_err());
    }
    for tag in [1, 9, 12, 32, 48, 255] {
        let invalid = payload(&[(tag, b"CCH0", vec![0; 16])], &[]);
        let error = MaterialInstance::slice(&container(&invalid)).unwrap_err();
        assert!(format!("{error:#}").contains("Unsupported MATI property type"));
    }
    let mut invalid = bytes.clone();
    invalid.push(0);
    assert!(MaterialInstance::slice(&container(&invalid)).is_err());
    let invalid = payload(&[(2, b"????", vec![0; 4])], &[]);
    assert!(MaterialInstance::slice(&container(&invalid)).is_err());
    let mut invalid = container(&bytes);
    invalid[24..28].copy_from_slice(&37u32.to_le_bytes());
    assert!(MaterialInstance::slice(&invalid).is_err());
    let mut valid = container(&bytes);
    valid.extend(b"asset footer");
    assert!(MaterialInstance::slice(&valid).is_ok());
}

#[test]
#[ignore = "requires MPR_ROOT containing the original game PAKs"]
fn native_mati_resources_preserve_decals_fallbacks_and_variables() -> Result<()> {
    use crate::{format::pack::Package, util::file::map_file};

    let root = std::path::PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let read = |pak: &str, id: &str| -> Result<MaterialInstance> {
        let package = map_file(root.join("romfs").join(pak))?;
        MaterialInstance::slice(&Package::<LittleEndian>::read_asset(
            &package,
            Uuid::parse_str(id)?,
        )?)
    };
    let decals = [
        ("Worlds/MP1/universeRoomMP1.pak", "01259032-cdb0-4e36-99b0-e05eb1979c57"),
        ("Worlds/MP1/universeRoomMP1.pak", "26350fd7-d800-4eca-8236-fa89e3f07a43"),
        ("Worlds/MP1/universeRoomMP1.pak", "79f552c2-fabb-4f39-be42-979b4a1cb6fb"),
        ("Worlds/MP1/!Mines_Master/00h_mines_connect.pak", "89e9792a-2a77-4bb0-974e-0842aacf9497"),
        ("Preload/DuplicateData.pak", "a25c4234-ac7c-4af6-b868-9000f69bd300"),
        ("Worlds/MP1/universeRoomMP1.pak", "d65656f1-1b8b-4976-bed0-15d40a3a85b0"),
    ];
    for (pak, id) in decals {
        let material = read(pak, id)?;
        assert_eq!(material.material_id.to_string(), "7fbd5054-6138-4de1-89c7-d7ecfed80384");
        assert_eq!(
            material.material_resource_id.to_string(),
            "01ec9fa1-e139-4d58-b248-d966770775d0"
        );
        assert_eq!((material.cull_mode, material.blend_mode), (-1, 10));
        assert_eq!(material.properties.len(), 4);
        assert_eq!(material.properties[0].data().data_id, EMaterialDataId::TCH0);
        assert_eq!(material.properties[0].kind(), PropertyKind::Texture2d);
        assert_eq!(material.properties[0].texture_fallback(), None);
        let CMaterialDataInner::Color(CColor4f { a, .. }) = material.properties[1].data().data
        else {
            panic!("missing decal tint")
        };
        assert_eq!(a, 1.0);
        assert_eq!(material.flags(), 0);
    }
    let nil = read("Preload/DuplicateData.pak", "387fc747-e89e-4464-b97b-c751eb4e334a")?;
    assert_eq!(nil.properties[0].texture_fallback(), Some(ETextureType::D2Array));
    let flags = read("Preload/DuplicateData.pak", "312d8419-e58e-4244-a635-3a9831099e51")?;
    assert_eq!(flags.flags(), 1 << 4);
    let variables = read("Preload/MPR1/PreloadMP1.pak", "307ea7c4-0c38-4e8a-bb11-7d8a0ac1756f")?;
    assert_eq!(variables.variable_descs.len(), 4);
    assert_eq!(variables.variable_descs[0].name, *b"EXSC");
    assert_eq!(variables.variable_descs[3].packing_rule, 1);
    assert_eq!(variables.properties[1].texture_fallback(), Some(ETextureType::D2));
    Ok(())
}
