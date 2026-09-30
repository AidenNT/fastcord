# eCord

An **unofficial** Discord desktop client written in Rust with [egui](https://github.com/emilk/egui). Native UI — no Electron, no webview — with a custom borderless window, customizable themes, voice calls, embedded video, and much of what you'd expect from the official client.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Status:** `v0.0.1` — early development. It works, but some pieces are unfinished (see the [Roadmap](#roadmap)).

---

## ⚠️ Important notice

eCord signs in with a **user account token** (via QR code or username/password) and talks to Discord's API and Gateway the way a browser would. Discord treats this as an *unofficial client* / *self-bot* and it **violates the [Discord Terms of Service](https://discord.com/terms)**.

- There is a real risk that **Discord will limit or suspend the account** you use with it.
- The protocols involved (remote QR auth, `settings-proto`, etc.) are **not public** and may change without notice.
- Use it at your own risk, preferably with a throwaway account. This project is experimental and intended for educational purposes.

This project is not affiliated with, endorsed by, or sponsored by Discord Inc.

---

## Features

### Account & session
- Sign in with a **QR code** (remote auth) or **username/password** with 2FA (TOTP and SMS).
- **Gateway** connection with `zlib-stream` compression, `RESUME`, and reconnection with exponential backoff + jitter.
- Persistent session: cookies and browser identifiers are stored on disk with private permissions (`0600` on Unix).
- Account settings sync (`PreloadedUserSettings`, protobuf) — currently only the **theme** is written back.

### Messaging
- Servers, channels, categories, **threads**, and **forums** (cards, search, sorting, and "New post").
- **DMs**, friends panel, and full profile view.
- A custom subset of **Discord markdown**: bold, italic, underline, strikethrough, code, quotes, lists, headings, spoilers, `<t:…>` timestamps, mentions (`@user`, `@role`, `#channel`, `@everyone`), and jumbo emoji.
- **Color emoji** (Twemoji and Fluent) and custom emoji, with a notice when an emoji requires Nitro.
- **Reactions**, replies with quotes, embeds, attachments (images, audio, files), and **stickers**.
- **Animated GIFs** in avatars, emoji, reactions, and attachments.
- **Bot components**: buttons in all five styles and basic components v2.
- **RTL** text support (Arabic/Hebrew) and system fallback fonts for CJK and other scripts.

### Servers & permissions
- Server rail with the **same order and folders** as the official client (`guild_folders`).
- **Local permission calculation** to show or hide channels (roles + overwrites), using a *fail-open* approach.
- Popup listing members of a role when clicking an `@role` mention.

### Profiles
- Profile popout with profile colors, banner, **avatar decoration**, animated **profile effects**, badges, server tag, custom status, current activity, roles, connections, and mutual friends/servers.

### Voice & video
- **Voice channels and DM calls** with a persistent call bar (mute, deafen, hang up).
- **DAVE** (Discord's E2EE) + RTP encryption (AES-GCM / ChaCha20-Poly1305) and the **Opus** codec.
- Capture and playback through `cpal`, plus **noise suppression** (`nnnoiseless`).
- Embedded **video player** for attachments (ffmpeg + `cpal`).

### Notifications
- Mention counter, in-app *toasts*, and **native OS notifications** when the window is unfocused.
- Respects the account's muted servers/channels/DMs and notification levels.

### Appearance
- Borderless window with a **custom top bar** (drag, minimize, maximize, close) and transparency support.
- Dark/light theme, **dynamic background based on the system wallpaper**, and custom themes with one color per role.
- [Inter](https://rsms.me/inter/) typography and [Lucide](https://lucide.dev/) icons.

---

## Requirements

| Component | Details |
|---|---|
| **Rust** | `1.88` or newer |
| **FFmpeg** | Development libraries **6 or 7** at build time and dynamic libraries at runtime (embedded video) |
| **libclang / LLVM** | Required by `bindgen` |
| **Audio** | ALSA/PulseAudio on Linux (`voice-playback` feature) |
| **Native toolchain** | CMake and NASM on Windows (the TLS dependency uses them); Visual Studio 2022 |
| **Compositor** | Optional; without one, theme transparency renders black |

> `protoc` is **not** required: `build.rs` uses a vendored binary (`protoc-bin-vendored`).

---

## Building & running

### Linux / macOS

```bash
git clone <repo-url> ecord
cd ecord
cargo run --release
```

Always build in `--release` (or with `opt-level=3`): in debug builds, video and the UI are noticeably slow.

For a lighter build **without voice audio capture/playback**:

```bash
cargo build --release --no-default-features
```

In that mode the client can still join voice channels and show who is speaking, but it won't transmit or play audio.

### Windows

`build.ps1` detects Visual Studio, FFmpeg 7 (in `C:\ffmpeg7` or via vcpkg), LLVM, and NASM; installs whatever is missing, builds, and copies the DLLs next to the `.exe`.

```powershell
.\build.ps1                    # release
.\build.ps1 -Profile debug     # debug
.\build.ps1 -SkipVcpkgInstall  # don't install FFmpeg via vcpkg
.\build.ps1 -Package           # produce a portable ZIP
```
### Project layout

```
ecord/
├── Cargo.toml · build.rs · build.ps1
├── proto/                     # discord_user_settings.proto → prost (via build.rs)
├── src/
│   ├── main.rs                # eframe window + startup
│   ├── theme.rs               # palette, typography, icons, base widgets
│   ├── bidi.rs                # RTL text support
│   ├── system_fonts.rs        # system fallback fonts
│   ├── paths.rs · logging.rs
│   ├── discord/               # everything that talks to Discord
│   │   ├── gateway.rs         # Gateway (zlib-stream, RESUME, backoff)
│   │   ├── rest.rs            # REST client built on reqwest
│   │   ├── uwu_rest.rs        # REST client on the vendored transport
│   │   ├── remote_auth.rs     # QR login
│   │   ├── password_auth.rs   # username/password login + 2FA
│   │   ├── fingerprint.rs     # shared "browser" identity (REST + Gateway)
│   │   ├── user_settings.rs   # PreloadedUserSettings (protobuf)
│   │   ├── models.rs · ids.rs
│   │   └── voice/             # voice gateway, RTP, DAVE, Opus, microphone, playback
│   ├── lib/                   # state and UI-free logic
│   │   ├── state.rs           # App: global state and event handling
│   │   ├── data.rs · permissions.rs · guild_order.rs · notifications.rs
│   ├── ui/                    # egui screens and components
│   │   ├── login · home · dm · server · chat · forum · friends_panel
│   │   ├── rail · nav · topbar · call_bar · settings · account_settings
│   │   ├── markdown · emoji · media · anim · video_player · components
│   │   └── profile_popup · role_popup · notifications · overlay
│   └── support/               # HTTP cache, audio, private files
└── vendor/
    ├── egui-video/            # fork ported to egui 0.36 (ffmpeg + cpal)
    └── discord_client_{rest,structs,macros,utils}/   # by UwUDev, MIT
```

## Roadmap

> A proposal based on the current state of the code (comments, `TODO`s, and features marked as pending). Adjust it to your priorities.

### ✅ Done — `v0.0.x`
- [x] QR and username/password login (TOTP/SMS)
- [x] Gateway with `RESUME` and backoff reconnection
- [x] Servers, channels, threads, forums, DMs, and friends
- [x] Markdown, color emoji, animated GIFs, stickers, attachments, and embeds
- [x] Bot buttons and reactions
- [x] Full profiles (decorations, effects, badges, activity)
- [x] Server folders and ordering, local channel permissions
- [x] Voice: channels and DM calls with DAVE, Opus, and noise suppression
- [x] Embedded video
- [x] Notifications (in-app and desktop)
- [x] Themes, wallpaper-based dynamic background, disk cache
- [x] MIT license

### 🚧 Next — `v0.1` · Finish what's half-done
- [ ] **Bot dropdown menus** (currently rendered disabled)
- [ ] **Choosing a target channel or DM** for actions that ask for one (currently shows "not supported yet")
- [ ] **Visible send errors**: a failed message or action is currently dropped silently → show a notice + retry
- [ ] **Password login with captcha**: currently you must fall back to QR
- [ ] **More 2FA methods** (beyond TOTP and SMS)
- [ ] **Dedicated palettes** for the two dark themes that currently fall back to `Dark`
- [ ] Missing Lucide icons (GIF, sticker, inbox, shop) — currently placeholders
- [ ] Cleanup in `main.rs` (remove the `println!("Hello, world!")`)

### 🔧 Quality & maintenance — `v0.2`
- [ ] Merge `rest.rs` and `uwu_rest.rs` into a single REST client
- [ ] Update `build.ps1` (it still mentions SDL2, which is no longer used) and document Linux/macOS dependencies
- [ ] Automated tests (`password_auth` has some; `permissions`, `guild_order`, and `markdown` need them)
- [ ] CI with GitHub Actions (build + `clippy` + `fmt`) for Linux, Windows, and macOS
- [ ] Consistent error handling and logging across `discord::*`
- [ ] Review memory usage of the GIF cache and message history
- [ ] Verify license compatibility of everything under `vendor/`

### ✨ Features — `v0.3`
- [ ] **Editable Account tab** (`PATCH` to `settings-proto` beyond the theme)
- [ ] Message search
- [ ] Internationalization (the UI is currently Spanish-only)
- [ ] Keyboard shortcuts and quick switcher
- [ ] Screen sharing / camera in calls
- [ ] Lottie sticker playback
- [ ] Mentions inbox

### 📦 Distribution — `v1.0`
- [ ] Packages: installer/portable ZIP (Windows), AppImage/Flatpak (Linux), `.app` (macOS)
- [ ] Updates and release notes
- [ ] User documentation and contributing guide
- [ ] Website or releases page

---

## Credits

- [**UwUDev/discord-client-rs**](https://github.com/UwUDev/discord-client-rs) — vendored REST transport and structs (`vendor/discord_client_*`, MIT).
- **egui-video** — video player, ported here to egui 0.36 (`vendor/egui-video`, MIT).
- **Concord** — source of the ported Gateway, password login, voice code and fingerprint v1.
- **Fastpotify** — origin of `theme.rs` and the top bar style.
- [Twemoji](https://github.com/twitter/twemoji), Fluent Emoji, [Inter](https://rsms.me/inter/), and [Lucide](https://lucide.dev/).

## Contributing

1. Fork the repo and create a branch (`git checkout -b feature/my-change`).
2. Build in `--release` and run `cargo fmt` and `cargo clippy`.
3. Open a pull request describing what changed and how you tested it.

## License

Released under the [MIT License](LICENSE). Components under `vendor/` keep their own licenses (also MIT).