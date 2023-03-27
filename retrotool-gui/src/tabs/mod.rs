pub mod lightprobe;
pub mod modcon;
pub mod model;
pub mod project;
pub mod room;
pub mod splash;
pub mod templates;
pub mod texture;

use bevy::{ecs::system::*, prelude::*, render::camera::*};
use bevy_egui::EguiContext;
use egui::Widget;
use egui_dock::{NodeIndex, Style, TabIndex};

use crate::{icon, AssetRef};

pub enum TabType {
    Project(Box<project::ProjectTab>),
    Texture(Box<texture::TextureTab>),
    Model(Box<model::ModelTab>),
    ModCon(Box<modcon::ModConTab>),
    LightProbe(Box<lightprobe::LightProbeTab>),
    Room(Box<room::RoomTab>),
    Templates(Box<templates::TemplatesTab>),
    Splash(Box<splash::SplashTab>),
}

pub struct OpenTab {
    pub tab: TabType,
    pub node: Option<NodeIndex>,
}

pub struct TabState {
    pub open_assets: Vec<AssetRef>,
    pub open_tab: Option<OpenTab>,
    pub viewport: Viewport,
    pub render_layer: u8,
    pub close_all: Option<NodeIndex>,
    pub close_others: Option<(NodeIndex, TabIndex)>,
}

impl TabState {
    fn open_tab(&mut self, tab: TabType) { self.open_tab = Some(OpenTab { tab, node: None }); }
}

pub trait SystemTab {
    type LoadParam: SystemParam;
    type UiParam: SystemParam;

    fn load(&mut self, _ctx: &mut EguiContext, _query: SystemParamItem<'_, '_, Self::LoadParam>) {}

    fn close(&mut self, _query: SystemParamItem<'_, '_, Self::LoadParam>) {} // , _ctx: &mut EguiContext

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        state: &mut TabState,
    );

    fn title(&mut self) -> egui::WidgetText;

    fn id(&self) -> String;
}

pub fn load_tab<T: SystemTab + 'static>(world: &mut World, ctx: &mut EguiContext, tab: &mut T) {
    let mut state: SystemState<T::LoadParam> = SystemState::new(world);
    tab.load(ctx, state.get_mut(world));
    state.apply(world);
}

fn render_tab<T: SystemTab + 'static>(
    world: &mut World,
    ui: &mut egui::Ui,
    tab: &mut T,
    tab_state: &mut TabState,
) {
    let mut state: SystemState<T::UiParam> = SystemState::new(world);
    ui.push_id(tab.id(), |ui| {
        tab.ui(ui, state.get_mut(world), tab_state);
    });
    state.apply(world);
}

fn close_tab<T: SystemTab + 'static>(world: &mut World, tab: &mut T) {
    let mut state: SystemState<T::LoadParam> = SystemState::new(world);
    tab.close(state.get_mut(world));
    state.apply(world);
}

pub struct TabViewer<'a> {
    pub world: &'a mut World,
    pub state: TabState,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = TabType;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            TabType::Project(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::Texture(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::Model(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::ModCon(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::LightProbe(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::Room(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::Templates(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
            TabType::Splash(tab) => render_tab(self.world, ui, tab.as_mut(), &mut self.state),
        }
    }

    fn context_menu(
        &mut self,
        ui: &mut egui::Ui,
        _tab: &mut Self::Tab,
        node: NodeIndex,
        tab_index: TabIndex,
    ) {
        if ui.button("Close others in group").clicked() {
            self.state.close_others = Some((node, tab_index));
            ui.close_menu();
        };
        if ui.button("Close all in group").clicked() {
            self.state.close_all = Some(node);
            ui.close_menu();
        };
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            TabType::Project(tab) => tab.title(),
            TabType::Texture(tab) => tab.title(),
            TabType::Model(tab) => tab.title(),
            TabType::ModCon(tab) => tab.title(),
            TabType::LightProbe(tab) => tab.title(),
            TabType::Room(tab) => tab.title(),
            TabType::Templates(tab) => tab.title(),
            TabType::Splash(tab) => tab.title(),
        }
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> bool {
        match tab {
            TabType::Project(_) => true,
            TabType::Texture(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
            TabType::Model(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
            TabType::ModCon(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
            TabType::LightProbe(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
            TabType::Room(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
            TabType::Templates(_) => true,
            TabType::Splash(tab) => {
                close_tab(self.world, tab.as_mut());
                true
            }
        }
    }

    fn add_popup(&mut self, ui: &mut egui::Ui, node: NodeIndex) {
        ui.set_min_width(100.0);
        ui.style_mut().visuals.button_frame = false;

        if ui.button(format!("{} Browser", icon::FILEBROWSER)).clicked() {
            self.state.open_tab =
                Some(OpenTab { tab: TabType::Project(Box::default()), node: Some(node) });
        }
        if ui.button(format!("{} Templates", icon::EDITMODE_HLT)).clicked() {
            self.state.open_tab = Some(OpenTab {
                tab: TabType::Templates(Box::new(templates::TemplatesTab::new())),
                node: Some(node),
            });
        }
    }

    fn inner_margin_override(&self, tab: &Self::Tab, style: &Style) -> egui::Margin {
        if self.clear_background(tab) {
            style.default_inner_margin
        } else {
            egui::Margin::same(0.0)
        }
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        !matches!(tab, TabType::Model(_) | TabType::ModCon(_) | TabType::Room(_))
    }
}

pub fn property_with_value(ui: &mut egui::Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", name));
        if egui::Label::new(&value)
            .sense(egui::Sense::click())
            .ui(ui)
            .on_hover_text_at_pointer("Click to copy")
            .clicked()
        {
            ui.output_mut(|out| out.copied_text = value);
        }
    });
}
