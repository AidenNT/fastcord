//! Páginas de Ajustes que EDITAN la cuenta, con el aspecto del borrador de
//! interfaz: Mi cuenta, Perfiles, Privacidad, Notificaciones, Dispositivos y
//! los interruptores de "Mensajes y contenido" de Apariencia.
//!
//! Mismo patrón que el resto del panel (`ui::settings`): estas funciones solo
//! DIBUJAN a partir de un [`Snapshot`] ya copiado de `App` y devuelven, como
//! mucho, un [`PageAction`] que `settings::show` aplica después de dibujar. Lo
//! que se está escribiendo (los borradores de los campos) vive en la memoria
//! temporal de egui, no en `App`.
//!
//! Qué se manda y a dónde (ver `lib::state::account_edit`):
//!
//! * nombre visible, usuario, correo, contraseña y avatar → `PATCH /users/@me`
//! * "Sobre mí", pronombres, color del banner y banner → `PATCH /users/@me/profile`
//! * los interruptores y "quién puede enviarte solicitudes" → `settings-proto`
//! * cerrar la sesión de otro dispositivo → `POST /auth/sessions/logout`

use std::collections::HashMap;

use egui::{pos2, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use serde_json::{json, Value};

use super::SettingsTab;
use crate::discord::models::User;
use crate::discord::user_settings::PreloadedUserSettings;
use crate::discord::{AccountEdit, AccountEditTarget, AuthSession};
use crate::lib::state::App;
use crate::ui::theme::{self, Icon, Palette};

/// Verde de los interruptores encendidos y de los estados "activado".
const OK: Color32 = Color32::from_rgb(0x23, 0xa5, 0x5a);

// --- Interruptores de la cuenta ---------------------------------------------

/// Genera el enum de interruptores y, para cada uno, dónde vive dentro del
/// `PreloadedUserSettings` (`sección.campo`). Todos son `Option<bool>` en el
/// proto: mandar `Some(false)` SÍ llega a Discord (un `bool` plano en `false`
/// no se codifica y no se podría apagar con un parche parcial).
macro_rules! toggles {
    ($( $variant:ident => ($section:ident . $field:ident, $title:expr, $desc:expr, $default:expr) ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Toggle { $($variant),* }

        impl Toggle {
            pub const ALL: &'static [Toggle] = &[$(Toggle::$variant),*];

            pub fn title(self) -> &'static str {
                match self { $(Toggle::$variant => $title),* }
            }

            pub fn description(self) -> &'static str {
                match self { $(Toggle::$variant => $desc),* }
            }

            /// Lo que vale cuando la cuenta nunca lo guardó.
            pub fn default_value(self) -> bool {
                match self { $(Toggle::$variant => $default),* }
            }

            pub fn get(self, s: &PreloadedUserSettings) -> Option<bool> {
                match self { $(Toggle::$variant => s.$section.as_ref().and_then(|x| x.$field)),* }
            }

            pub fn set(self, s: &mut PreloadedUserSettings, value: bool) {
                match self {
                    $(Toggle::$variant => {
                        s.$section.get_or_insert_with(Default::default).$field = Some(value);
                    }),*
                }
            }

            /// Copia a `to` la sección ENTERA (`privacy`, `notifications`...) donde
            /// vive este interruptor. Discord reemplaza cada sección de primer
            /// nivel completa en un `PATCH`: mandar un solo campo reiniciaría todo
            /// lo demás de esa sección, así que siempre se manda entera.
            pub fn copy_section(self, from: &PreloadedUserSettings, to: &mut PreloadedUserSettings) {
                match self {
                    $(Toggle::$variant => { to.$section = from.$section.clone(); }),*
                }
            }

            /// ¿Trae `s` la sección de este interruptor?
            pub fn has_section(self, s: &PreloadedUserSettings) -> bool {
                match self { $(Toggle::$variant => s.$section.is_some()),* }
            }
        }
    };
}

toggles! {
    // Actividad y privacidad
    ShowCurrentGame => (status.show_current_game, "Compartir actividad detectada", "Muestra a qué juegas o qué escuchas", true),
    RecentGames => (privacy.recent_games_enabled, "Mostrar juegos recientes", "Tus juegos recientes aparecen en tu perfil", true),
    PartyFriends => (privacy.allow_activity_party_privacy_friends, "Invitaciones a tu actividad (amigos)", "Tus amigos pueden unirse a lo que estás haciendo", true),
    PartyVoice => (privacy.allow_activity_party_privacy_voice_channel, "Invitaciones a tu actividad (voz)", "Quienes están en tu canal de voz pueden unirse", true),
    ShowLocalTime => (privacy.show_local_time, "Mostrar la hora local", "Muestra las horas en tu hora local", true),
    HideLegacyUsername => (privacy.hide_legacy_username, "Ocultar el nombre de usuario antiguo", "", false),
    MessageRequestsRestricted => (privacy.default_message_request_restricted, "Restringir solicitudes de mensajes", "Desactiva las solicitudes de mensajes en los servidores nuevos", false),
    InappropriateWarnings => (privacy.inappropriate_conversation_warnings, "Avisos de conversaciones inapropiadas", "Avisa si un mensaje directo parece inapropiado", true),
    ContactSync => (privacy.contact_sync_enabled, "Sincronizar contactos", "Encuentra amigos con los contactos de tu teléfono", false),
    DetectPlatformAccounts => (privacy.detect_platform_accounts, "Detectar cuentas de otras plataformas", "Sugiere conectar las cuentas que Discord encuentre", true),
    // Notificaciones
    InAppNotifications => (notifications.show_in_app_notifications, "Notificaciones dentro de la app", "Avisos mientras usas eCord", true),
    QuietMode => (notifications.quiet_mode, "Modo silencioso", "Silencia los avisos", false),
    VoiceActivityNotifications => (notifications.enable_voice_activity_notifications, "Actividad de voz", "Avisa cuando tus amigos entran a un canal de voz", true),
    FriendOnlineNotifications => (notifications.enable_friend_online_notifications, "Amigos que se conectan", "", true),
    FriendAnniversaryNotifications => (notifications.enable_friend_anniversary_notifications, "Aniversarios de amistad", "", true),
    UpcomingEventNotifications => (notifications.enable_upcoming_server_event_notifications, "Eventos de servidor próximos", "", true),
    GoLiveNotifications => (notifications.notify_friends_on_go_live, "Avisar a mis amigos cuando hago directo", "", false),
    // Mensajes y contenido
    CompactMessages => (text_and_images.message_display_compact, "Modo compacto de mensajes", "Más mensajes visibles a la vez", false),
    GifAutoPlay => (text_and_images.gif_auto_play, "Reproducir GIFs automáticamente", "Autoplay en el chat", true),
    AnimateEmoji => (text_and_images.animate_emoji, "Animar emojis", "Reproduce los emojis animados", true),
    RenderEmbeds => (text_and_images.render_embeds, "Mostrar vistas previas de enlaces", "Los enlaces compartidos muestran su contenido", true),
    RenderReactions => (text_and_images.render_reactions, "Mostrar reacciones", "", true),
    InlineAttachmentMedia => (text_and_images.inline_attachment_media, "Mostrar adjuntos como imagen o video", "", true),
    ConvertEmoticons => (text_and_images.convert_emoticons, "Convertir emoticones en emojis", "", true),
}

// --- Tipos que cruzan con `settings::show` ----------------------------------

/// A qué imagen del perfil se refiere el selector de archivos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileImage {
    Avatar,
    Banner,
}

impl ProfileImage {
    pub fn dialog_title(self) -> &'static str {
        match self {
            ProfileImage::Avatar => "Elegir avatar",
            ProfileImage::Banner => "Elegir banner",
        }
    }

    /// El cambio que pone (`value` = data URI) o quita (`Value::Null`) la imagen.
    pub fn edit_with(self, value: Value) -> AccountEdit {
        let removing = value.is_null();
        match self {
            ProfileImage::Avatar => AccountEdit {
                target: AccountEditTarget::Me,
                body: json!({ "avatar": value }),
                done: if removing { "Avatar quitado" } else { "Avatar actualizado" },
            },
            ProfileImage::Banner => AccountEdit {
                target: AccountEditTarget::Profile,
                body: json!({ "banner": value }),
                done: if removing { "Banner quitado" } else { "Banner actualizado" },
            },
        }
    }
}

/// Lo que puede pedir una de estas páginas en un frame.
pub enum PageAction {
    /// Mandar uno o varios cambios de cuenta (`App::submit_account_edits`).
    Edits(Vec<AccountEdit>),
    /// Abrir el selector de archivos para el avatar o el banner.
    PickImage(ProfileImage),
    /// Encender o apagar un interruptor de la cuenta.
    Toggle(Toggle, bool),
    /// "Quién puede enviarte solicitudes de amistad" (`friend_source_flags`).
    FriendSource(u32),
    /// Ir a otra página de Ajustes.
    GoTab(SettingsTab),
    /// Volver a pedir los dispositivos con sesión abierta.
    RefreshSessions,
    /// Cerrar la sesión de un dispositivo (`id_hash`).
    LogoutSession(String),
}

/// Todo lo que estas páginas necesitan de `App`, copiado antes de dibujar.
pub struct Snapshot {
    pub me: Option<User>,
    pub avatar_url: Option<String>,
    pub values: HashMap<Toggle, bool>,
    pub friend_flags: Option<u32>,
    pub busy: bool,
    pub has_nitro: Option<bool>,
    pub sessions: Option<Vec<AuthSession>>,
    pub sessions_loading: bool,
    pub sessions_error: Option<String>,
}

impl Snapshot {
    pub fn build(app: &App) -> Self {
        let settings = app.discord_settings.as_ref();
        let values = Toggle::ALL
            .iter()
            .map(|t| (*t, settings.and_then(|s| t.get(s)).unwrap_or_else(|| t.default_value())))
            .collect();
        let friend_flags = settings
            .and_then(|s| s.privacy.as_ref())
            .and_then(|p| p.friend_source_flags);
        let avatar_url = app.me.as_ref().and_then(|m| m.avatar_url());
        Self {
            me: app.me.clone(),
            avatar_url,
            values,
            friend_flags,
            busy: app.account_ui.busy(),
            has_nitro: app.has_nitro(),
            sessions: app.account_ui.sessions.clone(),
            sessions_loading: app.account_ui.sessions_loading,
            sessions_error: app.account_ui.sessions_error.clone(),
        }
    }
}

// --- Piezas visuales del borrador -------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Normal,
    Primary,
    Danger,
}

/// Botón del borrador (`.btn`, `.btn.p`, `.btn.d`): radio 10, relleno suave,
/// acento o contorno rojo. Con `enabled = false` no responde al clic.
fn btn(ui: &mut Ui, palette: &Palette, label: &str, kind: BtnKind, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let probe = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::semibold(13.0), palette.text);
    let size = probe.size() + Vec2::new(28.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && resp.hovered();
    let (fg, fill, stroke) = match kind {
        BtnKind::Normal => (
            palette.text,
            if hovered { palette.surface_active } else { palette.surface_hover },
            Stroke::NONE,
        ),
        BtnKind::Primary => (
            palette.on_accent,
            if hovered { palette.accent_hover } else { palette.accent },
            Stroke::NONE,
        ),
        BtnKind::Danger => (
            if hovered { Color32::WHITE } else { palette.danger },
            if hovered { palette.danger } else { Color32::TRANSPARENT },
            Stroke::new(1.0, palette.danger),
        ),
    };
    let (fg, fill) = if enabled {
        (fg, fill)
    } else {
        (palette.dim, palette.surface)
    };
    ui.painter().rect_filled(rect, 10.0, fill);
    if stroke != Stroke::NONE && enabled {
        ui.painter().rect_stroke(rect, 10.0, stroke, StrokeKind::Inside);
    }
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::semibold(13.0), fg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Interruptor del borrador (`.sw`): 42×24, verde encendido.
fn switch(ui: &mut Ui, palette: &Palette, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(42.0, 24.0), Sense::click());
    let fill = if on { OK } else { palette.surface_active };
    ui.painter().rect_filled(rect, 12.0, fill);
    let knob_x = if on { rect.right() - 12.0 } else { rect.left() + 12.0 };
    ui.painter()
        .circle_filled(pos2(knob_x, rect.center().y), 9.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Título de grupo (`h2` del borrador).
fn h2(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.add_space(20.0);
    theme::text(ui, text, theme::bold(15.0), palette.text);
    ui.add_space(10.0);
}

/// Tarjeta (`.card`): panel con borde y radio 16.
fn card<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Tarjeta con un bloque de filas adentro (`.card > .rows`).
fn card_rows<R>(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    card(ui, palette, |ui| rows_block(ui, palette, Margin::same(14), add))
}

fn rows_block<R>(ui: &mut Ui, palette: &Palette, outer: Margin, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(16, 4))
        .outer_margin(outer)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Una fila (`.row`): título y detalle a la izquierda, controles a la
/// derecha. `right` dibuja de derecha a izquierda (lo primero que agrega
/// queda más pegado al borde).
fn row(ui: &mut Ui, palette: &Palette, title: &str, sub: &str, last: bool, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(54.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            right(ui);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .font(theme::medium(14.0))
                                .color(palette.text),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                    if !sub.is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(sub)
                                    .font(theme::regular(12.0))
                                    .color(palette.secondary),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    }
                });
            });
        });
    });
    if !last {
        ui.separator();
    }
}

/// Etiqueta chica de un campo (`.fld label`).
fn field_label(ui: &mut Ui, palette: &Palette, text: &str) {
    theme::text(ui, text, theme::semibold(11.5), palette.secondary);
    ui.add_space(5.0);
}

/// Campo de texto de una línea con el aspecto del borrador.
fn input(ui: &mut Ui, palette: &Palette, text: &mut String, hint: &str, password: bool, limit: usize) -> egui::Response {
    ui.scope(|ui| {
        style_input(ui, palette);
        let edit = egui::TextEdit::singleline(text)
            .hint_text(hint)
            .password(password)
            .char_limit(limit)
            .font(theme::regular(14.0))
            .margin(Margin::symmetric(12, 9))
            .desired_width(f32::INFINITY);
        ui.add(edit)
    })
    .inner
}

fn style_input(ui: &mut Ui, palette: &Palette) {
    let v = ui.visuals_mut();
    v.extreme_bg_color = palette.window_solid;
    let radius = CornerRadius::same(10);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = radius;
        w.bg_stroke = Stroke::new(1.0, palette.outline);
    }
    v.selection.stroke = Stroke::new(1.5, palette.accent);
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn toggles_card(ui: &mut Ui, palette: &Palette, snap: &Snapshot, list: &[Toggle]) -> Option<PageAction> {
    let mut action = None;
    card_rows(ui, palette, |ui| {
        for (i, t) in list.iter().enumerate() {
            let on = snap.values.get(t).copied().unwrap_or(false);
            row(ui, palette, t.title(), t.description(), i + 1 == list.len(), |ui| {
                if switch(ui, palette, on).clicked() {
                    action = Some(PageAction::Toggle(*t, !on));
                }
            });
        }
    });
    action
}

// --- Mi cuenta ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    DisplayName,
    Username,
    Email,
    Password,
}

/// Lo que se está escribiendo en la fila abierta de Mi cuenta.
#[derive(Clone, Default)]
struct EditState {
    field: Option<Field>,
    text: String,
    password: String,
    new_password: String,
    confirm: String,
    error: Option<String>,
    reveal_email: bool,
}

/// Abre (o cierra, si ya estaba abierta) la edición de una fila.
fn toggle_field(st: &mut EditState, field: Field, initial: &str) {
    if st.field == Some(field) {
        st.field = None;
        return;
    }
    *st = EditState {
        field: Some(field),
        text: initial.to_owned(),
        reveal_email: st.reveal_email,
        ..Default::default()
    };
}

fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => {
            let first: String = user.chars().take(1).collect();
            format!("{first}•••••@{domain}")
        }
        None => "•••••".to_string(),
    }
}

/// Revisa lo escrito y arma el cambio, o devuelve el error para mostrar.
fn validate(field: Field, st: &EditState) -> Result<AccountEdit, String> {
    match field {
        Field::DisplayName => {
            let name = st.text.trim();
            if name.chars().count() > 32 {
                return Err("El nombre visible puede tener hasta 32 caracteres.".into());
            }
            let value = if name.is_empty() { Value::Null } else { json!(name) };
            Ok(AccountEdit {
                target: AccountEditTarget::Me,
                body: json!({ "global_name": value }),
                done: "Nombre visible actualizado",
            })
        }
        Field::Username => {
            let name = st.text.trim().to_lowercase();
            let valid = (2..=32).contains(&name.chars().count())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
            if !valid {
                return Err("Usa de 2 a 32 caracteres: letras minúsculas, números, guion bajo o punto.".into());
            }
            if st.password.is_empty() {
                return Err("Escribe tu contraseña actual para confirmar el cambio.".into());
            }
            Ok(AccountEdit {
                target: AccountEditTarget::Me,
                body: json!({ "username": name, "password": st.password }),
                done: "Nombre de usuario actualizado",
            })
        }
        Field::Email => {
            let email = st.text.trim();
            if !email.contains('@') || !email.contains('.') {
                return Err("Escribe un correo válido.".into());
            }
            if st.password.is_empty() {
                return Err("Escribe tu contraseña actual para confirmar el cambio.".into());
            }
            Ok(AccountEdit {
                target: AccountEditTarget::Me,
                body: json!({ "email": email, "password": st.password }),
                done: "Correo actualizado",
            })
        }
        Field::Password => {
            if st.password.is_empty() {
                return Err("Escribe tu contraseña actual.".into());
            }
            if st.new_password.chars().count() < 8 {
                return Err("La contraseña nueva debe tener al menos 8 caracteres.".into());
            }
            if st.new_password != st.confirm {
                return Err("Las contraseñas nuevas no coinciden.".into());
            }
            Ok(AccountEdit {
                target: AccountEditTarget::Me,
                body: json!({ "password": st.password, "new_password": st.new_password }),
                done: "Contraseña cambiada",
            })
        }
    }
}

/// El formulario que se abre debajo de la fila. Devuelve el cambio listo si se
/// apretó "Guardar" y todo está en orden.
fn edit_panel(ui: &mut Ui, palette: &Palette, st: &mut EditState, busy: bool) -> Option<AccountEdit> {
    let field = st.field?;
    let mut result: Option<AccountEdit> = None;
    Frame::new()
        .fill(palette.panel)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match field {
                Field::DisplayName => {
                    field_label(ui, palette, "NOMBRE VISIBLE");
                    input(ui, palette, &mut st.text, "Cómo te ven los demás", false, 32);
                    ui.add_space(4.0);
                    theme::subtle(ui, palette, "Déjalo vacío para mostrar tu nombre de usuario.");
                }
                Field::Username => {
                    field_label(ui, palette, "NUEVO NOMBRE DE USUARIO");
                    input(ui, palette, &mut st.text, "usuario", false, 32);
                    ui.add_space(10.0);
                    field_label(ui, palette, "CONTRASEÑA ACTUAL");
                    input(ui, palette, &mut st.password, "", true, 128);
                }
                Field::Email => {
                    field_label(ui, palette, "NUEVO CORREO ELECTRÓNICO");
                    input(ui, palette, &mut st.text, "correo@ejemplo.com", false, 128);
                    ui.add_space(10.0);
                    field_label(ui, palette, "CONTRASEÑA ACTUAL");
                    input(ui, palette, &mut st.password, "", true, 128);
                }
                Field::Password => {
                    field_label(ui, palette, "CONTRASEÑA ACTUAL");
                    input(ui, palette, &mut st.password, "", true, 128);
                    ui.add_space(10.0);
                    field_label(ui, palette, "CONTRASEÑA NUEVA");
                    input(ui, palette, &mut st.new_password, "", true, 128);
                    ui.add_space(10.0);
                    field_label(ui, palette, "REPETIR CONTRASEÑA NUEVA");
                    input(ui, palette, &mut st.confirm, "", true, 128);
                }
            }
            if let Some(error) = st.error.clone() {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(error)
                            .font(theme::regular(12.5))
                            .color(palette.danger),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if btn(ui, palette, "Guardar", BtnKind::Primary, !busy).clicked() {
                    match validate(field, st) {
                        Ok(edit) => result = Some(edit),
                        Err(message) => st.error = Some(message),
                    }
                }
                if btn(ui, palette, "Cancelar", BtnKind::Normal, true).clicked() {
                    st.field = None;
                }
            });
        });
    ui.add_space(8.0);
    if result.is_some() {
        // Lo escrito (sobre todo las contraseñas) no se queda en memoria.
        let reveal_email = st.reveal_email;
        *st = EditState {
            reveal_email,
            ..Default::default()
        };
    }
    result
}

/// Banner + avatar + nombre, como la cabecera de la tarjeta del borrador. `right`
/// dibuja los botones a la derecha del nombre.
fn profile_header(
    ui: &mut Ui,
    palette: &Palette,
    me: &User,
    avatar_url: Option<&str>,
    right: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    let banner_color = me.accent_color.map_or(palette.accent, rgb);
    let old_spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let (banner, _) = ui.allocate_exact_size(Vec2::new(width, 96.0), Sense::hover());
    ui.painter().rect_filled(
        banner,
        CornerRadius {
            nw: 15,
            ne: 15,
            sw: 0,
            se: 0,
        },
        banner_color,
    );
    let (head, _) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
    let center = pos2(head.left() + 20.0 + 42.0, banner.bottom());
    ui.painter().circle_filled(center, 48.0, palette.panel);
    let initial: String = me
        .display_name()
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default();
    crate::ui::shell::extra::avatar(ui, center, 42.0, avatar_url, palette.accent, &initial, palette);
    ui.painter().text(
        pos2(head.left() + 20.0 + 84.0 + 16.0, head.top() + 22.0),
        egui::Align2::LEFT_CENTER,
        me.display_name(),
        theme::bold(20.0),
        palette.text,
    );
    let area = Rect::from_min_max(pos2(head.right() - 240.0, head.top()), pos2(head.right() - 20.0, head.bottom()));
    let mut right_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::right_to_left(Align::Center)),
    );
    right(&mut right_ui);
    ui.spacing_mut().item_spacing = old_spacing;
}

/// Pestaña "Mi cuenta": tarjeta con tus datos (editables), contraseña y
/// verificación en dos pasos.
pub fn account_view(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    let Some(me) = snap.me.as_ref() else {
        theme::subtle(ui, palette, "Todavía no se cargó tu cuenta.");
        return None;
    };
    let mut action: Option<PageAction> = None;
    let id = Id::new("settings_account_edit");
    let mut st: EditState = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();

    card(ui, palette, |ui| {
        profile_header(ui, palette, me, snap.avatar_url.as_deref(), |ui| {
            if btn(ui, palette, "Editar perfil", BtnKind::Primary, true).clicked() {
                action = Some(PageAction::GoTab(SettingsTab::Profile));
            }
        });
        rows_block(
            ui,
            palette,
            Margin {
                left: 20,
                right: 20,
                top: 16,
                bottom: 20,
            },
            |ui| {
                // Nombre visible
                let display = me
                    .global_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| me.username.clone());
                let open = st.field == Some(Field::DisplayName);
                row(ui, palette, "Nombre visible", &display, false, |ui| {
                    let label = if open { "Cancelar" } else { "Editar" };
                    if btn(ui, palette, label, BtnKind::Normal, true).clicked() {
                        toggle_field(&mut st, Field::DisplayName, me.global_name.as_deref().unwrap_or(""));
                    }
                });
                if st.field == Some(Field::DisplayName) {
                    if let Some(edit) = edit_panel(ui, palette, &mut st, snap.busy) {
                        action = Some(PageAction::Edits(vec![edit]));
                    }
                }

                // Nombre de usuario
                let open = st.field == Some(Field::Username);
                row(ui, palette, "Nombre de usuario", &me.username, false, |ui| {
                    let label = if open { "Cancelar" } else { "Editar" };
                    if btn(ui, palette, label, BtnKind::Normal, true).clicked() {
                        toggle_field(&mut st, Field::Username, &me.username);
                    }
                });
                if st.field == Some(Field::Username) {
                    if let Some(edit) = edit_panel(ui, palette, &mut st, snap.busy) {
                        action = Some(PageAction::Edits(vec![edit]));
                    }
                }

                // Correo electrónico
                let email = me.email.clone().unwrap_or_default();
                let shown = if email.is_empty() {
                    "Sin correo".to_string()
                } else if st.reveal_email {
                    email.clone()
                } else {
                    mask_email(&email)
                };
                let open = st.field == Some(Field::Email);
                row(ui, palette, "Correo electrónico", &shown, false, |ui| {
                    let label = if open { "Cancelar" } else { "Editar" };
                    if btn(ui, palette, label, BtnKind::Normal, true).clicked() {
                        toggle_field(&mut st, Field::Email, &email);
                    }
                    if !email.is_empty() {
                        let label = if st.reveal_email { "Ocultar" } else { "Mostrar" };
                        if btn(ui, palette, label, BtnKind::Normal, true).clicked() {
                            st.reveal_email = !st.reveal_email;
                        }
                    }
                });
                if st.field == Some(Field::Email) {
                    if let Some(edit) = edit_panel(ui, palette, &mut st, snap.busy) {
                        action = Some(PageAction::Edits(vec![edit]));
                    }
                }

                // Teléfono (agregarlo necesita un código por SMS)
                let phone = me
                    .phone
                    .clone()
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "Sin teléfono añadido".to_string());
                row(ui, palette, "Teléfono", &phone, true, |ui| {
                    theme::text(ui, "Se cambia en el cliente oficial", theme::regular(12.0), palette.dim);
                });
            },
        );
    });

    h2(ui, palette, "Contraseña y autenticación");
    card_rows(ui, palette, |ui| {
        let open = st.field == Some(Field::Password);
        row(ui, palette, "Contraseña", "Cambia la contraseña de tu cuenta", false, |ui| {
            let label = if open { "Cancelar" } else { "Cambiar contraseña" };
            let kind = if open { BtnKind::Normal } else { BtnKind::Primary };
            if btn(ui, palette, label, kind, true).clicked() {
                toggle_field(&mut st, Field::Password, "");
            }
        });
        if st.field == Some(Field::Password) {
            if let Some(edit) = edit_panel(ui, palette, &mut st, snap.busy) {
                action = Some(PageAction::Edits(vec![edit]));
            }
        }
        let (sub, tag): (&str, Option<(&str, Color32)>) = match me.mfa_enabled {
            Some(true) => (
                "Tu cuenta está protegida con una app de códigos. Se gestiona desde el cliente oficial.",
                Some(("Activada", OK)),
            ),
            Some(false) => (
                "Protege tu cuenta con una app de códigos. Se activa desde el cliente oficial.",
                Some(("Desactivada", palette.dim)),
            ),
            None => ("Se gestiona desde el cliente oficial.", None),
        };
        row(ui, palette, "Autenticación en dos pasos", sub, true, |ui| {
            if let Some((text, color)) = tag {
                theme::text(ui, text, theme::semibold(13.0), color);
            }
        });
    });

    ui.ctx().data_mut(|d| d.insert_temp(id, st));
    action
}

// --- Perfiles -------------------------------------------------------------------

/// Lo que se está escribiendo en el editor de perfil.
#[derive(Clone)]
struct ProfileDraft {
    /// Id de la cuenta a la que pertenece (al cambiar de cuenta se reinicia).
    owner: String,
    name: String,
    pronouns: String,
    bio: String,
    color: Option<u32>,
}

impl ProfileDraft {
    fn from_user(me: &User) -> Self {
        Self {
            owner: me.id.clone(),
            name: me.global_name.clone().unwrap_or_default(),
            pronouns: me.pronouns.clone().unwrap_or_default(),
            bio: me.bio.clone().unwrap_or_default(),
            color: me.accent_color,
        }
    }
}

const BANNER_COLORS: [u32; 6] = [0x8b1a1a, 0x2a5bd7, 0x1f8a5b, 0x8b4ad7, 0xc26a1a, 0x8b8cf5];

/// Pestaña "Perfiles": formulario a la izquierda y vista previa en vivo a la
/// derecha.
pub fn profile_view(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    let Some(me) = snap.me.as_ref() else {
        theme::subtle(ui, palette, "Todavía no se cargó tu perfil.");
        return None;
    };
    let id = Id::new("settings_profile_draft");
    let mut draft: ProfileDraft = ui
        .ctx()
        .data(|d| d.get_temp::<ProfileDraft>(id))
        .filter(|d| d.owner == me.id)
        .unwrap_or_else(|| ProfileDraft::from_user(me));
    let mut action: Option<PageAction> = None;

    let total = ui.available_width();
    if total >= 560.0 {
        let preview_w = 260.0;
        let left_w = total - preview_w - 20.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(left_w, 10.0), Layout::top_down(Align::Min), |ui| {
                profile_form(ui, palette, snap, me, &mut draft, &mut action);
            });
            ui.add_space(20.0);
            ui.allocate_ui_with_layout(Vec2::new(preview_w, 10.0), Layout::top_down(Align::Min), |ui| {
                profile_preview(ui, palette, me, snap.avatar_url.as_deref(), &draft);
            });
        });
    } else {
        profile_form(ui, palette, snap, me, &mut draft, &mut action);
        ui.add_space(16.0);
        profile_preview(ui, palette, me, snap.avatar_url.as_deref(), &draft);
    }

    ui.ctx().data_mut(|d| d.insert_temp(id, draft));
    action
}

fn profile_form(
    ui: &mut Ui,
    palette: &Palette,
    snap: &Snapshot,
    me: &User,
    draft: &mut ProfileDraft,
    action: &mut Option<PageAction>,
) {
    field_label(ui, palette, "NOMBRE VISIBLE");
    input(ui, palette, &mut draft.name, &me.username, false, 32);
    ui.add_space(14.0);

    field_label(ui, palette, "PRONOMBRES");
    input(ui, palette, &mut draft.pronouns, "Añade tus pronombres", false, 40);
    ui.add_space(14.0);

    field_label(ui, palette, "SOBRE MÍ");
    ui.scope(|ui| {
        style_input(ui, palette);
        ui.add(
            egui::TextEdit::multiline(&mut draft.bio)
                .char_limit(190)
                .desired_rows(4)
                .font(theme::regular(14.0))
                .margin(Margin::symmetric(12, 9))
                .desired_width(f32::INFINITY),
        );
    });
    theme::text(
        ui,
        format!("{}/190", draft.bio.chars().count()),
        theme::regular(11.5),
        palette.dim,
    );
    ui.add_space(14.0);

    field_label(ui, palette, "COLOR DEL BANNER");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for color in BANNER_COLORS {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
            ui.painter().circle_filled(rect.center(), 12.0, rgb(color));
            if draft.color == Some(color) {
                ui.painter()
                    .circle_stroke(rect.center(), 14.0, Stroke::new(2.0, palette.text));
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                draft.color = Some(color);
            }
        }
    });
    ui.add_space(14.0);

    field_label(ui, palette, "AVATAR Y BANNER");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        if btn(ui, palette, "Cambiar avatar", BtnKind::Normal, !snap.busy).clicked() {
            *action = Some(PageAction::PickImage(ProfileImage::Avatar));
        }
        if me.avatar.is_some()
            && btn(ui, palette, "Quitar avatar", BtnKind::Normal, !snap.busy).clicked()
        {
            *action = Some(PageAction::Edits(vec![ProfileImage::Avatar.edit_with(Value::Null)]));
        }
        let banner_ok = snap.has_nitro != Some(false);
        let banner_btn = btn(ui, palette, "Cambiar banner", BtnKind::Normal, banner_ok && !snap.busy);
        if !banner_ok {
            banner_btn.on_hover_text("El banner de imagen necesita Nitro.");
        } else if banner_btn.clicked() {
            *action = Some(PageAction::PickImage(ProfileImage::Banner));
        }
    });
    ui.add_space(20.0);

    // Qué cambió respecto de lo guardado.
    let name_changed = draft.name.trim() != me.global_name.as_deref().unwrap_or("");
    let pronouns_changed = draft.pronouns.trim() != me.pronouns.as_deref().unwrap_or("");
    let bio_changed = draft.bio != me.bio.as_deref().unwrap_or("");
    let color_changed = draft.color.is_some() && draft.color != me.accent_color;
    let dirty = name_changed || pronouns_changed || bio_changed || color_changed;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if btn(ui, palette, "Guardar cambios", BtnKind::Primary, dirty && !snap.busy).clicked() {
            let mut edits = Vec::new();
            if name_changed {
                let name = draft.name.trim();
                let value = if name.is_empty() { Value::Null } else { json!(name) };
                edits.push(AccountEdit {
                    target: AccountEditTarget::Me,
                    body: json!({ "global_name": value }),
                    done: "Nombre visible actualizado",
                });
            }
            if pronouns_changed || bio_changed || color_changed {
                let mut body = serde_json::Map::new();
                if pronouns_changed {
                    body.insert("pronouns".into(), json!(draft.pronouns.trim()));
                }
                if bio_changed {
                    body.insert("bio".into(), json!(draft.bio));
                }
                if let (true, Some(color)) = (color_changed, draft.color) {
                    body.insert("accent_color".into(), json!(color));
                }
                edits.push(AccountEdit {
                    target: AccountEditTarget::Profile,
                    body: Value::Object(body),
                    done: "Perfil actualizado",
                });
            }
            *action = Some(PageAction::Edits(edits));
        }
        if btn(ui, palette, "Restablecer", BtnKind::Normal, dirty).clicked() {
            *draft = ProfileDraft::from_user(me);
        }
    });
}

/// Tu tarjeta de perfil tal como queda con lo que se está escribiendo.
fn profile_preview(ui: &mut Ui, palette: &Palette, me: &User, avatar_url: Option<&str>, draft: &ProfileDraft) {
    let width = ui.available_width();
    let banner_color = draft.color.map_or(palette.accent, rgb);
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(16))
        .show(ui, |ui| {
            ui.set_width(width - 2.0);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let (banner, _) = ui.allocate_exact_size(Vec2::new(width - 2.0, 70.0), Sense::hover());
            ui.painter().rect_filled(
                banner,
                CornerRadius {
                    nw: 15,
                    ne: 15,
                    sw: 0,
                    se: 0,
                },
                banner_color,
            );
            let (head, _) = ui.allocate_exact_size(Vec2::new(width - 2.0, 36.0), Sense::hover());
            let center = pos2(head.left() + 14.0 + 32.0, banner.bottom());
            ui.painter().circle_filled(center, 38.0, palette.panel);
            let name = if draft.name.trim().is_empty() {
                me.username.as_str()
            } else {
                draft.name.trim()
            };
            let initial: String = name.chars().next().map(|c| c.to_string()).unwrap_or_default();
            crate::ui::shell::extra::avatar(ui, center, 32.0, avatar_url, palette.accent, &initial, palette);
            Frame::new().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.set_width(width - 2.0 - 28.0);
                theme::text(ui, name, theme::bold(18.0), palette.text);
                theme::text(ui, format!("@{}", me.username), theme::regular(13.0), palette.secondary);
                if !draft.pronouns.trim().is_empty() {
                    ui.add_space(4.0);
                    theme::text(ui, draft.pronouns.trim(), theme::regular(12.5), palette.secondary);
                }
                if !draft.bio.is_empty() {
                    ui.add_space(10.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&draft.bio)
                                .font(theme::regular(13.0))
                                .color(palette.text),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            });
        });
}

// --- Privacidad y notificaciones ------------------------------------------------

/// Bits de `friend_source_flags` (docs.discord.food, "Friend Source Flags").
const MUTUAL_FRIENDS: u32 = 1 << 1;
const MUTUAL_GUILDS: u32 = 1 << 2;
/// Cualquiera puede agregarte; Discord exige que vaya junto a los otros dos.
const NO_RELATION: u32 = 1 << 3;
const EVERYONE: u32 = MUTUAL_FRIENDS | MUTUAL_GUILDS | NO_RELATION;

/// Opciones de "quién puede enviarte solicitudes de amistad".
const FRIEND_SOURCES: [(u32, &str); 5] = [
    (EVERYONE, "Todos"),
    (MUTUAL_FRIENDS | MUTUAL_GUILDS, "Amigos de amigos y miembros de servidores"),
    (MUTUAL_FRIENDS, "Solo amigos de amigos"),
    (MUTUAL_GUILDS, "Solo miembros de servidores"),
    (0, "Nadie"),
];

pub fn privacy_view(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    let mut action = None;

    h2(ui, palette, "Mensajes directos");
    if let Some(a) = toggles_card(
        ui,
        palette,
        snap,
        &[Toggle::MessageRequestsRestricted, Toggle::InappropriateWarnings],
    ) {
        action = Some(a);
    }

    h2(ui, palette, "Amistades");
    // Sin dato guardado Discord asume "todos".
    let current = match snap.friend_flags {
        Some(f) if f & NO_RELATION != 0 => EVERYONE,
        Some(f) => f & (MUTUAL_FRIENDS | MUTUAL_GUILDS),
        None => EVERYONE,
    };
    let current_label = FRIEND_SOURCES
        .iter()
        .find(|(flags, _)| *flags == current)
        .map_or("Todos", |(_, label)| *label);
    card_rows(ui, palette, |ui| {
        row(ui, palette, "Quién puede enviarte solicitudes", "Elige el alcance", true, |ui| {
            egui::ComboBox::from_id_salt("settings_friend_source")
                .selected_text(current_label)
                .width(250.0)
                .show_ui(ui, |ui| {
                    for (flags, label) in FRIEND_SOURCES {
                        if ui.selectable_label(current == flags, label).clicked() && current != flags {
                            action = Some(PageAction::FriendSource(flags));
                        }
                    }
                });
        });
    });

    h2(ui, palette, "Actividad");
    if let Some(a) = toggles_card(
        ui,
        palette,
        snap,
        &[
            Toggle::ShowCurrentGame,
            Toggle::RecentGames,
            Toggle::PartyFriends,
            Toggle::PartyVoice,
            Toggle::ShowLocalTime,
        ],
    ) {
        action = Some(a);
    }

    h2(ui, palette, "Otros");
    if let Some(a) = toggles_card(
        ui,
        palette,
        snap,
        &[
            Toggle::ContactSync,
            Toggle::DetectPlatformAccounts,
            Toggle::HideLegacyUsername,
        ],
    ) {
        action = Some(a);
    }
    action
}

pub fn notifications_view(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    toggles_card(
        ui,
        palette,
        snap,
        &[
            Toggle::InAppNotifications,
            Toggle::QuietMode,
            Toggle::VoiceActivityNotifications,
            Toggle::FriendOnlineNotifications,
            Toggle::FriendAnniversaryNotifications,
            Toggle::UpcomingEventNotifications,
            Toggle::GoLiveNotifications,
        ],
    )
}

/// Interruptores de mensajes y contenido que se suman a Apariencia.
pub fn message_toggles(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    h2(ui, palette, "Mensajes y contenido");
    toggles_card(
        ui,
        palette,
        snap,
        &[
            Toggle::CompactMessages,
            Toggle::GifAutoPlay,
            Toggle::AnimateEmoji,
            Toggle::RenderEmbeds,
            Toggle::RenderReactions,
            Toggle::InlineAttachmentMedia,
            Toggle::ConvertEmoticons,
        ],
    )
}

// --- Dispositivos -----------------------------------------------------------------

/// "activo ahora", "hace 5 min", "hace 2 h", "hace 3 d".
fn ago(iso: &str) -> String {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - when.timestamp()).max(0);
    match secs {
        0..=119 => "activo ahora".to_string(),
        120..=3599 => format!("hace {} min", secs / 60),
        3600..=86_399 => format!("hace {} h", secs / 3600),
        _ => format!("hace {} d", secs / 86_400),
    }
}

fn session_icon(session: &AuthSession) -> Icon {
    let p = session.platform.to_lowercase();
    if p.contains("ios") || p.contains("android") {
        Icon::Phone
    } else if p.contains("client") || p.contains("desktop") {
        Icon::Laptop
    } else {
        Icon::Globe
    }
}

pub fn devices_view(ui: &mut Ui, palette: &Palette, snap: &Snapshot) -> Option<PageAction> {
    let mut action = None;
    let confirm_id = Id::new("settings_session_confirm");
    let mut confirm: String = ui.ctx().data(|d| d.get_temp(confirm_id)).unwrap_or_default();

    ui.horizontal(|ui| {
        ui.set_min_height(34.0);
        let hint = if snap.sessions_loading {
            "Actualizando…"
        } else {
            "Dispositivos con sesión abierta en tu cuenta de Discord."
        };
        theme::text(ui, hint, theme::regular(13.0), palette.secondary);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if btn(ui, palette, "Actualizar", BtnKind::Normal, !snap.sessions_loading).clicked() {
                action = Some(PageAction::RefreshSessions);
            }
        });
    });
    ui.add_space(10.0);

    if let Some(error) = snap.sessions_error.as_deref() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(error)
                    .font(theme::regular(12.5))
                    .color(palette.danger),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(10.0);
    }

    match snap.sessions.as_deref() {
        None => {
            if snap.sessions_error.is_none() {
                theme::subtle(ui, palette, "Cargando dispositivos…");
            }
        }
        Some([]) => {
            theme::subtle(ui, palette, "No hay otras sesiones abiertas.");
        }
        Some(list) => {
            card_rows(ui, palette, |ui| {
                for (i, session) in list.iter().enumerate() {
                    let title = match (session.os.is_empty(), session.platform.is_empty()) {
                        (false, false) => format!("{} · {}", session.os, session.platform),
                        (false, true) => session.os.clone(),
                        (true, false) => session.platform.clone(),
                        (true, true) => "Dispositivo desconocido".to_string(),
                    };
                    let place = [session.location.clone(), ago(&session.last_used)]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let asking = confirm == session.id_hash;
                    ui.horizontal(|ui| {
                        ui.set_min_height(58.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if asking { "¿Seguro?" } else { "Cerrar sesión" };
                            if btn(ui, palette, label, BtnKind::Danger, !snap.sessions_loading).clicked() {
                                if asking {
                                    action = Some(PageAction::LogoutSession(session.id_hash.clone()));
                                    confirm.clear();
                                } else {
                                    confirm = session.id_hash.clone();
                                }
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                                ui.painter()
                                    .rect_filled(icon_rect, 10.0, palette.surface_hover);
                                theme::paint_icon(ui, session_icon(session), icon_rect, 18.0, palette.text);
                                ui.add_space(12.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    theme::text(ui, title.clone(), theme::medium(14.0), palette.text);
                                    if !place.is_empty() {
                                        theme::text(ui, place.clone(), theme::regular(12.0), palette.secondary);
                                    }
                                });
                            });
                        });
                    });
                    if i + 1 < list.len() {
                        ui.separator();
                    }
                }
            });
            ui.add_space(12.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Cuidado: si cierras la sesión de este mismo equipo, eCord te pedirá volver a entrar. \
                         Si no reconoces un dispositivo, ciérralo y cambia tu contraseña.",
                    )
                    .font(theme::regular(12.0))
                    .color(palette.dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    }

    ui.ctx().data_mut(|d| d.insert_temp(confirm_id, confirm));
    action
}
