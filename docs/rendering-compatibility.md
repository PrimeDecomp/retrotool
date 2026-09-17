# Graphics dependency compatibility

Maintenance notes for graphics dependency changes. The viewer now uses Bevy
0.19.1 and wgpu/Naga 29.0.4, matching remax's native tests. Remax still retains
Naga 0.11 as a second validator pending a separate compatibility cleanup. These
are host implementation constraints, separate from MPR's formats and Maxwell
semantics. Update or remove entries when their workarounds are retired.

## Package I/O adapter

**Retired:** `AssetIo` has been replaced by the named `mpr` asset source and
`AssetReader`. The ordered catalog remains independent of Bevy. Session identities
isolate stale tasks and cached dependencies after Clear; every child request
retains its source and session. Do not restore completion-order lookup or global
UUID-only cache keys when changing this adapter.

## Leading-bit builtins

[integer.wgsl](../remax/src/wgsl/integer.wgsl) implements `mpr_flo` with bounded
integer steps. Naga 0.11 lacks `countLeadingZeros`, and its Metal `firstLeadingBit`
lowering uses a leading-zero count where a bit index is needed. The semantic
[instruction emitter](../remax/src/program/wgsl/instruction.rs) also uses this
helper for `CountLeadingZeros`.
The scaled-multiply helper also uses it to normalize its integer significand.

After retiring the old validator, use `countLeadingZeros` and retain the
Maxwell signed-input, inversion and `.SH` rules around it. Parsing/validation of
the builtin passes with Naga 29 and fails with 0.11. Run the bit-operation numeric
cases and native kernel comparisons before removing the helper; compilation
alone does not check the Metal result.
Include the scaled-multiply GPU comparisons in that check. Integer product
scaling itself is required to avoid intermediate floating-point rounding; it is
not an old-Naga workaround.

## Texture-query types

Both [legacy texture emission](../remax/src/wgsl/texture.rs) and
[semantic texture emission](../remax/src/program/wgsl/texture/emit.rs) explicitly
convert texture dimensions, level counts and layer counts to unsigned values.
Naga 0.11 returns signed query values; Naga 29 returns unsigned values. A direct
unsigned-return probe passes only the newer validator.

The casts are harmless and may remain. During cleanup, remove only redundant
query-result casts; signed texture coordinates and native layer-bit handling
have separate semantics. Re-run texture bounds, array-layer and multisample tests.

## Compressed volume textures

[texture_format_supported](../retrotool-gui/src/loaders/texture.rs) still forces
CPU decoding for compressed non-2D textures. The original reason was wgpu 0.15;
the current viewer does not request `TEXTURE_COMPRESSION_BC_SLICED_3D` either.
Device format support is read from the initialized RenderDevice, not guessed
before backend initialization.

Replace the blanket restriction with format/dimension capability checks when
sliced-3D support is enabled. Ordinary BC support alone is insufficient. Keep
CPU decoding for unsupported formats/devices, and check mip uploads and probe
sampling before removing this restriction.

## Disabled anisotropy

**Retired:** the old `Option<NonZeroU8>` workaround is gone. The modern
[sampler_descriptor](../retrotool-gui/src/loaders/texture.rs) uses integer 1 for
isotropic sampling, preserving native maximum anisotropy 1. In the old Vulkan
backend, `Some(1)` incorrectly enabled anisotropy.

## Dock selection after removal

`egui_dock` 0.21.1's `retain_tabs` can leave the active index beyond the remaining
tabs. The viewer closes tabs through `remove_tab`, finding their paths again after
each tree rebalance. Keep that selection repair if bulk removal is changed;
Clear must show the surviving tab immediately. This is an upstream docking API
constraint, unrelated to shader semantics.

## Dispatcher final return

[control.rs](../remax/src/program/wgsl/control.rs) appends a typed fault return
after the dispatcher loop, despite every executable exit already returning.
This was introduced for the old validator, but **Naga 29 still requires it**:
a non-void function containing only `loop { switch ... { default: return ... } }`
fails return-type validation in both versions.

The same return is retained after a structured loop with no guest exit. A
[regression test](../remax/src/ir/tests/wgsl/loops.rs) removes it from the emitted
kernel and confirms that both Naga 0.11 and 29 reject the result. Finite structured
loops need no extra return because execution continues into their actual exit.

Keep the return until the selected validator accepts the exact emitted shape
without it. It is not evidence that normal shader execution can reach that fault.

## Compatibility tests and workspace split

[Texture validation](../remax/src/ir/tests/wgsl/texture.rs) skips Naga 0.11 for
ordinary depth sampling: old WGSL required a floating LOD, current WGSL an integer.
[Graphics validation](../remax/src/ir/tests/wgsl/graphics.rs) similarly uses only
the newer validator for quad-operation and dual-source-blending fragments, and
clip-distance vertex outputs.
Naga 0.11 predates `dual_source_blending` and `@blend_src`; the viewer's wgpu 29
supports both directly, with its `DUAL_SOURCE_BLENDING` device feature. No emulated
blend pass or rewritten output is needed. These compatibility exceptions
can now be removed with the old validator dependency. The viewer defaults to the
legacy translator and offers the semantic compiler as an experimental selection.
Both semantic profiles require asynchronous fault reporting; the viewer currently
limits them to Metal with subgroup and vertex-storage-write features enabled.

Remax and [render-tests](../render-tests/) have separate Cargo workspaces/lockfiles
originally because old/new graphics dependency trees had incompatible `web-sys`
requirements. All renderers now use wgpu 29. Re-evaluate that conflict before
folding the integration harness into the root workspace; the workspaces have not
yet been merged. Keeping an independently testable compiler remains useful and
does not require rejoining the root workspace.

## Clip distances on Metal

Remax emits WGSL `clip_distances` for enabled native vertex outputs. wgpu 29.0.4
advertises `CLIP_DISTANCES` on the tested NVIDIA Vulkan adapter, but not on
Apple M5 Pro/Metal. Naga 29 validates the generated modules; device creation
requesting the feature fails on that Metal adapter. The viewer checks the enabled
feature and reports a fallback instead of submitting an unsupported pipeline.
The legacy translator rejects clip-distance shaders explicitly.

The pinned wgpu development checkout (`0aee018d25ccd39f73668bf86d05cc2b03833f7a`,
version 30) enables this feature in its Metal adapter. Upgrade the aligned
Bevy/wgpu dependencies when available, then run Remax's clip-distance GPU test
and an original decal draw on Metal before removing this limitation. No fragment
discard substitute or local dependency patch has been added. Naga 0.11 predates
the WGSL extension; only the modern validator checks these vertex modules.

The Vulkan test checks 6,656 pixels against independent half-spaces, with sparse
plane masks, unequal clip W, 1×/4× MSAA and both compiler profiles. Enabled but
unwritten outputs fault; disabled outputs are not observed. This establishes the
wrapper's clipping on that backend, not the original decal shader or scene setup.

## Limits an upgrade does not remove

- Implicit derivatives still require correct lane participation. Naga 29's
  relaxed fragment-uniformity validation is not evidence of correct execution.
- Helper-inclusive quad fault reporting is backend-specific; it has only been
  established on the tested Metal device. Both semantic profiles currently use it;
  it is not a workaround for Naga 0.11.
- IEEE/SFU approximations, native sampling behavior, missing bindings and
  synchronization semantics still need their own evidence.
- Compression-block alignment and fallbacks for unsupported hardware formats
  remain necessary. Do not remove them with the old compressed-3D restriction.

## Metal triangle interpolation and vertex writes

The Eden comparison adapter duplicates vertices per triangle. On M5 Pro / macOS
26.6.2, adjacent indexed triangles produce different helper values depending on
whether the vertex shader can write storage. A never-taken vertex atomic branch
changes derivatives while leaving covered UVs unchanged. Separate triangle draws
and duplicated vertices agree with the instrumented path. The asset-free
[Metal probe](../render-tests/metal/triangle-interpolation.swift) reproduces this
without Naga, wgpu, textures or game code.

This is consistent with triangle merging: Mesa's Asahi compiler explicitly
[controls that optimization](https://gitlab.freedesktop.org/mesa/mesa/-/blob/822120d6d5c0109f98c3fbf85ec716c5d1fa00b6/src/asahi/compiler/agx_compile.c#L3807).
The precise Apple driver mechanism and Maxwell's corresponding behavior remain
unverified. An Eden/legacy image difference at these edges therefore does not,
by itself, establish a Remax lifting error.

Only comparison fixtures change vertex sharing; primitive order, attributes,
shaders and fault reporting are preserved. This is not an old-Naga workaround
or a production geometry change. Re-run the native probe after driver upgrades;
remove the adapter only when indexed and isolated triangles agree for the
reference comparison. Do not remove vertex fault reporting to force image parity.

## Modern Metal timestamps

The wgpu 29.0.4 [material harness](../render-tests/tests/material/gpu.rs) also
encounters missing/reversed render-pass timestamps on Apple M5 Pro, macOS 26.6.2.
It rejects these samples rather than converting underflow into a duration.
[wgpu #9414](https://github.com/gfx-rs/wgpu/issues/9414) reports zero counters on
macOS 26; a shared cause has not been established. This is separate from the
old viewer dependency constraints. Recheck after host/backend updates before
using these queries for performance comparisons; retain invalid-sample checks.

## Generated pipeline compilation

Bevy 0.19.1 still compiles pipelines synchronously on macOS, and its global cache
has no per-pipeline removal API. Its original [async compilation change](https://github.com/bevyengine/bevy/pull/10812#issuecomment-1913956813)
excluded macOS after Metal problems; the [wgpu discussion](https://github.com/gfx-rs/wgpu/issues/3794#issuecomment-1954631975)
reports slower threaded compilation through Metal's synchronous API. wgpu 29.0.4
does not expose native `create_render_pipeline_async`.

The viewer's [generated-pipeline adapter](../retrotool-gui/src/render/original_shader/pipelines.rs)
uses one Bevy task-pool worker, checked wgpu creation and demand-owned requests.
This avoids concurrent compiler jobs and permits individual programs to be released.
It does not change Bevy's ordinary material cache or bypass shader runtime checks.
An active driver call can finish after its last draw closes; the result cannot
reattach to a replacement instance. This is a native desktop adapter, not a browser
async implementation.

On the tested M5 Pro/Metal system, a vine replay with fresh generated identifiers
completed compilation while 94–97 frames advanced over about 1.35–1.39 seconds;
the slowest measured update was about 8 ms. Rendering remained identical. This is
responsiveness evidence for one program/device, not a compiler-speed improvement
or proof of other Metal versions. Re-evaluate the worker when wgpu exposes native
async compilation; retain explicit cache ownership and validation/fallback tests
when considering Bevy's cache again.

## Custom transparent draws

Generated material meshes require Bevy's `NoAutomaticBatching`. Their programs
and bind groups live outside Bevy's material cache, so the phase's shared pipeline
sentinel cannot identify compatible batches. Without that marker, Bevy 0.19 merges
Varia's four transparent eye/visor meshes into one batch; the custom draw command
draws its first mesh and the phase skips the other three. This affects both
translators. The gallery replay verifies separate phase entries and completed
semantic draw checks. Keep the marker unless the renderer gains real batch-aware
binding and draw support; a dependency upgrade alone does not remove the requirement.

## Reference vertex import

The independent [vertex comparison adapter](../render-tests/tests/material/reference/shader.rs)
uses Naga 29 to import Ryujinx output. Naga rejects `BuiltIn PointSize`, so the
adapter preserves that unused output as location 31 before converting the vertex
entry into a compute-callable function. Remove this remapping if a future importer
accepts PointSize; this test does not validate point rasterization. No original
Maxwell instruction or position/varying arithmetic is replaced.

Ryujinx also declares unused image/subgroup capabilities. Import accepts those
declarations with `strict_capabilities: false`, then fully validates the actual
module and removes unused declarations. Re-enable strict capability checking when
the reference emits only supported declarations or Naga supports the declared set.
These are temporary reference-import adaptations on **Naga 29**, independent of
the old viewer upgrade. SPIR-V remains test interchange, not a remax target.

## Reference fragment import

The [Eden draw adapter](../render-tests/tests/material/eden/shader.rs) uses
SPIRV-Cross to emit Metal directly. wgpu accepts only the two hash-pinned modules
through its shader passthrough API, with an explicit layout matching inspected
Metal resource slots. This also separates the reference backend from remax's
Naga backend. SPIR-V remains temporary reference interchange.

Direct import through Naga 29.0.4 exposed three limitations:

- Integer texture coordinates with named vector types trigger a lookup panic in
  `extract_image_coordinates`; `parse_type_image` only ensures an unnamed float
  coordinate type exists. Stripping SPIR-V debug names avoids that panic.
- `OpIsNan` imports and validates, but Naga's WGSL writer cannot express it.
- The imported fragment's loop/phi expressions expand to 98,927,300 bytes of MSL.
  Explicit expression names do not prevent this expansion. The Metal compile
  was stopped; that path produced no comparison results.

The intermediate probes and import adaptations were removed. Revisit direct
import only after reproducing and resolving these cases on the chosen Naga
version. They are independent of the old viewer upgrade; no workaround was
added to the production compiler. SPIRV-Cross emits a 17,084-byte fragment for
the same reference SPIR-V.

The old/new Naga comparisons were reproduced locally on 2026-09-16. Removal
checks should use the actual dependency versions selected for the upgraded viewer.
