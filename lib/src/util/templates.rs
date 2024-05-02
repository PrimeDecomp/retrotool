use std::{fmt, io::BufReader, path::Path};

use anyhow::Context;
use indexmap::IndexMap;
use serde::{de, ser};
use serde_derive::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::{EnumDiscriminants, EnumIter, EnumMessage, FromRepr};

#[derive(Clone, Debug)]
pub struct TemplateDatabase {
    pub root: TemplateRoot,
    pub objects: IndexMap<HexU32, TypeTemplate>,
    pub typedefs: IndexMap<HexU32, TypeTemplate>,
    pub structs: IndexMap<String, TypeTemplate>,
    pub enums: IndexMap<String, TypeTemplate>,
}

impl TemplateDatabase {
    pub fn find_object(&self, id: u32) -> (Option<&String>, Option<&TypeTemplate>) {
        (self.root.objects.get(&HexU32(id)), self.objects.get(&HexU32(id)))
    }

    pub fn find_typedef(&self, id: u32) -> (Option<&String>, Option<&TypeTemplate>) {
        (self.root.typedefs.get(&HexU32(id)), self.typedefs.get(&HexU32(id)))
    }

    pub fn find_struct(&self, name: &str) -> Option<&TypeTemplate> { self.structs.get(name) }

    pub fn find_enum(&self, name: &str) -> Option<&TypeTemplate> { self.enums.get(name) }
}

pub fn load_templates(path: &Path) -> anyhow::Result<Box<TemplateDatabase>> {
    let root: TemplateRoot = {
        let root_path = path.join("root.json");
        let file = std::fs::File::open(root_path.as_path())
            .with_context(|| format!("Failed to open {}", root_path.display()))?;
        serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("While reading {}", root_path.display()))?
    };
    let mut database = Box::new(TemplateDatabase {
        root,
        objects: IndexMap::new(),
        typedefs: IndexMap::new(),
        structs: IndexMap::new(),
        enums: IndexMap::new(),
    });
    for (id, name) in &database.root.objects {
        let type_path = path.join("objects").join(format!("{}.json", name));
        if !type_path.exists() {
            continue;
        }
        let file = std::fs::File::open(type_path.as_path())
            .with_context(|| format!("Failed to read {}", type_path.display()))?;
        let template = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("While parsing {}", type_path.display()))?;
        database.objects.insert(*id, template);
        log::info!("Loaded object template {}", name)
    }
    for (id, name) in &database.root.typedefs {
        let type_path = path.join("typedefs").join(format!("{}.json", name));
        if !type_path.exists() {
            continue;
        }
        let file = std::fs::File::open(type_path.as_path())
            .with_context(|| format!("Failed to read {}", type_path.display()))?;
        let template = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("While parsing {}", type_path.display()))?;
        database.typedefs.insert(*id, template);
        log::info!("Loaded typedef template {}", name)
    }
    for name in &database.root.structs {
        let type_path = path.join("structs").join(format!("{}.json", name));
        if !type_path.exists() {
            log::warn!("Struct template {} not found", name);
            continue;
        }
        let file = std::fs::File::open(type_path.as_path())
            .with_context(|| format!("Failed to read {}", type_path.display()))?;
        let template = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("While parsing {}", type_path.display()))?;
        database.structs.insert(name.clone(), template);
        log::info!("Loaded struct template {}", name)
    }
    for name in &database.root.enums {
        let type_path = path.join("enums").join(format!("{}.json", name));
        if !type_path.exists() {
            log::warn!("Enum template {} not found", name);
            continue;
        }
        let file = std::fs::File::open(type_path.as_path())
            .with_context(|| format!("Failed to read {}", type_path.display()))?;
        let template = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("While parsing {}", type_path.display()))?;
        database.enums.insert(name.clone(), template);
        log::info!("Loaded enum template {}", name)
    }
    Ok(database)
}

pub fn load_type_template(contents: &str) -> anyhow::Result<TypeTemplate> {
    serde_json::from_str(contents).map_err(|e| e.into())
}

pub trait FromRepr {
    fn from_repr(repr: usize) -> Option<Self>
    where Self: Sized;
}

pub trait IntoRepr {
    fn into_repr(self) -> usize;
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TemplateRoot {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub objects: IndexMap<HexU32, String>,
    #[serde(default)]
    pub typedefs: IndexMap<HexU32, String>,
    #[serde(default)]
    pub structs: Vec<String>,
    #[serde(default)]
    pub enums: Vec<String>,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TypeTemplate {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(flatten)]
    pub template: TypeTemplateType,
}

#[derive(Serialize, Deserialize, Clone, Debug, EnumDiscriminants, FromRepr)]
#[serde(tag = "type")]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
pub enum TypeTemplateType {
    #[serde(rename = "property_list")]
    #[strum_discriminants(strum(message = "Property List"))]
    PropertyList(PropertyListTemplate),
    #[serde(rename = "struct")]
    #[strum_discriminants(strum(message = "Struct"))]
    Struct(StructTemplate),
    #[serde(rename = "enum")]
    #[strum_discriminants(strum(message = "Enum"))]
    Enum(EnumTemplate),
}

impl FromRepr for TypeTemplateType {
    fn from_repr(repr: usize) -> Option<Self> { Self::from_repr(repr) }
}

impl IntoRepr for TypeTemplateTypeDiscriminants {
    fn into_repr(self) -> usize { self as usize }
}

#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct HexU32(pub u32);

impl fmt::Display for HexU32 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{:#010X}", self.0) }
}

impl fmt::Debug for HexU32 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{:#010X}", self.0) }
}

impl ser::Serialize for HexU32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: ser::Serializer {
        if serializer.is_human_readable() {
            serializer.serialize_str(&format!("{:#010X}", self.0))
        } else {
            serializer.serialize_u32(self.0)
        }
    }
}

struct HexU32Visitor;

impl<'de> de::Visitor<'de> for HexU32Visitor {
    type Value = u32;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a u32 or a hex string")
    }

    fn visit_u8<E>(self, value: u8) -> Result<Self::Value, E>
    where E: de::Error {
        Ok(value as u32)
    }

    fn visit_u16<E>(self, value: u16) -> Result<Self::Value, E>
    where E: de::Error {
        Ok(value as u32)
    }

    fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E>
    where E: de::Error {
        Ok(value)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where E: de::Error {
        u32::try_from(value).map_err(de::Error::custom)
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where E: de::Error {
        u32::try_from(value).map_err(de::Error::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where E: de::Error {
        let value = value
            .strip_prefix("0x")
            .ok_or_else(|| de::Error::custom("expected a hex string starting with \"0x\""))?;
        u32::from_str_radix(value, 16).map_err(de::Error::custom)
    }
}

impl<'de> de::Deserialize<'de> for HexU32 {
    fn deserialize<D>(deserializer: D) -> Result<HexU32, D::Error>
    where D: de::Deserializer<'de> {
        deserializer.deserialize_any(HexU32Visitor).map(Self)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct PropertyListTemplate {
    pub properties: IndexMap<HexU32, PropertyTemplate>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct StructTemplate {
    pub elements: Vec<PropertyTemplate>,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PropertyTemplate {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(flatten)]
    pub template: PropertyTemplateType,
}

#[derive(Serialize, Deserialize, Clone, Debug, EnumDiscriminants, FromRepr, Default)]
#[serde(tag = "type")]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
pub enum PropertyTemplateType {
    #[default]
    #[serde(rename = "unknown")]
    #[strum_discriminants(strum(message = "Unknown"))]
    Unknown,
    #[serde(rename = "enum")]
    #[strum_discriminants(strum(message = "Enum"))]
    Enum(EnumProperty),
    #[serde(rename = "struct")]
    #[strum_discriminants(strum(message = "Struct"))]
    Struct(StructProperty),
    #[serde(rename = "typedef")]
    #[strum_discriminants(strum(message = "Typedef"))]
    Typedef(TypedefProperty),
    #[serde(rename = "list")]
    #[strum_discriminants(strum(message = "List"))]
    List(ListProperty),
    #[serde(rename = "id")]
    #[strum_discriminants(strum(message = "ID"))]
    Id,
    #[serde(rename = "color")]
    #[strum_discriminants(strum(message = "Color"))]
    Color,
    #[serde(rename = "vector")]
    #[strum_discriminants(strum(message = "Vector"))]
    Vector,
    #[serde(rename = "bool")]
    #[strum_discriminants(strum(message = "Bool"))]
    Bool,
    #[serde(rename = "i8")]
    #[strum_discriminants(strum(message = "I8"))]
    I8,
    #[serde(rename = "i16")]
    #[strum_discriminants(strum(message = "I16"))]
    I16,
    #[serde(rename = "i32")]
    #[strum_discriminants(strum(message = "I32"))]
    I32,
    #[serde(rename = "i64")]
    #[strum_discriminants(strum(message = "I64"))]
    I64,
    #[serde(rename = "u8")]
    #[strum_discriminants(strum(message = "U8"))]
    U8,
    #[serde(rename = "u16")]
    #[strum_discriminants(strum(message = "U16"))]
    U16,
    #[serde(rename = "u32")]
    #[strum_discriminants(strum(message = "U32"))]
    U32,
    #[serde(rename = "u64")]
    #[strum_discriminants(strum(message = "U64"))]
    U64,
    #[serde(rename = "f32")]
    #[strum_discriminants(strum(message = "F32"))]
    F32,
    #[serde(rename = "f64")]
    #[strum_discriminants(strum(message = "F64"))]
    F64,
    #[serde(rename = "pooled_string")]
    #[strum_discriminants(strum(message = "Pooled String"))]
    PooledString,
}

impl FromRepr for PropertyTemplateType {
    fn from_repr(repr: usize) -> Option<Self> { Self::from_repr(repr) }
}

impl IntoRepr for PropertyTemplateTypeDiscriminants {
    fn into_repr(self) -> usize { self as usize }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct EnumProperty {
    #[serde(rename = "enum")]
    pub enum_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct StructProperty {
    #[serde(rename = "struct")]
    pub struct_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct TypedefProperty {
    #[serde(default)]
    pub supported_types: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ListProperty {
    pub element: Box<PropertyTemplateType>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct EnumTemplate {
    pub values: Vec<EnumElement>,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct EnumElement {
    pub name: Option<String>,
    pub description: Option<String>,
    pub value: HexU32,
}
