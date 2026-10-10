//! Calculadora de permisos de Discord (Ajustes → Permisos).
//!
//! Tres modos:
//! * **Permisos**: tildás permisos y sale el valor entero (decimal y hex), o
//!   pegás un valor y se destilda lo que corresponde. Con presets, buscador,
//!   filtro por tipo de canal y generador de enlace de invitación de bots.
//! * **Overwrite de canal**: cada permiso es permitir / neutral / denegar, y
//!   salen los dos enteros (`allow` y `deny`) que usa la API.
//! * **Permisos efectivos**: se cargan los permisos base (@everyone + roles) y
//!   los overwrites del canal (@everyone, roles, miembro) y se calcula qué
//!   puede hacer el miembro, con el mismo orden que documenta Discord (ver
//!   `lib::permissions`).
//!
//! Es autocontenida: todo el estado vive en la memoria temporal de egui (como
//! el cuadro de pegado de temas en `ui::settings`), así que no toca `App`.

use egui::{Align2, CornerRadius, Frame, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use crate::ui::theme::{self, Icon, Palette};

// ---------------------------------------------------------------------
// Tabla de permisos
// ---------------------------------------------------------------------

/// A qué tipos de canal aplica un permiso (máscara). `0` = solo a nivel
/// servidor (no se puede usar en un overwrite).
const T: u8 = 1; // texto
const V: u8 = 2; // voz
const S: u8 = 4; // escenario

struct Perm {
    bit: u8,
    /// Nombre de la constante en la API de Discord.
    key: &'static str,
    name: &'static str,
    desc: &'static str,
    ch: u8,
    cat: usize,
}

const fn p(bit: u8, key: &'static str, name: &'static str, desc: &'static str, ch: u8, cat: usize) -> Perm {
    Perm { bit, key, name, desc, ch, cat }
}

const CATS: [&str; 7] = [
    "General del servidor",
    "Membresía",
    "Canales de texto",
    "Canales de voz",
    "Canales de escenario",
    "Eventos",
    "Avanzado",
];

const ALL_CH: u8 = T | V | S;

const ADMINISTRATOR_BIT: u8 = 3;
const VIEW_CHANNEL_BIT: u8 = 10;

const PERMS: &[Perm] = &[
    // General del servidor
    p(10, "VIEW_CHANNEL", "Ver canales", "Ver los canales y leer su contenido.", ALL_CH, 0),
    p(4, "MANAGE_CHANNELS", "Administrar canales", "Crear, editar y borrar canales.", ALL_CH, 0),
    p(28, "MANAGE_ROLES", "Administrar roles", "Crear y editar roles inferiores al propio.", ALL_CH, 0),
    p(30, "MANAGE_GUILD_EXPRESSIONS", "Administrar expresiones", "Editar y borrar emojis, stickers y sonidos.", 0, 0),
    p(43, "CREATE_GUILD_EXPRESSIONS", "Crear expresiones", "Subir emojis, stickers y sonidos.", 0, 0),
    p(7, "VIEW_AUDIT_LOG", "Ver el registro de auditoría", "Ver quién hizo qué en el servidor.", 0, 0),
    p(19, "VIEW_GUILD_INSIGHTS", "Ver estadísticas del servidor", "Acceso a Server Insights.", 0, 0),
    p(29, "MANAGE_WEBHOOKS", "Administrar webhooks", "Crear, editar y borrar webhooks.", ALL_CH, 0),
    p(5, "MANAGE_GUILD", "Administrar servidor", "Cambiar nombre, región y ajustes del servidor.", 0, 0),
    p(41, "VIEW_CREATOR_MONETIZATION_ANALYTICS", "Ver análisis de monetización", "Ver las analíticas de monetización del creador.", 0, 0),
    // Membresía
    p(0, "CREATE_INSTANT_INVITE", "Crear invitación", "Invitar a gente nueva al servidor o canal.", ALL_CH, 1),
    p(26, "CHANGE_NICKNAME", "Cambiar apodo", "Cambiar el propio apodo.", 0, 1),
    p(27, "MANAGE_NICKNAMES", "Administrar apodos", "Cambiar el apodo de otros miembros.", 0, 1),
    p(1, "KICK_MEMBERS", "Expulsar miembros", "Sacar miembros del servidor.", 0, 1),
    p(2, "BAN_MEMBERS", "Banear miembros", "Banear miembros de forma permanente.", 0, 1),
    p(40, "MODERATE_MEMBERS", "Aislar miembros", "Poner a un miembro en timeout.", 0, 1),
    // Texto
    p(11, "SEND_MESSAGES", "Enviar mensajes", "Escribir en los canales de texto.", ALL_CH, 2),
    p(38, "SEND_MESSAGES_IN_THREADS", "Enviar mensajes en hilos", "Escribir dentro de los hilos.", T, 2),
    p(35, "CREATE_PUBLIC_THREADS", "Crear hilos públicos", "Abrir hilos que ve todo el canal.", T, 2),
    p(36, "CREATE_PRIVATE_THREADS", "Crear hilos privados", "Abrir hilos solo con invitados.", T, 2),
    p(14, "EMBED_LINKS", "Insertar enlaces", "Mostrar vista previa de los enlaces.", ALL_CH, 2),
    p(15, "ATTACH_FILES", "Adjuntar archivos", "Subir archivos e imágenes.", ALL_CH, 2),
    p(6, "ADD_REACTIONS", "Añadir reacciones", "Reaccionar a los mensajes con emojis.", ALL_CH, 2),
    p(18, "USE_EXTERNAL_EMOJIS", "Usar emojis externos", "Usar emojis de otros servidores.", ALL_CH, 2),
    p(37, "USE_EXTERNAL_STICKERS", "Usar stickers externos", "Usar stickers de otros servidores.", ALL_CH, 2),
    p(17, "MENTION_EVERYONE", "Mencionar @everyone, @here y roles", "Avisar a todo el mundo o a cualquier rol.", ALL_CH, 2),
    p(13, "MANAGE_MESSAGES", "Administrar mensajes", "Borrar mensajes de otros y quitar embeds.", ALL_CH, 2),
    p(51, "PIN_MESSAGES", "Fijar mensajes", "Fijar y desfijar mensajes.", ALL_CH, 2),
    p(34, "MANAGE_THREADS", "Administrar hilos", "Renombrar, borrar y archivar hilos.", T, 2),
    p(16, "READ_MESSAGE_HISTORY", "Ver el historial de mensajes", "Leer lo escrito antes de entrar.", ALL_CH, 2),
    p(12, "SEND_TTS_MESSAGES", "Enviar mensajes de texto a voz", "Usar /tts para que se lean en voz alta.", ALL_CH, 2),
    p(31, "USE_APPLICATION_COMMANDS", "Usar comandos de aplicaciones", "Usar comandos /, menús y botones de apps.", ALL_CH, 2),
    p(46, "SEND_VOICE_MESSAGES", "Enviar mensajes de voz", "Mandar mensajes de voz grabados.", ALL_CH, 2),
    p(49, "SEND_POLLS", "Crear encuestas", "Crear encuestas en el canal.", ALL_CH, 2),
    p(50, "USE_EXTERNAL_APPS", "Usar aplicaciones externas", "Usar apps instaladas en la cuenta, no en el servidor.", ALL_CH, 2),
    p(52, "BYPASS_SLOWMODE", "Saltarse el modo lento", "No se aplica el modo lento del canal.", ALL_CH, 2),
    // Voz
    p(20, "CONNECT", "Conectar", "Entrar a los canales de voz.", V | S, 3),
    p(21, "SPEAK", "Hablar", "Hablar en los canales de voz.", V, 3),
    p(9, "STREAM", "Vídeo y transmisión", "Activar la cámara y compartir pantalla.", V | S, 3),
    p(39, "USE_EMBEDDED_ACTIVITIES", "Usar actividades", "Lanzar actividades dentro de un canal.", T | V, 3),
    p(42, "USE_SOUNDBOARD", "Usar soundboard", "Reproducir sonidos del soundboard.", V, 3),
    p(45, "USE_EXTERNAL_SOUNDS", "Usar sonidos externos", "Usar sonidos de otros servidores.", V, 3),
    p(25, "USE_VAD", "Usar detección de voz", "Hablar sin pulsar para hablar.", V, 3),
    p(8, "PRIORITY_SPEAKER", "Prioridad de palabra", "Baja el volumen de los demás al hablar.", V, 3),
    p(22, "MUTE_MEMBERS", "Silenciar miembros", "Silenciar a otros en voz.", V | S, 3),
    p(23, "DEAFEN_MEMBERS", "Ensordecer miembros", "Ensordecer a otros en voz.", V | S, 3),
    p(24, "MOVE_MEMBERS", "Mover miembros", "Mover miembros entre canales de voz.", V | S, 3),
    p(48, "SET_VOICE_CHANNEL_STATUS", "Establecer estado del canal de voz", "Poner el texto de estado de un canal de voz.", V, 3),
    // Escenario
    p(32, "REQUEST_TO_SPEAK", "Pedir la palabra", "Pedir hablar en un canal de escenario.", S, 4),
    // Eventos
    p(44, "CREATE_EVENTS", "Crear eventos", "Crear eventos programados.", V | S, 5),
    p(33, "MANAGE_EVENTS", "Administrar eventos", "Editar y cancelar cualquier evento.", V | S, 5),
    // Avanzado
    p(3, "ADMINISTRATOR", "Administrador", "Todos los permisos y se saltea cualquier overwrite.", 0, 6),
];

fn known_mask() -> u64 {
    PERMS.iter().fold(0, |acc, perm| acc | (1u64 << perm.bit))
}

fn bits(list: &[u8]) -> u64 {
    list.iter().fold(0, |acc, b| acc | (1u64 << b))
}

const MEMBER_BITS: &[u8] = &[
    10, 0, 26, 11, 38, 35, 14, 15, 6, 18, 37, 16, 31, 46, 49, 50, 20, 21, 9, 39, 42, 45, 25, 32,
];
const MOD_EXTRA_BITS: &[u8] = &[1, 2, 40, 13, 51, 34, 22, 23, 24, 27, 17, 7, 52];

// ---------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Value,
    Overwrite,
    Effective,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tri {
    Deny,
    Neutral,
    Allow,
}

#[derive(Clone)]
struct CalcState {
    mode: Mode,
    /// Modo "Permisos": el valor.
    perms: u64,
    /// Modo "Overwrite": los dos enteros.
    allow: u64,
    deny: u64,
    /// Texto de cada campo numérico y el valor al que corresponde (para
    /// re-escribirlo solo cuando el valor cambió por otro lado, p. ej. un
    /// clic en la lista, sin pisar lo que el usuario está tecleando).
    text: String,
    text_for: u64,
    allow_text: String,
    allow_text_for: u64,
    deny_text: String,
    deny_text_for: u64,
    search: String,
    /// Filtro por tipo de canal: 0 = todos, o `T` / `V` / `S`.
    chan: u8,
    client_id: String,
    scope_bot: bool,
    scope_commands: bool,
    /// Campos del modo "Permisos efectivos" (ver `EFF_FIELDS`).
    eff: [String; 7],
    /// Último botón de copiar usado y cuándo (para mostrar "Copiado").
    copied: Option<(&'static str, f64)>,
}

impl Default for CalcState {
    fn default() -> Self {
        Self {
            mode: Mode::Value,
            perms: 0,
            allow: 0,
            deny: 0,
            text: String::new(),
            text_for: 0,
            allow_text: String::new(),
            allow_text_for: 0,
            deny_text: String::new(),
            deny_text_for: 0,
            search: String::new(),
            chan: 0,
            client_id: String::new(),
            scope_bot: true,
            scope_commands: true,
            eff: Default::default(),
            copied: None,
        }
    }
}

const EFF_FIELDS: [&str; 7] = [
    "Permisos base (@everyone + roles)",
    "Overwrite de @everyone · permitir",
    "Overwrite de @everyone · denegar",
    "Overwrites de roles · permitir",
    "Overwrites de roles · denegar",
    "Overwrite del miembro · permitir",
    "Overwrite del miembro · denegar",
];

// ---------------------------------------------------------------------
// Números
// ---------------------------------------------------------------------

fn parse_one(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0);
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    s.parse::<u64>().ok()
}

/// Acepta decimal o `0x…` hex; varios valores separados por coma, espacio,
/// `|` o `+` se unen (OR). Vacío = 0. `None` si algún trozo no es un número.
fn parse_perms(s: &str) -> Option<u64> {
    let mut acc = 0u64;
    for part in s.split(|c: char| c == ',' || c == ';' || c == '|' || c == '+' || c.is_whitespace()) {
        if part.is_empty() {
            continue;
        }
        acc |= parse_one(part)?;
    }
    Some(acc)
}

/// Algoritmo de Discord para un miembro (ver `lib::permissions`): base →
/// overwrite de @everyone → overwrites de roles → overwrite del miembro.
fn compute_effective(st: &CalcState) -> Option<(u64, bool)> {
    let mut v = [0u64; 7];
    for (i, text) in st.eff.iter().enumerate() {
        v[i] = parse_perms(text)?;
    }
    let [base, ev_allow, ev_deny, role_allow, role_deny, mem_allow, mem_deny] = v;
    if base & (1u64 << ADMINISTRATOR_BIT) != 0 {
        return Some((known_mask(), true));
    }
    let mut perms = base;
    perms &= !ev_deny;
    perms |= ev_allow;
    perms &= !role_deny;
    perms |= role_allow;
    perms &= !mem_deny;
    perms |= mem_allow;
    Some((perms, false))
}

fn channel_tags(ch: u8) -> String {
    if ch == 0 {
        return "Servidor".to_owned();
    }
    let mut parts = Vec::new();
    if ch & T != 0 {
        parts.push("T");
    }
    if ch & V != 0 {
        parts.push("V");
    }
    if ch & S != 0 {
        parts.push("E");
    }
    parts.join(" · ")
}

fn visible(perm: &Perm, query: &str, chan: u8, mode: Mode) -> bool {
    if mode == Mode::Overwrite && perm.ch == 0 {
        return false;
    }
    if chan != 0 && perm.ch & chan == 0 {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    perm.name.to_lowercase().contains(query)
        || perm.desc.to_lowercase().contains(query)
        || perm.key.to_lowercase().replace('_', " ").contains(query)
}

// ---------------------------------------------------------------------
// Pieza de UI: botones y campos
// ---------------------------------------------------------------------

fn copy_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    copied: &mut Option<(&'static str, f64)>,
    tag: &'static str,
    label: &str,
    text: String,
) {
    let now = ui.input(|i| i.time);
    let recent = (*copied).is_some_and(|(t, at)| t == tag && now - at < 1.5);
    let (icon, shown) = if recent { (Icon::Check, "Copiado") } else { (Icon::Copy, label) };
    if theme::soft_button(ui, palette, Some(icon), shown, false).clicked() {
        ui.ctx().copy_text(text);
        *copied = Some((tag, now));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1600));
    }
}

/// Campo numérico ligado a `value`: si `value` cambió por otro lado el texto
/// se reescribe; si el usuario teclea algo válido, `value` se actualiza.
/// Devuelve `false` si el texto actual no es un número válido.
fn number_field(
    ui: &mut egui::Ui,
    palette: &Palette,
    text: &mut String,
    text_for: &mut u64,
    value: &mut u64,
    width: f32,
) -> bool {
    if *value != *text_for {
        *text = value.to_string();
        *text_for = *value;
    }
    let valid = parse_perms(text).is_some();
    let mut edit = egui::TextEdit::singleline(text)
        .font(egui::TextStyle::Monospace)
        .hint_text("0")
        .desired_width(width);
    if !valid {
        edit = edit.text_color(palette.danger);
    }
    let response = ui.add(edit);
    if response.changed() {
        if let Some(v) = parse_perms(text) {
            *value = v;
            *text_for = v;
        }
    }
    parse_perms(text).is_some()
}

/// Caja de un permiso: panel con el fondo suave de las filas.
fn row_rect(ui: &mut egui::Ui, sense: Sense) -> (Rect, egui::Response) {
    ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense)
}

const ROW_H: f32 = 40.0;

fn paint_texts(ui: &egui::Ui, palette: &Palette, rect: Rect, perm: &Perm, left: f32, right_reserved: f32) {
    let clip = Rect::from_min_max(
        egui::pos2(rect.left() + left, rect.top()),
        egui::pos2(rect.right() - right_reserved, rect.bottom()),
    );
    let painter = ui.painter_at(clip);
    let name_color = if perm.bit == ADMINISTRATOR_BIT { palette.danger } else { palette.text };
    painter.text(
        egui::pos2(rect.left() + left, rect.top() + 6.0),
        Align2::LEFT_TOP,
        perm.name,
        theme::semibold(13.5),
        name_color,
    );
    painter.text(
        egui::pos2(rect.left() + left, rect.top() + 23.0),
        Align2::LEFT_TOP,
        perm.desc,
        theme::regular(11.5),
        palette.dim,
    );
}

fn paint_tags(ui: &egui::Ui, palette: &Palette, rect: Rect, perm: &Perm, right_margin: f32) {
    ui.painter().text(
        egui::pos2(rect.right() - right_margin, rect.center().y),
        Align2::RIGHT_CENTER,
        channel_tags(perm.ch),
        theme::medium(11.5),
        palette.dim,
    );
}

/// Fila con casilla. `readonly` solo muestra ✓ / ✗ (permisos efectivos).
/// Devuelve `true` si se hizo clic.
fn check_row(ui: &mut egui::Ui, palette: &Palette, perm: &Perm, on: bool, readonly: bool) -> bool {
    let (rect, response) = row_rect(ui, if readonly { Sense::hover() } else { Sense::click() });
    if ui.is_rect_visible(rect) {
        if response.hovered() && !readonly {
            ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
        }
        let danger = perm.bit == ADMINISTRATOR_BIT;
        let tone = if danger { palette.danger } else { palette.accent };
        let box_rect = Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(18.0));
        if on {
            ui.painter().rect_filled(box_rect, CornerRadius::same(5), tone);
            let icon_rect = Rect::from_center_size(box_rect.center(), Vec2::splat(13.0));
            Icon::Check.image(palette.on_accent, 13.0).paint_at(ui, icon_rect);
        } else if readonly {
            let icon_rect = Rect::from_center_size(box_rect.center(), Vec2::splat(15.0));
            Icon::X.image(palette.dim, 15.0).paint_at(ui, icon_rect);
        } else {
            ui.painter().rect_stroke(
                box_rect,
                CornerRadius::same(5),
                Stroke::new(1.5, palette.dim),
                StrokeKind::Inside,
            );
        }
        paint_texts(ui, palette, rect, perm, 44.0, 70.0);
        paint_tags(ui, palette, rect, perm, 10.0);
    }
    if readonly {
        return false;
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Fila con control de tres estados (denegar / neutral / permitir).
fn tri_row(ui: &mut egui::Ui, palette: &Palette, perm: &Perm, current: Tri) -> Option<Tri> {
    let (rect, response) = row_rect(ui, Sense::hover());
    let mut picked = None;
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
        }
        paint_texts(ui, palette, rect, perm, 12.0, 160.0);
        const BTN: Vec2 = Vec2::new(32.0, 26.0);
        const GAP: f32 = 3.0;
        let total = BTN.x * 3.0 + GAP * 2.0;
        let start_x = rect.right() - 10.0 - total;
        paint_tags(ui, palette, rect, perm, 10.0 + total + 12.0);
        let options = [
            (Tri::Deny, Icon::X, palette.danger, "Denegar"),
            (Tri::Neutral, Icon::Minus, palette.secondary, "Neutral (heredar)"),
            (Tri::Allow, Icon::Check, palette.accent, "Permitir"),
        ];
        for (i, (value, icon, tone, tip)) in options.into_iter().enumerate() {
            let btn = Rect::from_min_size(
                egui::pos2(start_x + i as f32 * (BTN.x + GAP), rect.center().y - BTN.y / 2.0),
                BTN,
            );
            let r = ui
                .interact(btn, egui::Id::new(("perm_tri", perm.bit, i)), Sense::click())
                .on_hover_text(tip)
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let selected = current == value;
            let fill = if selected {
                tone
            } else if r.hovered() {
                palette.surface_active
            } else {
                palette.surface
            };
            ui.painter().rect_filled(btn, CornerRadius::same(7), fill);
            let fg = if selected { palette.on_accent } else { palette.dim };
            let icon_rect = Rect::from_center_size(btn.center(), Vec2::splat(14.0));
            icon.image(fg, 14.0).paint_at(ui, icon_rect);
            if r.clicked() {
                picked = Some(value);
            }
        }
    }
    picked
}

fn card<R>(ui: &mut egui::Ui, palette: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::radius()))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn mono(ui: &mut egui::Ui, text: String, color: egui::Color32) {
    ui.label(RichText::new(text).font(egui::FontId::monospace(13.0)).color(color));
}

// ---------------------------------------------------------------------
// Vista principal
// ---------------------------------------------------------------------

pub fn view(ui: &mut egui::Ui, palette: &Palette) {
    let id = egui::Id::new("perm_calc_state");
    let mut st: CalcState = ui.ctx().data_mut(|d| d.get_temp(id)).unwrap_or_default();

    theme::section_title(ui, palette, "Calculadora de permisos");
    ui.add_space(4.0);
    theme::subtle(
        ui,
        palette,
        "Armá o decodificá valores de permisos de Discord: tildá permisos para obtener el entero, \
         pegá un número para ver qué contiene, o calculá los permisos efectivos de un miembro en un canal.",
    );
    ui.add_space(10.0);

    ui.horizontal_wrapped(|ui| {
        for (mode, label) in [
            (Mode::Value, "Permisos"),
            (Mode::Overwrite, "Overwrite de canal"),
            (Mode::Effective, "Permisos efectivos"),
        ] {
            if theme::soft_button(ui, palette, None, label, st.mode == mode).clicked() {
                st.mode = mode;
            }
        }
    });
    ui.add_space(10.0);

    // Valor mostrado en la lista de solo lectura (modo efectivo).
    let mut effective_value = 0u64;
    match st.mode {
        Mode::Value => value_header(ui, palette, &mut st),
        Mode::Overwrite => overwrite_header(ui, palette, &mut st),
        Mode::Effective => effective_value = effective_header(ui, palette, &mut st),
    }

    ui.add_space(12.0);
    filters(ui, palette, &mut st);
    ui.add_space(4.0);
    permission_list(ui, palette, &mut st, effective_value);

    if st.mode == Mode::Value {
        ui.add_space(16.0);
        invite_builder(ui, palette, &mut st);
    }
    ui.add_space(8.0);

    ui.ctx().data_mut(|d| d.insert_temp(id, st));
}

// ---------------------------------------------------------------------
// Modo "Permisos"
// ---------------------------------------------------------------------

fn value_header(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState) {
    card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            theme::text(ui, "Valor", theme::semibold(13.5), palette.text);
            let valid = {
                let CalcState { text, text_for, perms, .. } = &mut *st;
                number_field(ui, palette, text, text_for, perms, 240.0)
            };
            if !valid {
                theme::text(ui, "Número no válido", theme::medium(12.0), palette.danger);
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            theme::text(ui, "Hex", theme::semibold(12.5), palette.secondary);
            mono(ui, format!("0x{:X}", st.perms), palette.text);
            ui.add_space(10.0);
            let active = (st.perms & known_mask()).count_ones();
            theme::text(
                ui,
                format!("{active} de {} permisos activos", PERMS.len()),
                theme::regular(12.5),
                palette.secondary,
            );
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let perms = st.perms;
            copy_button(ui, palette, &mut st.copied, "dec", "Copiar decimal", perms.to_string());
            copy_button(ui, palette, &mut st.copied, "hex", "Copiar hex", format!("0x{perms:X}"));
            if theme::soft_button(ui, palette, None, "Seleccionar todo", false).clicked() {
                st.perms = known_mask();
            }
            if theme::soft_button(ui, palette, None, "Limpiar", false).clicked() {
                st.perms = 0;
            }
        });
        ui.add_space(10.0);
        theme::text(ui, "Presets", theme::semibold(12.5), palette.secondary);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if theme::soft_button(ui, palette, None, "Solo lectura", false).clicked() {
                st.perms = bits(&[10, 16]);
            }
            if theme::soft_button(ui, palette, None, "Miembro típico", false).clicked() {
                st.perms = bits(MEMBER_BITS);
            }
            if theme::soft_button(ui, palette, None, "Moderador", false).clicked() {
                st.perms = bits(MEMBER_BITS) | bits(MOD_EXTRA_BITS);
            }
            if theme::soft_button(ui, palette, None, "Administrador", false).clicked() {
                st.perms = 1u64 << ADMINISTRATOR_BIT;
            }
        });

        if st.perms & (1u64 << ADMINISTRATOR_BIT) != 0 {
            ui.add_space(8.0);
            theme::text(
                ui,
                "Administrador concede TODOS los permisos y se saltea los overwrites de canal. Dalo con cuidado.",
                theme::medium(12.0),
                palette.danger,
            );
        }
        let unknown = st.perms & !known_mask();
        if unknown != 0 {
            ui.add_space(8.0);
            theme::text(
                ui,
                format!(
                    "El valor tiene bits que esta tabla no conoce (0x{unknown:X}). Se conservan, pero no se muestran en la lista."
                ),
                theme::medium(12.0),
                palette.warning,
            );
        }
    });
}

// ---------------------------------------------------------------------
// Modo "Overwrite de canal"
// ---------------------------------------------------------------------

fn overwrite_header(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState) {
    card(ui, palette, |ui| {
        theme::subtle(
            ui,
            palette,
            "Para cada permiso elegí denegar (✗), neutral (–, hereda del rol) o permitir (✓). \
             La API de Discord guarda un overwrite como dos enteros: allow y deny.",
        );
        ui.add_space(8.0);
        let mut all_valid = true;
        ui.horizontal(|ui| {
            theme::text(ui, "allow", theme::semibold(13.5), palette.accent);
            let ok = {
                let CalcState { allow_text, allow_text_for, allow, .. } = &mut *st;
                number_field(ui, palette, allow_text, allow_text_for, allow, 200.0)
            };
            all_valid &= ok;
            let allow = st.allow;
            copy_button(ui, palette, &mut st.copied, "allow", "Copiar", allow.to_string());
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            theme::text(ui, "deny", theme::semibold(13.5), palette.danger);
            ui.add_space(8.0);
            let ok = {
                let CalcState { deny_text, deny_text_for, deny, .. } = &mut *st;
                number_field(ui, palette, deny_text, deny_text_for, deny, 200.0)
            };
            all_valid &= ok;
            let deny = st.deny;
            copy_button(ui, palette, &mut st.copied, "deny", "Copiar", deny.to_string());
        });
        if !all_valid {
            ui.add_space(4.0);
            theme::text(ui, "Alguno de los números no es válido.", theme::medium(12.0), palette.danger);
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let allowed = (st.allow & known_mask()).count_ones();
            let denied = (st.deny & known_mask()).count_ones();
            theme::text(
                ui,
                format!("{allowed} permitidos · {denied} denegados"),
                theme::regular(12.5),
                palette.secondary,
            );
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let channel_mask = PERMS.iter().filter(|p| p.ch != 0).fold(0u64, |a, p| a | (1u64 << p.bit));
            if theme::soft_button(ui, palette, None, "Permitir todos", false).clicked() {
                st.allow |= channel_mask;
                st.deny &= !channel_mask;
            }
            if theme::soft_button(ui, palette, None, "Denegar todos", false).clicked() {
                st.deny |= channel_mask;
                st.allow &= !channel_mask;
            }
            if theme::soft_button(ui, palette, None, "Todo neutral", false).clicked() {
                st.allow = 0;
                st.deny = 0;
            }
        });
        if st.allow & st.deny != 0 {
            ui.add_space(8.0);
            theme::text(
                ui,
                "Hay bits en allow y en deny a la vez: Discord aplica primero el deny y después el allow, así que gana permitir.",
                theme::medium(12.0),
                palette.warning,
            );
        }
    });
}

// ---------------------------------------------------------------------
// Modo "Permisos efectivos"
// ---------------------------------------------------------------------

/// Dibuja los campos y el resultado; devuelve el valor efectivo (0 si algún
/// campo no es válido).
fn effective_header(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState) -> u64 {
    card(ui, palette, |ui| {
        theme::subtle(
            ui,
            palette,
            "Orden de Discord: permisos base (@everyone + roles) → overwrite de @everyone → overwrites de roles → \
             overwrite del miembro (en cada paso primero se quita el deny y después se suma el allow). \
             Podés pegar varios valores por campo separados por coma: se unen.",
        );
        ui.add_space(8.0);
        egui::Grid::new("perm_eff_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            for (i, label) in EFF_FIELDS.iter().enumerate() {
                theme::text(ui, *label, theme::medium(12.5), palette.text);
                let bad = parse_perms(&st.eff[i]).is_none();
                let mut edit = egui::TextEdit::singleline(&mut st.eff[i])
                    .font(egui::TextStyle::Monospace)
                    .hint_text("0")
                    .desired_width(260.0);
                if bad {
                    edit = edit.text_color(palette.danger);
                }
                ui.add(edit);
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if theme::soft_button(ui, palette, None, "Usar el valor de la pestaña Permisos como base", false).clicked() {
                st.eff[0] = st.perms.to_string();
            }
            if theme::soft_button(ui, palette, None, "Limpiar", false).clicked() {
                st.eff = Default::default();
            }
        });
        ui.add_space(10.0);

        let Some((value, admin)) = compute_effective(st) else {
            theme::text(ui, "Corregí los campos en rojo para calcular.", theme::medium(12.5), palette.danger);
            return 0;
        };
        ui.horizontal_wrapped(|ui| {
            theme::text(ui, "Resultado", theme::semibold(13.5), palette.text);
            mono(ui, value.to_string(), palette.accent);
            mono(ui, format!("0x{value:X}"), palette.secondary);
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            copy_button(ui, palette, &mut st.copied, "eff_dec", "Copiar decimal", value.to_string());
            copy_button(ui, palette, &mut st.copied, "eff_hex", "Copiar hex", format!("0x{value:X}"));
            if theme::soft_button(ui, palette, None, "Cargar en la pestaña Permisos", false).clicked() {
                st.perms = value;
                st.mode = Mode::Value;
            }
        });
        if admin {
            ui.add_space(8.0);
            theme::text(
                ui,
                "Los permisos base incluyen Administrador: tiene todos los permisos y los overwrites no se aplican.",
                theme::medium(12.0),
                palette.danger,
            );
        } else if value & (1u64 << VIEW_CHANNEL_BIT) == 0 {
            ui.add_space(8.0);
            theme::text(
                ui,
                "Sin \"Ver canales\" el miembro no ve el canal, así que el resto de permisos no le sirve en la práctica.",
                theme::medium(12.0),
                palette.warning,
            );
        }
        value
    })
}

// ---------------------------------------------------------------------
// Filtros y lista
// ---------------------------------------------------------------------

fn filters(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState) {
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.search)
                .hint_text("Buscar permiso…")
                .desired_width(220.0),
        );
        ui.add_space(6.0);
        for (chan, label) in [(0u8, "Todos"), (T, "Texto"), (V, "Voz"), (S, "Escenario")] {
            if theme::soft_button(ui, palette, None, label, st.chan == chan).clicked() {
                st.chan = chan;
            }
        }
    });
    ui.add_space(4.0);
    theme::text(
        ui,
        "Etiquetas de la derecha: T = texto · V = voz · E = escenario · Servidor = solo a nivel servidor",
        theme::regular(11.5),
        palette.dim,
    );
}

fn permission_list(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState, effective_value: u64) {
    let query = st.search.trim().to_lowercase();
    let mode = st.mode;
    let mut shown = 0usize;
    for (ci, cat) in CATS.iter().enumerate() {
        let items: Vec<&Perm> = PERMS
            .iter()
            .filter(|p| p.cat == ci && visible(p, &query, st.chan, mode))
            .collect();
        if items.is_empty() {
            continue;
        }
        shown += items.len();
        ui.add_space(10.0);
        theme::text(ui, *cat, theme::bold(13.0), palette.secondary);
        ui.add_space(2.0);
        for perm in items {
            let mask = 1u64 << perm.bit;
            match mode {
                Mode::Value => {
                    if check_row(ui, palette, perm, st.perms & mask != 0, false) {
                        st.perms ^= mask;
                    }
                }
                Mode::Effective => {
                    check_row(ui, palette, perm, effective_value & mask != 0, true);
                }
                Mode::Overwrite => {
                    let current = if st.allow & mask != 0 {
                        Tri::Allow
                    } else if st.deny & mask != 0 {
                        Tri::Deny
                    } else {
                        Tri::Neutral
                    };
                    if let Some(next) = tri_row(ui, palette, perm, current) {
                        st.allow &= !mask;
                        st.deny &= !mask;
                        match next {
                            Tri::Allow => st.allow |= mask,
                            Tri::Deny => st.deny |= mask,
                            Tri::Neutral => {}
                        }
                    }
                }
            }
        }
    }
    if shown == 0 {
        ui.add_space(12.0);
        theme::subtle(ui, palette, "Ningún permiso coincide con la búsqueda.");
    }
}

// ---------------------------------------------------------------------
// Enlace de invitación
// ---------------------------------------------------------------------

fn invite_builder(ui: &mut egui::Ui, palette: &Palette, st: &mut CalcState) {
    egui::CollapsingHeader::new(
        RichText::new("Enlace de invitación de un bot")
            .font(theme::semibold(13.5))
            .color(palette.text),
    )
    .default_open(false)
    .show(ui, |ui| {
        theme::subtle(
            ui,
            palette,
            "Arma el enlace de OAuth2 para invitar a un bot con los permisos tildados arriba.",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::text(ui, "ID de la aplicación", theme::medium(12.5), palette.text);
            ui.add(
                egui::TextEdit::singleline(&mut st.client_id)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("123456789012345678")
                    .desired_width(210.0),
            );
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if theme::soft_button(ui, palette, None, "bot", st.scope_bot).clicked() {
                st.scope_bot = !st.scope_bot;
            }
            if theme::soft_button(ui, palette, None, "applications.commands", st.scope_commands).clicked() {
                st.scope_commands = !st.scope_commands;
            }
        });
        ui.add_space(6.0);
        let client_id = st.client_id.trim().to_owned();
        if client_id.is_empty() {
            theme::text(ui, "Pegá el ID de la aplicación para generar el enlace.", theme::regular(12.0), palette.dim);
        } else if !client_id.chars().all(|c| c.is_ascii_digit()) {
            theme::text(ui, "El ID de la aplicación solo lleva dígitos.", theme::medium(12.0), palette.danger);
        } else {
            let mut scopes = Vec::new();
            if st.scope_bot || !st.scope_commands {
                // Sin ningún scope Discord no deja invitar: se usa `bot`.
                scopes.push("bot");
            }
            if st.scope_commands {
                scopes.push("applications.commands");
            }
            let url = format!(
                "https://discord.com/oauth2/authorize?client_id={client_id}&permissions={}&scope={}",
                st.perms,
                scopes.join("%20"),
            );
            ui.label(RichText::new(url.clone()).font(egui::FontId::monospace(11.5)).color(palette.secondary));
            ui.add_space(6.0);
            copy_button(ui, palette, &mut st.copied, "invite", "Copiar enlace", url);
        }
    });
}
