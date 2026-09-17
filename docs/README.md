# Metroid Prime Remastered formats

These notes explain how model, material and scene data combine in MPR.
Exact layouts belong in the
[010 templates](https://github.com/PrimeDecomp/PrimeRemasterStructs) and
[parsers](../lib/src/format/). MP4 and original Prime compatibility is unverified.

| Resource or data | Role |
| --- | --- |
| CMDL / SMDL | Geometry, mesh state, material values and model data sources |
| MCON | Model instances and their placement |
| Cooked MTRL | Feature definitions, runtime permutation lookup and compiled programs |
| TXTR / REFL | Texture storage and reflection-probe resources |
| ROOM and its PAK metadata | Scene definitions, loading relationships and area lighting inputs |
| BLIT / LTPB | Baked-lighting references, atlas lookups and tiled probe data |

A material cannot generally be rendered from its textures alone. Its feature
graph selects a program using both model values and scene state; the selected
program's binders then assemble its inputs. ROOM definitions also need loading
and activation rules before they describe a live scene.

- [Materials and permutations](materials.md): identities, providers, feature selection and draw state.
- [Shader programs and bindings](shaders.md): NVN/DX containers, hardware bindings and translation limits.
- [Textures and surface interpretation](textures.md): views, samplers and recovered channel meanings.
- [Lighting](lighting.md): runtime lights, baked probes, atlas data and reflections.
- [Scene construction](scene.md): layers, load units, generated objects and script links.
- [Animation](animation.md): UV evaluation, skinning palettes and changing scene colors.
- [Research references](references.md): executable identity, entry points and external ISA references.

Renderer maintenance: [graphics compatibility workarounds](rendering-compatibility.md)
records version-specific constraints and their removal conditions.
