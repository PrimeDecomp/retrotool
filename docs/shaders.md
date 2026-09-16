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
| Direct TEXS descriptor operand | Word offset `8 + 2*n` |
| Stage compiler constants | That stage's physical bank `c[1]` |

These follow `PreDrawCallback` (`0x71001a8fdc`), SDK offsets `0x4825c0` and
`0x4839e0`, and the SDK's program-binding macro. Compute takes a different
driver path; the graphics mapping has not been established for it. uam and NVK
driver conventions are also not substitutes for NVN's bindings.

Uniform allocation does not clear the payload. Partial writes and one-byte
sentinels leave other bytes untouched; padding cannot be assumed to contain
zero. Retrotool tracks byte validity and rejects reads of unknown data.
Matrix binders may transpose CPU values, while uploaded snapshots are already
in shader order. See [uniform assembly](../lib/src/format/shader_nvn_uniforms.rs).

## Runtime semantics

Retrotool translates selected Maxwell programs into WGSL at runtime,
reconstructing stage IO, constant memory and resource access.

Some compiler intermediates are opaque. Supported `AL2P` → `ALD` and `AL2P` →
`IPA` pairs are lifted as logical attribute operations using bound selectors.
This does **not** recover arbitrary physical attribute-address or predicate bits.
Selector assumptions are recorded and checked again when binding. Two paired
`DataView_Model` DX/NVN variants establish the UV operation: material
`7405a780-310a-4b1a-8027-c83f5f557589`, hash `5b02d1a5`, technique `x`,
permutations `8c` and `94`. See
[attribute lifting](../remax/src/wgsl/attributes.rs).

Fragment `KIL` suppresses output while execution can continue. Converting it to
an immediate host discard can invalidate later derivative-dependent sampling.
Closed reconvergence regions are supported as per-invocation control flow;
arbitrary warp behavior is not established. A proven one-mip view with matching
min/mag filters and no anisotropy permits some implicit samples to use level
zero, but that proof depends on the actual bound resource.

The CPU control record also carries a pixel sample-shading flag. The game builds
three handles from the same programs: preserve that flag, force per-sample, or
force per-fragment execution. These are not three MSAA counts. Host sample
coverage and raster positions can still differ from the game's custom pattern.

Unresolved areas include broader control flow and texture forms, raw indexed
interpolation, and complete Maxwell floating-point behavior. SM75 measurements
of interpolation offsets support the adopted model but are not SM53 hardware
verification. Translation faults and safety limits are host policies. The
[translator](../remax/src/wgsl/mod.rs) is the current authority
for supported forms; pipeline compilation alone does not establish fidelity.
