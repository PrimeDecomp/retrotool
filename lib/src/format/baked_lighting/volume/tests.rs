use super::*;

fn bc1_tile(color: u16) -> Vec<u8> {
    let mut block = [0; 8];
    block[..2].copy_from_slice(&color.to_le_bytes());
    block[2..4].copy_from_slice(&color.to_le_bytes());
    block.repeat(8192)
}

#[test]
fn interpolation_crosses_negative_tile_boundaries_on_all_three_axes() -> Result<()> {
    let data: Vec<_> = (0..8)
        .map(|corner| {
            bc1_tile(
                if corner & 1 != 0 { 0xf800 } else { 0 }
                    | if corner & 2 != 0 { 0x07e0 } else { 0 }
                    | if corner & 4 != 0 { 0x001f } else { 0 },
            )
        })
        .collect();
    let mut volume = ProbeVolume::enclosing_grid([-128, -64, -16], [127, 63, 15])?;
    assert_eq!(volume.grid_origin(), [-128, -64, -16]);
    assert_eq!(volume.dimensions(), [256, 128, 32]);
    // Populate the internal sparse view with synthetic, correctly sized BC1
    // payloads. Public TextureData insertion is exercised by the asset test.
    for (i, data) in data.iter().enumerate() {
        volume.tiles[3]
            .insert([(i & 1) as u32, ((i >> 1) & 1) as u32, ((i >> 2) & 1) as u32], data);
    }
    for (index, expected) in [
        ([-1., -1., -1.], [0., 0., 0.]),
        ([0., 0., 0.], [1., 1., 1.]),
        ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
        ([-0.25, -0.75, -1.], [0.75, 0.25, 0.]),
    ] {
        assert_eq!(volume.sample_layer(3, volume.grid_coordinate(index)?)?, expected);
    }
    Ok(())
}

#[test]
fn black_border_is_filtered_but_missing_interior_tiles_remain_errors() -> Result<()> {
    let white = bc1_tile(0xffff);
    let mut volume = ProbeVolume::new([0, 0, 0], [256, 64, 16])?;
    volume.tiles[3].insert([0; 3], &white);
    for i in 1..128 {
        assert_eq!(volume.sample_layer(3, [0.01 + i as f32 * 0.0037, 0.23457, 0.45678])?, [1.; 3]);
    }
    for (index, expected) in [
        ([0., 0., 0.], 1.),
        ([-0.5, 0., 0.], 0.5),
        ([-0.5, -0.5, -0.5], 0.125),
        ([-1., 0., 0.], 0.),
        ([127., 0., 0.], 1.), // Zero-weight neighbor need not be supplied.
    ] {
        assert_eq!(volume.sample_layer(3, volume.grid_coordinate(index)?)?, [expected; 3]);
    }
    let error = volume.sample_layer(3, volume.grid_coordinate([127.5, 0., 0.])?).unwrap_err();
    assert!(error.to_string().contains("Missing probe tile in layer 3 at [1, 0, 0]"));
    assert_eq!(volume.sample_layer(3, [f32::MAX, 0., 0.])?, [0.; 3]);
    assert!(volume.sample_layer(3, [f32::NAN, 0., 0.]).is_err());
    assert!(volume.sample_layer(6, [0.; 3]).is_err());
    assert!(ProbeVolume::new([1, 0, 0], [128, 64, 16]).is_err());
    assert!(ProbeVolume::new([0; 3], [128, 64, 0]).is_err());
    assert!(ProbeVolume::enclosing_grid([0; 3], [-1, 0, 0]).is_err());
    Ok(())
}

#[test]
fn hdr_layers_use_their_own_tile_width_and_preserve_depth_order() -> Result<()> {
    // BC6H one-region mode 11, equal maximum endpoints in each RGB channel.
    let mut block = 3u128;
    for j in 0..6 {
        block |= 1023u128 << (5 + j * 10);
    }
    let bright = block.to_le_bytes().repeat(4096);
    let dark = vec![0; 65536];
    let mut alternating = bright.clone();
    for z in (1..16).step_by(2) {
        alternating[z * 4096..(z + 1) * 4096].fill(0);
    }
    let mut volume = ProbeVolume::new([0; 3], [128, 64, 16])?;
    volume.tiles[0].insert([0; 3], &alternating);
    volume.tiles[0].insert([1, 0, 0], &dark);
    for (index, expected) in [
        ([0., 0., 0.], 65504.),
        ([0., 0., 1.], 0.),
        ([0., 0., 0.5], 32752.),
        ([63.5, 0., 0.], 32752.),
        ([64., 0., 0.], 0.),
    ] {
        assert_eq!(volume.sample_layer(0, volume.grid_coordinate(index)?)?, [expected; 3]);
    }
    Ok(())
}

#[test]
#[ignore = "requires MPR_PROBE_PAK pointing to an extracted PAK containing LTPB assets"]
fn real_tiles_match_dense_decoding_and_detect_duplicate_mappings() -> Result<()> {
    use zerocopy::LittleEndian;

    use crate::{
        format::{
            foot::locate_meta,
            ltpb::{LightProbeData, K_FORM_LTPB},
            pack::Package,
            txtr::{decompress_image, slice_texture},
        },
        util::file::map_file,
    };

    let bytes = map_file(std::env::var("MPR_PROBE_PAK")?)?;
    let entries = Package::<LittleEndian>::read_sparse(&bytes)?;
    let mut checked = 0;
    let mut probes = 0;
    for entry in entries.into_iter().filter(|e| e.kind == K_FORM_LTPB) {
        let bytes = Package::<LittleEndian>::read_asset(&bytes, entry.id)?;
        let data =
            LightProbeData::<LittleEndian>::slice(&bytes, locate_meta::<LittleEndian>(&bytes)?)?;
        let mut volume = ProbeVolume::enclosing_grid(
            data.head.min_index.to_array().map(i32::from),
            data.head.max_index.to_array().map(i32::from),
        )?;
        for (texture, extra) in data.textures.iter().zip(&data.extra) {
            let position = extra.tile_position.to_array();
            if !volume.insert_tile(extra.layer, position, texture)? {
                continue;
            }
            assert!(volume.insert_tile(extra.layer, position, texture).is_err());
        }
        for (texture, extra) in data.textures.iter().zip(&data.extra) {
            let position = extra.tile_position.to_array();
            if volume.layers[extra.layer as usize].map_tile(position)?.is_none() {
                continue;
            }
            let slices = slice_texture(texture)?;
            // Distinct block rows, depths and tile-edge texels detect mistakes
            // in the sparse reader independently of the dense image path.
            for z in [0, 7, 15] {
                let slice = &slices[0][z];
                let image = decompress_image(
                    texture.head.format,
                    slice.width,
                    slice.height,
                    &texture.data[slice.data_range.clone()],
                )?
                .to_rgb32f();
                for (x, y) in [(0, 0), (3, 4), (slice.width - 1, slice.height - 1)] {
                    let local = [x, y, z as u32];
                    let layout = &volume.layers[extra.layer as usize];
                    let index = std::array::from_fn(|i| {
                        (position[i] as f32) * layout.tile_dimensions[i] as f32 + local[i] as f32
                    });
                    let actual =
                        volume.sample_layer(extra.layer, volume.grid_coordinate(index)?)?;
                    let expected = image.get_pixel(x, y).0;
                    for channel in 0..3 {
                        let tolerance = 1e-3 * expected[channel].abs().max(1.0);
                        assert!((actual[channel] - expected[channel]).abs() <= tolerance,
                            "{} layer {} tile {position:?} texel {local:?}: {actual:?} vs {expected:?}", entry.id, extra.layer);
                    }
                    checked += 1;
                }
            }
            if extra.layer == 0 {
                let index = std::array::from_fn(|i| {
                    (position[i] as f32 + 0.5) * volume.layers[0].tile_dimensions[i] as f32
                });
                let sampled = volume.sample(volume.grid_coordinate(index)?)?;
                let record = super::super::probe::NonlinearProbe::from_samples(&sampled, [
                    [1., 0., 0.],
                    [0., 1., 0.],
                    [0., 0., 1.],
                ])?;
                for normal in
                    [[1., 0., 0.], [-1., 0., 0.], [0., 1., 0.], [0., -1., 0.], [0., 0., 1.], [
                        0., 0., -1.,
                    ]]
                {
                    assert!(record.irradiance(normal)?.into_iter().all(f32::is_finite));
                }
                probes += 1;
            }
        }
        let min = data.head.min_index.to_array();
        let max = data.head.max_index.to_array();
        let center = std::array::from_fn(|i| (f32::from(min[i]) + f32::from(max[i])) * 0.5);
        let center_sample = volume.sample(volume.grid_coordinate(center)?);
        eprintln!(
            "{} default inspector position: {}",
            entry.id,
            center_sample.as_ref().map(|_| "sampled".to_string()).unwrap_or_else(|e| e.to_string())
        );
    }
    ensure!(checked > 0 && probes > 0, "PAK contained no mapped complete probes");
    eprintln!("Checked {checked} real probe texels and {probes} complete six-volume samples");
    Ok(())
}
