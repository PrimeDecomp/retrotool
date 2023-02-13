# retrotool [![Build Status]][actions]

[Build Status]: https://github.com/encounter/retrotool/actions/workflows/build.yml/badge.svg
[actions]: https://github.com/encounter/retrotool/actions

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
