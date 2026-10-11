# eCord

An **unofficial** Discord desktop client written in Rust with [egui](https://github.com/emilk/egui). Native UI — no Electron, no webview (except for captcha) — with a custom borderless window, customizable themes, voice calls, embedded video, and much of what you'd expect from the official client.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Status:** `v0.0.2` — early development. Core features work well; polish and remaining features are ongoing (see the [Roadmap](#roadmap)).

---

## ⚠️ Important notice

eCord signs in with a **user account token** (via QR code or username/password) and talks to Discord's API and Gateway the way a browser would. Discord treats this as an *unofficial client* / *self-bot* and it **violates the [Discord Terms of Service](https://discord.com/terms)**.

- There is a real risk that **Discord will limit or suspend the account** you use with it.
- The protocols involved (remote QR auth, `settings-proto`, etc.) are **not public** and may change without notice.
- Use it at your own risk, preferably with a throwaway account. This project is experimental and intended for educational purposes.

This project is not affiliated with, endorsed by, or sponsored by Discord Inc.

---

## Screenshots

> *Screenshots coming soon — run the app and snap some if you'd like to contribute!*

In the meantime, here's what the UI includes:
- **Classic mode:** custom top bar + server rail, inspired by Fastpotify.
- **Modern mode:** redesigned, slim title bar for a more Discord-native feel.
- **Dynamic wallpaper background:** the accent color is sampled from your system wallpaper and used to tint the theme backdrop.
- **Profiles, chat, forums, threads, voice calls, stream viewing, and embedded video** all render natively with egui.

---

## Features

### Account & session
- Sign in with a **QR code** (remote auth) or **username/password** with 2FA (TOTP and SMS).
- **Automatic hCaptcha handling** via a dedicated WebView window. If Discord rejects a request with a captcha challenge, the app opens a child window, waits for you to solve it, and retries transparently — works for login, sending messages, joining servers, opening DMs, and more.
- **Gateway** connection with `zlib-stream` compression, `RESUME`, and reconnection with exponential backoff + jitter.
- Persistent session: cookies and browser identifiers are stored on disk with private permissions (`0600` on Unix).
- Account settings sync (`PreloadedUserSettings`, protobuf) — currently only the **theme** is written back.
- **Single-instance enforcement:** launching the app a second time just focuses the existing window.

### Messaging
- Servers, channels, categories, **threads**, and **forums** (cards, search, sorting, and "New post").
- **DMs**, friends panel, and full profile view.
- A custom subset of **Discord markdown**: bold, italic, underline, strikethrough, code, quotes, lists, headings, spoilers, `<t:…>` timestamps, mentions (`@user`, `@role`, `#channel`, `@everyone`), and jumbo emoji.
- **Color emoji** (Twemoji and Fluent) and custom emoji, with a notice when an emoji requires Nitro.
- **Reactions**, replies with quotes, embeds, attachments (images, audio, files), **stickers**, and **polls**.
- **Animated GIFs** in avatars, emoji, reactions, and attachments.
- **Bot components**: buttons in all five styles, plus components v2 (containers, sections, text, media galleries, attachments, dividers). Dropdown menus (selects) are rendered as disabled placeholders.
- **Slash commands**: basic support for application commands.
- **RTL** text support (Arabic/Hebrew) and system fallback fonts for CJK and other scripts.

### Servers & permissions
- Server rail with the **same order and folders** as the official client (`guild_folders`).
- **Local permission calculation** to show or hide channels (roles + overwrites), using a *fail-open* approach.
- Popup listing members of a role when clicking an `@role` mention.

### Profiles
- Profile popout with profile colors, banner, **avatar decoration**, animated **profile effects**, badges, server tag, custom status, current activity, roles, connections, and mutual friends/servers.

### Voice, video & streams
- **Voice channels and DM calls** with a persistent call bar (mute, deafen, hang up), speaking indicators, and per-user volume controls.
- **DAVE** (Discord's E2EE) + RTP encryption (AES-GCM / ChaCha20-Poly1305) and the **Opus** codec.
- Capture and playback through `cpal`, plus **noise suppression** (`nnnoiseless`).
- **Soundboard** — Discord soundboard sounds and user favorites (where available).
- Embedded **video player** for attachments (ffmpeg + `cpal`).
- **View streams** (Go Live): H.264 video decode over RTP so you can watch other users' screen shares.

### Notifications & inbox
- Mention counter, in-app *toasts*, and **native OS notifications** when the window is unfocused.
- **Inbox / notification panel** (🔔 bell in the top bar): lists unread DMs and mentions with message previews, sorted newest-first.
- Respects the account's muted servers/channels/DMs and notification levels.

### Appearance
- Borderless window with a **custom top bar** (drag, minimize, maximize, close) and transparency support.
- Two UI layouts:
  - **Classic mode** — full top bar with nav arrows, search field, and window controls.
  - **Modern mode** — slim 32px title bar closer to the current Discord look.
- Dark/light theme, **dynamic background based on the system wallpaper**, and custom themes with one color per role.
- [Inter](https://rsms.me/inter/) typography and [Lucide](https://lucide.dev/) icons.

---

## Requirements

| Component | Details |
|---|---|
| **Rust** | `1.88` or newer (2024 edition — uses let-chains) |
| **FFmpeg** | Development libraries **6 or 7** at build time and dynamic libraries at runtime (embedded video and stream decode) |
| **libclang / LLVM** | Required by `bindgen` (FFmpeg bindings) |
| **Audio** | ALSA/PulseAudio on Linux (`voice-playback` feature) |
| **Native toolchain** | CMake and NASM on Windows (the TLS dependency uses them); Visual Studio 2022 |
| **Compositor** | Optional; without one, theme transparency renders black |
| **Captcha WebView** | Windows: WebView2 (bundled with Edge / Win10+). Linux: `libwebkit2gtk-4.1-dev` + GTK3. |

> `protoc` is **not** required: `build.rs` uses a vendored binary (`protoc-bin-vendored`).

---

## Building & running

### Linux / macOS

```bash
git clone <repo-url> ecord
cd ecord
cargo run --release
```

Always build in `--release` (or with `opt-level=3`): in debug builds, video decode, animated GIFs, and the UI are noticeably slow.

For a lighter build **without voice audio capture/playback**:

```bash
cargo build --release --no-default-features
```

In that mode the client can still join voice channels and show who is speaking, but it won't transmit or play audio.

#### Linux dependency notes (Debian / Ubuntu)

```bash
# Build essentials + FFmpeg + bindgen + WebKit for captcha
sudo apt install build-essential cmake nasm clang llvm \
    libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev \
    libasound2-dev libpulse-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev
```

### Windows

`build.ps1` detects Visual Studio, FFmpeg 7 (in `C:\ffmpeg7` or via vcpkg), LLVM, and NASM; installs whatever is missing, builds, and copies the DLLs next to the `.exe`.

```powershell
.\build.ps1                    # release
.\build.ps1 -Profile debug     # debug
.\build.ps1 -SkipVcpkgInstall  # don't install FFmpeg via vcpkg
.\build.ps1 -Package           # produce a portable ZIP
```

> ⚠️ `build.ps1` still references **SDL2** in its help text, vcpkg install step, and DLL copy list. SDL2 is **no longer used** by eCord (audio goes through `cpal`). The script will still work, but if you're maintaining it you can strip the SDL2 lines.

---

### Project layout

```
ecord/
├── Cargo.toml · build.rs · build.ps1
├── proto/                     # discord_user_settings.proto → prost (via build.rs)
├── src/
│   ├── main.rs                # eframe window + startup; also spawns captcha child window
│   ├── theme.rs               # palette, typography, icons, base widgets, classic+modern UI
│   ├── bidi.rs                # RTL text support
│   ├── system_fonts.rs        # system fallback fonts
│   ├── paths.rs · logging.rs · single_instance.rs
│   ├── discord/               # everything that talks to Discord
│   │   ├── gateway.rs         # Gateway (zlib-stream, RESUME, backoff)
│   │   ├── rest.rs            # REST client built on reqwest
│   │   ├── uwu_rest.rs        # REST client on the vendored transport (captcha-aware)
│   │   ├── captcha.rs         # hCaptcha broker: queues, WebView window, retry
│   │   ├── remote_auth.rs     # QR login
│   │   ├── password_auth.rs   # username/password login + 2FA (TOTP/SMS)
│   │   ├── fingerprint.rs     # shared "browser" identity (REST + Gateway)
│   │   ├── user_settings.rs   # PreloadedUserSettings (protobuf)
│   │   ├── slash.rs           # application/slash command support
│   │   ├── frecency.rs        # emoji frecency + soundboard favorites
│   │   ├── models.rs · ids.rs · invites.rs
│   │   └── voice/             # voice gateway, RTP, DAVE, Opus, mic, playback, H.264 stream decode
│   ├── lib/                   # state and UI-free logic
│   │   ├── state.rs           # App: global state, event handling, toasts, modals
│   │   ├── data.rs · permissions.rs · guild_order.rs · notifications.rs · accounts.rs
│   ├── ui/                    # egui screens and components
│   │   ├── login · home · dm · server · chat · forum · friends_panel
│   │   ├── rail · nav · topbar · call_bar · call_view · stream_view
│   │   ├── settings · account_settings · security_warning · changelog
│   │   ├── markdown · emoji · emoji_catalog · media · anim · video_player · components
│   │   ├── profile_popup · role_popup · notifications · inbox · overlay · slash
│   │   └── compose_menus · audio_menu · memory_view · dialog · extra · invite_card · gif_thumbs
│   └── support/               # HTTP cache, audio output, sound alerts, private files, memory report
└── vendor/
    ├── egui-video/            # fork ported to egui 0.36 (ffmpeg + cpal)
    └── discord_client_{rest,structs,macros,utils}/   # by UwUDev, MIT
```

---

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| **Window is black where the theme should be transparent** | No running compositor (Linux). Launch with one (Picom, Mutter, KWin, etc.) or use an opaque theme. |
| **Captcha window never opens on Linux** | Missing `libwebkit2gtk-4.1-dev` / GTK3 deps. Install them and rebuild. |
| **Build fails with `ffmpeg-the-third` / bindgen errors** | FFmpeg dev headers (6 or 7) not installed, or `libclang`/LLVM missing. Check the [Requirements](#requirements) table. |
| **Voice joins but no audio** | Built without `voice-playback` feature, or missing ALSA/PulseAudio on Linux. Try `cargo build --release` (default features). |
| **Login fails with "account suspended"** | The account was flagged by Discord — resolve via Discord's Safety Hub in a browser. See `SUSPENDED_ACCOUNT_ERROR` in `password_auth.rs`. |
| **High memory usage with many servers** | eCord uses `mimalloc` and runs periodic `mi_collect` passes, but a large `READY` payload still spikes. It should settle; if not, the Memory Viewer (`ui::memory_view`) is available in settings. |

---

## Roadmap

> This roadmap is a living document and items may be re-prioritized.

### ✅ Done — `v0.0.x`
- [x] QR and username/password login (TOTP/SMS)
- [x] Gateway with `RESUME` and backoff reconnection
- [x] Servers, channels, threads, forums, DMs, and friends
- [x] Markdown, color emoji, animated GIFs, stickers, attachments, and embeds
- [x] **Polls** in chat
- [x] **Slash commands** (application commands, basic support)
- [x] **Bot buttons** (all 5 styles + components v2: containers, sections, galleries, dividers)
- [x] Reactions
- [x] Full profiles (decorations, effects, badges, activity, connections, mutuals)
- [x] Server folders and ordering, local channel permissions
- [x] Voice: channels and DM calls with DAVE, Opus, noise suppression, speaking indicators
- [x] **Soundboard** favorites and sounds
- [x] Embedded video player
- [x] **View streams / Go Live** (H.264 RTP decode)
- [x] Notifications (in-app toasts + desktop native)
- [x] **Inbox / notification bell panel** with DM + mention list
- [x] Themes, wallpaper-based dynamic background, disk cache
- [x] **General captcha handling** (hCaptcha WebView window, automatic retries)
- [x] **Single-instance enforcement** + **modern UI mode** (slim title bar)
- [x] Memory reporting / counting allocator (mimalloc + RSS recovery)
- [x] **Two UI modes**: classic and modern title bar

### 🚧 Next — `v0.1` · Finish what's half-done
- [ ] **Bot dropdown menus** (types 3, 5, 6, 7, 8) — currently rendered as a disabled placeholder
- [ ] **Choosing a target channel or DM** for actions that ask for one (currently shows "not supported yet")
- [ ] **Visible send errors**: a failed message or action is currently dropped silently → show a notice + retry
- [ ] **More 2FA methods** (security keys / WebAuthn — beyond TOTP and SMS)
- [ ] **Dedicated palettes** for the two dark themes that currently fall back to `Dark`
- [ ] Missing Lucide icons (GIF, sticker, inbox, shop) — currently placeholders
- [ ] Cleanup in `main.rs` (remove the `println!("Hello, world!")`)
- [ ] Make the **search bar** functional (currently a text field that does nothing)
- [ ] **Lottie / animated sticker playback** (stickers render, but animated ones fall back to static)
- [ ] Update `build.ps1` to remove **SDL2** references (no longer used; left over from earlier audio stack)

### 🔧 Quality & maintenance — `v0.2`
- [ ] Merge `rest.rs` and `uwu_rest.rs` into a single REST client
- [ ] Document Linux/macOS dependencies clearly in the README (done above, verify with real distros)
- [ ] Automated tests (`password_auth` has some; `permissions`, `guild_order`, and `markdown` need them)
- [ ] CI with GitHub Actions (build + `clippy` + `fmt`) for Linux, Windows, and macOS
- [ ] Consistent error handling and logging across `discord::*`
- [ ] Review memory usage of the GIF cache and message history (LRU / eviction)
- [ ] Add a proper **changelog** mechanism and tie the in-app `changelog.rs` screen to releases

### ✨ Features — `v0.3`
- [ ] **Editable Account tab** (`PATCH` to `settings-proto` beyond the theme)
- [ ] Message search (per-channel and global)
- [ ] Internationalization (the UI is currently Spanish-only)
- [ ] Keyboard shortcuts and quick switcher
- [ ] Screen sharing / camera *output* in calls (we can decode/view, but not yet broadcast)
- [ ] Mentions inbox improvements (full message list, filters, mass-read)

### 📦 Distribution — `v1.0`
- [ ] Packages: installer/portable ZIP (Windows), AppImage/Flatpak (Linux), `.app` (macOS)
- [ ] In-app updater + release notes
- [ ] User documentation and contributing guide
- [ ] Website or releases page with screenshots and demo video

---

## Credits

- [**UwUDev/discord-client-rs**](https://github.com/UwUDev/discord-client-rs) — vendored REST transport and structs (`vendor/discord_client_*`, MIT).
- **egui-video** — video player, ported here to egui 0.36 (`vendor/egui-video`, MIT).
- **Concord** — source of the ported Gateway, password login, voice code and fingerprint v1.
- **Fastpotify** — origin of `theme.rs` and the classic top bar style.
- [Twemoji](https://github.com/twitter/twemoji), Fluent Emoji, [Inter](https://rsms.me/inter/), and [Lucide](https://lucide.dev/).

## Contributing

1. Fork the repo and create a branch (`git checkout -b feature/my-change`).
2. Build in `--release` and run `cargo fmt` and `cargo clippy`.
3. Open a pull request describing what changed and how you tested it.

## License

Released under the [MIT License](LICENSE). Components under `vendor/` keep their own licenses (also MIT).
