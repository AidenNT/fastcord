<div align="center">

<h1>eCord</h1>

<p><strong>🦀 An unofficial Discord desktop client, written in Rust</strong></p>

<p>Native UI with <a href="https://github.com/emilk/egui">egui</a> — no Electron, no webview (except for captcha). Custom themes, voice calls, screen sharing, embedded video and much of what you'd expect from the official client.</p>

<p>⭐ <strong>If you find this useful, please give it a star!</strong> ⭐</p>

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue.svg)](#-quick-start)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)
[![egui](https://img.shields.io/badge/egui-0.36-green.svg)](https://github.com/emilk/egui)
[![Version](https://img.shields.io/badge/version-0.0.13-41b883.svg)](#-roadmap)

</div>

> **Status:** `v0.0.13` — early development. Core features work well; polish and the remaining features are ongoing (see the [Roadmap](#-roadmap)).

---

## ⚠️ Important notice

> [!WARNING]
> **eCord is for educational purposes only.** It signs in with a **user account token** (QR code or username/password) and talks to Discord's API and Gateway the way a browser would. Discord treats this as an *unofficial client* / *self-bot* and it **violates the [Discord Terms of Service](https://discord.com/terms)**.

- There is a real risk that **Discord will limit or suspend the account** you use with it.
- The protocols involved (remote QR auth, `settings-proto`, etc.) are **not public** and may change without notice.
- Use it at your own risk, preferably with a throwaway account.

This project is not affiliated with, endorsed by, or sponsored by Discord Inc.

---

## 🚀 Quick Start

### Build from source

```bash
git clone <repo-url> ecord
cd ecord
cargo run --release
```

Always build in `--release` (or with `opt-level=3`): in debug builds, video decode, animated GIFs and the UI are noticeably slow.

Two executables are built: `ecord` (the app, what `cargo run` starts) and `ecord-captcha` (the captcha window on Linux, see [Architecture](#-architecture)).

For a lighter build **without voice audio capture/playback** (and without DeepFilterNet):

```bash
cargo build --release --no-default-features
```

In that mode the client can still join voice channels and show who is speaking, but it won't transmit or play audio.

### Requirements

| Component | Details |
|---|---|
| **Rust** | `1.88` or newer (2024 edition — uses let-chains) |
| **FFmpeg** | Development libraries **6 or 7** at build time, dynamic libraries at runtime (embedded video, stream decode and screen-share encode) |
| **libclang / LLVM** | Required by `bindgen` (FFmpeg bindings) |
| **Audio** | ALSA/PulseAudio on Linux (`voice-playback` feature) |
| **Native toolchain** | CMake and NASM on Windows (the TLS dependency uses them); Visual Studio 2022 |
| **Compositor** | Optional; without one, theme transparency renders black |
| **Captcha WebView** | Windows: WebView2 (bundled with Edge / Win10+). Linux: `libwebkit2gtk-4.1-dev` + GTK3 |

> `protoc` is **not** required: `build.rs` uses a vendored binary (`protoc-bin-vendored`).

### Linux dependencies (Debian / Ubuntu)

```bash
sudo apt install build-essential cmake nasm clang llvm \
    libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev \
    libasound2-dev libpulse-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev
```

### Windows

`build.ps1` detects Visual Studio, FFmpeg 7 (in `C:\ffmpeg7` or via vcpkg), LLVM and NASM; installs whatever is missing, builds, and copies the DLLs next to the `.exe`.

```powershell
.\build.ps1                    # release
.\build.ps1 -Profile debug     # debug
.\build.ps1 -SkipVcpkgInstall  # don't install FFmpeg via vcpkg
.\build.ps1 -Package           # produce a portable ZIP
```

> ⚠️ `build.ps1` may still reference **SDL2** (help text, vcpkg step, DLL list). SDL2 is **no longer used** — audio goes through `cpal`. The script still works; the SDL2 lines can be stripped.

### Packages

`Cargo.toml` carries `cargo-deb` metadata (a normal and a `bundled` variant) for Linux `.deb` packages. Windows installers and a macOS `.dmg` are produced by the release pipeline. Once a release is published, eCord updates itself (see [Updates](#-updates)).

### Sign in

1. **QR code** — scan it from the official mobile app (remote auth).
2. **Username / password** — with 2FA (TOTP and SMS).
3. **Saved accounts** — pick any previously used account from the account selector, or add another one.

---

## ✨ Features

### 🔐 Account & session
- **QR login** or **username/password** with 2FA (TOTP and SMS).
- **Multi-account**: saved-account picker at login and in Settings → Account, with a warning for accounts whose session expired.
- **Automatic hCaptcha handling** via a dedicated WebView window: if Discord asks for a captcha, a child window opens, waits for you to solve it, and the request is retried transparently (login, sending messages, joining servers, opening DMs, and more).
- **Gateway** with `zlib-stream` compression, `RESUME`, and reconnection with exponential backoff + jitter.
- Persistent session: cookies and browser identifiers stored on disk with private permissions (`0600` on Unix).
- Account settings sync (`PreloadedUserSettings`, protobuf) — currently only the **theme** is written back.
- **Single-instance enforcement:** launching the app a second time focuses the existing window.
- **Security notice** when running with insecure options (`ECORD_USE_MOCK_URL`, `ECORD_INSECURE_TLS`): the client won't connect until you accept it.
- **Remembers where you left off**: last channel per server and scroll position per channel.

### 💬 Messaging
- Servers, channels, categories, **threads** and **forums** (cards, search, sorting, "New post").
- **DMs**, friends panel, and full profile view.
- A custom subset of **Discord markdown**: bold, italic, underline, strikethrough, code, quotes, lists, headings, spoilers, `<t:…>` timestamps, mentions (`@user`, `@role`, `#channel`, `@everyone`), and jumbo emoji.
- **Color emoji** (Twemoji and Fluent) and custom emoji, with a notice when an emoji requires Nitro.
- **Reactions**, replies with quotes, embeds, attachments (images, audio, files — picker, drag & drop and clipboard paste), **stickers** and **polls**.
- **Animated GIFs** in avatars, emoji, reactions and attachments; GIF picker.
- **Bot components**: buttons in all five styles, **dropdown menus** (text, user, role, mentionable and channel selects), and components v2 (containers, sections, text, media galleries, attachments, dividers).
- **Slash commands**: basic support for application commands.
- **Server join form** (membership screening): rules and questions shown after joining with an invite.
- **RTL** text (Arabic/Hebrew) and system fallback fonts for CJK and other scripts.

### 🛡️ Servers & permissions
- Server rail with the **same order and folders** as the official client (`guild_folders`).
- **Local permission calculation** to show or hide channels (roles + overwrites), using a *fail-open* approach, with lock icons for private channels and voice channels you can't join.
- Popup listing the members of a role when clicking an `@role` mention.
- Access diagnostics are quiet by design: with debug logging on, the `access` log only reports **roles being assigned to you** and **channels that stop being visible to you** (see [Debug logging](#-debug-logging)).

### 👤 Profiles
- Profile popout with colors, banner, **avatar decoration**, animated **profile effects**, badges, server tag, custom status, current activity, roles, connections and mutual friends/servers.

### 🎙️ Voice, video & streams
- **Voice channels and DM calls** with a persistent call bar (mute, deafen, hang up), speaking indicators and per-user volume.
- **DAVE** (Discord's E2EE) + RTP encryption (AES-GCM / ChaCha20-Poly1305) and the **Opus** codec.
- Capture and playback through `cpal`.
- **Clean Mic**: a live-tunable microphone chain — high-pass, RNNoise, voice gate, auto-gain, EQ, compressor and limiter — preceded by echo cancellation (AEC3), plus optional **DeepFilterNet** noise suppression. DeepFilterNet runs on ONNX Runtime (`onnxruntime.dll` / `libonnxruntime.so`, shipped with every package) and downloads its model (a few MB) on first use; without it, eCord keeps using RNNoise.
- **Soundboard** — Discord sounds and user favorites (where available).
- **Watch streams** (Go Live): H.264 decode over RTP.
- **Share your screen** (Go Live output): source picker with live thumbnails (windows / full screen), quality selector (resolution + fps), H.264 over RTP with DAVE. Windows uses Windows Graphics Capture for per-window capture.
- Embedded **video player** for attachments (ffmpeg + `cpal`) and a media viewer.

### 🔔 Notifications & inbox
- Mention counter, in-app *toasts*, and **native OS notifications** when the window is unfocused.
- **Inbox** (🔔 bell): unread DMs and mentions with previews, newest first.
- Respects the account's muted servers/channels/DMs and notification levels.

### 🎨 Appearance
- Borderless window with a **custom top bar** and transparency support.
- Two UI layouts: **Classic** (full top bar with nav arrows, search and window controls) and **Modern** (slim 32px title bar, Discord-like).
- Dark/light themes, **dynamic background from your system wallpaper**, gradients/images, and custom themes with one color per role.
- Interface zoom (also `Ctrl +` / `Ctrl -`).
- [Inter](https://rsms.me/inter/) typography and [Lucide](https://lucide.dev/) icons.

### 🔄 Updates
- **In-app updater**: checks on startup, installs a new version, confirms it started correctly and restores the previous one if it didn't.
- **What's new** dialog the first time you run a new version, and the full history from Settings → Account.

### 🧰 Developer tools
- A floating, IDE-style panel (tabs, inspector, console, **network traffic**, Discord API explorer). It does not exist for regular users; enable it with `USE_DEVELOPER_FEATURES=1`, the `--use_developer_features` flag, or from the *Client → Environment* tab, then toggle with **Ctrl+Shift+D**.
- **Memory viewer** and counting allocator (`mimalloc` + RSS recovery).

---

## 🐞 Debug logging

eCord logs to a file (the log is restarted when it exceeds 5 MB) and, when launched from a terminal, to that console. The default level is `info`; for more detail:

```bash
RUST_LOG=debug cargo run --release     # Linux / macOS
$env:RUST_LOG = "debug"; .\ecord.exe   # Windows (PowerShell)
```

Targets group the output by area (`access`, `fingerprint`, `http`, …), so you can filter, e.g. `RUST_LOG=info,access=debug`.

The `access` target is intentionally low-noise. It prints only:

```text
server <id>: te asignaron rol(es): <role names>
server <id>: ya no puedes ver <N> canal(es): <channel names>
```

The first appears when a role is added to your own account in a server; the second when a channel that you could see becomes hidden (overwrite, role or permission changes). Nothing is logged for the initial load of a server.

---

## 🏗️ Architecture

```text
ecord/
├── Cargo.toml · build.rs · build.ps1
├── proto/                     # discord_user_settings.proto → prost (via build.rs)
├── src/
│   ├── main.rs · app_shell.rs # eframe window + startup, updater/tray/shell glue
│   ├── bin/ecord-captcha.rs   # captcha window as a separate executable (Linux)
│   ├── theme.rs               # palette, typography, icons, base widgets
│   ├── bidi.rs · system_fonts.rs · paths.rs · single_instance.rs
│   ├── logging.rs · logging/  # log to file/console + in-memory capture for devtools
│   ├── updater.rs · updater/  # in-app updates
│   ├── discord/               # everything that talks to Discord
│   │   ├── gateway.rs · websocket.rs      # Gateway (zlib-stream, RESUME, backoff)
│   │   ├── rest.rs · uwu_rest.rs · auth_http.rs   # REST clients (uwu = vendored, captcha-aware transport)
│   │   ├── captcha.rs · captcha_window.rs · captcha_proto.rs   # hCaptcha broker + WebView window
│   │   ├── remote_auth.rs · password_auth.rs      # QR login, password login + 2FA
│   │   ├── fingerprint.rs · launch_signature.rs   # shared "browser" identity
│   │   ├── user_settings.rs · frecency.rs · affinities.rs · slash.rs
│   │   ├── invites.rs · join_form.rs · uploads.rs · activity.rs · soundboard.rs
│   │   ├── clean_mic.rs                # live Clean Mic parameters
│   │   └── voice/                      # voice gateway, RTP, DAVE, Opus, mic, playback,
│   │                                   # noise chain (RNNoise/AEC3/DeepFilterNet),
│   │                                   # H.264 stream decode + screen capture/publish
│   ├── lib/                   # state and UI-free logic
│   │   ├── state.rs · state/  # App: global state, event handling, toasts, modals
│   │   └── data.rs · permissions.rs · guild_order.rs · notifications.rs
│   │       accounts.rs · last_view.rs · win_toast.rs · win_shortcut.rs
│   ├── ui/                    # egui screens and components
│   │   ├── auth/      login · welcome · accounts · security_warning
│   │   ├── shell/     topbar · nav · rail · status_bar · overlay · dialog · components · notifications
│   │   ├── chat/      markdown · emoji · attachments · media · media_viewer · video_player · share_picker · slash
│   │   ├── server/    channels · forum · role_popup · invite_card · permissions_calc
│   │   ├── dm/        friends_panel · inbox
│   │   ├── call/      call_bar · call_view · stream_view · soundboard · audio_menu · clean_mic_popup
│   │   ├── settings/  account_settings · updates · changelog · memory_view
│   │   ├── profile_popup/ · devtools/
│   │   └── home.rs
│   └── support/               # HTTP cache, audio output, sound alerts, private files, memory report, net trace
└── vendor/
    ├── egui-video/            # fork ported to egui 0.36 (ffmpeg + cpal)
    ├── deepfilter-rt/         # DeepFilterNet runtime
    └── discord_client_{rest,structs,macros,utils}/   # by UwUDev, MIT
```

---

## 🩺 Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| **Window is black where the theme should be transparent** | No running compositor (Linux). Launch with one (Picom, Mutter, KWin, etc.) or use an opaque theme. |
| **Captcha window never opens on Linux** | Missing `libwebkit2gtk-4.1-dev` / GTK3 deps (or the `ecord-captcha` binary isn't next to `ecord`). Install them and rebuild. |
| **Build fails with `ffmpeg-the-third` / bindgen errors** | FFmpeg dev headers (6 or 7) not installed, or `libclang`/LLVM missing. Check [Requirements](#requirements). |
| **Voice joins but no audio** | Built without `voice-playback`, or missing ALSA/PulseAudio on Linux. Build with default features. |
| **Login fails with "account suspended"** | The account was flagged by Discord — resolve it via Discord's Safety Hub in a browser. See `SUSPENDED_ACCOUNT_ERROR` in `password_auth.rs`. |
| **Client won't connect and shows a security notice** | `ECORD_USE_MOCK_URL` or `ECORD_INSECURE_TLS` is set; unset it, or accept the notice if you really mean it. |
| **High memory usage with many servers** | eCord uses `mimalloc` and runs periodic `mi_collect` passes, but a large `READY` payload still spikes. It should settle; otherwise use the Memory viewer in Settings. |

---

## 🗺️ Roadmap

> A living document; items may be re-prioritized.

### ✅ Done — `v0.0.x`
- [x] QR and username/password login (TOTP/SMS), multi-account
- [x] Gateway with `RESUME` and backoff reconnection
- [x] Servers, channels, threads, forums, DMs and friends
- [x] Markdown, color emoji, animated GIFs, stickers, attachments, embeds, polls
- [x] Slash commands (basic) and bot buttons (all styles + components v2)
- [x] **Bot dropdown menus** (string, user, role, mentionable, channel)
- [x] Reactions and full profiles (decorations, effects, badges, activity, connections, mutuals)
- [x] Server folders and ordering, local channel permissions, **server join form**
- [x] Voice with DAVE, Opus, **Clean Mic**, AEC3 and DeepFilterNet
- [x] Soundboard, embedded video player, media viewer
- [x] **Watch streams** and **share your screen** (Go Live in both directions)
- [x] Notifications (toasts + desktop native) and Inbox
- [x] Themes, wallpaper-based dynamic background, disk cache
- [x] General captcha handling (hCaptcha WebView, automatic retries)
- [x] Single-instance enforcement and the Classic / Modern UI modes
- [x] **In-app updater**, **what's new** dialog and release changelog
- [x] **Developer tools** (network traffic, console, API explorer) and memory reporting
- [x] **Resume where you left off** (last channel and scroll position)
- [x] Quiet `access` diagnostics (only role assignments and lost channel visibility)
- [x] `println!("Hello, world!")` removed from `main.rs`

### 🚧 Next — `v0.1` · Finish what's half-done
- [ ] **Choosing a target channel or DM** for actions that ask for one (still shows "not supported yet")
- [ ] **Visible send errors**: a failed message or action should show a notice + retry instead of being dropped
- [ ] **More 2FA methods** (security keys / WebAuthn — beyond TOTP and SMS)
- [ ] **Camera** and call **chat** buttons in the call bar (currently disabled)
- [ ] Make the **top search bar** global (today it only filters the friends panel)
- [ ] **Lottie / animated sticker playback** (stickers render, but Lottie and APNG fall back to static)
- [ ] Dedicated palettes for the dark themes that currently fall back to `Dark`
- [ ] Missing Lucide icons (GIF, sticker, inbox, shop) — still placeholders
- [ ] Update `build.ps1` to remove **SDL2** references

### 🔧 Quality & maintenance — `v0.2`
- [ ] Merge `rest.rs` and `uwu_rest.rs` into a single REST client
- [ ] Automated tests (`password_auth` has some; `permissions`, `guild_order` and `markdown` need them)
- [ ] CI with GitHub Actions (build + `clippy` + `fmt`) for Linux, Windows and macOS
- [ ] Consistent error handling and logging across `discord::*`
- [ ] Review memory usage of the GIF cache and message history (LRU / eviction)
- [ ] Verify the Linux/macOS dependency list on real distros

### ✨ Features — `v0.3`
- [ ] **Editable Account tab** (`PATCH` to `settings-proto` beyond the theme)
- [ ] Message search (per-channel and global)
- [ ] Internationalization (the UI is currently Spanish-only)
- [ ] Keyboard shortcuts and quick switcher
- [ ] Camera *output* in calls
- [ ] Inbox improvements (full message list, filters, mass-read)

### 📦 Distribution — `v1.0`
- [ ] Polished packages: installer/portable ZIP (Windows), `.deb` / AppImage / Flatpak (Linux), `.app` (macOS)
- [ ] User documentation and a contributing guide
- [ ] Website or releases page with screenshots and demo video

---

## 📸 Screenshots

> *Screenshots coming soon — run the app and snap some if you'd like to contribute!*

---

## 🙏 Credits

- [**UwUDev/discord-client-rs**](https://github.com/UwUDev/discord-client-rs) — vendored REST transport and structs (`vendor/discord_client_*`, MIT).
- **egui-video** — video player, ported here to egui 0.36 (`vendor/egui-video`, MIT).
- **Concord** — source of the ported Gateway, password login, voice code and fingerprint v1.
- **Fastpotify** — origin of `theme.rs` and the classic top bar style.
- [Twemoji](https://github.com/twitter/twemoji), Fluent Emoji, [Inter](https://rsms.me/inter/) and [Lucide](https://lucide.dev/).

## 🤝 Contributing

1. Fork the repo and create a branch (`git checkout -b feature/my-change`).
2. Build in `--release` and run `cargo fmt` and `cargo clippy`.
3. Open a pull request describing what changed and how you tested it.

## 📄 License

Released under the [MIT License](LICENSE). Components under `vendor/` keep their own licenses.

Third-party components (FFmpeg, fonts, icons, vendored code and more) are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). **The Linux `.deb` and the macOS `.dmg` bundle GPL-licensed FFmpeg libraries**, so those packages are distributed under the terms of the GPL; the Windows installers use an LGPL build of FFmpeg. eCord is an unofficial client and is not affiliated with Discord Inc.
