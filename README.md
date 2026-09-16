# retrotool [![Build Status]][actions]

[Build Status]: https://github.com/PrimeDecomp/retrotool/actions/workflows/build.yml/badge.svg
[actions]: https://github.com/PrimeDecomp/retrotool/actions

> **Warning**
> Under active development, not guaranteed to be useful or even function.

Tools for working with Retro game formats. Currently only supports *Metroid Prime Remastered*.

## Commands

### pak extract

Extracts files from a given `.pak`.

```shell
$ retrotool pak extract [in_pak] [out_dir]
```

### pak package

Re-packages a `.pak`, given an extracted directory.

```shell
$ retrotool pak package [in_dir] [out_pak]
```

### txtr convert

Converts a `.TXTR` file to `.dds` (recommended) or `.astc`.

Textures are often compressed with BCn or ASTC, which are not commonly supported by image viewers.  
[tacentview](https://github.com/bluescan/tacentview) is recommended to view and convert the resulting textures.

```shell
$ retrotool txtr convert [in].TXTR
# writes to [in].dds

$ retrotool txtr convert --astc [in].TXTR
# writes to [in].astc
```

### fmv0 extract

Extracts the contained video from a given `FMV0` file.

```shell
$ retrotool fmv0 extract [in_fmv0] [out_mp4]
```

### fmv0 replace

Replaces the video within the given `FMV0` file.

```shell
$ retrotool fmv0 replace [inout_fmv0] [in_mp4]
```

### mtrl extract

Extracts shaders, material metadata, and a JSON index from `MaterialArchive.arc`
or a cooked `.MTRL` extracted with retrotool. Add `--wgsl` to attempt experimental
NVN shader translation.

```shell
$ retrotool mtrl extract MaterialArchive.arc shaders
$ retrotool mtrl extract --wgsl material.MTRL material-shaders
```

### refl

Prints a reflection probe's texture UUID and intensity as JSON.

```shell
$ retrotool refl probe.REFL
```

## Viewer

`retrotool-gui` browses PAK assets and previews models. **View > Shader preview**
toggles translated original shaders and controls UV animation playback.
Unsupported materials show a fallback reason. Lighting is still a preview.

Reflection materials need the game's BRDF table, selected in the same menu or
with `--brdf-lut`; the viewer remembers successful loads.

## Format documentation

See [docs/](docs/README.md) for material selection, shader bindings, lighting,
and scene relationships. Field layouts are maintained in
[PrimeRemasterStructs](https://github.com/PrimeDecomp/PrimeRemasterStructs).

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
