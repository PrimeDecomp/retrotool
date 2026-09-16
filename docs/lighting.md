# Lighting and scene inputs

## Authored lights and runtime lights

ROOM `LightStatic` definitions include baking inputs. The native builder creates
a runtime node only when runtime creation is enabled and the light type is
eligible; entity and layer activation impose further gates. Only two of the
5,343 examined definitions requested runtime nodes. Instantiating every stored
light would add illumination that is already represented by baked data.

Intensity scales RGB directly and leaves alpha unchanged. Cached local lights
also contain a roughness adjustment, distance-normalization parameters, cubic
attenuation coefficients and cone scale/bias. The examined shader loops use
the cubic distance curve and clamped linear cone factor without an additional
inverse-square term. A host engine's default light model is not equivalent.

`LightStatic` building (`0x710078dc74`) and cached-light packing
(`0x710016b5c8`) connect [ROOM light properties](../lib/src/format/room_lighting.rs)
to [shader light data](../lib/src/format/material_lighting.rs).

Ambient, directional and clustered lights enter through separate providers.
The clustered mask is an integer texture indexing light records; it is part of
the scene's culling result. An empty light array writes a one-byte sentinel,
whereas absent clustered data binds explicit defaults. Those states are not
interchangeable. Likewise, native full-bright ambient uses RGB `pi`, matching
the shader's diffuse normalization. See
[scene bindings](../lib/src/format/material_scene_bindings.rs) and
[ambient assembly](../lib/src/format/material_ambient.rs).

## Baked atlases and probe volumes

ROOM baked-lighting data associates object IDs with atlas lookups and references
LTPB probe resources. An atlas lookup contains two offsets and **one shared UV
scale**; its fourth word is unused by the recovered transform. Reconstructing
the object association still requires the correct scene component identity.

LTPB stores a signed, inclusive probe grid plus physical texture tiles. Grid
indices multiply by spacing to locate probe centers; bounds extend half a
spacing beyond the endpoint centers. Tile positions are signed too. Resource
META offsets locate embedded texture metadata and TXTR records relative to the
LTPB start.

The six runtime volumes encode midpoint, range, exponent and three gradient
directions. Their physical tiles have different texel extents: midpoint/range
use unsigned BC6H at `64×64×16`; the other four use BC1 at `128×64×16`. Each
occupies 64 KiB. Sparse byte offsets are allocation addresses, not offsets into
a linear image; assembly must place deswizzled block rows at their XYZ positions.

Volume allocation rounds bounds to a common grid using floor and a half-open
upper endpoint. Truncation breaks negative coordinates. World-to-texture adds
a half-texel center offset and preserves the storage transform. The second
BPTX matrix is the inverse transpose of **view-to-world alone**, despite its
semantic name suggesting the volume transform belongs there too.

The BPTX feature's full option binds all six volumes; an ambient-only option
uses the first slot differently. In the examined cooked graphs, supplying BLPT
textures without the **BLCM modulation color** still selects the disabled baked
variant. Feature selection must receive the complete provider set.

Reference paths: probe loading `0x710021c210`, volume initialization
`0x71001c8cbc`, and BPTX matrix construction `0x71001c9f50`.
[LTPB](../lib/src/format/ltpb.rs) and
[baked-lighting helpers](../lib/src/format/baked_lighting.rs) contain the layouts
and arithmetic. CPU/GPU BC1 decoding has measured precision differences;
decompression should not be assumed bit-identical across hosts.

## Choosing scene bounds and color

Area lighting bounds come from the ROOM resource's **PAK metadata**; runtime
UINF records receive copies. A zone unions its resident RoomControllers' bounds.
Ordinary areas climb *loaded* parents to a zone, falling back to their own
metadata when that chain is absent. Definitions alone do not establish residency.

Probe-volume initialization prefers valid primary-context bounds, then falls
back to registered contexts in order. Color selection follows a different
precedence: developer override, manager override, first registered context in
the active group, primary context, then white. Bounds validity and priority do
not select the color. Context-list identifiers and scene-group IDs are separate
fields.

RoomSettings has no serialized baked-lighting color. Its runtime setter changes
the area's primary color, which multiplies a secondary effects color. Both
start white. ColorModifier mode 1 reaches that setter through script links;
startup state, playback and effects determine the result.

Sources: `GetZoneLightProbeBounds` (`0x71003f1248`),
`InitializeProbeTextureFromStoredNodes` (`0x71001c71b0`) and
`UpdateLightModulationColor` (`0x71001c6f4c`); see
[scene inputs](../lib/src/format/baked_lighting_scene.rs) and
[scene construction](scene.md). Overlapping storage ownership and complete live
registration remain unresolved.

## Reflection inputs

A REFL resource references a cubemap and intensity. The material additionally
needs runtime REFP parameters and the BRDF lookup texture. The native LUT is a
single-mip **16×8 RG8_UNORM** image embedded in the executable, with linear
filtering and edge clamping. It is not an ordinary PAK dependency.

The maximum reflection mip index is the **bound view's mip count minus one**.
The default zero makes rough surfaces sample the sharpest mip, which caused the
shiny ship hull. Intensity and range also come from the resolved probe.

`NReflectionProbeUtil::resolve` (`0x7100217170`) and graphics-cache construction
(`0x7100123c14`) establish these inputs. For the [referenced build](references.md),
the 256 LUT bytes begin at `0x7101d0dbfc` and hash to
`dce3de6e63da0a098b9523e1a4cdb8a5943261ba70de00341891c06fa22273de` (SHA-256).
See [reflection data](../lib/src/format/reflection_probe.rs). Correct inputs for
an isolated model still do not establish the game's spatial probe selection.
