use std::{
    fmt::Debug,
    io::{Cursor, Read, Seek},
    marker::PhantomData,
    path::Path,
};

use anyhow::{anyhow, bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use zerocopy::ByteOrder;

use crate::{
    format::{
        rfrm::FormDescriptor, slice_chunks, CColor4f, CObjectId, CStringFixed, CVector3f,
        CVector4f, FourCC, TaggedVec,
    },
    util::templates::{
        load_templates, EnumTemplate, HexU32, PropertyListTemplate, PropertyTemplateType,
        StructTemplate, TemplateDatabase, TypeTemplate, TypeTemplateType, TypedefProperty,
    },
};

// Room
pub const K_FORM_ROOM: FourCC = FourCC(*b"ROOM");

// Header
pub const K_FORM_HEAD: FourCC = FourCC(*b"HEAD");
// Room header
pub const K_CHUNK_RMHD: FourCC = FourCC(*b"RMHD");
// Performance group data
pub const K_CHUNK_PGRP: FourCC = FourCC(*b"PGRP");
// Generated object map
pub const K_CHUNK_LGEN: FourCC = FourCC(*b"LGEN");
// Docks
pub const K_CHUNK_DOCK: FourCC = FourCC(*b"DOCK");
// Baked lighting
pub const K_CHUNK_BLIT: FourCC = FourCC(*b"BLIT");
// Load unit count
pub const K_CHUNK_LUNS: FourCC = FourCC(*b"LUNS");
// Load unit
pub const K_FORM_LUNT: FourCC = FourCC(*b"LUNT");
// Load unit header
pub const K_CHUNK_LUHD: FourCC = FourCC(*b"LUHD");
// Load unit resource
pub const K_CHUNK_LRES: FourCC = FourCC(*b"LRES");
// Load unit layers
pub const K_CHUNK_LLYR: FourCC = FourCC(*b"LLYR");

// String pool
pub const K_CHUNK_STRP: FourCC = FourCC(*b"STRP");

// Script data
pub const K_FORM_SDTA: FourCC = FourCC(*b"SDTA");
// Script data header
pub const K_CHUNK_SDHR: FourCC = FourCC(*b"SDHR");
// Script data entity
pub const K_CHUNK_SDEN: FourCC = FourCC(*b"SDEN");
// Game object component instance data
pub const K_CHUNK_IDTA: FourCC = FourCC(*b"IDTA");

// Layers
pub const K_FORM_LYRS: FourCC = FourCC(*b"LYRS");
// Layer
pub const K_FORM_LAYR: FourCC = FourCC(*b"LAYR");
// Layer header
pub const K_CHUNK_LHED: FourCC = FourCC(*b"LHED");
// GSRP
pub const K_FORM_GSRP: FourCC = FourCC(*b"GSRP");
// SRIP
pub const K_FORM_SRIP: FourCC = FourCC(*b"SRIP");
// Game object component
pub const K_CHUNK_COMP: FourCC = FourCC(*b"COMP");

#[binrw]
#[derive(Clone, Debug)]
pub struct SGameAreaHeader {
    pub parent_room_id: CObjectId,
    pub unk1: u16,
    pub unk2: u16,
    pub unk3: u8,
    pub id_b: CObjectId,
    pub id_c: CObjectId,
    pub id_d: CObjectId,
    pub id_e: CObjectId,
    pub path_find_area_id: CObjectId,
    // TODO ProductionWorkStages
}

#[binrw]
#[derive(Copy, Clone, Debug, Default)]
pub struct SAtlasLookup(pub CVector4f);

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLightingLightMap {
    pub txtr_id: CObjectId,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub ids: Vec<CObjectId>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub atlas_lookups: Vec<SAtlasLookup>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLightingLightProbe {
    pub ltpb_id: CObjectId,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLighting {
    #[bw(calc = if light_map.is_some() { 1 } else { 0 } | if light_probe.is_some() { 2 } else { 0 })]
    pub flags: u32,
    #[br(if(flags & 1 != 0))]
    pub light_map: Option<BakedLightingLightMap>,
    #[br(if(flags & 2 != 0))]
    pub light_probe: Option<BakedLightingLightProbe>,
}

#[binrw]
#[derive(Clone, Debug)]
// name?
pub struct ScriptDataPairs {
    a: u32,
    b: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ScriptDataHeader {
    pub properties_count: u32,
    pub instance_data_count: u32,
    pub data_len: u32,
    #[br(count = data_len)]
    pub ids: Vec<CObjectId>,
    #[br(count = data_len)]
    pub pairs: Vec<ScriptDataPairs>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub component_type: u32,
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct PooledString {
    a: u32,
    b: u32,
    #[br(count = if a == u32::MAX { b } else { 0 })]
    bytes: Vec<u8>,
}

impl PooledString {
    pub fn get(&self, pool: Option<&StringPool>) -> Option<String> {
        if self.a == u32::MAX {
            String::from_utf8(self.bytes.clone()).ok()
        } else if let Some(data) =
            pool.and_then(|pool| pool.pool_data.get(self.a as usize..(self.a + self.b) as usize))
        {
            String::from_utf8(data.to_vec()).ok()
        } else {
            None
        }
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SizeofAllocations {
    a: u32,
    #[br(if(a != 0), map = |v: TaggedVec<u16, _>| v.data)]
    skip1: Vec<u8>,
    #[br(if(a != 0), map = |v: TaggedVec<u32, _>| v.data)]
    skip2: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SConnection {
    #[br(count = 0x1a)]
    pub skip0: Vec<u8>,
    pub event_criteria_sldr: SizeofAllocations,
    pub action_payload_sldr: SizeofAllocations,
    #[br(count = 0x13)]
    pub skip1: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SScriptLink {
    #[br(count = 0x14)]
    pub skip0: Vec<u8>,
    pub c: SizeofAllocations,
    #[br(count = 0x12)]
    pub skip1: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SGOComponentInstanceData {
    pub id: CObjectId,
    pub string: PooledString,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    pub connections: Vec<SConnection>,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    pub links: Vec<SScriptLink>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LayerHeader {
    #[br(try_map = CStringFixed::into_string)]
    #[bw(map = CStringFixed::from_string)]
    pub name: String,
    pub id: CObjectId,
    pub unk: u32,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub ids: Vec<CObjectId>,
    pub empty_id: CObjectId,
    pub unk2: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct GameObjectComponent {
    pub component_type: u32,
    pub property_index: u32,
    pub instance_index: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct GameObjectComponents {
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<GameObjectComponent>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct StringPool {
    pub unk1: u32,
    pub num_strings: u32,
    #[br(if(num_strings > 0))]
    pub pool_len: u32,
    #[br(if(num_strings > 0), count = pool_len)]
    pub pool_data: Vec<u8>,
    // Unused StringPool struct
    pub unk2: u32,
    pub unk3: u32,
    pub unk_pool_len: u32,
    #[br(count = unk_pool_len)]
    pub unk_pool_data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RoomData<O: ByteOrder> {
    pub string_pool: Option<StringPool>,
    pub room_header: SGameAreaHeader,
    pub baked_lighting: BakedLighting,
    pub component_properties: Vec<ComponentProperties>,
    pub constructed_properties: Vec<ConstructedProperty>,
    pub instance_data: Vec<SGOComponentInstanceData>,
    pub layers: Vec<Layer>,
    _marker: PhantomData<O>,
}

/// Property with an ID.
#[derive(Debug, Clone)]
pub struct ConstructedProperty {
    pub id: u32,
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub enum ConstructedPropertyValue {
    Unknown(Vec<u8>),
    Enum(Box<ConstructedEnumValue>),
    PropertyList(Box<ConstructedPropertyList>),
    Struct(Box<ConstructedStruct>),
    Typedef(Box<ConstructedTypedef>),
    List(Vec<ConstructedPropertyValue>),
    Id(CObjectId),
    Color(CColor4f),
    Vector(CVector3f),
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    String(String),
}

#[derive(Debug, Clone)]
pub struct ConstructedPropertyList {
    pub name: String,
    pub properties: Vec<ConstructedProperty>,
}

#[derive(Debug, Clone)]
pub struct ConstructedTypedef {
    pub id: u32,
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub struct ConstructedEnumValue {
    pub value: u32,
    pub enum_name: String,
    pub enum_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConstructedStruct {
    pub name: String,
    pub elements: Vec<ConstructedElement>,
}

/// Flat value for struct elements.
#[derive(Debug, Clone)]
pub struct ConstructedElement {
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub header: LayerHeader,
    pub components: Vec<GameObjectComponent>,
}

impl<O> RoomData<O>
where O: ByteOrder + 'static
{
    pub fn slice(data: &[u8]) -> Result<Self> {
        let (room_desc, room_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(room_desc.id == K_FORM_ROOM);
        ensure!(room_desc.reader_version.get() == 147);
        ensure!(room_desc.writer_version.get() == 160);

        let mut string_pool: Option<StringPool> = None;
        let mut room_header: Option<SGameAreaHeader> = None;
        let mut baked_lighting: Option<BakedLighting> = None;
        let mut component_properties: Vec<ComponentProperties> = vec![];
        let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
        let mut layers: Vec<Layer> = vec![];
        slice_chunks::<O, _, _>(
            room_data,
            |chunk, data| {
                let mut reader = Cursor::new(data);
                match chunk.id {
                    K_CHUNK_STRP => string_pool = Some(reader.read_type(Endian::Little)?),
                    id => bail!("Unknown ROOM chunk: {id:?}"),
                }
                Ok(())
            },
            |form, data| {
                match form.id {
                    K_FORM_HEAD => {
                        slice_chunks::<O, _, _>(
                            data,
                            |chunk, data| {
                                let mut reader = Cursor::new(data);
                                match chunk.id {
                                    K_CHUNK_RMHD => {
                                        room_header = Some(reader.read_type(Endian::Little)?)
                                    }
                                    K_CHUNK_BLIT => {
                                        baked_lighting = Some(reader.read_type(Endian::Little)?)
                                    }
                                    K_CHUNK_PGRP | K_CHUNK_LGEN | K_CHUNK_DOCK | K_CHUNK_LUNS => {
                                        // TODO
                                    }
                                    id => bail!("Unknown HEAD chunk: {id:?}"),
                                }
                                Ok(())
                            },
                            |form, _data| {
                                match form.id {
                                    K_FORM_LUNT => {
                                        // TODO
                                    }
                                    id => bail!("Unknown HEAD form: {id:?}"),
                                }
                                Ok(())
                            },
                        )?;
                    }
                    K_FORM_SDTA => {
                        (component_properties, instance_data) =
                            slice_script_data::<O>(data, Endian::Little)?
                    }
                    K_FORM_LYRS => layers = slice_layers::<O>(data, Endian::Little)?,
                    id => bail!("Unknown ROOM form: {id:?}"),
                }
                Ok(())
            },
        )?;

        let db = match load_templates(Path::new("lib/templates/mp1r")) {
            Ok(db) => Some(db),
            Err(e) => {
                log::error!("Failed to load templates: {:?}", e);
                None
            }
        };

        let mut constructed_properties = Vec::with_capacity(component_properties.len());
        for props in &component_properties {
            let (name, type_tmpl) = db
                .as_ref()
                .map(|db| db.find_object(props.component_type))
                .map_or((None, None), |v| v);
            let mut reader = Cursor::new(&*props.data);
            let value = if let Some(type_tmpl) = type_tmpl {
                match parse_type(
                    &mut reader,
                    Endian::Little,
                    type_tmpl,
                    db.as_ref().unwrap(),
                    string_pool.as_ref(),
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("Failed to parse type {}: {:?}", type_tmpl.name, e);
                        ConstructedPropertyValue::Unknown(props.data.clone())
                    }
                }
            } else {
                ConstructedPropertyValue::Unknown(props.data.clone())
            };
            constructed_properties.push(ConstructedProperty {
                id: props.component_type,
                name: name.cloned(),
                value,
            });
        }

        let room_header = room_header.ok_or_else(|| anyhow!("Missing RMHD chunk"))?;
        let baked_lighting = baked_lighting.ok_or_else(|| anyhow!("Missing BLIT chunk"))?;
        Ok(Self {
            string_pool,
            room_header,
            baked_lighting,
            component_properties,
            constructed_properties,
            instance_data,
            layers,
            _marker: PhantomData,
        })
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CDataEnumBitField {
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub ints: Vec<u32>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bools: Vec<u8>,
    pub enum_id: CObjectId,
    pub unk_id: CObjectId,
}

fn parse_type<R>(
    reader: &mut R,
    e: Endian,
    tmpl: &TypeTemplate,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
) -> Result<ConstructedPropertyValue>
where
    R: Read + Seek,
{
    Ok(match &tmpl.template {
        TypeTemplateType::PropertyList(plist_tmpl) => {
            parse_property_list(reader, e, db, string_pool, tmpl, plist_tmpl)?
        }
        TypeTemplateType::Struct(struct_tmpl) => {
            parse_struct(reader, e, db, string_pool, tmpl, struct_tmpl)?
        }
        TypeTemplateType::Enum(enum_tmpl) => parse_enum(reader, e, db, tmpl, enum_tmpl)?,
    })
}

fn parse_property<R>(
    reader: &mut R,
    e: Endian,
    tmpl: &PropertyTemplateType,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
) -> Result<Option<ConstructedPropertyValue>>
where
    R: Read + Seek,
{
    Ok(match tmpl {
        PropertyTemplateType::Unknown => None,
        PropertyTemplateType::Enum(enum_prop) => match db.find_enum(&enum_prop.enum_name) {
            Some(tmpl) => match &tmpl.template {
                TypeTemplateType::Enum(enum_tmpl) => {
                    Some(parse_enum(reader, e, db, tmpl, enum_tmpl)?)
                }
                _ => {
                    log::warn!("Wrong type for enum template {}", enum_prop.enum_name);
                    None
                }
            },
            None => Some(ConstructedPropertyValue::Enum(Box::new(ConstructedEnumValue {
                value: reader.read_type::<u32>(e)?,
                enum_name: enum_prop.enum_name.clone(),
                enum_value: None,
            }))),
        },
        PropertyTemplateType::Struct(struct_prop) => {
            match db.find_struct(&struct_prop.struct_name) {
                Some(tmpl) => match &tmpl.template {
                    TypeTemplateType::PropertyList(plist_tmpl) => {
                        Some(parse_property_list(reader, e, db, string_pool, tmpl, plist_tmpl)?)
                    }
                    TypeTemplateType::Struct(struct_tmpl) => {
                        Some(parse_struct(reader, e, db, string_pool, tmpl, struct_tmpl)?)
                    }
                    _ => {
                        log::warn!("Wrong type for struct template {}", struct_prop.struct_name);
                        None
                    }
                },
                None => None,
            }
        }
        PropertyTemplateType::Typedef(typedef_prop) => {
            Some(parse_typedef_interface(reader, e, db, string_pool, typedef_prop)?)
        }
        PropertyTemplateType::List(list_prop) => {
            Some(ConstructedPropertyValue::List(parse_list(reader, e, |reader, e| {
                parse_property(reader, e, &list_prop.element, db, string_pool)
                    // TODO better error handling
                    .map(|v| v.unwrap_or(ConstructedPropertyValue::Unknown(vec![])))
            })?))
        }
        PropertyTemplateType::Id => Some(ConstructedPropertyValue::Id(reader.read_type(e)?)),
        PropertyTemplateType::Color => Some(ConstructedPropertyValue::Color(reader.read_type(e)?)),
        PropertyTemplateType::Vector => {
            Some(ConstructedPropertyValue::Vector(reader.read_type(e)?))
        }
        PropertyTemplateType::Bool => {
            let v = reader.read_type::<u8>(e)?;
            if v > 1 {
                Some(ConstructedPropertyValue::U8(v))
            } else {
                Some(ConstructedPropertyValue::Bool(v != 0))
            }
        }
        PropertyTemplateType::I8 => Some(ConstructedPropertyValue::I8(reader.read_type(e)?)),
        PropertyTemplateType::I16 => Some(ConstructedPropertyValue::I16(reader.read_type(e)?)),
        PropertyTemplateType::I32 => Some(ConstructedPropertyValue::I32(reader.read_type(e)?)),
        PropertyTemplateType::I64 => Some(ConstructedPropertyValue::I64(reader.read_type(e)?)),
        PropertyTemplateType::U8 => Some(ConstructedPropertyValue::U8(reader.read_type(e)?)),
        PropertyTemplateType::U16 => Some(ConstructedPropertyValue::U16(reader.read_type(e)?)),
        PropertyTemplateType::U32 => Some(ConstructedPropertyValue::U32(reader.read_type(e)?)),
        PropertyTemplateType::U64 => Some(ConstructedPropertyValue::U64(reader.read_type(e)?)),
        PropertyTemplateType::F32 => Some(ConstructedPropertyValue::F32(reader.read_type(e)?)),
        PropertyTemplateType::F64 => Some(ConstructedPropertyValue::F64(reader.read_type(e)?)),
        PropertyTemplateType::PooledString => {
            let ps: PooledString = reader.read_type(e)?;
            ps.get(string_pool).map(ConstructedPropertyValue::String)
        }
    })
}

fn parse_property_list<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    outer: &TypeTemplate,
    tmpl: &PropertyListTemplate,
) -> Result<ConstructedPropertyValue> {
    let num_properties = reader.read_type::<u16>(e)?;
    let mut properties = Vec::with_capacity(num_properties as usize);
    for _ in 0..num_properties {
        let id = reader.read_type::<u32>(e)?;
        let size = reader.read_type::<u16>(e)?;
        let mut data = vec![0; size as usize];
        reader.read_exact(&mut data)?;
        let mut inner = Cursor::new(&*data);
        let (name, value) = match tmpl.properties.get(&HexU32(id)) {
            Some(prop_tmpl) => (
                prop_tmpl.name.clone(),
                parse_property(&mut inner, e, &prop_tmpl.template, db, string_pool)?,
            ),
            None => (None, None),
        };
        let value = value.unwrap_or_else(|| ConstructedPropertyValue::Unknown(data));
        properties.push(ConstructedProperty { id, name, value });
    }
    Ok(ConstructedPropertyValue::PropertyList(Box::new(ConstructedPropertyList {
        name: outer.name.to_string(),
        properties,
    })))
}

fn parse_typedef_interface<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    prop: &TypedefProperty,
) -> Result<ConstructedPropertyValue> {
    let (id, size) = {
        let kind: u32 = reader.read_type(e)?;
        let size: u16 = reader.read_type(e)?;
        (kind, size)
    };
    let mut data = vec![0u8; size as usize];
    reader.read_exact(&mut data)?;
    let mut inner = Cursor::new(&*data);
    let (name, type_tmpl) = db.find_typedef(id);
    let name = name.cloned();
    let value = match type_tmpl {
        Some(type_tmpl) => {
            if !prop.supported_types.contains(&type_tmpl.name) {
                log::warn!("Unsupported typedef type: {:?}", type_tmpl.name);
            }
            parse_type(&mut inner, e, type_tmpl, db, string_pool)?
        }
        None => ConstructedPropertyValue::Unknown(data),
    };
    Ok(ConstructedPropertyValue::Typedef(Box::new(ConstructedTypedef { id, name, value })))
}

fn parse_struct<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    outer: &TypeTemplate,
    tmpl: &StructTemplate,
) -> Result<ConstructedPropertyValue> {
    let mut elements = Vec::with_capacity(tmpl.elements.len());
    for prop_tmpl in &tmpl.elements {
        if let Some(elem) = parse_property(reader, e, &prop_tmpl.template, db, string_pool)? {
            elements.push(ConstructedElement { name: prop_tmpl.name.clone(), value: elem });
        }
    }
    Ok(ConstructedPropertyValue::Struct(Box::new(ConstructedStruct {
        name: outer.name.clone(),
        elements,
    })))
}

fn parse_enum<R>(
    reader: &mut R,
    e: Endian,
    _db: &TemplateDatabase,
    outer: &TypeTemplate,
    tmpl: &EnumTemplate,
) -> Result<ConstructedPropertyValue>
where
    R: Read + Seek,
{
    let value = reader.read_type::<u32>(e)?;
    let element = tmpl.values.iter().find(|v| value == v.value.0);
    let value = ConstructedEnumValue {
        value,
        enum_name: outer.name.clone(),
        enum_value: element.and_then(|e| e.name.clone()),
    };
    Ok(ConstructedPropertyValue::Enum(Box::new(value)))
}

#[inline]
fn parse_list<R, T, Cb>(reader: &mut R, e: Endian, mut cb: Cb) -> Result<Vec<T>>
where
    R: Read + Seek,
    Cb: FnMut(&mut R, Endian) -> Result<T>,
{
    let num_elements = reader.read_type::<u32>(e)?;
    let mut elements = Vec::with_capacity(num_elements as usize);
    for _ in 0..num_elements {
        elements.push(cb(reader, e)?);
    }
    Ok(elements)
}

fn slice_script_data<O>(
    data: &[u8],
    e: Endian,
) -> Result<(Vec<ComponentProperties>, Vec<SGOComponentInstanceData>)>
where
    O: ByteOrder + 'static,
{
    let mut sdhr: Option<ScriptDataHeader> = None;
    let mut component_properties: Vec<ComponentProperties> = vec![];
    let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
    slice_chunks::<O, _, _>(
        data,
        |chunk, data| {
            match chunk.id {
                K_CHUNK_SDHR => {
                    sdhr = Some(Cursor::new(data).read_type(e)?);
                    // println!("SDHR: {sdhr:?}");
                }
                K_CHUNK_SDEN => {
                    let comp: ComponentProperties = Cursor::new(data).read_type(e)?;
                    // if let ComponentType::Unknown(value) = comp.component_type {
                    //     log::warn!("Unknown component type: {value:#X}");
                    // }
                    component_properties.push(comp);
                }
                K_CHUNK_IDTA => {
                    instance_data.push(Cursor::new(data).read_type(e)?);
                }
                id => bail!("Unknown SDTA chunk: {id:?}"),
            }
            Ok(())
        },
        |form, _data| bail!("Unknown SDTA form: {:?}", form.id),
    )?;
    let sdhr = sdhr.ok_or_else(|| anyhow!("Missing SDHR chunk"))?;
    ensure!(sdhr.properties_count as usize == component_properties.len());
    ensure!(sdhr.instance_data_count as usize == instance_data.len());
    Ok((component_properties, instance_data))
}

fn slice_layers<O>(data: &[u8], e: Endian) -> Result<Vec<Layer>>
where O: ByteOrder + 'static {
    let mut layers = vec![];
    slice_chunks::<O, _, _>(
        data,
        |chunk, _data| bail!("Unknown LYRS chunk: {:?}", chunk.id),
        |form, data| {
            match form.id {
                K_FORM_LAYR => {
                    let mut header: Option<LayerHeader> = None;
                    let mut components: Vec<GameObjectComponent> = vec![];
                    slice_chunks::<O, _, _>(
                        data,
                        |chunk, _data| {
                            match chunk.id {
                                K_CHUNK_LHED => {
                                    header = Some(Cursor::new(_data).read_type(e)?);
                                }
                                id => bail!("Unknown LAYR chunk: {id:?}"),
                            }
                            Ok(())
                        },
                        |form, data| {
                            match form.id {
                                K_FORM_GSRP => {
                                    // TODO
                                }
                                K_FORM_SRIP => {
                                    slice_chunks::<O, _, _>(
                                        data,
                                        |chunk, _data| {
                                            match chunk.id {
                                                K_CHUNK_COMP => {
                                                    let comp: GameObjectComponents =
                                                        Cursor::new(_data).read_type(e)?;
                                                    components = comp.data;
                                                }
                                                id => bail!("Unknown SRIP chunk: {id:?}"),
                                            }
                                            Ok(())
                                        },
                                        |form, _data| bail!("Unknown SRIP form: {:?}", form.id),
                                    )?;
                                }
                                id => bail!("Unknown LAYR form: {id:?}"),
                            }
                            Ok(())
                        },
                    )?;
                    let header = header.ok_or_else(|| anyhow!("Missing LHED chunk"))?;
                    layers.push(Layer { header, components });
                }
                id => bail!("Unknown LYRS form: {id:?}"),
            }
            Ok(())
        },
    )?;
    Ok(layers)
}
