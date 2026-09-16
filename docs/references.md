# Research references

## Game executable and layouts

The primary source is the `remastered` Ghidra program, backed by MPR's ARM64
`main` module at base **0x7100000000**. Addresses in these pages refer to this
build unless explicitly labeled as SDK offsets.

| Module | NSO build ID, excluding zero padding |
| --- | --- |
| main | `67c9ca1428222752f951021464a7b3597eb98fec` |
| sdk | `2217a4a3c1f3c883278de72f8b008224ad252a24` |

The SDK is a separate module. Its offsets must not be added to main's base when
looking up code in Ghidra. Likewise, serialized structures often omit pointers
and padding found in runtime objects.

[PrimeRemasterStructs](https://github.com/PrimeDecomp/PrimeRemasterStructs) is the
primary field-layout reference. Function addresses accompany findings here so
that the original consumers can be revisited without preserving session logs.
Some interpretations were checked by executing original ARM64 routines with
controlled memory in Unicorn. Those comparisons establish the exercised routine
behavior, not a complete game run or Tegra GPU behavior.

Two analysis pitfalls are worth retaining. The loaded Ghidra processor model
misinterpreted the second load of `LDPSW` at `0x7100163c38`; DEFX uses data IDs
0 and 1, not 0 and 2. Also, relocated BSS is not initialized game state:
invalid-ID and script-event globals require their static initializers. Empty
BSS once misleadingly suggested that runtime ID zero was invalid.

## Shader references

- [devkitPro/uam](https://github.com/devkitPro/uam/tree/5a5afc2bae8b55409ab36ba45be63fcb73f68993): the GM107 encoder, NVC0 lowering and shader-header definitions are useful Maxwell references, especially for graphics instructions such as TEXS and AL2P pairs.
- [Mesa NVK/NAK](https://gitlab.freedesktop.org/mesa/mesa/-/tree/822120d6d5c0109f98c3fbf85ec716c5d1fa00b6/src/nouveau): `compiler/nak/sm50.rs`, SPH definitions and hardware tests provide independent encoding and behavior comparisons.
- [NVIDIA Shader Program Header specification](https://nvidia.github.io/open-gpu-doc/Shader-Program-Header/Shader-Program-Header.html): the CommonWord0 table agrees with observed VTG=1 / PS=2 headers; the opening prose reverses them.

uam and NAK are forward compilers, not MPR shader-container parsers. Their
instruction definitions help interpret Maxwell, while MPR and its bundled SDK
establish the game's binding conventions. NVIDIA `nvdisasm` 12.9.88 with
`-b SM53 -hex` supplied independent assembly comparisons. Startup DXBC was
disassembled using vkd3d 1.19's `dxbc-tpf` → `d3d-asm` conversion.
