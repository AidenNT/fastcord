//! Shared palette, typography, icons, and base widgets.
//!
//! Inter provides real font weights, and Lucide provides a consistent icon set.
//! All colors use [`Palette`] so light, dark, and album-art-tinted themes stay
//! consistent.
//!
//! NOTA (Fastpotify -> clon estilo Discord): este archivo es el theme.rs
//! ORIGINAL del proyecto Fastpotify, tal cual. Depende de:
//!   - assets/fonts/InterVariable.ttf y assets/fonts/NotoEmoji.ttf
//!   - assets/icons/*.svg (Lucide)
//!   - crate::bidi y crate::system_fonts (módulos propios del proyecto)
//!   - las crates egui_extras (con loaders) y skrifa
//! No se puede compilar de forma standalone sin esas piezas: pegalo en tu
//! proyecto real, donde ya existen.

use egui::{Color32, CornerRadius, Frame, Margin, Response, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub window: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub outline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub overlay: Color32,
    pub shadow: Color32,
    /// El color de `window` SIN transparencia. `window`, `panel`, `surface`…
    /// pueden llevar alfa cuando el tema activo es translúcido (ver
    /// [`Backdrop`]); este es el que se pinta como base sólida debajo de todo
    /// y el que hay que usar si se necesita un color opaco.
    pub window_solid: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            dark: true,
            window: Color32::from_rgb(0x0e, 0x0f, 0x14),
            panel: Color32::from_rgb(0x14, 0x16, 0x1d),
            surface: Color32::from_rgb(0x1b, 0x1e, 0x27),
            surface_hover: Color32::from_rgb(0x24, 0x28, 0x36),
            surface_active: Color32::from_rgb(0x2d, 0x32, 0x43),
            outline: Color32::from_rgb(0x26, 0x2a, 0x38),
            text: Color32::from_rgb(0xee, 0xf0, 0xf6),
            secondary: Color32::from_rgb(0xa6, 0xad, 0xc0),
            dim: Color32::from_rgb(0x6b, 0x73, 0x89),
            accent: Color32::from_rgb(0x7c, 0x8c, 0xff),
            accent_hover: Color32::from_rgb(0x96, 0xa3, 0xff),
            on_accent: Color32::from_rgb(0x0b, 0x0d, 0x1a),
            danger: Color32::from_rgb(0xff, 0x7a, 0x8a),
            warning: Color32::from_rgb(0xf4, 0xbf, 0x75),
            overlay: Color32::from_rgb(0x1a, 0x1d, 0x27),
            shadow: Color32::from_black_alpha(120),
            window_solid: Color32::from_rgb(0x0e, 0x0f, 0x14),
        }
    }

    /// Paleta "CreArts": grises tipo Discord clásico con acento azul violáceo.
    /// Colores tomados de la captura de referencia (fondo #36393f, tarjetas
    /// #2f3237, fila activa #4b4e52, acento #6675fb). Pensada para la
    /// interfaz nueva (`Usar nueva interfaz`), donde las tarjetas son MÁS
    /// OSCURAS que el fondo de la ventana.
    pub fn crearts() -> Self {
        Self {
            dark: true,
            window: Color32::from_rgb(0x36, 0x39, 0x3f),
            panel: Color32::from_rgb(0x2f, 0x32, 0x37),
            surface: Color32::from_rgb(0x36, 0x39, 0x3f),
            surface_hover: Color32::from_rgb(0x3f, 0x42, 0x49),
            surface_active: Color32::from_rgb(0x4b, 0x4e, 0x52),
            outline: Color32::from_rgb(0x3f, 0x42, 0x49),
            text: Color32::from_rgb(0xff, 0xff, 0xff),
            secondary: Color32::from_rgb(0xa0, 0xa2, 0xa7),
            dim: Color32::from_rgb(0x7d, 0x80, 0x86),
            accent: Color32::from_rgb(0x66, 0x75, 0xfb),
            accent_hover: Color32::from_rgb(0x7f, 0x8c, 0xfc),
            on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
            danger: Color32::from_rgb(0xed, 0x42, 0x45),
            warning: Color32::from_rgb(0xf0, 0xb2, 0x32),
            overlay: Color32::from_rgb(0x2b, 0x2d, 0x31),
            shadow: Color32::from_black_alpha(120),
            window_solid: Color32::from_rgb(0x36, 0x39, 0x3f),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            window: Color32::from_rgb(0xf4, 0xf5, 0xfa),
            panel: Color32::from_rgb(0xff, 0xff, 0xff),
            surface: Color32::from_rgb(0xec, 0xee, 0xf6),
            surface_hover: Color32::from_rgb(0xe1, 0xe4, 0xf0),
            surface_active: Color32::from_rgb(0xd4, 0xd8, 0xe8),
            outline: Color32::from_rgb(0xdf, 0xe2, 0xee),
            text: Color32::from_rgb(0x16, 0x18, 0x24),
            secondary: Color32::from_rgb(0x57, 0x5d, 0x73),
            dim: Color32::from_rgb(0x8d, 0x93, 0xa8),
            accent: Color32::from_rgb(0x5b, 0x6c, 0xf0),
            accent_hover: Color32::from_rgb(0x4a, 0x5a, 0xd8),
            on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
            danger: Color32::from_rgb(0xd8, 0x40, 0x5a),
            warning: Color32::from_rgb(0xb8, 0x7a, 0x14),
            overlay: Color32::from_rgb(0xff, 0xff, 0xff),
            shadow: Color32::from_black_alpha(50),
            window_solid: Color32::from_rgb(0xf4, 0xf5, 0xfa),
        }
    }

    /// A colour derived from album art, softened so it can sit behind text.
    pub fn tint_from_art(&self, rgb: [u8; 3]) -> Color32 {
        let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lightness = (max + min) / 2.0;
        let target = if self.dark { 0.30 } else { 0.72 };
        let (r, g, b) = if lightness < 0.01 {
            (target, target, target)
        } else {
            let scale = target / lightness;
            (
                (r * scale).min(1.0),
                (g * scale).min(1.0),
                (b * scale).min(1.0),
            )
        };
        Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }
}

static MODERN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Activa o desactiva la interfaz nueva: las barras flotantes (llamada y
/// usuario). El diseño suave de base (paleta, radios, espaciado) está siempre
/// activo. Es un global porque lo leen varias vistas que solo reciben `&Palette`.
pub fn set_modern(on: bool) {
    MODERN.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// `true` con la interfaz nueva (barras flotantes) activada.
pub fn is_modern() -> bool {
    MODERN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Radio estándar de tarjetas y campos.
pub fn radius() -> u8 {
    12
}

/// Radio chico de chips y botones.
pub fn radius_small() -> u8 {
    6
}

/// Radio de las tarjetas flotantes de la interfaz nueva.
pub const CARD_RADIUS: u8 = 18;

/// Hueco entre tarjetas/píldoras de la interfaz nueva.
pub const GAP: i8 = 16;

/// Alto de las píldoras flotantes de arriba (buscador, pestañas, íconos).
pub const PILL_H: f32 = 46.0;

/// Radio de esas píldoras.
pub const PILL_RADIUS: u8 = 18;

/// Color "hundido" para campos de búsqueda dentro de una tarjeta (#212327
/// con la paleta CreArts).
pub fn inset(palette: &Palette) -> Color32 {
    mix(palette.panel, Color32::BLACK, 0.30)
}

/// Color de la barra de título fina de la interfaz nueva.
pub fn titlebar_color(palette: &Palette) -> Color32 {
    mix(palette.window_solid, Color32::BLACK, 0.28)
}

/// Marco de una píldora flotante (buscador, pestañas, íconos de arriba).
pub fn pill_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(PILL_RADIUS))
}

/// Mezcla `a` hacia `b` en `t` (0..=1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

/// Marco de un panel de la interfaz nueva: una tarjeta redondeada que flota
/// sobre el fondo de la ventana, con aire alrededor (`left`/`right` son los
/// huecos laterales; arriba y abajo son fijos). Con la interfaz clásica
/// devuelve `classic` sin tocar, así cada vista elige su marco original.
pub fn card_frame(palette: &Palette, classic: Frame, inner: i8, left: i8, right: i8) -> Frame {
    if !is_modern() {
        return classic;
    }
    // Interfaz nueva: todas las tarjetas se separan GAP entre sí (GAP/2 de
    // cada lado) y el hueco con arriba lo pone la fila de píldoras
    // (`ui::topbar::show_row`); los argumentos `left`/`right` de la clásica
    // se ignoran para que el espaciado sea parejo en todas las pantallas.
    let _ = (left, right);
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .outer_margin(Margin { left: GAP / 2, right: GAP / 2, top: 0, bottom: GAP })
        .inner_margin(Margin::same(inner))
}

/// Fondo de una fila de lista sin seleccionar ni hover: el de la ventana en
/// la interfaz clásica y el de la tarjeta (`panel`) en la nueva.
pub fn row_bg(palette: &Palette) -> Color32 {
    if is_modern() { palette.panel } else { palette.window }
}

/// Relleno de una fila de lista (canal, DM, pestaña): en la interfaz nueva
/// la fila seleccionada se tiñe con el acento y el hover es más suave.
pub fn row_fill(palette: &Palette, selected: bool, hovered: bool) -> Color32 {
    if is_modern() {
        if selected {
            palette.surface_active
        } else if hovered {
            palette.surface_hover
        } else {
            palette.panel
        }
    } else if selected || hovered {
        palette.surface_hover
    } else {
        palette.window
    }
}

/// Borde del encabezado de cada pantalla: la nueva no lo lleva (las
/// tarjetas ya delimitan cada zona).
pub fn header_stroke(palette: &Palette) -> Stroke {
    if is_modern() {
        Stroke::NONE
    } else {
        Stroke::new(1.0, palette.outline)
    }
}

pub const ROW_HEIGHT: f32 = 56.0;
pub const COMPACT_ROW_HEIGHT: f32 = 48.0;
/// The compact track list: one line, no cover.
pub const THIN_ROW_HEIGHT: f32 = 36.0;
pub const PLAYER_BAR_HEIGHT: f32 = 88.0;
/// The narrowest either right-hand panel goes. The queue and the lyrics
/// take the same edge and swap places there, so a width that suits one
/// has to suit the other, or the window would jump on the swap.
pub const SIDE_PANEL_MIN_WIDTH: f32 = 280.0;
pub const TOP_BAR_HEIGHT: f32 = 56.0;

/// macOS hides the titlebar and draws the window content all the way to the
/// top edge, so whatever sits at the top of the window has to leave room for
/// the traffic lights. Zero everywhere else, and in fullscreen, where the
/// buttons are gone.
pub fn titlebar_inset(ctx: &egui::Context) -> f32 {
    if cfg!(target_os = "macos") && !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false))
    {
        28.0
    } else {
        0.0
    }
}

const INTER_MEDIUM: &str = "inter-medium";
const INTER_SEMIBOLD: &str = "inter-semibold";
const INTER_BOLD: &str = "inter-bold";

pub fn regular(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Proportional)
}

pub fn medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(INTER_MEDIUM.into()))
}

pub fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(INTER_SEMIBOLD.into()))
}

pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(INTER_BOLD.into()))
}

/// Install fonts, icons, and the base style once.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    register_icons(ctx);
    egui_extras::install_image_loaders(ctx);
    // Caché en disco de todo lo que se baja por HTTP. Va DESPUÉS de los
    // loaders de egui_extras: egui prueba primero el loader más nuevo.
    crate::support::http_cache::install(ctx);
}

/// Paleta activa tal como la dejó el último `apply` (dark si todavía no corrió).
/// Sirve para los menús contextuales (`ui::audio_menu`), que no reciben la
/// paleta por parámetro.
pub fn current(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(egui::Id::new("ecord_active_palette")))
        .unwrap_or_else(Palette::dark)
}

/// Applies the palette to egui's own widgets so dialogs, menus, and text
/// fields agree with the custom views.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("ecord_active_palette"), *palette));
    let modern = true;
    let mut style = (*ctx.global_style()).clone();
    let visuals = &mut style.visuals;
    *visuals = if palette.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.dark_mode = palette.dark;
    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.overlay;
    visuals.extreme_bg_color = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.code_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.secondary);
    visuals.hyperlink_color = palette.text;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(if modern { 0.28 } else { 0.35 });
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.window_stroke = Stroke::new(1.0, palette.outline);
    visuals.window_corner_radius = CornerRadius::same(radius() + if modern { 6 } else { 2 });
    visuals.menu_corner_radius = CornerRadius::same(radius());
    visuals.window_shadow = egui::epaint::Shadow {
        offset: if modern { [0, 12] } else { [0, 6] },
        blur: if modern { 40 } else { 24 },
        spread: 0,
        color: palette.shadow,
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: if modern { [0, 8] } else { [0, 4] },
        blur: if modern { 28 } else { 16 },
        spread: 0,
        color: palette.shadow,
    };
    let corner = CornerRadius::same(radius_small() + if modern { 4 } else { 2 });
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = corner;
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1.0, palette.text);
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.corner_radius = corner;
    visuals.widgets.noninteractive.bg_fill = palette.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.outline);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    if modern {
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.outline);
    }
    visuals.widgets.hovered.bg_fill = palette.surface_hover;
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hover;
    visuals.widgets.active.bg_fill = palette.surface_active;
    visuals.widgets.active.weak_bg_fill = palette.surface_active;
    visuals.widgets.open.bg_fill = palette.surface_hover;
    visuals.widgets.open.weak_bg_fill = palette.surface_hover;
    visuals.text_cursor.stroke = Stroke::new(2.0, palette.accent);
    visuals.striped = false;
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;

    use egui::FontFamily::{Monospace, Proportional};
    use egui::{FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Body, FontId::new(if modern { 14.5 } else { 14.0 }, Proportional)),
        (TextStyle::Button, FontId::new(if modern { 14.5 } else { 14.0 }, Proportional)),
        (TextStyle::Heading, FontId::new(if modern { 24.0 } else { 22.0 }, Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();
    style.spacing.item_spacing = if modern { Vec2::new(10.0, 8.0) } else { Vec2::new(8.0, 6.0) };
    style.spacing.button_padding = if modern { Vec2::new(14.0, 7.0) } else { Vec2::new(12.0, 6.0) };
    style.spacing.interact_size = if modern { Vec2::new(40.0, 32.0) } else { Vec2::new(40.0, 28.0) };
    style.spacing.menu_margin = egui::Margin::same(if modern { 8 } else { 6 });
    style.spacing.window_margin = egui::Margin::same(if modern { 20 } else { 16 });
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        floating_width: 6.0,
        floating_allocated_width: 0.0,
        handle_min_length: 28.0,
        bar_inner_margin: 3.0,
        bar_outer_margin: 2.0,
        dormant_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_background_opacity: 0.0,
        active_handle_opacity: 0.55,
        interact_handle_opacity: 0.85,
        foreground_color: true,
        ..egui::style::ScrollStyle::floating()
    };
    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.4;
    style.animation_time = if modern { 0.16 } else { 0.12 };
    style.url_in_tooltip = false;
    ctx.set_global_style(style);
}

fn install_fonts(ctx: &egui::Context) {
    use egui::epaint::text::VariationCoords;
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;

    let mut fonts = FontDefinitions::default();
    let inter = include_bytes!("../assets/fonts/InterVariable.ttf");
    let weighted = |weight: f32| {
        let mut data = FontData::from_static(inter);
        data.tweak.coords = VariationCoords::new([(b"wght", weight)]);
        Arc::new(data)
    };
    fonts.font_data.insert("inter".to_owned(), weighted(400.0));
    fonts
        .font_data
        .insert(INTER_MEDIUM.to_owned(), weighted(500.0));
    fonts
        .font_data
        .insert(INTER_SEMIBOLD.to_owned(), weighted(600.0));
    fonts
        .font_data
        .insert(INTER_BOLD.to_owned(), weighted(700.0));

    let noto_emoji = include_bytes!("../assets/fonts/NotoEmoji.ttf");
    fonts.font_data.insert(
        "noto_emoji".to_owned(),
        Arc::new(FontData::from_static(noto_emoji)),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    // Right behind the text face, ahead of the emoji subset and the icon
    // font egui bundles, so every emoji comes from the one full face and
    // wears the same style; egui's pair still serves what Noto lacks.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(1, "noto_emoji".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(1, "noto_emoji".to_owned());
    let fallbacks: Vec<String> = fonts.families[&FontFamily::Proportional]
        .iter()
        .skip(1)
        .cloned()
        .collect();
    for name in [INTER_MEDIUM, INTER_SEMIBOLD, INTER_BOLD] {
        let mut family = vec![name.to_owned()];
        family.extend(fallbacks.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }

    // Add installed fallbacks for scripts Inter does not cover. Keep them after
    // Inter and the emoji font to preserve Latin shapes and color emoji.
    for font in crate::system_fonts::fallbacks() {
        // The bytes are a memory map of the font file (see
        // `system_fonts::map_static`), so nothing is copied here and only the
        // pages epaint actually touches become resident.
        let mut data = FontData::from_static(font.bytes);
        data.index = font.index;
        let offset = fallback_baseline_y_offset(font.bytes, font.index);
        if offset.abs() > 0.001 {
            data.tweak.y_offset_factor = offset;
        }
        fonts.font_data.insert(font.name.clone(), Arc::new(data));
        for family in fonts.families.values_mut() {
            family.push(font.name.clone());
        }
    }

    ctx.set_fonts(fonts);
}

fn fallback_baseline_y_offset(bytes: &[u8], index: u32) -> f32 {
    use skrifa::MetadataProvider as _;

    let Ok(font) = skrifa::FontRef::from_index(bytes, index) else {
        return 0.0;
    };
    let metrics = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let upm = metrics.units_per_em as f32;
    if upm <= 0.0 {
        return 0.0;
    }
    let fallback_height = metrics.ascent - metrics.descent + metrics.leading;
    if fallback_height <= 0.0 {
        return 0.0;
    }
    const INTER_BASELINE_CENTER: f32 = (1984.0 / 2048.0) - 0.5 * ((1984.0 + 494.0) / 2048.0);
    let fallback_baseline_center = (metrics.ascent - 0.5 * fallback_height) / upm;
    INTER_BASELINE_CENTER - fallback_baseline_center
}

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        &[$((
            Icon::$variant,
            concat!("bytes://fastpotify-icon-", $file, ".svg"),
            include_bytes!(concat!("../assets/icons/", $file, ".svg")).as_slice(),
        )),*]
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Icon {
    ArrowLeft,
    ArrowRight,
    AudioLines,
    BadgeCheck,
    Bell,
    Bookmark,
    BookmarkFilled,
    Car,
    Cast,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    CircleAlert,
    CircleCheck,
    CirclePlay,
    CirclePlus,
    CircleX,
    Clock,
    Compass,
    Copy,
    Disc,
    Ellipsis,
    ExternalLink,
    Gamepad,
    Globe,
    GripVertical,
    Headphones,
    Heart,
    HeartFilled,
    House,
    Info,
    Laptop,
    Library,
    ListEnd,
    ListMusic,
    ListPlus,
    ListVideo,
    Loader,
    Lock,
    LogOut,
    // Ícono de "maximizar ventana" para el titlebar propio (ver
    // `ui::topbar`); el archivo ya venía en assets/icons pero no estaba
    // registrado como variante.
    Maximize2,
    Mic,
    MicOff,
    Minus,
    Monitor,
    Moon,
    Music,
    Pause,
    PauseFilled,
    PanelLeft,
    // Íconos de llamada de voz (ver `ui::server`/`ui::dm`/`ui::call_bar`):
    // Lucide no venía con set de "teléfono" en este proyecto (era un
    // cliente de música), así que se agregaron a `assets/icons` junto con
    // los demás — mismo estilo (24x24, stroke 2, cap/join redondeado).
    Phone,
    PhoneOff,
    Pin,
    PinOff,
    Pencil,
    Play,
    PlayFilled,
    Plus,
    Radio,
    Refresh,
    Repeat,
    Repeat1,
    Search,
    Settings,
    Shrink,
    Shuffle,
    SkipBack,
    SkipBackFilled,
    SkipForward,
    SkipForwardFilled,
    Smartphone,
    Smile,
    Sparkles,
    Speaker,
    Square,
    SquarePen,
    Sun,
    Tablet,
    Trash,
    TrendingUp,
    Tv,
    User,
    Users,
    Volume,
    Volume1,
    Volume2,
    VolumeX,
    Watch,
    X,
    // Barra de llamada nueva (`ui::call_bar::show_bottom`).
    MessageCircle,
    SquareArrowUp,
    Video,
    Zap,
    // Visor multimedia (`ui::media_viewer`).
    ZoomIn,
    ZoomOut,
    Download,
    Forward,
}

const ICONS: &[(Icon, &str, &[u8])] = icons! {
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    AudioLines => "audio-lines",
    BadgeCheck => "badge-check",
    Bell => "bell",
    Bookmark => "bookmark",
    BookmarkFilled => "bookmark-filled",
    Car => "car",
    Cast => "cast",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronUp => "chevron-up",
    CircleAlert => "circle-alert",
    CircleCheck => "circle-check",
    CirclePlay => "circle-play",
    CirclePlus => "circle-plus",
    CircleX => "circle-x",
    Clock => "clock",
    Compass => "compass",
    Copy => "copy",
    Disc => "disc-3",
    Ellipsis => "ellipsis",
    ExternalLink => "external-link",
    Gamepad => "gamepad-2",
    Globe => "globe",
    GripVertical => "grip-vertical",
    Headphones => "headphones",
    Heart => "heart",
    HeartFilled => "heart-filled",
    House => "house",
    Info => "info",
    Laptop => "laptop",
    Library => "library",
    ListEnd => "list-end",
    ListMusic => "list-music",
    ListPlus => "list-plus",
    ListVideo => "list-video",
    Loader => "loader-circle",
    Lock => "lock",
    LogOut => "log-out",
    Maximize2 => "maximize-2",
    Mic => "mic",
    MicOff => "mic-off",
    Minus => "minus",
    Monitor => "monitor",
    Moon => "moon",
    Music => "music",
    Pause => "pause",
    PauseFilled => "pause-filled",
    PanelLeft => "panel-left",
    Phone => "phone",
    PhoneOff => "phone-off",
    Pin => "pin",
    PinOff => "pin-off",
    Pencil => "pencil",
    Play => "play",
    PlayFilled => "play-filled",
    Plus => "plus",
    Radio => "radio",
    Refresh => "refresh-cw",
    Repeat => "repeat",
    Repeat1 => "repeat-1",
    Search => "search",
    Settings => "settings",
    Shrink => "shrink",
    Shuffle => "shuffle",
    SkipBack => "skip-back",
    SkipBackFilled => "skip-back-filled",
    SkipForward => "skip-forward",
    SkipForwardFilled => "skip-forward-filled",
    Smartphone => "smartphone",
    Smile => "smile",
    Sparkles => "sparkles",
    Speaker => "speaker",
    Square => "square",
    SquarePen => "square-pen",
    Sun => "sun",
    Tablet => "tablet",
    Trash => "trash-2",
    TrendingUp => "trending-up",
    Tv => "tv",
    User => "user",
    Users => "users",
    Volume => "volume",
    Volume1 => "volume-1",
    Volume2 => "volume-2",
    VolumeX => "volume-x",
    Watch => "watch",
    X => "x",
    MessageCircle => "message-circle",
    SquareArrowUp => "square-arrow-up",
    Video => "video",
    Zap => "zap",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
    Download => "download",
    Forward => "forward",
};

impl Icon {
    pub fn uri(self) -> &'static str {
        ICONS
            .iter()
            .find(|(icon, _, _)| *icon == self)
            .map_or("", |(_, uri, _)| *uri)
    }

    pub fn image(self, color: Color32, size: f32) -> egui::Image<'static> {
        egui::Image::new(self.uri())
            .tint(color)
            .fit_to_exact_size(Vec2::splat(size))
    }
}

fn register_icons(ctx: &egui::Context) {
    for (_, uri, bytes) in ICONS {
        ctx.include_bytes(*uri, *bytes);
    }
}

/// A static icon.
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> Response {
    ui.add(icon.image(color, size))
}

/// Paints an icon centred in `rect` without allocating space.
pub fn paint_icon(ui: &egui::Ui, icon: Icon, rect: egui::Rect, size: f32, color: Color32) {
    let icon_rect = egui::Rect::from_center_size(
        rect.center() + play_glyph_offset(icon, size),
        Vec2::splat(size),
    );
    icon.image(color, size).paint_at(ui, icon_rect);
}

/// Make keyboard focus visible without changing the control's layout.
pub fn focus_ring(ui: &egui::Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            4.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
}

/// A frameless icon control whose colour lifts on hover.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    color: Color32,
    hover: Color32,
    tooltip: &str,
) -> Response {
    let edge = size + 12.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(edge), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip)
    });
    if ui.is_rect_visible(rect) {
        let tint = if response.hovered() || response.has_focus() {
            hover
        } else {
            color
        };
        let scale = if response.is_pointer_button_down_on() {
            0.92
        } else {
            1.0
        };
        paint_icon(ui, icon, rect, size * scale, tint);
    }
    focus_ring(ui, &response);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// Horizontal offset that optically centers play triangles.
pub fn play_glyph_offset(icon: Icon, icon_size: f32) -> Vec2 {
    if matches!(icon, Icon::PlayFilled | Icon::Play) {
        Vec2::new(icon_size * (0.03 - 1.0 / 24.0), 0.0)
    } else {
        Vec2::ZERO
    }
}

/// The app's mark, the accent disc with the play triangle, drawn the same
/// wherever it appears.
pub fn logo(ui: &egui::Ui, center: egui::Pos2, diameter: f32, disc: Color32, glyph: Color32) {
    ui.painter().circle_filled(center, diameter / 2.0, disc);
    let icon_size = diameter * 0.45;
    let icon_rect = egui::Rect::from_center_size(
        center + play_glyph_offset(Icon::PlayFilled, icon_size),
        Vec2::splat(icon_size),
    );
    Icon::PlayFilled
        .image(glyph, icon_size)
        .paint_at(ui, icon_rect);
}

pub fn circle_button(
    ui: &mut egui::Ui,
    icon: Icon,
    diameter: f32,
    fill: Color32,
    fill_hover: Color32,
    icon_color: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let grow = if hovered { 1.05 } else { 1.0 };
        let radius = diameter / 2.0 * grow;
        let fill = if hovered { fill_hover } else { fill };
        ui.painter().circle_filled(rect.center(), radius, fill);
        let icon_size = diameter * 0.46;
        let offset = play_glyph_offset(icon, icon_size);
        let icon_rect =
            egui::Rect::from_center_size(rect.center() + offset, Vec2::splat(icon_size));
        icon.image(icon_color, icon_size).paint_at(ui, icon_rect);
    }
    focus_ring(ui, &response);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// A disc the size of a [`circle_button`] whose icon is replaced by a
/// spinner: the pressed play button itself shows that Spotify is reacting.
pub fn circle_spinner(
    ui: &mut egui::Ui,
    diameter: f32,
    fill: Color32,
    spin: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            ui.is_enabled(),
            tooltip,
        )
    });
    if ui.is_rect_visible(rect) {
        ui.painter()
            .circle_filled(rect.center(), diameter / 2.0, fill);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        ));
        spinner(&mut child, diameter * 0.55, spin);
    }
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// A pill-shaped text button: filled for the primary action, outlined otherwise.
pub fn pill_button(ui: &mut egui::Ui, palette: &Palette, label: &str, primary: bool) -> Response {
    let font = semibold(13.0);
    let color = if primary {
        palette.on_accent
    } else {
        palette.text
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let padding = Vec2::new(18.0, 8.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let radius = rect.height() / 2.0;
        if primary {
            let fill = if hovered {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, radius, fill);
        } else {
            let stroke_color = if hovered { palette.text } else { palette.dim };
            ui.painter().rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, stroke_color),
                egui::StrokeKind::Inside,
            );
        }
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }
    focus_ring(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A muted button with an icon and label, for row and header actions.
pub fn soft_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    active: bool,
) -> Response {
    let font = medium(13.0);
    // `on_accent` y no `window`: `window` puede ser translúcido (ver `Backdrop`).
    let color = if active { palette.on_accent } else { palette.text };
    let galley =
        ui.painter()
            .layout_no_wrap(crate::bidi::display_text(label).into_owned(), font, color);
    let icon_size = 15.0;
    let icon_width = if icon.is_some() { icon_size + 6.0 } else { 0.0 };
    let padding = Vec2::new(12.0, 7.0);
    let size = Vec2::new(galley.size().x + icon_width, galley.size().y) + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let fill = if active {
            palette.text
        } else if hovered {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
        let mut x = rect.left() + padding.x;
        if let Some(icon) = icon {
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(x + icon_size / 2.0, rect.center().y),
                Vec2::splat(icon_size),
            );
            icon.image(color, icon_size).paint_at(ui, icon_rect);
            x += icon_width;
        }
        let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
    }
    focus_ring(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// An animated busy indicator paced independently of the graphics driver.
pub fn spinner(ui: &mut egui::Ui, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        let radius = size / 2.0 - 2.0;
        let start = ui.input(|input| input.time) * std::f64::consts::TAU * 1.2;
        let sweep = 250_f64.to_radians();
        let points = (0..20)
            .map(|index| {
                let angle = start + sweep * f64::from(index) / 19.0;
                let (sin, cos) = angle.sin_cos();
                rect.center() + radius * egui::vec2(cos as f32, sin as f32)
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(2.0, color)));
    }
    response
}

/// Truncated single-line text in a given font and colour.
pub fn text(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    font: egui::FontId,
    color: Color32,
) -> Response {
    let text = text.into();
    if crate::bidi::is_rtl(&text) {
        let galley = crate::bidi::layout(
            ui.painter(),
            &text,
            font,
            color,
            ui.available_width(),
            1,
            Some(crate::bidi::ELLIPSIS),
        );
        return ui.add(egui::Label::new(galley).selectable(false));
    }
    // Emojis Unicode: la fuente los dibuja en blanco y negro, así que si el
    // texto trae alguno se pinta con imágenes a color (ver `ui::emoji`).
    if crate::ui::emoji::contains_emoji(&text) {
        return crate::ui::emoji::line(ui, &text, font, color);
    }
    ui.add(
        egui::Label::new(egui::RichText::new(text).font(font).color(color))
            .truncate()
            .selectable(false),
    )
}

/// Single-line text that acts like a link: underlines on hover, clickable.
pub fn link(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    font: egui::FontId,
    color: Color32,
) -> Response {
    let text = text.into();
    let response = if crate::bidi::is_rtl(&text) {
        let galley = crate::bidi::layout(
            ui.painter(),
            &text,
            font,
            color,
            ui.available_width(),
            1,
            Some(crate::bidi::ELLIPSIS),
        );
        ui.add(
            egui::Label::new(galley)
                .selectable(false)
                .sense(Sense::click()),
        )
    } else {
        ui.add(
            egui::Label::new(egui::RichText::new(text.clone()).font(font).color(color))
                .truncate()
                .selectable(false)
                .sense(Sense::click()),
        )
    };
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &text));
    focus_ring(ui, &response);
    if response.hovered() {
        let rect = response.rect;
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(1.0, color));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn section_title(ui: &mut egui::Ui, palette: &Palette, label: &str) -> Response {
    text(ui, label, bold(17.0), palette.text)
}

pub fn subtle(ui: &mut egui::Ui, palette: &Palette, label: &str) -> Response {
    text(ui, label, regular(13.0), palette.secondary)
}

// ---------------------------------------------------------------------
// Modo de tema: oscuro / claro / según el wallpaper / uno creado por el
// usuario. Ver `ui::settings` para el panel que lo controla, y
// `lib::state::App` para dónde vive (`App::theme_mode`) y cómo se
// persiste (misma `web_local_storage_api` que ya usás para `UserDB`).
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ThemeMode {
    Dark,
    Light,
    /// Paleta recalculada a partir del color dominante del wallpaper
    /// actual del sistema operativo (ver `sample_wallpaper_color_from_path`
    /// y `App::start_wallpaper_watch`).
    Wallpaper,
    /// Paleta "CreArts" (grises tipo Discord + acento violáceo), pensada para
    /// la interfaz nueva. Ver [`Palette::crearts`].
    CreArts,
    /// Nombre de un tema guardado en `App::custom_themes`.
    Custom(String),
}

impl Default for ThemeMode {
    fn default() -> Self {
        ThemeMode::Dark
    }
}

/// Versión serializable de [`Palette`] (colores como `"#rrggbb"`), para
/// guardar temas creados por el usuario en disco/localStorage y poder
/// levantarlos de nuevo. No incluye `overlay` ni `shadow`: esos dos se
/// derivan de `dark` al reconstruir la paleta (ver [`Palette::from_def`]),
/// así el editor de temas no lo obliga a elegir un color para cada
/// detalle interno.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub dark: bool,
    pub window: String,
    pub panel: String,
    pub surface: String,
    pub surface_hover: String,
    pub surface_active: String,
    pub outline: String,
    pub text: String,
    pub secondary: String,
    pub dim: String,
    pub accent: String,
    pub accent_hover: String,
    pub on_accent: String,
    pub danger: String,
    pub warning: String,
    /// Fondo (degradado / imagen), transparencia y translucidez. Los temas
    /// guardados antes de que existiera esto no tienen la clave: con
    /// `serde(default)` cargan como fondo sólido y 100 % opacos, o sea, igual
    /// que antes.
    #[serde(default)]
    pub backdrop: Backdrop,
}

fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Nunca falla: un hex inválido o incompleto cae en un gris neutro en vez
/// de tirar abajo la app (por ejemplo, si el usuario tocó a mano un
/// archivo de tema exportado).
pub fn from_hex(s: &str) -> Color32 {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16);
        let g = u8::from_str_radix(&s[2..4], 16);
        let b = u8::from_str_radix(&s[4..6], 16);
        if let (Ok(r), Ok(g), Ok(b)) = (r, g, b) {
            return Color32::from_rgb(r, g, b);
        }
    }
    Color32::from_rgb(0x33, 0x33, 0x33)
}

impl Palette {
    /// Convierte esta paleta a un [`ThemeDef`] guardable, con el nombre
    /// que le haya puesto el usuario en el editor.
    pub fn to_def(&self, name: impl Into<String>) -> ThemeDef {
        ThemeDef {
            name: name.into(),
            dark: self.dark,
            window: to_hex(self.window_solid),
            panel: to_hex(self.panel),
            surface: to_hex(self.surface),
            surface_hover: to_hex(self.surface_hover),
            surface_active: to_hex(self.surface_active),
            outline: to_hex(self.outline),
            text: to_hex(self.text),
            secondary: to_hex(self.secondary),
            dim: to_hex(self.dim),
            accent: to_hex(self.accent),
            accent_hover: to_hex(self.accent_hover),
            on_accent: to_hex(self.on_accent),
            danger: to_hex(self.danger),
            warning: to_hex(self.warning),
            backdrop: Backdrop::default(),
        }
    }

    /// Reconstruye una paleta completa a partir de un tema guardado.
    /// `overlay`/`shadow` no se guardan: se derivan de `dark` acá mismo.
    pub fn from_def(def: &ThemeDef) -> Self {
        Self::from_def_opaque(def).with_backdrop(&def.backdrop)
    }

    /// Igual que [`Palette::from_def`] pero SIN aplicar la translucidez del
    /// fondo: los colores tal cual los eligió el usuario. Es lo que necesita
    /// el editor de temas como punto de partida (un `Color32` con alfa
    /// premultiplicado no se puede editar con un color picker opaco).
    pub fn from_def_opaque(def: &ThemeDef) -> Self {
        Self {
            dark: def.dark,
            window: from_hex(&def.window),
            panel: from_hex(&def.panel),
            surface: from_hex(&def.surface),
            surface_hover: from_hex(&def.surface_hover),
            surface_active: from_hex(&def.surface_active),
            outline: from_hex(&def.outline),
            text: from_hex(&def.text),
            secondary: from_hex(&def.secondary),
            dim: from_hex(&def.dim),
            accent: from_hex(&def.accent),
            accent_hover: from_hex(&def.accent_hover),
            on_accent: from_hex(&def.on_accent),
            danger: from_hex(&def.danger),
            warning: from_hex(&def.warning),
            overlay: if def.dark { from_hex(&def.panel) } else { Color32::WHITE },
            shadow: if def.dark {
                Color32::from_black_alpha(150)
            } else {
                Color32::from_black_alpha(50)
            },
            window_solid: from_hex(&def.window),
        }
    }

    /// Aplica la translucidez de un [`Backdrop`] a los colores de fondo:
    /// `window` y `panel` usan `panel_opacity`; las superficies
    /// (`surface*`), `surface_opacity`. Como TODA la UI pinta sus marcos con
    /// `palette.window/panel/surface`, con solo esto se vuelve translúcida
    /// sin tocar ni una vista. `overlay` (menús, popups, este mismo panel de
    /// ajustes) nunca baja de 75 % para que siga leyéndose.
    pub fn with_backdrop(mut self, b: &Backdrop) -> Self {
        let panel = b.panel_opacity.clamp(0.0, 1.0);
        let surface = b.surface_opacity.clamp(0.0, 1.0);
        self.window = self.window.gamma_multiply(panel);
        self.panel = self.panel.gamma_multiply(panel);
        self.surface = self.surface.gamma_multiply(surface);
        // Hover/activo un poco más marcados que el reposo, o con superficies
        // muy transparentes no se vería feedback al pasar el mouse.
        self.surface_hover = self.surface_hover.gamma_multiply((surface + 0.10).min(1.0));
        self.surface_active = self.surface_active.gamma_multiply((surface + 0.20).min(1.0));
        self.overlay = self.overlay.gamma_multiply(0.75 + 0.25 * panel);
        self
    }

    /// Genera una paleta oscura completa a partir de un único color
    /// semilla (el color dominante del wallpaper). Mantiene el matiz de
    /// `rgb` y reparte luminosidad/saturación por rol, al estilo del
    /// fondo dinámico de la captura de referencia (Liked Songs de
    /// Spotify): fondo casi negro con un lavado de color, acento vívido
    /// del mismo tono.
    pub fn from_seed(rgb: [u8; 3]) -> Self {
        let (h, s, _l) = rgb_to_hsl(rgb);
        // Wallpapers casi grises (fotos en blanco y negro, fondos lisos)
        // dan una saturación muy baja; forzamos un mínimo para que
        // siempre se note algo de color, y un máximo para que no queme.
        let sat = s.clamp(0.30, 0.85);
        let mk = |lightness: f32, sat_scale: f32| hsl_to_rgb(h, (sat * sat_scale).clamp(0.0, 1.0), lightness);
        Self {
            dark: true,
            window: mk(0.07, 0.9),
            panel: mk(0.10, 0.85),
            surface: mk(0.14, 0.8),
            surface_hover: mk(0.18, 0.75),
            surface_active: mk(0.22, 0.7),
            outline: mk(0.20, 0.6),
            text: Color32::from_rgb(0xf2, 0xf4, 0xf6),
            secondary: mk(0.78, 0.22),
            dim: mk(0.55, 0.18),
            accent: mk(0.58, 1.0),
            accent_hover: mk(0.65, 1.0),
            on_accent: Color32::from_rgb(0x0a, 0x0a, 0x0a),
            danger: Color32::from_rgb(0xf5, 0x71, 0x7f),
            warning: Color32::from_rgb(0xf2, 0xb8, 0x5c),
            overlay: mk(0.12, 0.85),
            shadow: Color32::from_black_alpha(150),
            window_solid: mk(0.07, 0.9),
        }
    }
}

fn rgb_to_hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta.abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let mut h = (if max == r {
        ((g - b) / delta) % 6.0
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    }) * 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color32 {
    if s <= 0.0001 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return Color32::from_rgb(v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = if hp < 1.0 {
        (c, x, 0.0)
    } else if hp < 2.0 {
        (x, c, 0.0)
    } else if hp < 3.0 {
        (0.0, c, x)
    } else if hp < 4.0 {
        (0.0, x, c)
    } else if hp < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c / 2.0;
    let to_u8 = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(to_u8(r1), to_u8(g1), to_u8(b1))
}

/// Lee el wallpaper actual del sistema operativo (crate `wallpaper`, sabe
/// leerlo en Windows/macOS/GNOME/KDE/XFCE y otros) y saca su color
/// dominante con `color_thief`. Pensada para llamarse desde el hilo de
/// fondo que arma `App::start_wallpaper_watch`, no desde el hilo de UI:
/// decodificar la imagen del wallpaper puede tardar unos milisegundos y
/// no queremos trabar el frame.
///
/// Devuelve `None` si no se pudo leer el archivo o `image` no reconoce
/// el formato (por eso el llamador debe tener siempre un fallback, como
/// `Palette::dark()`).
pub fn sample_wallpaper_color_from_path(path: &str) -> Option<[u8; 3]> {
    let img = image::open(path).ok()?;
    // Un wallpaper 4K son ~8M píxeles; no hace falta ni uno solo de más
    // para sacar el color dominante, así que lo reducimos antes de
    // pasárselo a color_thief.
    let small = image::imageops::thumbnail(&img.to_rgb8(), 160, 90);
    let pixels = small.into_raw();
    let palette = color_thief::get_palette(&pixels, color_thief::ColorFormat::Rgb, 4, 5).ok()?;
    let dominant = palette.first()?;
    Some([dominant.r, dominant.g, dominant.b])
}

/// Estado del editor de temas (panel "Crear tema" / "Editar tema" de
/// `ui::settings`). A diferencia de [`ThemeDef`], guarda `Color32` de
/// verdad en vez de hex, porque así es como los pide
/// `egui::Ui::color_edit_button_srgba`; recién se convierte a `ThemeDef`
/// (y por lo tanto a hex) al guardar.
pub struct ThemeEditor {
    /// Nombre original del tema, si estamos editando uno ya guardado
    /// (para poder detectar que le cambiaron el nombre y no dejar un
    /// duplicado con el nombre viejo). `None` si es un tema nuevo.
    pub editing_name: Option<String>,
    pub name: String,
    pub dark: bool,
    pub window: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub outline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warning: Color32,
    /// Fondo / transparencia que se está editando.
    pub backdrop: Backdrop,
    /// Textura y carga de la imagen de la vista previa del editor (separada
    /// de la de la ventana, que sigue mostrando el tema activo).
    pub preview_rt: BackdropRuntime,
}

impl ThemeEditor {
    /// Arranca el editor con los colores de `palette` (la paleta activa,
    /// o la del wallpaper si hay una disponible) como punto de partida.
    pub fn from_palette(palette: &Palette, editing_name: Option<String>, name: String) -> Self {
        Self {
            editing_name,
            name,
            dark: palette.dark,
            window: palette.window,
            panel: palette.panel,
            surface: palette.surface,
            surface_hover: palette.surface_hover,
            surface_active: palette.surface_active,
            outline: palette.outline,
            text: palette.text,
            secondary: palette.secondary,
            dim: palette.dim,
            accent: palette.accent,
            accent_hover: palette.accent_hover,
            on_accent: palette.on_accent,
            danger: palette.danger,
            warning: palette.warning,
            backdrop: Backdrop::default(),
            preview_rt: BackdropRuntime::default(),
        }
    }

    /// Arranca el editor desde un tema guardado, con sus colores originales
    /// (opacos) y su fondo.
    pub fn from_def(def: &ThemeDef) -> Self {
        let mut editor = Self::from_palette(
            &Palette::from_def_opaque(def),
            Some(def.name.clone()),
            def.name.clone(),
        );
        editor.backdrop = def.backdrop.clone();
        editor
    }

    /// Paleta completa con los colores que se están editando ahora
    /// mismo, para la vista previa en vivo del panel de ajustes.
    pub fn to_palette(&self) -> Palette {
        Palette {
            dark: self.dark,
            window: self.window,
            panel: self.panel,
            surface: self.surface,
            surface_hover: self.surface_hover,
            surface_active: self.surface_active,
            outline: self.outline,
            text: self.text,
            secondary: self.secondary,
            dim: self.dim,
            accent: self.accent,
            accent_hover: self.accent_hover,
            on_accent: self.on_accent,
            danger: self.danger,
            warning: self.warning,
            overlay: if self.dark { self.panel } else { Color32::WHITE },
            shadow: if self.dark {
                Color32::from_black_alpha(150)
            } else {
                Color32::from_black_alpha(50)
            },
            window_solid: self.window,
        }
        .with_backdrop(&self.backdrop)
    }

    /// El `ThemeDef` a guardar, con el nombre ya recortado de espacios.
    ///
    /// Se arma con los colores opacos del editor, NO con `to_palette()`: esa
    /// ya trae la translucidez aplicada (alfa premultiplicado) y guardaría
    /// colores equivocados.
    pub fn to_def(&self) -> ThemeDef {
        ThemeDef {
            name: self.name.trim().to_owned(),
            dark: self.dark,
            window: to_hex(self.window),
            panel: to_hex(self.panel),
            surface: to_hex(self.surface),
            surface_hover: to_hex(self.surface_hover),
            surface_active: to_hex(self.surface_active),
            outline: to_hex(self.outline),
            text: to_hex(self.text),
            secondary: to_hex(self.secondary),
            dim: to_hex(self.dim),
            accent: to_hex(self.accent),
            accent_hover: to_hex(self.accent_hover),
            on_accent: to_hex(self.on_accent),
            danger: to_hex(self.danger),
            warning: to_hex(self.warning),
            backdrop: self.backdrop.clone(),
        }
    }
}

// ---------------------------------------------------------------------
// Fondo de la ventana: degradados, imagen, transparencia y translucidez.
//
// Tres capas independientes, de atrás hacia adelante:
//
//   1. El ESCRITORIO, si `background_opacity < 1` (transparencia REAL de la
//      ventana; hace falta que el sistema tenga compositor, ver `main.rs`).
//   2. El FONDO: color sólido, degradado lineal/radial, imagen o el
//      wallpaper del sistema. Lo pinta [`BackdropRuntime::paint`] una vez
//      por frame antes que cualquier panel (ver `App::ui`).
//   3. Los PANELES, translúcidos si `panel_opacity`/`surface_opacity` < 1.
//      Esto no requiere ningún cambio en las vistas: se aplica al armar la
//      [`Palette`] (ver [`Palette::with_backdrop`]), y como toda la UI pinta
//      con `palette.window/panel/surface`, el fondo se ve a través.
// ---------------------------------------------------------------------

/// (De)serializa un `Color32` como `"#rrggbb"`, igual que el resto de
/// [`ThemeDef`], para que el archivo del tema se pueda editar a mano.
mod hex_color {
    use egui::Color32;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(*c))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let s = String::deserialize(d)?;
        Ok(super::from_hex(&s))
    }
}

/// Un color de un degradado y dónde cae (0.0 = inicio, 1.0 = final).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub pos: f32,
    #[serde(with = "hex_color")]
    pub color: Color32,
}

fn stop(pos: f32, hex: &str) -> GradientStop {
    GradientStop { pos, color: from_hex(hex) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackdropKind {
    /// El color "Fondo de ventana" del tema, liso.
    #[default]
    Solid,
    /// Degradado lineal con ángulo (convención CSS: 0° hacia arriba,
    /// 90° hacia la derecha, 180° hacia abajo).
    Linear,
    /// Degradado radial desde un punto.
    Radial,
    /// Un archivo de imagen del disco.
    Image,
    /// El wallpaper de escritorio del sistema operativo, como imagen.
    DesktopWallpaper,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFit {
    /// Llena toda la ventana recortando lo que sobre (como `background-size: cover`).
    #[default]
    Cover,
    /// Entra entera, con franjas del color de fondo si las proporciones no coinciden.
    Contain,
    /// Se estira para llenar la ventana, deformándose si hace falta.
    Stretch,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backdrop {
    pub kind: BackdropKind,

    // -- Degradados (Linear / Radial) --
    /// De 2 a 5 colores. Con menos de 2 se usa el primero como color liso.
    pub stops: Vec<GradientStop>,
    /// Solo `Linear`, en grados (ver [`BackdropKind::Linear`]).
    pub angle: f32,
    /// Solo `Radial`: centro, en fracción de la ventana (0..1, 0..1).
    pub center: [f32; 2],
    /// Solo `Radial`: 1.0 = llega hasta media diagonal de la ventana.
    pub radius: f32,

    // -- Imagen (Image / DesktopWallpaper) --
    pub image_path: String,
    pub image_fit: ImageFit,
    /// 0..1. Se aplica al cargar (en un hilo aparte, sobre una copia reducida).
    pub image_blur: f32,
    /// 0..1. Velo oscuro (o claro, en temas claros) encima de la imagen para
    /// que el texto siga leyéndose.
    pub image_dim: f32,

    // -- Transparencia / translucidez --
    /// Opacidad de TODA la capa de fondo. Por debajo de 1.0 se ve el
    /// escritorio a través de la ventana (transparencia real).
    pub background_opacity: f32,
    /// Opacidad de `window` y `panel` (la mayoría de las áreas de la UI).
    pub panel_opacity: f32,
    /// Opacidad de `surface`, `surface_hover` y `surface_active`
    /// (tarjetas, campos de texto, botones suaves).
    pub surface_opacity: f32,
}

impl Default for Backdrop {
    fn default() -> Self {
        Self {
            kind: BackdropKind::Solid,
            // Puntos de partida al pasar a "Degradado" en el editor.
            stops: vec![stop(0.0, "#3b2a6b"), stop(1.0, "#0f1114")],
            angle: 160.0,
            center: [0.5, 0.0],
            radius: 1.0,
            image_path: String::new(),
            image_fit: ImageFit::Cover,
            image_blur: 0.0,
            image_dim: 0.35,
            background_opacity: 1.0,
            panel_opacity: 1.0,
            surface_opacity: 1.0,
        }
    }
}

impl Backdrop {
    /// `true` si algo de esto deja ver lo que hay detrás (fondo o escritorio).
    pub fn is_see_through(&self) -> bool {
        self.background_opacity < 0.999
            || self.panel_opacity < 0.999
            || self.surface_opacity < 0.999
    }

    /// Fondos de ejemplo para el editor: un clic y queda algo bonito.
    pub fn presets() -> Vec<(&'static str, Backdrop)> {
        let base = Backdrop::default();
        vec![
            (
                "Aurora",
                Backdrop {
                    kind: BackdropKind::Linear,
                    stops: vec![stop(0.0, "#0f2027"), stop(0.5, "#203a43"), stop(1.0, "#2c5364")],
                    angle: 135.0,
                    panel_opacity: 0.80,
                    surface_opacity: 0.85,
                    ..base.clone()
                },
            ),
            (
                "Atardecer",
                Backdrop {
                    kind: BackdropKind::Linear,
                    stops: vec![stop(0.0, "#2b1055"), stop(0.55, "#7a2f6b"), stop(1.0, "#d86a4a")],
                    angle: 200.0,
                    panel_opacity: 0.72,
                    surface_opacity: 0.80,
                    ..base.clone()
                },
            ),
            (
                "Neón",
                Backdrop {
                    kind: BackdropKind::Radial,
                    stops: vec![stop(0.0, "#5b21b6"), stop(0.6, "#160f30"), stop(1.0, "#07060d")],
                    center: [0.25, 0.1],
                    radius: 1.1,
                    panel_opacity: 0.78,
                    surface_opacity: 0.85,
                    ..base.clone()
                },
            ),
            (
                "Océano",
                Backdrop {
                    kind: BackdropKind::Linear,
                    stops: vec![stop(0.0, "#001d3d"), stop(1.0, "#00726b")],
                    angle: 160.0,
                    panel_opacity: 0.85,
                    surface_opacity: 0.90,
                    ..base.clone()
                },
            ),
            (
                // Casi todo transparente: pensado para ver el escritorio.
                "Cristal",
                Backdrop {
                    kind: BackdropKind::Solid,
                    background_opacity: 0.55,
                    panel_opacity: 0.55,
                    surface_opacity: 0.60,
                    ..base
                },
            ),
        ]
    }
}

/// Color del degradado en `t` (0..1). Interpola en sRGB, como CSS.
fn sample_stops(stops: &[GradientStop], t: f32) -> Color32 {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return Color32::TRANSPARENT;
    };
    if t <= first.pos {
        return first.color;
    }
    if t >= last.pos {
        return last.color;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t >= a.pos && t <= b.pos {
            let k = ((t - a.pos) / (b.pos - a.pos).max(1e-6)).clamp(0.0, 1.0);
            let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
            return Color32::from_rgb(
                lerp(a.color.r(), b.color.r()),
                lerp(a.color.g(), b.color.g()),
                lerp(a.color.b(), b.color.b()),
            );
        }
    }
    last.color
}

/// Degradado como malla de vértices coloreados. egui no tiene un primitivo
/// de degradado, pero interpola el color entre vértices: para un lineal de
/// 2 colores alcanzan las 4 esquinas (la interpolación es exacta, porque el
/// color es una función lineal de la posición); con más colores, o radial,
/// se muestrea en una grilla de 32×32.
fn paint_gradient(painter: &egui::Painter, rect: egui::Rect, b: &Backdrop, fallback: Color32, alpha: f32) {
    let mut stops = b.stops.clone();
    stops.sort_by(|a, c| a.pos.partial_cmp(&c.pos).unwrap_or(std::cmp::Ordering::Equal));
    if stops.len() < 2 {
        let color = stops.first().map(|s| s.color.gamma_multiply(alpha)).unwrap_or(fallback);
        painter.rect_filled(rect, 0.0, color);
        return;
    }

    let linear = b.kind == BackdropKind::Linear;
    let (cols, rows) = if linear && stops.len() == 2 { (1, 1) } else { (32, 32) };

    // Parámetros del degradado, calculados una sola vez.
    let angle = b.angle.to_radians();
    let dir = Vec2::new(angle.sin(), -angle.cos());
    let line_len = (rect.width() * dir.x).abs() + (rect.height() * dir.y).abs();
    let radial_center = egui::pos2(
        rect.left() + b.center[0].clamp(0.0, 1.0) * rect.width(),
        rect.top() + b.center[1].clamp(0.0, 1.0) * rect.height(),
    );
    let radial_radius = b.radius.max(0.05) * 0.5 * rect.size().length();

    let t_at = |p: egui::Pos2| -> f32 {
        if linear {
            0.5 + (p - rect.center()).dot(dir) / line_len.max(1.0)
        } else {
            (p - radial_center).length() / radial_radius.max(1.0)
        }
    };

    let mut mesh = egui::epaint::Mesh::default();
    for j in 0..=rows {
        for i in 0..=cols {
            let pos = egui::pos2(
                rect.left() + rect.width() * i as f32 / cols as f32,
                rect.top() + rect.height() * j as f32 / rows as f32,
            );
            let color = sample_stops(&stops, t_at(pos)).gamma_multiply(alpha);
            mesh.colored_vertex(pos, color);
        }
    }
    let stride = (cols + 1) as u32;
    for j in 0..rows as u32 {
        for i in 0..cols as u32 {
            let a = j * stride + i;
            let (b_, c, d) = (a + 1, a + stride, a + stride + 1);
            mesh.add_triangle(a, b_, c);
            mesh.add_triangle(b_, d, c);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Tablero de ajedrez gris: fondo de la vista previa para que se note qué
/// es transparente.
pub fn paint_checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    let tile = 10.0;
    painter.rect_filled(rect, 0.0, Color32::from_gray(0x55));
    let cols = (rect.width() / tile).ceil() as i32;
    let rows = (rect.height() / tile).ceil() as i32;
    for j in 0..rows {
        for i in 0..cols {
            if (i + j) % 2 == 0 {
                continue;
            }
            let min = rect.min + Vec2::new(i as f32 * tile, j as f32 * tile);
            let cell = egui::Rect::from_min_size(min, Vec2::splat(tile)).intersect(rect);
            painter.rect_filled(cell, 0.0, Color32::from_gray(0x77));
        }
    }
}

/// Rectángulo destino y UV para dibujar una textura de `tex` px dentro de
/// `rect` según [`ImageFit`].
fn fit_image(tex: Vec2, rect: egui::Rect, fit: ImageFit) -> (egui::Rect, egui::Rect) {
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if tex.x <= 0.0 || tex.y <= 0.0 {
        return (rect, full);
    }
    match fit {
        ImageFit::Stretch => (rect, full),
        ImageFit::Contain => {
            let scale = (rect.width() / tex.x).min(rect.height() / tex.y);
            (egui::Rect::from_center_size(rect.center(), tex * scale), full)
        }
        ImageFit::Cover => {
            let scale = (rect.width() / tex.x).max(rect.height() / tex.y);
            let visible = Vec2::new(rect.width() / (tex.x * scale), rect.height() / (tex.y * scale));
            (rect, egui::Rect::from_center_size(egui::pos2(0.5, 0.5), visible))
        }
    }
}

/// Decodifica la imagen de fondo. Se llama desde un hilo aparte.
/// Reduce a 2560 px de lado mayor (no tiene sentido subir una foto de 8K a
/// la GPU) y, si hay desenfoque, a 960 px antes de desenfocar: es mucho más
/// rápido y, como la textura se estira con filtro lineal, se ve igual.
fn load_backdrop_image(path: &str, blur: f32) -> Option<egui::ColorImage> {
    use image::imageops::{self, FilterType};
    let img = image::open(path).ok()?;
    let limit = if blur > 0.0 { 960 } else { 2560 };
    let img = if img.width().max(img.height()) > limit {
        img.resize(limit, limit, FilterType::Triangle)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    if blur > 0.0 {
        rgba = imageops::blur(&rgba, blur * 10.0);
    }
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Estado de runtime del fondo: la textura de la imagen y su carga en
/// segundo plano. NO se serializa; vive en `App` (fondo de la ventana) y en
/// `ThemeEditor` (vista previa).
#[derive(Default)]
pub struct BackdropRuntime {
    /// (origen, desenfoque en centésimas) de lo que muestra `texture`.
    loaded_key: Option<(String, u32)>,
    /// Lo que se está cargando ahora, si hay algo.
    pending_key: Option<(String, u32)>,
    texture: Option<egui::TextureHandle>,
    rx: Option<std::sync::mpsc::Receiver<Option<egui::ColorImage>>>,
}

impl BackdropRuntime {
    /// Pinta el fondo en `rect`. Llamar una vez por frame ANTES de dibujar
    /// los paneles.
    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: egui::Rect,
        backdrop: &Backdrop,
        palette: &Palette,
    ) {
        let alpha = backdrop.background_opacity.clamp(0.0, 1.0);
        let base = palette.window_solid.gamma_multiply(alpha);
        match backdrop.kind {
            BackdropKind::Solid => {
                painter.rect_filled(rect, 0.0, base);
            }
            BackdropKind::Linear | BackdropKind::Radial => {
                paint_gradient(painter, rect, backdrop, base, alpha);
            }
            BackdropKind::Image | BackdropKind::DesktopWallpaper => {
                self.paint_image(ctx, painter, rect, backdrop, palette, base, alpha);
            }
        }
    }

    fn paint_image(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: egui::Rect,
        b: &Backdrop,
        palette: &Palette,
        base: Color32,
        alpha: f32,
    ) {
        let source = if b.kind == BackdropKind::DesktopWallpaper {
            "@desktop".to_owned()
        } else {
            b.image_path.trim().to_owned()
        };
        if source.is_empty() {
            painter.rect_filled(rect, 0.0, base);
            return;
        }

        let blur = b.image_blur.clamp(0.0, 1.0);
        let key = (source, (blur * 100.0).round() as u32);
        if self.loaded_key.as_ref() != Some(&key) && self.pending_key.as_ref() != Some(&key) {
            self.start_load(ctx, key, blur);
        }
        self.poll_load(ctx);

        let Some(texture) = &self.texture else {
            // Todavía cargando (o no se pudo leer): color liso.
            painter.rect_filled(rect, 0.0, base);
            return;
        };

        let (dest, uv) = fit_image(texture.size_vec2(), rect, b.image_fit);
        if b.image_fit == ImageFit::Contain {
            // Franjas que deja la imagen, con el color base.
            for bar in [
                egui::Rect::from_min_max(rect.min, egui::pos2(dest.left(), rect.bottom())),
                egui::Rect::from_min_max(egui::pos2(dest.right(), rect.top()), rect.max),
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), dest.top())),
                egui::Rect::from_min_max(egui::pos2(rect.left(), dest.bottom()), rect.max),
            ] {
                if bar.width() > 0.5 && bar.height() > 0.5 {
                    painter.rect_filled(bar, 0.0, base);
                }
            }
        }
        painter
            .with_clip_rect(rect)
            .image(texture.id(), dest, uv, Color32::WHITE.gamma_multiply(alpha));

        let dim = b.image_dim.clamp(0.0, 0.95);
        if dim > 0.001 {
            let veil = if palette.dark { Color32::BLACK } else { Color32::WHITE };
            painter.rect_filled(rect, 0.0, veil.gamma_multiply(dim * alpha));
        }
    }

    fn start_load(&mut self, ctx: &egui::Context, key: (String, u32), blur: f32) {
        let (tx, rx) = std::sync::mpsc::channel();
        let source = key.0.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let path = if source == "@desktop" { wallpaper::get().ok() } else { Some(source) };
            let image = path.and_then(|p| load_backdrop_image(&p, blur));
            let _ = tx.send(image);
            // Despierta a egui: si no, el resultado esperaría al próximo evento.
            ctx.request_repaint();
        });
        // Si había otra carga en curso, su receptor se descarta acá y su
        // resultado se pierde: gana siempre la más reciente.
        self.rx = Some(rx);
        self.pending_key = Some(key);
    }

    fn poll_load(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(image) => {
                // Si no se pudo leer, `texture` queda en `None` pero la clave
                // se marca como cargada: no reintenta en cada frame.
                self.texture = image.map(|img| {
                    ctx.load_texture("backdrop_image", img, egui::TextureOptions::LINEAR)
                });
                self.loaded_key = self.pending_key.take();
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending_key = None;
                self.rx = None;
            }
        }
    }
}
