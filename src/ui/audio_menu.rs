//! Menú de clic derecho con el volumen de una persona (0–200 %), silenciarla y
//! restablecer. Sirve para cualquier lugar donde aparezca una persona: tiles de
//! la llamada, filas de voz de la barra lateral y la lista de miembros.
//!
//! Solo cambia lo que ESTE cliente oye de esa persona, y se guarda en la cuenta
//! (`audio_context_settings`), así que es el mismo ajuste que ve el cliente
//! oficial. Los lugares donde se dibuja no siempre pueden mutar `App` (la lista
//! de miembros recibe `&App`), por eso los cambios se juntan en `ctx.memory` y
//! `App::ui` los aplica una vez por frame con `take_changes`, igual que
//! `profile_popup::request_open`.
//!
//! El aspecto sigue al menú del cliente oficial (filas con hover, checkboxes a
//! la derecha, slider de volumen, submenús con flecha y "Bloquear" en rojo).
//! Cada fila pide su acción con un `AudioChange`; `App::ui` la ejecuta.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use std::collections::{HashMap, HashSet};

use crate::discord::{UserAction, VoiceParticipantPlaybackSettings, VoiceParticipantVolumePercent};
use crate::theme::{self, Icon, Palette};

/// Un cambio o acción pedida desde el menú. El `String` es el id de la persona.
/// `App::ui` los aplica una vez por frame (ver `take_changes`).
#[derive(Clone, Debug)]
pub enum AudioChange {
    Volume(String, u16),
    Muted(String, bool),
    Reset(String),
    /// "Silenciar panel de sonidos" (se guarda en la cuenta).
    SoundboardMuted(String, bool),
    /// "Deshabilitar vídeo" de esa persona.
    VideoDisabled(String, bool),
    /// Abrir su tarjeta de perfil: (id, nombre).
    Profile(String, String),
    /// Dejar `<@id> ` escrito en el composer.
    Mention(String),
    /// Abrir el DM: (id, nombre).
    Message(String, String),
    /// Abrir el DM y llamar: (id, nombre).
    Call(String, String),
    /// Abrir el editor de la nota privada: (id, nombre).
    Note(String, String),
    /// "Ver código de verificación" (id).
    VerificationCode(String),
    /// Acción de red: amistad, ignorar, bloquear, invitar a un server.
    Api(String, UserAction),
}

/// Lo que el menú necesita saber y no puede mirar en `App` (la lista de
/// miembros solo recibe `&App`, igual que con `AudioChange`). `App::ui` lo
/// publica con `publish_context` cuando cambia.
#[derive(Clone, Debug, Default)]
pub struct MenuContext {
    /// Tipo de relación por id de persona: 1 amigo, 2 bloqueado, 3 solicitud
    /// recibida, 4 solicitud enviada. Sin entrada = sin relación.
    pub relationships: HashMap<String, u8>,
    /// Personas ignoradas.
    pub ignored: HashSet<String>,
    /// Personas con el vídeo deshabilitado.
    pub video_disabled: HashSet<String>,
    /// Servers a los que se puede invitar: (guild_id, nombre).
    pub guilds: Vec<(String, String)>,
}

fn context_id() -> egui::Id {
    egui::Id::new("ecord_user_menu_context")
}

/// Deja disponible `MenuContext` para los menús de este frame en adelante.
pub fn publish_context(ctx: &egui::Context, menu_ctx: MenuContext) {
    ctx.data_mut(|d| d.insert_temp(context_id(), std::sync::Arc::new(menu_ctx)));
}

fn menu_context(ctx: &egui::Context) -> std::sync::Arc<MenuContext> {
    ctx.data(|d| d.get_temp::<std::sync::Arc<MenuContext>>(context_id()))
        .unwrap_or_default()
}

fn changes_id() -> egui::Id {
    egui::Id::new("ecord_participant_audio_changes")
}

fn push(ctx: &egui::Context, change: AudioChange) {
    ctx.memory_mut(|m| {
        m.data
            .get_temp_mut_or_default::<Vec<AudioChange>>(changes_id())
            .push(change);
    });
}

/// Saca los cambios pedidos en este frame. `App::ui` los aplica.
pub fn take_changes(ctx: &egui::Context) -> Vec<AudioChange> {
    ctx.memory_mut(|m| m.data.remove_temp::<Vec<AudioChange>>(changes_id()))
        .unwrap_or_default()
}

const MENU_W: f32 = 248.0;
const ROW_H: f32 = 34.0;
const ROW_H_SUB: f32 = 52.0;
const PAD: f32 = 10.0;

/// Lo que va a la derecha de una fila.
#[derive(Clone, Copy)]
enum Trail {
    None,
    /// Submenú (solo la flecha, por ahora).
    Chevron,
    /// Checkbox; el `bool` es si está tildado.
    Check(bool),
}

/// Una fila del menú: texto (con subtítulo opcional) y algo a la derecha.
fn row(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    sub: Option<&str>,
    color: Color32,
    trail: Trail,
) -> Response {
    let height = if sub.is_some() { ROW_H_SUB } else { ROW_H };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(
                rect,
                CornerRadius::same(theme::radius_small()),
                p.surface_hover,
            );
        }
        let left = rect.left() + PAD;
        match sub {
            Some(sub) => {
                painter.text(
                    Pos2::new(left, rect.top() + 18.0),
                    Align2::LEFT_CENTER,
                    label,
                    theme::medium(14.5),
                    color,
                );
                painter.text(
                    Pos2::new(left, rect.top() + 36.0),
                    Align2::LEFT_CENTER,
                    sub,
                    theme::regular(12.0),
                    p.dim,
                );
            }
            None => {
                painter.text(
                    Pos2::new(left, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::medium(14.5),
                    color,
                );
            }
        }
        let right = rect.right() - PAD;
        match trail {
            Trail::None => {}
            Trail::Chevron => {
                let r = Rect::from_center_size(
                    Pos2::new(right - 8.0, rect.center().y),
                    Vec2::splat(16.0),
                );
                theme::paint_icon(ui, Icon::ChevronRight, r, 16.0, p.secondary);
            }
            Trail::Check(on) => {
                let r = Rect::from_center_size(
                    Pos2::new(right - 11.0, rect.center().y),
                    Vec2::splat(22.0),
                );
                let radius = CornerRadius::same(6);
                if on {
                    painter.rect_filled(r, radius, p.accent);
                    theme::paint_icon(ui, Icon::Check, r, 14.0, p.on_accent);
                } else {
                    painter.rect_stroke(r, radius, Stroke::new(1.5, p.secondary), StrokeKind::Inside);
                }
            }
        }
    }
    resp
}

/// Línea fina entre grupos de filas.
fn separator(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.outline);
    ui.add_space(4.0);
}

/// Casilla con su estado actual; devuelve `true` si se la tocó.
fn check_row(ui: &mut Ui, p: &Palette, label: &str, on: bool) -> bool {
    row(ui, p, label, None, p.text, Trail::Check(on)).clicked()
}

/// Fila de acción: pide `change` y cierra el menú.
fn action_row(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    sub: Option<&str>,
    color: Color32,
    change: AudioChange,
) {
    if row(ui, p, label, sub, color, Trail::None).clicked() {
        push(ui.ctx(), change);
        ui.close();
    }
}

/// Submenú nativo de egui (se abre al pasar el mouse) con el mismo texto que
/// las demás filas.
fn submenu<R>(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) {
    let title = egui::RichText::new(label).font(theme::medium(14.5)).color(p.text);
    ui.menu_button(title, add_contents);
}

/// Engancha el menú al clic derecho de `resp` (que tiene que sentir clics).
/// `local` es el ajuste actual de la persona; `is_me`, si es uno mismo.
pub fn show(
    resp: &Response,
    name: &str,
    user_id: &str,
    local: VoiceParticipantPlaybackSettings,
    is_me: bool,
) {
    if user_id.is_empty() {
        return;
    }
    resp.clone().context_menu(|ui| {
        let p = theme::current(ui.ctx());
        let menu = menu_context(ui.ctx());
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);

        let uid = user_id.to_owned();
        let uname = name.to_owned();

        action_row(ui, &p, "Perfil", None, p.text, AudioChange::Profile(uid.clone(), uname.clone()));
        action_row(ui, &p, "Mencionar", None, p.text, AudioChange::Mention(uid.clone()));
        if is_me {
            return;
        }
        action_row(ui, &p, "Mensaje", None, p.text, AudioChange::Message(uid.clone(), uname.clone()));
        action_row(
            ui,
            &p,
            "Iniciar una llamada",
            None,
            p.text,
            AudioChange::Call(uid.clone(), uname.clone()),
        );
        action_row(
            ui,
            &p,
            "Añadir nota",
            Some("Solo visible para ti"),
            p.text,
            AudioChange::Note(uid.clone(), uname.clone()),
        );

        separator(ui, &p);

        // Volumen de la persona (0–200 %), doble clic para volver a 100 %.
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(PAD as i8, 6))
            .show(ui, |ui| {
                ui.set_width(MENU_W - 2.0 * PAD);
                theme::text(ui, "Volumen de usuario", theme::semibold(14.5), p.text);
                ui.add_space(8.0);
                ui.spacing_mut().slider_width = MENU_W - 2.0 * PAD;
                let visuals = ui.visuals_mut();
                visuals.selection.bg_fill = p.accent;
                visuals.widgets.inactive.bg_fill = p.surface_active;
                for w in [
                    &mut visuals.widgets.inactive,
                    &mut visuals.widgets.hovered,
                    &mut visuals.widgets.active,
                ] {
                    w.fg_stroke.color = Color32::WHITE;
                }
                let mut percent = f32::from(local.volume.value());
                let slider = egui::Slider::new(
                    &mut percent,
                    0.0..=f32::from(VoiceParticipantVolumePercent::maximum()),
                )
                .integer()
                .show_value(false);
                let slider_resp = ui.add(slider);
                if slider_resp.changed() {
                    push(ui.ctx(), AudioChange::Volume(uid.clone(), percent.round() as u16));
                }
                if slider_resp.double_clicked() {
                    push(ui.ctx(), AudioChange::Reset(uid.clone()));
                }
                slider_resp.on_hover_text(format!("{} %", percent.round() as u16));
            });

        separator(ui, &p);

        if check_row(ui, &p, "Silenciar", local.muted) {
            push(ui.ctx(), AudioChange::Muted(uid.clone(), !local.muted));
        }
        if check_row(ui, &p, "Silenciar panel de sonidos", local.soundboard_muted) {
            push(ui.ctx(), AudioChange::SoundboardMuted(uid.clone(), !local.soundboard_muted));
        }
        let video_off = menu.video_disabled.contains(user_id);
        if check_row(ui, &p, "Deshabilitar vídeo", video_off) {
            push(ui.ctx(), AudioChange::VideoDisabled(uid.clone(), !video_off));
        }
        action_row(
            ui,
            &p,
            "Ver código de verificación",
            None,
            p.text,
            AudioChange::VerificationCode(uid.clone()),
        );

        // Aplicaciones: el backend todavía no trae los comandos de usuario de
        // las apps, así que el submenú avisa que no hay ninguna.
        submenu(ui, &p, "Aplicaciones", |ui| {
            ui.set_min_width(200.0);
            ui.add_enabled(
                false,
                egui::Label::new(
                    egui::RichText::new("No hay aplicaciones disponibles").color(p.dim),
                ),
            );
        });

        // Invitar a servidor: crea una invitación y se la manda por DM.
        submenu(ui, &p, "Invitar a servidor", |ui| {
            ui.set_min_width(220.0);
            if menu.guilds.is_empty() {
                ui.add_enabled(
                    false,
                    egui::Label::new(egui::RichText::new("No hay servidores").color(p.dim)),
                );
                return;
            }
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (guild_id, guild_name) in &menu.guilds {
                    if ui.button(guild_name.as_str()).clicked() {
                        push(
                            ui.ctx(),
                            AudioChange::Api(
                                uid.clone(),
                                UserAction::InviteToServer { guild_id: guild_id.clone() },
                            ),
                        );
                    }
                }
            });
        });

        let kind = menu.relationships.get(user_id).copied();
        if kind != Some(2) {
            let (label, action) = match kind {
                Some(1) => ("Eliminar amigo", UserAction::RemoveFriend),
                Some(3) => (
                    "Aceptar solicitud de amistad",
                    UserAction::AcceptFriend { confirm: false },
                ),
                Some(4) => ("Cancelar solicitud de amistad", UserAction::RemoveFriend),
                _ => ("Añadir amigo", UserAction::AddFriend),
            };
            action_row(ui, &p, label, None, p.text, AudioChange::Api(uid.clone(), action));
        }
        if menu.ignored.contains(user_id) {
            action_row(
                ui,
                &p,
                "Dejar de ignorar",
                None,
                p.text,
                AudioChange::Api(uid.clone(), UserAction::Unignore),
            );
        } else {
            action_row(ui, &p, "Ignorar", None, p.text, AudioChange::Api(uid.clone(), UserAction::Ignore));
        }
        if kind == Some(2) {
            action_row(
                ui,
                &p,
                "Desbloquear",
                None,
                p.danger,
                AudioChange::Api(uid.clone(), UserAction::Unblock),
            );
        } else {
            action_row(ui, &p, "Bloquear", None, p.danger, AudioChange::Api(uid.clone(), UserAction::Block));
        }
    });
}
