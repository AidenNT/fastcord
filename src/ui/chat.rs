use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Area, Color32, CornerRadius, Frame, Margin, Order, ScrollArea, Sense, Stroke, Vec2};

use crate::lib::data::{
    ChatMessage, ComponentClick, EmojiGroup, Forward, ReactionKind, RepliedMessage, ReplyTarget,
    ThreadCard,
};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::media;
use crate::ui::markdown::MentionCtx;
use crate::ui::theme::{self, Icon, Palette};

const COMPOSER_HEIGHT: f32 = 64.0;
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
const TOOLBAR_BUTTON: f32 = 26.0;
const TOOLBAR_WIDTH: f32 = 148.0;
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
    // muestra un spinner arriba del todo de la lista. Ver
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
    // Hay un pedido de la página posterior (`after`) en curso — spinner
    // abajo del todo de la lista.
    loading_newer: bool,
    // Id del mensaje al que hay que volver a anclar el scroll apenas
    // vuelva a aparecer en `messages` (después de que se antepuso una
    // página más vieja) — ver `App::pending_scroll_anchor`. Se consume
    // (se pone en `None`) una sola vez, apenas se aplica.
    scroll_anchor: &mut Option<String>,
    // Mensaje al que hay que llevar el scroll (centrado) apenas aparezca en
    // `messages` — ver `App::pending_jump`. También se consume una sola vez.
    jump_target: &mut Option<String>,
) -> ChatEvent {
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
    let list_height =
        (ui.available_height() - COMPOSER_HEIGHT - reply_banner_extra - slash_extra - text_extra).max(80.0);
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

    ScrollArea::vertical()
        .id_salt("chat_scroll")
        .max_height(list_height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
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
            if loading && messages.is_empty() {
                // Todavía no llegó ni la primera página: placeholder tipo
                // "esqueleto" en vez de la lista vacía, como hace el
                // cliente real mientras carga un canal/DM.
                message_skeletons(ui, palette);
            } else if messages.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    theme::text(ui, "Todavía no hay mensajes acá.", theme::regular(12.5), palette.dim);
                });
            } else {
                // Centinela de 1px arriba del todo de la lista: si el
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
                if loading_more {
                    loading_more_row(ui, palette);
                } else if has_more
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
                    // Una respuesta siempre arranca un bloque nuevo, con
                    // avatar/header propios arriba del banner citado — no
                    // tendría sentido agruparla bajo el mensaje anterior
                    // y que el banner "Respondiendo a..." quede sin
                    // ningún encabezado propio, como en el cliente real.
                    && messages[i].replied_to.is_none();

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
                    ui.scroll_to_rect(row_rect, Some(egui::Align::TOP));
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
                    ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
                    start_jump_highlight(ui.ctx(), &messages[i].id);
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
                    .map(|next| next.kind != 18 && messages[i].kind != 18 && next.same_author(&messages[i]))
                    .unwrap_or(false);
                ui.add_space(if next_is_grouped { GROUPED_SPACING } else { NEW_GROUP_SPACING });
            }
            // Ventana que no termina en el último mensaje del canal: abajo
            // del todo se piden los siguientes (`?after=<id>`).
            if has_newer && !messages.is_empty() {
                if loading_newer {
                    loading_more_row(ui, palette);
                } else {
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
            }
            ui.add_space(8.0);

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

            for (index, rect) in &row_rects {
                let is_hovered = hovered_index == Some(*index);
                let panel_open_here = open_panel_index == Some(*index);
                if is_hovered || panel_open_here {
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
                if panel_open_here {
                    reaction_panel(ui, palette, *index, *rect, &mut toggled_reaction, &mut nitro_required, custom_emojis);
                }
            }
        });

    ui.ctx().memory_mut(|m| m.data.insert_temp(row_rects_memory_id(), row_rects));

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

    if let Some((id, name, owner_id)) = actions.open_thread {
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

/// Fila angosta con el spinner de "cargando más mensajes", arriba del
/// todo de la lista mientras `App::load_more_messages` espera la
/// respuesta de Discord.
fn loading_more_row(ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - 16.0) / 2.0).max(0.0));
        theme::spinner(ui, 16.0, palette.dim);
    });
    ui.add_space(NEW_GROUP_SPACING);
}

/// Placeholder mientras se pide la primera página de mensajes de un
/// canal/DM (`loading` en `show`), imitando el "esqueleto" gris que
/// muestra el cliente real de Discord antes de que llegue el historial:
/// unas filas de rectángulos grises (avatar + una o dos líneas de texto)
/// con un pulso sutil de opacidad para que no se sienta una pantalla
/// muerta mientras se espera.
fn message_skeletons(ui: &mut egui::Ui, palette: &Palette) {
    // Pulso lento y parejo entre `surface` y `surface_hover` (no el sweep
    // del `theme::spinner`, que es para un círculo cargando de verdad) —
    // solo para que las barras no se vean como un dibujo estático.
    let pulse = (ui.input(|i| i.time) * 1.6).sin() as f32 * 0.5 + 0.5;
    let bar_color = extra::blend(palette.surface, palette.surface_hover, pulse);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));

    // Anchos de línea (como fracción del ancho disponible) escritos a
    // mano para que las filas no se vean todas idénticas — mismo detalle
    // que usa el placeholder real de Discord para no sentirse repetido.
    const ROWS: &[(f32, Option<f32>)] = &[
        (0.55, Some(0.30)),
        (0.35, None),
        (0.62, Some(0.20)),
        (0.45, Some(0.40)),
        (0.70, None),
        (0.30, Some(0.55)),
        (0.50, None),
    ];

    for &(first_w, second_w) in ROWS {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (avatar_rect, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
            ui.painter().circle_filled(avatar_rect.center(), 18.0, bar_color);

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let width = ui.available_width().min(420.0);
                let (first_rect, _) =
                    ui.allocate_exact_size(Vec2::new(width * first_w, 10.0), Sense::hover());
                ui.painter().rect_filled(first_rect, 4.0, bar_color);
                if let Some(second_w) = second_w {
                    let (second_rect, _) =
                        ui.allocate_exact_size(Vec2::new(width * second_w, 10.0), Sense::hover());
                    ui.painter().rect_filled(second_rect, 4.0, bar_color);
                }
            });
        });
        ui.add_space(NEW_GROUP_SPACING);
    }
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
    ui.vertical(|ui| {
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
    })
    .response
    .rect
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

/// Barra flotante que aparece al pasar el mouse por un mensaje, con las
/// acciones rápidas: responder, reenviar, agregar la reacción rápida de
/// turno, y abrir el panel de más reacciones. Reemplaza al botón de "+"
/// que antes se mostraba siempre debajo de cada mensaje.
///
/// Nota sobre los íconos: el set de Lucide que trae este proyecto
/// (`assets/icons/`) no incluye ninguno de "responder" (flecha curva) ni
/// "reenviar" (flecha compartir) — se usan `ArrowLeft`/`ArrowRight` como
/// reemplazo, mismo criterio que ya usa `composer` con `Sparkles` para el
/// ícono de emoji que tampoco está en el set.
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
    actions: &mut RowActions,
    can_create_thread: bool,
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

                        if theme::icon_button(ui, Icon::ArrowLeft, 14.0, palette.dim, palette.text, "Responder").clicked() {
                            *reply_target = Some(ReplyTarget {
                                message_id: msg.id.clone(),
                                author: msg.author.clone(),
                                preview: preview_text(&msg.content),
                            });
                        }

                        // Reenviar: no hay picker de canal/DM destino
                        // todavía en este cliente, así que esto no manda
                        // nada por su cuenta — solo le avisa a quien
                        // llamó a `show` (ver `ChatEvent`), que es quien
                        // sabe mostrar un toast.
                        if theme::icon_button(ui, Icon::ArrowRight, 14.0, palette.dim, palette.text, "Reenviar").clicked() {
                            *forward_requested = true;
                        }

                        // Hilo: abre el que ya tiene el mensaje o, si no
                        // tiene y se puede, crea uno (con el principio del
                        // mensaje de nombre, como el cliente real).
                        let has_thread = msg.thread.is_some();
                        if !msg.id.is_empty() && (has_thread || (can_create_thread && msg.kind == 0)) {
                            let tooltip = if has_thread { "Abrir hilo" } else { "Crear hilo" };
                            if theme::icon_button(ui, Icon::ListPlus, 14.0, palette.dim, palette.text, tooltip).clicked() {
                                if let Some(card) = msg.thread.as_ref() {
                                    actions.open_thread =
                                        Some((card.id.clone(), card.name.clone(), card.owner_id.clone()));
                                } else {
                                    let name: String = msg.content.chars().take(100).collect();
                                    let name = if name.trim().is_empty() { "Nuevo hilo".to_string() } else { name };
                                    actions.create_thread = Some((msg.channel_id.clone(), msg.id.clone(), name));
                                }
                            }
                        }

                        if theme::icon_button(ui, Icon::Sparkles, 14.0, palette.dim, palette.text, "Reacción rápida").clicked() {
                            let quick = quick_reactions();
                            if let Some(next_emoji) = quick.iter().find(|e| {
                                let candidate = ReactionKind::Unicode((*e).clone());
                                !msg.reactions.iter().any(|r| r.emoji == candidate)
                            }) {
                                if let Some(key) = crate::discord::frecency::emoji_key_for_unicode(next_emoji) {
                                    crate::discord::frecency::record_reaction_use(&key);
                                }
                                *toggled_reaction = Some((index, ReactionKind::Unicode(next_emoji.clone())));
                            }
                        }

                        if theme::icon_button(ui, Icon::CirclePlus, 14.0, palette.dim, palette.text, "Más reacciones").clicked() {
                            set_open_reaction_panel(ui.ctx(), Some(index));
                        }
                    });
                });
        });
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
                    theme::icon(ui, Icon::CirclePlus, 16.0, palette.dim);

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
                    ctx.memory_mut(|m| m.data.insert_temp(text_extra_id, text_extra));
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
                    if enter_pressed && !compose_text.trim().is_empty() {
                        // `:nombre:` -> `<:nombre:id>` para los emojis personalizados.
                        let picked = menus::take_customs(&ctx, customs_id);
                        let content = menus::expand_shortcodes(compose_text.trim(), &picked, custom_emojis);
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
                        // `multiline` no suelta el foco solo, pero tampoco
                        // lo retiene mágicamente frame a frame después de
                        // vaciar el texto por fuera del widget — lo
                        // reafirmamos para que se pueda seguir tipeando
                        // sin tener que volver a clickear la barra.
                        response.request_focus();
                    }
                });
            });
        composer_rect = framed.response.rect;
    });

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
            menus::PickerResult::Sticker(sticker) => {
                // Un sticker se manda solo, sin texto. Si había una respuesta
                // en curso, el sticker la contesta.
                if let Some((token, channel_id, guild_id)) = send_target.clone() {
                    let reply_to = reply_target
                        .take()
                        .map(|r| r.message_id)
                        .filter(|id| !id.is_empty());
                    crate::discord::spawn_send_sticker(token, channel_id, guild_id, sticker.id.clone(), reply_to);
                }
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
                close(&ctx);
            }
        }
    }

    nitro_required
}