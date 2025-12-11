#![allow(clippy::double_parens)]

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, DirBuilder, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Result};
use argh::FromArgs;
use binrw::{binrw, BinReaderExt, BinWriterExt, Endian};
use gltf_json::{self as json, buffer::Stride, validation::USize64};
use half::f16;
use image::ColorType;
use json::validation::Checked::Valid;
use png::SrgbRenderingIntent;
use retrolib::{
    format::{
        cmdl::{
            CMaterialDataInner, CMaterialTextureTokenData, EBufferType, EMaterialDataId,
            EVertexComponent, EVertexDataFormat, ModelData,
        },
        foot::locate_meta,
        txtr::{decompress_image, slice_texture, TextureData},
    },
    util::file::map_file,
};
use serde_json::json;
use uuid::Uuid;
use zerocopy::LittleEndian;

#[derive(FromArgs, PartialEq, Debug)]
/// process CMDL files
#[argh(subcommand, name = "cmdl")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Convert(ConvertArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// converts a CMDL to glTF
#[argh(subcommand, name = "convert")]
pub struct ConvertArgs {
    #[argh(positional)]
    /// input CMDL
    input: PathBuf,
    #[argh(positional)]
    /// output directory
    out_dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Convert(c_args) => convert(c_args),
    }
}

#[derive(Debug, Clone)]
struct VertexBufferAttribute {
    pub in_offset: u32,
    pub out_offset: u32,
    pub in_format: EVertexDataFormat,
    pub in_size: u32,
    pub out_format: EVertexDataFormat,
    pub component: EVertexComponent,
}

#[derive(Debug, Clone, Default)]
struct VertexBufferInfo {
    pub vertex_count: u32,
    pub in_stride: u32,
    pub out_stride: u32,
    pub attributes: Vec<VertexBufferAttribute>,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct R16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct Rg16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub g: f16,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct Rgba16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub g: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub b: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub a: f16,
}

fn convert(args: ConvertArgs) -> Result<()> {
    let data = map_file(&args.input)?;
    let dir = args.input.parent().unwrap_or(Path::new("."));
    let meta = locate_meta::<LittleEndian>(&data)?;
    let ModelData { head, mtrl, mesh, vbuf, ibuf, mut vtx_buffers, idx_buffers, .. } =
        ModelData::<LittleEndian>::slice(&data, meta)?;

    // Build buffer to component index
    let mut buf_infos: Vec<VertexBufferInfo> = Vec::with_capacity(vtx_buffers.len());
    for info in &vbuf.info {
        let num_buffers = info.num_buffers as usize;
        let mut infos =
            vec![
                VertexBufferInfo { vertex_count: todo!("info.vertex_count"), ..Default::default() };
                num_buffers
            ];
        for component in &info.components {
            let out = &mut infos[component.buffer_index as usize];
            match out.in_stride {
                0 => out.in_stride = component.stride,
                stride if stride != component.stride => {
                    bail!("Mismatched strides: {} != {}", component.stride, stride);
                }
                _ => {}
            }
            out.attributes.push(VertexBufferAttribute {
                in_offset: component.offset,
                out_offset: 0,
                in_format: component.format,
                in_size: component.format.byte_size(),
                out_format: EVertexDataFormat::Unknown,
                component: component.component,
            });
        }
        buf_infos.append(&mut infos);
    }

    // Calculate out strides & offsets
    for info in &mut buf_infos {
        info.attributes.sort_by_key(|c| c.in_offset);
        let mut out_stride = 0u32;
        for attribute in &mut info.attributes {
            attribute.out_offset = out_stride;
            attribute.out_format = match attribute.in_format {
                // Translate f16 to f32 in output
                EVertexDataFormat::R16Float => EVertexDataFormat::R32Float,
                EVertexDataFormat::Rg16Float => EVertexDataFormat::Rg32Float,
                EVertexDataFormat::Rgba16Float => EVertexDataFormat::Rgba32Float,
                format => format,
            };
            out_stride += attribute.out_format.byte_size();
        }
        info.out_stride = out_stride;
    }

    // Rebuild vertex buffers if necessary
    for (buf, info) in vtx_buffers.iter_mut().zip(&buf_infos) {
        // Sanity check buffer size
        ensure!(buf.len() == info.vertex_count as usize * info.in_stride as usize);
        if info.in_stride == info.out_stride {
            // No rebuild necessary
            continue;
        }

        let mut reader = Cursor::new(&**buf);
        let mut out_buf: Vec<u8> = vec![0; info.vertex_count as usize * info.out_stride as usize];
        let mut w = Cursor::new(&mut *out_buf);
        let mut tmp_buf = [0u8; 16]; // max size of attribute
        let mut in_buf = vec![0u8; info.in_stride as usize];
        for _ in 0..info.vertex_count as usize {
            reader.read_exact(&mut in_buf)?;
            let mut r = Cursor::new(&*in_buf);
            for attribute in &info.attributes {
                r.set_position(attribute.in_offset as u64);
                if attribute.in_format == attribute.out_format {
                    let tmp = &mut tmp_buf[0..attribute.in_size as usize];
                    r.read_exact(tmp)?;
                    w.write_all(tmp)?;
                } else {
                    match (attribute.in_format, attribute.out_format) {
                        (EVertexDataFormat::R16Float, EVertexDataFormat::R32Float) => {
                            let tmp: R16F = r.read_type(Endian::Little)?;
                            w.write_type(&tmp, Endian::Little)?;
                        }
                        (EVertexDataFormat::Rg16Float, EVertexDataFormat::Rg32Float) => {
                            let tmp: Rg16F = r.read_type(Endian::Little)?;
                            w.write_type(&tmp, Endian::Little)?;
                        }
                        (EVertexDataFormat::Rgba16Float, EVertexDataFormat::Rgba32Float) => {
                            let tmp: Rgba16F = r.read_type(Endian::Little)?;
                            w.write_type(&tmp, Endian::Little)?;
                        }
                        (in_format, out_format) => {
                            todo!("Convertion from {in_format:?} => {out_format:?}")
                        }
                    }
                }
            }
        }
        *buf = out_buf;
    }

    DirBuilder::new().recursive(true).create(&args.out_dir)?;
    let mut json_buffers = Vec::with_capacity(vtx_buffers.len() + idx_buffers.len());
    for (idx, buf) in vtx_buffers.iter().enumerate() {
        let file_name = format!("vtxbuf{idx}.bin");
        fs::write(args.out_dir.join(&file_name), buf)?;
        json_buffers.push(json::Buffer {
            byte_length: buf.len().into(),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            uri: Some(file_name),
        });
    }
    for (idx, buf) in idx_buffers.iter().enumerate() {
        let file_name = format!("idxbuf{idx}.bin");
        fs::write(args.out_dir.join(&file_name), buf)?;
        json_buffers.push(json::Buffer {
            byte_length: buf.len().into(),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            uri: Some(file_name),
        });
    }

    let mut cur_buf = 0usize;
    let mut json_buffer_views = Vec::new();
    let mut json_accessors = Vec::new();
    let mut json_attributes: Vec<
        BTreeMap<json::validation::Checked<json::mesh::Semantic>, json::Index<json::Accessor>>,
    > = Vec::new();
    for buf_info in &vbuf.info {
        let num_buffers = buf_info.num_buffers as usize;
        let mut attribute_map = BTreeMap::new();
        for idx in 0..num_buffers {
            let target_vtx_buf = cur_buf + idx;
            let info = &buf_infos[target_vtx_buf];
            json_buffer_views.push(json::buffer::View {
                buffer: json::Index::new(target_vtx_buf as u32),
                byte_length: json_buffers[target_vtx_buf].byte_length,
                byte_offset: None,
                byte_stride: Some(Stride(info.out_stride as usize)),
                extensions: Default::default(),
                extras: Default::default(),
                name: Some(format!("Vertex buffer view {target_vtx_buf}")),
                target: Some(Valid(json::buffer::Target::ArrayBuffer)),
            });
            for attribute in &info.attributes {
                let accessor = json::Accessor {
                    buffer_view: Some(json::Index::new(target_vtx_buf as u32)),
                    byte_offset: Some(USize64(attribute.out_offset as u64)),
                    count: USize64(info.vertex_count as u64),
                    component_type: Valid(json::accessor::GenericComponentType(
                        match attribute.out_format {
                            EVertexDataFormat::R8Unorm
                            | EVertexDataFormat::R8Uint
                            | EVertexDataFormat::Rg8Unorm
                            | EVertexDataFormat::Rg8Uint
                            | EVertexDataFormat::Rgba8Unorm
                            | EVertexDataFormat::Rgba8Uint => json::accessor::ComponentType::U8,
                            EVertexDataFormat::R8Snorm
                            | EVertexDataFormat::R8Sint
                            | EVertexDataFormat::Rg8Snorm
                            | EVertexDataFormat::Rg8Sint
                            | EVertexDataFormat::Rgba8Snorm
                            | EVertexDataFormat::Rgba8Sint => json::accessor::ComponentType::I8,
                            EVertexDataFormat::R16Unorm
                            | EVertexDataFormat::R16Uint
                            | EVertexDataFormat::Rg16Unorm
                            | EVertexDataFormat::Rg16Uint
                            | EVertexDataFormat::Rgba16Unorm
                            | EVertexDataFormat::Rgba16Uint => json::accessor::ComponentType::U16,
                            EVertexDataFormat::R16Snorm
                            | EVertexDataFormat::R16Sint
                            | EVertexDataFormat::Rg16Snorm
                            | EVertexDataFormat::Rg16Sint
                            | EVertexDataFormat::Rgba16Snorm
                            | EVertexDataFormat::Rgba16Sint => json::accessor::ComponentType::I16,
                            EVertexDataFormat::R32Uint
                            | EVertexDataFormat::Rg32Uint
                            | EVertexDataFormat::Rgb32Uint
                            | EVertexDataFormat::Rgba32Uint => json::accessor::ComponentType::U32,
                            EVertexDataFormat::R32Float
                            | EVertexDataFormat::Rg32Float
                            | EVertexDataFormat::Rgb32Float
                            | EVertexDataFormat::Rgba32Float => json::accessor::ComponentType::F32,
                            format => todo!("Unsupported glTF component type {format:?}"),
                        },
                    )),
                    extensions: Default::default(),
                    extras: Default::default(),
                    type_: Valid(match attribute.out_format {
                        EVertexDataFormat::R8Unorm
                        | EVertexDataFormat::R8Uint
                        | EVertexDataFormat::R8Snorm
                        | EVertexDataFormat::R8Sint
                        | EVertexDataFormat::R16Unorm
                        | EVertexDataFormat::R16Uint
                        | EVertexDataFormat::R16Snorm
                        | EVertexDataFormat::R16Sint
                        | EVertexDataFormat::R32Uint
                        | EVertexDataFormat::R32Float => json::accessor::Type::Scalar,
                        EVertexDataFormat::Rg8Unorm
                        | EVertexDataFormat::Rg8Uint
                        | EVertexDataFormat::Rg8Snorm
                        | EVertexDataFormat::Rg8Sint
                        | EVertexDataFormat::Rg16Unorm
                        | EVertexDataFormat::Rg16Uint
                        | EVertexDataFormat::Rg16Snorm
                        | EVertexDataFormat::Rg16Sint
                        | EVertexDataFormat::Rg32Uint
                        | EVertexDataFormat::Rg32Float => json::accessor::Type::Vec2,
                        EVertexDataFormat::Rgb32Uint | EVertexDataFormat::Rgb32Float => {
                            json::accessor::Type::Vec3
                        }
                        EVertexDataFormat::Rgba8Unorm
                        | EVertexDataFormat::Rgba8Uint
                        | EVertexDataFormat::Rgba8Snorm
                        | EVertexDataFormat::Rgba8Sint
                        | EVertexDataFormat::Rgba16Unorm
                        | EVertexDataFormat::Rgba16Uint
                        | EVertexDataFormat::Rgba16Snorm
                        | EVertexDataFormat::Rgba16Sint
                        | EVertexDataFormat::Rgba32Uint
                        | EVertexDataFormat::Rgba32Float => match attribute.component {
                            EVertexComponent::TexCoord0
                            | EVertexComponent::TexCoord1
                            | EVertexComponent::TexCoord2
                            | EVertexComponent::TexCoord3 => json::accessor::Type::Vec2,
                            EVertexComponent::Position | EVertexComponent::Normal => {
                                json::accessor::Type::Vec3
                            }
                            _ => json::accessor::Type::Vec4,
                        },
                        format => todo!("Unsupported glTF accessor type {format:?}"),
                    }),
                    min: if attribute.component == EVertexComponent::Position {
                        Some(json::Value::Array(vec![
                            json!(head.bounds.min.x),
                            json!(head.bounds.min.y),
                            json!(head.bounds.min.z),
                        ]))
                    } else {
                        None
                    },
                    max: if attribute.component == EVertexComponent::Position {
                        Some(json::Value::Array(vec![
                            json!(head.bounds.max.x),
                            json!(head.bounds.max.y),
                            json!(head.bounds.max.z),
                        ]))
                    } else {
                        None
                    },
                    name: Some(format!(
                        "{:?} {:?} => {:?}",
                        attribute.component, attribute.in_format, attribute.out_format
                    )),
                    normalized: attribute.out_format.normalized(),
                    sparse: None,
                };
                let accessor_idx = json_accessors.len();
                json_accessors.push(accessor);
                let semantic = match attribute.component {
                    EVertexComponent::Position => json::mesh::Semantic::Positions,
                    EVertexComponent::Normal => json::mesh::Semantic::Normals,
                    EVertexComponent::Tangent0 => json::mesh::Semantic::Tangents,
                    EVertexComponent::Tangent1 => json::mesh::Semantic::Extras("TANGENT_1".into()),
                    EVertexComponent::Tangent2 => json::mesh::Semantic::Extras("TANGENT_2".into()),
                    EVertexComponent::TexCoord0 => json::mesh::Semantic::TexCoords(0),
                    EVertexComponent::TexCoord1 => json::mesh::Semantic::TexCoords(1),
                    EVertexComponent::TexCoord2 => json::mesh::Semantic::TexCoords(2),
                    EVertexComponent::TexCoord3 => json::mesh::Semantic::TexCoords(3),
                    // EVertexComponent::Color => json::mesh::Semantic::Colors(0),
                    EVertexComponent::BoneIndices => json::mesh::Semantic::Joints(0),
                    EVertexComponent::BoneWeights => json::mesh::Semantic::Weights(0),
                    EVertexComponent::BakedLightingCoord => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_COORD".into())
                    }
                    EVertexComponent::BakedLightingTangent => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_TANGENT".into())
                    }
                    EVertexComponent::VertInstanceParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceColor => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_COLOR".into())
                    }
                    EVertexComponent::VertTransform0 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_0".into())
                    }
                    EVertexComponent::VertTransform1 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_1".into())
                    }
                    EVertexComponent::VertTransform2 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_2".into())
                    }
                    EVertexComponent::CurrentPosition => {
                        json::mesh::Semantic::Extras("CURRENT_POSITION".into())
                    }
                    EVertexComponent::VertInstanceOpacityParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_OPACITY_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceColorIndexingParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_COLOR_INDEXING_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceOpacityIndexingParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_OPACITY_INDEXING_PARAMS".into())
                    }
                    EVertexComponent::VertInstancePaintParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_PAINT_PARAMS".into())
                    }
                    EVertexComponent::BakedLightingLookup => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_LOOKUP".into())
                    }
                    EVertexComponent::MaterialChoice0 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_0".into())
                    }
                    EVertexComponent::MaterialChoice1 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_1".into())
                    }
                    EVertexComponent::MaterialChoice2 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_2".into())
                    }
                    EVertexComponent::MaterialChoice3 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_3".into())
                    }
                    _ => continue,
                };
                attribute_map.insert(Valid(semantic), json::Index::new(accessor_idx as u32));
            }
        }
        json_attributes.push(attribute_map);
        cur_buf += num_buffers;
    }

    for (idx, _) in ibuf.info.iter().enumerate() {
        let target_buf = cur_buf + idx;
        json_buffer_views.push(json::buffer::View {
            buffer: json::Index::new(target_buf as u32),
            byte_length: json_buffers[target_buf].byte_length,
            byte_offset: None,
            byte_stride: None,
            extensions: Default::default(),
            extras: Default::default(),
            name: Some(format!("Index buffer view {}", target_buf - cur_buf)),
            target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
        });
    }

    let mut json_samplers = Vec::new();
    let mut json_textures = Vec::new();
    let mut json_images = Vec::new();
    let mut texture_map: HashMap<Uuid, usize> = HashMap::new();
    fn add_texture(
        texture: &CMaterialTextureTokenData,
        map: &mut HashMap<Uuid, usize>,
        samplers: &mut Vec<json::texture::Sampler>,
        textures: &mut Vec<json::Texture>,
        images: &mut Vec<json::Image>,
        in_dir: &Path,
        out_dir: &Path,
    ) -> Result<json::texture::Info> {
        let Some(usage) = &texture.usage else { bail!("Texture without usage!") };
        let texture_idx = if let Some(&existing) = map.get(&texture.id) {
            existing
        } else {
            let texture_idx = textures.len();
            samplers.push(json::texture::Sampler {
                mag_filter: match usage.filter {
                    0 => Some(Valid(json::texture::MagFilter::Nearest)),
                    1 => Some(Valid(json::texture::MagFilter::Linear)),
                    -1 => None,
                    filter => todo!("Filter {filter}"),
                },
                min_filter: match usage.filter {
                    0 => Some(Valid(json::texture::MinFilter::Nearest)),
                    1 => Some(Valid(json::texture::MinFilter::Linear)),
                    -1 => None,
                    filter => todo!("Filter {filter}"),
                },
                name: Some(format!("{} sampler", texture.id)),
                wrap_s: Valid(match usage.wrap_x {
                    0 => json::texture::WrappingMode::ClampToEdge,
                    1 => json::texture::WrappingMode::Repeat,
                    2 => json::texture::WrappingMode::MirroredRepeat,
                    _ => todo!(),
                }),
                wrap_t: Valid(match usage.wrap_y {
                    0 => json::texture::WrappingMode::ClampToEdge,
                    1 => json::texture::WrappingMode::Repeat,
                    2 => json::texture::WrappingMode::MirroredRepeat,
                    _ => todo!(),
                }),
                extensions: None,
                extras: None,
            });
            textures.push(json::Texture {
                name: Some(format!("{}", texture.id)),
                sampler: Some(json::Index::new(texture_idx as u32)),
                source: json::Index::new(texture_idx as u32),
                extensions: None,
                extras: None,
            });
            images.push(json::Image {
                buffer_view: None,
                mime_type: None,
                name: Some(format!("{}", texture.id)),
                uri: Some(format!("{}.png", texture.id)),
                extensions: None,
                extras: None,
            });
            // TODO: please clean up
            {
                log::info!("Converting TXTR {}", texture.id);
                let txtr_file = map_file(in_dir.join(format!("{}.TXTR", texture.id)))?;
                let meta = locate_meta::<LittleEndian>(&txtr_file)?;
                let txtr = TextureData::<LittleEndian>::slice(&txtr_file, meta)?;
                let slice = &slice_texture(&txtr)?[0][0];
                let image = decompress_image(
                    txtr.head.format,
                    slice.width,
                    slice.height,
                    &txtr.data[slice.data_range.clone()],
                )?;
                let mut f = File::create(out_dir.join(format!("{}.png", texture.id)))?;
                let mut p = png::Encoder::new(&mut f, image.width(), image.height());
                if txtr.head.format.is_srgb() {
                    p.set_source_srgb(SrgbRenderingIntent::Perceptual);
                }
                p.set_color(match image.color() {
                    ColorType::L8 | ColorType::L16 => png::ColorType::Grayscale,
                    ColorType::La8 | ColorType::La16 => png::ColorType::GrayscaleAlpha,
                    ColorType::Rgb8 | ColorType::Rgb16 | ColorType::Rgb32F => png::ColorType::Rgb,
                    ColorType::Rgba8 | ColorType::Rgba16 | ColorType::Rgba32F => {
                        png::ColorType::Rgba
                    }
                    color => todo!("PNG {color:?}"),
                });
                p.set_depth(match image.color() {
                    ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8 => {
                        png::BitDepth::Eight
                    }
                    ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16 => {
                        png::BitDepth::Sixteen
                    }
                    color => todo!("PNG {color:?}"),
                });
                p.add_text_chunk("TXTR".into(), format!("{}", texture.id))?;
                let mut w = p.write_header()?;
                w.write_image_data(image.as_bytes())?;
                w.finish()?;
                f.flush()?;
            }
            map.insert(texture.id, texture_idx);
            texture_idx
        };
        Ok(json::texture::Info {
            index: json::Index::new(texture_idx as u32),
            tex_coord: usage.tex_coord,
            extensions: None,
            extras: None,
        })
    }

    let mut json_materials = Vec::with_capacity(mtrl.materials.len());
    for mat in &mtrl.materials {
        let mut json_material = json::Material {
            alpha_cutoff: None,
            alpha_mode: Valid(json::material::AlphaMode::Opaque),
            double_sided: false,
            name: Some(mat.name.clone()),
            pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                base_color_factor: json::material::PbrBaseColorFactor([0.0, 0.0, 0.0, 0.0]),
                base_color_texture: None,
                metallic_factor: json::material::StrengthFactor(0.0),
                roughness_factor: json::material::StrengthFactor(0.0),
                metallic_roughness_texture: None,
                extensions: None,
                extras: None,
            },
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
            emissive_factor: Default::default(),
            extensions: None,
            extras: None,
        };
        for data in &mat.data {
            match data.data_id {
                EMaterialDataId::DIFT => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                                dir,
                                &args.out_dir,
                            )?);
                    }
                    _ => bail!("Unsupported data type for DIFT"),
                },
                EMaterialDataId::DIFC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        json_material.pbr_metallic_roughness.base_color_factor =
                            json::material::PbrBaseColorFactor([
                                color.r, color.g, color.b, color.a,
                            ]);
                    }
                    _ => bail!("Unsupported data type for DIFC"),
                },
                EMaterialDataId::ICAN => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.emissive_texture = Some(add_texture(
                            texture,
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                            dir,
                            &args.out_dir,
                        )?);
                    }
                    _ => bail!("Unsupported data type for ICAN"),
                },
                EMaterialDataId::ICNC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        json_material.emissive_factor =
                            json::material::EmissiveFactor([color.r, color.g, color.b]);
                    }
                    _ => bail!("Unsupported data type for ICNC"),
                },
                EMaterialDataId::NMAP => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        let info = add_texture(
                            texture,
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                            dir,
                            &args.out_dir,
                        )?;
                        json_material.normal_texture = Some(json::material::NormalTexture {
                            index: info.index,
                            scale: 1.0,
                            tex_coord: info.tex_coord,
                            extensions: None,
                            extras: None,
                        });
                    }
                    _ => bail!("Unsupported data type for NMAP"),
                },
                EMaterialDataId::BCLR => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        // json_material.pbr_metallic_roughness.metallic_factor =
                        //     json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                                dir,
                                &args.out_dir,
                            )?);
                    }
                    _ => bail!("Unsupported data type for BCLR"),
                },
                EMaterialDataId::METL => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.pbr_metallic_roughness.metallic_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.roughness_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.metallic_roughness_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                                dir,
                                &args.out_dir,
                            )?);
                    }
                    _ => bail!("Unsupported data type for METL"),
                },
                // TODO support layered properly
                EMaterialDataId::BCRL => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        // json_material.pbr_metallic_roughness.metallic_factor =
                        //     json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                &texture.textures[0],
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                                dir,
                                &args.out_dir,
                            )?);
                    }
                    _ => bail!("Unsupported data type for BCLR"),
                },
                EMaterialDataId::MTLL => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        json_material.pbr_metallic_roughness.metallic_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.roughness_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.metallic_roughness_texture =
                            Some(add_texture(
                                &texture.textures[0],
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                                dir,
                                &args.out_dir,
                            )?);
                    }
                    _ => bail!("Unsupported data type for MTLL"),
                },
                EMaterialDataId::NRML => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        let info = add_texture(
                            &texture.textures[0],
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                            dir,
                            &args.out_dir,
                        )?;
                        json_material.normal_texture = Some(json::material::NormalTexture {
                            index: info.index,
                            scale: 1.0,
                            tex_coord: info.tex_coord,
                            extensions: None,
                            extras: None,
                        });
                    }
                    _ => bail!("Unsupported data type for NRML"),
                },
                _id => {
                    // log::debug!("Ignoring material data ID {id:?}");
                    continue;
                }
            }
        }
        json_materials.push(json_material);
    }

    let mut json_meshes = Vec::with_capacity(mesh.meshes.len());
    for (mesh_idx, mesh) in mesh.meshes.iter().enumerate() {
        let index_type = ibuf.info[mesh.idx_buf_idx as usize];
        let index_buf_idx = cur_buf as u32 + mesh.idx_buf_idx as u32;
        let index_accessor_idx = json_accessors.len() as u32;
        json_accessors.push(json::Accessor {
            buffer_view: Some(json::Index::new(index_buf_idx)),
            byte_offset: Some(USize64(
                mesh.index_start as u64
                    * match index_type {
                        EBufferType::U8 => 1,
                        EBufferType::U16 => 2,
                        EBufferType::U32 => 4,
                    },
            )),
            count: USize64(mesh.index_count as u64),
            component_type: Valid(json::accessor::GenericComponentType(match index_type {
                EBufferType::U8 => json::accessor::ComponentType::U8,
                EBufferType::U16 => json::accessor::ComponentType::U16,
                EBufferType::U32 => json::accessor::ComponentType::U32,
            })),
            extensions: None,
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Scalar),
            min: None,
            max: None,
            name: Some(format!("Mesh {mesh_idx} indices")),
            normalized: false,
            sparse: None,
        });
        json_meshes.push(json::Mesh {
            extensions: None,
            extras: Default::default(),
            name: Some(format!("Mesh {mesh_idx}")),
            primitives: vec![json::mesh::Primitive {
                attributes: json_attributes[mesh.vtx_buf_idx as usize].clone(),
                extensions: None,
                extras: Default::default(),
                indices: Some(json::Index::new(index_accessor_idx)),
                material: Some(json::Index::new(mesh.material_idx as u32)),
                mode: Default::default(),
                targets: None,
            }],
            weights: None,
        });
    }

    let mut json_scene_nodes = Vec::with_capacity(json_meshes.len());
    let mut json_nodes = Vec::with_capacity(json_meshes.len());
    for (idx, _) in json_meshes.iter().enumerate() {
        json_nodes.push(json::Node {
            camera: None,
            children: None,
            extensions: None,
            extras: None,
            matrix: None,
            mesh: Some(json::Index::new(idx as u32)),
            name: None,
            rotation: None,
            scale: None,
            translation: None,
            skin: None,
            weights: None,
        });
        json_scene_nodes.push(json::Index::new(idx as u32));
    }

    let json_root = json::Root {
        accessors: json_accessors,
        animations: vec![],
        asset: Default::default(),
        buffers: json_buffers,
        buffer_views: json_buffer_views,
        scene: Some(json::Index::new(0)),
        extensions: None,
        extras: Default::default(),
        extensions_used: vec![],
        extensions_required: vec![],
        cameras: vec![],
        images: json_images,
        materials: json_materials,
        meshes: json_meshes,
        nodes: json_nodes,
        samplers: json_samplers,
        scenes: vec![json::Scene {
            extensions: Default::default(),
            extras: Default::default(),
            name: Some("Scene".into()),
            nodes: json_scene_nodes,
        }],
        skins: vec![],
        textures: json_textures,
    };
    let writer = fs::File::create(args.out_dir.join("out.gltf")).expect("I/O error");
    json::serialize::to_writer_pretty(writer, &json_root).expect("Serialization error");

    Ok(())
}
