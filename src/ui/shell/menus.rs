//! Menús del borrador de interfaz: menú del servidor (clic derecho en su ícono
//! de la barra lateral), menú de canal (clic derecho en la lista de canales) y
//! el menú del perfil propio (clic en tu perfil de la barra de usuario).
//!
//! Mismo patrón que `ui::call::audio_menu`: los lugares donde se dibujan los
//! menús no siempre pueden mutar `App`, así que cada fila empuja una
//! [`MenuAction`] a `ctx.memory` y `App::ui` las aplica una vez por frame con
//! [`apply`] (que también dibuja el menú del perfil propio).
//!
//! Lo que el backend todavía no sabe hacer (silenciar un servidor, salir de
//! él, invitar, ajustes de notificaciones por canal…) está en el menú igual,
//! como en el borrador, y avisa con un toast: es lo mismo que hace el resto de
//! la app con `ChatEvent::Unavailable`.

use egui::{
    Align2, Area, Color32, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};

use crate::lib::state::{App, ToastKind};
use crate::ui::chat::anim;
use crate::ui::chat::markdown::{self, MentionCtx};
use crate::ui::profile_popup::{self, Tone};
use crate::ui::settings::SettingsTab;
use crate::ui::shell::extra;
use crate::ui::theme::{self, Icon, Palette};

const MENU_W: f32 = 240.0;
const ROW_H: f32 = 34.0;
const PAD: f32 = 10.0;

/// Algo que pidió un menú. `apply` lo ejecuta con acceso a `App`.
#[derive(Clone, Debug)]
pub enum MenuAction {
    /// Copiar texto al portapapeles y avisar con un toast.
    Copy { text: String, what: &'static str },
    /// Función todavía sin soporte en el backend.
    Unavailable(&'static str),
    /// Abrir Ajustes en una pestaña.
    OpenSettings(SettingsTab),
    ToggleMute,
    ToggleDeafen,
    /// Cambiar tu estado de presencia ("online", "idle", "dnd", "invisible").
    SetStatus(&'static str),
    /// Ir a la llamada de voz en curso.
    OpenVoice,
    SwitchAccount(String),
    AddAccount,
    LogOut,
}

fn actions_id() -> Id {
    Id::new("ecord_shell_menu_actions")
}

fn push(ctx: &egui::Context, action: MenuAction) {
    ctx.memory_mut(|m| {
        m.data.get_temp_mut_or_default::<Vec<MenuAction>>(actions_id()).push(action);
    });
}

fn take(ctx: &egui::Context) -> Vec<MenuAction> {
    ctx.memory_mut(|m| m.data.remove_temp::<Vec<MenuAction>>(actions_id()))
        .unwrap_or_default()
}

// --- Piezas comunes -------------------------------------------------------

/// Fila del menú (mismo aspecto que `audio_menu`). `check` dibuja la casilla
/// a la derecha; `chevron` la flecha de submenú.
fn row(ui: &mut Ui, p: &Palette, label: &str, color: Color32, check: Option<bool>, chevron: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(theme::radius_small()), p.surface_hover);
        }
        ui.painter().text(
            Pos2::new(rect.left() + PAD, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::medium(14.0),
            color,
        );
        let right = rect.right() - PAD;
        if chevron {
            let r = Rect::from_center_size(Pos2::new(right - 8.0, rect.center().y), Vec2::splat(16.0));
            let icon = if check == Some(true) {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            theme::paint_icon(ui, icon, r, 16.0, p.secondary);
        } else if let Some(on) = check {
            let r = Rect::from_center_size(Pos2::new(right - 11.0, rect.center().y), Vec2::splat(22.0));
            let radius = CornerRadius::same(6);
            if on {
                ui.painter().rect_filled(r, radius, p.accent);
                theme::paint_icon(ui, Icon::Check, r, 14.0, p.on_accent);
            } else {
                ui.painter()
                    .rect_stroke(r, radius, Stroke::new(1.5, p.secondary), StrokeKind::Inside);
            }
        }
    }
    resp
}

fn separator(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.outline);
    ui.add_space(4.0);
}

/// Fila de acción: empuja `action` y cierra el menú contextual.
fn action_row(ui: &mut Ui, p: &Palette, label: &str, color: Color32, action: MenuAction) {
    if row(ui, p, label, color, None, false).clicked() {
        push(ui.ctx(), action);
        ui.close();
    }
}

fn menu_setup(ui: &mut Ui) -> Palette {
    ui.set_width(MENU_W);
    ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
    theme::current(ui.ctx())
}

// --- Menú del servidor ----------------------------------------------------

/// Engancha el menú del servidor al clic derecho de `resp`. `guild_id` vacío
/// (servidores de demo) deshabilita el copiado del ID.
pub fn server_menu(resp: &Response, guild_id: &str) {
    let guild_id = guild_id.to_owned();
    resp.clone().context_menu(|ui| {
        let p = menu_setup(ui);
        action_row(ui, &p, "Marcar como leído", p.text, MenuAction::Unavailable("Marcar el servidor como leído"));
        action_row(ui, &p, "Invitar gente", p.text, MenuAction::Unavailable("Invitar gente"));
        separator(ui, &p);
        action_row(ui, &p, "Silenciar servidor", p.text, MenuAction::Unavailable("Silenciar servidor"));
        action_row(
            ui,
            &p,
            "Ajustes de notificaciones",
            p.text,
            MenuAction::Unavailable("Ajustes de notificaciones del servidor"),
        );
        action_row(
            ui,
            &p,
            "Configuración del servidor",
            p.text,
            MenuAction::Unavailable("Configuración del servidor"),
        );
        separator(ui, &p);
        if !guild_id.is_empty() {
            action_row(
                ui,
                &p,
                "Copiar ID del servidor",
                p.text,
                MenuAction::Copy {
                    text: guild_id.clone(),
                    what: "ID del servidor",
                },
            );
        }
        action_row(ui, &p, "Salir del servidor", p.danger, MenuAction::Unavailable("Salir del servidor"));
    });
}

// --- Menú de canal --------------------------------------------------------

/// Engancha el menú del canal al clic derecho de `resp`. `guild_id` vacío =
/// canal de demo; `channel_id` `None` = canal sin id real.
pub fn channel_menu(resp: &Response, guild_id: &str, channel_id: Option<&str>) {
    let guild_id = guild_id.to_owned();
    let channel_id = channel_id.map(str::to_owned);
    resp.clone().context_menu(|ui| {
        let p = menu_setup(ui);
        action_row(ui, &p, "Marcar como leído", p.text, MenuAction::Unavailable("Marcar el canal como leído"));
        if let Some(id) = channel_id.as_deref() {
            if !guild_id.is_empty() {
                action_row(
                    ui,
                    &p,
                    "Copiar enlace",
                    p.text,
                    MenuAction::Copy {
                        text: format!("https://discord.com/channels/{guild_id}/{id}"),
                        what: "Enlace del canal",
                    },
                );
            }
        }
        action_row(ui, &p, "Silenciar canal", p.text, MenuAction::Unavailable("Silenciar canal"));
        action_row(
            ui,
            &p,
            "Ajustes de notificaciones",
            p.text,
            MenuAction::Unavailable("Ajustes de notificaciones del canal"),
        );
        if let Some(id) = channel_id.as_deref() {
            separator(ui, &p);
            action_row(
                ui,
                &p,
                "Copiar ID del canal",
                p.text,
                MenuAction::Copy {
                    text: id.to_owned(),
                    what: "ID del canal",
                },
            );
        }
    });
}

// --- Menú del perfil propio ----------------------------------------------
//
// Tarjeta flotante estilo Discord: banner con degradé del color de tu perfil,
// avatar con aro e insignia de estado, estado personalizado, actividad, canal
// de voz y filas con submenús que se abren AL COSTADO (estado y cambio de
// cuenta, este último con las fotos de perfil). Se abre al hacer clic en tu
// perfil de la barra de usuario (hacia arriba) o en el avatar de la barra
// superior (hacia abajo). Todo entra con una animación corta (fundido +
// deslizamiento) y las filas, interruptores y submenús también se animan.

pub(crate) const POP_W: f32 = 300.0;
const PROW_H: f32 = 46.0;
const SUB_GAP: f32 = 8.0;
const BANNER_H: f32 = 112.0;
const AVATAR_R: f32 = 40.0;
/// La decoración del avatar se dibuja más grande que el avatar (como en Discord).
const DECORATION_SCALE: f32 = 1.2;
/// Clave con la que se guarda el reloj del efecto de perfil de este menú.
const ME_EFFECT_KEY: &str = "me_menu";

/// ¿La biografía del menú está desplegada completa?
fn me_bio_id() -> Id {
    Id::new("ecord_me_menu_bio_expanded")
}
/// Radio de las esquinas del menú.
const MENU_RADIUS: u8 = 18;

fn me_open_id() -> Id {
    Id::new("ecord_me_menu_anchor")
}

/// Submenú abierto al costado: 0 = ninguno, 1 = estado, 2 = cuentas.
fn me_sub_id() -> Id {
    Id::new("ecord_me_menu_sub")
}

fn me_tint_id() -> Id {
    Id::new("ecord_me_menu_tint")
}

fn me_anim_id() -> Id {
    Id::new("ecord_me_menu_anim")
}

fn sub_anim_id(sub: u8) -> Id {
    Id::new(("ecord_me_menu_sub_anim", sub))
}

/// Progreso 0..1 (con suavizado de salida) de una animación que empezó la
/// primera vez que se pidió con este `id`. Pide repintado mientras avanza.
fn anim_t(ctx: &egui::Context, id: Id, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || now));
    let t = (((now - start) as f32) / secs).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    1.0 - (1.0 - t).powi(3)
}

/// Abre o cierra el menú del perfil propio. `anchor` es lo que se tocó (la
/// zona de tu perfil en la barra de usuario o el avatar de la barra superior):
/// el menú se despliega hacia arriba o hacia abajo según dónde esté.
pub fn toggle_me_menu(ctx: &egui::Context, anchor: Rect) {
    let open = ctx.data(|d| d.get_temp::<Rect>(me_open_id()).is_some());
    if open {
        close_me_menu(ctx);
    } else {
        ctx.data_mut(|d| {
            d.insert_temp(me_open_id(), anchor);
            d.insert_temp(me_sub_id(), 0u8);
            d.remove::<f64>(me_anim_id());
        });
    }
}

fn close_me_menu(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<Rect>(me_open_id());
        d.remove::<u8>(me_sub_id());
        d.remove::<f64>(me_anim_id());
        d.remove::<f64>(sub_anim_id(1));
        d.remove::<f64>(sub_anim_id(2));
        // Que el efecto de perfil vuelva a arrancar de cero la próxima vez.
        d.remove::<f64>(profile_popup::effect_start_id(ME_EFFECT_KEY));
        d.remove::<bool>(me_bio_id());
    });
}

/// Actividad propia (juego, música…) lista para dibujar.
struct ActivityView {
    heading: String,
    title: String,
    subtitle: String,
    image_url: Option<String>,
    /// Escuchando algo: se anima un ecualizador.
    listening: bool,
}

/// Canal de voz en el que estás, listo para dibujar.
struct VoiceView {
    channel: String,
    /// Nombre del servidor (`None` en llamadas de DM).
    server: Option<String>,
    /// (avatar, nombre) de quienes están en el canal.
    members: Vec<(Option<String>, String)>,
}

fn voice_view(app: &App) -> Option<VoiceView> {
    let target = app.voice_target.as_ref()?;
    let channel = app.current_voice_channel_label().unwrap_or_else(|| "Llamada".to_owned());
    let channel_id = target.channel_id.to_string();
    let (server, members) = match target.scope {
        crate::discord::VoiceScope::Guild(guild_id) => {
            let guild_id = guild_id.to_string();
            match app.servers.iter().find(|s| s.guild_id == guild_id) {
                Some(server) => {
                    let members: Vec<(Option<String>, String)> = server
                        .categories
                        .iter()
                        .flat_map(|category| category.channels.iter())
                        .find(|c| c.channel_id.as_deref() == Some(channel_id.as_str()))
                        .map(|c| {
                            c.voice_members
                                .iter()
                                .map(|m| (m.avatar_url.clone(), m.name.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (Some(server.name.clone()), members)
                }
                None => (None, Vec::new()),
            }
        }
        _ => (None, Vec::new()),
    };
    Some(VoiceView {
        channel,
        server,
        members,
    })
}

/// Una cuenta guardada, para el submenú de cambio de cuenta.
struct AccountView {
    id: String,
    name: String,
    username: String,
    avatar_url: Option<String>,
    current: bool,
}

/// Datos del menú del perfil propio, copiados de `App` antes de dibujar.
struct MeView {
    name: String,
    username: String,
    user_id: String,
    avatar_url: Option<String>,
    banner: Color32,
    banner_url: Option<String>,
    /// `[primario, secundario]` de los colores del tema del perfil.
    theme: Option<(Color32, Color32)>,
    /// Decoración (marco animado) del avatar.
    decoration_url: Option<String>,
    /// Efecto de perfil (capas animadas que caen sobre el menú).
    effect: Option<crate::discord::models::ProfileEffect>,
    pronouns: Option<String>,
    bio: Option<String>,
    clan: Option<crate::discord::models::ClanTag>,
    badges: Vec<profile_popup::BadgeView>,
    self_mute: bool,
    self_deaf: bool,
    status: crate::lib::data::Status,
    status_key: String,
    custom_status: Option<String>,
    activity: Option<ActivityView>,
    /// Canal de voz/llamada en curso, con quiénes están adentro.
    voice: Option<VoiceView>,
    /// Colección de juegos del perfil.
    games: Vec<crate::discord::models::ProfileGame>,
    accounts: Vec<AccountView>,
}

fn activity_heading(a: &crate::discord::models::PresenceActivity) -> String {
    match a.kind {
        1 => format!("Transmitiendo {}", a.name),
        2 => format!("Escuchando {}", a.name),
        3 => format!("Viendo {}", a.name),
        5 => format!("Compitiendo en {}", a.name),
        _ => format!("Jugando a {}", a.name),
    }
}

fn me_view(app: &App) -> MeView {
    let me = app.me.as_ref();
    let user_id = me.map(|m| m.id.clone()).unwrap_or_default();
    let accounts = app
        .accounts
        .accounts()
        .iter()
        .filter(|a| !a.user_id.is_empty())
        .map(|a| AccountView {
            id: a.user_id.clone(),
            name: a.label().to_owned(),
            username: a.username.clone(),
            // La cuenta abierta usa el avatar vivo; las demás, el último guardado.
            avatar_url: if a.user_id == user_id {
                me.and_then(|m| m.avatar_url()).or_else(|| a.avatar_url.clone())
            } else {
                a.avatar_url.clone()
            },
            current: a.user_id == user_id,
        })
        .collect();

    // `kind == 4` es el estado personalizado; el resto, actividades reales.
    let activities = if user_id.is_empty() {
        Vec::new()
    } else {
        app.activities_of(&user_id)
    };
    let custom_status = activities
        .iter()
        .find(|a| a.kind == 4)
        .and_then(|a| a.state.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| app.custom_status_of(&user_id));
    let activity = activities.iter().find(|a| a.kind != 4).map(|a| ActivityView {
        heading: activity_heading(a),
        title: a
            .details
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or(&a.name)
            .to_owned(),
        subtitle: a
            .state
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| if a.is_spotify() { s.replace(';', ",") } else { s.to_owned() })
            .unwrap_or_default(),
        image_url: a.image_url(),
        listening: a.kind == 2,
    });

    // Tu perfil completo (si ya llegó): de ahí salen los colores del tema, el
    // efecto, y a veces el banner/decoración que el usuario de READY no trae.
    let profile = app.me_profile.as_ref().filter(|p| p.user.id == user_id);
    let meta = profile.and_then(|p| p.user_profile.as_ref());
    let theme = profile_popup::theme_pair(meta);
    let accent_color = meta
        .and_then(|m| m.accent_color)
        .or_else(|| profile.and_then(|p| p.user.accent_color))
        .or_else(|| me.and_then(|m| m.accent_color));
    let banner_color = profile
        .and_then(|p| p.user.banner_color_rgb())
        .or_else(|| me.and_then(|m| m.banner_color_rgb()));
    let banner_url = profile
        .and_then(|p| p.user.banner_url())
        .or_else(|| me.and_then(|m| m.banner_url()));
    let decoration_url = profile
        .and_then(|p| p.user.avatar_decoration_url())
        .or_else(|| me.and_then(|m| m.avatar_decoration_url()));
    let effect = profile.and_then(|p| p.effect.clone());
    let non_empty = |s: Option<String>| s.filter(|t| !t.trim().is_empty());
    let pronouns = non_empty(meta.and_then(|m| m.pronouns.clone()))
        .or_else(|| non_empty(profile.and_then(|p| p.user.pronouns.clone())))
        .or_else(|| non_empty(me.and_then(|m| m.pronouns.clone())));
    let bio = non_empty(meta.and_then(|m| m.bio.clone()))
        .or_else(|| non_empty(profile.and_then(|p| p.user.bio.clone())))
        .or_else(|| non_empty(me.and_then(|m| m.bio.clone())));
    let clan = profile
        .and_then(|p| p.user.clan_tag())
        .or_else(|| me.and_then(|m| m.clan_tag()));
    let badges: Vec<profile_popup::BadgeView> = profile
        .map(|p| {
            p.badges
                .iter()
                .filter_map(|b| {
                    Some(profile_popup::BadgeView {
                        icon_url: b.icon_url()?,
                        description: b.description.clone().unwrap_or_default(),
                    })
                })
                .take(10)
                .collect()
        })
        .unwrap_or_default();

    MeView {
        name: me.map_or_else(|| "Aiden".to_owned(), |m| m.display_name().to_owned()),
        username: me.map(|m| m.username.clone()).unwrap_or_default(),
        user_id,
        avatar_url: me.and_then(|m| m.avatar_url()),
        // Igual que la tarjeta de perfil: color liso del banner, si no el
        // primario del tema, si no el color de acento.
        banner: banner_color
            .map(profile_popup::rgb)
            .or_else(|| theme.map(|(primary, _)| primary))
            .or_else(|| accent_color.map(profile_popup::rgb))
            .unwrap_or(app.palette.accent),
        banner_url,
        theme,
        decoration_url,
        effect,
        pronouns,
        bio,
        clan,
        badges,
        self_mute: app.voice_target.as_ref().map_or(app.self_mute, |t| t.self_mute),
        self_deaf: app.voice_target.as_ref().map_or(app.self_deaf, |t| t.self_deaf),
        status: app.own_status(),
        status_key: app.own_status_key().to_owned(),
        custom_status,
        activity,
        voice: voice_view(app),
        games: profile
            .map(|p| if p.games.is_empty() { p.widget_games() } else { p.games.clone() })
            .unwrap_or_default(),
        accounts,
    }
}

/// Estados que se pueden elegir: (clave de Discord, título, descripción, estado).
const STATUS_CHOICES: [(&str, &str, Option<&str>, crate::lib::data::Status); 4] = [
    ("online", "En línea", None, crate::lib::data::Status::Online),
    ("idle", "Ausente", None, crate::lib::data::Status::Idle),
    (
        "dnd",
        "No molestar",
        Some("No recibirás notificaciones de escritorio"),
        crate::lib::data::Status::Dnd,
    ),
    (
        "invisible",
        "Invisible",
        Some("Aparecerás Desconectado"),
        crate::lib::data::Status::Offline,
    ),
];

fn status_choice_key(s: &str) -> &'static str {
    STATUS_CHOICES.iter().find(|c| c.0 == s).map_or("online", |c| c.0)
}

/// Insignia de presencia (círculo, luna, "no molestar" o aro vacío).
/// `ring` es el color del aro que la separa de lo que tiene detrás.
pub(crate) fn paint_status_badge(
    ui: &Ui,
    center: Pos2,
    r: f32,
    status: crate::lib::data::Status,
    ring: Option<Color32>,
    p: &Palette,
) {
    use crate::lib::data::Status;
    let painter = ui.painter();
    if let Some(ring) = ring {
        painter.circle_filled(center, r + 3.0, ring);
    }
    let color = extra::status_color(status, p);
    match status {
        Status::Online => {
            painter.circle_filled(center, r, color);
        }
        Status::Idle => {
            let rect = Rect::from_center_size(center, Vec2::splat(r * 2.6));
            theme::paint_icon(ui, Icon::Moon, rect, r * 2.6, color);
        }
        Status::Dnd => {
            painter.circle_filled(center, r, color);
            let bar = Rect::from_center_size(center, Vec2::new(r * 1.2, r * 0.5));
            painter.rect_filled(bar, CornerRadius::same(2), ring.unwrap_or(p.overlay));
        }
        Status::Offline => {
            painter.circle_stroke(center, r - 1.2, Stroke::new(2.4, color));
        }
    }
}

enum Lead {
    None,
    Icon(Icon),
    Dot(crate::lib::data::Status),
}

enum Tail {
    None,
    Chevron,
    Switch(bool),
    Check,
}

/// Color de acento del menú (el del banner), guardado al abrir el frame.
fn tint_of(ui: &Ui, p: &Palette) -> Color32 {
    ui.ctx().data(|d| d.get_temp::<Color32>(me_tint_id())).unwrap_or(p.accent)
}

/// Fondo de una fila al pasar el puntero: se funde y trae una barrita de color.
fn row_hover(ui: &Ui, p: &Palette, resp: &Response, rect: Rect) -> f32 {
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    if t > 0.0 {
        let tint = tint_of(ui, p);
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(10),
            extra::blend(p.surface_hover, tint, 0.22).gamma_multiply(t),
        );
        let bar = Rect::from_center_size(
            Pos2::new(rect.left() + 3.0, rect.center().y),
            Vec2::new(3.0, 22.0 * t),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), tint.gamma_multiply(t));
    }
    t
}

/// Fila del popup. `sub` es la línea gris de abajo (descripción).
fn prow(ui: &mut Ui, p: &Palette, lead: Lead, label: &str, sub: Option<&str>, color: Color32, tail: Tail) -> Response {
    let h = if sub.is_some() { 58.0 } else { PROW_H };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let hov = row_hover(ui, p, &resp, rect);
        let tint = tint_of(ui, p);
        let cy = rect.center().y;
        let mut x = rect.left() + 14.0 + hov * 2.0;
        let icon_col = if color == p.danger {
            color
        } else {
            extra::blend(p.secondary, tint, 0.45 + 0.4 * hov)
        };
        match lead {
            Lead::None => {}
            Lead::Icon(icon) => {
                let r = Rect::from_center_size(Pos2::new(x + 10.0, cy), Vec2::splat(20.0));
                theme::paint_icon(ui, icon, r, 20.0, icon_col);
                x += 34.0;
            }
            Lead::Dot(status) => {
                paint_status_badge(ui, Pos2::new(x + 10.0, cy), 7.0, status, None, p);
                x += 34.0;
            }
        }
        if let Some(sub) = sub {
            ui.painter()
                .text(Pos2::new(x, cy - 10.0), Align2::LEFT_CENTER, label, theme::medium(15.0), color);
            ui.painter()
                .text(Pos2::new(x, cy + 11.0), Align2::LEFT_CENTER, sub, theme::regular(12.0), p.dim);
        } else {
            ui.painter()
                .text(Pos2::new(x, cy), Align2::LEFT_CENTER, label, theme::medium(15.0), color);
        }
        let right = rect.right() - 14.0;
        match tail {
            Tail::None => {}
            Tail::Chevron => {
                let r = Rect::from_center_size(Pos2::new(right - 6.0 + hov * 2.0, cy), Vec2::splat(18.0));
                theme::paint_icon(ui, Icon::ChevronRight, r, 18.0, extra::blend(p.secondary, tint, hov));
            }
            Tail::Check => {
                let c = Pos2::new(right - 9.0, cy);
                ui.painter().circle_filled(c, 10.0, tint);
                theme::paint_icon(ui, Icon::Check, Rect::from_center_size(c, Vec2::splat(13.0)), 13.0, p.on_accent);
            }
            Tail::Switch(on) => {
                let k = ui.ctx().animate_bool_with_time(resp.id.with("sw"), on, 0.16);
                let track = Rect::from_center_size(Pos2::new(right - 20.0, cy), Vec2::new(40.0, 22.0));
                ui.painter()
                    .rect_filled(track, CornerRadius::same(11), extra::blend(p.outline, tint, k));
                let kx = track.left() + 11.0 + k * 18.0;
                ui.painter().circle_filled(Pos2::new(kx, cy), 8.0, Color32::WHITE);
            }
        }
    }
    resp
}

/// Fila de una cuenta guardada: foto de perfil, nombre y @usuario.
fn account_row(ui: &mut Ui, p: &Palette, a: &AccountView) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let hov = row_hover(ui, p, &resp, rect);
        let tint = tint_of(ui, p);
        let center = Pos2::new(rect.left() + 18.0 + 21.0 + hov * 2.0, rect.center().y);
        let initial: String = a
            .name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        extra::avatar(ui, center, 21.0, a.avatar_url.as_deref(), p.accent, &initial, p);
        if a.current {
            ui.painter().circle_stroke(center, 24.5, Stroke::new(2.0, tint));
        }
        let x = center.x + 21.0 + 14.0;
        let name_color = if a.current { p.text } else { p.secondary };
        if a.username.is_empty() {
            ui.painter().text(
                Pos2::new(x, rect.center().y),
                Align2::LEFT_CENTER,
                clip(&a.name, 16),
                theme::semibold(15.0),
                name_color,
            );
        } else {
            ui.painter().text(
                Pos2::new(x, rect.center().y - 10.0),
                Align2::LEFT_CENTER,
                clip(&a.name, 16),
                theme::semibold(15.0),
                name_color,
            );
            ui.painter().text(
                Pos2::new(x, rect.center().y + 11.0),
                Align2::LEFT_CENTER,
                format!("@{}", clip(&a.username, 18)),
                theme::regular(12.0),
                p.dim,
            );
        }
        if a.current {
            let c = Pos2::new(rect.right() - 24.0, rect.center().y);
            ui.painter().circle_filled(c, 10.0, tint);
            theme::paint_icon(ui, Icon::Check, Rect::from_center_size(c, Vec2::splat(13.0)), 13.0, p.on_accent);
        }
    }
    resp
}

/// Caja redondeada que agrupa filas (como los bloques de la tarjeta de Discord),
/// con un velo del color de tu perfil.
fn pgroup(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    let tint = tint_of(ui, p);
    ui.add_space(14.0);
    Frame::new()
        .outer_margin(Margin::symmetric(12, 0))
        .fill(extra::blend(extra::blend(p.overlay, p.text, 0.05), tint, 0.10))
        .stroke(Stroke::new(1.0, extra::blend(p.outline, tint, 0.18)))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(5))
        .show(ui, |ui| {
            ui.set_width(POP_W - 24.0 - 10.0 - 4.0);
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            add(ui);
        });
}

fn psep(ui: &mut Ui, p: &Palette) {
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min + Vec2::new(12.0, 0.0), rect.max - Vec2::new(12.0, 0.0)),
        CornerRadius::ZERO,
        p.outline,
    );
    ui.add_space(2.0);
}

/// Marco del menú. `border` = color del borde cuando hay tema de perfil (si no,
/// se deriva del acento); `fill` = color liso de fondo.
fn menu_frame(p: &Palette, tint: Color32, border: Option<Color32>, fill: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(
            1.5,
            border.unwrap_or_else(|| extra::blend(p.outline, tint, 0.55)),
        ))
        .corner_radius(CornerRadius::same(MENU_RADIUS))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow {
            offset: [0, 14],
            blur: 40,
            spread: 0,
            color: p.shadow,
        })
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Estado de un frame del menú: qué se pidió y dónde están las filas con submenú.
#[derive(Default)]
struct MenuState {
    close: bool,
    /// Qué submenú pide abrir el puntero (0 = ninguno, 1 = estado, 2 = cuentas).
    hover: Option<u8>,
    status_row: Option<Rect>,
    accounts_row: Option<Rect>,
}

/// Fila normal: empuja `action` y cierra el menú, salvo que sea un
/// interruptor (`keep_open`).
#[allow(clippy::too_many_arguments)]
fn mitem(
    ui: &mut Ui,
    p: &Palette,
    st: &mut MenuState,
    lead: Lead,
    label: &str,
    color: Color32,
    tail: Tail,
    action: MenuAction,
    keep_open: bool,
) {
    let r = prow(ui, p, lead, label, None, color, tail);
    if r.hovered() {
        st.hover = Some(0);
    }
    if r.clicked() {
        push(ui.ctx(), action);
        if !keep_open {
            st.close = true;
        }
    }
}

fn paint_banner(ui: &mut Ui, rect: Rect, color: Color32, url: Option<&str>, fade_to: Color32) {
    let radius = CornerRadius {
        nw: MENU_RADIUS,
        ne: MENU_RADIUS,
        sw: 0,
        se: 0,
    };
    // Color de fondo mientras la imagen carga (o si no hay banner).
    ui.painter().rect_filled(rect, radius, color);
    let mut drew_image = false;
    if let Some(url) = url.filter(|_| !anim::hide_profile_images()) {
        // `source_banner` devuelve el cuadro ACTUAL si el banner es un GIF
        // animado (y pide el próximo repaint), así que hay que llamarlo en
        // cada frame. Con `anim::plain` se veía siempre el primer cuadro.
        let source = if ui.is_rect_visible(rect) {
            anim::source_banner(ui.ctx(), url, rect.width() * ui.ctx().pixels_per_point())
        } else {
            anim::plain(url)
        };
        let image = egui::Image::new(source)
            .corner_radius(radius)
            .show_loading_spinner(false);
        // El banner CUBRE el hueco (recorta lo que sobra, sin deformar).
        let uv = match image.load_for_size(ui.ctx(), rect.size()) {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                Some(profile_popup::cover_uv(texture.size, rect.size()))
            }
            _ => None,
        };
        if let Some(uv) = uv {
            image.uv(uv).paint_at(ui, rect);
            drew_image = true;
        }
    }
    if !drew_image {
        // Brillo diagonal suave para que no sea un color plano.
        let glow = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * 0.45, rect.top()),
            Pos2::new(rect.right(), rect.top() + rect.height() * 0.55),
        );
        ui.painter().rect_filled(
            glow,
            CornerRadius {
                nw: 0,
                ne: MENU_RADIUS,
                sw: 40,
                se: 0,
            },
            extra::blend(color, Color32::WHITE, 0.14).gamma_multiply(0.55),
        );
    }
    // Desvanecido hacia el fondo del popup en el tercio de abajo del banner.
    let fade = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 46.0), rect.max);
    let top = Color32::TRANSPARENT;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(fade.left_top(), top);
    mesh.colored_vertex(fade.right_top(), top);
    mesh.colored_vertex(fade.right_bottom(), fade_to);
    mesh.colored_vertex(fade.left_bottom(), fade_to);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// Cabecera: banner, avatar con aro de estado, burbuja de estado y nombre.
fn draw_me_header(ui: &mut Ui, p: &Palette, tone: &Tone, v: &MeView, tint: Color32) {
    let (banner, _) = ui.allocate_exact_size(Vec2::new(POP_W - 3.0, BANNER_H), Sense::hover());
    let opaque = Color32::from_rgb(p.overlay.r(), p.overlay.g(), p.overlay.b());
    paint_banner(ui, banner, v.banner, v.banner_url.as_deref(), opaque);

    // Hueco para la mitad del avatar que cuelga del banner.
    let (head, _) = ui.allocate_exact_size(Vec2::new(POP_W - 3.0, AVATAR_R + 4.0), Sense::hover());
    let center = Pos2::new(head.left() + 20.0 + AVATAR_R, banner.bottom());
    let ring = opaque;
    let status_col = extra::status_color(v.status, p);
    ui.painter().circle_filled(center, AVATAR_R + 7.0, p.overlay);
    let has_decoration = v.decoration_url.is_some() && !anim::hide_profile_images();
    // Aro del color del estado alrededor del avatar (con decoración no va: el
    // marco animado ya ocupa ese lugar, como en Discord).
    if !has_decoration {
        ui.painter()
            .circle_stroke(center, AVATAR_R + 3.0, Stroke::new(3.0, status_col));
    }
    let initial: String = v.name.chars().next().map(|c| c.to_string()).unwrap_or_default();
    extra::avatar(ui, center, AVATAR_R, v.avatar_url.as_deref(), p.accent, &initial, p);
    // Decoración animada (APNG) del avatar.
    if let Some(url) = v.decoration_url.as_ref().filter(|_| has_decoration) {
        let size = Vec2::splat(AVATAR_R * 2.0 * DECORATION_SCALE);
        let source = anim::source_animated(ui.ctx(), url, 160, None);
        egui::Image::new(source)
            .show_loading_spinner(false)
            .paint_at(ui, Rect::from_center_size(center, size));
    }
    let off = AVATAR_R * 0.72;
    paint_status_badge(ui, center + Vec2::new(off, off), 10.5, v.status, Some(ring), p);

    // Burbuja con el estado personalizado, a la derecha del avatar.
    if let Some(text) = v.custom_status.as_deref() {
        let galley = ui.painter().layout_no_wrap(clip(text, 20), theme::regular(14.0), p.text);
        let left = center.x + if has_decoration { AVATAR_R * DECORATION_SCALE + 14.0 } else { AVATAR_R + 18.0 };
        let w = (galley.size().x + 28.0).min(banner.right() - left - 14.0);
        let bubble = Rect::from_min_size(Pos2::new(left, banner.bottom() - 8.0), Vec2::new(w, 38.0));
        let fill = extra::blend(p.panel, tint, 0.18);
        ui.painter().rect_filled(bubble, CornerRadius::same(19), fill);
        ui.painter()
            .rect_stroke(bubble, CornerRadius::same(19), Stroke::new(1.0, extra::blend(p.outline, tint, 0.4)), StrokeKind::Inside);
        ui.painter().circle_filled(Pos2::new(left - 5.0, bubble.bottom() - 2.0), 6.0, fill);
        ui.painter().circle_filled(Pos2::new(left - 13.0, bubble.bottom() + 6.0), 3.5, fill);
        ui.painter().galley(
            Pos2::new(bubble.left() + 14.0, bubble.center().y - galley.size().y / 2.0),
            galley,
            p.text,
        );
    }

    // El avatar (y su decoración, que es más grande) cuelga del banner: el
    // nombre empieza DEBAJO de todo eso. Antes el hueco era fijo y, con
    // decoración, el nombre quedaba pegado/encima de la foto.
    let below = if has_decoration {
        AVATAR_R * DECORATION_SCALE + 12.0
    } else {
        AVATAR_R + 10.0
    };
    ui.add_space((below - (AVATAR_R + 4.0)).max(0.0));
    let (names, _) = ui.allocate_exact_size(Vec2::new(POP_W - 3.0, 58.0), Sense::hover());
    ui.painter().text(
        Pos2::new(names.left() + 22.0, names.top() + 12.0),
        Align2::LEFT_CENTER,
        clip(&v.name, 18),
        theme::bold(21.0),
        p.text,
    );
    if !v.username.is_empty() || v.pronouns.is_some() {
        let line = match (v.username.is_empty(), v.pronouns.as_deref()) {
            (false, Some(pr)) => format!("@{} \u{2022} {}", v.username, clip(pr, 24)),
            (false, None) => format!("@{}", v.username),
            (true, Some(pr)) => clip(pr, 30),
            (true, None) => String::new(),
        };
        ui.painter().text(
            Pos2::new(names.left() + 22.0, names.top() + 40.0),
            Align2::LEFT_CENTER,
            line,
            theme::regular(14.0),
            extra::blend(p.dim, tint, 0.35),
        );
    }

    // Etiqueta de servidor + insignias.
    if v.clan.is_some() || !v.badges.is_empty() {
        Frame::new().inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                if let Some(clan) = &v.clan {
                    profile_popup::clan_chip(ui, tone, clan);
                }
                if !v.badges.is_empty() {
                    profile_popup::badges_pill(ui, tone, &v.badges);
                }
            });
        });
    }

    // Biografía ("Sobre mí", con markdown), recortada con "Ver biografía completa".
    if let Some(bio) = v.bio.as_deref() {
        ui.add_space(12.0);
        let expanded: bool = ui.ctx().data(|d| d.get_temp(me_bio_id())).unwrap_or(false);
        Frame::new().inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            profile_popup::panel(ui, tone, |ui| {
                let long = bio.chars().count() > profile_popup::BIO_PREVIEW;
                let shown = if expanded || !long {
                    bio.to_owned()
                } else {
                    format!("{}...", bio.chars().take(profile_popup::BIO_PREVIEW).collect::<String>())
                };
                markdown::show(ui, p, &shown, 13.5, p.text, &MentionCtx::none());
                if long {
                    ui.add_space(4.0);
                    let label = if expanded { "Ver menos" } else { "Ver biografía completa" };
                    if theme::link(ui, label, theme::medium(12.5), p.text).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(me_bio_id(), !expanded));
                    }
                }
            });
        });
    }
}

fn draw_activity(ui: &mut Ui, p: &Palette, a: &ActivityView) {
    let tint = tint_of(ui, p);
    pgroup(ui, p, |ui| {
        let w = ui.available_width();
        ui.add_space(8.0);
        let (head, _) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
        let mut hx = head.left() + 12.0;
        // Ecualizador animado mientras escuchas algo.
        if a.listening {
            let time = ui.ctx().input(|i| i.time) as f32;
            for k in 0..3 {
                let hgt = 5.0 + 9.0 * (0.5 + 0.5 * (time * 5.0 + k as f32 * 1.7).sin());
                let bar = Rect::from_min_max(
                    Pos2::new(hx + k as f32 * 5.0, head.center().y + 7.0 - hgt),
                    Pos2::new(hx + k as f32 * 5.0 + 3.0, head.center().y + 7.0),
                );
                ui.painter().rect_filled(bar, CornerRadius::same(1), tint);
            }
            ui.ctx().request_repaint();
            hx += 22.0;
        }
        ui.painter().text(
            Pos2::new(hx, head.center().y),
            Align2::LEFT_CENTER,
            clip(&a.heading, 28),
            theme::semibold(13.0),
            extra::blend(p.text, tint, 0.35),
        );
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, 68.0), Sense::hover());
        let art = Rect::from_min_size(row.min + Vec2::new(12.0, 7.0), Vec2::splat(54.0));
        let mut drew = false;
        if let Some(url) = a.image_url.as_deref() {
            let image = egui::Image::new(crate::ui::chat::anim::plain(url))
                .corner_radius(CornerRadius::same(10))
                .fit_to_exact_size(Vec2::splat(54.0))
                .show_loading_spinner(false);
            if let Ok(egui::load::TexturePoll::Ready { .. }) = image.load_for_size(ui.ctx(), Vec2::splat(54.0)) {
                ui.put(art, image);
                drew = true;
            }
        }
        if !drew {
            ui.painter().rect_filled(art, CornerRadius::same(10), extra::blend(p.surface_hover, tint, 0.3));
            theme::paint_icon(ui, Icon::Gamepad, art, 24.0, tint);
        }
        let tx = art.right() + 12.0;
        ui.painter().text(
            Pos2::new(tx, row.top() + 20.0),
            Align2::LEFT_CENTER,
            clip(&a.title, 20),
            theme::semibold(15.0),
            p.text,
        );
        ui.painter().text(
            Pos2::new(tx, row.top() + 42.0),
            Align2::LEFT_CENTER,
            clip(&a.subtitle, 24),
            theme::regular(13.0),
            p.dim,
        );
    });
}

/// "Colección de juegos": una fila con los íconos de los juegos del perfil.
fn draw_games(ui: &mut Ui, p: &Palette, games: &[crate::discord::models::ProfileGame]) {
    let tint = tint_of(ui, p);
    pgroup(ui, p, |ui| {
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, 54.0), Sense::hover());
        ui.painter().text(
            Pos2::new(row.left() + 14.0, row.center().y),
            Align2::LEFT_CENTER,
            "Colección de juegos",
            theme::semibold(14.0),
            p.text,
        );
        const ICON: f32 = 32.0;
        const GAP: f32 = 6.0;
        const MAX: usize = 3;
        let shown = games.len().min(MAX);
        let extra_count = games.len().saturating_sub(MAX);
        let slots = shown + usize::from(extra_count > 0);
        let mut x = row.right() - 12.0 - slots as f32 * ICON - (slots.saturating_sub(1)) as f32 * GAP;
        for (i, game) in games.iter().take(shown).enumerate() {
            let rect = Rect::from_min_size(Pos2::new(x, row.center().y - ICON / 2.0), Vec2::splat(ICON));
            let mut drew = false;
            if let Some(url) = game.icon_url.as_deref().filter(|_| !anim::hide_profile_images()) {
                let image = egui::Image::new(anim::plain(url))
                    .corner_radius(CornerRadius::same(8))
                    .show_loading_spinner(false);
                if let Ok(egui::load::TexturePoll::Ready { .. }) = image.load_for_size(ui.ctx(), rect.size()) {
                    image.paint_at(ui, rect);
                    drew = true;
                }
            }
            if !drew {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(8), extra::blend(p.surface_hover, tint, 0.3));
                theme::paint_icon(ui, Icon::Gamepad, rect, 18.0, tint);
            }
            if let Some(name) = game.name.as_deref() {
                ui.interact(rect, ui.id().with(("me_game", i)), Sense::hover()).on_hover_text(name);
            }
            x += ICON + GAP;
        }
        if extra_count > 0 {
            let rect = Rect::from_min_size(Pos2::new(x, row.center().y - ICON / 2.0), Vec2::splat(ICON));
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), extra::blend(p.surface_hover, tint, 0.2));
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("+{extra_count}"),
                theme::semibold(12.0),
                p.text,
            );
        }
    });
}

/// "En canal de voz": quiénes están, canal y servidor, y el botón "Abrir voz".
fn draw_voice(ui: &mut Ui, p: &Palette, st: &mut MenuState, voice: &VoiceView) {
    let tint = tint_of(ui, p);
    pgroup(ui, p, |ui| {
        let w = ui.available_width();
        ui.add_space(8.0);
        let (head, _) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
        // Punto verde que "respira" mientras estás conectado.
        let time = ui.ctx().input(|i| i.time) as f32;
        let pulse = 0.5 + 0.5 * (time * 2.5).sin();
        let dot = Pos2::new(head.left() + 18.0, head.center().y);
        ui.painter().circle_filled(dot, 5.0 + 2.0 * pulse, extra::blend(p.overlay, tint, 0.3 + 0.2 * pulse));
        ui.painter().circle_filled(dot, 3.5, tint);
        ui.ctx().request_repaint();
        ui.painter().text(
            Pos2::new(head.left() + 32.0, head.center().y),
            Align2::LEFT_CENTER,
            "En canal de voz",
            theme::semibold(13.0),
            extra::blend(p.text, tint, 0.35),
        );

        let (row, _) = ui.allocate_exact_size(Vec2::new(w, 54.0), Sense::hover());
        // Pila de avatares (hasta 3) + "+N".
        const R: f32 = 16.0;
        const STEP: f32 = 22.0;
        let shown = voice.members.len().min(3);
        let more = voice.members.len().saturating_sub(3);
        let mut x = row.left() + 14.0 + R;
        for (url, name) in voice.members.iter().take(shown) {
            let c = Pos2::new(x, row.center().y);
            ui.painter().circle_filled(c, R + 2.0, p.overlay);
            let initial: String = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
            extra::avatar(ui, c, R, url.as_deref(), p.accent, &initial, p);
            x += STEP;
        }
        if more > 0 {
            let c = Pos2::new(x, row.center().y);
            ui.painter().circle_filled(c, R + 2.0, p.overlay);
            ui.painter().circle_filled(c, R, extra::blend(p.surface_hover, tint, 0.3));
            ui.painter()
                .text(c, Align2::CENTER_CENTER, format!("+{more}"), theme::semibold(12.0), p.text);
            x += STEP;
        }
        let tx = if shown > 0 { x - STEP + R + 12.0 } else { row.left() + 14.0 };
        let max_chars = if shown > 0 { 18 } else { 26 };
        let (title_y, sub_y) = if voice.server.is_some() {
            (row.center().y - 9.0, row.center().y + 11.0)
        } else {
            (row.center().y, row.center().y)
        };
        ui.painter().text(
            Pos2::new(tx, title_y),
            Align2::LEFT_CENTER,
            clip(&voice.channel, max_chars),
            theme::semibold(15.0),
            p.text,
        );
        if let Some(server) = voice.server.as_deref() {
            ui.painter().text(
                Pos2::new(tx, sub_y),
                Align2::LEFT_CENTER,
                format!("en {}", clip(server, max_chars)),
                theme::regular(13.0),
                p.dim,
            );
        }

        // Botón "Abrir voz".
        let (btn, resp) = ui.allocate_exact_size(Vec2::new(w - 16.0, 40.0), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let hov = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
        let btn = btn.translate(Vec2::new(8.0, 0.0));
        ui.painter().rect_filled(
            btn,
            CornerRadius::same(10),
            extra::blend(extra::blend(p.surface_hover, tint, 0.25), tint, 0.5 * hov),
        );
        ui.painter()
            .text(btn.center(), Align2::CENTER_CENTER, "Abrir voz", theme::semibold(14.5), p.text);
        if resp.clicked() {
            push(ui.ctx(), MenuAction::OpenVoice);
            st.close = true;
        }
        ui.add_space(6.0);
    });
}

/// Paleta del menú. Con tema de perfil, los colores salen de `Tone` (el mismo
/// velo translúcido que usa la tarjeta de perfil, para que el texto siempre se
/// lea); sin tema, es la paleta de la app tal cual.
fn menu_palette(p: Palette, theme: Option<(Color32, Color32)>) -> (Palette, Tone) {
    let tone = Tone::from_theme(theme, &p);
    if theme.is_none() {
        return (p, tone);
    }
    let mut q = p;
    q.overlay = tone.top;
    q.text = tone.text;
    q.secondary = tone.dim;
    q.dim = extra::blend(tone.dim, tone.top, 0.3);
    q.outline = extra::blend(tone.top, tone.text, 0.16);
    q.surface_hover = extra::blend(tone.top, tone.text, 0.12);
    q.accent = tone.accent;
    q.on_accent = tone.on_accent;
    // `panel` pinta la burbuja del estado personalizado.
    let dark_bg = tone.text.r() > 128;
    q.panel = extra::blend(tone.top, if dark_bg { Color32::BLACK } else { Color32::WHITE }, 0.4);
    (q, tone)
}

fn draw_me_menu(ctx: &egui::Context, p: Palette, v: &MeView, anchor: Rect) {
    let screen = ctx.viewport_rect();
    let (p, tone) = menu_palette(p, v.theme);
    let border = v.theme.map(|_| tone.border);
    let tint = if v.theme.is_some() {
        extra::blend(tone.accent, p.text, 0.12)
    } else {
        extra::blend(v.banner, p.text, 0.12)
    };
    ctx.data_mut(|d| d.insert_temp(me_tint_id(), tint));
    let x_max = (screen.right() - POP_W - 8.0).max(8.0);
    // Pegado abajo de la pantalla (barra de usuario) se abre hacia arriba.
    let up = anchor.center().y > screen.center().y;
    // Entrada: fundido + deslizamiento corto desde el borde del que sale.
    let e = anim_t(ctx, me_anim_id(), 0.20);
    let slide = (1.0 - e) * 14.0;
    let (pos, pivot, max_h) = if up {
        (
            Pos2::new(anchor.left().clamp(8.0, x_max), anchor.top() - 8.0 + slide),
            Align2::LEFT_BOTTOM,
            (anchor.top() - 16.0).max(260.0),
        )
    } else {
        (
            Pos2::new((anchor.right() - POP_W).clamp(8.0, x_max), anchor.bottom() + 6.0 - slide),
            Align2::LEFT_TOP,
            (screen.bottom() - anchor.bottom() - 14.0).max(260.0),
        )
    };

    let mut st = MenuState::default();
    let cur_sub: u8 = ctx.data(|d| d.get_temp(me_sub_id())).unwrap_or(0);

    let shown = Area::new(Id::new("ecord_me_menu_area"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            ui.set_opacity(e);
            menu_frame(&p, tint, border, tone.bottom).show(ui, |ui| {
                ui.set_width(POP_W - 3.0);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                // Lugar reservado (debajo de todo) para el degradado de los
                // colores del perfil: recién al final se sabe cuánto mide el menú.
                let gradient_idx = ui.painter().add(egui::Shape::Noop);
                let frame_top = ui.cursor().min.y;
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .min_scrolled_height(max_h.min(760.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(POP_W - 3.0);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        draw_me_header(ui, &p, &tone, v, tint);

                        if !v.games.is_empty() {
                            draw_games(ui, &p, &v.games);
                        }
                        if let Some(a) = v.activity.as_ref() {
                            draw_activity(ui, &p, a);
                        }
                        if let Some(voice) = v.voice.as_ref() {
                            draw_voice(ui, &p, &mut st, voice);
                        }

                        pgroup(ui, &p, |ui| {
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(Icon::Pencil),
                                "Editar perfil",
                                p.text,
                                Tail::None,
                                MenuAction::OpenSettings(SettingsTab::Profile),
                                false,
                            );
                            let cur_label = STATUS_CHOICES
                                .iter()
                                .find(|c| c.0 == status_choice_key(&v.status_key))
                                .map_or("En línea", |c| c.1);
                            let r = prow(ui, &p, Lead::Dot(v.status), cur_label, None, p.text, Tail::Chevron);
                            st.status_row = Some(r.rect);
                            if r.hovered() || r.clicked() {
                                st.hover = Some(1);
                            }
                        });

                        pgroup(ui, &p, |ui| {
                            let r = prow(ui, &p, Lead::Icon(Icon::User), "Cambiar de cuenta", None, p.text, Tail::Chevron);
                            st.accounts_row = Some(r.rect);
                            if r.hovered() || r.clicked() {
                                st.hover = Some(2);
                            }
                            if !v.user_id.is_empty() {
                                mitem(
                                    ui,
                                    &p,
                                    &mut st,
                                    Lead::Icon(Icon::Copy),
                                    "Copiar ID del usuario",
                                    p.text,
                                    Tail::None,
                                    MenuAction::Copy {
                                        text: v.user_id.clone(),
                                        what: "ID del usuario",
                                    },
                                    false,
                                );
                            }
                        });

                        pgroup(ui, &p, |ui| {
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(Icon::Settings),
                                "Ajustes de usuario",
                                p.text,
                                Tail::None,
                                MenuAction::OpenSettings(SettingsTab::Account),
                                false,
                            );
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(Icon::Mic),
                                "Voz y video",
                                p.text,
                                Tail::None,
                                MenuAction::OpenSettings(SettingsTab::Voice),
                                false,
                            );
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(if v.self_mute { Icon::MicOff } else { Icon::Mic }),
                                "Silenciar micrófono",
                                p.text,
                                Tail::Switch(v.self_mute),
                                MenuAction::ToggleMute,
                                true,
                            );
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(if v.self_deaf { Icon::VolumeX } else { Icon::Headphones }),
                                "Ensordecer",
                                p.text,
                                Tail::Switch(v.self_deaf),
                                MenuAction::ToggleDeafen,
                                true,
                            );
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(Icon::Bell),
                                "Notificaciones",
                                p.text,
                                Tail::None,
                                MenuAction::OpenSettings(SettingsTab::Notifications),
                                false,
                            );
                            psep(ui, &p);
                            mitem(
                                ui,
                                &p,
                                &mut st,
                                Lead::Icon(Icon::LogOut),
                                "Cerrar sesión",
                                p.danger,
                                Tail::None,
                                MenuAction::LogOut,
                                false,
                            );
                        });
                        ui.add_space(18.0);
                    });
                // Degradado primario -> secundario del tema, de arriba a abajo.
                // Las esquinas redondeadas quedan del color liso del marco.
                if v.theme.is_some() {
                    let r = ui.min_rect();
                    let radius = f32::from(MENU_RADIUS);
                    let gradient = Rect::from_min_max(
                        Pos2::new(r.left(), frame_top + radius),
                        Pos2::new(r.right(), (r.bottom() - radius).max(frame_top + radius + 1.0)),
                    );
                    ui.painter()
                        .set(gradient_idx, profile_popup::gradient_mesh(gradient, tone.top, tone.bottom));
                }
            });
        });

    let rect = shown.response.rect;

    // Efecto de perfil: capas animadas por ENCIMA del menú (se pasan de sus bordes).
    if let Some(effect) = v.effect.as_ref() {
        profile_popup::draw_effect(ctx, shown.response.layer_id, rect, effect, ME_EFFECT_KEY, screen);
    }

    // El puntero manda qué submenú está abierto: al pasar por una fila con
    // submenú se abre; al pasar por cualquier otra fila se cierra. Mientras el
    // puntero viaja hacia el submenú no toca ninguna fila, así que sigue abierto.
    let sub = st.hover.unwrap_or(cur_sub);
    if sub != cur_sub {
        ctx.data_mut(|d| {
            d.insert_temp(me_sub_id(), sub);
            // La animación del submenú que se abre arranca de cero.
            d.remove::<f64>(sub_anim_id(sub));
        });
    }

    let mut sub_rect: Option<Rect> = None;
    let row = match sub {
        1 => st.status_row,
        2 => st.accounts_row,
        _ => None,
    };
    if let Some(row) = row {
        let width = if sub == 1 { 290.0 } else { 300.0 };
        let se = anim_t(ctx, sub_anim_id(sub), 0.16);
        let dx = (1.0 - se) * 10.0;
        // A la derecha del popup; si no entra, a la izquierda.
        let fits_right = rect.right() + SUB_GAP + width <= screen.right() - 8.0;
        let (spos, spivot) = if fits_right {
            (Pos2::new(rect.right() + SUB_GAP - dx, row.top() - 8.0), Align2::LEFT_TOP)
        } else {
            (Pos2::new(rect.left() - SUB_GAP + dx, row.top() - 8.0), Align2::RIGHT_TOP)
        };
        let area = Area::new(Id::new("ecord_me_menu_sub_area"))
            .order(Order::Foreground)
            .fixed_pos(spos)
            .pivot(spivot)
            .show(ctx, |ui| {
                ui.set_opacity(se);
                menu_frame(&p, tint, border, p.overlay).show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
                    Frame::new().inner_margin(Margin::same(7)).show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        if sub == 1 {
                            let current = status_choice_key(&v.status_key);
                            for (i, (key, title, desc, status)) in STATUS_CHOICES.iter().enumerate() {
                                if i == 1 {
                                    psep(ui, &p);
                                }
                                let tail = if *key == current { Tail::Check } else { Tail::None };
                                let r = prow(ui, &p, Lead::Dot(*status), title, *desc, p.text, tail);
                                if r.clicked() {
                                    push(ui.ctx(), MenuAction::SetStatus(*key));
                                    st.close = true;
                                }
                            }
                        } else {
                            for a in &v.accounts {
                                let r = account_row(ui, &p, a);
                                if r.clicked() && !a.current {
                                    push(ui.ctx(), MenuAction::SwitchAccount(a.id.clone()));
                                    st.close = true;
                                }
                            }
                            psep(ui, &p);
                            let r = prow(ui, &p, Lead::Icon(Icon::Users), "Gestionar cuentas", None, p.text, Tail::None);
                            if r.clicked() {
                                push(ui.ctx(), MenuAction::AddAccount);
                                st.close = true;
                            }
                        }
                    });
                });
            });
        sub_rect = Some(area.response.rect);
    }

    // Se cierra con Esc o con un clic afuera (tu perfil de la barra lo alterna
    // por su cuenta).
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|pos| {
                !rect.contains(pos) && !anchor.contains(pos) && !sub_rect.is_some_and(|r| r.contains(pos))
            })
    });
    if st.close || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close_me_menu(ctx);
    }
}

// --- Aplicar --------------------------------------------------------------

/// Se llama una vez por frame desde `App::ui`: dibuja el menú del perfil
/// propio si está abierto y ejecuta lo que pidió cualquiera de los menús.
pub fn apply(app: &mut App, ctx: &egui::Context) {
    if let Some(anchor) = ctx.data(|d| d.get_temp::<Rect>(me_open_id())) {
        // Tu perfil completo trae los colores del tema, el efecto y la
        // decoración: se pide al abrir el menú (una vez por apertura).
        app.ensure_me_profile();
        let view = me_view(app);
        draw_me_menu(ctx, app.palette, &view, anchor);
    } else {
        // Cerrado: la próxima apertura vuelve a pedirlo (por si lo editaste).
        app.me_profile_requested = false;
    }
    for action in take(ctx) {
        match action {
            MenuAction::Copy { text, what } => {
                ctx.copy_text(text);
                app.push_toast(ToastKind::Success, "Copiado", format!("{what} copiado al portapapeles."));
            }
            MenuAction::Unavailable(what) => {
                app.push_toast(ToastKind::Info, what, "Todavía no está disponible en eCord.");
            }
            MenuAction::OpenSettings(tab) => {
                app.settings_tab = tab;
                app.settings_open = true;
                if tab == SettingsTab::Voice {
                    app.refresh_voice_audio_sources();
                }
            }
            MenuAction::ToggleMute => app.toggle_self_mute(),
            MenuAction::ToggleDeafen => app.toggle_self_deafen(),
            MenuAction::SetStatus(key) => app.set_own_status(key),
            MenuAction::OpenVoice => app.go_to_voice_call(),
            MenuAction::SwitchAccount(id) => app.switch_account(&id),
            MenuAction::AddAccount => app.show_account_picker(),
            MenuAction::LogOut => app.ask_log_out(),
        }
    }
}