use anyhow::{ensure, Result};
use retrolib::format::baked_lighting::{probe::NonlinearProbe, volume::ProbeVolume};

use crate::loaders::lightprobe::LightProbeAsset;

pub(super) struct ProbeInspection {
    position: [f32; 3],
    normal: [f32; 3],
    result: Option<Result<NonlinearProbe, String>>,
}

impl ProbeInspection {
    pub(super) fn new(asset: &LightProbeAsset) -> Self {
        let min = asset.head.min_index.to_array();
        let max = asset.head.max_index.to_array();
        let spacing = asset.head.grid_spacing.to_array();
        Self {
            position: std::array::from_fn(|i| {
                (f32::from(min[i]) + f32::from(max[i])) * 0.5 * spacing[i]
            }),
            normal: [0., 0., 1.],
            result: None,
        }
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui, asset: &LightProbeAsset) {
        egui::CollapsingHeader::new("Probe lighting").default_open(true).show(ui, |ui| {
            ui.label("Isolated volume; position and normal use storage coordinates.");
            let mut sample_changed = self.result.is_none();
            ui.horizontal(|ui| {
                ui.label("Position");
                for value in &mut self.position {
                    sample_changed |= ui.add(egui::DragValue::new(value).speed(0.25)).changed();
                }
                sample_changed |= ui.button("Sample").clicked();
            });
            ui.horizontal(|ui| {
                ui.label("Normal");
                for value in &mut self.normal {
                    ui.add(egui::DragValue::new(value).speed(0.05));
                }
            });
            if sample_changed {
                self.result = Some(sample(asset, self.position).map_err(|e| format!("{e:#}")));
            }
            let probe = match self.result.as_ref() {
                Some(Ok(probe)) => probe,
                Some(Err(error)) => {
                    ui.colored_label(egui::Color32::LIGHT_YELLOW, error);
                    return;
                }
                None => return,
            };
            match probe.irradiance(self.normal) {
                Ok(irradiance) => {
                    let rgb = irradiance.map(|v| v * std::f32::consts::FRAC_1_PI);
                    let color = rgb.map(|v| v.clamp(0., 1.));
                    let response = ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(28., 28.), egui::Sense::hover());
                        let color: egui::Color32 =
                            egui::Rgba::from_rgb(color[0], color[1], color[2]).into();
                        ui.painter().rect_filled(rect, 3., color);
                        ui.label(format!(
                            "Unit-albedo diffuse RGB: {:.4}, {:.4}, {:.4}",
                            rgb[0], rgb[1], rgb[2],
                        ));
                    });
                    response.response.on_hover_text(concat!(
                        "White diffuse material, unit modulation. The swatch clips to display ",
                        "range; numeric values retain HDR and negative channels.",
                    ));
                }
                Err(error) => {
                    ui.colored_label(egui::Color32::LIGHT_YELLOW, error.to_string());
                }
            }
            ui.small(
                "CPU reference sampling; complete ROOM lighting also depends on scene inputs.",
            );
        });
    }
}

fn sample(asset: &LightProbeAsset, position: [f32; 3]) -> Result<NonlinearProbe> {
    ensure!(asset.textures.len() == asset.extra.len(), "Probe texture metadata count mismatch");
    let spacing = asset.head.grid_spacing.to_array();
    ensure!(spacing.iter().all(|v| v.is_finite() && *v > 0.), "Invalid probe grid spacing");
    let mut volume = ProbeVolume::enclosing_grid(
        asset.head.min_index.to_array().map(i32::from),
        asset.head.max_index.to_array().map(i32::from),
    )?;
    for (texture, extra) in asset.textures.iter().zip(&asset.extra) {
        volume.insert_tile(extra.layer, extra.tile_position.to_array(), &texture.inner)?;
    }
    let index = std::array::from_fn(|i| position[i] / spacing[i]);
    let samples = volume.sample(volume.grid_coordinate(index)?)?;
    NonlinearProbe::from_samples(&samples, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]])
}
