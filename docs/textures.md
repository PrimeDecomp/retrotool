# Textures and surface interpretation

## Storage, views and samplers

A TXTR describes storage. A shader uses a **view** of that storage and a sampler,
which may come from the texture defaults, material overrides or runtime scene
state. An instruction's dimensionality is insufficient to invent a compatible
view. In particular, layered-vine shaders have observed 2D-array accesses whose
material assets are ordinary one-layer 2D textures; the native adaptation remains
unresolved.

Material sampler overrides of `-1` inherit defaults. Runtime streaming can select
temporary minimum-LOD variants; ordinary, bindless and sample-shading paths do
not all choose the same sampler. Loading the complete texture does not reproduce
the streaming scheduler. View base mip/layer values accumulate relative to the
parent; the accompanying range values are **counts**, despite misleading `max`
names in some decompiled structures. `BuildTextureView` (`0x7100116088`) is the
relevant engine path.

For serialized texture tokens, nil handling depends on context: a top-level
material TXTR value still includes usage information, while a nil slot inside
layered texture data omits it. See [TXTR](../lib/src/format/txtr.rs),
[sampler inheritance](../lib/src/format/texture_sampler.rs), and
[NVN views](../lib/src/format/texture_nvn.rs).

## Layered values

The three slots in `CLayeredTextureData` mean **Texture0, Texture1, BlendTexture**.
They are not independent color, normal and metallic textures. Each material
channel can have its own layered value. Blend registers are coefficients, not
three tint colors; the control mode selects embedded, separate, external or
disabled blending. Tangent flags select one- or two-stream normal variants when
the feature's allowed options permit them.

Binding flags are separate from those serialized tangent flags. One misleading
name has confirmed inverted behavior: LAYR's `IgnoreBlendTexture` bit **enables**
the blend-texture binding when set. Original callbacks at `0x7100170464` /
`0x7100170480` establish this. See
[texture binders](../lib/src/format/material_texture_bindings.rs).

## Recovered surface conventions

The examined single-map and layered vine programs provide useful channel
interpretations, not a universal material specification:

- METL RGB supplies an occlusion/intensity factor, raw roughness and metallic
  weight, respectively. The metallic weight interpolates specular F0 between
  dielectric `0.04` and base color; diffuse tint is applied separately.
- Normal-map RG expands as `sample * (255/128) - 1`. Z is reconstructed from
  the remaining unit-sphere component. The single-map result is not normalized
  after applying the interpolated tangent frame; the layered result is
  normalized after blending its two mapped normals.
- Vertex RGB becomes `2 * pow(abs(rgb), 2.2)` **before interpolation**. Vertex
  alpha passes through, but its later use depends on the selected program.
- The single-map alpha path multiplies squared base alpha, diffuse alpha and
  vertex alpha. In the examined layered path, vertex alpha controls blending
  and is not multiplied into output alpha again.

These equations explain why a generic PBR preview can differ substantially even
with the correct textures. They also explain the apparent holes in corroded
model `8bd2594c-db35-40b8-8303-d825d1b2b68a`: its shader and blend state preserve
authored transparency. Its use as an overlay is an interpretation; the alpha
behavior itself was checked against the original program.

The shader-derived helpers in [mpr_layered.wgsl](../retrotool-gui/assets/mpr_layered.wgsl)
retain these earlier surface comparisons. Original-program rendering obtains
the equations from each selected shader instead of applying them to every model.
