# Materials and permutations

## Resource identity and instance values

A model material's first GUID selects a cooked **MTRL resource**; its second
identifies the **implementation** inside it. Resources can share an implementation
while containing different permutations. The material-set hash identifies
external variable bindings. It is not the displayed material name's hash in the
checked assets; its naming input remains unknown.

The model supplies material values and shared values, while the draw context
supplies transforms, lighting, animation and other runtime data. Ordinary typed
lookups visit providers in order and return the first nonnull value. The normal
model draw path puts caller data before per-material data, then shared model
data. Flags and presence masks combine across providers. Presence can therefore
select a feature even when the eventual typed lookup returns null.

A `BKLT` backlighting trait creates a shared `BKLB` height-range provider for
all model materials. It uses the model's native Y bounds and follows serialized
shared values in provider order; it is not another record in the file.
`BKGL` comes from the render pass: back/top backlight intensities followed by
two zeros. The UI pass uses the engine defaults `(4, 2)`; world passes can
modulate them through backlight hints. This is separate from baked probe lighting.

`LODP` is runtime view data: the **culling camera's world position**, followed
by `1`. It is not a vector of authored LOD thresholds. The render graph constructs
it in `NSceneRenderGraphs::SContext` (`0x71001af4cc`); using a fixed value while
moving the camera changes distance-dependent shader behavior.

`LoadMaterialCache` (`0x7100348fe8`) and `SetupMaterialForRenderMesh`
(`0x71001fc014`) establish these relationships. See
[model parsing](../lib/src/format/cmdl.rs) and
[instance selection](../lib/src/format/material_predicates.rs) for the data types.

Standalone **MATI** resources supply the same kinds of instance values for
consumers such as decals, without a model material cache. Their implementation
and MTRL GUIDs appear in the reverse order from a model's. Cull and blend values
are requests; unset values receive defaults from the consumer. For geometry
decals, all six checked materials request the dual-source blend described below.

A nil MATI texture reference is **a present black texture**, not missing data.
Its property tag selects a 2D, array, cube or volume fallback; the native loader
uses a raw texture binding and ignores serialized usage overrides. Non-nil
references instead use the actual TXTR type and apply usage overrides. This
distinction affects both feature selection and binding. See the
[MATI parser](../lib/src/format/material/instance.rs), traced from the native
factory (`0x71001fdd20`).

## Archive roles

The standalone `MaterialArchive.arc` is MTRL version 22, containing shader
backends and material definitions. Cooked PAK resources use version 168: their
compressed payload contains `EXPT`, then an inner version-22 archive.

Two offset conventions are easy to confuse:

- Backend TOC offsets start **after the TOC**, before the source count. Material
  data ranges cover the length-prefixed data blob, excluding its material header.
- EXPT offsets address a **reconstructed runtime allocation**. Arrays are read
  sequentially from the file; allocation padding is not serialized.

Sources: `ImportMaterialArchiveWithTOC` (`0x710017fb20`),
`CMaterialIdentifierRuntime` (`0x710017b62c`),
[archive](../lib/src/format/material_archive.rs) and
[EXPT](../lib/src/format/material_runtime.rs) parsers.

## CPermutation

`CPermutation` stores local option indices for one feature set. Each feature uses
`max(1, ceil(log2(optionCount)))` bits, in feature order from the low bit of the
first word. A feature with one option still consumes one bit. Composite options
contain a separate permutation for their child set. **There is no universal
128-bit flag dictionary.**

For option counts `[1, 3, 5]`, the widths are `[1, 2, 3]`. Local choices
`[0, 2, 4]` pack as `0 | (2 << 1) | (4 << 3) = 0x24`. A local option then names
a feature-definition option; those two indices need not be equal. Binary words
are low first, while the engine's hexadecimal string is high first.

Cooked selection usually takes a shorter route. The feature graph builds an
ordered list of dynamic simple features; a technique's EXPT mask skips entries
in that list. Remaining choices form a **64-bit dynamic runtime ID**, which EXPT
maps to the full permutation. Static choices have already been incorporated
into that mapping. The dynamic key cannot substitute for the final permutation.

The main entry points are `FindMatchingShader` (`0x710018a644`),
`SelectDynamicRuntimeId` (`0x710014c078`) and `SelectPermutation`
(`0x710014be1c`). [Selection code](../lib/src/format/material_selection.rs)
contains the packing, traversal and lookup rules.

## Conditions and binding

Conditions address **one-hot option groups**, not permutation bits. Selection
sees preceding choices, prefixed by composite arguments where applicable.
Binding evaluates against complete choices and a separate composite argument
list. Treating these as one traversal produces incorrect binding flags.

Simple-feature requirements remove allowed *definition* options; composite
requirements remove *local* options. An ordinary composite's final option is a
fallback even if a requirement removed that option's bit. Explicit composites
delegate selection to their selector, but retain composite binding behavior.

A feature consumes its definition's complete semantic list from the packed
variable stream, including unbound entries. Skipping absent semantics shifts
all later bindings. The selected option supplies names; NVN reflection supplies
locations. Selection success and resource readiness are separate questions.

See [feature metadata](../lib/src/format/material_features.rs) and
[semantic definitions](../lib/src/format/material_feature_semantics.rs).

## Draw state

The engine composes technique state, current draw state, then conditional
permutation overrides. Each merge replaces only enabled fields. Overrides apply
in file order; an empty condition applies unconditionally. Blend state is
replaced as a whole, while its target mask controls which live targets change.
Explicit factors and equations are authoritative even when a preset is named.

Some transparent materials use **dual-source blending**: two pixel color exports
feed one framebuffer attachment. Varia's glass uses preset 10, whose RGB equation
is `source0.rgb + destination.rgb * (1 - source1.rgb)`; alpha instead uses
`source0.a + destination.a * (1 - source0.a)`. The secondary RGB permits different
background transmission per channel. Treating it as another attachment, or using
ordinary alpha blending, loses that behavior. The factors are established by
`SetBlendMode` (`0x71001addf0`) and the preset constructor (`0x71000ef984`).

Mesh classes, two-sided flags and transform handedness also affect a draw.
Shader alpha alone does not determine transparency. Mirroring must change the
front-face convention as well as culling because shaders can read front-facing
state. See [graphics state](../lib/src/format/material_graphics_state.rs), based
on `ApplyShaderPermutation` (`0x710018a830`).

## Engine edge cases

`SetMaskedValue` (`0x710014dec8`) mishandles writes straddling bit 64: the spill
is not inserted into the high word. The checked cooked sets use at most 58 bits,
so this has no demonstrated asset impact.

The separate bottom-provider path also has a native indexing defect: texture,
complex, debug, color and scalar getters test a bottom slot's mask but call a
normal slot's provider. Integer, integer-vector and matrix getters use the
matching bottom slot. Normal model setup uses ordinary append, so this should
not become a general provider rule. These are machine-code observations, not
intended format semantics.
