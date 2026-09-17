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

Static MCON placements carry their own atlas lookups, separately for CMDL and
WMDL placements. The unsorted draw path supplies three providers together:
`BLAT` (the area's lightmap TXTR), `BLAL` (the placement lookup) and `BLCM`
(the manager's modulation color). The texture uses default usage: UV index zero
and inherited sampler settings. It can be an array; the examined Overworld hall
uses four signed BC6H layers. Treating it as an ordinary 2D color image loses
shader inputs.

The manager selects texture data from the first registered context matching the
draw's scene-group byte. It does not search later matching contexts when that
context lacks storage. This texture choice is separate from the modulation-color
precedence below. Relevant consumers are `CModConSceneNode::RenderUnsortedEntryConcurrent`
(`0x71001b5bcc`), `CWorldModelRenderable::RenderUnsorted` (`0x710034428c`),
`GetTextureData` (`0x71001c6b24`) and `BuildRenderThreadResources` (`0x71001cd088`).

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

All six volumes use linear filtering and an **opaque black** border. The builder
overrides the sampler's initial border at `0x71001c93fc`; the SDK default alone
does not establish the bound value. Exterior border samples are distinct from
missing interior sparse pages, whose contents remain unresolved. The
[CPU volume sampler](../lib/src/format/baked_lighting/volume.rs) reports missing
tiles and conflicting ownership instead of choosing their contents. The viewer's
LTPB tab uses it for isolated probe inspection in storage coordinates.

Volume allocation rounds bounds to a common grid using floor and a half-open
upper endpoint. Truncation breaks negative coordinates. World-to-texture adds
a half-texel center offset and preserves the storage transform. The second
BPTX matrix is the inverse transpose of **view-to-world alone**, despite its
semantic name suggesting the volume transform belongs there too.

The BPTX feature's full option binds all six volumes; an ambient-only option
uses the first slot differently. In the examined cooked graphs, supplying BLPT
textures without the **BLCM modulation color** still selects the disabled baked
variant. Feature selection must receive the complete provider set.

Probe evaluation also has a storage-buffer path. `EvaluateProbesStorageBuffer`
(`0x71001c6c78`) allocates 1,024 records of 80 bytes, evaluates registered probe
positions from the volume textures, and exposes the result as **BLPD**. Probe
handles receive record indices; entries beyond capacity use index zero. This
buffer is generated scene data, separate from BLPT's volume resources. The
record, named `SLightProbeNonlinear` in DX reflection, holds midpoint/range RGB
and three direction/exponent vectors, one per color channel.

The producer samples all six volumes at LOD zero. Midpoint/range pass through;
directions decode as `2*RGB - 1` and exponents as `2*RGB + 1`. Directions then
multiply by the linear part of global `uc_bakedToView`, **without normalization**.
This is distinct from BPTX's inverse-transpose matrix. The global builder
multiplies native world-to-view by the transform supplied to `BindWorldToBaked`;
the setter's caller-space convention remains unresolved.
The bound input starts as identity. Graphics-cache `Reset` clears the uploaded
matrices but preserves that input, so a reset does not establish a new binding.

`ProbeLighting`, technique `x`, permutation `0`, evaluates each channel using
the normalized view-space surface normal `n`, stored direction `d` and exponent
`e`:

```text
t = clamp(0.5 * dot(n, d) + 0.5, 0, 1)
weight = (e + 1) / 2 * t^e
irradiance = (midpoint - range) + 2 * range * weight
```

The shader computes the range term through the two endpoints; the helper
preserves that operation order. Gradient length affects the result, and there
is no final clamp. `midpoint + range` is not an upper bound when `e > 1`.
Permutation `1` uses midpoint RGB alone. Both multiply irradiance by diffuse
RGB, BLCM RGB and `1/pi`, with output alpha one.

These pairs share GUID/hash, technique and permutation across backends:
`BakedLighting_ProbeTexture` (`761388bc-974e-4b92-b00d-c312e89cd868`, `e3eb59fb`)
uses DX compute source 271 versus NVN vertex source 98 with rasterizer discard;
`ProbeLighting` (`33e72f05-2050-48f7-9df7-10ff8565c799`, `46f8fbaa`) uses
DX pixel sources 577/579 versus NVN 328/329. The
[probe record and evaluator](../lib/src/format/baked_lighting/probe.rs) reproduce
the finite equations; host special functions do not establish SM53 precision.
The nonlinear pair's vertex stage derives the probe sphere's local normal from
its position, applies the instance matrix's inverse transpose, then the global
normal matrix. It passes the probe-record index as a flat integer. This is the
debug sphere path; ordinary materials obtain normals from their own vertex and
texture inputs.

The complete NVN pair has interpreter and rasterized WGSL comparison coverage
with explicit transforms, sample offsets and global-memory snapshots. Constructing
those inputs from live scene state remains separate.

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
[scene construction](scene.md). Complete live registration remains unresolved.

## Overlapping probe tiles

Lighting contexts belong to loaded areas. After asynchronous area loading
finishes, the area's baked-lighting manager creates a scene-node proxy if it has
atlas or probe storage. Zone contexts carry the primary flag; priority starts
at zero. The render node registers when active and unregisters on deactivation
or removal. Reactivation copies the current color and priority before registering
again. Updating an already registered node does not re-register its tiles.

Area unloading retires this proxy before unloading script layers. Consequently,
definition order or a set of loaded LTPB files is insufficient to recover tile
ownership: scene activation and transfer timing are part of the input. Sources:
`LoadAreaAsync` (`0x71003ebca0`), `InitializeRenderNodes` (`0x71003fc2f4`),
`PullTransferData` (`0x71001cd56c`) and `ContinueUnload` (`0x71003fcc94`).

Different probe grids can occupy the same physical tile coordinates. For example,
`00F_intro_begin` and `00g_intro_begin` have disjoint declared storage grids along
Y, yet share two tile coordinates in every layer, with different data in all
twelve tiles. Tile ownership therefore matters beyond overlaps between the
declared probe bounds themselves.

Tile ownership depends on registration and group-change history. Each physical
tile copies its context's runtime node ID, signed priority and scene group when
registered. Later priority edits do not change those copies. Tiles at the same
layer/coordinate remain in a multimap, newest first.

Area contexts are created with priority zero. The authored
`BakedLightingPriorityModifierMP1` component can set the owning area's priority
on entity load or activation, provided all component activity gates pass. Its
signed property `8582d268` defaults to one; deactivation does not restore the
previous priority. The callbacks (`0x7100f915a8`, `0x7100f915dc`) update the
context through its proxy; when that value reaches registered tile copies
depends on registration order.

Registration replaces the first active entry only for a **strictly greater**
priority. Removal of an active entry chooses the highest-priority remaining
entry from another node, keeping the first candidate on a tie and ignoring group.
Thus ties behave differently on registration and removal.

A change to a present group visits that group's inactive tiles in multimap order.
Each queues a mapping, even when a higher-priority entry remains flagged active;
it clears only the first active entry of equal or lower priority. Consequently,
several entries can be flagged active at one coordinate. The last queued mapping
determines the requested texture, not the flags alone. Repeating the same group
does nothing, and clearing the group does not undo existing mappings.

The [tile registry](../lib/src/format/baked_lighting/residency.rs) reproduces these
transitions and exposes both bookkeeping flags and scheduled physical tiles.
Its regression covers 1,536 event transitions against the original ARM64,
including native insertion, removal compaction and mapping-request construction.
It models requests, not asynchronous GPU job completion or unbound-page contents.
Sources: registration `0x71001ca01c`, group activation `0x71001c9c48`, and the
removal predicate `0x71001cba28`.

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
