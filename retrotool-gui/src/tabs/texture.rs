use bevy::{
    asset::LoadState,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::render_resource::*,
};
use bevy_egui::EguiContext;
use retrolib::format::txtr::{ETextureFormat, ETextureType};

use crate::{loaders::TextureAsset, tabs::SystemTab, AssetRef, TabState, icon};

pub struct LoadedTexture {
    pub texture_ids: Vec<egui::TextureId>,
}

pub struct TextureTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<TextureAsset>,
    pub loaded_texture: Option<LoadedTexture>,
}

impl SystemTab for TextureTab {
    type LoadParam = (SRes<Assets<TextureAsset>>, SResMut<Assets<Image>>);
    type UiParam = (SRes<AssetServer>, SRes<Assets<TextureAsset>>);

    fn load(&mut self, ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        if self.loaded_texture.is_some() {
            return;
        }

        let (textures, mut images) = query;
        let Some(txtr) = textures.get(&self.handle) else { return; };
        let mut texture_ids = Vec::new();
        if let Some(rgba) = &txtr.rgba {
            let image_handle = images.add(Image {
                data: rgba.clone(),
                texture_descriptor: TextureDescriptor {
                    label: None,
                    size: Extent3d {
                        width: txtr.inner.head.width,
                        height: txtr.inner.head.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: if txtr.inner.head.format.is_srgb() {
                        TextureFormat::Rgba8UnormSrgb
                    } else {
                        TextureFormat::Rgba8Unorm
                    },
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                },
                sampler_descriptor: default(),
                texture_view_descriptor: None,
            });
            texture_ids.push(ctx.add_image(image_handle));
        } else {
            let array_stride: usize =
                (txtr.inner.head.mip_sizes.iter().sum::<u32>() / txtr.inner.head.layers) as usize;
            for layer in 0..txtr.inner.head.layers as usize {
                let image_handle = images.add(Image {
                    data: txtr.inner.data
                        [layer * array_stride..(layer * array_stride) + array_stride]
                        .to_vec(),
                    texture_descriptor: TextureDescriptor {
                        label: None,
                        size: Extent3d {
                            width: txtr.inner.head.width,
                            height: txtr.inner.head.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: txtr.inner.head.mip_sizes.len() as u32,
                        sample_count: 1,
                        dimension: match txtr.inner.head.kind {
                            ETextureType::_1D => TextureDimension::D1,
                            ETextureType::_2D => TextureDimension::D2,
                            ETextureType::_3D => TextureDimension::D3,
                            ETextureType::Cube => TextureDimension::D2,
                            ETextureType::_1DArray => TextureDimension::D1,
                            ETextureType::_2DArray => TextureDimension::D2,
                            ETextureType::_2DMultisample => TextureDimension::D2,
                            ETextureType::_2DMultisampleArray => TextureDimension::D2,
                            ETextureType::CubeArray => TextureDimension::D2,
                        },
                        format: match txtr.inner.head.format {
                            ETextureFormat::R8Unorm => TextureFormat::Rgba8Unorm,
                            ETextureFormat::R8Snorm => TextureFormat::R8Snorm,
                            ETextureFormat::R8Uint => TextureFormat::R8Uint,
                            ETextureFormat::R8Sint => TextureFormat::R8Sint,
                            ETextureFormat::R16Unorm => TextureFormat::R16Unorm,
                            ETextureFormat::R16Snorm => TextureFormat::R16Snorm,
                            ETextureFormat::R16Uint => TextureFormat::R16Uint,
                            ETextureFormat::R16Sint => TextureFormat::R16Sint,
                            ETextureFormat::R16Float => TextureFormat::R16Float,
                            ETextureFormat::R32Uint => TextureFormat::R32Uint,
                            ETextureFormat::R32Sint => TextureFormat::R32Sint,
                            ETextureFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
                            ETextureFormat::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
                            ETextureFormat::Rgba16Float => TextureFormat::Rgba16Float,
                            ETextureFormat::Rgba32Float => TextureFormat::Rgba32Float,
                            ETextureFormat::Depth16Unorm => TextureFormat::Depth16Unorm,
                            ETextureFormat::Depth16Unorm2 => TextureFormat::Depth16Unorm,
                            ETextureFormat::Depth24S8Unorm => TextureFormat::Depth24PlusStencil8,
                            ETextureFormat::Depth32Float => TextureFormat::Depth32Float,
                            ETextureFormat::RgbaBc1Unorm => TextureFormat::Bc1RgbaUnorm,
                            ETextureFormat::RgbaBc1Srgb => TextureFormat::Bc1RgbaUnormSrgb,
                            ETextureFormat::RgbaBc2Unorm => TextureFormat::Bc2RgbaUnorm,
                            ETextureFormat::RgbaBc2Srgb => TextureFormat::Bc2RgbaUnormSrgb,
                            ETextureFormat::RgbaBc3Unorm => TextureFormat::Bc3RgbaUnorm,
                            ETextureFormat::RgbaBc3Srgb => TextureFormat::Bc3RgbaUnormSrgb,
                            ETextureFormat::RgbaBc4Unorm => TextureFormat::Bc4RUnorm,
                            ETextureFormat::RgbaBc4Snorm => TextureFormat::Bc4RSnorm,
                            ETextureFormat::RgbaBc5Unorm => TextureFormat::Bc5RgUnorm,
                            ETextureFormat::RgbaBc5Snorm => TextureFormat::Bc5RgSnorm,
                            ETextureFormat::Rg11B10Float => TextureFormat::Rg11b10Float,
                            ETextureFormat::R32Float => TextureFormat::R32Float,
                            ETextureFormat::Rg8Unorm => TextureFormat::Rg8Unorm,
                            ETextureFormat::Rg8Snorm => TextureFormat::Rg8Snorm,
                            ETextureFormat::Rg8Uint => TextureFormat::Rg8Uint,
                            ETextureFormat::Rg8Sint => TextureFormat::Rg8Sint,
                            ETextureFormat::Rg16Float => TextureFormat::Rg16Float,
                            ETextureFormat::Rg16Unorm => TextureFormat::Rg16Unorm,
                            ETextureFormat::Rg16Snorm => TextureFormat::Rg16Snorm,
                            ETextureFormat::Rg16Uint => TextureFormat::Rg16Uint,
                            ETextureFormat::Rg16Sint => TextureFormat::Rg16Sint,
                            ETextureFormat::Rgb10A2Unorm => TextureFormat::Rgb10a2Unorm,
                            ETextureFormat::Rg32Uint => TextureFormat::Rg32Uint,
                            ETextureFormat::Rg32Sint => TextureFormat::Rg32Sint,
                            ETextureFormat::Rg32Float => TextureFormat::Rg32Float,
                            ETextureFormat::Rgba16Unorm => TextureFormat::Rgba16Unorm,
                            ETextureFormat::Rgba16Snorm => TextureFormat::Rgba16Snorm,
                            ETextureFormat::Rgba16Uint => TextureFormat::Rgba16Uint,
                            ETextureFormat::Rgba16Sint => TextureFormat::Rgba16Sint,
                            ETextureFormat::Rgba32Uint => TextureFormat::Rgba32Uint,
                            ETextureFormat::Rgba32Sint => TextureFormat::Rgba32Sint,
                            ETextureFormat::BptcUfloat => TextureFormat::Bc6hRgbUfloat,
                            ETextureFormat::BptcSfloat => TextureFormat::Bc6hRgbSfloat,
                            ETextureFormat::BptcUnorm => TextureFormat::Bc7RgbaUnorm,
                            ETextureFormat::BptcUnormSrgb => TextureFormat::Bc7RgbaUnormSrgb,
                            _ => todo!(),
                        },
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    },
                    sampler_descriptor: default(),
                    texture_view_descriptor: None,
                });
                texture_ids.push(ctx.add_image(image_handle));
            }
        };
        self.loaded_texture = Some(LoadedTexture { texture_ids });
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        _state: &mut TabState,
    ) {
        let (server, textures) = query;

        ui.label(format!("{} {}", self.asset_ref.kind, self.asset_ref.id));

        match server.get_load_state(&self.handle) {
            LoadState::NotLoaded => {
                return;
            }
            LoadState::Loading => {
                ui.spinner();
                return;
            }
            LoadState::Loaded => {}
            LoadState::Failed => {
                ui.colored_label(egui::Color32::RED, "Loading failed");
                return;
            }
            LoadState::Unloaded => {
                return;
            }
        };

        let loaded = self.loaded_texture.as_mut().unwrap();
        if let Some(txtr) = textures.get(&self.handle) {
            ui.label(format!("Type: {:?}", txtr.inner.head.kind));
            ui.label(format!("Format: {:?}", txtr.inner.head.format));
            ui.label(format!(
                "Dimensions: {}x{}x{} (mips: {})",
                txtr.inner.head.width,
                txtr.inner.head.height,
                txtr.inner.head.layers,
                txtr.inner.head.mip_sizes.len()
            ));
            if txtr.inner.head.kind == ETextureType::Cube && loaded.texture_ids.len() == 6 {
                let width = txtr.inner.head.width;
                let height = txtr.inner.head.height;
                let (_, rect) =
                    ui.allocate_space(egui::Vec2 { x: (width * 4) as f32, y: (height * 3) as f32 });
                let size = egui::Vec2 { x: width as f32, y: height as f32 };
                let mut draw_image = |i: usize, x: u32, y: u32| {
                    let min = egui::Vec2 { x: (width * x) as f32, y: (height * y) as f32 };
                    let max =
                        egui::Vec2 { x: (width * (x + 1)) as f32, y: (height * (y + 1)) as f32 };
                    egui::widgets::Image::new(loaded.texture_ids[i], size)
                        .paint_at(ui, egui::Rect { min: rect.min + min, max: rect.min + max });
                };
                draw_image(2, 1, 0);
                draw_image(1, 0, 1);
                draw_image(4, 1, 1);
                draw_image(0, 2, 1);
                draw_image(5, 3, 1);
                draw_image(3, 1, 2);
            } else {
                for image in &loaded.texture_ids {
                    ui.add(egui::widgets::Image::new(*image, egui::Vec2 {
                        x: txtr.inner.head.width as f32,
                        y: txtr.inner.head.height as f32,
                    }));
                }
            }
        }
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::TEXTURE, self.asset_ref.kind, self.asset_ref.id).into()
    }
}
