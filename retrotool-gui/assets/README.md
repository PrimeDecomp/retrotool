# Viewer assets

The Papermill environment maps are the existing preview cubemaps. Their incomplete
KTX2 data format descriptors were repaired during the Bevy 0.19 upgrade; compressed
mip payloads are unchanged. The RGB9E5 descriptor comes from Bevy's
[Pisa map](https://github.com/bevyengine/bevy/blob/b56fc29d3016e641754765244b5ba3f9cc504671/assets/environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2),
which declares the same Vulkan format. No Pisa texels were copied.

Preview shading uses normalized ambient/environment intensities (0.05/1.0),
without Bevy's physical-camera exposure. Original shaders receive game-style
bindings separately. These cubemaps do not represent ROOM lighting.
