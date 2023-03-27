use std::str::FromStr;

use bevy::{ecs::system::SystemParamItem, prelude::*};
use egui::Widget;
use retrolib::util::templates::{
    load_type_template, FromRepr, HexU32, IntoRepr, PropertyTemplate, PropertyTemplateType,
    PropertyTemplateTypeDiscriminants, TypeTemplate, TypeTemplateType,
    TypeTemplateTypeDiscriminants,
};
use serde::{
    de::{value::StringDeserializer, IntoDeserializer},
    Deserialize,
};
use strum::{EnumMessage, IntoEnumIterator};

use crate::{
    icon,
    tabs::{SystemTab, TabState},
};

pub struct TemplatesTab {
    pub current: TypeTemplate,
    pub editing_key: Option<(HexU32, String)>,
}

impl TemplatesTab {
    pub fn new() -> Self {
        Self {
            current: load_type_template(include_str!(
                "../../../lib/templates/mp1r/objects/Render.json"
            ))
            .unwrap(),
            editing_key: None,
        }
    }
}

impl SystemTab for TemplatesTab {
    type LoadParam = ();
    type UiParam = ();

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _query: SystemParamItem<'_, '_, Self::UiParam>,
        _state: &mut TabState,
    ) {
        egui::TextEdit::singleline(&mut self.current.name).hint_text("Name").ui(ui);
        optional_text_edit(ui, &mut self.current.description, "Description");
        enum_ui::<_, TypeTemplateTypeDiscriminants>(
            ui,
            &mut self.current.template,
            "Template type",
        );
        match &mut self.current.template {
            TypeTemplateType::PropertyList(template) => {
                ui.group(|ui| {
                    ui.heading("Properties");
                    let mut prop_remove: Option<HexU32> = None;
                    let mut prop_replace: Option<(HexU32, HexU32)> = None;
                    for (i, (&key, value)) in template.properties.iter_mut().enumerate() {
                        ui.separator();
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .button(format!("{}", icon::REMOVE))
                                    .on_hover_text("Remove property")
                                    .clicked()
                                {
                                    prop_remove = Some(key);
                                };
                                ui.label(format!("{}:", i));
                                ui.separator();

                                let mut key_string =
                                    if let Some((editing_key, buffer)) = &self.editing_key {
                                        if *editing_key == key {
                                            buffer.clone()
                                        } else {
                                            key.to_string()
                                        }
                                    } else {
                                        key.to_string()
                                    };
                                let response = egui::TextEdit::singleline(&mut key_string)
                                    .hint_text("Key")
                                    .ui(ui);
                                if response.lost_focus() {
                                    if let Ok(new_key) = HexU32::deserialize::<
                                        StringDeserializer<serde::de::value::Error>,
                                    >(
                                        key_string.clone().into_deserializer()
                                    ) {
                                        self.editing_key = None;
                                        prop_replace = Some((key, new_key));
                                    } else {
                                        self.editing_key = Some((key, key_string));
                                    }
                                } else if response.changed() {
                                    self.editing_key = Some((key, key_string));
                                }
                            });
                            optional_text_edit(ui, &mut value.name, "Name");
                            optional_text_edit(ui, &mut value.description, "Description");
                            enum_ui::<_, PropertyTemplateTypeDiscriminants>(
                                ui,
                                &mut value.template,
                                "Property type",
                            );
                            property_template_type_ui(ui, &mut value.template);
                        });
                    }
                    if let Some(key) = prop_remove {
                        template.properties.remove(&key);
                    }
                    if let Some((old_key, new_key)) = prop_replace {
                        if let Some(index) = template.properties.get_index_of(&old_key) {
                            let (_, value) = template.properties.shift_remove_index(index).unwrap();
                            let new_index = match template.properties.entry(new_key) {
                                indexmap::map::Entry::Occupied(_) => unreachable!(),
                                indexmap::map::Entry::Vacant(e) => {
                                    let index = e.index();
                                    e.insert(value);
                                    index
                                }
                            };
                            template.properties.move_index(new_index, index);
                        }
                    }
                    if ui.button(format!("{}", icon::ADD)).on_hover_text("Add property").clicked() {
                        let mut key = HexU32(0);
                        loop {
                            match template.properties.entry(key) {
                                indexmap::map::Entry::Occupied(_) => {
                                    key.0 += 1;
                                    continue;
                                }
                                indexmap::map::Entry::Vacant(e) => {
                                    e.insert(PropertyTemplate::default());
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            TypeTemplateType::Struct(template) => {
                ui.group(|ui| {
                    ui.heading("Elements");
                    for (i, value) in template.elements.iter_mut().enumerate() {
                        ui.separator();
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:", i));
                                ui.separator();
                            });
                            optional_text_edit(ui, &mut value.name, "Name");
                            optional_text_edit(ui, &mut value.description, "Description");
                            enum_ui::<_, PropertyTemplateTypeDiscriminants>(
                                ui,
                                &mut value.template,
                                "Property type",
                            );
                            property_template_type_ui(ui, &mut value.template);
                        });
                    }
                    if ui.button(format!("{}", icon::ADD)).clicked() {
                        template.elements.push(PropertyTemplate::default());
                    }
                });
            }
            TypeTemplateType::Enum(_template) => {}
        }
    }

    fn title(&mut self) -> egui::WidgetText { format!("{} Templates", icon::EDITMODE_HLT).into() }

    fn id(&self) -> String { "Templates".into() }
}

fn enum_ui<Type, Discriminants>(
    ui: &mut egui::Ui,
    value: &mut Type,
    label: impl Into<egui::WidgetText>,
) where
    Type: FromRepr,
    Discriminants:
        for<'a> From<&'a Type> + IntoEnumIterator + EnumMessage + PartialEq + IntoRepr + Copy,
{
    let mut current = Discriminants::from(value);
    let response = egui::ComboBox::from_label(label)
        .selected_text(current.get_message().unwrap_or("[unknown]").to_string())
        .show_ui(ui, |ui| {
            let mut result = false;
            for v in Discriminants::iter() {
                result |= ui
                    .selectable_value(&mut current, v, v.get_message().unwrap_or("[unknown]"))
                    .changed();
            }
            result
        });
    if response.inner.unwrap_or_default() {
        *value = Type::from_repr(current.into_repr()).unwrap();
    };
}

fn optional_text_edit<T: ToString + FromStr>(
    ui: &mut egui::Ui,
    value: &mut Option<T>,
    hint_text: impl Into<egui::WidgetText>,
) {
    let mut text = value.as_ref().map(T::to_string).unwrap_or_default();
    if egui::TextEdit::singleline(&mut text).hint_text(hint_text).ui(ui).changed() {
        *value = if text.is_empty() { None } else { T::from_str(&text).ok() };
    }
}

fn property_template_type_ui(ui: &mut egui::Ui, value: &mut PropertyTemplateType) {
    match value {
        PropertyTemplateType::Unknown => {}
        PropertyTemplateType::Enum(property) => {
            egui::TextEdit::singleline(&mut property.enum_name).hint_text("Enum").ui(ui);
        }
        PropertyTemplateType::Struct(property) => {
            egui::TextEdit::singleline(&mut property.struct_name).hint_text("Struct").ui(ui);
        }
        PropertyTemplateType::Typedef(_property) => {
            // TODO
        }
        PropertyTemplateType::List(_property) => {
            // TODO
        }
        PropertyTemplateType::Id => {}
        PropertyTemplateType::Color => {}
        PropertyTemplateType::Vector => {}
        PropertyTemplateType::Bool => {}
        PropertyTemplateType::I8 => {}
        PropertyTemplateType::I16 => {}
        PropertyTemplateType::I32 => {}
        PropertyTemplateType::I64 => {}
        PropertyTemplateType::U8 => {}
        PropertyTemplateType::U16 => {}
        PropertyTemplateType::U32 => {}
        PropertyTemplateType::U64 => {}
        PropertyTemplateType::F32 => {}
        PropertyTemplateType::F64 => {}
    }
}
