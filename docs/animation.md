# Animation inputs

## Animated UVs

Model HEAD data sources can contain `ANUV`, a CEvalSystem program with sample
handles, transform sets and a mesh-to-set map. The map follows **render-mesh
order**, not material-cache order. Evaluation produces up to three 2×4 UV
matrices for the material provider.

The serialized context mixes element sizes and big-endian values inside an
otherwise little-endian format. A backwards type map and length suffix describe
the elements to reverse; blindly swapping every word corrupts it. EvalIds select
either a value or time operation and address context words. The fourth index in
each transform set remains unexplained.

CEvalSystem quantization is independent of the ColorModifier clock below.
UV updates do not require shader translation unless specialization inputs change. The viewer's supported ANUV path updates these uniforms
during playback; finding a later ANUV source behind an unknown, unframed HEAD
source still requires that source's reader.

Original readers include `0x7100231c10` and `0x7100234170`; evaluators begin at
`0x7100233bb4`. See [ANUV](../lib/src/format/model_animated_uv.rs) and the
[CEvalSystem template](https://github.com/PrimeDecomp/PrimeRemasterStructs/blob/main/CEvalSystem.bt).

## Skinning

SMDL's SKHD is independent of HEAD. It supplies a bone count and mesh visibility
data; the per-mesh bytes are **visibility groups, not bone mappings**.

The SKIN binder uploads a palette of 3×4 matrices in shader order without
transposing or calculating a pose. An animation render-data reference wins over
the fallback palette even when its bone count is zero. Instanced bindings append
nonempty palettes in instance order. The native fallback constructor initializes
identity matrices, which explains why some skinned meshes can render before
animation is implemented.

Identity palettes do not establish skeleton hierarchy, inverse-bind transforms
or clip evaluation. Those remain to be recovered. The engine also has a vertex
buffer override path for more than 240 bones or a missing skinning flag; its
complete behavior is unresolved.

The palette binder is `0x7100166620`, fallback construction `0x7100166788`, and
mesh visibility test `0x71002ea810`. See
[skinning data](../lib/src/format/material_skinning.rs).

## Color curves and playback

Modern ColorModifier combines a timing spline with RGBA endpoints or a gradient.
Runtime spline preparation repairs closely spaced knots and normalizes tangents;
it must be separate from the authored representation. Exact-knot step behavior,
infinity modes and amplitude handling differ from common spline defaults.
The no-cache evaluator used by ColorModifier has been recovered; the persistent
cache path is not covered by that API.

ColorModifier's clock rounds playing time to 1/60000-second ticks. Looping wraps
at most once per update, uses strict unrounded endpoint comparisons, and checks
zero at the backward boundary even when the minimum is nonzero. Replacing it
with modulo changes events and colors. Startup randomness also consumes a
shared game RNG stream; independent seeds per object cannot reproduce it.

An explicit Think call can calculate color while stopped. Whether Think is
scheduled depends on activation and external-time state. The resulting color
still needs [script dispatch](scene.md) to reach its targets.

References are spline construction/evaluation (`0x7100099ecc`, `0x710009b364`),
playback update (`0x71005e6b54`), and ColorModifier Think (`0x7100734a04`). See
[spline evaluation](../lib/src/format/maya_spline_runtime.rs),
[playback](../lib/src/format/time_playback.rs), and
[color evaluation](../lib/src/format/room_color.rs).
