# Third-party notices

eCord's own source code is released under the MIT License (see [`LICENSE`](LICENSE)).

eCord is an unofficial client. It is not affiliated with, endorsed by, or
sponsored by Discord Inc. "Discord" is a trademark of Discord Inc.

This file lists the main third-party components that are included in, or linked
into, eCord's released builds. It does not replace the license texts, which
travel with each component (see the paths below).

## FFmpeg

eCord decodes video and encodes screen shares with the FFmpeg libraries
(`libavcodec`, `libavformat`, `libavutil`, `libavdevice`, `libavfilter`,
`libswscale`, `libswresample`). FFmpeg is a trademark of Fabrice Bellard,
originator of the FFmpeg project. The libraries are loaded as shared libraries
(`.so` / `.dll` / `.dylib`) that sit next to or alongside eCord, never merged
into the executable.

| Release build | FFmpeg build bundled | License of that build |
| --- | --- | --- |
| Linux `.deb` | Shared libraries from [BtbN FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (`gpl-shared`), installed in `/usr/lib/ecord` | **GNU GPL** (it includes GPL components such as libx264) |
| Windows installer (`-setup.exe`) and portable `.zip` | DLLs from BtbN FFmpeg-Builds (`lgpl-shared`), installed next to `ecord.exe` | **GNU LGPL 2.1 or later** |
| macOS `.dmg` (experimental) | Libraries from Homebrew's `ffmpeg@7`, copied into the app bundle | Treat as **GNU GPL**: Homebrew builds FFmpeg with GPL components |

The exact FFmpeg license terms of each build are in the `FFmpeg-LICENSE.txt` /
`ffmpeg-LICENSE.txt` file shipped with the package (on Linux:
`/usr/share/doc/ecord/ffmpeg-LICENSE.txt`).

**What this means for the Linux and macOS packages.** Because they ship GPL
libraries, those packages are distributed under the terms of the GPL as a whole.
eCord's own source code stays available under the MIT License, which is
compatible with the GPL.

**LGPL (Windows).** The FFmpeg DLLs are separate files. You may replace them
with another build of the same FFmpeg major version.

**Source code.**

- FFmpeg: <https://ffmpeg.org/download.html> (the release matching the build
  used; the build is named by `FFMPEG_ASSET` in
  [`.github/workflows/release.yml`](.github/workflows/release.yml)).
- Build scripts and configuration of the bundled binaries:
  <https://github.com/BtbN/FFmpeg-Builds>.
- x264: <https://www.videolan.org/developers/x264.html>.
- eCord: the repository this software was downloaded from. If you cannot get
  any of the above, open an issue there and the corresponding source will be
  provided.

## ONNX Runtime and DeepFilterNet (noise suppression)

The optional DeepFilterNet noise suppression ("Clean Mic") runs on ONNX Runtime,
which `vendor/deepfilter-rt` loads at run time as a shared library
(`onnxruntime.dll` / `libonnxruntime.so`) shipped next to or alongside eCord. It
is not merged into the executable. On macOS it is `libonnxruntime.dylib` inside the app bundle (`Contents/Resources`).

| Component | License | Origin |
| --- | --- | --- |
| ONNX Runtime 1.23.2 (CPU build; on macOS the arm64 and universal2 builds) | MIT, © Microsoft Corporation | <https://github.com/microsoft/onnxruntime> (full text in `ONNXRuntime-LICENSE.txt`, shipped with the package; on Linux: `/usr/share/doc/ecord/onnxruntime-LICENSE.txt`) |
| DeepFilterNet (`deep_filter` crate, used for the STFT/ISTFT processing) | MIT or Apache-2.0, © Hendrik Schröter | <https://github.com/Rikorose/DeepFilterNet> |
| DeepFilterNet3 model (`dfn3`) | Same terms as DeepFilterNet (MIT or Apache-2.0) | Not bundled: downloaded once, on first use, from <https://github.com/shimondoodkin/deepfilter-rt> into eCord's cache folder. If it cannot be downloaded, eCord keeps using RNNoise. |

<!-- TODO: confirm the license of the DeepFilterNet3 `.onnx` export published in
     shimondoodkin/deepfilter-rt, and of `vendor/deepfilter-rt` itself (its
     Cargo.toml declares none and the folder has no LICENSE file). Add its
     LICENSE text under vendor/deepfilter-rt/ and list it in the vendor table. -->

## Embedded fonts

| Font | License | Notice |
| --- | --- | --- |
| Inter | SIL Open Font License 1.1, © 2016 The Inter Project Authors | `assets/fonts/Inter-LICENSE.txt` |
| Noto Emoji | SIL Open Font License 1.1, © 2013 Google LLC | `assets/fonts/NotoEmoji-LICENSE.txt` |

## Icons

| Set | License | Notice |
| --- | --- | --- |
| Lucide | ISC, © Lucide Icons and Contributors | `assets/icons/LICENSE.txt` |

## Sounds

<!-- TODO: say where assets/sounds/alert.mp3 comes from and under what license. -->

## Vendored source code (`vendor/`)

| Component | License | Origin |
| --- | --- | --- |
| `egui-video` (fork ported to egui 0.36) | MIT, © 2023 n00kii | <https://github.com/n00kii/egui-video> (full text in `vendor/egui-video/LICENSE`) |
| `discord_client_rest`, `discord_client_structs`, `discord_client_macros` | MIT, as declared in each crate's `Cargo.toml` | <https://github.com/UwUDev/discord-client-rs> by UwUDev |
| `discord_client_utils` | See the upstream repository | <https://github.com/UwUDev/discord-client-rs> by UwUDev |
| `deepfilter-rt` (real-time DeepFilterNet on ONNX Runtime) | See the TODO under "ONNX Runtime and DeepFilterNet" | <https://github.com/shimondoodkin/deepfilter-rt> |

<!-- TODO: add the upstream LICENSE text (with its copyright line) for the
     discord_client_* crates under vendor/, and confirm the license of
     discord_client_utils, which declares none in its Cargo.toml. -->

## Native libraries compiled into the executable

| Library | Used through | License |
| --- | --- | --- |
| BoringSSL | `btls-sys` / `wreq` | OpenSSL / ISC-style licenses ([text](https://boringssl.googlesource.com/boringssl/+/refs/heads/master/LICENSE)) |
| Opus | `opusic-sys` | BSD-3-Clause |
| Zstandard | `zstd-sys` | BSD-3-Clause (zstd is dual-licensed BSD / GPLv2; used under BSD) |
| mimalloc | `libmimalloc-sys` | MIT |
| ring | `ring` | ISC / OpenSSL-style licenses |

<!-- TODO: the audio chain also uses `nnnoiseless` (RNNoise port) and `sonora`
     (echo cancellation, AEC3). Confirm their licenses (and, for RNNoise, the
     Xiph.Org / Jean-Marc Valin BSD-3-Clause notice) and list them here. They are
     compiled into the executable. -->

## Rust crates

eCord is built from the crates listed in `Cargo.lock`, each under its own
license (mostly MIT and/or Apache-2.0). A complete list with the license texts
can be generated with [cargo-about](https://github.com/EmbarkStudios/cargo-about):

```
cargo install cargo-about --locked
cargo about init
cargo about generate about.hbs > THIRD_PARTY_CRATES.html
```

## System components (not redistributed)

These are provided by the operating system, not included in eCord's packages:
GTK 3, WebKitGTK, ALSA, PulseAudio and the X11/Wayland libraries on Linux; the
Microsoft Edge WebView2 Runtime on Windows.
