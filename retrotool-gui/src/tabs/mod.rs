pub mod model;
pub mod project;
pub mod texture;

use bevy::{ecs::system::*, prelude::*, render::camera::*};
use bevy_egui::EguiContext;

use crate::AssetRef;

pub enum TabType {
    Project(project::ProjectTab),
    Texture(texture::TextureTab),
    Model(model::ModelTab),
    Empty,
}

pub struct TabState {
    pub open_assets: Vec<AssetRef>,
    pub open_tab: Option<TabType>,
    pub viewport: Viewport,
    pub render_layer: u8,
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
            TabType::Project(tab) => render_tab(self.world, ui, tab, &mut self.state),
            TabType::Texture(tab) => render_tab(self.world, ui, tab, &mut self.state),
            TabType::Model(tab) => render_tab(self.world, ui, tab, &mut self.state),
            TabType::Empty => {}
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            TabType::Project(tab) => tab.title(),
            TabType::Texture(tab) => tab.title(),
            TabType::Model(tab) => tab.title(),
            TabType::Empty => "".into(),
        }
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> bool {
        match tab {
            TabType::Project(_) => false,
            TabType::Texture(tab) => {
                close_tab(self.world, tab);
                true
            }
            TabType::Model(tab) => {
                close_tab(self.world, tab);
                true
            }
            TabType::Empty => false,
        }
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        !matches!(tab, TabType::Empty | TabType::Model(_))
    }
}
