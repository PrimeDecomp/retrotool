# Shader programs and bindings

## NVN and D3D archives

The startup `MaterialArchive.arc` contains both `SNVN` and `SDX ` backends.
The examined cooked model MTRL resources contain SNVN only. A DX counterpart
for the main cooked model ubershader has not been located.

Pair backends by material GUID **and hash**, technique and full permutation.
Source indices are backend-local; NVN sources commonly contain several stages.
Matching keys do not guarantee identical bindings or stage topology: some
startup effects use DX compute versus NVN vertex/pixel stages.

SDX contains a byte-packed length envelope and zlib-compressed DXBC. Its layout
was established from assets; no D3D loader was found in the Switch executable.
SM5.0 reflection retains buffer, variable and resource names. These names help
interpret NVN data, but source HLSL is not present in the examined records.
See [SDX](../lib/src/format/shader_sdx.rs) and
[DXBC](../lib/src/format/shader_dxbc.rs).

## Separating NVN code from data

An SNVN envelope decompresses into a GPU pool followed by a CPU pool. Each stage
has an offset into each pool. Stage order does not establish range boundaries;
the vendor CPU control record supplies exact program and allocation lengths.
`LoadBinaryPools` (`0x71001a790c`) and `BuildFromDelayedData` (`0x71001a9370`)
connect those pools to the bundled NVN SDK.

For the supported control-v16 revision, graphics records contain a prefix,
an 80-byte hardware shader header (SPH), then Maxwell instructions. Compute uses
a different prefix and no SPH. Compiler constants begin after the code
allocation; their exact length excludes the remaining padding. Decoding the
whole GPU pool as instructions would include both headers and data.

Maxwell interleaves one scheduling word with three instruction words. The SPH
supplies stage IO and interpolation needed to reconstruct the graphics interface.
See [NVN programs](../lib/src/format/shader_nvn_program.rs) and
[Maxwell decoding](../remax/src/maxwell/mod.rs).

Pixel outputs occupy compacted GPR groups at shader exit: each present color
target reserves four registers, while absent targets consume none. The mask
register follows those groups; depth is one register later even when mask output
is disabled. Thus depth-only shaders export R1, and one-color-plus-depth shaders
export R0–R3 and R5. These assignments agree with MPR's original programs,
[uam's slot allocation](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/source/compiler_iface.cpp#L129)
and [NAK's output lowering](https://gitlab.freedesktop.org/mesa/mesa/-/blob/822120d6d5c0109f98c3fbf85ec716c5d1fa00b6/src/nouveau/compiler/nak/from_nir.rs#L2995).

Output routing is separate from register allocation. The SPH MRT flag and runtime
MRT state determine whether colors go to corresponding targets or output zero is
broadcast. All 565 indexed startup VS/PS pairs without geometry have the SPH
flag enabled; that alone does not establish runtime routing. See
[NVIDIA's MRT definition](https://nvidia.github.io/open-gpu-doc/Shader-Program-Header/Shader-Program-Header.html).

Dual-source blending is another routing case. Varia's glass pixel shader exports
R0–R3 and R4–R7 with both color masks enabled; its [blend state](materials.md#draw-state)
uses the latter as secondary blend operands for attachment zero. The masks alone
cannot distinguish this from two framebuffer outputs. Resolve routing from the
effective draw state before linking the shader interface. WGSL expresses this
using two outputs at location zero with distinct
[`blend_src` attributes](https://www.w3.org/TR/WGSL/#attribute-blend_src).

## Thread-local storage

The SPH's local-memory sizes describe runtime storage private to each shader
invocation, not another region of serialized shader data. `STL` and `LDL` address
that storage in bytes. It commonly holds register spills: Sheegoth's body pixel
shader reserves 16 bytes and spills two words at offsets 0 and 4. The separate
CRS allocation serves call/return stacks; it is not additional spill space.
See [NVIDIA's SPH specification](https://nvidia.github.io/open-gpu-doc/Shader-Program-Header/Shader-Program-Header.html).

Cache modifiers depend on the instruction family. In this shader, the loads
are `LDL.LU`, whose code 1 means last use; code 1 on `STL` is `.CG`. Sharing a
global-load cache enum would mislabel them. CUDA 12.9's SM53 disassembler confirms
these spellings. Local contents are established by executed stores, not by
zero-filled bytes in the asset. Cache hints do not provide initial values.

## Stage interfaces and defaults

A pixel input need not appear in the preceding shader's output map. The hardware
can supply missing components from default state; this differs from an output
that is declared but never written. NVIDIA describes the distinction in its
[inter-stage attribute mapping](https://patents.google.com/patent/US20110080415A1/en).

MPR's bundled SDK emits `SET_ATTRIBUTE_DEFAULT = 0xe`, selecting `(0, 0, 0, 1)`
for missing generic components. The command is at SDK-relative `0xa51f28`;
instructions at `0x48c880`–`0x48c898` copy its containing quadword into a command
buffer. These are offsets in the decompressed SDK NSO, not the main executable.
See [NVIDIA's method definition](https://github.com/NVIDIA/open-gpu-doc/blob/master/classes/3d/clb197.h).

Sheegoth's head and eye shaders illustrate why the maps matter: their vertex
programs export `0xcc`, but omit `0xc0/0xc4/0xc8`, which the pixel programs read.
Those three components receive zero. Defaults apply component by component;
neither the material nor the renderer should invent writes to the omitted outputs.
An exported component whose value is unavailable still remains unavailable.

Clip distances are rasterizer outputs, independent of pixel-shader inputs.
SPHv3 VTG headers declare eight distances at attributes `0x2c0..0x2dc`;
their input map occupies word10 bits16–23 and output map word18 bits0–7.
MPR's SDK initializes `USER_CLIP_ENABLE` to `0xff`: the immediate command
`0x80ff0544` at SDK offset `0xa53ca4` is copied into the initialization buffer
by `0x48c53c`–`0x48c550`. Thus all declared distances participate in clipping
under this state. The final Decal_Scorch vertex shader exports six distances
from the geometry generator, clipping whole triangles to the decal box before
pixel shading. A fragment discard test would not reproduce that stage boundary.

## From feature semantics to hardware bindings

Binding crosses several distinct namespaces:

```text
material data ID → selected feature semantic → packed shader variable
    → NVN reflection entry → logical API binding → hardware location
```

A uniform variable identifies a buffer and byte offset; its binder supplies
the type and write length. A resource variable identifies per-stage API slots.
Unbound semantic entries still occupy positions in the feature stream.

NVN reflection itself has two offset origins: outer ranges start after its
header, while buffer-reference, uniform and resource tables are relative to
the nested binding blob. See [reflection](../lib/src/format/shader_nvn.rs).

The bundled SDK establishes these **graphics** mappings:

| Input | Hardware location |
| --- | --- |
| Logical uniform buffer `n` | Constant bank `(n + 3) & 31` |
| Logical texture slot `n` | 64-bit handle at driver-table byte offset `0x20 + 8*n` |
| Direct TEX/TEXS descriptor operand | Word offset `8 + 2*n` |
| Stage compiler constants | That stage's physical bank `c[1]` |
| Storage buffer `n`, vertex / pixel / geometry | 16-byte descriptor at `c[0][base + 16*n]`, with base `0x110` / `0x510` / `0x410` |

These follow `PreDrawCallback` (`0x71001a8fdc`), SDK offsets `0x4825c0` and
`0x4839e0`, and the SDK's program-binding macro. Compute takes a different
driver path; the graphics mapping has not been established for it. uam and NVK
driver conventions are also not substitutes for NVN's bindings.

Some material shaders use ordinary `TEX` alongside `TEXS`. Both consume the
same descriptor table, but `TEX` packs enabled output channels into consecutive
registers. In particular, its R/B mask appears in Observatory materials and
cannot be represented by a single two-result `TEXS` group. Instruction family,
channel mask and source grouping must survive decoding before lowering to a
shared sampling operation.

Reflection resource kind 1 denotes a storage buffer, distinct from kind 0
textures. `BindStorageBuffer` (`0x71001a7174`) supplies its GPU address and
`element count × stride` byte length. SDK `0x4829d0` and macro 16 establish the
descriptor mapping above: an address low/high pair, byte length, then padding.
Single-buffer binding leaves that padding untouched. Shaders construct addresses
from these words and use `LDG`; the descriptor is not the buffer's contents or
an offset into an asset. The cached material binder does not supply storage
buffers, so resolving a material's constants alone cannot establish these inputs.

Startup `LumaUpdate` uses `LDG.E.CI.U.128`, which `nvdisasm` distinguishes from
ordinary `.128` loads found in other programs. The extra `.U` semantics remain
unresolved; a 16-byte memory snapshot alone does not establish their equivalence.

`GSV_` binds the shared `ub_globalSystemValues` buffer, independently of the
feature's packed variable. It supplies absolute modular time and inverse tonemap
exposure; the native constructor initializes these to zero and one. Reflection
flag bit 3 identifies the buffer. A standalone renderer must explicitly supply
this state rather than look for it in model material data. The binder
(`0x7100171c54`) and upload (`0x71000f95c4`) are implemented by
[global uniforms](../lib/src/format/shader_nvn_globals.rs).

Uniform allocation does not clear the payload. Partial writes and one-byte
sentinels leave other bytes untouched; padding cannot be assumed to contain
zero. Retrotool tracks byte validity and rejects reads of unknown data.
Matrix binders may transpose CPU values, while uploaded snapshots are already
in shader order. See [uniform assembly](../lib/src/format/shader_nvn_uniforms.rs).

`SimpleTexture/x/permutation8` is a compact example of this chain. Its `TCH0`
texture semantic resolves to pixel slot0 (descriptor word8). `CCH0` supplies
UV offset XY and scale ZW; `CCH1` supplies RGBA modulation; `RCH0` supplies the
explicit mip level. These data IDs are reused by other materials, so their
meaning depends on the selected feature graph. The native instance builder
(`0x7100184f10`) initializes the mip level to zero; it remains a uniform read,
not an instruction literal. Exact DX/NVN pairs identify `uc_diffuseMap` as a
float-returning 2D texture, without fixing its storage format or bound mip range.
The [original-program fixture](../remax/src/ir/tests/wgsl/graphics/original/simple_texture.rs)
retains the shader identity and tests those bindings together.

## Runtime semantics

Retrotool translates selected Maxwell programs into WGSL at runtime,
reconstructing stage IO, constant memory and resource access.

Float-to-integer NaN handling also depends on runtime GPU state
(`NVB197_SET_SHADER_CONTROL.FP32_F2I_NAN_BEHAVIOR`, register `0x1690`, bit 2),
not just bytecode.
This matters even without lights: the vine shader's zero cluster parameters
produce `log2(0) * 0`, then convert the resulting NaN at PC `0x5a8` to an integer
texture coordinate. The preview explicitly selects conversion to zero. The
game's register setting remains unverified; this preview choice is not a Naga
workaround or a recovered material property.

The baked-atlas and decal shaders use `F2I.U16.F32`. This is a saturating
conversion, not a 32-bit conversion followed by a mask. Signed 16-bit results
extend their sign to the register; the indefinite NaN value is instead
`0x00008000` for either signedness. Remax retains the destination width.
These edge rules have CPU/Metal regressions and SM75 hardware evidence; SM53
exceptional-value behavior remains unverified.

`FMNMX` has two independent predicates: one controls execution, while its operand
predicate chooses minimum or maximum. It prefers a number to a NaN and orders
negative zero below positive zero; FTZ preserves the sign of flushed subnormals.
These rules agree with [PTX min/max semantics](https://docs.nvidia.com/cuda/parallel-thread-execution/index.html#floating-point-instructions-min)
and an SM75 hardware probe. That probe returned `0x7fffffff` when both operands
were NaNs. Remax uses this payload in its comparison model; it has not been
verified on the Switch's SM53 GPU.

`FMUL` can scale its product by a power of two. This is not generally equivalent
to inserting another rounded multiplication before or after it: intermediate
overflow or underflow can change the answer. SM75 measurements support scaling
the exact product before its final rounding, with FTZ flushing a tiny exact
result even when rounding would produce the minimum normal number. Saturation
follows the scaled result. Remax retains the scale in its semantic operation;
the hardware evidence does not yet establish SM53 edge behavior or NaN payloads.

`AL2P` converts a logical attribute selection into a native address/predicate
pair. Recovering the complete `AL2P` → `ALD` or `AL2P` → `IPA` operation permits
logical reads without reconstructing those intermediate bits. The IPA operand
predicate belongs to this pair; it is distinct from the instruction's execution
guard. Copies and control-flow joins must preserve the pairing.

A register can hold an attribute address on one path and loaded data on another.
For example, a guarded load can overwrite its destination while the complementary
load still uses the earlier address. Such joins require path-sensitive value
recovery; treating every value in that register as either an address or raw bits
loses the distinction. The read's execution guard determines which producer
reaches it.

Vertex selectors choose packed XY/ZW coordinate pairs; fragment selectors choose
among the transformed varying vectors. Both can change through uniform updates
without changing the shader. Linked stage headers bound the possible inputs,
but only the selected component must be available when a read executes. The
semantic compiler preserves this runtime selection; the legacy translator
specializes from bound selectors and checks them again on upload. Two paired
`DataView_Model` DX/NVN variants establish the UV operation: material
`7405a780-310a-4b1a-8027-c83f5f557589`, hash `5b02d1a5`, technique `x`,
permutations `8c` and `94`. See
[attribute lifting](../remax/src/maxwell/lift/attributes.rs).

The established fragment path uses matching AL2P pairs with perspective generic
inputs. Raw native addresses, mismatched predicates and state-selected or flat
indexed interpolation remain unresolved. Out-of-domain selections fault under
the host contract; substituting zero is not established game behavior.

`RRO.EX2` prepares an opaque intermediate for `MUFU.EX2`; `RRO.SINCOS` prepares
one for `MUFU.SIN` or `MUFU.COS`. Complete pairs approximate exp2 or sin/cos of
the original argument, with input modifiers before reduction and saturation
after the special function. Sin/cos take **radians**; the normalized direct-MUFU
convention on newer GPUs does not apply to this pair. Mesa's pre-SM70
[builders](https://gitlab.freedesktop.org/mesa/mesa/-/blob/822120d6d5c0109f98c3fbf85ec716c5d1fa00b6/src/nouveau/compiler/nak/builder.rs)
and Switch recompilers agree on the complete operation. Copies and compatible
joins can retain the intermediate, and one producer can feed several consumers.
Its native bits remain unknown; exponent and sin/cos reductions cannot mix.

Host special functions do not reproduce Maxwell's approximation precision.
This matters beyond smooth lighting: `SimpleTexture` permutation `0x10` uses
`fract(sin(argument) * 43758.546875)` for noise. Small sine errors can change
the fractional result substantially, especially across an integer boundary.
The retained [original arithmetic slice](../remax/src/ir/tests/trigonometry/original.rs)
tests the operation sequence; exact noise requires an SM53 numerical reference.

Fragment `KIL` demotes an invocation to a helper: its exports are suppressed,
but subsequent instructions can still supply neighboring fragments' derivatives.
It is not an ordinary early return. The grass source130 fragment illustrates
this with predicated `KIL` at `0x150`, followed by implicit samples at `0x158`
and `0x168`. Mesa lowers NIR demotion to this instruction; Eden likewise retains
demotion as a separate effect in its IR.

Closed reconvergence regions are supported as per-invocation control flow;
arbitrary warp behavior is not established. A proven one-mip view with matching
min/mag filters and no anisotropy permits some implicit samples to use level
zero, but that proof depends on the actual bound resource.

A predicated instruction differs from a control-flow branch: skipped lanes
still participate in the next instruction. Area mushroom shaders illustrate
this with complementary predicated moves followed by an unconditional implicit
texture sample. Lowering the guards to scalar branches must preserve that rejoin;
placing the later sample inside a per-lane dispatcher would lose its derivative
context. A sample inside the guarded instruction remains conditional. Remax
retains this provenance separately from SSY/SYNC regions and defers host faults
until the proven sampling prefix has finished. This does not establish general
reconvergence after native branches or loops.

Texture `NODEP` marks accesses whose results need not be available to helper
invocations. It is separate from implicit LOD and does not justify substituting
level zero or moving the access across control flow. uam encodes its `liveOnly`
flag at bit 49; Mesa NAK derives `nodep` from NIR's `skip_helpers`.
See [uam's Maxwell encoder](https://github.com/devkitPro/uam/blob/5a5afc2bae8b55409ab36ba45be63fcb73f68993/mesa-imported/codegen/nv50_ir_emit_gm107.cpp#L2918)
and [NAK texture lowering](https://gitlab.freedesktop.org/mesa/mesa/-/blob/822120d6d5c0109f98c3fbf85ec716c5d1fa00b6/src/nouveau/compiler/nak_nir_lower_tex.c#L448).

The CPU control record also carries a pixel sample-shading flag. The game builds
three handles from the same programs: preserve that flag, force per-sample, or
force per-fragment execution. These are not three MSAA counts. Host sample
coverage and raster positions can still differ from the game's custom pattern.

Offset IPA samples relative to the pixel center. Perspective attributes must
first interpolate their screen-linear `attribute / clip-W` plane at that offset;
the instruction then applies its multiplier. The reciprocal-W plane is a
separate input. Offsetting the already perspective-correct quotient is not
equivalent. A host sample or centroid value must also be recentered before it
can serve as a plane's center. Derivatives used to build these planes belong
before divergent shader execution and must use the correct raster axis directions.
WGSL's [interpolation qualifiers](https://www.w3.org/TR/WGSL/#interpolation)
provide center, centroid and sample locations; remax's dynamic offset path uses
explicit planes rather than changing the varying qualifier.

Unresolved areas include broader control flow and texture forms, general indexed
interpolation, and complete Maxwell floating-point behavior. SM75 measurements
of interpolation offsets support the adopted model but are not SM53 hardware
verification. Translation faults and safety limits are host policies. The
[compiler](../remax/src/program.rs) is the current authority
for supported forms; pipeline compilation alone does not establish fidelity.
