pub mod lightprobe;
pub mod modcon;
pub mod model;
pub mod project;
pub mod room;
pub mod splash;
pub mod templates;
pub mod texture;

use bevy::{ecs::system::*, prelude::*, render::camera::*};
use egui::Widget;
use egui_dock::{NodeIndex, Style, TabIndex};

use crate::{icon, AssetRef};

pub type TabType = Box<dyn EditorTab>;

pub struct OpenTab {
    pub tab: TabType,
    pub node: Option<NodeIndex>,
}

#[derive(Default)]
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

pub trait EditorTab: Send + Sync {
    fn new() -> Box<Self>
    where Self: Default {
        default()
    }

    fn load(&mut self, world: &mut World);

    fn ui(&mut self, world: &mut World, ui: &mut egui::Ui, tab_state: &mut TabState);

    fn close(&mut self, world: &mut World) -> bool;

    fn title(&self) -> egui::WidgetText;

    fn id(&self) -> String;

    fn clear_background(&self) -> bool { true }

    fn asset(&self) -> Option<AssetRef> { None }
}

pub trait EditorTabSystem: Send + Sync {
    type LoadParam: SystemParam + 'static;
    type UiParam: SystemParam + 'static;

    fn load(&mut self, _query: SystemParamItem<Self::LoadParam>) {}

    fn close(&mut self, _query: SystemParamItem<Self::LoadParam>) -> bool { true }

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
        EditorTabSystem::load(self, state.get_mut(world));
        state.apply(world);
    }

    fn ui(&mut self, world: &mut World, ui: &mut egui::Ui, tab_state: &mut TabState) {
        let mut state: SystemState<T::UiParam> = SystemState::new(world);
        ui.push_id(self.id(), |ui| {
            EditorTabSystem::ui(self, ui, state.get_mut(world), tab_state);
        });
        state.apply(world);
    }

    fn close(&mut self, world: &mut World) -> bool {
        let mut state: SystemState<T::LoadParam> = SystemState::new(world);
        let result = EditorTabSystem::close(self, state.get_mut(world));
        state.apply(world);
        result
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

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText { tab.title() }

    fn on_close(&mut self, tab: &mut Self::Tab) -> bool { tab.close(self.world) }

    fn add_popup(&mut self, ui: &mut egui::Ui, node: NodeIndex) {
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

    fn inner_margin_override(&self, tab: &Self::Tab, style: &Style) -> egui::Margin {
        if self.clear_background(tab) {
            style.default_inner_margin
        } else {
            egui::Margin::same(0.0)
        }
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool { tab.clear_background() }
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
