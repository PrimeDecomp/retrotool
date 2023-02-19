use std::{
    fs::File,
    io::{Cursor, Write},
    path::PathBuf,
};

use anyhow::{ensure, Result};
use argh::FromArgs;
use binrw::{binrw, BinReaderExt, Endian};
use retrolib::{
    format::{
        chunk::ChunkDescriptor, foot::K_FORM_FOOT, rfrm::FormDescriptor, CAABox, COBBox, CVector3f,
        FourCC,
    },
    util::file::map_file,
};

// CAABoxCollisionTree
pub const K_FORM_CLSN: FourCC = FourCC(*b"CLSN");
// COBBoxCollisionTree
pub const K_FORM_DCLN: FourCC = FourCC(*b"DCLN");

// COBBCollisionTree Header (only used in DCLN)
//pub const K_CHUNK_INFO: FourCC = FourCC(*b"INFO");

// Vertex data
pub const K_CHUNK_VERT: FourCC = FourCC(*b"VERT");

// Material data
pub const K_CHUNK_MTRL: FourCC = FourCC(*b"MTRL");

// Triangle data
pub const K_CHUNK_TRIS: FourCC = FourCC(*b"TRIS");

// Octree data
//pub const K_CHUNK_TREE: FourCC = FourCC(*b"TREE");

pub const K_CLSN_READER_VERSION: u32 = 11;
pub const K_CLSN_WRITER_VERSION: u32 = 22;

pub const K_DCLN_READER_VERSION: u32 = 9;
pub const K_DCLN_WRITER_VERSION: u32 = 18;

#[binrw]
#[derive(Clone, Debug)]
pub struct Vertices {
    pub count: u32,
    #[br(count = count)]
    vertices: Vec<CVector3f>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct Materials {
    pub count: u32,
    #[br(count = count)]
    materials: Vec<CCollisionMaterial>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CCollisionMaterial {
    orientation: u32,
    material_type: u32,
    world_type: u32,
    behavior_list: u32,
    filter_list: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CIndexedTriangle {
    idx1: u32,
    idx2: u32,
    idx3: u32,
    material: u16,
    unk: u16,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct Triangles {
    count: u32,
    #[br(count = count)]
    triangles: Vec<CIndexedTriangle>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct AABoxTreeNode {
    bounds: CAABox,
    start: u32,
    end: u32,
    unk1: u8,
    unk2: u8,
    unk3: u8,
    unk4: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct OBBoxTreeNode {
    bounds: COBBox,
    start: u32,
    end: u32,
    unk1: u8,
    unk2: u8,
    unk3: u8,
    unk4: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct AABoxCollisionTree {
    count: u32,
    #[br(count = count)]
    nodes: Vec<AABoxTreeNode>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct OBBoxCollisionTree {
    count: u32,
    #[br(count = count)]
    nodes: Vec<OBBoxTreeNode>,
}

#[derive(FromArgs, PartialEq, Debug)]
/// process CLSN/DCLN files
#[argh(subcommand, name = "collision")]
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
/// converts a CLSN/DCLN to obj
#[argh(subcommand, name = "convert")]
pub struct ConvertArgs {
    #[argh(positional)]
    /// input CLSN/DCLN
    input: PathBuf,
    #[argh(positional)]
    /// output file
    out: PathBuf,
}

#[allow(unused)]
pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Convert(c_args) => convert(c_args),
    }
}

fn convert(args: ConvertArgs) -> Result<()> {
    ensure!(args.input != args.out);

    // TODO: Migrate to real model format (glTF?)
    let data = map_file(&args.input)?;
    let (form_desc, mut col_data, remain) = FormDescriptor::slice(&data, Endian::Little)?;

    if form_desc.id == K_FORM_DCLN {
        ensure!(form_desc.version_a == K_DCLN_READER_VERSION);
        ensure!(form_desc.version_b == K_DCLN_WRITER_VERSION);
    } else if form_desc.id == K_FORM_CLSN {
        ensure!(form_desc.version_a == K_CLSN_READER_VERSION);
        ensure!(form_desc.version_b == K_CLSN_WRITER_VERSION);
    }

    let (foot_desc, _, remain) = FormDescriptor::slice(remain, Endian::Little)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.version_a == 1);
    ensure!(foot_desc.version_b == 1);
    ensure!(remain.is_empty());

    //let mut bounds: Option<CAABox> = None;
    let mut vertices: Option<Vertices> = None;
    //let mut materials: Option<Materials> = None;
    let mut triangles: Option<Triangles> = None;
    //let mut aboxtree: Option<AABoxCollisionTree> = None;
    //let mut oboxtree: Option<OBBoxCollisionTree> = None;

    while !col_data.is_empty() {
        let (desc, data, remain) = ChunkDescriptor::slice(col_data, Endian::Little)?;
        /*
        if desc.id == K_CHUNK_INFO {
            //bounds = Some(Cursor::new(data).read_type(Endian::Little)?);
            //log::debug!("Bounds: {bounds:#?}");
        } else */
        if desc.id == K_CHUNK_VERT {
            vertices = Some(Cursor::new(data).read_type(Endian::Little)?);
            log::debug!("Vertices: {vertices:#?}");
        } else if desc.id == K_CHUNK_MTRL {
            //materials = Some(Cursor::new(data).read_type(Endian::Little)?);
            //log::debug!("Materials: {materials:#?}");
        } else if desc.id == K_CHUNK_TRIS {
            triangles = Some(Cursor::new(data).read_type(Endian::Little)?);
            log::debug!("Triangles: {triangles:#?}");
        } /* else if desc.id == K_CHUNK_TREE {
              if form_desc.id == K_FORM_CLSN {
                  //aboxtree = Some(Cursor::new(data).read_type(Endian::Little)?);
                  //log::debug!("Tree: {aboxtree:#?}");
              } else {
                  //oboxtree = Some(Cursor::new(data).read_type(Endian::Little)?);
                  //log::debug!("Tree: {oboxtree:#?}");
              }
          } */
        col_data = remain;
    }

    let mut file = File::create(&args.out)?;
    if let Some(verts) = vertices {
        let tris = triangles.unwrap();
        file.write_fmt(format_args!(
            "# Generated by retrotool, {} vertices, {} triangles\n# Vertices\n",
            verts.count, tris.count
        ))?;
        for vertex in verts.vertices.iter() {
            file.write_fmt(format_args!("v {} {} {}\n", vertex.x, vertex.y, vertex.z))?;
        }
        file.write_fmt(format_args!("\n# Triangles\n"))?;
        for triangle in tris.triangles.iter() {
            file.write_fmt(format_args!(
                "f {} {} {}\n",
                triangle.idx1 + 1,
                triangle.idx2 + 1,
                triangle.idx3 + 1
            ))?;
        }
    }
    Ok(())
}
