pub mod lightprobe;
pub mod modcon;
pub mod model;
pub mod project;
pub mod room;
pub mod splash;
pub mod templates;
pub mod texture;

use bevy::{ecs::system::*, prelude::*};
use egui::Widget;
use egui_dock::{widgets::tab_viewer::OnCloseResponse, NodePath, TabStyle};
use uuid::Uuid;

use crate::{icon, AssetRef};

pub type TabType = Box<dyn EditorTab>;

pub struct OpenTab {
    pub tab: TabType,
    pub node: Option<NodePath>,
}

#[derive(Default)]
pub struct TabState {
    pub open_assets: Vec<AssetRef>,
    pub open_tab: Option<OpenTab>,
    pub render_layer: usize,
    pub close_all: Option<NodePath>,
    pub close_others: Option<(NodePath, String)>,
    pub package_session: u64,
    pub clear_packages: bool,
}

impl TabState {
    fn open_tab(&mut self, tab: TabType) { self.open_tab = Some(OpenTab { tab, node: None }); }

    pub fn asset_path(&self, asset: AssetRef) -> bevy::asset::AssetPath<'static> {
        crate::loaders::package::asset_path(self.package_session, asset)
    }
}

pub trait EditorTab: Send + Sync {
    fn new() -> Box<Self>
    where Self: Default {
        default()
    }

    fn load(&mut self, world: &mut World);

    fn ui(&mut self, world: &mut World, ui: &mut egui::Ui, tab_state: &mut TabState);

    fn close(&mut self, world: &mut World) -> bool;

    fn clear_assets(&mut self, world: &mut World);

    fn title(&self) -> egui::WidgetText;

    #[allow(unused)]
    fn id(&self) -> String;

    fn clear_background(&self) -> bool { true }

    fn asset(&self) -> Option<AssetRef> { None }
}

pub trait EditorTabSystem: Send + Sync {
    type LoadParam: SystemParam + 'static;
    type UiParam: SystemParam + 'static;

    fn load(&mut self, _query: SystemParamItem<Self::LoadParam>) {}

    fn close(&mut self, _query: SystemParamItem<Self::LoadParam>) -> bool { true }

    fn clear_assets(&mut self, _query: SystemParamItem<Self::LoadParam>) {}

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<Self::UiParam>,
        state: &mut TabState,
    );

    fn title(&self) -> egui::WidgetText;

    fn id(&self) -> String;

    fn clear_background(&self) -> bool { true }

    fn asset(&self) -> Option<AssetRef> { None }
}

impl<T: EditorTabSystem> EditorTab for T {
    fn load(&mut self, world: &mut World) {
        let mut state: SystemState<T::LoadParam> = SystemState::new(world);
        EditorTabSystem::load(self, state.get_mut(world).expect("editor tab system parameters"));
        state.apply(world);
    }

    fn ui(&mut self, world: &mut World, ui: &mut egui::Ui, tab_state: &mut TabState) {
        let mut state: SystemState<T::UiParam> = SystemState::new(world);
        ui.push_id(self.id(), |ui| {
            EditorTabSystem::ui(
                self,
                ui,
                state.get_mut(world).expect("editor tab system parameters"),
                tab_state,
            );
        });
        state.apply(world);
    }

    fn close(&mut self, world: &mut World) -> bool {
        let mut state: SystemState<T::LoadParam> = SystemState::new(world);
        let result = EditorTabSystem::close(
            self,
            state.get_mut(world).expect("editor tab system parameters"),
        );
        state.apply(world);
        result
    }

    fn clear_assets(&mut self, world: &mut World) {
        let mut state: SystemState<T::LoadParam> = SystemState::new(world);
        EditorTabSystem::clear_assets(
            self,
            state.get_mut(world).expect("editor tab system parameters"),
        );
        state.apply(world);
    }

    fn title(&self) -> egui::WidgetText { EditorTabSystem::title(self) }

    fn id(&self) -> String { EditorTabSystem::id(self) }

    fn clear_background(&self) -> bool { EditorTabSystem::clear_background(self) }

    fn asset(&self) -> Option<AssetRef> { EditorTabSystem::asset(self) }
}

pub struct TabViewer<'a> {
    pub world: &'a mut World,
    pub state: TabState,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = TabType;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        tab.ui(self.world, ui, &mut self.state);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab, node: NodePath) {
        if ui.button("Close others in group").clicked() {
            self.state.close_others = Some((node, tab.id()));
            ui.close();
        };
        if ui.button("Close all in group").clicked() {
            self.state.close_all = Some(node);
            ui.close();
        };
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText { tab.title() }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id { egui::Id::new(tab.id()) }

    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        if tab.close(self.world) {
            OnCloseResponse::Close
        } else {
            OnCloseResponse::Ignore
        }
    }

    fn add_popup(&mut self, ui: &mut egui::Ui, node: NodePath) {
        ui.set_min_width(100.0);
        ui.style_mut().visuals.button_frame = false;

        if ui.button(format!("{} Browser", icon::FILEBROWSER)).clicked() {
            self.state.open_tab =
                Some(OpenTab { tab: project::ProjectTab::new(), node: Some(node) });
        }
        if ui.button(format!("{} Templates", icon::EDITMODE_HLT)).clicked() {
            self.state.open_tab =
                Some(OpenTab { tab: templates::TemplatesTab::new(), node: Some(node) });
        }
    }

    fn tab_style_override(&self, tab: &Self::Tab, style: &TabStyle) -> Option<TabStyle> {
        if tab.clear_background() {
            None
        } else {
            let mut style = style.clone();
            style.tab_body.inner_margin = egui::Margin::ZERO;
            Some(style)
        }
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool { tab.clear_background() }
}

pub fn property_with_value(ui: &mut egui::Ui, name: &str, value: String) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", name));
        let response = egui::Label::new(&value)
            .sense(egui::Sense::click())
            .ui(ui)
            .on_hover_text_at_pointer("Click to copy");
        if response.clicked() {
            ui.ctx().copy_text(value);
        }
        response
    })
    .inner
}

pub fn property_with_id(ui: &mut egui::Ui, name: &str, id: Uuid) {
    property_with_value(ui, name, id.to_string()).context_menu(|ui| {
        if ui.button("Open").clicked() {
            ui.ctx().copy_text(id.to_string());
        }
    });
}
