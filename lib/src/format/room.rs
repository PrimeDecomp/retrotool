use std::{
    fmt::{Debug, Display, Formatter},
    io::{Cursor, Read, Seek},
};

use anyhow::{anyhow, bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use dyn_clone::DynClone;

use crate::{
    format::{
        rfrm::FormDescriptor, slice_chunks, CColor4f, CObjectId, CStringFixed, CVector3f,
        CVector4f, FourCC, TaggedVec,
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
pub enum ComponentType {
    Known(EGOComponentType),
    Unknown(u32),
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub component_type: ComponentType,
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct PooledString {
    a: i32,
    b: u32,
    #[br(count = if a == -1 { b } else { 0 })]
    skip: Vec<u8>,
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
    pub component_type: ComponentType,
    pub property_index: u32,
    pub instance_index: u32,
}

impl Display for ComponentType {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            ComponentType::Known(t) => write!(f, "{:?}", t),
            ComponentType::Unknown(t) => write!(f, "{:#X}", t),
        }
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct GameObjectComponents {
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<GameObjectComponent>,
}

#[derive(Debug, Clone)]
pub struct RoomData {
    pub room_header: SGameAreaHeader,
    pub baked_lighting: BakedLighting,
    pub component_properties: Vec<ComponentProperties>,
    pub constructed_properties: Vec<ConstructedPropertyValue>,
    pub instance_data: Vec<SGOComponentInstanceData>,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone)]
pub struct ConstructedProperty {
    pub id: u32,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub enum ConstructedPropertyValue {
    ObjectId(CObjectId),
    Enum(i32, &'static str),
    Int(i32),
    Float(f32),
    Bool(bool),
    Color(CColor4f),
    Struct(ConstructedPropertyStruct),
    TypedefInterface(ETypedefInterfaceType, Box<ConstructedPropertyValue>),
    List(Vec<ConstructedPropertyValue>),
    Opaque(Box<dyn OpaqueProperty>),
    Unknown(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct ConstructedPropertyStruct {
    pub name: &'static str,
    pub properties: Vec<ConstructedProperty>,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub header: LayerHeader,
    pub components: Vec<GameObjectComponent>,
}

pub trait OpaqueProperty: Debug + DynClone + Send + Sync {}
dyn_clone::clone_trait_object!(OpaqueProperty);

#[binrw]
#[derive(Debug, Clone)]
pub struct EntityProperties {
    pub b0: u8,
    pub b1: u8,
    pub v1: CVector3f,
    pub v2: CVector3f,
    pub v3: CVector3f,
}
impl OpaqueProperty for EntityProperties {}

impl RoomData {
    pub fn slice(data: &[u8], e: Endian) -> Result<Self> {
        let (room_desc, room_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(room_desc.id == K_FORM_ROOM);
        ensure!(room_desc.reader_version == 147);
        ensure!(room_desc.writer_version == 160);

        let mut room_header: Option<SGameAreaHeader> = None;
        let mut baked_lighting: Option<BakedLighting> = None;
        let mut component_properties: Vec<ComponentProperties> = vec![];
        let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
        let mut layers: Vec<Layer> = vec![];
        slice_chunks(
            room_data,
            e,
            |chunk, _data| {
                match chunk.id {
                    K_CHUNK_STRP => {} // skip
                    id => bail!("Unknown ROOM chunk: {id:?}"),
                }
                Ok(())
            },
            |form, data| {
                match form.id {
                    K_FORM_HEAD => {
                        slice_chunks(
                            data,
                            e,
                            |chunk, data| {
                                let mut reader = Cursor::new(data);
                                match chunk.id {
                                    K_CHUNK_RMHD => room_header = Some(reader.read_type(e)?),
                                    K_CHUNK_BLIT => baked_lighting = Some(reader.read_type(e)?),
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
                        (component_properties, instance_data) = slice_script_data(data, e)?
                    }
                    K_FORM_LYRS => layers = slice_layers(data, e)?,
                    id => bail!("Unknown ROOM form: {id:?}"),
                }
                Ok(())
            },
        )?;

        let mut constructed_properties = Vec::with_capacity(component_properties.len());
        for props in &component_properties {
            use EGOComponentType::*;
            let mut reader = Cursor::new(&*props.data);
            constructed_properties.push(match props.component_type {
                ComponentType::Known(
                    ActorMP1
                    | Boolean
                    | ControllerAnalogShapeAction
                    | ObjectFollow
                    | SplineMotion
                    | Render
                    | ModCon
                    | NotSTD_DockMP1,
                )
                | ComponentType::Unknown(0xEE06CF39 | 0x1314E68C) => {
                    ConstructedPropertyValue::Struct(parse_property_struct(&mut reader, e, "")?)
                }
                ComponentType::Known(Entity) => ConstructedPropertyValue::Opaque(Box::new(
                    reader.read_type::<EntityProperties>(e)?,
                )),
                _ => ConstructedPropertyValue::Unknown(props.data.clone()),
            });
        }

        let room_header = room_header.ok_or_else(|| anyhow!("Missing RMHD chunk"))?;
        let baked_lighting = baked_lighting.ok_or_else(|| anyhow!("Missing BLIT chunk"))?;
        Ok(RoomData {
            room_header,
            baked_lighting,
            component_properties,
            constructed_properties,
            instance_data,
            layers,
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

fn parse_property_struct<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    name: &'static str,
) -> Result<ConstructedPropertyStruct> {
    let num_properties = reader.read_type::<u16>(e)?;
    let mut properties = Vec::with_capacity(num_properties as usize);
    for _ in 0..num_properties {
        let id = reader.read_type::<u32>(e)?;
        let size = reader.read_type::<u16>(e)?;
        let mut data = vec![0; size as usize];
        reader.read_exact(&mut data)?;
        let mut inner = Cursor::new(&*data);
        let value = match id {
            0xA8E2BA93 => ConstructedPropertyValue::ObjectId(inner.read_type(e)?),
            // SLdrRender
            0x948d7f67 => parse_typedef_interface(&mut inner, e)?,
            0xBFA1049E => {
                let value = inner.read_type::<i32>(e)?;
                ConstructedPropertyValue::Enum(value, "RenderTargetScene")
            }
            0x8c7307b4 => ConstructedPropertyValue::Int(inner.read_type(e)?),
            0x80ec379b | 0x9b56a30e => {
                ConstructedPropertyValue::Bool(inner.read_type::<u8>(e)? != 0)
            }
            // SLdrRenderStaticModel
            0xba315753 => ConstructedPropertyValue::Struct(parse_property_struct(
                &mut inner,
                e,
                "DepthBiasOverride",
            )?),
            0xc88cfe7c => ConstructedPropertyValue::Struct(parse_property_struct(
                &mut inner,
                e,
                "RenderStaticModelFlags",
            )?),
            0x7721158b => ConstructedPropertyValue::Struct(parse_property_struct(
                &mut inner,
                e,
                "RenderStaticModelSort",
            )?),
            0xfe69f600 => {
                ConstructedPropertyValue::List(parse_list(&mut inner, e, |reader, e| {
                    Ok(ConstructedPropertyValue::Struct(parse_property_struct(
                        reader,
                        e,
                        "MaterialDataOverride",
                    )?))
                })?)
            }
            0xe8bdc12b => ConstructedPropertyValue::ObjectId(inner.read_type(e)?),
            // SLdrModelLightingData
            0x2b1317a0 => ConstructedPropertyValue::Struct(parse_property_struct(
                &mut inner,
                e,
                "ModelLightingData",
            )?),
            0x1ac9c6f6 => ConstructedPropertyValue::Color(inner.read_type(e)?),
            0xb65b2ef5 => ConstructedPropertyValue::Int(inner.read_type(e)?),
            0xb24e2719 | 0x9AB5FD2B | 0x0b639a58 | 0x2331cbaa => {
                ConstructedPropertyValue::Bool(inner.read_type::<u8>(e)? != 0)
            }
            _ => ConstructedPropertyValue::Unknown(data),
        };
        properties.push(ConstructedProperty { id, value });
    }
    Ok(ConstructedPropertyStruct { name, properties })
}

fn parse_typedef_interface<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
) -> Result<ConstructedPropertyValue> {
    let (kind, size) = {
        let kind: ETypedefInterfaceType = reader.read_type(e)?;
        let size: u16 = reader.read_type(e)?;
        (kind, size)
    };
    let mut data = vec![0; size as usize];
    reader.read_exact(&mut data)?;
    let mut inner = Cursor::new(&*data);
    let value = match kind {
        ETypedefInterfaceType::RenderStaticModel => ConstructedPropertyValue::Struct(
            parse_property_struct(&mut inner, e, "RenderStaticModel")?,
        ),
        _ => ConstructedPropertyValue::Unknown(data),
    };
    Ok(ConstructedPropertyValue::TypedefInterface(kind, Box::new(value)))
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

fn slice_script_data(
    data: &[u8],
    e: Endian,
) -> Result<(Vec<ComponentProperties>, Vec<SGOComponentInstanceData>)> {
    let mut sdhr: Option<ScriptDataHeader> = None;
    let mut component_properties: Vec<ComponentProperties> = vec![];
    let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
    slice_chunks(
        data,
        e,
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

fn slice_layers(data: &[u8], e: Endian) -> Result<Vec<Layer>> {
    let mut layers = vec![];
    slice_chunks(
        data,
        e,
        |chunk, _data| bail!("Unknown LYRS chunk: {:?}", chunk.id),
        |form, data| {
            match form.id {
                K_FORM_LAYR => {
                    let mut header: Option<LayerHeader> = None;
                    let mut components: Vec<GameObjectComponent> = vec![];
                    slice_chunks(
                        data,
                        e,
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
                                    slice_chunks(
                                        data,
                                        e,
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

#[binrw]
#[brw(repr(u32))]
#[repr(u32)]
#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EGOComponentType {
    Entity = 0x749749f1,
    FakePlayerControls = 0x4ec5fa3a,
    StaticCollision = 0x57153aa4,
    RenderWorld = 0x41956904,
    ActorCollision = 0xb4361e7b,
    LightStatic = 0x2be8bc19,
    LightDynamic = 0x9ee5541d,
    Effect = 0xcd098f70,
    Relay = 0x8fe0bfc9,
    Counter = 0xa7db53c1,
    Timer = 0x9e8a4940,
    ControllerAction = 0xd616ee8b,
    Waypoint = 0xd898656d,
    PathControl = 0xf0240d23,
    CameraHint = 0x4fe57689,
    TouchableTrigger = 0x97e65ddd,
    Touch = 0x1a4117ab,
    TriggerLogic = 0xc49d730e,
    CustomInterpolation = 0x1349e5ac,
    CameraTarget = 0xc1f64515,
    Generator = 0xde522669,
    SplineMotion = 0x2c4f2d31,
    DynamicActorCollision = 0x21c57d2b,
    Render = 0xdf31ec16,
    TakeDamage = 0xbde4ab05,
    AnimationMountRider = 0x5453c979,
    TimerSequence = 0x46fad23,
    ApplyDamage = 0x3175df36,
    RelayRandom = 0x65e2349b,
    RelayConditional = 0xabbcfc6a,
    SpawnPoint = 0xea30e0b1,
    ActorKeyframe = 0x1fb9af22,
    ObjectFollow = 0x10ea9ec8,
    Pickup = 0x481ea5af,
    CameraManager = 0xbc9a60ae,
    ColorModifier = 0xb85d6790,
    Explosion = 0x21c14b0,
    ReloadSetLoader = 0x16702f15,
    Checkpoint = 0x660fc7c1,
    PlayerRespawn = 0x8c2ccfac,
    ImpulseDriver = 0xc0281ae7,
    Health = 0x6e89be7,
    Respawn = 0x9373fec0,
    ActorInteraction = 0xc7b43da6,
    CameraTargetPlayer = 0x78d8893a,
    PoiObject = 0xce22001e,
    Sound = 0xfdd83489,
    PerformanceGroupController = 0xccad4bd9,
    TouchSet = 0x502506d6,
    LiquidVolume = 0xe1e1c49c,
    NearVisible = 0x6c5d597d,
    LiquidInhabitant = 0x3449a5df,
    CharacterPrimitivesCollision = 0x591d8f55,
    TriggerForce = 0xbd4cfa2f,
    GroupSpawn = 0x2131c235,
    AnimationGridController = 0x2751ebf2,
    TimerAnimationGridParamProvider = 0x18b96d29,
    CameraShaker = 0x343a47f7,
    PlayerActor = 0x7e6063c8,
    Tippy = 0x91f22dcf,
    CinematicCameraShot = 0x89f426f2,
    ConveyorModifier = 0x1eb2749f,
    FogVolume = 0x73aecd92,
    Retronome = 0x845e492a,
    WaterRenderVolume = 0x23c5dff4,
    CinematicSkipHandler = 0x55f80cce,
    InventoryItem = 0xf34edd1f,
    EndGame = 0x18c0ad0f,
    UVTransform = 0x6302bbb3,
    MusicData = 0x25899782,
    MusicStateController = 0x1ac8c8fd,
    MusicSystemTransport = 0x47a60dc3,
    AdapterManager = 0x5cf17bfe,
    RelayAutoFire = 0x7f6d1cba,
    GraphicalTransition = 0xd745dc42,
    RetronomeDriver = 0xfa3899c,
    CausticVolume = 0xa7d1c922,
    Playlist = 0xba796fef,
    VoiceEffect = 0x24d9d323,
    DSP = 0xf7602744,
    RoomSettings = 0x160e9af9,
    DynamicActorControl = 0x5746a908,
    TimeKeyframe = 0xf442a668,
    RelayProbabilityGameMode = 0xb6040870,
    SimpleShadow = 0x212bcdf5,
    AudioBusController = 0x655c5175,
    PlayerKeyframe = 0xf6751a5d,
    SurfaceControl = 0xbe7fc29,
    SimpleSound = 0xb6ec1a51,
    LightGroupProxy = 0xd2cac9a5,
    LevelDarkener = 0xbd040603,
    RenderGroup = 0xa72a9926,
    SkinSwap = 0x3abcff68,
    StreamedMovie = 0xbff963d2,
    BloomEffect = 0x7dcaf170,
    ProjectedSimpleShadowReceiver = 0x1243c3e3,
    LoadUnitController = 0x4c53a836,
    DynamicLoadManager = 0xee0d6fd4,
    AudioBusMixer = 0x62913993,
    FSMManager = 0x8832ea4d,
    RoomController = 0x83cc17aa,
    PathFind = 0xd0368513,
    OrbitTarget = 0xceafd99c,
    NavigationMesh = 0x49bd721e,
    NavigationMeshHint = 0x4490513e,
    NavigationOffMeshLink = 0x85298bea,
    Tonemap = 0xddea916d,
    ScreenSpaceAO = 0x8061e045,
    TextPane = 0x38acaaf1,
    InventoryTextPane = 0x79738030,
    ContextInteraction = 0x59cfbbd7,
    DetectionPlane = 0x69091b92,
    CameraFilter = 0xb0c9b2f9,
    AnimatedMeter = 0xad2098fa,
    ModCon = 0x451740eb,
    GlobalStateMonitor = 0xecd265d2,
    RelayMemory = 0x91c301a5,
    EnvironmentVarModifier = 0xf05bc9c3,
    Terrain = 0x2ba4912d,
    ControllerAnalogShapeAction = 0x998ffc9,
    UICamera = 0xb35b0375,
    Drivable = 0x244019eb,
    Occlusion = 0xf6a30345,
    AnimatedSprite = 0xd8726471,
    PlayerEventListener = 0xf06bbe92,
    EnvironmentVarQuery = 0x4c183ebb,
    ControllerAnalogMovement = 0xc6f6f28c,
    TriggerInhabitant = 0x91f73d69,
    CameraTargetCreature = 0x9e806c4,
    UIButton = 0x13daaac,
    ControllerAnalogInputDriver = 0xb7332353,
    GradientAmbient = 0x87efbcf0,
    CameraSystemHint = 0x59372fe6,
    CollisionProbe = 0xa3b90913,
    HUDSuppressor = 0xa02cf18f,
    UIProxy = 0xc80ddc,
    GameFlowRelay = 0x70292d3d,
    SettingsMenuManager = 0x91666769,
    AnalogDriver = 0xdec78ecb,
    EnvironmentVarSender = 0xc26abe1a,
    EnvironmentVarListener = 0x2f0c8179,
    ControlCommandDisabler = 0x29b7e32a,
    ControlCommandSpoofer = 0xf0d1bcc7,
    AiTarget = 0x9e20d0af,
    UIRelay = 0x2376de64,
    TriggerProgressionVolume = 0x780e031e,
    Projectile = 0xb692d28b,
    FrontEndManager = 0x8f472283,
    RelayFrameDelay = 0x455285e2,
    GameFlowHub = 0x46b12081,
    NavigationAreaLink = 0x676a5ac4,
    DamageResponderVulnerability = 0x736dd919,
    GameFlowProxy = 0x10be69f9,
    ObjectSelector = 0xed02de02,
    AIGeneratorSelector = 0x55960d16,
    AIGenerator = 0xac073df4,
    Label = 0x78add45f,
    RelayAttackTypePayload = 0xf20f2698,
    Faction = 0xafd50f72,
    ParticleVolume = 0x3fcf5a88,
    PeriodicAdditiveAnimationDriver = 0xb793d90a,
    AnimVisibilityStateGroupManager = 0xd34ab306,
    PlayerDeathFall = 0xcba2fc47,
    SaveSlot = 0x8b306e38,
    CameraAdditiveFOV = 0x8666855a,
    TriggerDeathFall = 0xfd46f49e,
    RelayOriginatorFilter = 0x5959db91,
    ObjectTeleport = 0xe1c825c9,
    PlayerTeleport = 0x238467f5,
    CameraPredictivePitchHint = 0x514043dd,
    PlayerInventoryEffects = 0xc338fd04,
    CameraPredictiveYawHint = 0x322f85e1,
    ToneData = 0x67598be,
    UIMenu = 0x7edf6b2,
    UIWidget = 0x3cfffc01,
    RhythmMatcher = 0x3dfd89f7,
    ToneSelectorSequence = 0x8f4a3a35,
    ToneSelectorScriptDriven = 0xd0e99272,
    CinematicActor = 0xd298fc35,
    SlideShow = 0x7a6ed813,
    BoostPad = 0x854924d1,
    SetInventory = 0x7a38cc26,
    ToneSelectorHint = 0x4adc88b7,
    PathGenerator = 0xb3b55602,
    UIListView = 0x5445f685,
    MIDINoteAdaptor = 0x84bb17fa,
    NavigationMeshDynamicObstruction = 0x3536e079,
    CameraPointOfInterest = 0xc16b7565,
    AITeleportProxy = 0x41ad2545,
    Cinematic = 0xc9eebc83,
    AABoxMagnet = 0xd09fbbe7,
    FlowPath = 0xf9f62fd4,
    MIDISequence = 0x18e10190,
    MIDIRetronome = 0x76aac155,
    ToneSelectorMIDI = 0x49d039cd,
    Dock = 0xa10baa67,
    NavigationMeshDock = 0xf190a097,
    GibManager = 0x6221a411,
    ScreenShot = 0xd3e0d14,
    MIDIMatcher = 0x2249de78,
    ControllerMotionDriver = 0xae579212,
    SetAnimVisibilityState = 0xd2bcb71a,
    RenderClipPlane = 0x21a978d3,
    IncandescenceModulator = 0x1955e30c,
    UISlider = 0x5f0fa37c,
    SampleBankHint = 0xcf9e2373,
    AudioSamplerProxy = 0xfc397b56,
    SubtitleSequence = 0x57ab06b5,
    PhaseToEvents = 0x921362,
    SplineMotionPhaseResponder = 0xc6c04116,
    ActorKeyframePhaseResponder = 0x35fa03a6,
    SimpleSoundPhaseResponder = 0xf3bc4ee8,
    MIDINoteEmitter = 0x4ca2e586,
    Boolean = 0x5dd288e,
    LogicGate = 0x9340932c,
    RenderDecal = 0xd7b10efd,
    PatternMatcher = 0xdf279c9b,
    MIDIControllerMessageEmitter = 0x9d97d9c1,
    SoundVolumeModifier = 0xc03cd39d,
    Reactivator = 0xf75add4d,
    SampleBankData = 0x8be6648a,
    ActionControlPoint = 0xe795d2b1,
    MusicStateTransitionData = 0xa86001c4,
    Footprint = 0x74bd547a,
    SamplerVoiceBankData = 0x61ef47c6,
    PlayerSpawnTeleporter = 0x916f072f,
    AudioSampleBank = 0x17982019,
    AudioSampler = 0xdc777c06,
    HardwareProxy = 0xea6d58e8,
    RoomPreloader = 0xb2da6936,
    BroadcastSpatialListener = 0xef9abe4,
    BroadcastSpatial = 0x9aafcb03,
    AudioBusDriver = 0xd515e89d,
    AudioBusDriverInteractor = 0x2c9918ae,
    CombatStateProxy = 0x1a4bc6bd,
    MIDITransmitter = 0xe6a64862,
    MIDIReceiver = 0x4f10becb,
    MIDIResponderSampler = 0xf12e9c71,
    RuleSetManager = 0x3325a532,
    MIDIResponderArpeggiator = 0xd9c9a3f5,
    MIDIResponderChordMaker = 0xba249e78,
    TemplateManager = 0xd645278a,
    MIDIResponderPatternMatcher = 0x11f81321,
    AddActorEntityFlag = 0xd813a35a,
    ColorSampler = 0x12931ce3,
    PickupManager = 0x57dc20f,
    SoundPitchModifier = 0xd3fd952c,
    ActorVertexAnimationPlayList = 0x75560ce3,
    ActorVertexAnimationPhaseResponder = 0x412e0d82,
    BGMHint = 0x5cb0de9c,
    LightShafts = 0x37e758da,
    Choreographer = 0x8ce59f27,
    RelayChoreography = 0xd7c7e6f9,
    AudioSamplerPhaseResponder = 0xd54b0dcc,
    PhaseLooper = 0x8ce6e4f6,
    CameraLetterBox = 0x945e0902,
    ReflectionProbe = 0x27807e39,
    HeatDistortionHint = 0xd008829f,
    CameraProxy = 0x5e3f39ee,
    LavaFlowSurface = 0xf75ea7ef,
    FogShape = 0x92561a4a,
    DockDoor = 0xae371a20,
    GenericSplineControl = 0xcc8f9ff2,
    ControlSplineMappingsDefinition = 0x682591,
    Scannable = 0x157533dd,
    PhaseRelay = 0xc04d4753,
    Skybox = 0x5112a065,
    ControlSchemesDefinition = 0x637e29a,
    ControlSchemeAccessor = 0x60e107e0,
    AudioObstruction = 0xece8e32e,
    BroadcastGlobal = 0xa482e214,
    BroadcastGlobalListener = 0x3e9f6170,
    CreatureBase = 0x986ad9ea,
    PickupDropper = 0x720a3ac5,
    AnimationController = 0xbca078f7,
    FSMController = 0x8f6c4769,
    CollisionLogic = 0x4368e960,
    PhysicsDriver = 0xa3b4fd2,
    CreatureMovement = 0xa60a6693,
    CreatureHealth = 0x81519976,
    RegisterEntity = 0xcb54e84d,
    ProxyRegisteredEntity = 0x6654ca01,
    CharacterPrimitives = 0x6548cfcc,
    Steering = 0x2c9d30b,
    WorldTransition = 0x564dee0f,
    CreatureRules = 0x4a8d8e14,
    CreatureTargeting = 0x2095a979,
    TerrainAlignment = 0x2d0f036c,
    CreatureLeashBehavior = 0xe9b9f6ee,
    CreatureChangePostureBehavior = 0x97422c3f,
    CreatureTurnBehavior = 0x8bb84eb1,
    CreatureNavigateBehavior = 0x228a335d,
    CreaturePatrolBehavior = 0xfc76d74c,
    CreatureDeployActionBehavior = 0x949e5bbe,
    CreatureWanderBehavior = 0x24eb1a32,
    CreatureSpawnBehavior = 0xc1c02476,
    CreatureRangedCombatBehavior = 0x834cf41d,
    CreatureMeleeCombatBehavior = 0x45924f80,
    CreatureHitReactionBehavior = 0xcdb24fad,
    CreatureHecklerBehavior = 0x2e8a5117,
    CreatureDespawnBehavior = 0x52bfd9ad,
    CreatureFallBehavior = 0x1b1ebdd,
    CreatureActionBroadcaster = 0xa3fe9467,
    CreatureAttackPathBehavior = 0xf3133cdf,
    CreatureScriptedAnimationBehavior = 0xabe969d,
    CreatureAmbushBehavior = 0x6b3cdb28,
    CreatureEvadeBehavior = 0xc062fd1a,
    CameraTargetProxy = 0x2bc6608a,
    VolumetricFog = 0x1b9cd84f,
    CreatureFlinch = 0x874aa246,
    VolumetricFogRegion = 0xaffe9cf9,
    ResetObject = 0x7db10f4c,
    Scoring = 0x7d3512d7,
    ScorableTarget = 0x3254e042,
    DynamicControlScriptedRealDefinition = 0x40f9629d,
    InventoryInitializer = 0x24d8fdd0,
    GameOptionsDefinition = 0xb28e4996,
    GameOptionsAccessor = 0xa190eaa2,
    CreatureWaitAtPositionBehavior = 0xa38743d1,
    CreatureContextInteractionBehavior = 0x81718106,
    CreatureContextInteraction = 0x931b692f,
    BoolToRealMap = 0xde3024e9,
    EnumToRealMap = 0x69504acb,
    IntToRealMap = 0x8c9b8856,
    RealToRealMap = 0xecffb88f,
    CoverPoint = 0xc0366dc7,
    CreatureCoverCombatBehavior = 0x9e73eb5d,
    CreatureArmor = 0x1f969d66,
    RealNumberFunction = 0x2cdaf986,
    RealNumber = 0xd855b36e,
    RealNumberComparison = 0xbc121d69,
    DebugMenuItemsDefinition = 0x5a82703d,
    CreatureFleeBehavior = 0x2ab5ef6f,
    TimeDilation = 0x3ec5d79a,
    CreatureFireDamage = 0x5904644c,
    CreatureHeadTracking = 0xdb0310b9,
    OriginatorProxy = 0x9e219714,
    FleePoint = 0xc55dc4e,
    Enumeration = 0x528c1159,
    CreatureAiming = 0x87cf06df,
    ProjectileIntersectionHint = 0xfda2465a,
    CreatureIceDamage = 0x86692789,
    AIPostOwner = 0x71a1c997,
    WanderPoint = 0xe262f4af,
    LineOfSight = 0xb1a9eced,
    CreatureThunderDamage = 0xd73ebb11,
    CreatureProjectileLauncher = 0xde991e1e,
    ChainLightning = 0x6027b2ce,
    CreatureSecondaryAnimation = 0x16169aea,
    CreatureSecondaryActions = 0xd1bbf0e7,
    AudioPluginEffect = 0x57312ed0,
    AnimationRateModifier = 0x97947f1e,
    CollisionWithWorld = 0x27568f71,
    CreatureScriptedJumpBehavior = 0x51422a18,
    ScriptedJumpData = 0x331613d1,
    SniperPoint = 0xbc5eb9d5,
    CreatureSniperBehavior = 0xd8a9f583,
    GenericAimTarget = 0xfa73d359,
    ControlCommandMappingPhysical = 0x65823457,
    ControlCommandMappingNode = 0xecdce2e3,
    ControlCommandMappingsDefinition = 0x7cdfbc83,
    ReverbFieldNode = 0x712755e8,
    InputMacroPlayer = 0x6467b289,
    InputMacroRecorder = 0x83eb0628,
    ControlCommandMappingOne = 0x98aacf09,
    ControlCommandMappingZero = 0x48c1cef7,
    ControlCommandMappingScriptedReal = 0xe33f78b,
    ControlCommandMappingAdd = 0x66ad5099,
    ControlCommandMappingDebugMenuBoolean = 0xdaee1aad,
    ControlCommandMappingDebugMenuInt = 0xcc5ae725,
    ControlCommandMappingDebugMenuRealEvaluator = 0xab505eab,
    ControlCommandMappingDebugMenuRealMultiplier = 0x8b123910,
    ControlCommandMappingMultiply = 0xee0426d8,
    ControlCommandMappingAnd = 0xac392770,
    ControlCommandMappingNot = 0xdb4ab61d,
    ControlCommandMappingOr = 0x81ec3bb6,
    ControlCommandMappingSpline = 0x94147bd8,
    ControlCommandMappingNegate = 0x571f8fd2,
    ControlCommandMappingNegativeOne = 0x7b8db9c7,
    HealthPhases = 0xe7c5ba34,
    AiTargetManager = 0x88e7782,
    RumbleEffectsDefinition = 0x12d9053f,
    RumbleSensoryDefinition = 0x1f0afab0,
    RumbleEmitter = 0x224f782c,
    FactionManager = 0x3bbbe401,
    CreatureTurnProcedural = 0x7fcbc2ad,
    RealNumberPhaseResponder = 0x3713fd4f,
    CreatureActionPatternBehavior = 0xa619d230,
    PathMovement = 0x1a828893,
    CreatureProneBehavior = 0xd36b0797,
    PathMovementMagnetizationPoint = 0x9c7df587,
    DistanceCompare = 0x46033f8d,
    CreatureTargetable = 0xefed81f1,
    WindWaker = 0x53744d89,
    AnimationEventListener = 0xc3ccc605,
    CreaturePositioning = 0xd5dcb013,
    TriggerDamage = 0xa9584c89,
    CreaturePositioningManager = 0x5ba03233,
    AttackManager = 0x3da7da05,
    AttackManagerTest = 0x6acaa7da,
    FSMHotSwapper = 0x2ce325,
    ScriptedAnimationData = 0x80b94fcd,
    HeightfieldSurfaceDescription = 0x3a080954,
    AnchorPoint = 0x44ed3d2,
    CreatureScriptedActionBehavior = 0xfc7d9a11,
    CreatureSplineFollowBehavior = 0xfb26fd7d,
    LavaRenderVolume = 0xa7ee9c33,
    CreatureGrabTarget = 0xf33fd0c8,
    BossMeterState = 0xd4ac7206,
    PlayerSpawnPoint_InputMacroRelay = 0xce2470cb,
    AudioEmitter = 0xeea718b9,
    NavigationMeshIncludeExcludeHint = 0x20c7601e,
    AnimationVariableReal = 0x1f8ca08e,
    CounterPhaseResponder = 0x47e7ee64,
    AutoExposureHint = 0x98694074,
    ShockWave = 0xcd85a847,
    ColorGrade = 0x6b091e44,
    VolumetricFogHint = 0x84fb5798,
    PointOfInterest = 0xafb53a8c,
    ColorGradeHint = 0xa36cd908,
    EffectVariableDriver = 0xfd5075f3,
    TimerProgression = 0xef5e0348,
    ProjectedShadowBlob = 0x414d77ff,
    CreatureCollisionAvoidance = 0x444959ad,
    SoundFilterModifier = 0x512a3ee4,
    DistanceCompareGroup = 0x9f61cd7f,
    PhaseCombinator = 0xb9fac2df,
    CollisionAvoidanceManager = 0x536567,
    BloomEffectHint = 0xf66b9100,
    AngleCompare = 0xdae5d368,
    AudioOcclusionVolume = 0xc3049839,
    AnimationUserEventRouter = 0x917fd37d,
    AnimatedMeterPhaseResponder = 0xe493532f,
    Backlight = 0x190d20d7,
    NavigationPathPoint = 0xce69684b,
    NavigationPathPointOffMeshLink = 0x37b90174,
    ChromaticAberration = 0x2b854b47,
    SoundLevelMeterPhaseDriver = 0x99dc9c6e,
    AudioPluginEffectCrossfadePhaseResponder = 0x8e881c85,
    LightHint = 0x56cf02a6,
    AudioObstructionOcclusionOverride = 0x135c010e,
    BroadcastAudioEvent = 0x62fc6e51,
    BroadcastAudioEventListener = 0x458501cf,
    GameVariableAccess = 0xea33d242,
    PerformanceHint = 0x6f5a20be,
    AudioBusPhaseResponder = 0x62c5d8c7,
    CounterAdapter = 0x4812e080,
    TimerStopwatch = 0xd81587e0,
    HealthDisplay = 0x7e394ff2,
    HealthDisplaySource = 0xad67f77f,
    SurfaceGenerator = 0x4eebaad3,
    BlackboardBoolean = 0xa41fc3a9,
    BlackboardRealNumber = 0x13d0f71d,
    BlackboardPhaseRelay = 0x25e4d42f,
    BlackboardBooleanWriter = 0xc8a56ecc,
    BlackboardPhaseRelayWriter = 0xbfb2c3ae,
    BlackboardRealNumberWriter = 0x12a1d49f,
    FSMMessage = 0x7c9b685c,
    CreatureHurlBehavior = 0xcf272bd6,
    AudioPluginEffectCrossfadeSend = 0xc5bfd655,
    TimerSequencePhaseResponder = 0xa623afa1,
    AmbientParticleEffect = 0xcbe2042d,
    RealValueRelay = 0x63dd6841,
    RealNumberDriver = 0xd81ef16f,
    Beam = 0x392dcdf1,
    SnapToPath = 0x1d4532db,
    ScriptedMotion = 0xf78649c8,
    DamageRelay = 0x75c4e493,
    POIRoomController = 0x5b5fb670,
    ScreenCoverageTrigger = 0x12c9e761,
    VolumetricFogRegionTransition = 0xdf95ac1a,
    AudioOutputEffects = 0xf6559cea,
    AnimationPlaybackRate = 0xeb5b364d,
    Condition = 0x76025bb8,
    ScriptedMotionPhaseResponder = 0x50b4f3d7,
    PlayerRoomTeleporter = 0x7fae7590,
    CreaturePositionLogic_WaypointScripting = 0x2a7e80cc,
    Credits = 0xa60583cc,

    // MP1
    ActorMP1 = 0xb6200be6,
    ColorModulateMP1 = 0xa856f484,
    CounterMP1 = 0x32aef7dd,
    DebugActorMP1 = 0x59bc6b3b,
    WaypointMP1 = 0x8e4c86ec,
    PlayerMP1 = 0x99ea4919,
    SpawnPointMP1 = 0x79f02e11,
    EnergyProjectileMP1 = 0x7003c6aa,
    ControllerActionMP1 = 0x42a7692d,
    PlasmaProjectileMP1 = 0xf5ce0e14,
    BombMP1 = 0xe9f08e02,
    PowerBombMP1 = 0xd62862ca,
    NewFlameThrowerMP1 = 0x2ca49de4,
    WaveBusterMP1 = 0x7191e814,
    ExplosionMP1 = 0x50e62ae4,
    ShockWaveMP1 = 0x2cb074d1,
    FlameThrowerMP1 = 0x618b3777,
    IceAttackProjectileMP1 = 0xb130ab71,
    DamageEffectMP1 = 0xdaf49ad2,
    CollisionActorMP1 = 0x7059aa54,
    ScriptRelayMP1 = 0x1b78f9a4,
    TimerMP1 = 0x25c6d2a3,
    TriggerMP1 = 0xf526fc2a,
    SwitchMP1 = 0xc0e28b3d,
    AiJumpPointMP1 = 0xefa09db4,
    RumbleEffectMP1 = 0xa7c1e08d,
    ActorKeyframeMP1 = 0x3ce6630a,
    SteamMP1 = 0xfc64463e,
    BallTriggerMP1 = 0x50f2f67a,
    FirstPersonCameraMP1 = 0x1fab791d,
    BallCameraMP1 = 0x76a6385f,
    FreeCameraMP1 = 0x4e9f22e,
    InterpolationCameraMP1 = 0xe44d80f3,
    SpecialFunctionMP1 = 0x71337f4f,
    AmbientAIMP1 = 0xa497918d,
    ActorRotateMP1 = 0x3391e02b,
    MemoryRelayMP1 = 0x1205609a,
    PlatformMP1 = 0xa12967fb,
    EffectMP1 = 0xb421cdcb,
    HUDMemoMP1 = 0x77e59f98,
    DamageableTriggerMP1 = 0xdcb49607,
    WaterMP1 = 0x12db855d,
    CameraMP1 = 0x2eaec98,
    VisorFlareMP1 = 0x8bedb563,
    DebrisExtendedMP1 = 0x6665f6df,
    DebrisMP1 = 0xe7a0171,
    WorldTeleporterTooMP1 = 0x2fa104ff,
    PlayerActorMP1 = 0xdb389155,
    CameraWaypointMP1 = 0xf649eec3,
    AreaAttributesMP1 = 0x48f8341d,
    PointOfInterestMP1 = 0xd5c1882,
    CameraBlurKeyframeMP1 = 0xcc92b31b,
    CameraFilterKeyframeMP1 = 0x750702ea,
    StreamedAudioMP1 = 0x7e1f2807,
    SoundMP1 = 0x46787ddd,
    MidiMP1 = 0x3bab18a0,
    VisorGooMP1 = 0xcb5682c4,
    CameraShakerOldMP1 = 0x9b0f9c29,
    CameraShakerNewMP1 = 0x99dfc912,
    RandomRelayMP1 = 0xdf4fcca3,
    NotSTD_DockMP1 = 0x3168fe6,
    PickupMP1 = 0x36386a69,
    GrapplePointMP1 = 0xeb82e3b,
    ContraptionMP1 = 0xb5cd58cd,
    DoorMP1 = 0x564a1641,
    RoomAcousticsMP1 = 0xcd695064,
    PlayerHintMP1 = 0x23f1e994,
    GunTurretMP1 = 0x1704c1f0,
    CameraHintTriggerMP1 = 0x4ca870ef,
    GeneratorMP1 = 0x8ee5d87e,
    WallCrawlerSwarmMP1 = 0x16b8b319,
    DistanceFogMP1 = 0x2131945e,
    CameraHintMP1 = 0xad191c7e,
    EnvFxDensityControllerMP1 = 0x3c832ec7,
    ElectroMagneticPulseMP1 = 0x7d070ef0,
    CameraPitchVolumeMP1 = 0x176f6f27,
    PickupGeneratorMP1 = 0x4ca59eb7,
    MazeNodeMP1 = 0x47e5d68b,
    PathCameraMP1 = 0xc6e2de1f,
    CoverPointMP1 = 0x2b3ad4b6,
    RadialDamageMP1 = 0x93c5b36d,
    PhazonPoolMP1 = 0xacc3ce1e,
    SpiderBallAttractionSurfaceMP1 = 0xd4cd5e63,
    SpiderBallWaypointMP1 = 0xc7668e15,
    TargetingPointMP1 = 0x48ae7ca9,
    IntroBossMP1 = 0x65ee9092,
    RippleMP1 = 0x3ab7170d,
    TeamAiMgrMP1 = 0xc30c5758,
    ThermalHeatFaderMP1 = 0x33d9ffa6,
    SpindleCameraMP1 = 0x36b8d0d0,
    SpankWeedMP1 = 0x705b28fc,
    ShadowProjectorMP1 = 0x17db2246,
    WorldLightFaderMP1 = 0x7a4bc122,
    ThardusMP1 = 0xeaa40b01,
    AThardusRockProjectileMP1 = 0x2249224e,
    AtomicAlphaMP1 = 0x61653ed2,
    AtomicBetaMP1 = 0x87412090,
    GeemerMP1 = 0xb99d2b06,
    OculusMP1 = 0x9eaf010,
    PufferMP1 = 0xada0ec66,
    BouncyGrenadeMP1 = 0xf800eff9,
    RidleyMP1 = 0x4f9d71ec,
    BabygothMP1 = 0x254d4d49,
    TryclopsMP1 = 0x1498ba74,
    BloodFlowerMP1 = 0x4deb1e3a,
    FlyingPirateMP1 = 0x6cfa6902,
    BurrowerMP1 = 0x1c3d435d,
    ChozoGhostMP1 = 0x84b92d36,
    DroneMP1 = 0x50b7576d,
    SpacePirateMP1 = 0xeb1f342,
    ElitePirateMP1 = 0xf4326a02,
    OmegaPirateMP1 = 0x7f2845c7,
    PuddleSporeMP1 = 0x242c3c90,
    EyeBallMP1 = 0xc963cc17,
    FireFleaMP1 = 0x52ebc2a7,
    FlaahgraMP1 = 0x3c669acb,
    FlaahgraTentacleMP1 = 0x30a6b6de,
    FlaahgraProjectileMP1Runtime = 0xfffba0c,
    FlaahgraPlantsMP1Runtime = 0xd2806770,
    FlaahgraRendererMP1Runtime = 0xdb9430c7,
    ParasiteMP1 = 0xbe462b0e,
    FlickerBatMP1 = 0x1a03f50a,
    GarBeetleMP1 = 0xe8ca3ea0,
    IceSheegothMP1 = 0xcbe62855,
    WarWaspMP1 = 0xb6e36fd4,
    JellyZapMP1 = 0xfe939195,
    MagdoliteMP1 = 0xb0c4378e,
    MetareeMP1 = 0x1230fa75,
    MetroidBetaMP1 = 0x55032cda,
    SeedlingMP1 = 0xb6b2691,
    TargetableProjectileMP1 = 0xbcf96c94,
    RipperMP1 = 0xb5d157db,
    SnakeWeedSwarmMP1 = 0xac1c4aad,
    MetroidMP1 = 0x7fc7debc,
    PuddleToadGammaMP1 = 0x719ee8c2,
    FishCloudModifierMP1 = 0x41bc06dd,
    MetroidPrimeRelayMP1 = 0x622f6b23,
    GrapplePointMP1Runtime = 0x5048d44f,
    ControlledPlatformMP1Runtime = 0xd909c936,
    FishCloudMP1 = 0x4e8a3ce2,
    PhazonHealingNoduleMP1 = 0xa68d2eb5,
    MetroidPrimeMP1 = 0xae757b71,
    ProxyPlayerMP1 = 0x5797d3c7,
    MetroidPrimeStage2MP1 = 0x980ef312,
    VisorFlareMP1Runtime = 0x9b592020,
    DroneLaserMP1Runtime = 0xd39988c0,
    DeathCameraEffectMP1Runtime = 0xdcf9614b,
    ElitePirateGrenadeLauncherMP1Runtime = 0x27116d98,
    PoisonProjectileMP1 = 0x7cf4c8c0,
    MissileTargetMP1 = 0x1e3517fc,
    DestroyableRockMP1Runtime = 0x42020da4,
    RoomOcclusionOverrideMP1 = 0x6ea08bad,
    ElectricBeamProjectileMP1Runtime = 0x82980e58,
    CameraOverrideMP1 = 0xe5740bfd,
    AnimatedCameraMP1 = 0xcb3b5bea,
    ARepulsorMP1 = 0x85e5e721,
    PlayerStateChangeMP1 = 0x90905f24,
    AEnergyBallMP1 = 0xae09e5e2,
    AScriptBeamMP1 = 0x515541d,
    SustainedPlayerDamageMP1Runtime = 0xeb70cd36,
    HUDManagerMP1 = 0x104a7116,
    ScriptHUDBillboardEffectMP1 = 0x7383d6b5,
    RumbleEventResponderMP1 = 0x333f83d4,
    CinematicStateProxyMP1 = 0x14d107fe,
    HUDBillboardFreezeEffectTestMP1 = 0x63b6ecdc,
    ScriptedOcclusionVolumeMP1 = 0xc1fac87d,
    MaterialVariableDriverMP1 = 0x8a12a2f9,
    EffectProxyMP1 = 0xe12aae70,
    CinematicMP1 = 0xf06785e1,
    PhazonDriverMP1 = 0x73a6d8f1,
    AmbientParticleEffectPrimitiveShapeProviderMP1 = 0x55a188f6,
    XRayHintMP1 = 0x98eea4ca,
    CameraWaterStateProxyMP1 = 0x6a7a53b0,
    WaterTransitionMP1 = 0xdf22bd66,
    AreaOcclusionModifierMP1 = 0x8b8689d,
    BakedLightingPriorityModifierMP1 = 0x9feab9b3,
    AutoExposureHintMP1 = 0x451c85b6,
    ActorIceDarkenerMP1 = 0x427c6c50,
    IceImpactMP1 = 0xcac82b67,

    // MPT
    FrontEndManagerMPT = 0x72f2a8fe,
    SaveSlotMPT = 0x983b318f,
}

#[binrw]
#[brw(repr(u32))]
#[repr(u32)]
#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ETypedefInterfaceType {
    // Complex
    ApplyDamageAlongPathActionPayload = 0x6afe5850,
    ArpeggiatorModeActionPayload = 0x58e36260,
    AttackTypePayload = 0x5e7c44c4,
    BoolActionPayload = 0x3346656e,
    ChoreographyColorPayload = 0xb95151fc,
    ColorActionPayload = 0x3cd78da5,
    ContextInteractionStatePayload = 0x4cb17288,
    ControlSchemeCommandIcon = 0x8c9bff9,
    ConvergenceActionPayload = 0x4ce75ae1,
    CRC32ActionPayload = 0xce5607a2,
    DamageInfoPayload = 0x42a85d6f,
    DataEnumValueActionPayload = 0x2cef3d77,
    DeleteMySetAfterOneShotsActionPayload = 0x961dd741,
    DistanceComparePayload = 0x541693ac,
    DockMessagePayload = 0x88717fcd,
    FactionPayload = 0xe8133192,
    FactionResponsePayload = 0xe161f2c2,
    GameOptionBoolActionPayload = 0x96a29021,
    GameOptionEnumActionPayload = 0xf7c47725,
    GameOptionIntActionPayload = 0xac0f7e64,
    GameOptionRealActionPayload = 0x266916ca,
    HDRColorActionPayload = 0x63ebea0f,
    HealPayload = 0xff642290,
    IntActionPayload = 0xc2add041,
    InventoryActionPayload = 0xeab00801,
    LabelActionPayload = 0xd9026e05,
    ListViewActionPayload = 0x357ec713,
    LocomotionAnimationPayload = 0xefa05071,
    MIDIChannelActionPayload = 0x6c015db9,
    MIDIChannelDoubleActionPayload = 0xa90d69d5,
    MIDIControllerActionPayload = 0xb95b6c4c,
    MIDINamedControllerActionPayload = 0xbb1c284,
    MIDINoteActionPayload = 0x2801cdd2,
    MIDIValueOverride = 0x83a02668,
    MinMaxRealActionPayload = 0x35227470,
    MovementContextActionPayload = 0xf14344fa,
    ObjectIdActionPayload = 0x6c4ee3c4,
    PatrolSpeedActionPayload = 0x517f96ce,
    PatternMatcherTokenInputPayload = 0x1185d9bb,
    PhaseActionPayload = 0x22647f6f,
    PhaseLooperActionPayload = 0x90f5e5e5,
    PickupSelector_Generic = 0x5108ccd0,
    PickupTossStatePayload = 0x4cead793,
    PlayAnimByNamePayload = 0x508cb36b,
    PosturePayload = 0xac5f17a5,
    ProjectileLauncherTargetInaccuracyPayload = 0x29e905e5,
    QueryPlayerStateActionPayload = 0x7ea320c4,
    RealActionPayload = 0x2bfd7e9c,
    RealCompareActionPayload = 0x9cb93e77,
    RenderVertexAnimatedModelPlayAnimPayload = 0xe791385d,
    RenderVertexAnimatedModelSetAnimPhasePayload = 0xc9aa7f0f,
    RumbleActionPayload = 0xe4d57f9c,
    ScriptDataRuleConditionPayload = 0xbec867ea,
    ScriptedMotionDirectionActionPayload = 0x2342d564,
    ScriptedMotionDirectionActionPayloadWithTag = 0xce897384,
    ScriptedMotionTimestampActionPayload = 0xf6c87229,
    ScriptedMotionTimestampActionPayloadWithTag = 0x6ff9a00f,
    SequenceRhythmActionPayload = 0xb32f793e,
    SetAllowRenderPayload = 0xd1975cf3,
    SetTimeWithOffsetPayload = 0x4c298f57,
    SlotSelectActionPayload = 0x1dd71ca2,
    SpawnActionPayload = 0xfc857869,
    SplineRebuildActionPayload = 0xf714c12b,
    StringActionPayload = 0x4f0cd05e,
    TransitionActionPayload = 0xb3070511,
    TutorialActionPayload = 0x844456c,
    UnitVectorPayload = 0xaa6c960d,
    VectorActionPayload = 0x1072468c,
    WidgetStateActionPayload = 0x781cf6a7,
    WindImpulseDataActionPayload = 0x58d3e0a8,
    AnimOverIndexEventCriteria = 0x7c1c5173,
    AnimStartedIndexEventCriteria = 0xfc308e0,
    AttackTypeEventCriteria = 0xea065305,
    BoolEventCriteria = 0xbcb6fefa,
    BounceJumpEventCriteria = 0xe1db472f,
    ChoreographyColorEventCriteria = 0x8419534c,
    ContextInteractionStateEventCriteria = 0x322bfeb1,
    CounterConditionEventCriteria = 0xf0e34255,
    CRC32EventCriteria = 0xf4a7d0d6,
    DataEnumValueEqualsIntegerEventCriteria = 0xdc0cc904,
    DataEnumValueEventCriteria = 0xa62a133d,
    DirectionEventCriteria = 0x7c471701,
    DockMessageEventCriteria = 0x8a70d4c9,
    IntCompareEventCriteria = 0x96bcd2cf,
    IntEventCriteria = 0xceae520,
    LiquidTypeEventCriteria = 0x19bd33ba,
    MIDIControllerEventCriteria = 0x63ab26c4,
    MIDINoteEventCriteria = 0xab15c81a,
    PickupEventCriteria = 0x5b00054,
    PostureEventCriteria = 0x5f8f982f,
    ProjectileTypeEventCriteria = 0x90c7584d,
    RealCompareEventCriteria = 0x96686e6c,
    RealEventCriteria = 0x72268f18,
    RespondToDamageResultEventCriteria = 0xc2f99479,
    ScriptedMotionReachedTimeEventCriteria = 0xe8fc4979,
    ScriptedMotionReachedTimeEventCriteriaWithTag = 0xb3d97bdf,
    SubWeaponSelectCriteria = 0xe65f6844,
    VisorTypeEventCriteria = 0xefaf4378,
    WindImpulseCompleteEventCriteria = 0xa1e65289,
    ActorActionPlaylistConditional = 0xf5b5ea5b,
    ActorActionPlaylistSequential = 0xf6715dba,
    CharacterPrimitivesData = 0x5c2dce7a,
    CreatureRuleSetData = 0xc1cb7111,
    FSMData = 0xb21e82e4,
    AimTargetingMultiTargetCentroid = 0xbcdadc8d,
    AIPostScoringCriterionClosestToForward = 0x5985296c,
    AIPostScoringCriterionDistanceToPoint = 0x8397dc23,
    AIPostScoringCriterionDistanceToTarget = 0x5c4acc16,
    AIPostScoringCriterionLinkedWanderPoint = 0x112c2a70,
    AIPostScoringCriterionRandom = 0x9380ed3b,
    AnimGridDriverTrackObject = 0xfe714ac1,
    AttackManagerTimeSinceLastAction = 0xc04c05d,
    BloomEffectInterpolation_Time = 0xf1be1936,
    AccumulatedTimeCameraDataInput = 0x2783fb7f,
    CounterAdapterLogic_Health = 0x9516c419,
    CreatureActionReference = 0xe720399c,
    CreatureActionVariant = 0x3ff09494,
    CreatureActionPatternRoundRobin = 0xa74dc8d0,
    CreatureActionPatternSequence = 0xffe7889a,
    CreatureConditionActorType = 0xa80fe573,
    CreatureConditionAiming = 0x6419129d,
    CreatureConditionAND = 0xda097ca1,
    CreatureConditionActionControlPoint = 0x306e21dc,
    CreatureConditionCollision = 0x23eec03b,
    CreatureConditionFacingTarget = 0x31eef58a,
    CreatureConditionFaction = 0x9e6048f5,
    CreatureConditionGrabbedTarget = 0x8f9f7f7d,
    CreatureConditionHealth = 0xd57c1798,
    CreatureConditionHealthDamagePreventedByArmor = 0xfb1b9078,
    CreatureConditionHealthPhase = 0x1c4dfc86,
    CreatureConditionIncomingProjectile = 0xbda26328,
    CreatureConditionLineOfSight = 0xf1b44da,
    CreatureConditionMoveDirection = 0x8c7db178,
    CreatureConditionMovementContext = 0xd988ff87,
    CreatureConditionMovementObstructed = 0x626ce52b,
    CreatureConditionNearbyActors = 0xe763a808,
    CreatureConditionNOT = 0x19e71fda,
    CreatureConditionOnPost = 0x30978088,
    CreatureConditionOnScreen = 0xc9bdf35c,
    CreatureConditionOR = 0x9c71bb96,
    CreatureConditionRecentlyAttacked = 0xec382ee2,
    CreatureConditionRecentlyDamaged = 0x68c63618,
    CreatureConditionRecentlyDamagedTarget = 0x9fc1f1b0,
    CreatureConditionRecentlySelectedAction = 0x76756c68,
    CreatureConditionRecentlyTurned = 0xe6577b10,
    CreatureConditionSelfAction = 0x75f4c370,
    CreatureConditionTargetVelocity = 0x974d5d66,
    CreatureConditionTimeInCombat = 0xfa3e613d,
    CreatureConditionUnobstructedPath = 0x28d5a413,
    CreatureConditionWithinHeight = 0xf7107974,
    CreatureConditionWithinRange = 0x73745802,
    CreatureConditionWithinRangeOfPlayer = 0x9f17d292,
    AdoptCameraStateBehaviorData = 0x61b8fb8,
    AnimatedCameraBehaviorData = 0xf5bd278d,
    CameraTargetOrientationBehaviorData = 0x895340ac,
    ChaseBehaviorData = 0x8836830b,
    ColliderPositionBehaviorData = 0xf3657a44,
    CollisionBehaviorData = 0x3a66469e,
    CombatPredictiveOrientationBehaviorData = 0x993b065d,
    DetectTargetInCombatBehaviorData = 0x5dc84558,
    FirstPersonAimBehaviorData = 0xde3a27b,
    FirstPersonFreeBehaviorData = 0xfc991ffc,
    FollowLocatorCameraBehaviorData = 0x156c1c64,
    FOVInputBehaviorData = 0x332eaf7b,
    FrameGroundPositionBehaviorData = 0xaf1299bd,
    FrameTargetBoundsPositionBehaviorData = 0x9e7e5c83,
    FrameTargetsPositionBehaviorData = 0xf380d4a8,
    FramingEnforcementBehaviorData = 0xba667a7d,
    FreelookBehaviorData = 0x9ab9ad47,
    FreelookOverrideBehaviorData = 0xcc3837b1,
    FreelookPitchBehaviorData = 0xdb36b1cf,
    FreelookYawBehaviorData = 0xd733ef08,
    HorizontalLeadPositionBehaviorData = 0x52245677,
    LineOfSightCollisionBehaviorData = 0x30808930,
    LockOnCameraBehaviorData = 0xf4d3fb0c,
    LookAtRotationBehaviorData = 0xe3fe85ea,
    LoopedMotionBehaviorData = 0xd59638af,
    MotionPredictiveOrientationBehaviorData = 0x1e855696,
    MoveSurfaceToTargetBehaviorData = 0x5791540c,
    OffsetPositionBehaviorData = 0xb5f34bd6,
    OrbitLookAtBehaviorData = 0x64f9033f,
    OrientationPathBehaviorData = 0x44a7d1aa,
    PanTiltBehaviorData = 0x86385ca6,
    PathPositionBehaviorData = 0xd9286fe9,
    PredictivePitchOrientationBehaviorData = 0x2582fb17,
    PredictiveYawOrientationBehaviorData = 0x2a6d92b6,
    PrimaryTargetTrackingBehaviorData = 0xf598b4b0,
    ResetDetectionCameraBehaviorData = 0x28c6d69a,
    RestrictLookAtBehaviorData = 0xcd640e79,
    RestrictPositionCameraBehaviorData = 0x244c1936,
    RotationBehaviorData = 0x99a5de2a,
    SidescrollerTrackingBehaviorData = 0x9b395543,
    SidescrollPositionBehaviorData = 0x775ea30a,
    SimpleMotionBehaviorData = 0xbb84c366,
    SpinCameraBehaviorData = 0x29219f57,
    SurfaceInputBehaviorData = 0xf5828b90,
    SurfacePositionBehaviorData = 0xe0368b3f,
    TargetWhiskersBehaviorData = 0x5ff8a028,
    TransformCameraHintBehaviorData = 0x74f8410a,
    VerticalLeadPositionBehaviorData = 0x26d83772,
    ControllerCameraDataInput = 0xc79ba739,
    ConvergeCameraDataInput = 0xb1503217,
    DelayDecreaseCameraDataInput = 0xeb849ee6,
    DisplacementFromCameraDataInput = 0xb4f0a604,
    TargetBoundingBoxSizeCameraDataInput = 0x8633a644,
    TargetSpeedCameraDataInput = 0xc1f9764f,
    CollisionFilterAnd = 0x12a0b563,
    CollisionFilterExclude = 0xe54547f0,
    CollisionFilterInclude = 0x2d0a9dd3,
    CollisionFilterOr = 0xccedcef1,
    CollisionFilterOrientation = 0x8683448a,
    CollisionFilterPreset = 0x4fa1b01d,
    CombatStateCondition_AND = 0x1e1613f2,
    CombatStateCondition_EnemyCountAndProximity = 0x2eedf359,
    CombatStateCondition_NOT = 0xacb8ef4a,
    CombatStateCondition_OR = 0xcdeeb8f3,
    CombatStateCondition_TargetProximity = 0xf3f4fbad,
    GaussianConvergenceData = 0xfc929767,
    PIDConvergenceData = 0x435c7f1c,
    ProportionalConvergenceData = 0xfce7c683,
    SpringConvergenceData = 0x66596967,
    VelocityConvergenceData = 0xc538d36f,
    CreatureNavigationLogic_Ground = 0xf32aab92,
    CreatureNavigationLogic_Wall = 0x2f34c084,
    CreaturePositionLogic_AnchorPoint = 0xb9331e95,
    CreaturePositionLogic_Circle = 0xefb3c0b1,
    CreaturePositionLogic_HoldPosition = 0x4053b041,
    CreaturePositionLogic_MoveToTarget = 0x2eb3cb6c,
    CreaturePositionLogic_RandomInDirection = 0x9a5f8c2d,
    CreaturePositionLogic_Waypoint = 0xedaaa59f,
    DebugMenuDataBool = 0x747f07e1,
    DebugMenuDataInt = 0x2d75771c,
    DebugMenuDataMenu = 0x80d8f0d7,
    DebugMenuDataProxy = 0xd6c19911,
    DebugMenuDataReal = 0xb1d91b9f,
    DeltaTimeModifier_PlayerSpeed = 0xe2ac39e2,
    DynamicControlAdd = 0xef7a42bf,
    DynamicControlAnd = 0x26d791dd,
    DynamicControlDebugMenuBoolean = 0x7b0693be,
    DynamicControlDebugMenuInt = 0xb3a8923,
    DynamicControlDebugMenuRealEvaluator = 0x5861dfec,
    DynamicControlDebugMenuRealMultiplier = 0xc918d2d1,
    DynamicControlMultiply = 0x6b1222cb,
    DynamicControlNegate = 0xb0ccf480,
    DynamicControlNegativeOne = 0xb112749b,
    DynamicControlNot = 0xb1d4f567,
    DynamicControlOne = 0x26e891cb,
    DynamicControlOr = 0x14b1379f,
    DynamicControlPhysical = 0x878ab37c,
    DynamicControlReference = 0x7d4c0d2a,
    DynamicControlScriptedReal = 0xce80aaf5,
    DynamicControlSpline = 0xbfe5b7e2,
    DynamicControlZero = 0xb54e6375,
    DirectionalForceField = 0xb04dbf28,
    ProgressiveForceField = 0x6ad00a54,
    RadialForceField = 0xd7951700,
    RadialProgressiveForceField = 0x69c85ede,
    DragCoefficientForceField = 0xbe2bde23,
    GameOptionBool = 0xeeba1127,
    GameOptionEnum = 0xfd6ab794,
    GameOptionInt = 0xae78ba5f,
    GameOptionReal = 0xf0d3112c,
    GameSurfaceCylinder = 0xa5b71844,
    GameSurfacePlane = 0x982529a5,
    GameSurfaceSphere = 0xb9a3ae53,
    GameSurfaceTrapezoid = 0x782d4103,
    GenericTriggerLogic = 0x14d66eed,
    LineOfSightTriggerLogic = 0xa9c784ae,
    LogicalANDTriggerLogic = 0xad4a1dea,
    LogicalORTriggerLogic = 0xbe2be2f1,
    MovementFacingTriggerLogic = 0xdaf279e5,
    PlayerProjectileTriggerLogic = 0xd6cd268f,
    TouchTagsTriggerLogic = 0x41a28134,
    HitReactionLogic_Creature = 0x4390e231,
    HitReactionLogic_FrontBack = 0xe1f817e6,
    HitReactionLogic_FrontSnapTopAllowed = 0x1bedd375,
    HitReactionLogic_NoSnap = 0x1e36cc71,
    HitReactionLogic_StdSnapAI = 0x8881b5ef,
    InventoryConsumable = 0xdff63833,
    InventoryPowerup = 0x71fb80c6,
    KnockbackReactionLogic_FrontBackSnap = 0xcbd5ee5f,
    KnockbackReactionLogic_FrontSnap = 0xd24f5784,
    CreatureActionSelectorFirstUsable = 0xfe6c35fc,
    CreatureActionSelectorLeastRecentlyUsed = 0xb4564bb,
    CreatureActionSelectorRandom = 0xc027df1e,
    CreatureActionExternalFSM = 0xa17046ef,
    CreatureActionLeap = 0x14127d89,
    CreatureActionAnimSequence = 0x81faf6e0,
    MotionSplineCollisionGeneration_Pipe = 0x93a6f298,
    MotionSplineCollisionGeneration_RectangularPipe = 0xc2e75fbb,
    MotionSplineCollisionGeneration_Rectilinear = 0x86bfc871,
    MotionSplineCollisionGeneration_Tubular = 0x88b2375b,
    FlatDirectionalObjectSelectionMethod = 0xb9846c71,
    PlayerControllerObjectSelectionMethod = 0xad276e7,
    SequenceObjectSelectionMethod = 0xc912948c,
    OrbitPositionControl_Joint = 0xab054402,
    AdoptSplineOrientationSplineControl = 0xffc90130,
    EulerOrientationSplineControl = 0x2c408d6b,
    TargetOrientationSplineControl = 0x39c3a3cb,
    WaypointOrientationSplineControl = 0x5402f59b,
    PhaseCombinatorOperationAverage = 0x6a93535d,
    PhaseCombinatorOperationWeightedAverage = 0x5befa257,
    PhysicsDriverMotionJump = 0x38f6cb24,
    ActorCollisionAABox = 0xc4b002dd,
    ActorCollisionCapsule = 0x6e42f199,
    ActorCollisionDCLN = 0xf81daa61,
    ActorCollisionRenderBounds = 0xe24e4b70,
    DynamicAABox = 0xc9991902,
    DynamicCapsule = 0x3e5d2d22,
    DynamicCylinderBox = 0xf125900a,
    DynamicSphere = 0x29be1666,
    PickupSelector_InventoryCheck = 0x27a1ca82,
    PendulumJumpMomentumOverride = 0x6f0ca389,
    SplineMotionTimeJumpMomentumOverride = 0x44d912a5,
    ConstantSpeedSplineControl = 0x1c8a8b6c,
    MotionSplineControl = 0x64e405e8,
    TargetPositionSplineControl = 0xdb08643d,
    XYZPositionSplineControl = 0x99350c3b,
    AABoxShapeData = 0x80da4f71,
    CappedCylinderShapeData = 0x6c7e0ae,
    CapsuleShapeData = 0x32fa4c2d,
    OBBoxShapeData = 0xcef3fcf1,
    PointShapeData = 0xf80976bd,
    SphereShapeData = 0x6ede23e6,
    ProjectileInaccuracyCone = 0x63250de0,
    ProjectileInaccuracyFlat = 0xd3099e89,
    ProjectileMotionPhysics = 0x20073581,
    ProjectileMotionPlayerRangedAttack = 0x5587fd5c,
    ProjectileMotionSequence = 0x7b6d66c4,
    ProjectileMotionSpline = 0x2dc4d32b,
    ProjectileMotionTargetedPhysics = 0x346ed8ef,
    ProjectileMotionTargetedSpline = 0x2bfbe0b6,
    ProjectileMotionTerrainMovement = 0x96a13b,
    ProjectileMotionWaypointFollower = 0xfd6de071,
    RenderAnimatedModel = 0xada6b7db,
    RenderCharacterModel = 0xf8771358,
    RenderMethodGameMode = 0x91f17031,
    RenderStaticModel = 0x13f5701a,
    RenderStaticModelArray = 0x26be03fa,
    RenderTexture = 0x6fc13b67,
    RenderVertexAnimatedModel = 0xb93bfeb8,
    ScaleSplineControl = 0x198cb0a2,
    UnifiedScaleSplineControl = 0xef25d664,
    ScriptConditionAND = 0xf8e103c7,
    ScriptConditionFacingOther = 0xba3dc77c,
    ScriptConditionHealth = 0x11f02c93,
    ScriptConditionLineOfSight = 0xe6c4cfea,
    ScriptConditionNOT = 0xd7f69652,
    ScriptConditionOR = 0xbda5ef31,
    ScriptConditionRelativeToOther = 0x920222e7,
    ScriptConditionWithinRange = 0x174fcbb6,
    ScriptedMotionControl_RotateViaSpline = 0x91adf011,
    ScriptedMotionControl_ScaleViaSpline = 0x87a0ea3c,
    ScriptedMotionControl_TranslateViaSpline = 0x7f4f6e1e,
    ScriptedMotionTimestamp_Absolute = 0x53ca6e7d,
    ScriptedMotionTimestamp_Normalized = 0x3cca7bb9,
    ScriptedMotionTimestamp_PickRandomPercent = 0xc826eb23,
    Circle = 0xfd5cf041,
    Rectangle = 0x1d8fdce1,
    PositionAtTimeSplineControl = 0xba393d16,
    WaypointSpeedControl = 0xd09b6eae,
    TagConditionAlwaysTrue = 0xaf5b8805,
    TagConditionAnd = 0xcd12cb97,
    TagConditionHasAll = 0xf6924a2b,
    TagConditionHasAny = 0xd7a450aa,
    TagConditionOr = 0x5c9ebd48,
    TargetPriorityCriteraIsTargetingMe = 0x2207ae6b,
    TargetPriorityCriteriaEvadeCreature = 0x2b3c7c2a,
    TargetPriorityCriterionArmor = 0xb3020202,
    TargetPriorityCriterionCurrentTarget = 0x2961d003,
    TargetPriorityCriterionDamagedBy = 0x87260a27,
    TargetPriorityCriterionDistance = 0x2fb4702c,
    TargetPriorityCriterionFacingTarget = 0x9f5a596f,
    TargetPriorityCriterionFaction = 0xebfd6f2,
    TargetPriorityCriterionFactionResponse = 0x7cc0eaa2,
    TargetPriorityCriterionPlayerTarget = 0xdbb3181f,
    TargetPriorityCriterionScriptedOverride = 0x9d60e6c6,
    TargetPriorityCriterionTeamSize = 0xeb76a74e,
    TargetPriorityCriterionUnobstructedPath = 0x8751ff7c,
    TargetSelectorActionListener = 0xe11ea3ca,
    TargetSelectorContacted = 0x67539b1c,
    TargetSelectorInRange = 0x2d8474c,
    TargetSelectorLineOfSight = 0xb9b839e4,
    TargetSelectorLinked = 0x755342c6,
    TargetSelectorOnScreen = 0xf492d01b,
    TargetSelectorOverCollision = 0x31da90fd,
    TargetSelectorPosture = 0xdb7c5a5e,
    TargetSelectorRaycast = 0x30c95bcf,
    TargetSelectorStateFlags = 0xd5337c6,
    TargetSelector_AND = 0x44ab82ac,
    TargetSelector_NOT = 0x40392935,
    TargetSelector_OR = 0xcfdb713d,
    PhysicsTriggerDetectionStates = 0xdcdce2d1,
    VolumetricFogInterpolation_Time = 0xecfbbf1f,
    DataEnumValueLinkData = 0x413f7859,
    IntLinkData = 0xfcd32049,
    PhaseDriverLinkData = 0x13e7569a,
    RoomResourceLinkData = 0x61eb4904,

    // MP1 complex
    MP1CameraBehaviorData = 0xcceddd28,
    PlayerVisorMP1EventCriteria = 0x8de0e586,
    ScanStateEventMP1EventCriteria = 0x9705f9da,
    DeltaTimeModifier_MP1PlayerSpeed = 0xcf24ede1,
    MetroidPrimeVulnerabilityCriteria = 0xc015dbdd,
    PlayerMorphBallStateMP1EventCriteria = 0xfd382654,
    PerformanceTestPathCameraMP1BehaviorData = 0x8ebcff02,
}
