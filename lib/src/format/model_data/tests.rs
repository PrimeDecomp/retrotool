use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

use super::{wind::*, ModelDataSource, ModelDataSources};

fn wind_record() -> Vec<u8> {
    let mut bytes = b"WIND".to_vec();
    bytes.extend(1u32.to_le_bytes());
    for value in [0.4f32, 0.2, 0.4, 4.0, 4.0, 4.0, 4.0, 0.25, 0.15] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(2u32.to_le_bytes());
    bytes.extend([0, 255]);
    bytes
}

fn uv_record() -> Vec<u8> {
    let mut bytes = b"ANUV".to_vec();
    // Empty context/handles, three dispatch IDs and three empty vectors.
    bytes.extend([0; 32]);
    bytes.push(0);
    bytes
}

#[test]
fn reads_known_sources_in_either_order_and_preserves_unframed_tail() -> Result<()> {
    for records in [[wind_record(), uv_record()], [uv_record(), wind_record()]] {
        let mut bytes = 3u32.to_le_bytes().to_vec();
        bytes.extend(records.concat());
        bytes.extend(b"????WIND\0");
        let parsed = ModelDataSources::parse(&bytes)?;
        assert_eq!(parsed.sources.len(), 2);
        assert_eq!(parsed.unread_sources, 1);
        assert_eq!(parsed.remaining, b"????WIND\0");
        let wind = parsed
            .sources
            .iter()
            .find_map(|source| match source {
                ModelDataSource::Wind(data) => Some(data),
                _ => None,
            })
            .unwrap();
        assert_eq!(wind.mesh_set(0)?.unwrap().time_scale, 4.0);
        assert!(wind.mesh_set(1)?.is_none());
        assert!(wind.mesh_set(2).is_err());

        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        let parsed = ModelDataSources::parse(&bytes)?;
        assert_eq!(parsed.unread_sources, 0);
        assert_eq!(parsed.remaining, b"????WIND\0");
    }
    Ok(())
}

#[test]
fn rejects_truncated_sources_and_invalid_mesh_references() -> Result<()> {
    let mut bytes = 1u32.to_le_bytes().to_vec();
    bytes.extend(wind_record());
    for length in 0..bytes.len() {
        assert!(ModelDataSources::parse(&bytes[..length]).is_err(), "length {length}");
    }
    let mut wind = ModelWindData::parse(&bytes[8..])?.0;
    wind.mesh_sets[0] = 1;
    assert!(wind.mesh_set(0).is_err());
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ModelDataSources::parse(&bytes).is_err());
    Ok(())
}

#[test]
fn wind_phase_and_impulse_repair_follow_native_rules() -> Result<()> {
    let data = ModelWindData::parse(&wind_record()[4..])?.0;
    let set = data.sets[0];
    let mut environment = WindEnvironment::native_fallback();
    let first = set.advance(&environment, 0.0, 0.25)?;
    assert_eq!(first.phase(), 1.0);
    assert_eq!(set.advance(&environment, first.phase(), 0.5)?.phase(), 3.0);
    assert_eq!(set.at_time(&environment, 0.5)?.phase(), 2.0);
    assert_eq!(first.vectors[1], [0.4, 0.2, 0.4, 1.0]);
    assert_eq!(first.vectors[2], [4.0, 4.0, 4.0, 1.0]);
    assert_eq!(first.vectors[3], [0.4375, 0.4375, 1.0, 0.0]);
    assert_eq!(first.vectors[7], [0.0, 0.0, -1.0, 1.0]);
    assert_eq!(first.vectors[6][1].to_bits(), (-0.0f32).to_bits());
    environment.impulses[0].inner_radius = 2.0;
    environment.impulses[0].outer_radius = 1.0;
    let impulse = set.at_time(&environment, 0.0)?.vectors[6];
    assert_eq!(impulse[0], 1.0 / (2.001f32 - 2.0));
    assert_eq!(impulse[1], -2.0 / (2.001f32 - 2.0));
    assert!(set.at_time(&environment, f32::INFINITY).is_err());
    environment.direction[0] = f32::NAN;
    assert!(set.at_time(&environment, 0.0).is_err());
    Ok(())
}

#[test]
#[ignore = "requires MPR_WIND_FIXTURE with original ARM64 wind captures"]
fn matches_original_arm64_wind_updates() -> Result<()> {
    let directory = PathBuf::from(
        std::env::var_os("MPR_WIND_FIXTURE")
            .context("Set MPR_WIND_FIXTURE to research/wind/fixture")?,
    );
    let bytes = fs::read(directory.join("native.bin"))?;
    assert_eq!(bytes.len(), 1285 * 348);
    for (index, bytes) in bytes.chunks_exact(348).enumerate() {
        let values: Vec<_> = bytes
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        let set = WindSet {
            vector0: values[0..3].try_into()?,
            vector1: values[3..6].try_into()?,
            time_scale: values[6],
            independence: values[7],
            independent_magnitude: values[8],
        };
        let environment = WindEnvironment {
            direction: values[9..12].try_into()?,
            magnitude: values[12],
            impulses: std::array::from_fn(|i| {
                let base = 13 + 10 * i;
                WindImpulse {
                    position: values[base..base + 3].try_into().unwrap(),
                    direction: values[base + 3..base + 6].try_into().unwrap(),
                    outer_radius: values[base + 6],
                    inner_radius: values[base + 7],
                    amplitude: values[base + 8],
                    phase: values[base + 9],
                }
            }),
        };
        if index < 5 {
            assert_eq!(environment, WindEnvironment::native_fallback());
        }
        let snapshot = if values[35].to_bits() != 0 {
            set.at_time(&environment, values[34])?
        } else {
            set.advance(&environment, values[33], values[34])?
        };
        assert_eq!(snapshot.to_bytes().as_slice(), &bytes[144..336], "case {index} uniforms");
        let padding: Vec<_> =
            snapshot.bounds_padding.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(padding, &bytes[336..348], "case {index} bounds");
    }
    Ok(())
}
