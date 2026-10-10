use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Area, Color32, CornerRadius, Frame, Margin, Order, ScrollArea, Sense, Stroke, Vec2};

use crate::lib::data::{
    ChatMessage, ComponentClick, EmojiGroup, Forward, ReactionKind, RepliedMessage, ReplyTarget,
    ThreadCard, UnreadMarker,
};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::media;
use crate::ui::markdown::MentionCtx;
use crate::ui::theme::{self, Icon, Palette};

const COMPOSER_HEIGHT: f32 = 64.0;

/// Alto de cada línea de aviso sobre la caja de texto (modo lento, permisos
/// que faltan…).
const NOTICE_H: f32 = 20.0;

/// Lo que el canal le permite hacer a la cuenta en el chat: sale de los
/// permisos (`lib::permissions::ChannelAccess`) y del modo lento. El
/// `Default` es "sin límites" (DMs, hilos y cualquier dato que falte).
///
/// Lo arma quien llama a `show` y se lo deja con `set_next_limits` justo antes
/// (en la memoria temporal de egui): así `show` no cambia de firma para los
/// demás chats. `show` lo consume y lo deja a mano para las funciones de
/// adentro (compositor, reacciones).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatLimits {
    pub can_send: bool,
    pub can_attach: bool,
    pub can_embed: bool,
    pub can_read_history: bool,
    pub can_react: bool,
    /// Segundos de modo lento que le tocan a ESTA cuenta (0 = sin modo lento
    /// o exenta).
    pub slowmode_secs: u32,
    /// Tamaño máximo por archivo adjunto (ver `discord::uploads::max_upload_bytes`).
    pub max_upload_bytes: u64,
}

impl Default for ChatLimits {
    fn default() -> Self {
        Self {
            can_send: true,
            can_attach: true,
            can_embed: true,
            can_read_history: true,
            can_react: true,
            slowmode_secs: 0,
            max_upload_bytes: crate::discord::uploads::max_upload_bytes(None, true),
        }
    }
}

impl ChatLimits {
    pub fn from_channel(channel: &crate::lib::data::Channel) -> Self {
        let access = channel.access;
        Self {
            can_send: access.can_send,
            can_attach: access.can_attach,
            can_embed: access.can_embed,
            can_read_history: access.can_read_history,
            can_react: access.can_react,
            slowmode_secs: if access.bypass_slowmode { 0 } else { channel.slowmode_secs },
            // Lo ajusta quien llama (`ui::server`) según el Nitro de la cuenta.
            max_upload_bytes: Self::default().max_upload_bytes,
        }
    }
}

fn next_limits_id() -> egui::Id {
    egui::Id::new("ecord_chat_limits_next")
}

fn current_limits_id() -> egui::Id {
    egui::Id::new("ecord_chat_limits_current")
}

/// Deja los límites para el próximo `show` (se consumen ahí).
pub fn set_next_limits(ctx: &egui::Context, limits: ChatLimits) {
    ctx.data_mut(|d| d.insert_temp(next_limits_id(), limits));
}

fn current_limits(ctx: &egui::Context) -> ChatLimits {
    ctx.data(|d| d.get_temp::<ChatLimits>(current_limits_id())).unwrap_or_default()
}

/// Aviso rojo pasajero sobre el compositor ("no tienes permiso para…").
fn set_notice(ctx: &egui::Context, text: &str) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("ecord_chat_notice"), (text.to_owned(), now)));
    ctx.request_repaint_after(std::time::Duration::from_millis(4100));
}

fn active_notice(ctx: &egui::Context) -> Option<String> {
    let (text, at) = ctx.data(|d| d.get_temp::<(String, f64)>(egui::Id::new("ecord_chat_notice")))?;
    let now = ctx.input(|i| i.time);
    (now - at < 4.0).then_some(text)
}

fn slowmode_id(channel_id: &str) -> egui::Id {
    egui::Id::new(("ecord_slowmode_until", channel_id.to_owned()))
}

/// Segundos que faltan para poder mandar otro mensaje en el canal.
fn slowmode_wait(ctx: &egui::Context, channel_id: &str) -> f64 {
    let until = ctx.data(|d| d.get_temp::<f64>(slowmode_id(channel_id))).unwrap_or(0.0);
    (until - ctx.input(|i| i.time)).max(0.0)
}

/// Arranca la cuenta regresiva del modo lento (se llama al enviar).
fn start_slowmode(ctx: &egui::Context, channel_id: &str, secs: u32) {
    let until = ctx.input(|i| i.time) + f64::from(secs);
    ctx.data_mut(|d| d.insert_temp(slowmode_id(channel_id), until));
    ctx.request_repaint();
}

fn format_secs(secs: u32) -> String {
    if secs >= 3600 && secs % 3600 == 0 {
        format!("{} h", secs / 3600)
    } else if secs >= 60 && secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// Líneas de aviso que van sobre la caja de texto, en orden. Las usan `show`
/// (para reservar el alto) y `composer` (para dibujarlas), así que tienen que
/// salir iguales en los dos.
fn composer_notices(
    ctx: &egui::Context,
    palette: &Palette,
    limits: ChatLimits,
    channel_id: &str,
) -> Vec<(String, Color32)> {
    let mut out = Vec::new();
    if let Some(text) = active_notice(ctx) {
        out.push((text, palette.danger));
    }
    if let Some(text) = crate::ui::attachments::status_line(ctx, scoped_id("ecord_composer_jobs")) {
        out.push((text, palette.dim));
    }
    if limits.slowmode_secs > 0 {
        let wait = slowmode_wait(ctx, channel_id);
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            out.push((
                format!("Modo lento: podrás enviar otro mensaje en {} s.", wait.ceil() as u32),
                palette.warning,
            ));
        } else {
            out.push((
                format!("Modo lento activado: {} entre mensajes.", format_secs(limits.slowmode_secs)),
                palette.dim,
            ));
        }
    }
    let mut missing = Vec::new();
    if !limits.can_attach {
        missing.push("adjuntar archivos");
    }
    if !limits.can_embed {
        missing.push("insertar enlaces");
    }
    if !missing.is_empty() {
        out.push((
            format!("No tienes permiso para {} en este canal.", missing.join(" ni ")),
            palette.dim,
        ));
    }
    out
}

/// Barra que reemplaza a la caja de texto cuando no se puede escribir.
fn restriction_bar(ui: &mut egui::Ui, palette: &Palette, text: &str, tone: Color32) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(theme::radius() + 6))
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 16.0);
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Lock, 16.0, palette.dim);
                    theme::text(ui, text, theme::regular(13.0), tone);
                });
            });
    });
}

/// Aviso arriba de la lista cuando no hay permiso para ver el historial.
fn history_notice(ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::radius()))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Lock, 14.0, palette.warning);
                    theme::text(
                        ui,
                        "No tienes permiso para ver el historial de este canal: solo verás los mensajes nuevos.",
                        theme::regular(12.5),
                        palette.secondary,
                    );
                });
            });
    });
    ui.add_space(8.0);
}
/// Alto máximo de la caja de texto: pasado esto, scrollea en vez de crecer.
const COMPOSER_TEXT_MAX_HEIGHT: f32 = 150.0;
/// Banner "Respondiendo a..." arriba del compositor (con su espaciado).
const REPLY_BANNER_HEIGHT: f32 = 36.0;
/// Barra `/comando` + descripción arriba del formulario de un slash command.
const SLASH_HEADER_HEIGHT: f32 = 40.0;
/// Ancho del avatar (36px) + el espaciado por defecto de egui entre
/// widgets de un `horizontal` (8px, ver `theme::apply`) + el padding
/// izquierdo de la fila (16px). Los mensajes agrupados (sin avatar propio)
/// dejan este mismo hueco para que su texto quede alineado bajo el del
/// mensaje anterior, en vez de pegado al borde.
const AVATAR_GUTTER: f32 = 16.0 + 36.0 + 8.0;
/// Emoji del botón de "reacción rápida" de la barra flotante: clickearlo
/// agrega el primero de esta lista que el mensaje todavía no tenga (un
/// solo click, sin abrir ningún panel).
const QUICK_REACTIONS: &[&str] = &["👍", "❤️", "😂", "🎉", "😮", "🙏"];

/// Las reacciones del botón de reacción rápida: primero las que más usa la
/// cuenta (frecency de reacciones, ver `discord::frecency`), completadas con
/// las de siempre hasta llegar a `QUICK_REACTIONS.len()`.
fn quick_reactions() -> Vec<String> {
    use crate::discord::frecency;
    let mut list: Vec<String> = frecency::top_reactions(QUICK_REACTIONS.len())
        .iter()
        .filter_map(|key| frecency::unicode_for_emoji_key(key))
        .map(str::to_owned)
        .collect();
    for default in QUICK_REACTIONS {
        if list.len() >= QUICK_REACTIONS.len() {
            break;
        }
        if !list.iter().any(|e| e == default) {
            list.push((*default).to_string());
        }
    }
    list
}
/// Margen máximo (en píxeles, arriba o abajo de una fila) dentro del cual
/// el mouse todavía cuenta como "sobre" un mensaje aunque ya no esté
/// literalmente encima de su rect — sirve para no descolgar la barra
/// flotante apenas el cursor sube un poco para clickear un botón que
/// sobresale del borde de arriba de la fila. Solo se usa como una zona de
/// tolerancia para elegir la fila más cercana (ver `hovered_index` en
/// `show`), nunca hace que dos filas cuenten como hovereadas a la vez.
const HOVER_ZONE_MARGIN: f32 = 18.0;
const TOOLBAR_BUTTON: f32 = 28.0;
/// 3 reacciones + separador + 4 botones (ver `hover_toolbar`).
const TOOLBAR_WIDTH: f32 = 236.0;
/// Ancho del menú de un mensaje (`message_menu`).
const MENU_WIDTH: f32 = 290.0;
/// Ancho de la tarjeta "Hilo de respuestas" que va debajo de un mensaje.
const THREAD_CARD_WIDTH: f32 = 380.0;
/// Margen derecho de los mensajes (igual al izquierdo): sin él, en un panel
/// angosto (el de hilos) embeds y tarjetas llegan hasta el borde y se ven
/// cortados.
const MESSAGE_RIGHT_GUTTER: f32 = 16.0;
/// Pasado este tiempo sin mensajes, la tarjeta de un hilo dice que no hay
/// mensajes recientes (3 días = el archivado automático por defecto).
const THREAD_RECENT_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// Alto fijo de toda píldora de reacción, sin importar el glyph del
/// emoji que le toque dibujar. Antes el alto salía de
/// `emoji_galley.size().y`, que la fuente mide distinto según el emoji
/// (por ejemplo una secuencia de "regional indicator" como las que arma
/// alguien deletreando una palabra letra por letra con reacciones puede
/// medir bastante más alto que un emoji normal) — con eso cada píldora
/// de una fila terminaba con una altura ligeramente distinta y, al
/// quedar centradas cada una en su propio alto dentro de
/// `horizontal_wrapped`, la fila entera se leía como una escalera
/// bajando de izquierda a derecha en vez de una línea prolija. Fijando
/// el alto acá, todas las píldoras de una fila miden exactamente lo
/// mismo pase lo que pase con el emoji.
const REACTION_PILL_HEIGHT: f32 = 22.0;
/// Separación entre el final de una racha de mensajes de un autor y el
/// primer mensaje de la siguiente racha (autor distinto, o el primero
/// del canal) — igual que el `margin-top` que usa el cliente real para
/// separar un "grupo" de mensajes del anterior.
const NEW_GROUP_SPACING: f32 = 17.0;
/// Separación entre dos mensajes consecutivos de la MISMA racha (mismo
/// autor seguido) — bien chica, como en el cliente real, donde quedan
/// casi pegados.
const GROUPED_SPACING: f32 = 2.0;
/// Franja extra (en píxeles, arriba y abajo del área realmente visible)
/// dentro de la cual una fila SE SIGUE dibujando entera aunque esté
/// justo fuera del viewport — evita el "pop-in" (fila en blanco un
/// instante) al scrollear rápido, sin llegar a dibujar toda la lista.
/// Ver el comentario grande sobre virtualización más abajo, en `show`.
const VIRTUALIZE_BUFFER: f32 = 800.0;
/// Cuánto esperar, después de reanclar el scroll tras cargar "más"
/// mensajes, antes de volver a permitir OTRO pedido automático aunque el
/// centinela de arriba vuelva a estar visible. `ui.scroll_to_rect` no
/// salta al instante: anima la posición durante un rato (~0.2-0.3s en
/// egui), así que en los frames intermedios el offset todavía está a
/// mitad de camino y el centinela puede seguir "visible" un rato después
/// de haber pedido la página anterior — sin este margen, esos frames
/// disparaban un SEGUNDO pedido (o más) antes de que la animación
/// terminara de asentarse, lo que traía otra página de encima, volvía a
/// mover el ancla, y así en cascada: exactamente el "se cargan 2 veces y
/// el scroll se vuelve loco" que este margen evita. Con más frames de
/// sobra que la duración real de la animación no hay costo real: el
/// usuario no puede llegar arriba de nuevo tan rápido de todos modos.
const LOAD_MORE_SETTLE: Duration = Duration::from_millis(400);
/// Alto estimado (antes de medir el real) para una fila SIN encabezado
/// (agrupada) — se usa solo la primera vez que una fila cae fuera del
/// viewport y todavía no se la midió nunca.
const ESTIMATED_ROW_HEIGHT_GROUPED: f32 = 20.0;
/// Igual que el de arriba pero para una fila CON avatar/encabezado
/// propio (siempre un poco más alta).
const ESTIMATED_ROW_HEIGHT_HEADER: f32 = 42.0;
/// Alto del esqueleto de "historial más viejo" que va arriba de la lista,
/// como múltiplo del alto visible de la lista. Es lo bastante alto como
/// para que el usuario pueda seguir scrolleando hacia arriba POR ENCIMA del
/// esqueleto mientras llega la página (en vez de chocar con un tope), y para
/// que cuando los mensajes lo reemplacen no haya un cambio brusco.
const SKELETON_HISTORY_VIEWPORTS: f32 = 1.5;
/// Tope de filas que se dibujan al repetir el esqueleto para llenar alto
/// (por si el alto pedido fuera absurdo).
const SKELETON_MAX_ROWS: usize = 64;

thread_local! {
    /// Qué lista de mensajes se está dibujando: 0 = el chat principal, 1 =
    /// el panel lateral de un hilo. Los dos pueden estar en pantalla a la
    /// vez, y todo lo que `show` guarda en la memoria de egui por id (rects
    /// de filas, panel de reacciones, barra flotante...) tiene que ser
    /// distinto para cada uno o se pisarían entre sí.
    static CHAT_SCOPE: Cell<u8> = const { Cell::new(0) };
}

fn scope() -> u8 {
    CHAT_SCOPE.with(Cell::get)
}

/// Corre `f` (una llamada a [`show`]) dibujando en el "scope" `scope` —
/// ver `CHAT_SCOPE`. Vuelve al chat principal al terminar.
pub fn with_scope<R>(scope: u8, f: impl FnOnce() -> R) -> R {
    CHAT_SCOPE.with(|c| c.set(scope));
    let result = f();
    CHAT_SCOPE.with(|c| c.set(0));
    result
}

/// Id de memoria de egui propio del scope actual.
fn scoped_id(name: &'static str) -> egui::Id {
    egui::Id::new((name, scope()))
}

/// ¿Este compositor es el que recibe lo que llega "a la ventana" (archivos
/// soltados, imagen pegada)? Con dos a la vez (chat y panel de hilo) manda el
/// último que tuvo el foco, si todavía se está dibujando.
fn is_input_target(ctx: &egui::Context, text_id: egui::Id) -> bool {
    let now = ctx.input(|i| i.time);
    let last: Option<egui::Id> = ctx.data(|d| d.get_temp(last_composer_key()));
    !last.is_some_and(|l| {
        l != text_id
            && ctx
                .data(|d| d.get_temp::<f64>(egui::Id::new(("ecord_composer_alive", l))))
                .is_some_and(|seen| now - seen < 0.5)
    })
}

fn type_to_focus_key() -> egui::Id {
    egui::Id::new("ecord_type_to_focus_allowed")
}

/// ¿Lo tipeado sin ningún campo enfocado puede ir al compositor / formulario?
pub fn type_to_focus_allowed(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(type_to_focus_key())).unwrap_or(false)
}

fn last_composer_key() -> egui::Id {
    egui::Id::new("ecord_last_composer")
}

/// `App::ui` avisa en cada frame si lo tipeado sin ningún campo enfocado puede
/// ir al compositor (no, si hay un diálogo, ajustes, un visor, etc. encima).
pub fn set_type_to_focus_allowed(ctx: &egui::Context, allowed: bool) {
    ctx.data_mut(|d| d.insert_temp(type_to_focus_key(), allowed));
}

/// Lo que el usuario hizo este frame sobre alguna fila de mensaje (además de
/// reaccionar, que va aparte por `toggled_reaction`). Cada fila lo va
/// llenando mientras se dibuja y `show` lo convierte en un [`ChatEvent`].
#[derive(Default)]
struct RowActions {
    /// Abrir un hilo: (id, nombre, quién lo empezó).
    open_thread: Option<(String, String, Option<String>)>,
    /// Crear un hilo desde un mensaje: (canal del mensaje, id, nombre).
    create_thread: Option<(String, String, String)>,
    /// Botón de bot apretado.
    component: Option<ComponentClick>,
    /// Click en la cita de una respuesta: id del mensaje original al que ir.
    jump_to: Option<String>,
}

/// Resultado de un frame de `show`: casi siempre `None`, salvo cuando el
/// usuario clickeó "Reenviar" en la barra flotante de algún mensaje — ese
/// botón no tiene forma de elegir un canal/DM destino todavía (no hay
/// ningún selector así en este cliente chico), así que `chat::show` no lo
/// resuelve solo: le avisa a quien lo llamó (`ui::dm`/`ui::server`) para
/// que muestre un aviso.
pub enum ChatEvent {
    None,
    ForwardRequested,
    /// El usuario llegó arriba del todo de los mensajes ya cargados y
    /// todavía puede quedar historial más viejo (`has_more` seguía en
    /// `true`) — quien llamó a `show` (`ui::dm`/`ui::server`) tiene que
    /// pedirle a `App::load_more_messages` la página anterior.
    LoadMoreRequested,
    /// El usuario clickeó en el picker un emoji personalizado bloqueado
    /// (`EmojiGroup::is_locked`: hace falta Nitro y la cuenta no lo tiene).
    /// No se reaccionó; quien llamó a `show` (`ui::dm`/`ui::server`) tiene
    /// que abrir el popup que lo explica.
    NitroRequired,
    /// Se clickeó la tarjeta de un hilo (o "Ver todos los hilos"): quien
    /// llamó a `show` lo abre en el panel lateral (`App::open_thread_panel`).
    OpenThread { id: String, name: String, owner_id: Option<String> },
    /// Se pidió crear un hilo desde un mensaje (`App::create_thread_from_message`).
    CreateThread { channel_id: String, message_id: String, name: String },
    /// Se apretó un botón de un bot (`App::press_component`).
    Component(ComponentClick),
    /// El usuario llegó abajo del todo de una ventana que NO termina en el
    /// mensaje más nuevo del canal (`has_newer`): hay que pedir la página
    /// posterior (`App::load_newer_messages`, `?after=<id>`).
    LoadNewerRequested,
    /// Click en la cita de una respuesta: ir al mensaje original
    /// (`App::jump_to_message`).
    JumpToMessage { message_id: String },
    /// Click en "N mensajes nuevos desde...": ir al primer mensaje sin ver
    /// (`App::jump_to_first_unread`).
    JumpToFirstUnread,
    /// Click en "Marcar como leído" (`App::mark_viewed_read`).
    MarkAsRead,
    /// Click en "Ir al actual" de "Estás viendo mensajes antiguos"
    /// (`App::jump_to_present`).
    JumpToPresent,
    /// "Marcar no leídos" desde el menú de un mensaje (`App::mark_message_unread`).
    MarkUnread { message_id: String },
    /// Opción del menú que todavía no existe en este cliente: se avisa con un
    /// toast con este nombre.
    Unavailable(&'static str),
}

/// Dibuja la lista de mensajes (scrollable) + el input de abajo. Si el
/// usuario escribe algo y aprieta Enter, se agrega como mensaje propio a
/// `messages` (eco local inmediato) y, si `send_target` trae un
/// `(token, channel_id, guild_id)` real (`guild_id` = `None` en un DM),
/// además se manda de verdad por REST.
/// `channels`: id de canal -> nombre, para resolver menciones `<#id>` —
/// `None` cuando no aplica (DMs no tienen lista de canales).
/// `reply_target`: mensaje al que se está respondiendo ahora mismo (botón
/// "Responder" de la barra al pasar el mouse), si hay alguno — un campo
/// más de `App`, igual que `compose_text`.
///
/// Mensajes consecutivos del mismo autor (por ejemplo alguien que manda
/// tres líneas seguidas en vez de un solo mensaje) se agrupan: solo el
/// primero de la racha muestra avatar/nombre/hora, y los siguientes van
/// directo con el contenido, alineados debajo — igual que en el cliente
/// real.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    palette: &Palette,
    messages: &mut Vec<ChatMessage>,
    compose_text: &mut String,
    placeholder: &str,
    own_author: &str,
    own_color: Color32,
    send_target: Option<(String, String, Option<String>)>,
    channels: Option<&HashMap<String, String>>,
    reply_target: &mut Option<ReplyTarget>,
    // Emojis personalizados disponibles para reaccionar, una sección por
    // server (`EmojiGroup`, con el ícono para la barra lateral del
    // picker). En un DM vienen los de TODOS los servers en los que estás
    // (sin un orden especial); en un canal de server, primero los de ESE
    // server y después los de los demás. Vacío en la demo.
    custom_emojis: &[EmojiGroup],
    // Todavía no llegó ni la primera página de mensajes de este canal/DM
    // (pedido inicial en curso, `messages` sigue vacío) — mientras esto
    // sea `true` se muestra el placeholder tipo "esqueleto" en vez de la
    // lista vacía. Ver `Friend::loading` / `Channel::loading`.
    loading: bool,
    // Hay un pedido de "cargar más" (página más vieja) en curso — se
    // muestra el esqueleto animado arriba del todo de la lista. Ver
    // `Friend::loading_more` / `Channel::loading_more`.
    loading_more: bool,
    // Si probablemente queda historial más viejo por cargar. Con esto en
    // `false` no se dispara ningún pedido nuevo aunque el usuario llegue
    // arriba del todo. Ver `Friend::has_more` / `Channel::has_more`.
    has_more: bool,
    // La lista NO llega hasta el mensaje más nuevo del canal (se saltó a un
    // mensaje con `around`): abajo del todo hay que seguir pidiendo con
    // `after`. Ver `Friend::has_newer` / `Channel::has_newer`.
    has_newer: bool,
    // Hay un pedido de la página posterior (`after`) en curso — esqueleto
    // animado abajo del todo de la lista.
    loading_newer: bool,
    // Id del mensaje al que hay que volver a anclar el scroll apenas
    // vuelva a aparecer en `messages` (después de que se antepuso una
    // página más vieja) — ver `App::pending_scroll_anchor`. Se consume
    // (se pone en `None`) una sola vez, apenas se aplica.
    scroll_anchor: &mut Option<String>,
    // Mensaje al que hay que llevar el scroll (centrado) apenas aparezca en
    // `messages` — ver `App::pending_jump`. También se consume una sola vez.
    jump_target: &mut Option<String>,
    // Marca de "mensajes nuevos" del chat abierto (`App::unread_markers`).
    // `None` en el panel de hilos.
    unread: Option<&mut UnreadMarker>,
    // La cuenta puede borrar mensajes ajenos y sacar reacciones de otros
    // (permiso "Administrar mensajes"; `false` en DMs y en hilos).
    can_manage_messages: bool,
) -> ChatEvent {
    // Límites del canal (permisos y modo lento) que dejó quien llamó; se
    // publican también para las funciones de adentro (compositor, reacciones).
    let limits = ui
        .ctx()
        .data_mut(|d| d.remove_temp::<ChatLimits>(next_limits_id()))
        .unwrap_or_default();
    ui.ctx().data_mut(|d| d.insert_temp(current_limits_id(), limits));
    // El render de mensajes no recibe el estado de la app: se publica el
    // token para que las tarjetas de invitación puedan consultar la API.
    if let Some((token, _, _)) = &send_target {
        crate::discord::invites::set_token(token);
    }
    // Alto que se reserva para el compositor: el base + lo que suman la barra
    // de respuesta, la del slash command y las líneas de más de la caja de
    // texto (esto último, medido en el frame anterior y siempre acotado: la
    // caja scrollea pasado `COMPOSER_TEXT_MAX_HEIGHT`). Todo es de alto
    // conocido, así que no hay forma de que se retroalimente y el compositor
    // se vaya corriendo solo.
    let reply_banner_extra = if reply_target.is_some() { REPLY_BANNER_HEIGHT } else { 0.0 };
    let slash_form_open =
        crate::ui::slash::load_active(ui.ctx(), scoped_id("ecord_composer_slash_active")).is_some();
    let slash_extra = if slash_form_open { SLASH_HEADER_HEIGHT } else { 0.0 };
    let text_extra: f32 = ui
        .ctx()
        .memory(|m| m.data.get_temp(scoped_id("ecord_composer_text_extra")))
        .unwrap_or(0.0_f32)
        .clamp(0.0, COMPOSER_TEXT_MAX_HEIGHT);
    // Líneas de aviso (modo lento, permisos) que se dibujan sobre la caja.
    let notice_extra = if limits.can_send {
        let channel_key = send_target.as_ref().map(|(_, c, _)| c.as_str()).unwrap_or("");
        composer_notices(ui.ctx(), palette, limits, channel_key).len() as f32 * NOTICE_H
    } else {
        0.0
    };
    let list_height = (ui.available_height()
        - COMPOSER_HEIGHT
        - reply_banner_extra
        - slash_extra
        - text_extra
        - notice_extra)
        .max(80.0);
    // Justo el frame en que termina la carga inicial de este canal/DM
    // (`loading`: true -> false, mismo frame en que ya aparecen sus
    // mensajes) el `ScrollArea` de acá abajo todavía no tuvo chance de
    // saltar al final (`stick_to_bottom`): en su primer frame arranca
    // con el offset en 0 (arriba del todo) y recién DESPUÉS de dibujar
    // el contenido de este mismo frame corrige la posición para el
    // próximo repintado. Con el offset todavía en 0 en este frame, el
    // centinela de "cargar más" de más abajo queda visible y dispara un
    // segundo pedido de historial enseguida, sin que el usuario haya
    // scrolleado nada — eso es lo que se sentía como "carga los
    // mensajes dos veces" al abrir un canal por primera vez. Enganchamos
    // acá el mismo lock que ya usa el reanclaje de páginas
    // (`LOAD_MORE_SETTLE`) apenas se detecta esa transición, para darle
    // tiempo al scroll de asentarse abajo antes de evaluar el centinela
    // más abajo en este mismo frame.
    {
        let prev_loading_id = scoped_id("ecord_chat_prev_loading");
        let was_loading: bool = ui.ctx().memory(|m| m.data.get_temp(prev_loading_id).unwrap_or(false));
        ui.ctx().memory_mut(|m| m.data.insert_temp(prev_loading_id, loading));
        if was_loading && !loading {
            lock_load_more_until_settled(ui.ctx());
        }
    }
    // Reacción clickeada este frame (índice del mensaje en `messages` +
    // emoji), aplicada recién después de terminar de dibujar la lista para
    // no pelear con el préstamo inmutable que usa el scroll de acá abajo.
    let mut toggled_reaction: Option<(usize, ReactionKind)> = None;
    let mut actions = RowActions::default();
    // Crear un hilo solo tiene sentido en un canal de server (`channels` es
    // `None` en los DMs) y no dentro del propio panel de un hilo.
    let can_create_thread = channels.is_some() && scope() == 0;
    // Canal donde se manda / se ven los mensajes (para completar el canal de
    // los eventos de hilo de un mensaje que no trae el suyo).
    let fallback_channel = send_target.as_ref().map(|(_, id, _)| id.clone()).unwrap_or_default();
    let mut nitro_required = false;
    let mut forward_requested = false;
    // Acción pedida desde el menú de un mensaje (`message_menu`).
    let mut menu_event: Option<ChatEvent> = None;
    let mut message_op: Option<MessageOp> = None;
    let menu_guild: Option<String> = send_target.as_ref().and_then(|(_, _, g)| g.clone());
    let mut load_more_requested = false;
    let mut load_newer_requested = false;
    // Qué mensaje tiene abierto el panel de "más reacciones" (botón "+"
    // de la barra flotante), leído UNA sola vez al principio del frame.
    // El click que lo abre recién queda guardado para el próximo frame
    // (ver `set_open_reaction_panel`) — si lo leyéramos de nuevo después
    // de ese click y dibujáramos el panel en el mismo frame, el propio
    // click de apertura se contaría como "click afuera" del panel
    // (`Response::clicked_elsewhere`) y lo cerraría al toque.
    let open_panel_index = open_reaction_panel_index(ui.ctx());
    // Igual que el panel de reacciones: se lee una sola vez por frame.
    let open_menu = open_message_menu(ui.ctx());

    // Posición del mouse UNA sola vez al principio del frame — se usa dos
    // veces más abajo (para el backlight y para elegir a qué mensaje le
    // toca la barra flotante) y tiene que ser la misma en ambos lados.
    let pointer_pos = ui.ctx().input(|i| i.pointer.hover_pos());

    // Rects de cada fila TAL COMO quedaron dibujados el frame anterior
    // (ver el `insert_temp` al final de este mismo `show`). Sirven para
    // poder pintar el "backlight" de la fila hovereada ANTES de dibujar
    // las filas de este frame — así queda detrás del contenido en vez de
    // encima — sin depender de ningún truco de reservar/completar shapes.
    // Un frame de diferencia (~16ms) entre la lista real y este rect
    // "viejo" no se nota: el layout de la lista es estable de frame a
    // frame y el highlight se re-ajusta solo apenas el usuario mueve el
    // mouse.
    let prev_row_rects: Vec<(usize, egui::Rect)> =
        ui.ctx().memory(|m| m.data.get_temp(row_rects_memory_id()).unwrap_or_default());
    let backlit_index = hovered_row_index(pointer_pos, &prev_row_rects);

    // Filas ya dibujadas este frame — se guardan para, DESPUÉS de haber
    // dibujado todas, decidir cuál le toca la barra flotante (con el rect
    // exacto de este mismo frame, sin el desfasaje de un frame que sí es
    // aceptable para el backlight) y para que el próximo frame pueda
    // pintar el backlight detrás de sus propias filas.
    let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();

    // Mensaje que quedó arriba del todo de la vista este frame y cuántos
    // píxeles de él están por encima del borde (para `lib::last_view`).
    let mut top_visible: Option<(String, f32)> = None;
    // Este frame se hizo un salto/reanclaje: el scroll todavía no se movió,
    // así que lo medido no vale para guardar.
    let mut moved_this_frame = false;
    // Dónde (en pantalla, relativo al borde de arriba del scroll) estaba cada
    // mensaje en el frame ANTERIOR, por id. Sirve para que, cuando una página
    // más vieja reemplaza al esqueleto, el mensaje ancla se quede EXACTAMENTE
    // donde estaba en pantalla en vez de saltar al borde de arriba: los
    // mensajes nuevos "rellenan" el lugar del esqueleto sin mover nada.
    let prev_row_views: Vec<(String, f32)> =
        ui.ctx().memory(|m| m.data.get_temp(row_views_memory_id()).unwrap_or_default());
    let mut row_views: Vec<(String, f32)> = Vec::new();

    // ---- Mensajes sin leer ("N mensajes nuevos desde...") y "Ir al actual" ----
    let mut unread = unread;
    let mut unread_view: Option<UnreadView> = None;
    let mut unread_first_visible = false;
    let mut unread_live = false;
    if scope() == 0 && !loading && !messages.is_empty() {
        if let Some(marker) = unread.as_deref_mut() {
            let view = compute_unread_view(messages.as_slice(), &marker.last_read, has_more);
            if !marker.evaluated {
                marker.evaluated = true;
                // Entró sin nada nuevo: nunca se muestra la barra en este chat.
                if view.is_none() && !has_newer {
                    marker.dismissed = true;
                }
            }
            if view.is_some() && !marker.dismissed {
                marker.had_unread = true;
            }
            unread_live = marker.had_unread;
            unread_view = view;
        }
    }
    // La línea "NUEVO" solo se dibuja si se conoce el punto exacto (la
    // ventana cargada incluye el último mensaje leído).
    let divider_index: Option<usize> = if unread_live {
        unread_view.as_ref().filter(|v| v.exact).map(|v| v.first_index)
    } else {
        None
    };
    // "Ir al actual" / "Marcar como leído" con la ventana en el medio: bajar
    // el scroll apenas llegue la lista nueva.
    let now = ui.ctx().input(|i| i.time);
    let want_bottom = ui
        .ctx()
        .memory(|m| m.data.get_temp::<f64>(scroll_bottom_id()))
        .is_some_and(|t| now - t < 15.0);
    let bottom_ready = want_bottom && !loading && !has_newer && !messages.is_empty() && jump_target.is_none();
    let mut did_scroll_bottom = false;

    let scroll_out = ScrollArea::vertical()
        .id_salt("chat_scroll")
        .max_height(list_height)
        .auto_shrink([false, false])
        // Mientras la ventana cargada NO llega al último mensaje del canal
        // (`has_newer`, p. ej. tras volver a una posición guardada o saltar
        // a un mensaje) no se pega al final: si se pegara, cada página de
        // "más nuevos" que llega haría saltar la vista abajo del todo, el
        // centinela de abajo seguiría visible y pediría la siguiente página,
        // en cadena hasta cargar todo el canal.
        .stick_to_bottom(!has_newer)
        // Sin animar los reanclajes: al llegar una página de mensajes más
        // viejos, `scroll_to_rect` animado arrastraba la vista ~2500 px
        // hacia abajo y, con la rueda girando rápido, el usuario volvía a
        // chocar con el tope una y otra vez (la "carga infinita").
        // Instantáneo, la página aparece arriba sin mover lo que se ve.
        .animated(false)
        .show(ui, |ui| {
            // El estilo global (`theme::apply`) deja `item_spacing.y` en
            // 6px para el resto de la app — acá, dentro de la lista de
            // mensajes, eso se sumaba SOBRE el `ui.add_space(...)`
            // explícito que ya usamos para separar cada fila (egui inserta
            // ese espaciado por defecto antes de cada widget nuevo en un
            // layout vertical, `add_space` incluido). El resultado era una
            // separación real de "6 + lo que pusimos + 6" en vez de
            // exactamente lo que pusimos, así que agrupados y no agrupados
            // terminaban viéndose con una diferencia mucho más chica —y
            // más pareja/rara— de la que las constantes de acá abajo
            // hacían pensar. Dejando esto en 0, `add_space` pasa a ser la
            // ÚNICA fuente de separación vertical entre mensajes.
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(8.0);
            if !limits.can_read_history {
                history_notice(ui, palette);
            }
            if loading && messages.is_empty() {
                // Todavía no llegó ni la primera página: placeholder tipo
                // "esqueleto" en vez de la lista vacía, como hace el
                // cliente real mientras carga un canal/DM.
                skeleton_rows(ui, palette, SKELETON_INITIAL);
            } else if messages.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    theme::text(ui, "Todavía no hay mensajes acá.", theme::regular(12.5), palette.dim);
                });
            } else {
                // Mientras pueda quedar historial más viejo (o se esté
                // pidiendo), arriba de todo hay un esqueleto ALTO que es parte
                // del contenido del scroll: se puede seguir subiendo por
                // encima de los mensajes cargados y mientras tanto se ve el
                // shimmer. Al llegar la página, los mensajes lo reemplazan
                // sin mover lo que el usuario tenía en pantalla (ver
                // `prev_row_views` y el reanclaje más abajo). Como ya está
                // ahí desde antes del pedido, tampoco empuja la lista de
                // golpe cuando el pedido arranca.
                if has_more || loading_more {
                    skeleton_history(ui, palette, list_height * SKELETON_HISTORY_VIEWPORTS);
                }
                // Centinela de 1px justo debajo del esqueleto (arriba del
                // primer mensaje): si el
                // `ScrollArea` todavía lo está pintando (`is_rect_visible`,
                // mismo criterio que ya se usa más abajo para las filas de
                // mensajes) es que el usuario scrolleó hasta arriba de lo
                // ya cargado, así que corresponde pedir la página anterior
                // — siempre que no haya ya un pedido en curso y todavía
                // pueda quedar historial más viejo.
                //
                // El chequeo de `scroll_anchor` de acá es necesario porque
                // el centinela SIEMPRE está en la posición y=0 del
                // contenido (nunca se mueve, aunque se antepongan
                // mensajes) y el offset del scroll recién se corrige
                // (ver `scroll_anchor` más abajo) DESPUÉS de que esta
                // parte ya corrió en el mismo frame — así que, sin este
                // chequeo, el mismo frame en el que llega una página
                // vieja todavía ve el centinela "visible" (con el offset
                // viejo, antes de corregirse) y dispara OTRO pedido de
                // "más" enseguida, en cascada, sin que el usuario haya
                // hecho nada. Eso es lo que se sentía como "se queda un
                // toque y después salta a cualquier lado": cada pedido
                // encadenado tarda lo que tarda la red, y cuando
                // resuelve reubica el scroll en el ancla de ESE pedido,
                // pisando lo que el usuario haya hecho mientras tanto.
                // Mientras haya un ancla pendiente de consumir, no
                // corresponde pedir una página más todavía.
                let (sentinel_rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                // Mientras el usuario tenga el botón del mouse apretado
                // (por ejemplo arrastrando el handle de la barra de scroll,
                // o clickeando sostenido sobre su riel) no correspondía
                // seguir disparando pedidos de "más" cada vez que soltar el
                // centinela pasaba por la vista: antes, mantener clickeada
                // la barra de scroll arriba del todo bastaba para que cada
                // página que llegaba dejara el centinela visible de nuevo
                // enseguida y encadenara un pedido tras otro sin que el
                // usuario soltara nada. Con el botón sostenido, esperamos a
                // que lo suelte para recién ahí evaluar si corresponde
                // pedir la página siguiente.
                let pointer_held = ui.ctx().input(|i| i.pointer.primary_down());
                if !loading_more
                    && has_more
                    && scroll_anchor.is_none()
                    && jump_target.is_none()
                    && !pointer_held
                    && !load_more_locked(ui.ctx())
                    && ui.is_rect_visible(sentinel_rect)
                {
                    load_more_requested = true;
                }
            }
            // Rect visible de verdad (recortado por el `ScrollArea`) tal
            // como está el scroll ESTE frame, y cache de altos ya medidos
            // por id de mensaje — ver la explicación grande de
            // virtualización más abajo.
            let clip_rect = ui.clip_rect();
            let heights = row_height_cache(ui.ctx());

            for i in 0..messages.len() {
                // Agrupado si el mensaje anterior es del mismo autor (mismo
                // nombre Y mismo "es mío" — así dos personas distintas que
                // por casualidad se llaman igual en la demo no se agrupan
                // entre sí, y un mensaje propio nunca se agrupa con uno
                // ajeno aunque comparta nombre de display).
                let grouped = i > 0
                    && messages[i - 1].kind != 18
                    && messages[i].kind != 18
                    && messages[i - 1].same_author(&messages[i])
                    // Pasados 7 minutos entre un mensaje y el siguiente del
                    // mismo autor, el segundo vuelve a llevar encabezado.
                    && messages[i - 1].within_group_gap(&messages[i])
                    // Una respuesta siempre arranca un bloque nuevo, con
                    // avatar/header propios arriba del banner citado — no
                    // tendría sentido agruparla bajo el mensaje anterior
                    // y que el banner "Respondiendo a..." quede sin
                    // ningún encabezado propio, como en el cliente real.
                    && messages[i].replied_to.is_none()
                    // La línea "NUEVO" corta la racha: el primer mensaje
                    // nuevo arranca con su propio header.
                    && divider_index != Some(i);

                if divider_index == Some(i) {
                    unread_divider(ui, palette);
                }

                // VIRTUALIZACIÓN: armar cada fila entera (parsear el
                // markdown, pedir el avatar, las reacciones...) cuesta lo
                // mismo esté o no a la vista — y antes se hacía para
                // TODOS los mensajes cargados, cada frame, sin importar
                // cuántos hubiera. Con un historial corto no se nota, pero
                // apenas se cargan varias páginas de "más viejos" ese
                // costo crece con el total de mensajes en memoria (no con
                // los ~20 que entran en pantalla) y termina siendo el
                // motivo de que la app se vaya poniendo cada vez más
                // pesada cuantos más mensajes se van acumulando.
                //
                // Para evitarlo, a una fila que va a caer bien afuera de
                // lo visible (más allá de `VIRTUALIZE_BUFFER` de margen)
                // no la armamos de verdad: le reservamos el alto que
                // midió la ÚLTIMA vez que sí se dibujó (`heights`, por id
                // de mensaje real — los ecos locales sin id, que son
                // poquísimos, siempre se dibujan enteros) o, si todavía
                // nunca se dibujó, una estimación
                // (`ESTIMATED_ROW_HEIGHT_*`). El resto de la fila (ancla
                // de scroll al cargar "más", espaciado con la próxima)
                // sigue funcionando igual usando ese rect estimado en vez
                // del real, así que entrar por primera vez a una zona
                // nunca vista solo implica dibujarla entera esa vez (con
                // un reacomodo mínimo si la estimación no daba justo) — de
                // ahí en más ya queda el alto real en cache.
                let cached_height = if messages[i].id.is_empty() {
                    None
                } else {
                    heights.lock().unwrap().get(&messages[i].id).copied()
                };
                let estimated_height = cached_height.unwrap_or(if grouped {
                    ESTIMATED_ROW_HEIGHT_GROUPED
                } else {
                    ESTIMATED_ROW_HEIGHT_HEADER
                });
                let estimated_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    Vec2::new(ui.available_width().max(1.0), estimated_height),
                );
                // Además de "está a la vista", también se dibuja entera
                // si TODAVÍA no se midió nunca (`cached_height` en
                // `None`) — si no, un mensaje recién llegado (por
                // ejemplo, toda una página nueva de "más viejos" que se
                // acaba de anteponer) se queda con la estimación fija de
                // acá arriba hasta que el scroll lo alcanza, momento en
                // el que su alto salta de golpe de la estimación al real
                // y corre todo lo que tiene debajo — eso es,
                // literalmente, el salto. Midiéndolo una sola vez apenas
                // llega (aunque en ese momento esté afuera de lo visible)
                // el costo es chico —una página, un frame— y de ahí en
                // más ya queda con su alto real en cache para siempre, sin
                // sorpresas mientras se scrollea.
                let within_viewport = cached_height.is_none()
                    || (estimated_rect.bottom() >= clip_rect.top() - VIRTUALIZE_BUFFER
                        && estimated_rect.top() <= clip_rect.bottom() + VIRTUALIZE_BUFFER);

                let row_rect = if within_viewport {
                    // Backlight de esta fila, pintado ANTES de su contenido
                    // (con el rect del frame pasado) para que quede detrás.
                    if backlit_index == Some(i) {
                        if let Some((_, prev_rect)) = prev_row_rects.iter().find(|(idx, _)| *idx == i) {
                            paint_backlight(ui, palette, *prev_rect);
                        }
                    }
                    let rect = message_row(
                        ui,
                        palette,
                        &messages[i],
                        channels,
                        grouped,
                        i,
                        &mut toggled_reaction,
                        &mut actions,
                    );
                    if !messages[i].id.is_empty() {
                        heights.lock().unwrap().insert(messages[i].id.clone(), rect.height());
                    }
                    // Solo cuenta como "hovereable" (y por lo tanto puede
                    // tener barra flotante) si de verdad se pintó este frame
                    // — `ui.is_rect_visible` da `false` para las filas que el
                    // `ScrollArea` recortó por estar arriba o abajo de lo que
                    // se ve ahora mismo. Antes se guardaba el rect igual,
                    // aunque estuviera fuera del área visible, así que con el
                    // mouse quieto en un punto fijo y la lista scrolleando
                    // por scroll del mouse/teclado, una fila que ya no se ve
                    // podía seguir "ganando" el hover (por cercanía, ver
                    // `hovered_row_index`) y dejar la barra flotante mostrada
                    // sobre un mensaje invisible.
                    if ui.is_rect_visible(rect) {
                        row_rects.push((i, rect));
                    }
                    rect
                } else {
                    // Fuera del viewport (+ margen): ni backlight, ni
                    // barra flotante, ni contenido — solo el espacio.
                    ui.add_space(estimated_height);
                    estimated_rect
                };
                // Posición en pantalla de esta fila (solo la primera y las que
                // se ven) para poder reanclar sin saltos cuando llegue una
                // página más vieja.
                if !messages[i].id.is_empty()
                    && (i == 0
                        || (row_rect.bottom() >= clip_rect.top() - 1.0
                            && row_rect.top() <= clip_rect.bottom() + 1.0))
                {
                    row_views.push((messages[i].id.clone(), row_rect.top() - clip_rect.top()));
                }
                // ¿Ya se ve el primer mensaje nuevo? (oculta la barra de arriba)
                if unread_live
                    && unread_view.as_ref().is_some_and(|v| v.first_index == i)
                    && row_rect.top() >= clip_rect.top() - 4.0
                    && row_rect.top() < clip_rect.bottom() - 24.0
                {
                    unread_first_visible = true;
                }
                // Primer mensaje con algo a la vista: es "donde estás" para
                // retomar el chat en el mismo lugar al volver a abrirlo.
                if top_visible.is_none()
                    && !messages[i].id.is_empty()
                    && row_rect.bottom() > clip_rect.top() + 1.0
                    && row_rect.top() < clip_rect.bottom()
                {
                    top_visible = Some((messages[i].id.clone(), clip_rect.top() - row_rect.top()));
                }
                // Si este es el mensaje que estaba arriba del todo cuando
                // se pidió "cargar más" (ver `scroll_anchor`), y la página
                // más vieja ya se antepuso (`!loading_more`: si todavía
                // está en curso, `messages` no cambió aún y este mismo
                // `if` matchearía de una, ANTES de que la nueva página
                // exista, gastando el ancla para nada), lo volvemos a
                // dejar arriba del scroll — así la lista no "salta" apenas
                // aparecen mensajes más viejos por encima. Funciona igual
                // esté la fila dibujada entera o virtualizada: en los dos
                // casos `row_rect` refleja dónde está (o va a estar) esta
                // fila en este frame.
                if !loading_more
                    && !messages[i].id.is_empty()
                    && scroll_anchor.as_deref() == Some(messages[i].id.as_str())
                {
                    // El mensaje vuelve al MISMO lugar de la pantalla donde
                    // estaba justo antes de que llegara la página (no al
                    // borde de arriba): lo que el usuario veía queda quieto y
                    // los mensajes nuevos aparecen donde estaba el esqueleto.
                    // Si no se tiene su posición anterior, cae al borde.
                    let held_at = prev_row_views
                        .iter()
                        .find(|(id, _)| id == &messages[i].id)
                        .map(|(_, rel)| *rel);
                    let target = match held_at {
                        Some(rel) => row_rect.translate(Vec2::new(0.0, -rel)),
                        None => row_rect,
                    };
                    ui.scroll_to_rect(target, Some(egui::Align::TOP));
                    moved_this_frame = true;
                    *scroll_anchor = None;
                    // Ver `LOAD_MORE_SETTLE`: recién a partir de acá
                    // arranca la ventana en la que no se dispara otro
                    // pedido automático, aunque el centinela vuelva a
                    // "verse" mientras la animación del scroll todavía está
                    // en camino hacia esta fila.
                    lock_load_more_until_settled(ui.ctx());
                }
                // Ir a un mensaje puntual (`App::pending_jump`): se centra la
                // fila en el scroll y se resalta un rato, como el cliente real.
                if !messages[i].id.is_empty() && jump_target.as_deref() == Some(messages[i].id.as_str()) {
                    // Retomar donde se había quedado (posición guardada): el
                    // mensaje vuelve a su lugar exacto, sin centrarlo ni
                    // resaltarlo. Un salto común (respuesta, canal reciente)
                    // lo centra y lo resalta.
                    match crate::lib::last_view::take_restore(&fallback_channel, &messages[i].id) {
                        Some(offset) => {
                            let shifted = row_rect.translate(Vec2::new(0.0, offset.max(0.0)));
                            ui.scroll_to_rect(shifted, Some(egui::Align::TOP));
                        }
                        None => {
                            ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
                            start_jump_highlight(ui.ctx(), &messages[i].id);
                        }
                    }
                    moved_this_frame = true;
                    *jump_target = None;
                    lock_load_more_until_settled(ui.ctx());
                }
                if let Some(strength) = jump_highlight_strength(ui.ctx(), &messages[i].id) {
                    ui.painter().rect_filled(
                        row_rect.expand2(Vec2::new(8.0, 1.0)),
                        CornerRadius::same(theme::radius_small() + 2),
                        palette.accent.gamma_multiply(0.22 * strength),
                    );
                }
                // El espacio que va DESPUÉS de esta fila depende de si la
                // PRÓXIMA fila la va a continuar en la misma racha — no de
                // si esta fila continuó la racha anterior (`grouped`, que
                // ya se usó arriba para decidir el header de ESTA fila).
                // Antes se reusaba `grouped` acá y quedaba al revés: el
                // salto entre un mensaje con header y su primera línea
                // agrupada (que sí va pegada) usaba el espacio grande de
                // "autor nuevo", porque el mensaje CON header nunca está
                // "grouped" respecto al de arriba — pero eso no dice nada
                // de qué separación necesita respecto al de ABAJO.
                let next_is_grouped = messages
                    .get(i + 1)
                    .map(|next| {
                        next.kind != 18
                            && messages[i].kind != 18
                            && next.same_author(&messages[i])
                            && messages[i].within_group_gap(next)
                            && divider_index != Some(i + 1)
                    })
                    .unwrap_or(false);
                ui.add_space(if next_is_grouped { GROUPED_SPACING } else { NEW_GROUP_SPACING });
            }
            // Ventana que no termina en el último mensaje del canal: abajo
            // del todo se piden los siguientes (`?after=<id>`).
            //
            // Mientras llega la página posterior se muestra el esqueleto
            // animado. Como el usuario está pegado al final y el esqueleto
            // se agrega DEBAJO de lo que ve, se scrollea una sola vez hasta
            // dejarlo a la vista (si no, quedaría fuera de pantalla hasta
            // que scrollee de nuevo). Cuando llega la página, los mensajes
            // reemplazan al esqueleto en el mismo lugar, sin saltos.
            if has_newer && !messages.is_empty() {
                // Sentinel de 1px justo debajo del último mensaje: si se ve, hay que
                // pedir la página siguiente. Se evalúa ANTES del esqueleto para que el
                // pedido se dispare apenas se llega al fondo, y así el esqueleto puede
                // aparecer en el mismo lugar donde estaba el sentinel — sin saltos.
                if !loading_newer {
                    let (bottom_rect, _) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                    let pointer_held = ui.ctx().input(|i| i.pointer.primary_down());
                    if scroll_anchor.is_none()
                        && jump_target.is_none()
                        && !pointer_held
                        && !load_more_locked(ui.ctx())
                        && ui.is_rect_visible(bottom_rect)
                    {
                        load_newer_requested = true;
                    }
                }
                // Esqueleto de "más nuevos" en el mismo lugar del sentinel: aparece
                // mientras carga y NO empuja lo que ya se ve, porque `stick_to_bottom`
                // está en `false` (por `has_newer`) y no se fuerza ningún
                // `scroll_to_rect`. El salto a `Align::BOTTOM` era justamente lo que se
                // sentía como "tosco / encima del mensaje".
                if loading_newer {
                    skeleton_rows(ui, palette, SKELETON_NEWER);
                }
            }
            ui.add_space(8.0);
            if bottom_ready {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                did_scroll_bottom = true;
            }

            // Un solo mensaje puede estar "hovereado" a la vez. Antes esto
            // se decidía fila por fila mientras se dibujaban (cada una
            // preguntando "¿el mouse está cerca mío?", con una zona
            // ensanchada para no perder la barra flotante al subir el
            // cursor) — el problema es que esa zona ensanchada de dos
            // filas vecinas se superpone fácilmente (los mensajes quedan
            // MUY cerca entre sí), así que cerca del borde entre dos
            // mensajes ambos se consideraban hovereados a la vez. Ahora se
            // decide una única vez, con todos los rects ya conocidos (ver
            // `hovered_row_index`) — un solo ganador.
            let hovered_index = hovered_row_index(pointer_pos, &row_rects);

            // Click derecho sobre un mensaje: abre su menú en el cursor.
            if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary)) {
                if let Some(p) = pointer_pos {
                    let on_list = ui.ctx().layer_id_at(p) == Some(ui.layer_id());
                    if let (true, Some((idx, _))) = (on_list, row_rects.iter().find(|(_, r)| r.contains(p))) {
                        set_open_reaction_panel(ui.ctx(), None);
                        set_open_message_menu(ui.ctx(), Some(MsgMenu { index: *idx, pos: p, toggle: None }));
                    }
                }
            }

            for (index, rect) in &row_rects {
                let is_hovered = hovered_index == Some(*index);
                let panel_open_here = open_panel_index == Some(*index);
                let menu_open_here = open_menu.as_ref().is_some_and(|m| m.index == *index);
                if (is_hovered && open_menu.is_none()) || panel_open_here || menu_open_here {
                    hover_toolbar(
                        ui,
                        palette,
                        &messages[*index],
                        *index,
                        *rect,
                        &mut toggled_reaction,
                        reply_target,
                        &mut forward_requested,
                        &mut actions,
                        can_create_thread,
                    );
                }
                if let Some(menu) = open_menu.as_ref().filter(|m| m.index == *index) {
                    message_menu(
                        ui,
                        palette,
                        &messages[*index],
                        menu,
                        &mut actions,
                        can_create_thread,
                        MenuOutputs {
                            toggled_reaction: &mut toggled_reaction,
                            reply_target: &mut *reply_target,
                            forward_requested: &mut forward_requested,
                            event: &mut menu_event,
                            guild_id: menu_guild.as_deref(),
                            op: &mut message_op,
                            can_manage: can_manage_messages,
                        },
                    );
                }
                if panel_open_here {
                    reaction_panel(ui, palette, *index, *rect, &mut toggled_reaction, &mut nitro_required, custom_emojis);
                }
            }
        });

    ui.ctx().memory_mut(|m| m.data.insert_temp(row_rects_memory_id(), row_rects));
    ui.ctx().memory_mut(|m| m.data.insert_temp(row_views_memory_id(), row_views));
    if did_scroll_bottom {
        ui.ctx().memory_mut(|m| m.data.remove::<f64>(scroll_bottom_id()));
    }

    // Barras flotantes: "N mensajes nuevos desde..." (arriba) y "Estás
    // viendo mensajes antiguos" (abajo). Solo en el chat principal.
    let mut overlay_event: Option<ChatEvent> = menu_event;
    if scope() == 0 {
        let inner = scroll_out.inner_rect;
        let settled = !moved_this_frame
            && jump_target.is_none()
            && scroll_anchor.is_none()
            && !load_more_locked(ui.ctx());
        let mut banner_text: Option<String> = None;
        if let (Some(marker), Some(view)) = (unread.as_deref_mut(), unread_view.as_ref()) {
            if marker.had_unread && !marker.dismissed {
                if marker.created.elapsed() < UNREAD_ARM {
                    ui.ctx().request_repaint_after(UNREAD_ARM);
                } else if settled && unread_first_visible {
                    marker.dismissed = true;
                } else {
                    banner_text = Some(unread_banner_text(view));
                }
            }
        }
        let banner_t = ui
            .ctx()
            .animate_bool_with_time(scoped_id("ecord_unread_banner_anim"), banner_text.is_some(), 0.15);
        if let (Some(text), true) = (banner_text.as_deref(), banner_t > 0.01) {
            match draw_unread_banner(ui, palette, inner, text, banner_t) {
                BannerClick::Jump => overlay_event = Some(ChatEvent::JumpToFirstUnread),
                BannerClick::MarkRead => {
                    if let Some(marker) = unread.as_deref_mut() {
                        marker.dismissed = true;
                    }
                    if has_newer {
                        request_scroll_to_bottom(ui.ctx());
                    }
                    overlay_event = Some(ChatEvent::MarkAsRead);
                }
                BannerClick::None => {}
            }
        }

        let max_offset = (scroll_out.content_size.y - inner.height()).max(0.0);
        let dist_from_bottom = max_offset - scroll_out.state.offset.y;
        let show_old = !loading
            && !messages.is_empty()
            && (has_newer || dist_from_bottom > inner.height() * OLD_VIEW_VIEWPORTS);
        let old_t = ui
            .ctx()
            .animate_bool_with_time(scoped_id("ecord_old_bar_anim"), show_old, 0.15);
        if old_t > 0.01 && draw_old_messages_bar(ui, palette, inner, old_t) {
            request_scroll_to_bottom(ui.ctx());
            overlay_event = Some(ChatEvent::JumpToPresent);
        }
    }

    // Guarda dónde estás (mensaje de arriba + píxeles) para retomar el canal
    // en el mismo lugar después de cerrar el cliente (`lib::last_view`). Solo
    // en el chat principal (no en el panel de hilos), cuando el scroll ya se
    // asentó, y sin guardar nada si estás pegado al último mensaje: en ese
    // caso el canal vuelve a abrir en lo más nuevo, como siempre.
    if scope() == 0 && !loading && !messages.is_empty() {
        if let Some((_, channel_id, _)) = &send_target {
            let settled = !moved_this_frame
                && jump_target.is_none()
                && scroll_anchor.is_none()
                && !load_more_locked(ui.ctx());
            if settled {
                let max_offset = (scroll_out.content_size.y - scroll_out.inner_rect.height()).max(0.0);
                let at_bottom = !has_newer && scroll_out.state.offset.y >= max_offset - 4.0;
                if at_bottom {
                    crate::lib::last_view::record_position(channel_id, None);
                } else if top_visible.is_some() {
                    crate::lib::last_view::record_position(channel_id, top_visible.clone());
                }
            }
        }
    }

    // Confirmación de "Eliminar mensaje" (shift + click la salta).
    if let Some(confirm) = open_delete_confirm(ui.ctx()) {
        match messages.iter().find(|m| m.id == confirm.message_id) {
            None => set_delete_confirm(ui.ctx(), None),
            Some(msg) => match delete_confirm_dialog(ui, palette, msg) {
                Some(true) => {
                    message_op = Some(MessageOp::Delete { message_id: confirm.message_id.clone() });
                    set_delete_confirm(ui.ctx(), None);
                }
                Some(false) => set_delete_confirm(ui.ctx(), None),
                None => {}
            },
        }
    }

    // Borrar mensaje / sacar reacciones: se aplica en la lista local al
    // toque y se le avisa a Discord (si falla, solo queda en el log).
    if let Some(op) = message_op {
        let message_id = match &op {
            MessageOp::Delete { message_id }
            | MessageOp::ClearAll { message_id }
            | MessageOp::ClearEmoji { message_id, .. } => message_id.clone(),
        };
        if let Some(pos) = messages.iter().position(|m| m.id == message_id) {
            let channel_id = if messages[pos].channel_id.is_empty() {
                send_target.as_ref().map(|(_, c, _)| c.clone()).unwrap_or_default()
            } else {
                messages[pos].channel_id.clone()
            };
            let token = send_target.as_ref().map(|(t, _, _)| t.clone());
            match op {
                MessageOp::Delete { .. } => {
                    messages.remove(pos);
                    if let Some(token) = token.filter(|_| !channel_id.is_empty()) {
                        crate::discord::spawn_delete_message(token, channel_id, message_id);
                    }
                }
                MessageOp::ClearAll { .. } => {
                    messages[pos].reactions.clear();
                    if let Some(token) = token.filter(|_| !channel_id.is_empty()) {
                        crate::discord::spawn_clear_reactions(token, channel_id, message_id, None);
                    }
                }
                MessageOp::ClearEmoji { emoji, .. } => {
                    messages[pos].reactions.retain(|r| r.emoji != emoji);
                    if let Some(token) = token.filter(|_| !channel_id.is_empty()) {
                        crate::discord::spawn_clear_reactions(token, channel_id, message_id, Some(emoji.api_format()));
                    }
                }
            }
        }
    }

    // Sin "Añadir reacciones" no se puede poner una reacción NUEVA (sumarse a
    // una que ya existe sí).
    let toggled_reaction = toggled_reaction.filter(|(index, emoji)| {
        let is_new = messages
            .get(*index)
            .is_some_and(|m| !m.reactions.iter().any(|r| r.emoji == *emoji));
        let blocked = !limits.can_react && is_new;
        if blocked {
            set_notice(ui.ctx(), "No tienes permiso para añadir reacciones nuevas en este canal.");
        }
        !blocked
    });
    if let Some((index, emoji)) = toggled_reaction {
        if let Some(msg) = messages.get_mut(index) {
            let adding = msg.toggle_reaction(&emoji);
            // Solo avisamos a Discord de verdad si el mensaje es uno real
            // (tiene id) y estamos en un canal/DM real (`send_target`
            // trae token+channel_id). Los mensajes de demo se quedan con
            // el toggle solo local.
            if let Some((token, channel_id, _)) = send_target.clone() {
                // El mensaje inicial de un hilo vive en el canal padre: la
                // reacción se pone ahí, no en el hilo.
                let channel_id = if msg.channel_id.is_empty() { channel_id } else { msg.channel_id.clone() };
                if !msg.id.is_empty() {
                    crate::discord::spawn_toggle_reaction(
                        token,
                        channel_id,
                        msg.id.clone(),
                        emoji.api_format(),
                        adding,
                    );
                }
            }
        }
    }

    nitro_required |= composer(
        ui,
        palette,
        messages,
        compose_text,
        placeholder,
        own_author,
        own_color,
        send_target,
        reply_target,
        custom_emojis,
    );

    if let Some(event) = overlay_event {
        event
    } else if let Some((id, name, owner_id)) = actions.open_thread {
        ChatEvent::OpenThread { id, name, owner_id }
    } else if let Some((channel_id, message_id, name)) = actions.create_thread {
        let channel_id = if channel_id.is_empty() { fallback_channel } else { channel_id };
        ChatEvent::CreateThread { channel_id, message_id, name }
    } else if let Some(click) = actions.component {
        ChatEvent::Component(click)
    } else if let Some(message_id) = actions.jump_to {
        ChatEvent::JumpToMessage { message_id }
    } else if forward_requested {
        ChatEvent::ForwardRequested
    } else if nitro_required {
        ChatEvent::NitroRequired
    } else if load_more_requested {
        ChatEvent::LoadMoreRequested
    } else if load_newer_requested {
        ChatEvent::LoadNewerRequested
    } else {
        ChatEvent::None
    }
}

/// Cuánto espera la barra "N mensajes nuevos" antes de aparecer (si lo nuevo
/// ya se ve en pantalla, nunca llega a mostrarse).
const UNREAD_ARM: Duration = Duration::from_millis(500);
/// A cuántas alturas de lista del final aparece "Estás viendo mensajes antiguos".
const OLD_VIEW_VIEWPORTS: f32 = 1.5;

/// Mensajes sin leer del chat abierto, respecto de `UnreadMarker::last_read`.
struct UnreadView {
    /// Índice (en `messages`) del primer mensaje nuevo.
    first_index: usize,
    /// Cuántos mensajes nuevos (de otros) hay cargados.
    count: usize,
    /// `false` si la ventana cargada empieza después del último leído: hay
    /// más nuevos de los que se ven ("65+") y el punto exacto no se conoce.
    exact: bool,
    /// Id del mensaje cuya hora se muestra en "desde ...": el primer mensaje
    /// nuevo si se conoce, si no el último leído.
    since_id: String,
}

fn compute_unread_view(messages: &[ChatMessage], last_read: &str, has_more: bool) -> Option<UnreadView> {
    use crate::lib::notifications::snowflake;
    let last = snowflake(last_read);
    if last == 0 {
        return None;
    }
    let is_new = |m: &ChatMessage| !m.is_own && !m.id.is_empty() && snowflake(&m.id) > last;
    let first_index = messages.iter().position(|m| is_new(m))?;
    let count = messages.iter().filter(|m| is_new(*m)).count();
    let covers = messages.iter().any(|m| !m.id.is_empty() && snowflake(&m.id) <= last);
    let exact = covers || !has_more;
    let since_id = if exact { messages[first_index].id.clone() } else { last_read.to_string() };
    Some(UnreadView { first_index, count, exact, since_id })
}

/// Hora/día de un mensaje a partir de su id, en hora local: "las 0:29" (hoy),
/// "ayer a las 0:29" o "el 7 de octubre" (más viejo).
fn since_label(message_id: &str) -> String {
    use chrono::Datelike;
    let id: u64 = message_id.parse().unwrap_or(0);
    if id == 0 {
        return String::new();
    }
    let ms = (id >> 22) as i64 + 1_420_070_400_000;
    let Some(utc) = chrono::DateTime::from_timestamp_millis(ms) else { return String::new() };
    let local = utc.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    let days_ago = (today - local.date_naive()).num_days();
    let time = local.format("%-H:%M");
    match days_ago {
        i64::MIN..=0 => format!("las {time}"),
        1 => format!("ayer a las {time}"),
        _ => {
            const MONTHS: [&str; 12] = [
                "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto",
                "septiembre", "octubre", "noviembre", "diciembre",
            ];
            let month = MONTHS[local.month0() as usize];
            if local.year() == today.year() {
                format!("el {} de {month}", local.day())
            } else {
                format!("el {} de {month} de {}", local.day(), local.year())
            }
        }
    }
}

fn unread_banner_text(view: &UnreadView) -> String {
    let plus = if view.exact { "" } else { "+" };
    let noun = if view.count == 1 && view.exact { "mensaje nuevo" } else { "mensajes nuevos" };
    let since = since_label(&view.since_id);
    if since.is_empty() {
        format!("{}{plus} {noun}", view.count)
    } else {
        format!("{}{plus} {noun} desde {since}", view.count)
    }
}

fn scroll_bottom_id() -> egui::Id {
    scoped_id("ecord_scroll_to_bottom")
}

/// Pide bajar el scroll hasta el último mensaje apenas la lista esté lista.
fn request_scroll_to_bottom(ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    ctx.memory_mut(|m| m.data.insert_temp(scroll_bottom_id(), now));
}

/// Línea roja con la etiqueta "NUEVO" justo arriba del primer mensaje nuevo.
fn unread_divider(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let color = palette.danger;
    let luma = 0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
    let label_color = if luma > 150.0 { Color32::from_rgb(0x1a, 0x1a, 0x1a) } else { Color32::WHITE };
    let painter = ui.painter();
    let label_w = painter
        .layout_no_wrap("NUEVO".to_string(), theme::bold(10.0), label_color)
        .size()
        .x;
    let y = rect.center().y;
    painter.hline(
        (rect.left() + 16.0)..=(rect.right() - 16.0),
        y,
        Stroke::new(1.0, color),
    );
    let pill = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 16.0 - (label_w + 12.0) / 2.0, y),
        Vec2::new(label_w + 12.0, 15.0),
    );
    painter.rect_filled(pill, CornerRadius::same(8), color);
    painter.text(pill.center(), egui::Align2::CENTER_CENTER, "NUEVO", theme::bold(10.0), label_color);
}

enum BannerClick {
    None,
    Jump,
    MarkRead,
}

/// Barra de arriba: "65 mensajes nuevos desde las 0:29" + "Marcar como leído".
/// Click en la barra = ir al primer mensaje nuevo.
fn draw_unread_banner(ui: &mut egui::Ui, palette: &Palette, inner: egui::Rect, text: &str, alpha: f32) -> BannerClick {
    let size = Vec2::new((inner.width() - 24.0).max(120.0), 32.0);
    let pos = egui::pos2(inner.left() + 8.0, inner.top() + 6.0);
    let mut click = BannerClick::None;
    Area::new(scoped_id("ecord_unread_banner"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            let (rect, bar) = ui.allocate_exact_size(size, Sense::click());
            let bar = bar.on_hover_cursor(egui::CursorIcon::PointingHand);
            let fg = palette.on_accent.gamma_multiply(alpha);
            let font = theme::semibold(12.5);
            let show_label = rect.width() >= 430.0;
            let label = "Marcar como leído";
            let icon_size = 15.0;
            let label_w = if show_label {
                ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg).size().x
            } else {
                0.0
            };
            let btn_w = 8.0 + if show_label { label_w + 6.0 } else { 0.0 } + icon_size + 8.0;
            let btn_rect = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 6.0 - btn_w, rect.top() + 4.0),
                egui::pos2(rect.right() - 6.0, rect.bottom() - 4.0),
            );
            let btn = ui
                .interact(btn_rect, scoped_id("ecord_unread_mark_read"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);

            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(8), palette.accent.gamma_multiply(alpha));
            painter.text(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                fg,
            );
            if btn.hovered() {
                painter.rect_filled(btn_rect, CornerRadius::same(6), fg.gamma_multiply(0.18));
            }
            if show_label {
                painter.text(
                    egui::pos2(btn_rect.left() + 8.0, btn_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    font,
                    fg,
                );
            }
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(btn_rect.right() - 8.0 - icon_size / 2.0, btn_rect.center().y),
                Vec2::splat(icon_size),
            );
            theme::paint_icon(ui, Icon::CircleCheck, icon_rect, icon_size, fg);

            if btn.clicked() {
                click = BannerClick::MarkRead;
            } else if bar.clicked() {
                click = BannerClick::Jump;
            }
        });
    click
}

/// Píldora de abajo: "Estás viendo mensajes antiguos  [Ir al actual]".
/// Devuelve `true` si se apretó el botón.
fn draw_old_messages_bar(ui: &mut egui::Ui, palette: &Palette, inner: egui::Rect, alpha: f32) -> bool {
    let text = "Estás viendo mensajes antiguos";
    let label = "Ir al actual";
    let font = theme::semibold(12.5);
    let (text_w, label_w) = {
        let painter = ui.painter();
        (
            painter.layout_no_wrap(text.to_string(), font.clone(), palette.text).size().x,
            painter.layout_no_wrap(label.to_string(), font.clone(), palette.on_accent).size().x,
        )
    };
    let btn_size = Vec2::new(label_w + 22.0, 26.0);
    let size = Vec2::new(16.0 + text_w + 14.0 + btn_size.x + 5.0, 36.0);
    let x = (inner.center().x - size.x / 2.0).max(inner.left() + 4.0);
    let pos = egui::pos2(x, inner.bottom() - size.y - 10.0);
    let mut clicked = false;
    Area::new(scoped_id("ecord_old_messages_bar"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 5.0 - btn_size.x, rect.center().y - btn_size.y / 2.0),
                btn_size,
            );
            let btn = ui
                .interact(btn_rect, scoped_id("ecord_old_messages_go"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(18), palette.overlay.gamma_multiply(alpha));
            painter.rect_stroke(
                rect,
                CornerRadius::same(18),
                Stroke::new(1.0, palette.outline.gamma_multiply(alpha)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                egui::pos2(rect.left() + 16.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                palette.text.gamma_multiply(alpha),
            );
            let fill = if btn.hovered() { palette.accent_hover } else { palette.accent };
            painter.rect_filled(btn_rect, CornerRadius::same(13), fill.gamma_multiply(alpha));
            painter.text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                font,
                palette.on_accent.gamma_multiply(alpha),
            );
            clicked = btn.clicked();
        });
    clicked
}

/// Una fila del placeholder tipo "esqueleto": avatar + barra de nombre +
/// líneas de texto (+ opcionalmente un bloque de imagen/adjunto debajo),
/// igual que el que muestra el cliente real de Discord mientras llegan
/// los mensajes.
struct SkeletonRow {
    /// Ancho de la barra del "nombre", como fracción de `SKELETON_TEXT_WIDTH`.
    name_w: f32,
    /// Ancho de cada línea de texto, como fracción de `SKELETON_TEXT_WIDTH`
    /// (un valor por línea). Escritos a mano para que las filas no se vean
    /// todas idénticas.
    lines: &'static [f32],
    /// Bloque de imagen debajo del texto: (fracción de `SKELETON_MEDIA_WIDTH`, alto en px).
    media: Option<(f32, f32)>,
}

/// Ancho máximo (px) sobre el que se reparten las fracciones de texto.
const SKELETON_TEXT_WIDTH: f32 = 420.0;
/// Ancho máximo (px) del bloque de imagen del esqueleto.
const SKELETON_MEDIA_WIDTH: f32 = 460.0;

/// Carga inicial de un canal/DM (`loading` en `show`): llena la pantalla.
const SKELETON_INITIAL: &[SkeletonRow] = &[
    SkeletonRow { name_w: 0.33, lines: &[0.95], media: None },
    SkeletonRow { name_w: 0.28, lines: &[0.80, 0.55], media: Some((1.0, 220.0)) },
    SkeletonRow { name_w: 0.36, lines: &[0.55, 0.40, 0.90], media: None },
    SkeletonRow { name_w: 0.30, lines: &[0.45, 0.85, 0.35, 0.65], media: None },
    SkeletonRow { name_w: 0.26, lines: &[0.70, 0.45], media: None },
    SkeletonRow { name_w: 0.34, lines: &[0.60], media: None },
];

/// Historial más viejo (`has_more` / `loading_more`): se muestra arriba de la
/// lista, encima del mensaje más viejo ya cargado, repetido en ciclo hasta
/// llenar `SKELETON_HISTORY_VIEWPORTS` alturas de la lista (ver `skeleton_history`).
const SKELETON_OLDER: &[SkeletonRow] = &[
    SkeletonRow { name_w: 0.30, lines: &[0.85, 0.50], media: None },
    SkeletonRow { name_w: 0.26, lines: &[0.60], media: Some((0.80, 160.0)) },
    SkeletonRow { name_w: 0.34, lines: &[0.45, 0.75, 0.30], media: None },
    SkeletonRow { name_w: 0.28, lines: &[0.90], media: None },
];

/// Scroll hacia abajo (`loading_newer`): se muestra abajo de la lista,
/// debajo del mensaje más nuevo ya cargado.
const SKELETON_NEWER: &[SkeletonRow] = &[
    SkeletonRow { name_w: 0.32, lines: &[0.65, 0.35], media: None },
    SkeletonRow { name_w: 0.27, lines: &[0.90], media: None },
    SkeletonRow { name_w: 0.35, lines: &[0.50, 0.80, 0.40], media: None },
    SkeletonRow { name_w: 0.29, lines: &[0.55], media: Some((0.75, 150.0)) },
];

/// Duración (s) de una pasada completa de la franja de luz del shimmer.
const SKELETON_SWEEP_SECS: f64 = 1.4;
/// Ancho (px) de la franja de luz.
const SKELETON_BAND_WIDTH: f32 = 160.0;
/// Cuánto se inclina la franja (px de corrimiento horizontal por px hacia
/// abajo): con eso la luz baja en diagonal por las filas en vez de pegarles
/// a todas a la vez.
const SKELETON_BAND_SLANT: f32 = 0.35;

/// Colores y posición de la franja de luz de un frame, compartidos por todas
/// las formas del esqueleto para que el barrido sea continuo entre ellas.
struct Shimmer {
    base: Color32,
    highlight: Color32,
    /// X (en pantalla) del centro de la franja a la altura `y == 0`.
    center_x: f32,
}

impl Shimmer {
    /// Intensidad (0..=1) de la franja en `x` para una forma cuyo borde
    /// superior está en `top`.
    fn intensity(&self, x: f32, top: f32) -> f32 {
        let center = self.center_x + top * SKELETON_BAND_SLANT;
        (1.0 - (x - center).abs() / (SKELETON_BAND_WIDTH * 0.5)).clamp(0.0, 1.0)
    }

    /// Rectángulo redondeado con la franja de luz pasando por encima.
    fn rect(&self, painter: &egui::Painter, rect: egui::Rect, radius: f32, fade: f32) {
        let base = self.base.gamma_multiply(fade);
        painter.rect_filled(rect, radius, base);
        // La luz solo se pinta entre las puntas redondeadas (`radius` de cada
        // lado): ahí el rect es sólido en todo su alto, así que el gradiente
        // nunca se sale de la forma.
        let x0 = rect.left() + radius;
        let x1 = rect.right() - radius;
        if x1 <= x0 {
            return;
        }
        let center = self.center_x + rect.top() * SKELETON_BAND_SLANT;
        let half = SKELETON_BAND_WIDTH * 0.5;
        if center + half < x0 || center - half > x1 {
            return;
        }
        // Cortes del gradiente: bordes recortados + el pico si cae adentro.
        let mut xs = vec![x0.max(center - half)];
        if center > x0 && center < x1 {
            xs.push(center);
        }
        xs.push(x1.min(center + half));

        let mut mesh = egui::Mesh::default();
        for &x in &xs {
            let color = self.highlight.gamma_multiply(self.intensity(x, rect.top()) * fade);
            mesh.colored_vertex(egui::pos2(x, rect.top()), color);
            mesh.colored_vertex(egui::pos2(x, rect.bottom()), color);
        }
        for i in 0..(xs.len() as u32 - 1) {
            let v = i * 2;
            mesh.add_triangle(v, v + 1, v + 2);
            mesh.add_triangle(v + 1, v + 3, v + 2);
        }
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Círculo (avatar): demasiado chico para un gradiente, así que se
    /// mezcla el color con la intensidad de la franja en su centro.
    fn circle(&self, painter: &egui::Painter, rect: egui::Rect) {
        let t = self.intensity(rect.center().x, rect.top());
        painter.circle_filled(rect.center(), rect.width() * 0.5, extra::blend(self.base, self.highlight, t));
    }
}

/// Dibuja un conjunto de filas "esqueleto" (ver `SkeletonRow`) con un
/// shimmer: una franja de luz inclinada que barre las barras de izquierda a
/// derecha en loop, mientras las formas respiran apenas de brillo. Sirve
/// tanto para la carga inicial como para las páginas más viejas (arriba) y
/// más nuevas (abajo) que se piden al scrollear.
///
/// Si `min_height` es mayor que 0, las filas se repiten (en ciclo) hasta
/// llenar al menos ese alto, con UN solo shimmer continuo para todo el
/// bloque. Con 0 se dibuja cada fila una sola vez.
///
/// Devuelve el rect que ocupó todo el esqueleto.
fn skeleton_fill(ui: &mut egui::Ui, palette: &Palette, rows: &[SkeletonRow], min_height: f32) -> egui::Rect {
    let time = ui.input(|i| i.time);
    let top_left = ui.cursor().min;
    let span = ui.available_width().min(SKELETON_TEXT_WIDTH + 80.0);
    // La franja arranca fuera por la izquierda y termina fuera por la derecha.
    let phase = (time / SKELETON_SWEEP_SECS).fract() as f32;
    let travel = span + SKELETON_BAND_WIDTH * 2.0;
    let center_at_top = top_left.x - SKELETON_BAND_WIDTH + travel * phase;
    // Respiración muy sutil del brillo base, para que no quede estático
    // entre pasada y pasada.
    let breath = (time * 2.2).sin() as f32 * 0.5 + 0.5;
    let base = extra::blend(palette.surface, palette.surface_hover, 0.45 + 0.2 * breath);
    let highlight = extra::blend(palette.surface_active, palette.text, 0.10);
    let shimmer = Shimmer {
        base,
        highlight,
        // `intensity` suma el corrimiento por altura; se compensa para que
        // el centro sea `center_at_top` justo en el borde de arriba del esqueleto.
        center_x: center_at_top - top_left.y * SKELETON_BAND_SLANT,
    };
    ui.ctx().request_repaint_after(Duration::from_millis(16));

    let mut drawn = 0usize;
    for row in rows.iter().cycle() {
        // Una vuelta completa como mínimo; después se sigue repitiendo solo
        // hasta alcanzar `min_height`.
        if drawn >= rows.len()
            && (ui.cursor().min.y - top_left.y >= min_height || drawn >= SKELETON_MAX_ROWS)
        {
            break;
        }
        drawn += 1;
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (avatar_rect, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
            shimmer.circle(ui.painter(), avatar_rect);

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let text_w = ui.available_width().min(SKELETON_TEXT_WIDTH);
                // Barra del nombre: un toque más alta que las líneas.
                let (name_rect, _) =
                    ui.allocate_exact_size(Vec2::new(text_w * row.name_w, 12.0), Sense::hover());
                shimmer.rect(ui.painter(), name_rect, 4.0, 1.0);
                for &line_w in row.lines {
                    let (line_rect, _) =
                        ui.allocate_exact_size(Vec2::new(text_w * line_w, 10.0), Sense::hover());
                    shimmer.rect(ui.painter(), line_rect, 4.0, 1.0);
                }
                if let Some((media_w, media_h)) = row.media {
                    let width = ui.available_width().min(SKELETON_MEDIA_WIDTH) * media_w;
                    let (media_rect, _) =
                        ui.allocate_exact_size(Vec2::new(width, media_h), Sense::hover());
                    // El bloque de imagen va más tenue que las barras.
                    shimmer.rect(ui.painter(), media_rect, 8.0, 0.6);
                }
            });
        });
        ui.add_space(NEW_GROUP_SPACING);
    }
    egui::Rect::from_min_max(top_left, egui::pos2(top_left.x + ui.available_width(), ui.cursor().min.y))
}

/// Una pasada de filas de esqueleto, sin repetir (ver `skeleton_fill`).
fn skeleton_rows(ui: &mut egui::Ui, palette: &Palette, rows: &[SkeletonRow]) -> egui::Rect {
    skeleton_fill(ui, palette, rows, 0.0)
}

/// Esqueleto alto de "historial más viejo" que va arriba de la lista de
/// mensajes y es parte del contenido scrolleable. Se virtualiza igual que
/// las filas de mensajes: si cae bien afuera de lo visible solo se reserva
/// su alto (medido la última vez que se dibujó) y no se pinta ni se pide
/// repintado continuo para el shimmer.
fn skeleton_history(ui: &mut egui::Ui, palette: &Palette, min_height: f32) {
    let id = scoped_id("ecord_chat_history_skeleton_height");
    let cached: Option<(f32, f32)> = ui.ctx().memory(|m| m.data.get_temp(id));
    if let Some((for_min_height, height)) = cached {
        if (for_min_height - min_height).abs() < 1.0 {
            let rect = egui::Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width().max(1.0), height));
            let clip = ui.clip_rect();
            if rect.bottom() < clip.top() - VIRTUALIZE_BUFFER || rect.top() > clip.bottom() + VIRTUALIZE_BUFFER {
                ui.add_space(height);
                return;
            }
        }
    }
    let rect = skeleton_fill(ui, palette, SKELETON_OLDER, min_height);
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, (min_height, rect.height())));
}

fn row_views_memory_id() -> egui::Id {
    scoped_id("ecord_chat_row_views")
}

fn row_rects_memory_id() -> egui::Id {
    scoped_id("ecord_chat_row_rects")
}

fn load_more_lock_memory_id() -> egui::Id {
    scoped_id("ecord_chat_load_more_lock")
}

/// Si todavía estamos "asentando" el scroll después del último reanclaje
/// (ver `LOAD_MORE_SETTLE`) — mientras esto dé `true` no corresponde
/// disparar un pedido nuevo de "cargar más" aunque el centinela esté
/// visible.
fn load_more_locked(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.data.get_temp::<Instant>(load_more_lock_memory_id()))
        .is_some_and(|until| Instant::now() < until)
}

/// Arranca (o reinicia) la ventana de "asentado" de acá arriba, justo
/// después de reanclar el scroll al consumir `scroll_anchor`.
fn lock_load_more_until_settled(ctx: &egui::Context) {
    let until = Instant::now() + LOAD_MORE_SETTLE;
    ctx.memory_mut(|m| m.data.insert_temp(load_more_lock_memory_id(), until));
}

fn row_height_cache_memory_id() -> egui::Id {
    egui::Id::new("ecord_chat_row_heights")
}

/// Alto REAL medido de cada fila ya dibujada al menos una vez, por id de
/// mensaje (los mensajes de eco local sin id todavía no entran acá —
/// siempre se terminan dibujando enteros, ver `show`). Vive en la memoria
/// de egui como un `Arc<Mutex<..>>` en vez de guardarse y devolverse por
/// valor (como `row_rects`) porque con miles de mensajes cargados
/// clonar el `HashMap` entero en cada frame (una vez para leerlo, otra
/// para devolverlo) sería exactamente el tipo de costo-que-crece-con-el-
/// historial que esto existe para evitar: con el `Arc` el `get_temp`
/// clona un puntero, no el mapa.
fn row_height_cache(ctx: &egui::Context) -> Arc<Mutex<HashMap<String, f32>>> {
    if let Some(cache) = ctx.memory(|m| m.data.get_temp::<Arc<Mutex<HashMap<String, f32>>>>(row_height_cache_memory_id())) {
        return cache;
    }
    let cache = Arc::new(Mutex::new(HashMap::new()));
    ctx.memory_mut(|m| m.data.insert_temp(row_height_cache_memory_id(), cache.clone()));
    cache
}

/// Entre todos los `(índice, rect)` de mensajes ya dibujados, cuál es EL
/// mensaje sobre el que está el mouse ahora mismo — nunca más de uno.
/// Primero se busca si el puntero está literalmente adentro del rect de
/// alguna fila (ahí nunca hay ambigüedad: las filas no se superponen).
/// Si no está sobre ninguna al pixel exacto — está en el huequito entre
/// dos, o un poco arriba/abajo de todas para alcanzar la barra flotante
/// que sobresale del borde de la fila — se elige la más cercana, pero
/// solo dentro de `HOVER_ZONE_MARGIN`, y solo una: la de menor distancia.
fn hovered_row_index(pointer_pos: Option<egui::Pos2>, rows: &[(usize, egui::Rect)]) -> Option<usize> {
    let p = pointer_pos?;
    if let Some((idx, _)) = rows.iter().find(|(_, rect)| rect.contains(p)) {
        return Some(*idx);
    }
    rows.iter()
        .filter(|(_, rect)| p.x >= rect.left() - 4.0 && p.x <= rect.right() + 4.0)
        .filter_map(|(idx, rect)| {
            let dist = if p.y < rect.top() { rect.top() - p.y } else { p.y - rect.bottom() };
            (dist <= HOVER_ZONE_MARGIN).then_some((*idx, dist))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(idx, _)| idx)
}

/// Resalte sutil detrás de un mensaje hovereado — un rect un toque más
/// ancho que la fila, con las puntas redondeadas, muy parecido al
/// highlight que usa el cliente real de Discord. Se pinta ANTES del
/// contenido de la fila (ver el llamado en `show`) para quedar detrás.
fn paint_backlight(ui: &egui::Ui, palette: &Palette, rect: egui::Rect) {
    let highlight_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() - 8.0, rect.top() - 1.0),
        egui::pos2(rect.right() + 8.0, rect.bottom() + 1.0),
    );
    // Interfaz nueva: el resalte es una tarjetita más redondeada y suave.
    let (radius, fill) = if theme::is_modern() {
        (theme::radius_small() + 6, palette.surface)
    } else {
        (theme::radius_small() + 2, palette.surface_hover)
    };
    ui.painter().rect_filled(highlight_rect, CornerRadius::same(radius), fill);
}

fn reaction_panel_memory_id() -> egui::Id {
    scoped_id("ecord_chat_open_reaction_panel")
}

fn open_reaction_panel_index(ctx: &egui::Context) -> Option<usize> {
    ctx.memory(|m| m.data.get_temp(reaction_panel_memory_id()))
}

fn set_open_reaction_panel(ctx: &egui::Context, index: Option<usize>) {
    ctx.memory_mut(|m| match index {
        Some(i) => {
            m.data.insert_temp(reaction_panel_memory_id(), i);
        }
        None => {
            m.data.remove::<usize>(reaction_panel_memory_id());
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn message_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    msg: &ChatMessage,
    channels: Option<&HashMap<String, String>>,
    grouped: bool,
    index: usize,
    toggled_reaction: &mut Option<(usize, ReactionKind)>,
    actions: &mut RowActions,
) -> egui::Rect {
    // Mensaje de sistema "X ha empezado un hilo": una línea, sin avatar.
    if msg.kind == 18 {
        return thread_created_row(ui, palette, msg, actions);
    }
    // Envuelve todo en un `vertical` para que el banner "Respondiendo
    // a..." (si el mensaje es una respuesta) quede arriba del header, en
    // vez de compartir la misma fila horizontal que el avatar. El rect
    // que se devuelve termina incluyendo el banner, así que el hover y el
    // backlight cubren el bloque entero (banner + mensaje) — igual que en
    // el cliente real, donde pasar el mouse por la cita también resalta
    // el mensaje.
    let row = ui.vertical(|ui| {
    if let Some(replied) = &msg.replied_to {
        if reply_preview_row(ui, palette, replied) {
            actions.jump_to = Some(replied.message_id.clone());
        }
    }
    if let Some(line) = &msg.interaction {
        interaction_row(ui, palette, line);
    }
    ui
        .horizontal(|ui| {
            // Fuerza a que la fila ocupe todo el ancho disponible (no solo
            // lo que mide el texto), para que la barra flotante se pueda
            // anclar contra el borde derecho de verdad y para que el mouse
            // cuente como "sobre el mensaje" en toda la fila, no solo
            // sobre el texto.
            ui.set_min_width(ui.available_width());
            // Header, contenido y reacciones de un mismo mensaje quedan
            // más pegados entre sí que el espaciado por defecto del resto
            // de la app (6px) — ese espaciado grande es el que hacía que
            // la lista se sintiera "aireada de más".
            ui.spacing_mut().item_spacing.y = 2.0;
            if grouped {
                // Racha del mismo autor: sin avatar ni encabezado, el
                // contenido va directo, alineado bajo el texto del
                // mensaje anterior.
                ui.add_space(AVATAR_GUTTER);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - MESSAGE_RIGHT_GUTTER).max(120.0));
                    ui.spacing_mut().item_spacing.y = 2.0;
                    message_body(ui, palette, msg, channels, index, toggled_reaction, actions);
                });
            } else {
                ui.add_space(16.0);
                let (rect, avatar_response) =
                    ui.allocate_exact_size(Vec2::splat(36.0), egui::Sense::click());
                extra::avatar(ui, rect.center(), 18.0, msg.avatar_url.as_deref(), msg.avatar_color, &msg.initial(), palette);
                let avatar_response = avatar_response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if avatar_response.clicked() {
                    crate::ui::profile_popup::request_open(
                        ui.ctx(),
                        &msg.author_id,
                        &msg.author,
                        msg.avatar_url.clone(),
                        msg.avatar_color,
                    );
                }

                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - MESSAGE_RIGHT_GUTTER).max(120.0));
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        // Como en Discord: el nombre lleva el color del rol
                        // más alto del autor en este server. Sin rol con
                        // color (o en DMs) queda el color de siempre.
                        let default_color = if msg.is_own { palette.accent } else { palette.text };
                        let name_color = msg.author_color.unwrap_or(default_color);
                        let name_response = theme::text(ui, &msg.author, theme::semibold(13.5), name_color);
                        let name_response = ui
                            .interact(name_response.rect, name_response.id.with("profile"), egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if name_response.clicked() {
                            crate::ui::profile_popup::request_open(
                                ui.ctx(),
                                &msg.author_id,
                                &msg.author,
                                msg.avatar_url.clone(),
                                msg.avatar_color,
                            );
                        }
                        if msg.is_bot {
                            app_tag(ui, palette, msg.bot_verified);
                        }
                        theme::text(ui, &msg.time, theme::regular(10.5), palette.dim);
                    });
                    message_body(ui, palette, msg, channels, index, toggled_reaction, actions);
                });
            }
        })
    });
    let rect = row.response.rect;
    // Mensaje agrupado (sin encabezado): al pasar el mouse sale su hora en el
    // margen izquierdo, como en Discord. Solo la hora, aunque `msg.time`
    // lleve fecha. Se pinta directo, sin ocupar lugar, así la fila no salta.
    if grouped && ui.rect_contains_pointer(rect) {
        let short_time = msg.time.rsplit(' ').next().unwrap_or(&msg.time);
        ui.painter().text(
            egui::pos2(rect.left() + AVATAR_GUTTER / 2.0 + 2.0, rect.top() + 3.0),
            egui::Align2::CENTER_TOP,
            short_time,
            theme::regular(10.5),
            palette.dim,
        );
    }
    rect
}

/// Insignia "APP" que Discord pone junto al nombre de un bot (con un tilde
/// si es un bot verificado).
fn app_tag(ui: &mut egui::Ui, palette: &Palette, verified: bool) {
    let galley = ui
        .painter()
        .layout_no_wrap("APP".to_string(), theme::semibold(9.5), palette.on_accent);
    let check = if verified { 10.0 } else { 0.0 };
    let gap = if verified { 2.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + check + gap + 8.0, 15.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(3), palette.accent);
        let mut x = rect.left() + 4.0;
        if verified {
            let icon = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 5.0), Vec2::splat(10.0));
            theme::paint_icon(ui, Icon::Check, icon, 10.0, palette.on_accent);
            x += check + gap;
        }
        ui.painter().galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.on_accent,
        );
    }
}

/// Encabezado "[avatar] Fulano ha utilizado /comando" arriba de la respuesta
/// de un bot a un comando, con el mismo gancho en L que una cita.
fn interaction_row(ui: &mut egui::Ui, palette: &Palette, line: &crate::lib::data::InteractionLine) {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.x = 4.0;
        let (hook_rect, _) = ui.allocate_exact_size(Vec2::new(AVATAR_GUTTER, 14.0), Sense::hover());
        let stroke = Stroke::new(1.5, palette.dim);
        let x = hook_rect.right() - 22.0;
        let mid_y = hook_rect.center().y;
        ui.painter().line_segment([egui::pos2(x, hook_rect.top()), egui::pos2(x, mid_y)], stroke);
        ui.painter().line_segment([egui::pos2(x, mid_y), egui::pos2(hook_rect.right() - 4.0, mid_y)], stroke);

        let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
        extra::avatar(
            ui,
            rect.center(),
            8.0,
            line.avatar_url.as_deref(),
            line.avatar_color,
            line.user.chars().next().map(|c| c.to_string()).unwrap_or_default().as_str(),
            palette,
        );
        let name = theme::text(ui, &line.user, theme::semibold(11.5), palette.dim);
        if response.clicked() || ui.interact(name.rect, name.id.with("profile"), Sense::click()).clicked() {
            crate::ui::profile_popup::request_open(
                ui.ctx(),
                &line.user_id,
                &line.user,
                line.avatar_url.clone(),
                line.avatar_color,
            );
        }
        theme::text(ui, "ha utilizado", theme::regular(11.5), palette.dim);
        Frame::new()
            .fill(palette.accent.gamma_multiply(0.22))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(5, 1))
            .show(ui, |ui| {
                theme::text(ui, &format!("/{}", line.command), theme::medium(11.5), palette.accent);
            });
    });
}

/// Línea "Fulano  contenido citado..." que aparece ARRIBA de un mensaje
/// que es respuesta a otro, con el gancho en L hacia el avatar del
/// mensaje real — igual que en el cliente real. Si Discord marcó el
/// mensaje como respuesta pero no mandó el original embebido (se borró, o
/// es muy viejo), se avisa en vez de mostrar un autor/preview que no
/// tenemos (ver `RepliedMessage::deleted`).
/// Devuelve `true` si se clickeó la cita (y se sabe a qué mensaje ir).
fn reply_preview_row(ui: &mut egui::Ui, palette: &Palette, replied: &RepliedMessage) -> bool {
    let row = ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.x = 4.0;
        // Reserva el mismo ancho que el avatar + su padding (`AVATAR_GUTTER`)
        // para que el texto de la cita quede alineado bajo el nombre del
        // mensaje real, y dibuja ahí el gancho en L que lo conecta con el
        // avatar de abajo.
        let (hook_rect, _) = ui.allocate_exact_size(Vec2::new(AVATAR_GUTTER, 14.0), Sense::hover());
        let stroke = Stroke::new(1.5, palette.dim);
        let x = hook_rect.right() - 22.0;
        let mid_y = hook_rect.center().y;
        ui.painter().line_segment([egui::pos2(x, hook_rect.top()), egui::pos2(x, mid_y)], stroke);
        ui.painter().line_segment([egui::pos2(x, mid_y), egui::pos2(hook_rect.right() - 4.0, mid_y)], stroke);

        if replied.deleted {
            theme::text(ui, "Mensaje original eliminado", theme::regular(11.5), palette.dim);
        } else {
            let author = if replied.author.is_empty() { "Alguien" } else { replied.author.as_str() };
            theme::text(ui, author, theme::semibold(11.5), replied.author_color.unwrap_or(palette.dim));
            if !replied.preview.is_empty() {
                theme::text(ui, &replied.preview, theme::regular(11.5), palette.dim);
            }
        }
    });
    if replied.deleted || replied.message_id.is_empty() {
        return false;
    }
    ui.interact(row.response.rect, row.response.id.with("reply_jump"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn jump_highlight_memory_id() -> egui::Id {
    scoped_id("ecord_chat_jump_highlight")
}

/// Cuánto dura el resaltado del mensaje al que se saltó (segundos).
const JUMP_HIGHLIGHT_SECS: f64 = 2.5;

/// Marca `message_id` como el recién visitado: se resalta un rato.
fn start_jump_highlight(ctx: &egui::Context, message_id: &str) {
    let now = ctx.input(|i| i.time);
    ctx.memory_mut(|m| m.data.insert_temp(jump_highlight_memory_id(), (message_id.to_string(), now)));
    ctx.request_repaint();
}

/// Intensidad (1 → 0) del resaltado de `message_id`, o `None` si no es el
/// mensaje resaltado o ya se terminó.
fn jump_highlight_strength(ctx: &egui::Context, message_id: &str) -> Option<f32> {
    if message_id.is_empty() {
        return None;
    }
    let (id, started) = ctx.memory(|m| m.data.get_temp::<(String, f64)>(jump_highlight_memory_id()))?;
    if id != message_id {
        return None;
    }
    let elapsed = ctx.input(|i| i.time) - started;
    if elapsed >= JUMP_HIGHLIGHT_SECS {
        return None;
    }
    ctx.request_repaint();
    Some((1.0 - elapsed / JUMP_HIGHLIGHT_SECS) as f32)
}

/// Todo lo que va debajo del encabezado de un mensaje: el texto, los
/// adjuntos (imágenes/videos/archivos), los stickers, los embeds y las reacciones — en
/// ese orden, como en el cliente real. Es lo mismo para un mensaje con
/// avatar y para uno agrupado, por eso vive acá y no repetido en cada rama
/// de `message_row`.
fn message_body(
    ui: &mut egui::Ui,
    palette: &Palette,
    msg: &ChatMessage,
    channels: Option<&HashMap<String, String>>,
    index: usize,
    toggled_reaction: &mut Option<(usize, ReactionKind)>,
    actions: &mut RowActions,
) {
    // Un mensaje que es solo el link de una imagen/GIF se ve como la
    // imagen sola, sin la URL repetida arriba (ver `media::hides_content`).
    if !media::hides_content(&msg.content, &msg.embeds) {
        let ctx = MentionCtx { mentions: &msg.mentions, channels };
        crate::ui::markdown::show(ui, palette, &msg.content, 13.5, palette.text, &ctx);
    }
    if let Some(forward) = &msg.forwarded {
        forward_block(ui, palette, forward, channels);
    }
    media::show_attachments(ui, palette, &msg.attachments);
    media::show_stickers(ui, palette, &msg.stickers);
    media::show_embeds(ui, palette, &msg.embeds);
    // Tarjeta de invitación (`discord.gg/...`): Discord no manda embed para
    // las invitaciones, la arma el cliente (ver `ui::invite_card`).
    crate::ui::invite_card::show(ui, palette, &msg.content, msg.is_own);
    // Botones del bot (debajo de los embeds, arriba de las reacciones).
    if let Some(click) = crate::ui::components::show(ui, palette, msg) {
        actions.component = Some(click);
    }
    if msg.flags & crate::ui::components::FLAG_EPHEMERAL != 0 {
        ui.add_space(2.0);
        theme::text(ui, "Solo vos podés ver este mensaje.", theme::regular(11.5), palette.dim);
    }
    reactions_row(ui, palette, msg, index, toggled_reaction);
    if let Some(card) = msg.thread.as_ref() {
        thread_card(ui, palette, card, actions);
    }
}

/// Bloque "Reenviado" de un mensaje reenviado (`message_snapshots`): una
/// barra a la izquierda, el rótulo, el contenido del original (texto,
/// adjuntos, stickers, embeds — con los mismos renderers que un mensaje
/// normal) y, al pie, la hora original y el canal de donde venía si lo
/// conocemos.
fn forward_block(ui: &mut egui::Ui, palette: &Palette, forward: &Forward, channels: Option<&HashMap<String, String>>) {
    let source_channel = forward
        .source_channel_id
        .as_deref()
        .and_then(|id| channels.and_then(|names| names.get(id)));
    // Server de origen (nombre + ícono): solo si la cuenta está en él.
    let source_guild = forward
        .source_guild_id
        .as_deref()
        .and_then(|id| crate::lib::data::guild_brief(ui.ctx(), id));

    let block = ui.horizontal_top(|ui| {
        // Hueco para la barra, que se pinta después, cuando ya se sabe la
        // altura del bloque.
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                theme::icon(ui, Icon::ArrowRight, 13.0, palette.dim);
                ui.label(
                    egui::RichText::new("Reenviado")
                        .font(theme::semibold(12.0))
                        .color(palette.dim)
                        .italics(),
                );
            });

            if forward.snapshots.is_empty() {
                theme::text(ui, "No se pudo cargar el mensaje reenviado", theme::regular(12.5), palette.dim);
            }
            for snapshot in &forward.snapshots {
                if !media::hides_content(&snapshot.content, &snapshot.embeds) {
                    let ctx = MentionCtx { mentions: &snapshot.mentions, channels };
                    crate::ui::markdown::show(ui, palette, &snapshot.content, 13.5, palette.text, &ctx);
                }
                media::show_attachments(ui, palette, &snapshot.attachments);
                media::show_stickers(ui, palette, &snapshot.stickers);
                media::show_embeds(ui, palette, &snapshot.embeds);

                // Pie como en el cliente real: [ícono] Server · hora. Sin el
                // server (DM, o uno donde la cuenta no está) queda "#canal"
                // si es de este mismo server, y si no solo la hora.
                let mut when = snapshot.time.clone();
                if snapshot.edited {
                    when = format!("{when} (editado)").trim().to_string();
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let mut origin = false;
                    if let Some(guild) = &source_guild {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        extra::avatar(ui, rect.center(), 8.0, guild.icon_url.as_deref(), guild.color, &guild.initial, palette);
                        theme::text(ui, &guild.name, theme::medium(12.0), palette.dim);
                        origin = true;
                    } else if let Some(name) = source_channel {
                        theme::text(ui, format!("#{name}"), theme::medium(12.0), palette.dim);
                        origin = true;
                    }
                    if !when.is_empty() {
                        if origin {
                            theme::text(ui, "•", theme::regular(11.0), palette.dim);
                        }
                        theme::text(ui, &when, theme::regular(12.0), palette.dim);
                    }
                });
            }
        });
    });

    let rect = block.response.rect;
    let bar = egui::Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height()));
    ui.painter().rect_filled(bar, CornerRadius::same(2), palette.dim);
}

/// Segundos -> "hace 5 min" / "hace 3 h" / "hace 19 d".
fn ago(elapsed_ms: i64) -> String {
    let secs = elapsed_ms.max(0) / 1000;
    if secs < 60 {
        "hace un momento".to_string()
    } else if secs < 3600 {
        format!("hace {} min", secs / 60)
    } else if secs < 86_400 {
        format!("hace {} h", secs / 3600)
    } else {
        format!("hace {} d", secs / 86_400)
    }
}

/// Tarjeta "Hilo de respuestas · N mensajes ›" debajo del mensaje que
/// originó un hilo. Clickearla abre el hilo en el panel lateral.
fn thread_card(ui: &mut egui::Ui, palette: &Palette, card: &ThreadCard, actions: &mut RowActions) {
    ui.add_space(4.0);
    let inner = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::radius_small()))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            // Un ancho fijo de 380px (+ margen y borde, ~402px) no entra en
            // el panel lateral de hilos (420px menos avatar y márgenes del
            // mensaje): desbordaba y el panel entero crecía hacia la
            // izquierda, quedando tapado por el chat central. Dentro del
            // Frame `available_width` ya descuenta margen y borde, así que
            // alcanza con topearlo.
            ui.set_width(ui.available_width().min(THREAD_CARD_WIDTH));
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
            ui.horizontal(|ui| {
                theme::text(ui, &card.name, theme::semibold(13.0), palette.text);
                let count = match card.message_count {
                    0 => "Ver hilo ›".to_string(),
                    1 => "1 mensaje ›".to_string(),
                    n => format!("{n} mensajes ›"),
                };
                theme::text(ui, count, theme::semibold(12.0), palette.accent);
            });
            let now = crate::lib::data::now_ms();
            let detail = match card.last_activity_ms() {
                Some(ms) if now - ms <= THREAD_RECENT_MS => format!("Último mensaje {}", ago(now - ms)),
                Some(ms) => format!("No hay mensajes recientes en este hilo.  {}", ago(now - ms)),
                None => "Todavía no hay mensajes en este hilo.".to_string(),
            };
            theme::text(ui, detail, theme::regular(12.0), palette.dim);
        });
    let response = ui
        .interact(inner.response.rect, ui.id().with(("thread_card", card.id.as_str())), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        actions.open_thread = Some((card.id.clone(), card.name.clone(), card.owner_id.clone()));
    }
}

/// Mensaje de sistema "X ha empezado un hilo: <nombre>. Ver todos los
/// hilos." (tipo 18). El nombre del hilo abre el panel.
fn thread_created_row(ui: &mut egui::Ui, palette: &Palette, msg: &ChatMessage, actions: &mut RowActions) -> egui::Rect {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space(16.0);
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::new(AVATAR_GUTTER - 16.0, 20.0), Sense::hover());
        theme::paint_icon(ui, Icon::ListPlus, egui::Rect::from_center_size(icon_rect.center(), Vec2::splat(16.0)), 16.0, palette.dim);
        theme::text(ui, &msg.author, theme::semibold(13.5), msg.author_color.unwrap_or(palette.text));
        theme::text(ui, "ha empezado un hilo:", theme::regular(13.5), palette.dim);
        match msg.thread.as_ref() {
            Some(card) => {
                let name = if msg.content.is_empty() { card.name.as_str() } else { msg.content.as_str() };
                if theme::link(ui, name, theme::semibold(13.5), palette.text).clicked() {
                    actions.open_thread = Some((card.id.clone(), name.to_string(), card.owner_id.clone()));
                }
            }
            None => {
                theme::text(ui, &msg.content, theme::semibold(13.5), palette.text);
            }
        }
        theme::text(ui, ". Ver todos los hilos.", theme::regular(13.5), palette.dim);
        theme::text(ui, &msg.time, theme::regular(10.5), palette.dim);
    })
    .response
    .rect
}

/// Menú abierto de un mensaje (botón "..." de la barra flotante o click
/// derecho). Vive en la memoria de egui, como el panel de reacciones.
#[derive(Clone)]
struct MsgMenu {
    index: usize,
    /// Esquina superior izquierda del menú (egui lo corre si no entra).
    pos: egui::Pos2,
    /// Botón "..." que lo abrió (apretarlo de nuevo lo cierra).
    toggle: Option<egui::Rect>,
}

fn message_menu_id() -> egui::Id {
    scoped_id("ecord_message_menu")
}

fn open_message_menu(ctx: &egui::Context) -> Option<MsgMenu> {
    ctx.memory(|m| m.data.get_temp(message_menu_id()))
}

fn set_open_message_menu(ctx: &egui::Context, menu: Option<MsgMenu>) {
    ctx.memory_mut(|m| match menu {
        Some(menu) => {
            m.data.insert_temp(message_menu_id(), menu);
        }
        None => {
            m.data.remove::<MsgMenu>(message_menu_id());
        }
    });
    ctx.request_repaint();
}

/// Botón cuadrado con un emoji (reacciones rápidas de la barra flotante).
fn quick_emoji_button(ui: &mut egui::Ui, palette: &Palette, emoji: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOOLBAR_BUTTON), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
    }
    let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(18.0));
    if twemoji::paint(ui, icon_rect, emoji) == twemoji::State::Failed {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            emoji,
            theme::regular(16.0),
            palette.text,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Barra flotante que aparece al pasar el mouse por un mensaje: tres
/// reacciones rápidas (las que más usa la cuenta), separador, agregar
/// reacción, responder, reenviar y "..." (menú del mensaje, ver
/// `message_menu`).
#[allow(clippy::too_many_arguments)]
fn hover_toolbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    msg: &ChatMessage,
    index: usize,
    row_rect: egui::Rect,
    toggled_reaction: &mut Option<(usize, ReactionKind)>,
    reply_target: &mut Option<ReplyTarget>,
    forward_requested: &mut bool,
    _actions: &mut RowActions,
    _can_create_thread: bool,
) {
    let pos = egui::pos2(row_rect.right() - TOOLBAR_WIDTH - 12.0, row_rect.top() - TOOLBAR_BUTTON / 2.0);

    Area::new(egui::Id::new(("msg_hover_toolbar", scope(), index)))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius()))
                .inner_margin(Margin::symmetric(4, 3))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

                        for emoji in quick_reactions().into_iter().take(3) {
                            if quick_emoji_button(ui, palette, &emoji).clicked() {
                                if let Some(key) = crate::discord::frecency::emoji_key_for_unicode(&emoji) {
                                    crate::discord::frecency::record_reaction_use(&key);
                                }
                                *toggled_reaction = Some((index, ReactionKind::Unicode(emoji)));
                            }
                        }

                        // Separador vertical.
                        let (sep, _) = ui.allocate_exact_size(Vec2::new(9.0, TOOLBAR_BUTTON), Sense::hover());
                        ui.painter().vline(
                            sep.center().x,
                            (sep.top() + 5.0)..=(sep.bottom() - 5.0),
                            Stroke::new(1.0, palette.outline),
                        );

                        if theme::icon_button(ui, Icon::Smile, 16.0, palette.dim, palette.text, "Añadir reacción").clicked() {
                            set_open_message_menu(ui.ctx(), None);
                            set_open_reaction_panel(ui.ctx(), Some(index));
                        }

                        if theme::icon_button(ui, Icon::Reply, 16.0, palette.dim, palette.text, "Responder").clicked() {
                            *reply_target = Some(ReplyTarget {
                                message_id: msg.id.clone(),
                                author: msg.author.clone(),
                                preview: preview_text(&msg.content),
                            });
                        }

                        // Reenviar: todavía no hay selector de canal/DM
                        // destino; solo le avisa a quien llamó a `show`
                        // (ver `ChatEvent`), que muestra un toast.
                        if theme::icon_button(ui, Icon::Forward, 16.0, palette.dim, palette.text, "Reenviar").clicked() {
                            *forward_requested = true;
                        }

                        let more = theme::icon_button(ui, Icon::Ellipsis, 16.0, palette.dim, palette.text, "Más");
                        if more.clicked() {
                            let ctx = ui.ctx().clone();
                            if open_message_menu(&ctx).is_some_and(|m| m.index == index) {
                                set_open_message_menu(&ctx, None);
                            } else {
                                set_open_reaction_panel(&ctx, None);
                                set_open_message_menu(
                                    &ctx,
                                    Some(MsgMenu {
                                        index,
                                        pos: egui::pos2(more.rect.right() - MENU_WIDTH, more.rect.bottom() + 6.0),
                                        toggle: Some(more.rect),
                                    }),
                                );
                            }
                        }
                    });
                });
        });
}

/// Fila del menú: ícono + texto (+ flecha si abre un submenú).
fn menu_item(ui: &mut egui::Ui, palette: &Palette, icon: Icon, label: &str, arrow: bool, danger: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    let color = if danger { palette.danger } else { palette.text };
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
    }
    let cy = rect.center().y;
    theme::paint_icon(
        ui,
        icon,
        egui::Rect::from_center_size(egui::pos2(rect.left() + 22.0, cy), Vec2::splat(18.0)),
        18.0,
        if danger { color } else { palette.secondary },
    );
    ui.painter().text(
        egui::pos2(rect.left() + 44.0, cy),
        egui::Align2::LEFT_CENTER,
        label,
        theme::regular(14.0),
        color,
    );
    if arrow {
        theme::paint_icon(
            ui,
            Icon::ChevronRight,
            egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, cy), Vec2::splat(16.0)),
            16.0,
            palette.dim,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn menu_separator(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(
        (rect.left() + 4.0)..=(rect.right() - 4.0),
        rect.center().y,
        Stroke::new(1.0, palette.outline),
    );
}

/// Lo que el menú de un mensaje puede pedirle a `show`.
struct MenuOutputs<'a> {
    toggled_reaction: &'a mut Option<(usize, ReactionKind)>,
    reply_target: &'a mut Option<ReplyTarget>,
    forward_requested: &'a mut bool,
    event: &'a mut Option<ChatEvent>,
    /// Server del chat (`None` en un DM), para armar el enlace del mensaje.
    guild_id: Option<&'a str>,
    /// Borrar el mensaje / sacar reacciones (se aplica en `show`).
    op: &'a mut Option<MessageOp>,
    /// La cuenta puede administrar mensajes en este canal.
    can_manage: bool,
}

/// Cambio pedido desde el menú de un mensaje.
enum MessageOp {
    Delete { message_id: String },
    ClearAll { message_id: String },
    ClearEmoji { message_id: String, emoji: ReactionKind },
}

/// Mensaje que espera confirmación para borrarse (memoria de egui).
#[derive(Clone)]
struct DeleteConfirm {
    message_id: String,
}

fn delete_confirm_id() -> egui::Id {
    scoped_id("ecord_delete_confirm_state")
}

fn open_delete_confirm(ctx: &egui::Context) -> Option<DeleteConfirm> {
    ctx.memory(|m| m.data.get_temp(delete_confirm_id()))
}

fn set_delete_confirm(ctx: &egui::Context, confirm: Option<DeleteConfirm>) {
    ctx.memory_mut(|m| match confirm {
        Some(confirm) => {
            m.data.insert_temp(delete_confirm_id(), confirm);
        }
        None => {
            m.data.remove::<DeleteConfirm>(delete_confirm_id());
        }
    });
    ctx.request_repaint();
}

/// Cartel "Eliminar mensaje": `Some(true)` = confirmar, `Some(false)` =
/// cancelar (botón o Esc), `None` = sigue abierto.
fn delete_confirm_dialog(ui: &mut egui::Ui, palette: &Palette, msg: &ChatMessage) -> Option<bool> {
    let mut result = None;
    let center = ui.clip_rect().center();
    Area::new(scoped_id("ecord_delete_confirm_area"))
        .order(Order::Foreground)
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_pos(center)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(18))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(360.0);
                    theme::text(ui, "Eliminar mensaje", theme::bold(17.0), palette.text);
                    ui.add_space(4.0);
                    theme::text(ui, "¿Seguro que quieres eliminar este mensaje?", theme::regular(13.5), palette.secondary);
                    ui.add_space(10.0);
                    Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.outline))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            theme::text(ui, &msg.author, theme::semibold(13.5), palette.text);
                            let preview = if msg.content.trim().is_empty() {
                                "(sin texto)".to_string()
                            } else {
                                preview_text(&msg.content)
                            };
                            theme::text(ui, &preview, theme::regular(13.0), palette.dim);
                        });
                    ui.add_space(14.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let danger = palette.danger;
                        let luma = 0.299 * danger.r() as f32 + 0.587 * danger.g() as f32 + 0.114 * danger.b() as f32;
                        let on_danger = if luma > 150.0 { Color32::from_rgb(0x1a, 0x1a, 0x1a) } else { Color32::WHITE };

                        let (rect, response) = ui.allocate_exact_size(Vec2::new(96.0, 34.0), Sense::click());
                        let fill = if response.hovered() { danger.gamma_multiply(0.85) } else { danger };
                        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Eliminar",
                            theme::semibold(13.5),
                            on_danger,
                        );
                        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            result = Some(true);
                        }

                        let (rect, response) = ui.allocate_exact_size(Vec2::new(90.0, 34.0), Sense::click());
                        if response.hovered() {
                            ui.painter()
                                .rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
                        }
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Cancelar",
                            theme::semibold(13.5),
                            palette.text,
                        );
                        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            result = Some(false);
                        }
                    });
                });
        });
    if result.is_none() && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        result = Some(false);
    }
    result
}

/// Menú de un mensaje (botón "..." o click derecho). Las opciones que
/// todavía no existen en este cliente (marcar mensaje, recordatorio,
/// aplicaciones, leer, ver reacciones y denunciar) avisan con un toast.
fn message_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    msg: &ChatMessage,
    menu: &MsgMenu,
    actions: &mut RowActions,
    can_create_thread: bool,
    out: MenuOutputs<'_>,
) {
    let MenuOutputs { toggled_reaction, reply_target, forward_requested, event, guild_id, op, can_manage } = out;
    // Lo de reacciones solo aparece si el mensaje tiene alguna.
    let has_reactions = !msg.reactions.is_empty() && !msg.id.is_empty();
    // Submenú "Eliminar reacciones": fila que lo abre (se arma abajo).
    let mut reactions_rect: Option<egui::Rect> = None;
    let ctx = ui.ctx().clone();
    let index = menu.index;
    let mut close = false;

    let shown = Area::new(scoped_id("ecord_message_menu_area"))
        .order(Order::Foreground)
        .fixed_pos(menu.pos)
        .show(&ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(6))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(MENU_WIDTH);
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);

                    // Reacciones rápidas.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                        let tile_w = (MENU_WIDTH - 3.0 * 6.0) / 4.0;
                        for emoji in quick_reactions().into_iter().take(4) {
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(tile_w, 40.0), Sense::click());
                            let fill = if response.hovered() { palette.surface_active } else { palette.surface_hover };
                            ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                            let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(22.0));
                            if twemoji::paint(ui, icon_rect, &emoji) == twemoji::State::Failed {
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    &emoji,
                                    theme::regular(18.0),
                                    palette.text,
                                );
                            }
                            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                if let Some(key) = crate::discord::frecency::emoji_key_for_unicode(&emoji) {
                                    crate::discord::frecency::record_reaction_use(&key);
                                }
                                *toggled_reaction = Some((index, ReactionKind::Unicode(emoji)));
                                close = true;
                            }
                        }
                    });
                    ui.add_space(4.0);

                    let item = |ui: &mut egui::Ui, icon: Icon, label: &str, arrow: bool, danger: bool| -> bool {
                        menu_item(ui, palette, icon, label, arrow, danger).clicked()
                    };

                    if item(ui, Icon::SmilePlus, "Añadir reacción", true, false) {
                        set_open_reaction_panel(&ctx, Some(index));
                        close = true;
                    }
                    if has_reactions && item(ui, Icon::Smile, "Ver reacciones", false, false) {
                        *event = Some(ChatEvent::Unavailable("Ver reacciones"));
                        close = true;
                    }
                    menu_separator(ui, palette);
                    if item(ui, Icon::Reply, "Responder", false, false) {
                        *reply_target = Some(ReplyTarget {
                            message_id: msg.id.clone(),
                            author: msg.author.clone(),
                            preview: preview_text(&msg.content),
                        });
                        close = true;
                    }
                    if item(ui, Icon::Forward, "Reenviar", false, false) {
                        *forward_requested = true;
                        close = true;
                    }

                    // Hilo: abre el que ya tiene el mensaje o, si se puede,
                    // crea uno (con el principio del mensaje de nombre).
                    let has_thread = msg.thread.is_some();
                    if !msg.id.is_empty() && (has_thread || (can_create_thread && msg.kind == 0)) {
                        let label = if has_thread { "Abrir hilo" } else { "Crear hilo" };
                        if item(ui, Icon::ListPlus, label, false, false) {
                            if let Some(card) = msg.thread.as_ref() {
                                actions.open_thread =
                                    Some((card.id.clone(), card.name.clone(), card.owner_id.clone()));
                            } else {
                                let name: String = msg.content.chars().take(100).collect();
                                let name = if name.trim().is_empty() { "Nuevo hilo".to_string() } else { name };
                                actions.create_thread = Some((msg.channel_id.clone(), msg.id.clone(), name));
                            }
                            close = true;
                        }
                    }

                    menu_separator(ui, palette);
                    if item(ui, Icon::Copy, "Copiar texto", false, false) {
                        ctx.copy_text(msg.content.clone());
                        close = true;
                    }
                    if item(ui, Icon::Bookmark, "Marcar mensaje", false, false) {
                        *event = Some(ChatEvent::Unavailable("Marcar mensaje"));
                        close = true;
                    }
                    if item(ui, Icon::Clock, "Crear recordatorio", true, false) {
                        *event = Some(ChatEvent::Unavailable("Crear recordatorio"));
                        close = true;
                    }
                    if item(ui, Icon::LayoutGrid, "Aplicaciones", true, false) {
                        *event = Some(ChatEvent::Unavailable("Aplicaciones"));
                        close = true;
                    }
                    if item(ui, Icon::MessageCircle, "Marcar no leídos", false, false) && !msg.id.is_empty() {
                        *event = Some(ChatEvent::MarkUnread { message_id: msg.id.clone() });
                        close = true;
                    }
                    if item(ui, Icon::Link, "Copiar enlace del mensaje", false, false) {
                        ctx.copy_text(format!(
                            "https://discord.com/channels/{}/{}/{}",
                            guild_id.unwrap_or("@me"),
                            msg.channel_id,
                            msg.id
                        ));
                        close = true;
                    }
                    if item(ui, Icon::Volume2, "Leer mensaje", false, false) {
                        *event = Some(ChatEvent::Unavailable("Leer mensaje"));
                        close = true;
                    }
                    let can_clear = has_reactions && can_manage;
                    let can_delete = !msg.id.is_empty() && (msg.is_own || can_manage);
                    let can_report = !msg.is_own && !msg.id.is_empty();
                    if can_clear || can_delete || can_report {
                        menu_separator(ui, palette);
                    }
                    if can_clear {
                        let row = menu_item(ui, palette, Icon::Smile, "Eliminar reacciones", true, true);
                        reactions_rect = Some(row.rect);
                        if item(ui, Icon::SmileMinus, "Eliminar todas las reacciones", false, true) {
                            *op = Some(MessageOp::ClearAll { message_id: msg.id.clone() });
                            close = true;
                        }
                    }
                    if can_delete && item(ui, Icon::Trash, "Eliminar mensaje", false, true) {
                        if ui.input(|i| i.modifiers.shift) {
                            *op = Some(MessageOp::Delete { message_id: msg.id.clone() });
                        } else {
                            set_delete_confirm(&ctx, Some(DeleteConfirm { message_id: msg.id.clone() }));
                        }
                        close = true;
                    }
                    if can_report && item(ui, Icon::Flag, "Denunciar mensaje", false, true) {
                        *event = Some(ChatEvent::Unavailable("Denunciar mensaje"));
                        close = true;
                    }
                    menu_separator(ui, palette);
                    if item(ui, Icon::Hash, "Copiar ID del mensaje", false, false) {
                        ctx.copy_text(msg.id.clone());
                        close = true;
                    }
                });
        });

    // Se cierra con Esc o con un click afuera (el botón "..." que lo abrió
    // se encarga solo de alternarlo).
    let menu_rect = shown.response.rect;

    // Submenú "Eliminar reacciones": una fila por emoji. Se abre con el mouse
    // sobre la fila y sigue abierto mientras el mouse esté en él.
    let sub_id = scoped_id("ecord_message_submenu");
    let hover = ctx.input(|i| i.pointer.hover_pos());
    let prev_sub: Option<egui::Rect> = ctx.memory(|m| m.data.get_temp(sub_id));
    let mut sub_rect: Option<egui::Rect> = None;
    if let Some(item_rect) = reactions_rect {
        let over_item = hover.is_some_and(|p| item_rect.contains(p));
        let over_sub = prev_sub.zip(hover).is_some_and(|(r, p)| r.expand(8.0).contains(p));
        if over_item || over_sub {
            let sub = Area::new(scoped_id("ecord_message_submenu_area"))
                .order(Order::Foreground)
                .fixed_pos(egui::pos2(menu_rect.right() + 4.0, item_rect.top() - 6.0))
                .show(&ctx, |ui| {
                    Frame::new()
                        .fill(palette.overlay)
                        .stroke(Stroke::new(1.0, palette.outline))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(6))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 6],
                            blur: 18,
                            spread: 0,
                            color: palette.shadow,
                        })
                        .show(ui, |ui| {
                            ui.set_width(200.0);
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            for reaction in &msg.reactions {
                                let (rect, response) =
                                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
                                if response.hovered() {
                                    ui.painter()
                                        .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
                                }
                                let icon_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.left() + 20.0, rect.center().y),
                                    Vec2::splat(18.0),
                                );
                                match &reaction.emoji {
                                    ReactionKind::Unicode(e) => {
                                        if twemoji::paint(ui, icon_rect, e) == twemoji::State::Failed {
                                            ui.painter().text(
                                                icon_rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                e,
                                                theme::regular(15.0),
                                                palette.text,
                                            );
                                        }
                                    }
                                    ReactionKind::Custom { name, .. } => {
                                        ui.painter().text(
                                            egui::pos2(rect.left() + 12.0, rect.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            format!(":{name}:"),
                                            theme::regular(13.0),
                                            palette.text,
                                        );
                                    }
                                }
                                ui.painter().text(
                                    egui::pos2(rect.right() - 12.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    reaction.count.to_string(),
                                    theme::regular(13.0),
                                    palette.dim,
                                );
                                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    *op = Some(MessageOp::ClearEmoji {
                                        message_id: msg.id.clone(),
                                        emoji: reaction.emoji.clone(),
                                    });
                                    close = true;
                                }
                            }
                        });
                });
            sub_rect = Some(sub.response.rect);
        }
    }
    ctx.memory_mut(|m| match sub_rect {
        Some(r) => {
            m.data.insert_temp(sub_id, r);
        }
        None => {
            m.data.remove::<egui::Rect>(sub_id);
        }
    });

    let toggle = menu.toggle;
    let outside_press = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|p| {
                !menu_rect.contains(p)
                    && !toggle.is_some_and(|t| t.contains(p))
                    && !sub_rect.is_some_and(|r| r.contains(p))
            })
    });
    if close || outside_press || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        set_open_message_menu(&ctx, None);
    }
}

/// Recorta el contenido de un mensaje para mostrarlo como vista previa en
/// el banner "Respondiendo a..." del composer y en la línea de "citado"
/// arriba de una respuesta ya enviada (ver `RepliedMessage` en
/// `lib::data`) — sin saltos de línea y con un tope de caracteres, como
/// en el cliente real. `pub(crate)` porque `ChatMessage::from_discord` la
/// usa para armar esa segunda vista previa.
pub(crate) fn preview_text(content: &str) -> String {
    const MAX_CHARS: usize = 60;
    let flat: String = content.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() > MAX_CHARS {
        let truncated: String = flat.chars().take(MAX_CHARS).collect();
        format!("{truncated}…")
    } else {
        flat
    }
}

/// Id del selector de emojis que se abre al reaccionar. Solo puede haber uno
/// abierto a la vez (ver `reaction_panel_memory_id`), así que un único id por
/// scope alcanza; no hace falta que sea por-índice de mensaje.
fn reaction_picker_id() -> egui::Id {
    scoped_id("ecord_reaction_picker")
}

/// Cierra el selector de reacciones y borra lo que recordaba (buscador,
/// pestaña, salto pendiente), así la próxima vez abre limpio.
fn clear_reaction_panel_state(ctx: &egui::Context) {
    crate::ui::compose_menus::reset_picker(ctx, reaction_picker_id());
}

/// Selector para reaccionar: el MISMO selector de emojis del compositor
/// (`compose_menus::show_emoji_picker`) — pestañas GIF/Stickers/Emojis,
/// buscador, barra lateral de servers, catálogo Unicode completo y vista
/// previa — pero en modo reacción: GIF y Stickers se ven apagadas porque no
/// se puede reaccionar con eso (tampoco en el cliente real). Se abre desde el
/// botón "+" de `hover_toolbar` y se cierra solo (elegir un emoji, Esc, o
/// clickear en cualquier otro lado).
fn reaction_panel(
    ui: &mut egui::Ui,
    palette: &Palette,
    index: usize,
    row_rect: egui::Rect,
    toggled_reaction: &mut Option<(usize, ReactionKind)>,
    // Se pone en `true` si se clickeó un emoji bloqueado (falta Nitro).
    nitro_required: &mut bool,
    custom_emojis: &[EmojiGroup],
) {
    use crate::ui::compose_menus as menus;

    let ctx = ui.ctx().clone();
    // Esquina superior derecha del selector, un poco abajo del borde de la
    // fila (egui lo corre solo si no entra en la pantalla).
    let anchor = egui::pos2(row_rect.right() - 12.0, row_rect.top() + 20.0);
    let close = |ctx: &egui::Context| {
        set_open_reaction_panel(ctx, None);
        clear_reaction_panel_state(ctx);
    };
    match menus::show_emoji_picker(
        &ctx,
        palette,
        reaction_picker_id(),
        anchor,
        custom_emojis,
        menus::PickerMode::React,
        None,
    ) {
        menus::PickerResult::Pending => {}
        menus::PickerResult::Closed => close(&ctx),
        menus::PickerResult::Unicode(emoji) => {
            *toggled_reaction = Some((index, ReactionKind::Unicode(emoji)));
            close(&ctx);
        }
        menus::PickerResult::Custom(emoji) => {
            *toggled_reaction = Some((
                index,
                ReactionKind::Custom {
                    id: emoji.id.clone(),
                    name: emoji.name.clone(),
                    animated: emoji.animated,
                },
            ));
            close(&ctx);
        }
        menus::PickerResult::Locked => {
            // Sin Nitro no se reacciona: se avisa con un popup (ver
            // `ChatEvent::NitroRequired`).
            *nitro_required = true;
            close(&ctx);
        }
        // En modo reacción las pestañas GIF y Stickers están apagadas: estos
        // resultados no pueden llegar.
        menus::PickerResult::Sticker(_) | menus::PickerResult::Gif(_) => {}
    }
}

/// Fila de reacciones ya puestas debajo del contenido: una "pill" por
/// cada reacción (emoji + contador, resaltada si la propia cuenta ya
/// reaccionó así). No se muestra nada si el mensaje todavía no tiene
/// ninguna — para agregar la primera está la barra flotante
/// (`hover_toolbar`), no un botón fijo acá.
fn reactions_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    msg: &ChatMessage,
    index: usize,
    toggled_reaction: &mut Option<(usize, ReactionKind)>,
) {
    if msg.reactions.is_empty() {
        return;
    }
    ui.add_space(3.0);
    ui.horizontal_wrapped(|ui| {
        // Un poco más de aire horizontal entre píldoras que antes (6 -> 7)
        // para que, sumado al borde de `paint_pill_background`, cada
        // reacción se lea claramente separada de sus vecinas incluso
        // cuando hay muchas seguidas con emojis angostos.
        ui.spacing_mut().item_spacing = Vec2::new(7.0, 6.0);
        for reaction in &msg.reactions {
            if reaction_pill(ui, palette, &reaction.emoji, reaction.count, reaction.reacted_by_me).clicked() {
                *toggled_reaction = Some((index, reaction.emoji.clone()));
            }
        }
    });
}

/// Una reacción individual ("👍 3" o el ícono real de un emoji
/// personalizado + contador), pintada como una píldora chica.
fn reaction_pill(ui: &mut egui::Ui, palette: &Palette, kind: &ReactionKind, count: u32, mine: bool) -> egui::Response {
    match kind {
        ReactionKind::Unicode(emoji) => reaction_pill_unicode(ui, palette, emoji, count, mine),
        ReactionKind::Custom { id, name, animated } => {
            reaction_pill_custom(ui, palette, id, name, *animated, count, mine)
        }
    }
}

fn pill_fill(palette: &Palette, mine: bool, hovered: bool) -> Color32 {
    if mine {
        extra::blend(palette.accent, palette.surface, if hovered { 0.32 } else { 0.22 })
    } else if hovered {
        palette.surface_hover
    } else {
        palette.surface
    }
}

fn paint_pill_background(ui: &egui::Ui, palette: &Palette, rect: egui::Rect, mine: bool, hovered: bool) {
    let fill = pill_fill(palette, mine, hovered);
    ui.painter().rect_filled(rect, theme::radius_small() as f32 + 2.0, fill);
    // Antes solo las reacciones "mías" tenían un borde — el resto se
    // apoyaba solo en el `item_spacing` de `reactions_row` para separarse
    // de sus vecinas, y con emojis angostos (p.ej. las banderitas-letra de
    // "regional indicator") ese espacio no alcanza para que se lean como
    // píldoras separadas. Un borde sutil en todas, no solo en las propias,
    // les da un borde definido pase lo que pase con el contenido.
    let stroke_color = if mine { palette.accent } else { palette.outline };
    ui.painter().rect_stroke(
        rect,
        theme::radius_small() as f32 + 2.0,
        Stroke::new(1.0, stroke_color),
        egui::StrokeKind::Inside,
    );
}

/// Reacción con un emoji Unicode, con la imagen a color (Twemoji) — igual que
/// `reaction_pill_custom` pero pidiéndola al set de Twemoji en vez de al CDN
/// de Discord. Si no se puede cargar, `reaction_pill_unicode` cae al glifo de
/// la fuente.
fn reaction_pill_twemoji(ui: &mut egui::Ui, palette: &Palette, emoji: &str, count: u32, mine: bool) -> egui::Response {
    let font = theme::regular(12.0);
    let text_color = if mine { palette.accent } else { palette.secondary };
    let icon_size = 16.0_f32;
    let gap = 5.0_f32;
    let padding = Vec2::new(8.0, 3.0);

    let count_galley = ui.painter().layout_no_wrap(count.to_string(), font, text_color);
    // Alto fijo (ver `REACTION_PILL_HEIGHT`) para que toda la fila quede pareja.
    let inner_size = Vec2::new(icon_size + gap + count_galley.size().x, REACTION_PILL_HEIGHT - padding.y * 2.0);
    let size = Vec2::new(inner_size.x + padding.x * 2.0, REACTION_PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "reacción")
    });
    if ui.is_rect_visible(rect) {
        paint_pill_background(ui, palette, rect, mine, response.hovered());
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + padding.x, rect.center().y - icon_size / 2.0),
            Vec2::splat(icon_size),
        );
        // `paint` usa `Image::paint_at` (no reserva espacio en el `ui` padre),
        // mismo motivo que en `reaction_pill_custom`.
        twemoji::paint(ui, icon_rect, emoji);
        let text_pos = egui::pos2(icon_rect.right() + gap, rect.center().y - count_galley.size().y / 2.0);
        ui.painter().galley(text_pos, count_galley, text_color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn reaction_pill_unicode(ui: &mut egui::Ui, palette: &Palette, emoji: &str, count: u32, mine: bool) -> egui::Response {
    if twemoji::probe(ui.ctx(), emoji, 16.0) != twemoji::State::Failed {
        return reaction_pill_twemoji(ui, palette, emoji, count, mine);
    }
    let font = theme::regular(12.0);
    let text_color = if mine { palette.accent } else { palette.secondary };
    // El emoji y el contador se miden y pintan por separado (con un gap
    // fijo entre los dos) en vez de un único string "emoji count": metidos
    // en el mismo string, el ancho del espacio depende de la fuente, y con
    // emojis angostos (como las letras-bandera "regional indicator") el
    // contador terminaba pegado al emoji — a tamaño chico eso se llega a
    // leer como si fuera parte del glifo en vez de un número aparte.
    let emoji_galley = ui.painter().layout_no_wrap(emoji.to_string(), font.clone(), text_color);
    let count_galley = ui.painter().layout_no_wrap(count.to_string(), font, text_color);
    let gap = 5.0_f32;
    let padding = Vec2::new(8.0, 3.0);
    // El alto de la píldora es SIEMPRE `REACTION_PILL_HEIGHT`, nunca el
    // alto medido de los galleys — así todas las píldoras de una fila
    // quedan a la misma altura sin importar qué tan alto mida la fuente
    // el emoji de turno (ver el comentario de la constante).
    let inner_size = Vec2::new(
        emoji_galley.size().x + gap + count_galley.size().x,
        REACTION_PILL_HEIGHT - padding.y * 2.0,
    );
    let size = Vec2::new(inner_size.x + padding.x * 2.0, REACTION_PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "reacción")
    });
    if ui.is_rect_visible(rect) {
        paint_pill_background(ui, palette, rect, mine, response.hovered());
        let emoji_pos = egui::pos2(rect.left() + padding.x, rect.center().y - emoji_galley.size().y / 2.0);
        let count_pos = egui::pos2(emoji_pos.x + emoji_galley.size().x + gap, rect.center().y - count_galley.size().y / 2.0);
        ui.painter().galley(emoji_pos, emoji_galley, text_color);
        ui.painter().galley(count_pos, count_galley, text_color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Reacción con un emoji personalizado del server: en vez de caer a
/// texto ":nombre:" (como hacía antes `Emoji::display`), pide el ícono
/// real al CDN de Discord — mismo patrón que ya usa `markdown::custom_emoji`
/// para los emojis personalizados que aparecen en el propio texto de un
/// mensaje. Mientras la imagen todavía no cargó (o si falló), se pinta un
/// cuadradito de relleno en su lugar en vez de dejar un hueco.
fn reaction_pill_custom(
    ui: &mut egui::Ui,
    palette: &Palette,
    id: &str,
    name: &str,
    animated: bool,
    count: u32,
    mine: bool,
) -> egui::Response {
    let font = theme::regular(12.0);
    let text_color = if mine { palette.accent } else { palette.secondary };
    let icon_size = 16.0_f32;
    let gap = 4.0_f32;
    let padding = Vec2::new(8.0, 3.0);

    let count_galley = ui.painter().layout_no_wrap(count.to_string(), font, text_color);
    // Mismo criterio que en `reaction_pill_unicode`: alto fijo, no el
    // alto medido, para que una reacción con emoji personalizado no
    // desnivele la fila contra las que sí son unicode.
    let inner_size = Vec2::new(icon_size + gap + count_galley.size().x, REACTION_PILL_HEIGHT - padding.y * 2.0);
    let size = Vec2::new(inner_size.x + padding.x * 2.0, REACTION_PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "reacción")
    });

    if ui.is_rect_visible(rect) {
        paint_pill_background(ui, palette, rect, mine, response.hovered());

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + padding.x, rect.center().y - icon_size / 2.0),
            Vec2::splat(icon_size),
        );
        // `.gif` para las reacciones con emoji animado, `.png` para el
        // resto — mismo criterio que `CustomEmoji::url()` (picker) y
        // `markdown::custom_emoji` (emoji dentro del texto), con el mismo
        // empujoncito `gif_safe_url` para que el loader animado lo detecte
        // a pesar del `?size=48` después de la extensión.
        let ext = if animated { "gif" } else { "png" };
        let url = format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48");
        let url = if animated { crate::ui::media::gif_safe_url(&url) } else { url };
        let image = egui::Image::new(crate::ui::anim::source(ui.ctx(), &url))
            .fit_to_exact_size(Vec2::splat(icon_size))
            .show_loading_spinner(false);
        match image.load_for_size(ui.ctx(), Vec2::splat(icon_size)) {
            Ok(egui::load::TexturePoll::Ready { .. }) => {
                // Antes: `ui.put(icon_rect, image)`. `Ui::put` no es un
                // simple "pintar en este rect": arma una sub-`Ui` y al
                // final llama `allocate_rect` sobre el `ui` PADRE (el
                // mismo que está corriendo el `horizontal_wrapped` de
                // `reactions_row`), lo que mete una asignación de espacio
                // extra — "fantasma", no pensada como una píldora más —
                // en el cursor del wrap. Esa asignación fantasma solo
                // pasaba en la rama de emoji custom (la unicode nunca usó
                // `put`), y por eso el corrimiento diagonal ("escalera")
                // solo se veía con reacciones custom: cada una de esas
                // sumaba un desplazamiento extra al cursor que se iba
                // acumulando con la siguiente. `Image::paint_at` pinta
                // directo con el painter, sin asignarle nada al padre.
                image.paint_at(ui, icon_rect);
            }
            _ => {
                ui.painter().rect_filled(icon_rect, 3.0, palette.surface_hover);
            }
        }

        let text_pos = egui::pos2(icon_rect.right() + gap, rect.center().y - count_galley.size().y / 2.0);
        ui.painter().galley(text_pos, count_galley, text_color);
    }

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(":{name}:"))
}

#[allow(clippy::too_many_arguments)]
fn composer(
    ui: &mut egui::Ui,
    palette: &Palette,
    messages: &mut Vec<ChatMessage>,
    compose_text: &mut String,
    placeholder: &str,
    own_author: &str,
    own_color: Color32,
    send_target: Option<(String, String, Option<String>)>,
    reply_target: &mut Option<ReplyTarget>,
    custom_emojis: &[EmojiGroup],
) -> bool {
    use crate::ui::compose_menus as menus;
    use crate::ui::slash as slash_ui;

    // Se pone en `true` si se clickeó en el selector un emoji bloqueado (falta
    // Nitro): quien llama (`show`) lo convierte en `ChatEvent::NitroRequired`.
    let mut nitro_required = false;
    let ctx = ui.ctx().clone();

    // Ids de memoria (por scope: el chat principal y el panel de un hilo
    // pueden estar a la vez en pantalla, cada uno con su propio compositor).
    let text_id = scoped_id("ecord_composer_text");
    let menu_state_id = scoped_id("ecord_composer_mention_state");
    let mentions_id = scoped_id("ecord_composer_mentions");
    let customs_id = scoped_id("ecord_composer_customs");
    let picker_open_id = scoped_id("ecord_composer_emoji_open");
    let picker_id = scoped_id("ecord_composer_emoji_picker");
    let mention_menu_id = scoped_id("ecord_composer_mention_menu");
    let slash_active_id = scoped_id("ecord_composer_slash_active");
    let slash_menu_id = scoped_id("ecord_composer_slash_menu");
    let slash_state_id = scoped_id("ecord_composer_slash_state");
    let text_extra_id = scoped_id("ecord_composer_text_extra");

    // Permisos y modo lento del canal (ver `ChatLimits`).
    let limits = current_limits(&ctx);
    let channel_key: String = send_target.as_ref().map(|(_, c, _)| c.clone()).unwrap_or_default();

    // Sin "Enviar mensajes" no hay caja de texto: una barra lo explica.
    if !limits.can_send {
        ctx.memory_mut(|m| m.data.insert_temp(text_extra_id, 0.0_f32));
        match active_notice(&ctx) {
            Some(text) => restriction_bar(ui, palette, &text, palette.danger),
            None => restriction_bar(
                ui,
                palette,
                "No tienes permiso para enviar mensajes en este canal.",
                palette.dim,
            ),
        }
        return nitro_required;
    }
    // Modo lento: mientras corre la cuenta regresiva no se puede mandar nada.
    let slow_blocked = limits.slowmode_secs > 0 && slowmode_wait(&ctx, &channel_key) > 0.0;

    // ---- Slash command elegido: su formulario de opciones reemplaza al
    // compositor hasta que se envía (Enter) o se cancela (Esc).
    if let Some(mut active) = slash_ui::load_active(&ctx, slash_active_id) {
        // El formulario reemplaza a la caja de texto: no suma líneas de más.
        ctx.memory_mut(|m| m.data.insert_temp(text_extra_id, 0.0_f32));
        match slash_ui::show_command_form(ui, palette, &mut active, slash_active_id) {
            slash_ui::FormOutcome::None => slash_ui::store_active(&ctx, slash_active_id, Some(active)),
            slash_ui::FormOutcome::Cancel => {
                // Esc: se descarta el comando (y lo escrito a su lado).
                slash_ui::store_active(&ctx, slash_active_id, None);
                ctx.memory_mut(|m| m.request_focus(text_id));
            }
            slash_ui::FormOutcome::ToText => {
                // Retroceso sobre el comando: pasa de interacción a texto
                // (`/comando`) y reaparecen las sugerencias. Lo escrito fuera
                // de los campos desaparece.
                let text = active.command_text();
                let len = text.chars().count();
                *compose_text = text;
                slash_ui::store_active(&ctx, slash_active_id, None);
                ctx.memory_mut(|m| m.request_focus(text_id));
                menus::set_caret(&ctx, text_id, len);
            }
            slash_ui::FormOutcome::Submit(options) => {
                if let Some((_, channel_id, guild_id)) = send_target.clone() {
                    slash_ui::request_run(
                        &ctx,
                        slash_ui::SlashInvocation {
                            channel_id,
                            guild_id,
                            command: active.entry.command.clone(),
                            path: active.entry.path.clone(),
                            options,
                        },
                    );
                }
                slash_ui::store_active(&ctx, slash_active_id, None);
                ctx.memory_mut(|m| m.request_focus(text_id));
            }
        }
        return nitro_required;
    }

    // ---- Adjuntos: archivos pendientes de este canal, subidas en curso y lo
    // que llega del selector, de arrastrar y soltar o del portapapeles.
    use crate::ui::attachments as files_ui;
    let files_id = scoped_id("ecord_composer_files").with(&channel_key);
    let jobs_id = scoped_id("ecord_composer_jobs");
    let file_picker_id = scoped_id("ecord_composer_filepicker");
    let mut pending = files_ui::load(&ctx, files_id);
    for msg in files_ui::poll_jobs(&ctx, jobs_id) {
        set_notice(&ctx, &msg);
    }
    if send_target.is_some() {
        let mut incoming = files_ui::take_picked(&ctx, file_picker_id);
        let mut rejected_for_permission = false;
        if is_input_target(&ctx, text_id) {
            if files_ui::hovering_files(&ctx) {
                files_ui::drop_overlay(&ctx, palette, limits.can_attach);
            }
            let dropped = files_ui::dropped_paths(&ctx);
            if !dropped.is_empty() {
                if limits.can_attach {
                    incoming.extend(dropped);
                } else {
                    rejected_for_permission = true;
                }
            }
            // Ctrl+V con una imagen en el portapapeles (y sin texto): captura
            // de pantalla, "copiar imagen" desde el navegador...
            let paste_image_wanted = ctx.memory(|m| m.has_focus(text_id))
                && ctx.input(|i| {
                    i.modifiers.command
                        && i.key_pressed(egui::Key::V)
                        && !i.events.iter().any(|e| matches!(e, egui::Event::Paste(t) if !t.is_empty()))
                });
            if paste_image_wanted {
                if !limits.can_attach {
                    // Sin texto que pegar y sin permiso: se avisa solo si de
                    // verdad había una imagen (si no, es un Ctrl+V vacío).
                    if arboard::Clipboard::new().is_ok_and(|mut c| c.get_image().is_ok()) {
                        rejected_for_permission = true;
                    }
                } else {
                    match files_ui::paste_image(&mut pending, limits.max_upload_bytes) {
                        Ok(_) => {}
                        Err(msg) => set_notice(&ctx, &msg),
                    }
                }
            }
        }
        if rejected_for_permission {
            set_notice(&ctx, "No tienes permiso para adjuntar archivos en este canal.");
        }
        if !incoming.is_empty() {
            for problem in files_ui::add_paths(&mut pending, incoming, limits.max_upload_bytes) {
                set_notice(&ctx, &problem);
            }
        }
    }

    // Si el selector de emojis estaba abierto, leído UNA sola vez al principio
    // del frame: el click del botón que lo abre/cierra queda guardado para el
    // próximo frame (mismo motivo que `open_panel_index` en `show`: si no, el
    // propio click de apertura contaría como "click afuera" y lo cerraría).
    let picker_open: bool = ctx.memory(|m| m.data.get_temp(picker_open_id).unwrap_or(false));

    if let Some(reply) = reply_target.clone() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius::same(theme::radius()))
                .inner_margin(Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    // `available_width` ya descuenta el margen interno de ambos lados
                    // del Frame: restar 16 deja 16px a la derecha (igual que a la
                    // izquierda); con 32 quedaban 32px y el campo se veía corrido.
                    ui.set_width(ui.available_width() - 16.0);
                    ui.horizontal(|ui| {
                        theme::text(ui, format!("Respondiendo a {}", reply.author), theme::regular(11.5), palette.dim);
                        theme::text(ui, &reply.preview, theme::regular(11.5), palette.secondary);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, Icon::X, 10.0, palette.dim, palette.text, "Cancelar respuesta").clicked() {
                                *reply_target = None;
                            }
                        });
                    });
                });
        });
    }

    // ---- Escribir sin foco: si nada usa el teclado y se tipea (o se pega)
    // algo, va directo al compositor, sin tener que clickearlo antes. Se pide
    // el foco ANTES de dibujar el `TextEdit` para que ese mismo frame se lleve
    // las teclas y no se pierda la primera letra.
    if send_target.is_some() && !picker_open {
        let now = ctx.input(|i| i.time);
        let alive_key = egui::Id::new(("ecord_composer_alive", text_id));
        ctx.data_mut(|d| d.insert_temp(alive_key, now));
        let last: Option<egui::Id> = ctx.data(|d| d.get_temp(last_composer_key()));
        // Con dos compositores a la vez (chat y panel de hilo) manda el último
        // que tuvo el foco, si todavía se está dibujando.
        let other_is_last = last.is_some_and(|l| {
            l != text_id
                && ctx
                    .data(|d| d.get_temp::<f64>(egui::Id::new(("ecord_composer_alive", l))))
                    .is_some_and(|seen| now - seen < 0.5)
        });
        if ctx.memory(|m| m.has_focus(text_id)) {
            ctx.data_mut(|d| d.insert_temp(last_composer_key(), text_id));
        } else if !other_is_last && ctx.data(|d| d.get_temp(type_to_focus_key())).unwrap_or(false) {
            // Un campo de texto con el foco (buscador, ajustes...) se queda con
            // el teclado. Un botón con el foco (tras un click) no lo usa.
            let busy = ctx
                .memory(|m| m.focused())
                .is_some_and(|f| egui::text_edit::TextEditState::load(&ctx, f).is_some());
            let typed = !busy
                && ctx.input(|i| {
                    i.events.iter().any(|e| match e {
                        // Un espacio suelto no arranca un mensaje (es el atajo
                        // de pausa de los videos).
                        egui::Event::Text(t) => !t.is_empty() && !(compose_text.is_empty() && t.trim().is_empty()),
                        egui::Event::Paste(t) => !t.is_empty(),
                        _ => false,
                    })
                });
            if typed {
                ctx.memory_mut(|m| m.request_focus(text_id));
                ctx.data_mut(|d| d.insert_temp(last_composer_key(), text_id));
            }
        }
    }

    // ---- Menú de menciones, parte 1: las teclas.
    // Se procesan ANTES de dibujar el `TextEdit`, que si no se quedaría con
    // las flechas (mover el cursor) y con Tab. Usa el texto y el cursor del
    // frame anterior, que son lo que el usuario estaba viendo.
    let mut menu = menus::MentionMenuState::load(&ctx, menu_state_id);
    let focused_before = ctx.memory(|m| m.has_focus(text_id));
    let mention_before = if focused_before {
        menus::caret_index(&ctx, text_id).and_then(|caret| menus::active_mention(compose_text, caret))
    } else {
        None
    };
    let mut key_accept: Option<usize> = None;
    if let Some(m) = mention_before.as_ref() {
        if menu.query != m.query {
            menu.query = m.query.clone();
            menu.selected = 0;
        }
        if menu.dismissed_start != Some(m.start) {
            if let Some(results) = menus::results_for(&ctx, &m.query) {
                let n = results.items.len();
                if n > 0 {
                    ctx.input_mut(|i| {
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            menu.selected = (menu.selected + 1) % n;
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            menu.selected = (menu.selected + n - 1) % n;
                        }
                        // Enter y Tab eligen la fila resaltada (en vez de mandar el
                        // mensaje / saltar de foco).
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                            || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        {
                            key_accept = Some(menu.selected.min(n - 1));
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                            menu.dismissed_start = Some(m.start);
                        }
                    });
                }
            }
        }
    }

    // ---- Slash commands, parte 1: las teclas. Igual que las del menú de
    // menciones, se procesan ANTES del `TextEdit` (si no, flechas, Tab y Enter
    // se las lleva el campo) y con el texto del frame anterior.
    let slash_catalog = slash_ui::catalog(&ctx);
    slash_ui::set_wanted(&ctx, send_target.is_some() && slash_ui::browse_query(compose_text).is_some());
    let mut slash_state = slash_ui::MenuState::load(&ctx, slash_state_id);
    let mut slash_key_entry: Option<std::sync::Arc<crate::discord::slash::SlashEntry>> = None;
    if focused_before && send_target.is_some() {
        if let (Some(query), Some(catalog)) = (slash_ui::browse_query(compose_text), slash_catalog.as_ref()) {
            slash_state.refresh(catalog, &query);
            if !slash_state.dismissed && !slash_state.matches.is_empty() {
                let n = slash_state.matches.len();
                let mut accept: Option<usize> = None;
                ctx.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                        slash_state.selected = (slash_state.selected + 1) % n;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                        slash_state.selected = (slash_state.selected + n - 1) % n;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    {
                        accept = Some(slash_state.selected.min(n - 1));
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                        slash_state.dismissed = true;
                    }
                });
                slash_key_entry = accept
                    .and_then(|row| slash_state.matches.get(row))
                    .and_then(|index| catalog.entries.get(*index))
                    .cloned();
            }
        }
    }

    ui.add_space(6.0);
    // Rect del campo (marco incluido), para anclar los menús flotantes.
    let mut composer_rect = egui::Rect::NOTHING;
    for (text, color) in composer_notices(&ctx, palette, limits, &channel_key) {
        ui.horizontal(|ui| {
            ui.set_min_height(NOTICE_H);
            ui.add_space(22.0);
            theme::text(ui, text, theme::medium(11.5), color);
        });
    }
    // Archivos pendientes de enviar (su alto se suma al que se le reserva al
    // compositor, ver `text_extra` más abajo).
    let chips_h = files_ui::show_chips(ui, palette, &mut pending);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let framed = Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(theme::radius() + 6))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                // `available_width` ya descuenta el margen interno de ambos lados
                // del Frame: restar 16 deja 16px a la derecha (igual que a la
                // izquierda); con 32 quedaban 32px y el campo se veía corrido.
                ui.set_width(ui.available_width() - 16.0);
                ui.horizontal(|ui| {
                    let plus_tone = if limits.can_attach { palette.dim } else { palette.danger };
                    let plus_tip = if limits.can_attach {
                        "Adjuntar archivos"
                    } else {
                        "No tienes permiso para adjuntar archivos en este canal."
                    };
                    if theme::icon_button(ui, Icon::CirclePlus, 16.0, plus_tone, palette.text, plus_tip).clicked() {
                        if limits.can_attach {
                            files_ui::open_picker(&ctx, file_picker_id);
                        } else {
                            set_notice(&ctx, "No tienes permiso para adjuntar archivos en este canal.");
                        }
                    }

                    let text_width = ui.available_width() - 44.0;
                    // `return_key`: Enter sin Shift no inserta un salto de
                    // línea (por defecto en un `multiline` SÍ lo haría) —
                    // se detecta más abajo como "hay que mandar" y se
                    // consume antes de llegar al widget. Shift+Enter queda
                    // libre para agregar una línea nueva de verdad, como
                    // en el cliente real: sin esto la barra era
                    // `singleline` y no había forma de escribir un bloque
                    // de código o una lista de varias líneas.
                    //
                    // El `id` explícito hace falta para poder leer/mover el
                    // cursor (menciones y emojis se insertan donde está).
                    // Con mucho texto la caja deja de crecer y pasa a tener su
                    // propio scroll (el cursor se mantiene a la vista).
                    // El scroll necesita un alto disponible de verdad (dentro de
                    // un `horizontal` solo hay una fila y se inflaba hasta su
                    // alto mínimo), pero SIN reservarlo entero: `scope_builder`
                    // solo ocupa lo que el contenido usó (`allocate_ui` reservaba
                    // los 150px aunque la caja estuviera vacía).
                    let scroll_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        Vec2::new(text_width, COMPOSER_TEXT_MAX_HEIGHT),
                    );
                    let scroll_out = ui
                        .scope_builder(
                            egui::UiBuilder::new()
                                .max_rect(scroll_rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt(text_id.with("scroll"))
                                    .max_height(COMPOSER_TEXT_MAX_HEIGHT)
                                    .min_scrolled_height(24.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(compose_text)
                                                .id(text_id)
                                                .hint_text(placeholder)
                                                .frame(egui::Frame::NONE)
                                                .desired_width(text_width)
                                                .desired_rows(1)
                                                .return_key(Some(egui::KeyboardShortcut::new(
                                                    egui::Modifiers::SHIFT,
                                                    egui::Key::Enter,
                                                ))),
                                        )
                                    })
                            },
                        )
                        .inner;
                    // Cuánto más alto que una fila quedó la caja (para que la
                    // lista de mensajes ceda ese espacio). Se mide el CONTENIDO (acotado):
                    // el rect del scroll puede ser más grande que lo que se ve.
                    let shown_height = scroll_out.content_size.y.min(COMPOSER_TEXT_MAX_HEIGHT);
                    let text_extra = (shown_height - ui.spacing().interact_size.y).max(0.0);
                    ctx.memory_mut(|m| m.data.insert_temp(text_extra_id, text_extra + chips_h));
                    let response = scroll_out.inner;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Botón de emojis: abre/cierra el selector (se dibuja
                        // más abajo, anclado a este campo).
                        let tint = if picker_open { palette.text } else { palette.dim };
                        if theme::icon_button(ui, Icon::Smile, 17.0, tint, palette.text, "Emojis").clicked() {
                            ctx.memory_mut(|m| m.data.insert_temp(picker_open_id, !picker_open));
                            if picker_open {
                                menus::reset_picker(&ctx, picker_id);
                            }
                        }
                    });

                    // Como el `return_key` de arriba es Shift+Enter, un
                    // Enter suelto ya NO se consume como salto de línea
                    // adentro del `TextEdit` — sigue disponible acá para
                    // detectarlo nosotros y mandar el mensaje, sin que el
                    // widget haya perdido el foco. (Si el menú de menciones
                    // estaba abierto, ya se llevó ese Enter más arriba.)
                    let enter_pressed = response.has_focus()
                        && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    let has_files = !pending.files.is_empty();
                    if enter_pressed && (!compose_text.trim().is_empty() || has_files) && !slow_blocked {
                        // `:nombre:` -> `<:nombre:id>` para los emojis personalizados.
                        let picked = menus::take_customs(&ctx, customs_id);
                        let content = menus::expand_shortcodes(compose_text.trim(), &picked, custom_emojis);
                        if has_files {
                            // Mensaje con archivos: sin eco local (el mensaje
                            // real llega por el Gateway cuando termina la subida).
                            if !limits.can_attach {
                                set_notice(&ctx, "No tienes permiso para adjuntar archivos en este canal.");
                            } else if let Some(big) =
                                pending.files.iter().find(|f| f.size > limits.max_upload_bytes)
                            {
                                set_notice(&ctx, &format!("«{}» supera el tamaño máximo permitido.", big.filename));
                            } else if let Some((token, channel_id, guild_id)) = send_target.clone() {
                                let reply_to = reply_target
                                    .take()
                                    .map(|r| r.message_id)
                                    .filter(|id| !id.is_empty());
                                let job = crate::discord::uploads::UploadJob::new(pending.files.len());
                                files_ui::push_job(&ctx, jobs_id, job.clone());
                                crate::discord::uploads::spawn_send_with_files(
                                    ctx.clone(),
                                    token,
                                    channel_id,
                                    guild_id,
                                    content,
                                    reply_to,
                                    files_ui::to_upload_files(&pending),
                                    job,
                                );
                                pending.files.clear();
                                compose_text.clear();
                                if limits.slowmode_secs > 0 {
                                    start_slowmode(&ctx, &channel_key, limits.slowmode_secs);
                                }
                            }
                            response.request_focus();
                        } else {
                        let mut echo = ChatMessage::own(own_author, "ahora", &content, own_color);
                        // Para que el eco local muestre `@Nombre` y no `<@id>`.
                        echo.mentions = menus::take_remembered(&ctx, mentions_id, &content);
                        // Si hay una respuesta en curso, el eco local ya
                        // muestra el banner citado de una — no hace falta
                        // esperar a que el Gateway devuelva el
                        // `MESSAGE_CREATE` real para que aparezca. Usa
                        // directamente el autor/preview que ya se armaron al
                        // apretar "Responder" (ver `hover_toolbar`), sin
                        // depender de tener un `message_id` real.
                        if let Some(reply) = reply_target.as_ref() {
                            echo.replied_to = Some(RepliedMessage {
                                author: reply.author.clone(),
                                preview: reply.preview.clone(),
                                deleted: false,
                                ..Default::default()
                            });
                        }
                        messages.push(echo);
                        // Vacío para mensajes de demo/eco local: no hay
                        // nada real a qué referenciar, así que no se manda
                        // `message_reference` aunque hubiera un banner de
                        // respuesta puesto.
                        let reply_to = reply_target
                            .take()
                            .map(|r| r.message_id)
                            .filter(|id| !id.is_empty());
                        if let Some((token, channel_id, guild_id)) = send_target.clone() {
                            crate::discord::spawn_send_message(token, channel_id, guild_id, content, reply_to);
                        }
                        compose_text.clear();
                        if limits.slowmode_secs > 0 {
                            start_slowmode(&ctx, &channel_key, limits.slowmode_secs);
                        }
                        // `multiline` no suelta el foco solo, pero tampoco
                        // lo retiene mágicamente frame a frame después de
                        // vaciar el texto por fuera del widget — lo
                        // reafirmamos para que se pueda seguir tipeando
                        // sin tener que volver a clickear la barra.
                        response.request_focus();
                        }
                    }
                });
            });
        composer_rect = framed.response.rect;
    });
    files_ui::store(&ctx, files_id, pending);

    // ---- Slash commands, parte 2: dibujar el menú y activar el comando
    // elegido (con el teclado, arriba, o con el mouse, acá).
    let slash_now = if send_target.is_some() && ctx.memory(|m| m.has_focus(text_id)) {
        slash_ui::browse_query(compose_text)
    } else {
        None
    };
    let mut slash_pointer_entry: Option<std::sync::Arc<crate::discord::slash::SlashEntry>> = None;
    if let Some(query) = slash_now.as_ref() {
        let anchor = egui::pos2(composer_rect.left(), composer_rect.top() - 6.0);
        let width = composer_rect.width().clamp(380.0, 640.0);
        match slash_catalog.as_ref() {
            Some(catalog) => {
                slash_state.refresh(catalog, query);
                if !slash_state.dismissed && !slash_state.matches.is_empty() {
                    slash_state.selected = slash_state.selected.min(slash_state.matches.len() - 1);
                    let outcome = slash_ui::show_slash_menu(
                        &ctx,
                        palette,
                        slash_menu_id,
                        anchor,
                        width,
                        catalog,
                        &slash_state,
                    );
                    if let Some(hovered) = outcome.hovered {
                        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            slash_state.selected = hovered;
                        }
                    }
                    slash_pointer_entry = outcome
                        .pressed
                        .and_then(|row| slash_state.matches.get(row))
                        .and_then(|index| catalog.entries.get(*index))
                        .cloned();
                }
            }
            // Todavía no llegó el catálogo de este server (se pide en `App::ui`).
            None => {
                slash_ui::show_slash_loading(&ctx, palette, slash_menu_id, anchor, width);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    } else {
        slash_state.dismissed = false;
    }
    if let Some(entry) = slash_key_entry.or(slash_pointer_entry) {
        slash_ui::activate(&ctx, slash_active_id, entry);
        compose_text.clear();
        slash_state = slash_ui::MenuState::default();
    }
    slash_state.store(&ctx, slash_state_id);

    // ---- Menú de menciones, parte 2: dibujarlo y aplicar la elección.
    // Con el texto y el cursor YA actualizados por el `TextEdit` de este frame.
    let focused = ctx.memory(|m| m.has_focus(text_id));
    let mention = if focused {
        menus::caret_index(&ctx, text_id).and_then(|caret| menus::active_mention(compose_text, caret))
    } else {
        None
    };
    // `App::ui` lee esto en el próximo frame para armar los candidatos.
    menus::set_active_query(&ctx, mention.as_ref().map(|m| m.query.as_str()));
    if mention.is_none() {
        menu.dismissed_start = None;
    }
    let mut pointer_accept: Option<usize> = None;
    if let Some(m) = mention.as_ref() {
        if menu.query != m.query {
            menu.query = m.query.clone();
            menu.selected = 0;
        }
        if menu.dismissed_start != Some(m.start) {
            match menus::results_for(&ctx, &m.query) {
                Some(results) => {
                    let local_users = results.items.iter().filter(|c| c.kind == menus::MentionKind::User).count();
                    menus::drive_member_search(&ctx, &m.query, local_users);
                    if !results.items.is_empty() {
                        menu.selected = menu.selected.min(results.items.len() - 1);
                        let outcome = menus::show_mention_menu(
                            &ctx,
                            palette,
                            mention_menu_id,
                            egui::pos2(composer_rect.left(), composer_rect.top() - 6.0),
                            composer_rect.width().clamp(260.0, 480.0),
                            &m.query,
                            &results.items,
                            menu.selected,
                        );
                        // El resaltado sigue al mouse solo cuando se mueve, para
                        // no pelearse con las flechas.
                        if let Some(hovered) = outcome.hovered {
                            if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                menu.selected = hovered;
                            }
                        }
                        pointer_accept = outcome.pressed;
                    }
                }
                // Todavía no llegaron los candidatos de esta consulta (se arman
                // en `App::ui`, un frame después).
                None => ctx.request_repaint_after(Duration::from_millis(50)),
            }
        }
    }

    let chosen = match (key_accept, pointer_accept) {
        (Some(index), _) => mention_before.as_ref().map(|m| (m, index)),
        (None, Some(index)) => mention.as_ref().map(|m| (m, index)),
        (None, None) => None,
    };
    if let Some((m, index)) = chosen {
        if let Some(results) = menus::results_for(&ctx, &m.query) {
            if let Some(cand) = results.items.get(index) {
                let insertion = format!("{} ", cand.token());
                menus::replace_chars(compose_text, m.start, m.end, &insertion);
                menus::set_caret(&ctx, text_id, m.start + insertion.chars().count());
                if cand.kind == menus::MentionKind::User {
                    menus::remember_user(&ctx, mentions_id, &cand.id, &cand.name);
                }
                // Apretar en el menú le quitó el foco al campo: se lo devolvemos.
                ctx.memory_mut(|mem| mem.request_focus(text_id));
                menus::set_active_query(&ctx, None);
                menu = menus::MentionMenuState::default();
            }
        }
    }
    menu.store(&ctx, menu_state_id);

    // ---- Selector de emojis.
    if picker_open {
        let anchor = egui::pos2(composer_rect.right(), composer_rect.top() - 8.0);
        let close = |ctx: &egui::Context| {
            ctx.memory_mut(|m| m.data.insert_temp(picker_open_id, false));
            menus::reset_picker(ctx, picker_id);
        };
        // Inserta `insertion` donde está el cursor (o al final si nunca se
        // enfocó el campo) y le devuelve el foco.
        let insert = |text: &mut String, insertion: &str| {
            let len = text.chars().count();
            let caret = menus::caret_index(&ctx, text_id).unwrap_or(len).min(len);
            menus::replace_chars(text, caret, caret, insertion);
            menus::set_caret(&ctx, text_id, caret + insertion.chars().count());
            ctx.memory_mut(|mem| mem.request_focus(text_id));
        };
        // El token solo hace falta para pedir GIFs (pestaña "GIF"): en el chat
        // de demo (`send_target` vacío) esa pestaña no tiene qué mostrar.
        let token = send_target.as_ref().map(|(token, _, _)| token.as_str());
        match menus::show_emoji_picker(
            &ctx,
            palette,
            picker_id,
            anchor,
            custom_emojis,
            menus::PickerMode::Compose,
            token,
        ) {
            menus::PickerResult::Pending => {}
            menus::PickerResult::Closed => close(&ctx),
            menus::PickerResult::Unicode(emoji) => {
                insert(compose_text, &emoji);
                close(&ctx);
            }
            menus::PickerResult::Custom(emoji) => {
                // Se escribe `:nombre:` (legible en el campo) y recién al mandar
                // se convierte en `<:nombre:id>` (ver `expand_shortcodes`).
                menus::remember_custom(&ctx, customs_id, &emoji);
                insert(compose_text, &format!(":{}: ", emoji.name));
                close(&ctx);
            }
            menus::PickerResult::Locked => {
                // Sin Nitro no se puede usar: se avisa con el popup de siempre.
                nitro_required = true;
                close(&ctx);
            }
            menus::PickerResult::Sticker(_) if slow_blocked => {
                set_notice(&ctx, "Modo lento: espera a que termine la cuenta para enviar el sticker.");
                close(&ctx);
            }
            menus::PickerResult::Sticker(sticker) => {
                // Un sticker se manda solo, sin texto. Si había una respuesta
                // en curso, el sticker la contesta.
                if let Some((token, channel_id, guild_id)) = send_target.clone() {
                    let reply_to = reply_target
                        .take()
                        .map(|r| r.message_id)
                        .filter(|id| !id.is_empty());
                    crate::discord::spawn_send_sticker(token, channel_id, guild_id, sticker.id.clone(), reply_to);
                    if limits.slowmode_secs > 0 {
                        start_slowmode(&ctx, &channel_key, limits.slowmode_secs);
                    }
                }
                close(&ctx);
            }
            menus::PickerResult::Gif(_) if !limits.can_embed => {
                set_notice(&ctx, "No tienes permiso para insertar enlaces: no se puede enviar el GIF.");
                close(&ctx);
            }
            menus::PickerResult::Gif(_) if slow_blocked => {
                set_notice(&ctx, "Modo lento: espera a que termine la cuenta para enviar el GIF.");
                close(&ctx);
            }
            menus::PickerResult::Gif(url) => {
                // Un GIF se manda como mensaje con su link (Discord lo
                // convierte en el GIF), igual que mandar un texto.
                let mut echo = ChatMessage::own(own_author, "ahora", &url, own_color);
                if let Some(reply) = reply_target.as_ref() {
                    echo.replied_to = Some(RepliedMessage {
                        author: reply.author.clone(),
                        preview: reply.preview.clone(),
                        deleted: false,
                        ..Default::default()
                    });
                }
                messages.push(echo);
                let reply_to = reply_target
                    .take()
                    .map(|r| r.message_id)
                    .filter(|id| !id.is_empty());
                if let Some((token, channel_id, guild_id)) = send_target.clone() {
                    crate::discord::spawn_send_message(token, channel_id, guild_id, url, reply_to);
                }
                if limits.slowmode_secs > 0 {
                    start_slowmode(&ctx, &channel_key, limits.slowmode_secs);
                }
                close(&ctx);
            }
        }
    }

    nitro_required
}