//! Tarjeta de perfil de usuario, al estilo del popout que abre el cliente
//! real de Discord al clickear un avatar o un nombre:
//!
//! * aparece AL LADO de donde se clickeó (no centrada con un fondo oscuro), y
//!   se cierra con Escape o clickeando afuera;
//! * fondo con el degradado de los **colores del perfil** (`theme_colors`), con
//!   el texto claro u oscuro según qué tan claro sea ese fondo;
//! * banner que cubre todo el ancho, avatar con **decoración** (el marco
//!   animado), punto de estado y el globito del **estado personalizado**;
//! * nombre con el color de su estilo, **etiqueta de servidor**, **insignias**;
//! * "Sobre mí" con markdown, bloque de lo que está **jugando/escuchando** (con
//!   cronómetro), roles, conexiones y "en común";
//! * el **efecto de perfil** (las capas animadas que se derraman por encima de
//!   la tarjeta) dibujado sobre todo lo demás;
//! * abajo, "Editar perfil" si es el propio, o la cajita de mensaje rápido.
//!
//! Se abre con `App::open_user_profile` (llamado desde `ui::chat`, `ui::dm` o
//! `ui::friends_panel` al clickear un avatar/nombre) y este módulo solo se
//! encarga de DIBUJARLA.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::load::{SizeHint, TexturePoll};
use egui::{
    Area, Color32, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, Sense, Shape, Stroke, TextureId,
    TextureOptions, Vec2, pos2, vec2,
};

use crate::discord::models::{
    ClanTag, FrameLayerAnchor, FrameLayerKind, FrameLayerOrder, PresenceActivity, ProfileEffect,
    ProfileFrame, Role,
};
use crate::lib::data::Status;
use crate::lib::state::{App, ProfilePopup};
use crate::ui::markdown::{self, MentionCtx};
use crate::ui::theme::{self, Icon, Palette};
use crate::ui::{anim, extra};

mod full;
pub use full::show_full;

const CARD_WIDTH: f32 = 360.0;
const BANNER_HEIGHT: f32 = 112.0;
const AVATAR_RADIUS: f32 = 40.0;
/// La decoración se dibuja más grande que el avatar (como en Discord).
const DECORATION_SCALE: f32 = 1.2;
const PAD: f32 = 16.0;
const SCREEN_MARGIN: f32 = 12.0;
/// Alto que se reserva para el pie (botón / mensaje rápido) al calcular cuánto
/// puede medir la parte scrolleable.
const FOOTER_RESERVE: f32 = 72.0;
/// Ancho de referencia en el que Discord diseñó las capas de los efectos.
const EFFECT_REF_WIDTH: f32 = 450.0;
/// Alto (en px del arte original) de las esquinas del borde que NO se estiran
/// cuando la tarjeta es más alta o baja que el arte.
const FRAME_CORNER_ART_PX: f32 = 120.0;
/// Bio más larga que esto se recorta con "Ver biografía completa".
const BIO_PREVIEW: usize = 220;

const STATUS_ONLINE: Color32 = Color32::from_rgb(0x23, 0xa5, 0x5a);
const STATUS_IDLE: Color32 = Color32::from_rgb(0xf0, 0xb2, 0x32);
const STATUS_DND: Color32 = Color32::from_rgb(0xf2, 0x3f, 0x43);
const STATUS_OFFLINE: Color32 = Color32::from_rgb(0x80, 0x84, 0x8e);

/// Quién se clickeó y con qué se lo pintaba (nombre/avatar ya conocidos
/// localmente), para abrir su tarjeta de perfil.
#[derive(Clone)]
pub struct ProfileClickRequest {
    pub user_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub avatar_color: Color32,
    /// Dónde estaba el puntero al clickear: la tarjeta se abre al lado.
    pub anchor: Option<Pos2>,
}

fn click_request_id() -> egui::Id {
    egui::Id::new("ecord_profile_click_request")
}

/// Marca a `user_id` para que se le abra la tarjeta de perfil. Se llama
/// desde cualquier avatar/nombre clickeable (`ui::chat`, `ui::dm`,
/// `ui::friends_panel`) — como esos lugares solo tienen `&Palette`/datos
/// puntuales a mano, no un `&mut App` entero (sobre todo `ui::chat`,
/// fuertemente virtualizado, donde agregarle un parámetro más a cada capa
/// de la lista de mensajes sería un cambio mucho más grande y arriesgado
/// que esto), el pedido se deja en la memoria de egui y `App::ui` lo
/// levanta una vez por frame con `take_requested` (ver `lib/state.rs`).
///
/// La posición del puntero se toma acá mismo, así los llamadores no cambian.
pub fn request_open(
    ctx: &egui::Context,
    user_id: impl Into<String>,
    name: impl Into<String>,
    avatar_url: Option<String>,
    avatar_color: Color32,
) {
    let user_id = user_id.into();
    if user_id.is_empty() {
        return;
    }
    let anchor = ctx.input(|i| i.pointer.latest_pos());
    let request = ProfileClickRequest { user_id, name: name.into(), avatar_url, avatar_color, anchor };
    ctx.memory_mut(|m| m.data.insert_temp(click_request_id(), request));
}

/// Saca (si había) el pedido de `request_open` de este frame. `App::ui`
/// lo llama una vez por frame, antes de dibujar nada; si hay uno, abre la
/// tarjeta con `App::open_user_profile`.
pub fn take_requested(ctx: &egui::Context) -> Option<ProfileClickRequest> {
    ctx.memory_mut(|m| {
        let request: Option<ProfileClickRequest> = m.data.get_temp(click_request_id());
        if request.is_some() {
            m.data.remove::<ProfileClickRequest>(click_request_id());
        }
        request
    })
}

/// Todo lo que puede pasar en un frame de la tarjeta, aplicado a `app`
/// recién después de que termina de dibujarse — mismo motivo que en
/// `ui::settings`: evita pelear con el borrow checker por tener `&mut App`
/// afuera y closures anidados adentro que también lo quieren tocar.
enum Action {
    Close,
    SendMessage { prefill: Option<String> },
    ToggleBio,
    EditProfile,
    CopyId,
    /// Solo en el panel del DM: abre la tarjeta flotante completa.
    OpenFull,
}

// ---------------------------------------------------------------------
// Datos ya masticados para dibujar
// ---------------------------------------------------------------------

struct BadgeView {
    icon_url: String,
    description: String,
}

struct ConnectionView {
    kind: String,
    name: String,
    verified: bool,
}

/// Todo lo que hace falta para dibujar un frame, sacado de `app` ANTES de
/// entrar al `Area::show` (que no puede tener `&mut App` adentro).
struct View {
    user_id: String,
    is_me: bool,
    display_name: String,
    name_color: Option<Color32>,
    username_line: Option<String>,
    pronouns: Option<String>,
    bio: Option<String>,
    avatar_url: Option<String>,
    avatar_color: Color32,
    decoration_url: Option<String>,
    banner_url: Option<String>,
    banner_color: Option<Color32>,
    accent_color: Option<Color32>,
    theme: Option<(Color32, Color32)>,
    clan: Option<ClanTag>,
    badges: Vec<BadgeView>,
    connections: Vec<ConnectionView>,
    member_since: Option<String>,
    mutual_friends: Option<usize>,
    mutual_guilds: Option<usize>,
    roles: Vec<Role>,
    status: Option<Status>,
    custom_status: Option<String>,
    activities: Vec<PresenceActivity>,
    effect: Option<ProfileEffect>,
    frame: Option<ProfileFrame>,
    loading: bool,
    failed: bool,
    bio_expanded: bool,
    anchor: Option<Pos2>,
}

impl View {
    fn build(app: &App, popup: &ProfilePopup) -> Option<Self> {
        let profile = popup.profile.as_ref();
        let user = profile.map(|p| &p.user);
        let meta = profile.and_then(|p| p.user_profile.as_ref());
        let guild_meta = profile.and_then(|p| p.guild_member_profile.as_ref());
        let non_empty = |s: Option<String>| s.filter(|t| !t.trim().is_empty());

        // Nombre/avatar: preferimos lo que trajo el perfil completo; si
        // todavía no llegó (o falló), caemos a lo que ya sabíamos al abrir la
        // tarjeta (`fallback_*`) para no mostrar una fila en blanco.
        let display_name = user
            .map(|u| u.display_name().to_string())
            .unwrap_or_else(|| popup.fallback_name.clone());
        let username_line = user.and_then(|u| {
            (!u.username.is_empty()).then(|| match &u.discriminator {
                Some(d) if d != "0" => format!("{}#{}", u.username, d),
                _ => u.username.clone(),
            })
        });

        // La bio y los pronombres del server tienen prioridad sobre los
        // globales; `user.bio` es el último recurso (cuentas/respuestas viejas).
        let pronouns = non_empty(guild_meta.and_then(|m| m.pronouns.clone()))
            .or_else(|| non_empty(meta.and_then(|m| m.pronouns.clone())))
            .or_else(|| non_empty(user.and_then(|u| u.pronouns.clone())));
        let bio = non_empty(guild_meta.and_then(|m| m.bio.clone()))
            .or_else(|| non_empty(meta.and_then(|m| m.bio.clone())))
            .or_else(|| non_empty(user.and_then(|u| u.bio.clone())));

        let theme = theme_pair(guild_meta).or_else(|| theme_pair(meta));
        let accent_color = meta
            .and_then(|m| m.accent_color)
            .or_else(|| user.and_then(|u| u.accent_color))
            .map(rgb);

        let avatar_url = user
            .and_then(|u| u.avatar_url())
            .or_else(|| popup.fallback_avatar_url.clone())
            .map(|url| url.replace("size=128", "size=256"));
        let banner_url = user
            .and_then(|u| u.banner_url())
            .map(|url| url.replace("size=512", "size=600"));

        let badges: Vec<BadgeView> = profile
            .map(|p| {
                p.badges
                    .iter()
                    .filter_map(|badge| {
                        Some(BadgeView {
                            icon_url: badge.icon_url()?,
                            description: badge.description.clone().unwrap_or_default(),
                        })
                    })
                    .take(10)
                    .collect()
            })
            .unwrap_or_default();

        let connections: Vec<ConnectionView> = profile
            .map(|p| {
                p.connected_accounts
                    .iter()
                    .filter_map(|account| {
                        let name = account.name.clone()?.trim().to_string();
                        if name.is_empty() {
                            return None;
                        }
                        Some(ConnectionView {
                            kind: connection_label(account.kind.as_deref().unwrap_or("")),
                            name,
                            verified: account.verified.unwrap_or(false),
                        })
                    })
                    .take(6)
                    .collect()
            })
            .unwrap_or_default();

        let mutual_friends = profile.and_then(|p| {
            p.mutual_friends_count
                .map(|c| c as usize)
                .or_else(|| (!p.mutual_friends.is_empty()).then_some(p.mutual_friends.len()))
        });
        let mutual_guilds = profile.map(|p| p.mutual_guilds.len());

        // Roles del usuario en el server desde el que se abrió la tarjeta
        // (si se abrió desde uno): cruzamos los ids de `guild_member.roles`
        // contra `Server::roles` para sacarles nombre y color.
        let roles: Vec<Role> = profile
            .and_then(|p| p.guild_member.as_ref())
            .map(|member| {
                let Some(guild_id) = &popup.guild_id else { return Vec::new() };
                let Some(server) = app.servers.iter().find(|s| &s.guild_id == guild_id) else {
                    return Vec::new();
                };
                let mut roles: Vec<Role> = member
                    .roles
                    .iter()
                    .filter_map(|id| server.roles.iter().find(|r| &r.id == id).cloned())
                    .collect();
                roles.sort_by(|a, b| b.position.cmp(&a.position));
                roles
            })
            .unwrap_or_default();

        let activities: Vec<PresenceActivity> = app
            .activities_of(&popup.user_id)
            .into_iter()
            .filter(|a| a.kind != 4 && !a.name.is_empty())
            .take(2)
            .collect();

        Some(Self {
            user_id: popup.user_id.clone(),
            is_me: app.me.as_ref().is_some_and(|me| me.id == popup.user_id),
            name_color: user.and_then(|u| u.display_name_color()).map(rgb),
            display_name,
            username_line,
            pronouns,
            bio,
            avatar_url,
            avatar_color: popup.fallback_avatar_color,
            decoration_url: user.and_then(|u| u.avatar_decoration_url()),
            banner_url,
            banner_color: user.and_then(|u| u.banner_color_rgb()).map(rgb),
            accent_color,
            theme,
            clan: user.and_then(|u| u.clan_tag()),
            badges,
            connections,
            member_since: member_since(&popup.user_id),
            mutual_friends,
            mutual_guilds,
            roles,
            status: app.presence_status_of(&popup.user_id),
            custom_status: app.custom_status_of(&popup.user_id),
            activities,
            effect: profile.and_then(|p| p.effect.clone()),
            frame: profile.and_then(|p| p.frame.clone()),
            loading: popup.loading,
            failed: popup.failed,
            bio_expanded: popup.bio_expanded,
            anchor: popup.anchor,
        })
    }
}

/// Colores de la tarjeta según el tema del perfil (o la paleta de la app si
/// la persona no configuró colores).
#[derive(Clone, Copy)]
struct Tone {
    /// Fondo arriba (justo debajo del banner) y abajo del degradado.
    top: Color32,
    bottom: Color32,
    text: Color32,
    dim: Color32,
    /// Fondo de las secciones, chips y de la cajita de mensaje.
    panel: Color32,
    stroke: Color32,
    /// Borde de la tarjeta entera (con tema de perfil: el color primario).
    border: Color32,
    accent: Color32,
    on_accent: Color32,
}

/// Discord no pinta los colores del perfil "a pelo": les pone encima una capa
/// translúcida (negra en tema oscuro, blanca en claro) para que el texto y
/// los paneles siempre se lean. Sin esto, un primario como `#ff0000` se ve
/// rojo chillón. El valor oscuro está calibrado contra el popup oficial
/// (`#ff0000` -> ~`#660000`); el claro es una estimación.
const PROFILE_VEIL_DARK: f32 = 0.60;
const PROFILE_VEIL_LIGHT: f32 = 0.78;

impl Tone {
    fn for_view(view: &View, palette: &Palette) -> Self {
        match view.theme {
            Some((primary, secondary)) => {
                // Capa translúcida sobre el degradado (ver PROFILE_VEIL_*).
                let (veil, amount) = if palette.dark {
                    (Color32::BLACK, PROFILE_VEIL_DARK)
                } else {
                    (Color32::WHITE, PROFILE_VEIL_LIGHT)
                };
                let top = extra::blend(primary, veil, amount);
                let bottom = extra::blend(secondary, veil, amount);

                // Texto claro sobre fondos oscuros y oscuro sobre claros,
                // decidido con los colores YA cubiertos por la capa.
                let light = luma(extra::blend(top, bottom, 0.5)) > 0.62;
                let (text, dim, panel, stroke) = if light {
                    (
                        Color32::from_rgb(6, 6, 7),
                        Color32::from_rgb(0x2e, 0x30, 0x38),
                        Color32::from_black_alpha(26),
                        Color32::from_black_alpha(40),
                    )
                } else {
                    (
                        Color32::WHITE,
                        Color32::from_rgb(0xdb, 0xde, 0xe1),
                        Color32::from_black_alpha(90),
                        Color32::from_white_alpha(28),
                    )
                };
                // Botones y borde toman el color del perfil (primario), no
                // un gris derivado del fondo.
                let accent = extra::blend(primary, veil, 0.25);
                let on_accent = if luma(accent) > 0.6 { Color32::BLACK } else { Color32::WHITE };
                let border = extra::blend(primary, veil, 0.10);
                Self { top, bottom, text, dim, panel, stroke, border, accent, on_accent }
            }
            None => Self {
                top: palette.overlay,
                bottom: palette.overlay,
                text: palette.text,
                dim: palette.secondary,
                panel: palette.surface,
                stroke: palette.outline,
                border: palette.outline,
                accent: palette.accent,
                on_accent: palette.on_accent,
            },
        }
    }
}

// ---------------------------------------------------------------------
// Punto de entrada
// ---------------------------------------------------------------------

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(popup) = app.profile_popup.as_ref() else { return };
    let Some(view) = View::build(app, popup) else { return };
    let palette = app.palette;
    let tone = Tone::for_view(&view, &palette);
    let mut quick_message = app
        .profile_popup
        .as_ref()
        .map(|p| p.quick_message.clone())
        .unwrap_or_default();

    let ctx = ui.ctx().clone();
    let screen = ctx.viewport_rect();
    let mut action: Option<Action> = None;

    // El frame sobresale de la tarjeta: se le reserva ese espacio para que no
    // se corte contra el borde de la ventana.
    let (pad_top, pad_bottom, pad_side) = match &view.frame {
        Some(f) => frame_padding(f, CARD_WIDTH.min(screen.width() - SCREEN_MARGIN * 2.0)),
        None => (0.0, 0.0, 0.0),
    };
    let usable = Rect::from_min_max(
        screen.min + vec2(pad_side, pad_top),
        screen.max - vec2(pad_side, pad_bottom),
    );
    let card_w = CARD_WIDTH.min(usable.width() - SCREEN_MARGIN * 2.0);
    let max_h = (usable.height() - SCREEN_MARGIN * 2.0).max(240.0);
    // El alto real recién se conoce después de dibujar: se recuerda el del
    // frame anterior para ubicar la tarjeta sin que se salga de la pantalla.
    let height_id = Id::new("profile_popout_height");
    let last_h = ctx
        .memory(|m| m.data.get_temp::<f32>(height_id))
        .unwrap_or(540.0)
        .min(max_h);
    let pos = card_position(view.anchor, usable, card_w, last_h);

    let inner = Area::new(Id::new("user_profile_popout"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            draw_card_with_frame(
                ui, &view, &tone, &palette, card_w, max_h, &mut quick_message, &mut action, false, screen,
            )
        });
    let card_rect = inner.response.rect;

    if (card_rect.height() - last_h).abs() > 1.0 {
        ctx.memory_mut(|m| m.data.insert_temp(height_id, card_rect.height()));
        ctx.request_repaint();
    }

    // El efecto va por ENCIMA de la tarjeta (y se pasa de sus bordes).
    if let Some(effect) = &view.effect {
        draw_effect(&ctx, inner.response.layer_id, card_rect, effect, &view.user_id, screen);
    }

    // Cerrar: Escape, o apretar el mouse fuera de la tarjeta.
    let pressed_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !card_rect.contains(p));
    if action.is_none() && (ctx.input(|i| i.key_pressed(egui::Key::Escape)) || pressed_outside) {
        action = Some(Action::Close);
    }

    // Aplicamos recién acá: `quick_message` puede haber cambiado arriba
    // (el usuario tipeando) aunque no se haya apretado Enter.
    if let Some(popup) = &mut app.profile_popup {
        if popup.user_id == view.user_id {
            popup.quick_message = quick_message;
        }
    }

    match action {
        Some(Action::Close) => {
            // Que el efecto vuelva a arrancar desde cero la próxima vez.
            ctx.memory_mut(|m| m.data.remove::<f64>(effect_start_id(&view.user_id)));
            app.close_profile_popup();
        }
        Some(Action::ToggleBio) => {
            if let Some(popup) = &mut app.profile_popup {
                if popup.user_id == view.user_id {
                    popup.bio_expanded = !popup.bio_expanded;
                }
            }
        }
        Some(Action::SendMessage { prefill }) => {
            app.message_user_from_profile(
                &view.user_id,
                &view.display_name,
                view.avatar_url.clone(),
                view.avatar_color,
                prefill,
            );
        }
        Some(Action::EditProfile) => {
            app.settings_tab = crate::ui::settings::SettingsTab::Account;
            app.settings_open = true;
            app.close_profile_popup();
        }
        Some(Action::CopyId) => ctx.copy_text(view.user_id.clone()),
        Some(Action::OpenFull) | None => {}
    }
}

/// Dibuja la tarjeta con su profile frame (si tiene): las capas `back` debajo
/// y las `front` encima. `clip` = hasta dónde puede sobresalir el frame.
#[allow(clippy::too_many_arguments)]
fn draw_card_with_frame(
    ui: &mut egui::Ui,
    view: &View,
    tone: &Tone,
    palette: &Palette,
    card_w: f32,
    max_h: f32,
    quick_message: &mut String,
    action: &mut Option<Action>,
    panel: bool,
    clip: Rect,
) -> Rect {
    // Pintor sin el recorte del contenedor: el frame se pasa de la tarjeta.
    let wide = ui.painter().with_clip_rect(clip);
    // Lugar reservado DEBAJO de la tarjeta para las capas `back` (recién se
    // sabe el alto de la tarjeta cuando termina de dibujarse).
    let back_idx = wide.add(Shape::Noop);
    let card = draw_card(ui, view, tone, palette, card_w, max_h, quick_message, action, panel);
    if let Some(frame) = &view.frame {
        let (back, front) = frame_shapes(ui.ctx(), frame, card);
        wide.set(back_idx, Shape::Vec(back));
        wide.extend(front);
    }
    card
}

/// Tarjeta de perfil incrustada en el panel derecho de un DM, como la del
/// cliente de Discord: la tarjeta va inset (con margen para que entre el
/// profile frame), con banner, decoración, efecto, insignias, bio, "miembro
/// desde" y el botón "Ver perfil completo". Usa `App::dm_profile`.
pub fn show_panel(app: &mut App, ui: &mut egui::Ui) {
    let Some(popup) = app.dm_profile.as_ref() else { return };
    let Some(view) = View::build(app, popup) else { return };
    let palette = app.palette;
    let tone = Tone::for_view(&view, &palette);
    let ctx = ui.ctx().clone();
    let avail = ui.available_rect_before_wrap();
    let mut action: Option<Action> = None;
    let mut quick_message = String::new();

    // Margen alrededor de la tarjeta: lo que sobresale el frame (si hay).
    let base = 12.0;
    let pad = |w: f32| match &view.frame {
        Some(f) => frame_padding(f, w),
        None => (0.0, 0.0, 0.0),
    };
    let (_, _, side0) = pad(avail.width() - base * 2.0);
    let margin = base.max(side0 + 6.0);
    let card_w = (avail.width() - margin * 2.0).max(120.0);
    let (pad_top, pad_bottom, _) = pad(card_w);
    let top = base + pad_top;
    let bottom = base + pad_bottom;
    let max_h = (avail.height() - top - bottom).max(240.0);
    let rect = Rect::from_min_size(
        pos2(avail.left() + margin, avail.top() + top),
        vec2(card_w, max_h),
    );

    let card_rect = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            draw_card_with_frame(
                ui, &view, &tone, &palette, card_w, max_h, &mut quick_message, &mut action, true, avail,
            )
        })
        .inner;

    // El efecto se recorta al panel para no pisar el chat.
    if let Some(effect) = &view.effect {
        draw_effect(&ctx, ui.layer_id(), card_rect, effect, &view.user_id, avail);
    }

    match action {
        Some(Action::ToggleBio) => {
            if let Some(p) = &mut app.dm_profile {
                p.bio_expanded = !p.bio_expanded;
            }
        }
        Some(Action::OpenFull) => app.open_full_profile(view.user_id.clone()),
        Some(Action::EditProfile) => {
            app.settings_tab = crate::ui::settings::SettingsTab::Account;
            app.settings_open = true;
        }
        Some(Action::CopyId) => ctx.copy_text(view.user_id.clone()),
        Some(Action::Close | Action::SendMessage { .. }) | None => {}
    }
}

/// Dónde poner la tarjeta: al lado del punto clickeado (a la derecha si entra,
/// si no a la izquierda), sin salirse de la pantalla.
fn card_position(anchor: Option<Pos2>, screen: Rect, w: f32, h: f32) -> Pos2 {
    let m = SCREEN_MARGIN;
    let Some(a) = anchor else {
        return pos2(
            screen.center().x - w / 2.0,
            (screen.center().y - h / 2.0).max(screen.top() + m),
        );
    };
    let x = if a.x + 16.0 + w + m <= screen.right() {
        a.x + 16.0
    } else {
        (a.x - 16.0 - w).max(screen.left() + m)
    };
    let y = (a.y - 48.0).min(screen.bottom() - h - m).max(screen.top() + m);
    pos2(x, y)
}

// ---------------------------------------------------------------------
// La tarjeta
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_card(
    ui: &mut egui::Ui,
    view: &View,
    tone: &Tone,
    palette: &Palette,
    card_w: f32,
    max_h: f32,
    quick_message: &mut String,
    action: &mut Option<Action>,
    panel: bool,
) -> Rect {
    let radius: u8 = theme::RADIUS + 6;
    // Ids distintos para la tarjeta del panel y la flotante: pueden estar
    // abiertas a la vez.
    let sfx = if panel { "panel" } else { "popup" };

    Frame::new()
        .fill(tone.bottom)
        .stroke(Stroke::new(1.5, tone.border))
        .corner_radius(CornerRadius::same(radius))
        .shadow(if panel {
            egui::epaint::Shadow::NONE
        } else {
            egui::epaint::Shadow { offset: [0, 16], blur: 40, spread: 0, color: palette.shadow }
        })
        .show(ui, |ui| {
            ui.set_width(card_w);
            // El header se posiciona a mano: sin separación automática.
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);

            // Lugar reservado (debajo de todo lo demás) para el degradado del
            // fondo: recién al final se sabe cuánto mide la tarjeta.
            let gradient_idx = ui.painter().add(Shape::Noop);
            let card_top = ui.cursor().min.y;

            // -- Banner --
            let banner_rect = Rect::from_min_size(ui.cursor().min, vec2(card_w, BANNER_HEIGHT));
            paint_banner(ui, view, palette, banner_rect, radius);
            ui.allocate_rect(banner_rect, Sense::hover());

            // -- Botones flotando sobre el banner --
            let button = 32.0;
            let mut x = banner_rect.right() - PAD - button;
            let y = banner_rect.top() + 10.0;
            let more = round_button(
                ui,
                Rect::from_min_size(pos2(x, y), Vec2::splat(button)),
                Icon::Ellipsis,
                "Copiar ID de usuario",
                Id::new(("profile_popout_more", sfx)),
            );
            if more.clicked() {
                *action = Some(Action::CopyId);
            }
            if !view.is_me && !panel {
                x -= button + 8.0;
                let message = round_button(
                    ui,
                    Rect::from_min_size(pos2(x, y), Vec2::splat(button)),
                    Icon::SquarePen,
                    "Enviar mensaje",
                    Id::new(("profile_popout_message", sfx)),
                );
                if message.clicked() {
                    *action = Some(Action::SendMessage { prefill: None });
                }
            }

            // -- Avatar (con anillo del color del fondo), decoración, estado --
            let center = pos2(banner_rect.left() + PAD + AVATAR_RADIUS + 2.0, banner_rect.bottom());
            ui.painter().circle_filled(center, AVATAR_RADIUS + 6.0, tone.top);
            let initial = view.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            extra::avatar(
                ui,
                center,
                AVATAR_RADIUS,
                view.avatar_url.as_deref(),
                view.avatar_color,
                &initial,
                palette,
            );
            if let Some(url) = view.decoration_url.as_ref().filter(|_| !anim::hide_profile_images()) {
                let size = Vec2::splat(AVATAR_RADIUS * 2.0 * DECORATION_SCALE);
                let source = anim::source_animated(ui.ctx(), url, 160, None);
                egui::Image::new(source)
                    .show_loading_spinner(false)
                    .paint_at(ui, Rect::from_center_size(center, size));
            }
            if let Some(status) = view.status {
                paint_status_badge(ui, center, AVATAR_RADIUS, status, tone);
            }

            // -- Globito del estado personalizado --
            if let Some(text) = &view.custom_status {
                paint_status_bubble(ui, tone, banner_rect, center, text);
            }

            // Debajo del avatar.
            ui.add_space(AVATAR_RADIUS + 10.0);

            // -- Nombre / usuario / pronombres / etiqueta / insignias --
            padded(ui, |ui| {
                ui.set_width(card_w - PAD * 2.0);
                let name_color = view
                    .name_color
                    .map(|c| readable_on(c, tone.top, tone.text, 4.5))
                    .unwrap_or(tone.text);
                theme::text(ui, &view.display_name, theme::bold(20.0), name_color);
                if view.username_line.is_some() || view.pronouns.is_some() {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if let Some(username) = &view.username_line {
                            theme::text(ui, username, theme::regular(13.0), tone.dim);
                        }
                        if let Some(pronouns) = &view.pronouns {
                            let sep = if view.username_line.is_some() { "•  " } else { "" };
                            theme::text(ui, format!("{sep}{pronouns}"), theme::regular(13.0), tone.dim);
                        }
                    });
                }
                if view.clan.is_some() || !view.badges.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        if let Some(clan) = &view.clan {
                            clan_chip(ui, tone, clan);
                        }
                        if !view.badges.is_empty() {
                            badges_pill(ui, tone, &view.badges);
                        }
                    });
                }
            });
            ui.add_space(12.0);

            // -- Cuerpo scrolleable --
            let header_h = ui.cursor().min.y - card_top;
            let scroll_max = (max_h - header_h - FOOTER_RESERVE).max(100.0);
            egui::ScrollArea::vertical()
                .id_salt(("profile_popout_body", sfx))
                .max_height(scroll_max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    draw_body(ui, view, tone, palette, action);
                });

            // -- Pie: editar perfil / mensaje rápido --
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            ui.add_space(4.0);
            padded(ui, |ui| {
                if view.is_me && !panel {
                    edit_profile_button(ui, tone, action);
                } else if panel {
                    view_full_profile_button(ui, tone, action);
                } else {
                    quick_message_box(ui, view, tone, quick_message, action);
                }
            });
            ui.add_space(PAD);

            // Ahora que se sabe cuánto mide todo, se pinta el degradado en el
            // lugar reservado. Las últimas `radius` px quedan del color liso
            // de abajo para respetar las esquinas redondeadas.
            let bottom = ui.min_rect().bottom();
            let gradient = Rect::from_min_max(
                pos2(banner_rect.left(), banner_rect.bottom()),
                pos2(
                    banner_rect.left() + card_w,
                    (bottom - f32::from(radius)).max(banner_rect.bottom() + 1.0),
                ),
            );
            ui.painter().set(gradient_idx, gradient_mesh(gradient, tone.top, tone.bottom));
        })
        .response
        .rect
}

fn draw_body(
    ui: &mut egui::Ui,
    view: &View,
    tone: &Tone,
    palette: &Palette,
    action: &mut Option<Action>,
) {
    if view.loading {
        padded(ui, |ui| {
            ui.horizontal(|ui| {
                theme::spinner(ui, 14.0, tone.dim);
                theme::text(ui, "Cargando perfil...", theme::regular(12.5), tone.dim);
            });
        });
        return;
    }
    if view.failed {
        padded(ui, |ui| {
            theme::text(ui, "No se pudo cargar el perfil completo.", theme::regular(12.5), tone.dim);
        });
        ui.add_space(6.0);
    }

    // -- Sobre mí + miembro desde --
    if view.bio.is_some() || view.member_since.is_some() {
        padded(ui, |ui| {
            panel(ui, tone, |ui| {
                if let Some(bio) = &view.bio {
                    let long = bio.chars().count() > BIO_PREVIEW;
                    let shown = if view.bio_expanded || !long {
                        bio.clone()
                    } else {
                        format!("{}...", bio.chars().take(BIO_PREVIEW).collect::<String>())
                    };
                    markdown::show(ui, palette, &shown, 13.5, tone.text, &MentionCtx::none());
                    if long && !view.bio_expanded {
                        ui.add_space(4.0);
                        if theme::link(ui, "Ver biografía completa", theme::medium(12.5), tone.text).clicked() {
                            *action = Some(Action::ToggleBio);
                        }
                    }
                }
                if let Some(since) = &view.member_since {
                    if view.bio.is_some() {
                        ui.add_space(8.0);
                    }
                    theme::text(ui, "MIEMBRO DE DISCORD DESDE", theme::semibold(10.5), tone.dim);
                    ui.add_space(2.0);
                    theme::text(ui, since, theme::regular(13.0), tone.text);
                }
            });
        });
        ui.add_space(8.0);
    }

    // -- Jugando / escuchando --
    for activity in &view.activities {
        padded(ui, |ui| {
            panel(ui, tone, |ui| activity_block(ui, tone, activity));
        });
        ui.add_space(8.0);
    }

    // -- Roles --
    if !view.roles.is_empty() {
        padded(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for role in &view.roles {
                    role_chip(ui, tone, role);
                }
            });
        });
        ui.add_space(8.0);
    }

    // -- Conexiones --
    if !view.connections.is_empty() {
        padded(ui, |ui| {
            panel(ui, tone, |ui| {
                theme::text(ui, "CONEXIONES", theme::semibold(10.5), tone.dim);
                ui.add_space(4.0);
                for connection in &view.connections {
                    ui.horizontal(|ui| {
                        theme::icon(ui, Icon::Globe, 14.0, tone.dim);
                        theme::text(ui, connection.name.clone(), theme::medium(13.0), tone.text);
                        theme::text(ui, format!("· {}", connection.kind), theme::regular(12.0), tone.dim);
                        if connection.verified {
                            theme::icon(ui, Icon::BadgeCheck, 13.0, STATUS_ONLINE);
                        }
                    });
                }
            });
        });
        ui.add_space(8.0);
    }

    // -- En común --
    let friends = view.mutual_friends.filter(|n| *n > 0);
    let guilds = view.mutual_guilds.filter(|n| *n > 0);
    if friends.is_some() || guilds.is_some() {
        padded(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(n) = friends {
                    theme::icon(ui, Icon::Users, 13.0, tone.dim);
                    theme::text(
                        ui,
                        format!("{n} amigo{} en común", if n == 1 { "" } else { "s" }),
                        theme::regular(12.5),
                        tone.text,
                    );
                }
                if let Some(n) = guilds {
                    if friends.is_some() {
                        theme::text(ui, "•", theme::regular(12.5), tone.dim);
                    }
                    theme::icon(ui, Icon::House, 13.0, tone.dim);
                    theme::text(
                        ui,
                        format!("{n} servidor{} en común", if n == 1 { "" } else { "es" }),
                        theme::regular(12.5),
                        tone.text,
                    );
                }
            });
        });
        ui.add_space(8.0);
    }
}

// ---------------------------------------------------------------------
// Piezas
// ---------------------------------------------------------------------

/// Contenido con el margen lateral de la tarjeta.
fn padded<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .inner_margin(Margin::symmetric(PAD as i8, 0))
        .show(ui, add)
        .inner
}

/// Sección con fondo translúcido (bio, actividad, conexiones).
fn panel<R>(ui: &mut egui::Ui, tone: &Tone, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = ui.available_width();
    Frame::new()
        .fill(tone.panel)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width((width - 24.0).max(40.0));
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            add(ui)
        })
        .inner
}

/// Banner que CUBRE todo el ancho (recorta lo que sobra, no lo deforma).
fn paint_banner(ui: &mut egui::Ui, view: &View, palette: &Palette, rect: Rect, radius: u8) {
    let top_round = CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 };
    let fill = view
        .banner_color
        .or(view.theme.map(|(top, _)| top))
        .or(view.accent_color)
        .unwrap_or_else(|| extra::blend(view.avatar_color, palette.overlay, 0.35));
    ui.painter().rect_filled(rect, top_round, fill);

    let Some(url) = view.banner_url.as_ref().filter(|_| !anim::hide_profile_images()) else { return };
    let source = if ui.is_rect_visible(rect) {
        anim::source_banner(ui.ctx(), url, rect.width() * ui.ctx().pixels_per_point())
    } else {
        anim::plain(url)
    };
    let image = egui::Image::new(source).corner_radius(top_round).show_loading_spinner(false);
    // Hasta que la imagen carga no se sabe su proporción: se ve el color liso.
    let uv = match image.load_for_size(ui.ctx(), rect.size()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => cover_uv(texture.size, rect.size()),
        _ => return,
    };
    image.uv(uv).paint_at(ui, rect);
}

/// Región (en UV) de una textura de tamaño `tex` que hay que mostrar para que
/// llene `target` sin deformarse, recortando parejo de los lados que sobran.
fn cover_uv(tex: Vec2, target: Vec2) -> Rect {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if tex.x <= 0.0 || tex.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return full;
    }
    let tex_ratio = tex.x / tex.y;
    let target_ratio = target.x / target.y;
    if tex_ratio > target_ratio {
        // Imagen más ancha que el hueco: se recortan los costados.
        let w = target_ratio / tex_ratio;
        let x0 = (1.0 - w) / 2.0;
        Rect::from_min_max(pos2(x0, 0.0), pos2(x0 + w, 1.0))
    } else {
        // Imagen más alta: se recorta arriba y abajo.
        let h = tex_ratio / target_ratio;
        let y0 = (1.0 - h) / 2.0;
        Rect::from_min_max(pos2(0.0, y0), pos2(1.0, y0 + h))
    }
}

/// Botón circular translúcido sobre el banner.
fn round_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, tooltip: &str, id: Id) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let fill = if response.hovered() {
        Color32::from_black_alpha(170)
    } else {
        Color32::from_black_alpha(115)
    };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, fill);
    theme::paint_icon(ui, icon, rect, 15.0, Color32::WHITE);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Punto de estado en la esquina del avatar (con anillo del color del fondo).
fn paint_status_badge(
    ui: &mut egui::Ui,
    avatar_center: Pos2,
    avatar_radius: f32,
    status: Status,
    tone: &Tone,
) {
    let c = avatar_center + Vec2::splat(avatar_radius * 0.72);
    // El punto crece con el avatar (9.5 px para el de 40 px de radio).
    let r = 9.5 * avatar_radius / AVATAR_RADIUS;
    let painter = ui.painter();
    painter.circle_filled(c, r + 4.0, tone.top);
    match status {
        Status::Online => {
            painter.circle_filled(c, r, STATUS_ONLINE);
        }
        Status::Idle => {
            // Luna: círculo amarillo con un mordisco del color del fondo.
            painter.circle_filled(c, r, STATUS_IDLE);
            painter.circle_filled(c + vec2(-r * 0.38, -r * 0.38), r * 0.62, tone.top);
        }
        Status::Dnd => {
            painter.circle_filled(c, r, STATUS_DND);
            painter.rect_filled(Rect::from_center_size(c, vec2(r * 1.2, r * 0.42)), 2.0, tone.top);
        }
        Status::Offline => {
            painter.circle_filled(c, r, STATUS_OFFLINE);
            painter.circle_filled(c, r * 0.5, tone.top);
        }
    }
}

/// Globito de pensamiento con el estado personalizado, al lado del avatar.
fn paint_status_bubble(ui: &mut egui::Ui, tone: &Tone, banner: Rect, avatar_center: Pos2, text: &str) {
    let left = avatar_center.x + AVATAR_RADIUS * DECORATION_SCALE + 14.0;
    let right = banner.right() - PAD;
    let width = right - left;
    if width < 90.0 {
        return;
    }
    let fill = extra::blend(tone.top, Color32::BLACK, 0.4);
    let text_color = if luma(fill) > 0.6 { Color32::BLACK } else { Color32::WHITE };
    let mut job = egui::text::LayoutJob::simple(
        text.trim().to_string(),
        theme::regular(12.5),
        text_color,
        width - 22.0,
    );
    job.wrap.max_rows = 2;
    let galley = ui.painter().layout_job(job);
    let height = galley.size().y + 16.0;
    let rect = Rect::from_min_size(pos2(left, avatar_center.y - 8.0), vec2(width, height));
    let painter = ui.painter();
    painter.rect_filled(rect, 12.0, fill);
    // Las dos burbujitas que "salen" del avatar.
    painter.circle_filled(pos2(left - 6.0, rect.top() + height * 0.62), 4.0, fill);
    painter.circle_filled(pos2(left - 14.0, rect.top() + height * 0.62 + 7.0), 2.5, fill);
    painter.galley(pos2(rect.left() + 11.0, rect.top() + 8.0), galley, text_color);
}

/// Etiqueta de servidor (`[icono] NULL`).
fn clan_chip(ui: &mut egui::Ui, tone: &Tone, clan: &ClanTag) {
    let galley = ui
        .painter()
        .layout_no_wrap(clan.tag.clone(), theme::bold(12.0), tone.text);
    let icon = 14.0;
    let icon_space = if clan.badge_url.is_some() { icon + 4.0 } else { 0.0 };
    let size = vec2(8.0 + icon_space + galley.size().x + 8.0, 24.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 8.0, tone.panel);
    let mut x = rect.left() + 8.0;
    if let Some(url) = &clan.badge_url {
        let icon_rect = Rect::from_min_size(pos2(x, rect.center().y - icon / 2.0), Vec2::splat(icon));
        egui::Image::new(anim::plain(url)).show_loading_spinner(false).paint_at(ui, icon_rect);
        x += icon + 4.0;
    }
    let text_pos = pos2(x, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_pos, galley, tone.text);
}

/// Insignias en una pastilla, con su descripción al pasar el mouse.
fn badges_pill(ui: &mut egui::Ui, tone: &Tone, badges: &[BadgeView]) {
    let icon = 18.0;
    let gap = 4.0;
    let pad = 7.0;
    let count = badges.len() as f32;
    let width = pad * 2.0 + count * icon + (count - 1.0).max(0.0) * gap;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, tone.panel);
    for (i, badge) in badges.iter().enumerate() {
        let x = rect.left() + pad + i as f32 * (icon + gap);
        let icon_rect = Rect::from_min_size(pos2(x, rect.center().y - icon / 2.0), Vec2::splat(icon));
        egui::Image::new(anim::plain(&badge.icon_url))
            .show_loading_spinner(false)
            .paint_at(ui, icon_rect);
        let response = ui.interact(icon_rect, Id::new(("profile_popout_badge", i)), Sense::hover());
        if !badge.description.is_empty() {
            response.on_hover_text(badge.description.as_str());
        }
    }
}

fn role_chip(ui: &mut egui::Ui, tone: &Tone, role: &Role) {
    let color = if role.color != 0 { rgb(role.color) } else { tone.dim };
    let galley = ui
        .painter()
        .layout_no_wrap(role.name.clone(), theme::medium(12.0), tone.text);
    let dot = 12.0;
    let pad = vec2(8.0, 4.0);
    let size = vec2(galley.size().x + dot + 6.0, galley.size().y) + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 7.0, tone.panel);
    ui.painter()
        .rect_stroke(rect, 7.0, Stroke::new(1.0, tone.stroke), egui::StrokeKind::Inside);
    let dot_center = pos2(rect.left() + pad.x + dot / 2.0, rect.center().y);
    ui.painter().circle_filled(dot_center, 4.5, color);
    let text_pos = pos2(dot_center.x + dot / 2.0 + 2.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_pos, galley, tone.text);
}

/// "Jugando / Escuchando ...": imagen, nombre, detalle y cronómetro.
fn activity_block(ui: &mut egui::Ui, tone: &Tone, activity: &PresenceActivity) {
    let header = match activity.kind {
        0 => "Jugando",
        1 => "Transmitiendo",
        2 => "Escuchando",
        3 => "Viendo",
        5 => "Compitiendo en",
        _ => "Actividad",
    };
    theme::text(ui, header, theme::semibold(12.5), tone.text);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let side = 56.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        let corner = CornerRadius::same(8);
        ui.painter().rect_filled(rect, corner, Color32::from_black_alpha(70));
        match activity.image_url() {
            Some(url) => {
                egui::Image::new(anim::plain(&url))
                    .corner_radius(corner)
                    .show_loading_spinner(false)
                    .paint_at(ui, rect);
            }
            None => {
                let icon = if activity.kind == 2 { Icon::Music } else { Icon::Gamepad };
                theme::paint_icon(ui, icon, rect, 24.0, tone.dim);
            }
        }

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            theme::text(ui, activity.name.clone(), theme::bold(13.5), tone.text);
            for line in [&activity.details, &activity.state].into_iter().flatten() {
                if !line.trim().is_empty() {
                    theme::text(ui, line.clone(), theme::regular(12.5), tone.dim);
                }
            }
            if let Some(start) = activity.start_ms() {
                let elapsed = now_ms().saturating_sub(start) / 1000;
                let text = match activity.end_ms().filter(|end| *end > start) {
                    Some(end) => {
                        let total = (end - start) / 1000;
                        format!("{} / {}", clock(elapsed.min(total)), clock(total))
                    }
                    None => clock(elapsed),
                };
                ui.horizontal(|ui| {
                    let icon = if activity.kind == 2 { Icon::Music } else { Icon::Gamepad };
                    theme::icon(ui, icon, 13.0, STATUS_ONLINE);
                    theme::text(ui, text, theme::medium(12.0), STATUS_ONLINE);
                });
                // El cronómetro avanza cada segundo.
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
        });
    });
}

/// Botón "Ver perfil completo" del panel del DM (abre el modal grande, `show_full`).
fn view_full_profile_button(ui: &mut egui::Ui, tone: &Tone, action: &mut Option<Action>) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let fill = if response.hovered() {
        extra::blend(tone.text, tone.bottom, 0.82)
    } else {
        extra::blend(tone.text, tone.bottom, 0.90)
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Ver perfil completo",
        theme::semibold(14.0),
        tone.text,
    );
    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *action = Some(Action::OpenFull);
    }
}

/// Botón "Editar perfil" (solo en el propio perfil).
fn edit_profile_button(ui: &mut egui::Ui, tone: &Tone, action: &mut Option<Action>) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let hovered = response.hovered();
    let fill = if hovered {
        extra::blend(tone.accent, tone.on_accent, 0.12)
    } else {
        tone.accent
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    let galley = ui
        .painter()
        .layout_no_wrap("Editar perfil".to_string(), theme::semibold(14.0), tone.on_accent);
    let total = 16.0 + 6.0 + galley.size().x;
    let x0 = rect.center().x - total / 2.0;
    theme::paint_icon(
        ui,
        Icon::Pencil,
        Rect::from_center_size(pos2(x0 + 8.0, rect.center().y), Vec2::splat(16.0)),
        15.0,
        tone.on_accent,
    );
    ui.painter().galley(
        pos2(x0 + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        tone.on_accent,
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        *action = Some(Action::EditProfile);
    }
}

/// Cajita "Enviar mensaje a @usuario".
fn quick_message_box(
    ui: &mut egui::Ui,
    view: &View,
    tone: &Tone,
    quick_message: &mut String,
    action: &mut Option<Action>,
) {
    let width = ui.available_width();
    Frame::new()
        .fill(tone.panel)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width((width - 24.0).max(40.0));
            let hint = egui::RichText::new(format!("Enviar mensaje a @{}", view.display_name)).color(tone.dim);
            let response = ui.add(
                egui::TextEdit::singleline(&mut *quick_message)
                    .hint_text(hint)
                    .text_color(tone.text)
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width()),
            );
            let enter = response.lost_focus() && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));
            if enter && !quick_message.trim().is_empty() {
                *action = Some(Action::SendMessage { prefill: Some(quick_message.trim().to_string()) });
            }
        });
}

// ---------------------------------------------------------------------
// Efecto de perfil
// ---------------------------------------------------------------------

fn effect_start_id(user_id: &str) -> Id {
    Id::new(("profile_popout_effect_start", user_id))
}

/// Dibuja las capas del efecto por encima de la tarjeta. Cada capa es un PNG
/// animado (APNG): las de "intro" se reproducen una sola vez desde que se abre
/// la tarjeta y las de bucle se repiten. Si todavía se están bajando o
/// decodificando, esa capa simplemente no se ve (nunca un cuadro estático
/// suelto).
fn draw_effect(
    ctx: &egui::Context,
    layer: egui::LayerId,
    card: Rect,
    effect: &ProfileEffect,
    user_id: &str,
    clip: Rect,
) {
    if anim::image_mode() != anim::ImageMode::Normal {
        return;
    }
    let now = ctx.input(|i| i.time);
    let start = ctx.memory_mut(|m| *m.data.get_temp_mut_or_insert_with(effect_start_id(user_id), || now));
    let elapsed = (now - start).max(0.0);
    let scale = card.width() / EFFECT_REF_WIDTH;
    let painter = ctx.layer_painter(layer).with_clip_rect(clip);
    let full_uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

    let mut layers: Vec<_> = effect.layers.iter().collect();
    layers.sort_by_key(|l| l.z);
    for l in layers {
        if l.width <= 0.0 || l.height <= 0.0 {
            continue;
        }
        let t = elapsed - f64::from(l.start_ms) / 1000.0;
        if t < 0.0 {
            // Todavía no le toca: volver a mirar en un rato.
            ctx.request_repaint_after(Duration::from_millis(50));
            continue;
        }
        // Las capas que no van en bucle se apagan cuando terminan.
        if !l.looping && l.duration_ms > 0.0 && t * 1000.0 > f64::from(l.duration_ms) + 250.0 {
            continue;
        }
        let rect = Rect::from_min_size(
            card.min + vec2(l.x * scale, l.y * scale),
            vec2(l.width * scale, l.height * scale),
        );
        if let egui::ImageSource::Texture(texture) = anim::source_animated(ctx, &l.src, 480, Some(t)) {
            painter.image(texture.id, rect, full_uv, Color32::WHITE);
        }
    }
}

// ---------------------------------------------------------------------
// Profile frame
// ---------------------------------------------------------------------

/// Px de pantalla por px del arte original: el arte se diseñó para una
/// tarjeta de `inner_width` px de ancho.
fn frame_scale(frame: &ProfileFrame, card_w: f32) -> f32 {
    if frame.inner_width > 0.0 { card_w / frame.inner_width } else { 1.0 }
}

/// Cuánto sobresale el frame de la tarjeta, en px de pantalla:
/// `(arriba, abajo, cada lado)`.
fn frame_padding(frame: &ProfileFrame, card_w: f32) -> (f32, f32, f32) {
    let s = frame_scale(frame, card_w);
    (frame.overflow_top * s, frame.overflow_bottom * s, frame.overflow_horizontal * s)
}

/// Arma las capas del profile frame alrededor de `card`. Devuelve
/// `(detrás, delante)`: las `back` van debajo de la tarjeta y las `front`
/// encima. Una capa que todavía se está bajando simplemente no se dibuja
/// (`try_load_texture` avisa cuando termina y repinta solo).
///
/// Geometría (todo en la escala del arte, `inner_width` = ancho de la tarjeta):
/// el "lienzo" es la tarjeta más `overflow_*` por cada lado. `border` llena el
/// lienzo (con sus esquinas intactas si la tarjeta es más alta/baja que el
/// arte); el resto de las capas mantienen su tamaño natural, centradas en el
/// lienzo y pegadas arriba/abajo/al centro según `anchor`.
fn frame_shapes(ctx: &egui::Context, frame: &ProfileFrame, card: Rect) -> (Vec<Shape>, Vec<Shape>) {
    let scale = frame_scale(frame, card.width());
    let canvas = Rect::from_min_max(
        pos2(card.left() - frame.overflow_horizontal * scale, card.top() - frame.overflow_top * scale),
        pos2(
            card.right() + frame.overflow_horizontal * scale,
            card.bottom() + frame.overflow_bottom * scale,
        ),
    );

    // Bajar (o sacar de caché) cada capa.
    let loaded: Vec<_> = frame
        .layers
        .iter()
        .map(|layer| {
            let url = frame.layer_url(layer);
            match ctx.try_load_texture(&url, TextureOptions::LINEAR, SizeHint::default()) {
                Ok(TexturePoll::Ready { texture }) => Some((texture.id, texture.size)),
                _ => None,
            }
        })
        .collect();

    // Si los PNG no vienen a la resolución del diseño (p. ej. al doble), se
    // deduce la escala real del ancho del borde, que abarca todo el lienzo.
    let art_canvas_w = frame.inner_width + frame.overflow_horizontal * 2.0;
    let border_w = frame
        .layers
        .iter()
        .zip(&loaded)
        .find(|(l, _)| l.kind == FrameLayerKind::Border)
        .and_then(|(_, t)| *t)
        .map(|(_, size)| size.x)
        .filter(|w| *w > 0.0);
    let px = match border_w {
        Some(w) => canvas.width() / w,
        None => scale,
    };
    // Píxeles de la imagen por px del arte (1.0 si el PNG es de tamaño de diseño).
    let src_per_art = match border_w {
        Some(w) if art_canvas_w > 0.0 => w / art_canvas_w,
        _ => 1.0,
    };

    let (mut back, mut front) = (Vec::new(), Vec::new());
    for (layer, tex) in frame.layers.iter().zip(&loaded) {
        let Some((id, size)) = *tex else { continue };
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let mut out: Vec<Shape> = Vec::new();
        if layer.kind == FrameLayerKind::Border {
            border_slices(
                &mut out,
                id,
                size,
                canvas,
                px,
                (frame.overflow_top + FRAME_CORNER_ART_PX) * src_per_art,
                (frame.overflow_bottom + FRAME_CORNER_ART_PX) * src_per_art,
            );
        } else {
            // `responsive`: se estira al ancho del lienzo manteniendo la proporción.
            let mut dest_size = size * px;
            if layer.responsive {
                dest_size = vec2(canvas.width(), canvas.width() * size.y / size.x);
            }
            let x = canvas.center().x - dest_size.x / 2.0;
            let y = match layer.anchor {
                FrameLayerAnchor::Top => canvas.top(),
                FrameLayerAnchor::Bottom => canvas.bottom() - dest_size.y,
                FrameLayerAnchor::Center => canvas.center().y - dest_size.y / 2.0,
            };
            let dest = Rect::from_min_size(pos2(x, y), dest_size);
            out.push(Shape::image(id, dest, full_uv(), Color32::WHITE));
        }
        match layer.order {
            FrameLayerOrder::Back => back.extend(out),
            FrameLayerOrder::Front => front.extend(out),
        }
    }
    (back, front)
}

fn full_uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Dibuja el borde llenando `canvas`. Si el alto natural (a escala `px`) no
/// coincide con el del lienzo porque la tarjeta es más alta o baja que el
/// arte, se estira SOLO la franja del medio (3 cortes verticales) para no
/// deformar las esquinas ni los adornos de arriba y abajo.
/// `top_cut` / `bottom_cut`: alto en px de la imagen de las franjas fijas.
fn border_slices(
    out: &mut Vec<Shape>,
    id: TextureId,
    size: Vec2,
    canvas: Rect,
    px: f32,
    top_cut: f32,
    bottom_cut: f32,
) {
    let natural_h = size.y * px;
    let top_cut = top_cut.min(size.y / 2.0);
    let bottom_cut = bottom_cut.min(size.y / 2.0);
    let (top_h, bottom_h) = (top_cut * px, bottom_cut * px);
    let fits = (natural_h - canvas.height()).abs() < 1.0;
    if fits || top_h + bottom_h >= canvas.height() {
        // Mismo alto (o tarjeta demasiado baja para cortar): una sola imagen.
        out.push(Shape::image(id, canvas, full_uv(), Color32::WHITE));
        return;
    }
    let (v1, v2) = (top_cut / size.y, 1.0 - bottom_cut / size.y);
    let y1 = canvas.top() + top_h;
    let y2 = canvas.bottom() - bottom_h;
    let strip = |top: f32, bottom: f32, v_top: f32, v_bottom: f32| {
        Shape::image(
            id,
            Rect::from_min_max(pos2(canvas.left(), top), pos2(canvas.right(), bottom)),
            Rect::from_min_max(pos2(0.0, v_top), pos2(1.0, v_bottom)),
            Color32::WHITE,
        )
    };
    out.push(strip(canvas.top(), y1, 0.0, v1));
    out.push(strip(y1, y2, v1, v2));
    out.push(strip(y2, canvas.bottom(), v2, 1.0));
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Degradado vertical como malla (barato: dos triángulos).
fn gradient_mesh(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Luminancia relativa WCAG (con gamma), para medir contraste.
fn rel_luma(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// Relación de contraste WCAG entre dos colores (1 = igual, 21 = negro/blanco).
fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (rel_luma(a), rel_luma(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Si `fg` no llega a `min` de contraste sobre `bg`, lo acerca de a poco a
/// `toward` (el color de texto de la tarjeta) hasta que se lea. Mantiene el
/// tono cuando puede, en vez de saltar directo a blanco.
fn readable_on(fg: Color32, bg: Color32, toward: Color32, min: f32) -> Color32 {
    if contrast_ratio(fg, bg) >= min {
        return fg;
    }
    for step in 1..=20 {
        let candidate = extra::blend(fg, toward, step as f32 * 0.05);
        if contrast_ratio(candidate, bg) >= min {
            return candidate;
        }
    }
    toward
}

/// Luminancia aproximada (0 = negro, 1 = blanco).
fn luma(c: Color32) -> f32 {
    (0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b())) / 255.0
}

/// `[primario, secundario]` de los colores del tema de un perfil.
fn theme_pair(meta: Option<&crate::discord::models::UserProfileMeta>) -> Option<(Color32, Color32)> {
    let colors = meta?.theme_colors.as_ref()?;
    let first = *colors.first()?;
    let second = colors.get(1).copied().unwrap_or(first);
    Some((rgb(first), rgb(second)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `1:44` o `1:02:03`.
fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Fecha de creación de la cuenta, sacada del id (snowflake) de Discord.
fn member_since(user_id: &str) -> Option<String> {
    use chrono::Datelike;
    const MONTHS: [&str; 12] =
        ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    let id: u64 = user_id.parse().ok()?;
    let ms = (id >> 22) as i64 + 1_420_070_400_000;
    let date = chrono::DateTime::from_timestamp_millis(ms)?;
    Some(format!("{} {} {}", date.day(), MONTHS[date.month0() as usize], date.year()))
}

fn connection_label(kind: &str) -> String {
    let label = match kind {
        "steam" => "Steam",
        "github" => "GitHub",
        "spotify" => "Spotify",
        "twitch" => "Twitch",
        "youtube" => "YouTube",
        "twitter" => "X",
        "reddit" => "Reddit",
        "xbox" => "Xbox",
        "playstation" => "PlayStation",
        "tiktok" => "TikTok",
        "instagram" => "Instagram",
        "facebook" => "Facebook",
        "epicgames" => "Epic Games",
        "battlenet" => "Battle.net",
        "leagueoflegends" => "League of Legends",
        "riotgames" => "Riot Games",
        "roblox" => "Roblox",
        "bluesky" => "Bluesky",
        "domain" => "Sitio web",
        "paypal" => "PayPal",
        "ebay" => "eBay",
        "" => "Cuenta",
        other => return other.to_string(),
    };
    label.to_string()
}