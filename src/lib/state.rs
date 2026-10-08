use std::time::Instant;

use egui::Color32;

use crate::discord::models::{PrivateChannel, User, UserProfileResponse};
use crate::discord::voice::VoiceRuntimeEvent;
use crate::discord::voice::{StreamWatchHandle, StreamWatchParams, spawn_stream_watch};
use crate::discord::StreamWatchStatus;
use crate::discord::voice::{VoiceAudioSourceOptions, VoiceAudioSources, list_voice_audio_sources};
use crate::discord::{AppEvent, CurrentVoiceConnectionState, VoiceAudioSettings, VoiceCache, VoiceConnectionStatus};
use crate::lib::data::{demo_activity, demo_friends, demo_servers, ActivityCard, ChatMessage, Friend, NameResolver, Server};
use crate::theme::{Backdrop, BackdropRuntime, Palette, ThemeDef, ThemeEditor, ThemeMode};
use crate::ui::settings::SettingsTab;

/// Cuánto se espera sin cambios en el volumen/silencio de alguien antes de
/// guardarlo en la cuenta.
const PARTICIPANT_AUDIO_SYNC_DELAY: std::time::Duration = std::time::Duration::from_millis(700);

/// Mezcla lo que trae la cuenta con el ajuste local de volumen/silencio.
/// `replace`: `entries` es la lista completa (si no, solo lo que cambió). Lo que
/// se tocó acá y todavía no se mandó (`dirty`) gana siempre sobre lo que llegue.
fn merged_playback(
    current: &std::collections::HashMap<u64, crate::discord::VoiceParticipantPlaybackSettings>,
    dirty: &std::collections::HashSet<u64>,
    entries: Vec<(
        crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
        crate::discord::VoiceParticipantPlaybackSettings,
    )>,
    replace: bool,
) -> std::collections::HashMap<u64, crate::discord::VoiceParticipantPlaybackSettings> {
    let mut next: std::collections::HashMap<u64, crate::discord::VoiceParticipantPlaybackSettings> =
        if replace {
            dirty
                .iter()
                .filter_map(|raw| current.get(raw).map(|playback| (*raw, *playback)))
                .collect()
        } else {
            current.clone()
        };
    for (id, playback) in entries {
        let raw = id.get();
        if dirty.contains(&raw) {
            continue;
        }
        if playback == crate::discord::VoiceParticipantPlaybackSettings::default() {
            next.remove(&raw);
        } else {
            next.insert(raw, playback);
        }
    }
    next
}
use web_local_storage_api;

// Informe de memoria (Ajustes → Memoria): hijo de este módulo para poder leer
// los campos privados de `App`. Ver `lib/state/memory.rs`.
pub mod memory;

/// Pantalla completa que se está mostrando.
pub enum Screen {
    Login,
    /// Lista de amigos (la pantalla "Home" estilo Discord).
    Home,
    /// Conversación directa abierta; el índice referencia `App::friends`.
    Dm(usize),
    /// Servidor abierto; el índice referencia `App::servers`.
    Server(usize),
}

/// Estado del flujo de autenticación (login screen). El camino normal es
/// SignedOut -> Starting -> AwaitingScan -> Confirming -> Connecting ->
/// Connected; con un token guardado de antes se salta directo a
/// Connecting.
pub enum AuthStatus {
    SignedOut,
    /// Ya nos conectamos al servidor de "remote auth" pero todavía no nos
    /// mandó el fingerprint para armar el QR.
    Starting,
    /// QR listo para escanear con el celular.
    AwaitingScan { url: String },
    /// Escaneado; falta que el usuario confirme en el celular.
    Confirming { username: String },
    /// Login resuelto (QR o token guardado); esperando el `READY` del
    /// Gateway con los datos reales.
    Connecting,
    Connected,
    Failed(String),
    /// Pantalla de usuario/contraseña, esperando que el usuario la
    /// complete (o mostrando el error de un intento anterior — ver
    /// `App::login_error`). No hay un `Starting` equivalente: acá no hace
    /// falta abrir ninguna conexión hasta que se toca "Iniciar sesión".
    PasswordForm,
    /// Mandando el POST de login por contraseña.
    PasswordSubmitting,
    /// Discord pidió 2FA (`App::pending_mfa` tiene el desafío: métodos
    /// disponibles, ticket, login_instance_id).
    PasswordMfaRequired,
    /// Mandando el código de 2FA.
    PasswordMfaSubmitting,
}

/// De qué tipo es un [`Toast`]; solo cambia el color del punto de acento.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
}

/// Notificación flotante apilada en una esquina de la ventana. Se
/// autodescarta pasados unos segundos; ver `ui::overlay::show_toasts`.
pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    pub message: String,
    pub created: Instant,
}

/// A dónde lleva una notificación al hacerle click (ver
/// `App::open_notification_target`).
#[derive(Clone)]
pub enum NotificationTarget {
    Dm { channel_id: String },
    Channel { guild_id: String, channel_id: String },
}

impl NotificationTarget {
    pub fn channel_id(&self) -> &str {
        match self {
            NotificationTarget::Dm { channel_id } => channel_id,
            NotificationTarget::Channel { channel_id, .. } => channel_id,
        }
    }
}

/// Límites y valor por defecto del tamaño de la interfaz (`App::ui_scale`).
pub const UI_SCALE_MIN: f32 = 0.6;
pub const UI_SCALE_MAX: f32 = 1.5;
pub const UI_SCALE_DEFAULT: f32 = 0.85;

/// De qué lado de la ventana se apilan las tarjetas de notificación
/// (`ui::notifications::show_in_app`). Se elige en Ajustes → Apariencia y se
/// guarda en `web_local_storage_api` bajo `notification_side`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationSide {
    Left,
    #[default]
    Right,
}

impl NotificationSide {
    fn as_str(self) -> &'static str {
        match self {
            NotificationSide::Left => "left",
            NotificationSide::Right => "right",
        }
    }

    fn from_str(raw: &str) -> Option<Self> {
        match raw.trim().trim_matches('"') {
            "left" => Some(NotificationSide::Left),
            "right" => Some(NotificationSide::Right),
            _ => None,
        }
    }
}

/// Notificación de mención/DM que se muestra DENTRO de la app (en el lado
/// elegido por `NotificationSide`) cuando la ventana tiene el foco; con la
/// ventana sin foco se manda una del escritorio en su lugar. Ver
/// `ui::notifications::show_in_app` y `App::notify_incoming`.
pub struct InAppNotification {
    pub title: String,
    pub body: String,
    /// Avatar de quien escribió, si tiene uno (si no, la tarjeta dibuja un
    /// círculo con `initial`).
    pub avatar_url: Option<String>,
    /// Inicial del autor para el avatar de reemplazo.
    pub initial: String,
    pub target: NotificationTarget,
    /// Mensajes de la misma conversación juntados en esta tarjeta.
    pub count: u32,
    pub created: Instant,
}

/// Panel lateral con un hilo abierto (a la derecha del chat, como en el
/// cliente real). Los mensajes del hilo viven en un canal "oculto" del
/// server (`Server::open_thread`), así que el historial, los mensajes en
/// vivo y el reconciliado del eco local funcionan igual que en cualquier
/// canal; acá solo está lo propio del panel.
pub struct ThreadPanel {
    pub thread_id: String,
    pub title: String,
    /// Quién lo empezó, si se conoce (`ThreadChannel::owner_id`).
    pub owner_id: Option<String>,
    pub compose_text: String,
    pub reply_target: Option<crate::lib::data::ReplyTarget>,
    pub scroll_anchor: Option<String>,
}

/// Un campo de texto de un [`ComponentModal`].
pub struct ModalField {
    pub custom_id: String,
    pub label: String,
    pub placeholder: String,
    pub paragraph: bool,
    pub required: bool,
    pub min_length: usize,
    pub max_length: usize,
    pub value: String,
}

/// Formulario que un bot pidió tras apretar uno de sus botones (por ejemplo
/// "Responder anónimo"). Se dibuja en `ui::overlay::show_component_modal`.
pub struct ComponentModal {
    pub request: crate::discord::models::ModalRequest,
    pub fields: Vec<ModalField>,
    /// Aviso de validación (campo obligatorio vacío, texto muy corto...).
    pub error: Option<String>,
}

impl ComponentModal {
    pub fn new(request: crate::discord::models::ModalRequest) -> Self {
        let mut found = Vec::new();
        for component in &request.components {
            component.collect_text_inputs(&mut found);
        }
        let fields = found
            .into_iter()
            .filter_map(|(input, label)| {
                Some(ModalField {
                    custom_id: input.custom_id.clone()?,
                    label: label.unwrap_or_default().to_string(),
                    placeholder: input.placeholder.clone().unwrap_or_default(),
                    paragraph: input.style == 2,
                    required: input.required.unwrap_or(true),
                    min_length: input.min_length.unwrap_or(0) as usize,
                    max_length: input.max_length.map(|n| n as usize).unwrap_or(4000),
                    value: input.value.clone().unwrap_or_default(),
                })
            })
            .collect();
        Self { request, fields, error: None }
    }

    /// Revisa los campos; si hay algún problema deja el aviso en `error` y
    /// devuelve `false`.
    pub fn validate(&mut self) -> bool {
        for field in &self.fields {
            let len = field.value.trim().chars().count();
            let name = if field.label.is_empty() { "El campo" } else { field.label.as_str() };
            if field.required && len == 0 {
                self.error = Some(format!("{name} es obligatorio."));
                return false;
            }
            if len > 0 && len < field.min_length {
                self.error = Some(format!("{name} necesita al menos {} caracteres.", field.min_length));
                return false;
            }
        }
        self.error = None;
        true
    }
}

/// Popup modal centrado con fondo semi-transparente y un botón de
/// confirmación. Ver `ui::overlay::show_modal`.
pub struct Modal {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
}

/// Lista de miembros de un rol, abierta al clickear una mención `@rol`
/// (ver `ui::role_popup`).
pub struct RolePopup {
    pub role_id: String,
    pub name: String,
    /// Color `0xRRGGBB` del rol (0 = sin color).
    pub color: u32,
    /// Posición del puntero al clickear; el popup se abre al lado.
    pub anchor: Option<egui::Pos2>,
}

/// Tarjeta de perfil abierta al clickear un avatar/nombre en cualquier
/// lado (chat, lista de miembros, panel de amigos) — ver
/// `ui::profile_popup`. Arranca con lo que ya tengamos a mano localmente
/// (nombre, avatar, si es de la lista de amigos) mientras el pedido REST
/// de `App::open_user_profile` trae el resto (bio, banner, en común,
/// roles en el server actual).
pub struct ProfilePopup {
    pub user_id: String,
    /// Server desde el que se abrió (si se abrió desde uno), para pedirle
    /// a Discord el apodo/roles de esa persona EN ESE server puntual.
    pub guild_id: Option<String>,
    /// Nombre a mostrar mientras el perfil completo todavía no llegó (el
    /// que ya teníamos del mensaje/fila que se clickeó).
    pub fallback_name: String,
    pub fallback_avatar_url: Option<String>,
    pub fallback_avatar_color: Color32,
    /// `true` mientras el pedido de perfil está en vuelo.
    pub loading: bool,
    /// `true` si el pedido de perfil falló (usuario eliminado, sin
    /// permiso para verlo, cortó la red, etc.) — la tarjeta se sigue
    /// mostrando con lo que había en `fallback_*`, solo que sin bio ni
    /// en común.
    pub failed: bool,
    /// Perfil completo, una vez que llega.
    pub profile: Option<UserProfileResponse>,
    /// Si la bio se está mostrando completa o recortada (\"Ver biografía
    /// completa\").
    pub bio_expanded: bool,
    /// Texto que se está escribiendo en la cajita de mensaje rápido del
    /// pie de la tarjeta (\"Enviar mensaje a @...\").
    pub quick_message: String,
    /// Dónde se clickeó (posición del puntero) para abrir la tarjeta al
    /// lado del avatar, como el popout de Discord. `None` = centrada.
    pub anchor: Option<egui::Pos2>,
}

/// Perfil completo abierto (modal grande, ver `ui::profile_popup::show_full`).
/// Los datos salen del perfil ya cargado (`dm_profile` o `profile_popup`).
pub struct FullProfile {
    pub user_id: String,
    /// Pestaña de la derecha: 0 actividad, 1 amigos en común, 2 servidores.
    pub tab: usize,
}

/// Lo que ya se sabe de un stream (Go Live) que pedimos ver. `STREAM_CREATE`
/// y `STREAM_SERVER_UPDATE` llegan por separado y pueden invertirse, así que
/// se van juntando acá hasta tener los tres datos (`try_start_stream_watch`).
#[derive(Default)]
struct PendingStreamSession {
    rtc_server_id: Option<String>,
    endpoint: Option<String>,
    token: Option<String>,
}

/// El stream que se está viendo ahora mismo (a lo sumo uno).
pub struct WatchedStream {
    /// `guild:<guild>:<canal>:<usuario>` — identifica el stream en Discord.
    pub stream_key: String,
    /// Canal de voz donde transmite (para mostrar el visor en esa vista).
    pub channel_id: String,
    /// Nombre de quien transmite, para el título del visor.
    pub owner_name: String,
    pub status: StreamWatchStatus,
    /// Detalle del error cuando `status == Failed`.
    pub message: Option<String>,
    /// Conexión de media. `None` hasta que llegan `rtc_server_id`, endpoint y
    /// token; al soltarlo se corta la conexión.
    handle: Option<StreamWatchHandle>,
    /// Último frame subido a la GPU.
    pub texture: Option<egui::TextureHandle>,
    /// Tamaño en píxeles de `texture`, para respetar la proporción.
    pub frame_size: [usize; 2],
    /// Cuántos frames se subieron a la textura (contador de diagnóstico que
    /// muestra el visor).
    pub frames_shown: u64,
}

pub struct App {
    pub screen: Screen,
    pub auth: AuthStatus,
    /// Usuario logueado (viene de `READY`). `None` mientras no haya sesión
    /// real (pantallas demo).
    pub me: Option<User>,
    /// Cuentas guardadas en este equipo (ver `lib::accounts`): alimentan el
    /// selector de cuentas del login y de Ajustes → Cuenta.
    pub accounts: crate::lib::accounts::AccountStore,
    /// Token de la cuenta ya autenticada, para pedidos de REST puntuales
    /// (abrir un DM, traer los canales de un server) que arma `App`
    /// mismo, aparte de la conexión al Gateway.
    discord_token: Option<String>,
    /// `PreloadedUserSettings` real de la cuenta, tal cual lo trajo
    /// `discord::spawn_fetch_user_settings` la última vez (ver
    /// `AppEvent::UserSettings`). `None` hasta que llega esa respuesta —
    /// o si el pedido falló, o si `App` sigue en una pantalla demo sin
    /// sesión real. Guardado entero (no solo lo que ya usamos para
    /// `theme_mode`) para que otras partes de la UI puedan leer más
    /// adelante otros campos (status, notificaciones, privacidad...) sin
    /// tener que pedirlo de nuevo.
    pub discord_settings: Option<crate::discord::user_settings::PreloadedUserSettings>,
    /// Volumen y silencio local de cada persona de las llamadas (clic derecho
    /// sobre su tile). Es el espejo de `audio_context_settings.user` de la
    /// cuenta: se llena con lo que trae `AppEvent::UserSettings`/`UserSettingsUpdate`
    /// y se manda de vuelta con `flush_participant_audio`. Solo guarda a quien
    /// se apartó de lo normal (100 %, sin silenciar). Clave: id de usuario.
    participant_playback:
        std::collections::HashMap<u64, crate::discord::VoiceParticipantPlaybackSettings>,
    /// Personas cuyo ajuste cambió acá y todavía no se mandó a la cuenta.
    participant_audio_dirty: std::collections::HashSet<u64>,
    /// Último cambio pendiente; el `PATCH` sale cuando pasa
    /// `PARTICIPANT_AUDIO_SYNC_DELAY` sin nuevos cambios (arrastrar el slider
    /// genera uno por frame).
    participant_audio_dirty_at: Option<Instant>,
    /// Lo mismo para el AUDIO DE LOS STREAMS (clic derecho sobre el video):
    /// volumen y silencio por streamer, espejo de `audio_context_settings.stream`.
    /// Clave: id de usuario de quien transmite.
    stream_playback:
        std::collections::HashMap<u64, crate::discord::VoiceParticipantPlaybackSettings>,
    /// Streamers cuyo ajuste cambió acá y todavía no se mandó (comparte
    /// `participant_audio_dirty_at` con las personas).
    stream_audio_dirty: std::collections::HashSet<u64>,
    /// Relación con cada persona, por id: 1 amigo, 2 bloqueado, 3 solicitud
    /// recibida, 4 solicitud enviada. Sin entrada = sin relación. Alimenta el
    /// menú de clic derecho (`ui::audio_menu`): "Añadir amigo" / "Desbloquear".
    relationship_kinds: std::collections::HashMap<String, u8>,
    /// Solicitudes de amistad pendientes (tipos 3 y 4 de `relationship_kinds`,
    /// con los datos para dibujarlas). Alimenta la pestaña "Pendiente" del
    /// Inicio; se mantiene al día con `RELATIONSHIP_ADD`/`RELATIONSHIP_REMOVE`.
    pub pending_requests: Vec<crate::lib::data::PendingRequest>,
    /// Texto del campo de la pantalla "Añadir amigo".
    pub add_friend_input: String,
    /// Hay una solicitud por nombre de usuario en camino.
    pub add_friend_busy: bool,
    /// Resultado de la última solicitud por nombre: (salió bien, mensaje).
    pub add_friend_status: Option<(bool, String)>,
    /// Personas ignoradas (`Relationship::user_ignored` en `READY`, y las que
    /// se ignoran desde el menú).
    ignored_users: std::collections::HashSet<String>,
    /// Personas con el vídeo deshabilitado desde el menú de clic derecho.
    video_disabled_users: std::collections::HashSet<String>,
    /// Llamar a esta persona apenas llegue su canal de DM (`AppEvent::DmOpened`).
    pending_call_user: Option<String>,
    /// (id, nombre) de la nota que se está por editar, hasta que llegue su texto.
    pending_note: Option<(String, String)>,
    /// Versión de lo que ve el menú de clic derecho; se sube cuando cambia y
    /// `publish_menu_context` lo vuelve a publicar.
    menu_ctx_rev: u64,
    menu_ctx_published: Option<(u64, usize)>,
    /// Ids de las carpetas de servers (`GuildFolder::id`) que están
    /// expandidas en la barra lateral (`ui::rail`). Solo en memoria — no
    /// se persiste entre sesiones a propósito, a diferencia del cliente
    /// oficial: implementarlo bien requeriría mandar un PATCH de
    /// `guild_folders` de vuelta a Discord cada vez que el usuario
    /// pliega/despliega una, y por ahora esto es de solo lectura.
    pub open_guild_folders: std::collections::HashSet<i64>,
    /// Extremo de recepción de eventos del hilo de Discord
    /// (`discord::spawn_login_flow` y afines). `None` hasta el primer
    /// `start_sign_in`.
    event_rx: Option<std::sync::mpsc::Receiver<AppEvent>>,
    /// Extremo de envío, guardado para poder lanzar pedidos puntuales
    /// (`open_dm`, `open_server`) que reusan el mismo canal hacia `App`.
    event_tx: Option<std::sync::mpsc::Sender<AppEvent>>,
    /// Extremo de envío de comandos al Gateway (ver
    /// `AppEvent::GatewayCommands`). `None` hasta que arranca la conexión.
    gateway_commands: Option<tokio::sync::mpsc::UnboundedSender<crate::discord::gateway::GatewayCommand>>,
    /// (guild, canal) de la última suscripción a la lista de miembros que
    /// se le mandó al Gateway. `None` = en esta conexión todavía no hay
    /// ninguna.
    member_subscription: Option<(String, String)>,
    /// Suscripción que se quiere mandar pero espera unos milisegundos por
    /// si el usuario sigue cambiando de canal (ver
    /// `App::flush_member_subscription`): (guild, canal, desde cuándo).
    pending_member_subscription: Option<(String, String, Instant)>,
    pub friends: Vec<Friend>,
    pub dms: Vec<PrivateChannel>,
    pub activity: Vec<ActivityCard>,
    pub servers: Vec<Server>,
    /// Paleta activa (tu `Palette::dark()` / `Palette::light()` real).
    pub palette: Palette,
    /// (categoría, canal) actualmente abierto dentro de `servers[Screen::Server(i)]`.
    pub current_channel: (usize, usize),
    /// Texto de búsqueda de la lista de amigos, en la pantalla Home.
    pub friends_search: String,
    /// Tab seleccionada en la pantalla Home: 0 = En línea, 1 = Todos,
    /// 2 = Pendiente, 3 = Sugerencias (demo, sin datos reales para 2 y 3).
    pub friends_tab: usize,
    /// Texto que se está escribiendo en el input de un DM o canal.
    pub compose_text: String,
    /// Mensaje al que se está respondiendo ahora mismo (botón "Responder"
    /// de la barra que aparece al pasar el mouse — ver `ui::chat`), si
    /// hay alguno. Un solo campo, no por canal, igual que `compose_text`.
    pub reply_target: Option<crate::lib::data::ReplyTarget>,
    /// Id del mensaje al que hay que volver a "anclar" el scroll apenas
    /// llegue (y se prependa) la próxima página de `App::load_more_messages`
    /// — así la lista no salta cuando aparecen mensajes más viejos arriba
    /// de lo que el usuario ya estaba mirando. Lo consume `ui::chat::show`
    /// (parámetro `scroll_anchor`) una sola vez, apenas ese mensaje vuelve
    /// a aparecer en la lista actualizada.
    pub pending_scroll_anchor: Option<String>,
    /// Id del mensaje al que hay que llevar el scroll del chat apenas
    /// aparezca en la lista (ir a un mensaje puntual: click en una
    /// respuesta, abrir un canal reciente en su último leído...). Lo
    /// consume `ui::chat::show` una sola vez, centrando ese mensaje.
    pub pending_jump: Option<String>,
    /// Si el panel de notificaciones de la barra superior está abierto
    /// (ver `ui::inbox`).
    pub inbox_open: bool,
    /// Si falló un pedido `after`, hasta cuándo no se reintenta (la UI lo
    /// vuelve a pedir en cuanto el final de la lista queda a la vista, y sin
    /// este freno lo repetiría cada frame).
    newer_retry_after: Option<Instant>,
    /// Hilo abierto en el panel lateral del server, si hay alguno.
    pub thread_panel: Option<ThreadPanel>,
    /// Último momento (reloj de la UI, segundos) en que se recortó el
    /// historial de los chats inactivos (`App::trim_inactive_history`).
    pub last_history_trim: f64,
    /// Formulario pedido por un bot (tras apretar uno de sus botones).
    pub component_modal: Option<ComponentModal>,
    /// Botones de bots que ya mandaron su interacción y esperan respuesta:
    /// se dibujan con un spinner adentro (ver `ui::components`).
    pub pending_buttons: Vec<crate::lib::data::PendingButton>,
    /// El Gateway se cortó y `gateway::run` está reintentando: la barra
    /// superior muestra el spinner amarillo con un "!" hasta que vuelve
    /// (`GatewayDisconnected` lo prende; `GatewayReconnected` o perder la
    /// conexión del todo lo apagan).
    pub gateway_disconnected: bool,
    /// `session_id` del Gateway (del `READY`): las interacciones con
    /// botones de bots lo piden.
    pub gateway_session_id: String,
    /// Catálogos de slash commands ya pedidos, por server (o por canal en un
    /// DM). Ver `ui::slash` y `discord::slash`.
    pub slash_catalogs: std::collections::HashMap<String, crate::discord::slash::CachedCatalog>,
    /// Claves de `slash_catalogs` con un pedido en vuelo.
    pub slash_pending: std::collections::HashSet<String>,
    /// Texto del buscador de la barra superior (`ui::topbar`); decorativo,
    /// no filtra nada todavía.
    pub top_search: String,
    /// Notificaciones flotantes en pantalla, más recientes al final.
    pub toasts: Vec<Toast>,
    /// Si la ventana tiene el foco (se actualiza cada frame en `ui`). Decide
    /// si una mención se avisa dentro de la app o en el escritorio.
    pub window_focused: bool,
    /// Menciones sin leer por canal (id de canal → cantidad), tanto de DMs
    /// (todo mensaje cuenta) como de canales de server (solo menciones).
    pub unread_mentions: std::collections::HashMap<String, u32>,
    /// Las mismas menciones sumadas por server (`guild_id` → cantidad), para
    /// el badge del rail.
    pub guild_mentions: std::collections::HashMap<String, u32>,
    /// Último mensaje que Discord tiene como leído por canal (canal → id de
    /// mensaje). Arranca con lo que trae el `READY` (`read_state`) y se
    /// actualiza cada vez que este cliente manda un ack o llega un
    /// `MESSAGE_ACK`. Sirve para no repetir acks.
    acked_messages: std::collections::HashMap<String, String>,
    /// Cuándo se mandó el último ack por canal, para no pegarle a la API en
    /// cada mensaje cuando el chat abierto recibe muchos seguidos.
    ack_sent_at: std::collections::HashMap<String, Instant>,
    /// Menciones sin leer que trajo el `READY` y todavía no se pudieron
    /// asignar a un canal conocido (los canales de un server recién se
    /// cargan al abrirlo). Se reintenta al llegar `GuildChannels`.
    pending_mentions: std::collections::HashMap<String, u32>,
    /// Última presencia conocida por usuario (estado + actividad). Cubre a
    /// los que están en la lista de DMs sin ser amigos, que no están en
    /// `friends`.
    presences: std::collections::HashMap<String, (crate::lib::data::Status, Option<String>)>,
    /// Actividades (juego, música...) por usuario, del Gateway. Alimenta el
    /// panel "Activo ahora".
    user_activities: std::collections::HashMap<String, Vec<crate::discord::models::PresenceActivity>>,
    /// Texto del estado personalizado (el "Can't I Not take it in vain..."
    /// que se ve en el globito del perfil) por usuario. Las actividades de
    /// arriba lo descartan a propósito (no es un juego), así que se guarda aparte.
    custom_statuses: std::collections::HashMap<String, String>,
    /// Actividades de la propia cuenta (`READY.sessions`/`SESSIONS_REPLACE`).
    own_activities: Vec<crate::discord::models::PresenceActivity>,
    /// Afinidad (%) con cada usuario, de `GET /users/@me/affinities/users`.
    /// Vacío hasta que llega `AppEvent::Affinities`. Ordena los amigos del
    /// Inicio de la interfaz nueva.
    pub affinity_users: crate::discord::affinities::AffinityMap,
    /// Afinidad (%) con cada server, de `GET /users/@me/affinities/guilds`.
    /// Ordena "Servidores frecuentes" en el Inicio de la interfaz nueva.
    pub affinity_guilds: crate::discord::affinities::AffinityMap,
    /// Pedido de scroll de los carruseles del Inicio nuevo (0 = amigos,
    /// 1 = servers): offset horizontal al que mover la fila en el próximo
    /// frame (lo ponen las flechas `‹ ›`).
    pub home_scroll_req: [Option<f32>; 2],
    /// Tarjetas de notificación dentro de la app, más recientes al final.
    pub in_app_notifications: Vec<InAppNotification>,
    /// Lado de la ventana donde se apilan esas tarjetas y los avisos.
    pub notification_side: NotificationSide,
    /// Servers/canales/DMs silenciados y nivel de mensajes (vienen en el
    /// `READY`, ver `lib::notifications::MuteRules`).
    mute_rules: crate::lib::notifications::MuteRules,
    /// Último total de menciones puesto en el título de la ventana.
    shown_title_count: Option<u32>,
    /// Popup modal actualmente abierto, si hay alguno.
    pub modal: Option<Modal>,
    /// Cola de diálogos (avisos, confirmaciones, novedades, formularios) del
    /// sistema `ui::dialog`; se muestra de a uno, el primero de la cola.
    /// Usar `App::open_dialog`.
    pub dialogs: std::collections::VecDeque<crate::ui::dialog::Dialog>,
    /// Token de la sesión guardada que NO se conectó al arrancar porque hay un
    /// aviso de seguridad pendiente (ver `ui::security_warning`). Se usa (y se
    /// vacía) cuando la persona acepta el aviso; si cierra ecord, nunca se
    /// envía a ningún lado.
    pub pending_resume_token: Option<String>,
    /// Tarjeta de perfil de usuario actualmente abierta, si hay alguna
    /// (ver `App::open_user_profile` / `ui::profile_popup`).
    pub profile_popup: Option<ProfilePopup>,
    /// Miembros de un rol (clic en una mención `@rol`), si está abierto.
    pub role_popup: Option<RolePopup>,
    /// Perfil que se muestra de forma permanente en el panel derecho de un DM
    /// (`ui::profile_popup::show_panel`). Mismo formato que `profile_popup`.
    pub dm_profile: Option<ProfilePopup>,
    /// Modal de "Ver perfil completo", si está abierto.
    pub profile_full: Option<FullProfile>,
    /// Modo de tema activo (oscuro / claro / según el wallpaper / uno de
    /// `custom_themes`). Se persiste solo; ver `set_theme_mode`.
    pub theme_mode: ThemeMode,
    /// Temas creados por el usuario desde el editor de `ui::settings`,
    /// cargados de `web_local_storage_api` al abrir la app.
    pub custom_themes: Vec<ThemeDef>,
    /// Si el panel de ajustes (`ui::settings`) está abierto.
    pub settings_open: bool,
    /// "Usar nueva interfaz" (Ajustes → Apariencia): la barra de llamada va a
    /// todo el ancho abajo de la ventana (`ui::call_bar::show_bottom`) en vez
    /// de la tarjeta chica del panel izquierdo. Se persiste solo; ver
    /// `set_new_call_ui`.
    pub new_call_ui: bool,
    /// Tamaño de toda la interfaz (zoom): 1.0 = tamaño original, menos = más
    /// chica. Se elige en Ajustes → Apariencia (o con Ctrl + / Ctrl -) y se
    /// persiste; ver `set_ui_scale`.
    pub ui_scale: f32,
    /// Último zoom que se le puso a egui (NaN = todavía ninguno). Si el de egui
    /// difiere, el cambio vino de afuera (atajo de teclado) y se adopta.
    pub ui_scale_applied: f32,
    /// Editor de temas abierto dentro del panel de ajustes ("Crear
    /// tema"/"Editar tema"), si hay uno.
    pub theme_editor: Option<ThemeEditor>,
    /// Fondo activo (degradado / imagen / sólido + transparencia). Sale del
    /// tema `Custom` elegido; con oscuro/claro/wallpaper es `Backdrop::default()`
    /// (sólido y opaco, o sea, como siempre).
    pub backdrop: Backdrop,
    /// Textura y carga en segundo plano de la imagen del fondo activo.
    pub backdrop_rt: BackdropRuntime,
    /// Último color dominante que se sacó del wallpaper, para no dejar
    /// la app sin color mientras el hilo de fondo saca el primero y para
    /// arrancar el editor de temas con ese color como base. `None` hasta
    /// que `poll_wallpaper_updates` recibe el primer resultado.
    wallpaper_seed: Option<[u8; 3]>,
    /// Extremo de recepción del hilo de fondo que vigila el wallpaper
    /// (ver `start_wallpaper_watch`). `None` hasta la primera vez que se
    /// activa `ThemeMode::Wallpaper`; una vez arrancado, el hilo vive
    /// todo lo que dure la app (es barato: chequea cada pocos segundos y
    /// solo vuelve a decodificar la imagen si el wallpaper cambió).
    wallpaper_rx: Option<std::sync::mpsc::Receiver<[u8; 3]>>,
    /// Buffers del formulario de usuario/contraseña (`ui::login`). Viven
    /// acá sueltos, igual que `compose_text`/`friends_search`, en vez de
    /// adentro de `AuthStatus`, para no mezclar "en qué paso del login
    /// estamos" con "qué hay tipeado ahora mismo".
    pub login_email: String,
    pub login_password: String,
    pub login_mfa_code: String,
    /// Si ya se pidió el SMS de 2FA en este desafío (para no dejar
    /// mandarlo de nuevo con cada repintado, y para cambiar el texto del
    /// botón a "Reenviar").
    pub login_sms_sent: bool,
    /// Error del intento anterior de login/2FA por contraseña, para
    /// mostrarlo arriba del formulario en vez de mandar a la pantalla
    /// genérica de `AuthStatus::Failed` (que perdería lo ya tipeado).
    pub login_error: Option<String>,
    /// Acciones que va haciendo la conexión mientras se muestra "Conectando
    /// con Discord…" (las últimas, la más reciente al final). Se vacía al
    /// arrancar cada conexión y al volver al login.
    pub connection_steps: Vec<String>,
    /// Desafío de 2FA que mandó Discord (`AuthStatus::PasswordMfaRequired`
    /// necesita esto para saber qué métodos ofrecer y con qué ticket
    /// contestar).
    pub pending_mfa: Option<crate::discord::password_auth::MfaChallenge>,
    /// Captchas (hCaptcha) que Discord pidió en cualquier request —login,
    /// mensajes, DMs...— y la ventana donde la persona los resuelve. Ver
    /// `discord::captcha`.
    pub captcha: crate::discord::captcha::CaptchaController,
    /// Participantes por canal/DM y ajustes de audio persistidos; ver
    /// `discord::voice::state::VoiceCache`. Se actualiza desde
    /// `apply_voice_wire_event` (definido ahí mismo, adentro de
    /// `discord::voice`, porque necesita tocar campos `pub(in
    /// crate::discord)`).
    pub voice: VoiceCache,
    /// Última lista de micrófonos/salidas de audio que devolvió el
    /// sistema, para el picker de `ui::settings` (pestaña "Voz y audio").
    /// A diferencia de `voice.audio_sources` (qué id está elegido, eso sí
    /// persistido), esto es solo un cache de inventario en RAM: se refresca
    /// con `App::refresh_voice_audio_sources`, no con cada frame (`cpal`
    /// puede tardar, sobre todo con ALSA/PulseAudio reiniciando).
    pub voice_audio_source_options: crate::discord::voice::VoiceAudioSourceOptions,
    /// Pestaña activa del panel de ajustes (`ui::settings`). Vive acá y no
    /// adentro del propio `ui::settings` porque, como `settings_open`, hace
    /// falta que sobreviva entre frames sin que el módulo de UI necesite su
    /// propio estado con lifetime raro.
    pub settings_tab: crate::ui::settings::SettingsTab,
    /// Extremo de envío hacia el hilo del voice runtime (el que abre el
    /// socket de voz de verdad y captura/reproduce audio). `None` hasta
    /// `ensure_voice_runtime_started`, que se llama sola la primera vez que
    /// hace falta (unirse a un canal, o al recibir el primer `Ready`).
    voice_runtime_tx: Option<tokio::sync::mpsc::UnboundedSender<VoiceRuntimeEvent>>,
    /// A qué canal de voz queremos estar conectados y con qué mute/deaf/mic,
    /// tal cual se lo pedimos por última vez al runtime y al Gateway. Es la
    /// fuente de verdad para la UI (botón "Silenciar", barra de llamada)
    /// mientras Discord todavía no confirmó el cambio con su propio
    /// `VOICE_STATE_UPDATE` — a diferencia de `voice.states`, que refleja
    /// lo último que SÍ confirmó Discord. `None` = no pedimos estar en voz.
    pub voice_target: Option<CurrentVoiceConnectionState>,
    /// Último estado de la conexión de voz que publicó el runtime
    /// (`AppEvent::VoiceConnectionStatusChanged`), para la barra de
    /// llamada: "Conectando...", "Voz conectada", un error, etc.
    pub voice_connection_status: Option<VoiceConnectionStatus>,
    pub voice_connection_message: Option<String>,
    /// Preferencia de mute/deafen del propio usuario, independiente de
    /// estar en una llamada o no — igual que en el cliente real, los
    /// botones de mic/audífonos de la barra de usuario (`ui::friends_panel
    /// ::user_bar`) valen tanto para "pre-silenciarte" antes de unirte a
    /// una llamada como para silenciarte en una que ya está en curso. Se
    /// usa como valor inicial en `requested_voice_state` y se mantiene en
    /// sync con `voice_target.self_mute`/`self_deaf` mientras haya una
    /// llamada activa (ver `toggle_self_mute`/`toggle_self_deafen`).
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Streams pedidos (Go Live) a los que todavía les falta algún dato del
    /// Gateway, por `stream_key`.
    stream_sessions: std::collections::HashMap<String, PendingStreamSession>,
    /// Stream que se está viendo (`watch_stream`), si hay uno.
    pub watching_stream: Option<WatchedStream>,
    /// Contexto de egui, guardado para que el hilo de video pueda pedir un
    /// repintado cada vez que hay un frame nuevo (los eventos de Discord se
    /// procesan sin acceso al contexto).
    egui_ctx: Option<egui::Context>,
}

impl Default for App {
    fn default() -> Self {
        // Cuentas guardadas (migra la sesión única del formato viejo). Si la
        // última cuenta usada sigue con sesión se conecta sola; si no, se
        // muestra el selector de cuentas en la pantalla de login.
        let accounts = crate::lib::accounts::AccountStore::load();
        let startup_token = accounts.startup_token();
        let theme_mode = match web_local_storage_api::get_item("theme_mode") {
            Ok(Some(json)) => serde_json::from_str::<ThemeMode>(&json).unwrap_or(ThemeMode::Dark),
            _ => ThemeMode::Dark,
        };
        // Modo de imágenes (diagnóstico de RAM), ver `ui::anim::ImageMode`.
        if let Ok(Some(raw)) = web_local_storage_api::get_item("image_mode") {
            if let Ok(n) = raw.trim().parse::<u8>() {
                crate::ui::anim::set_image_mode(crate::ui::anim::ImageMode::from_u8(n));
            }
        }
        let notification_side = match web_local_storage_api::get_item("notification_side") {
            Ok(Some(raw)) => NotificationSide::from_str(&raw).unwrap_or_default(),
            _ => NotificationSide::default(),
        };
        let custom_themes = match web_local_storage_api::get_item("custom_themes") {
            Ok(Some(json)) => serde_json::from_str::<Vec<ThemeDef>>(&json).unwrap_or_default(),
            _ => Vec::new(),
        };
        // Sensibilidad/volumen/supresión de ruido y el id de dispositivo
        // elegido (ver `ui::settings`, pestaña "Voz y audio") — mismo
        // patrón que `custom_themes` arriba, dos claves separadas porque
        // viajan al runtime de voz por canales distintos (ver
        // `App::sync_voice_audio_settings_to_target` vs.
        // `App::publish_voice_audio_sources`).
        let voice_audio_settings = match web_local_storage_api::get_item("voice_audio_settings") {
            Ok(Some(json)) => serde_json::from_str::<VoiceAudioSettings>(&json).unwrap_or_default(),
            _ => VoiceAudioSettings::default(),
        };
        let voice_audio_sources = match web_local_storage_api::get_item("voice_audio_sources") {
            Ok(Some(json)) => serde_json::from_str::<VoiceAudioSources>(&json).unwrap_or_default(),
            _ => VoiceAudioSources::default(),
        };

        let new_call_ui = matches!(
            web_local_storage_api::get_item("new_call_ui"),
            Ok(Some(ref raw)) if raw == "1"
        );
        // Barras flotantes de la interfaz nueva.
        crate::theme::set_modern(new_call_ui);
        // Tamaño de la interfaz (zoom). Sin valor guardado arranca más chica
        // que el 100 %: el rediseño se veía demasiado grande.
        let ui_scale = match web_local_storage_api::get_item("ui_scale") {
            Ok(Some(raw)) => raw
                .trim()
                .parse::<f32>()
                .map(|v| v.clamp(UI_SCALE_MIN, UI_SCALE_MAX))
                .unwrap_or(UI_SCALE_DEFAULT),
            _ => UI_SCALE_DEFAULT,
        };

        let mut app = Self {
            screen: Screen::Login,
            auth: AuthStatus::SignedOut,
            me: None,
            accounts,
            discord_token: None,
            discord_settings: None,
            participant_playback: std::collections::HashMap::new(),
            participant_audio_dirty: std::collections::HashSet::new(),
            participant_audio_dirty_at: None,
            stream_playback: std::collections::HashMap::new(),
            stream_audio_dirty: std::collections::HashSet::new(),
            relationship_kinds: std::collections::HashMap::new(),
            pending_requests: Vec::new(),
            add_friend_input: String::new(),
            add_friend_busy: false,
            add_friend_status: None,
            ignored_users: std::collections::HashSet::new(),
            video_disabled_users: std::collections::HashSet::new(),
            pending_call_user: None,
            pending_note: None,
            menu_ctx_rev: 0,
            menu_ctx_published: None,
            open_guild_folders: std::collections::HashSet::new(),
            event_rx: None,
            event_tx: None,
            gateway_commands: None,
            member_subscription: None,
            pending_member_subscription: None,
            friends: demo_friends(),
            dms: Vec::new(),
            activity: demo_activity(),
            servers: demo_servers(),
            palette: Palette::dark(),
            current_channel: (0, 0),
            friends_search: String::new(),
            friends_tab: 0,
            compose_text: String::new(),
            reply_target: None,
            pending_scroll_anchor: None,
            pending_jump: None,
            inbox_open: false,
            newer_retry_after: None,
            thread_panel: None,
            last_history_trim: 0.0,
            component_modal: None,
            pending_buttons: Vec::new(),
            gateway_disconnected: false,
            gateway_session_id: String::new(),
            slash_catalogs: std::collections::HashMap::new(),
            slash_pending: std::collections::HashSet::new(),
            top_search: String::new(),
            toasts: Vec::new(),
            window_focused: true,
            unread_mentions: std::collections::HashMap::new(),
            guild_mentions: std::collections::HashMap::new(),
            acked_messages: std::collections::HashMap::new(),
            ack_sent_at: std::collections::HashMap::new(),
            pending_mentions: std::collections::HashMap::new(),
            presences: std::collections::HashMap::new(),
            user_activities: std::collections::HashMap::new(),
            custom_statuses: std::collections::HashMap::new(),
            own_activities: Vec::new(),
            affinity_users: Default::default(),
            affinity_guilds: Default::default(),
            home_scroll_req: [None; 2],
            in_app_notifications: Vec::new(),
            notification_side,
            mute_rules: crate::lib::notifications::MuteRules::default(),
            shown_title_count: None,
            modal: None,
            dialogs: std::collections::VecDeque::new(),
            pending_resume_token: None,
            profile_popup: None,
            role_popup: None,
            dm_profile: None,
            profile_full: None,
            theme_mode: theme_mode.clone(),
            custom_themes,
            settings_open: false,
            theme_editor: None,
            backdrop: Backdrop::default(),
            backdrop_rt: BackdropRuntime::default(),
            wallpaper_seed: None,
            wallpaper_rx: None,
            login_email: String::new(),
            login_password: String::new(),
            login_mfa_code: String::new(),
            login_sms_sent: false,
            login_error: None,
            connection_steps: Vec::new(),
            pending_mfa: None,
            captcha: Default::default(),
            voice: VoiceCache {
                audio: voice_audio_settings,
                audio_sources: voice_audio_sources,
                ..VoiceCache::default()
            },
            voice_audio_source_options: VoiceAudioSourceOptions::default(),
            settings_tab: SettingsTab::default(),
            voice_runtime_tx: None,
            voice_target: None,
            voice_connection_status: None,
            voice_connection_message: None,
            self_mute: false,
            self_deaf: false,
            stream_sessions: std::collections::HashMap::new(),
            watching_stream: None,
            egui_ctx: None,
            new_call_ui,
            ui_scale,
            ui_scale_applied: f32::NAN,
        };
        // Con el modo ya cargado, calculamos la paleta que corresponde
        // (y, si es `Wallpaper`, arrancamos el hilo que lo vigila) antes
        // del primer frame.
        app.apply_theme_mode();

        // Si ya había una sesión guardada, nos saltamos el QR y vamos
        // directo al Gateway con ese token.
        let security_dialog = crate::ui::security_warning::detect();
        if let Some(token) = startup_token {
            if security_dialog.is_some() {
                // Modo inseguro (mock / TLS sin verificar): NO se toca la
                // red con el token hasta que la persona acepte el aviso.
                app.pending_resume_token = Some(token);
            } else {
                app.resume_saved_session(token);
            }
        }
        // El aviso de seguridad va primero en la cola; después, las novedades
        // si es la primera vez que se abre esta versión.
        if let Some(dialog) = security_dialog {
            crate::ui::security_warning::play_alert();
            app.dialogs.push_back(dialog);
        }
        crate::ui::changelog::queue_if_new(&mut app);

        app
    }
}

impl App {
    /// Arranca el login por QR: abre un canal nuevo hacia un hilo de fondo
    /// que hace todo el handshake de `remote_auth` y, si sale bien, sigue
    /// derecho a la conexión del Gateway.
    pub fn start_sign_in(&mut self) {
        self.login_error = None;
        self.auth = AuthStatus::Starting;
        let (tx, rx) = std::sync::mpsc::channel();
        self.event_tx = Some(tx.clone());
        self.event_rx = Some(rx);
        crate::discord::spawn_login_flow(tx);
    }

    /// Vuelve a la pantalla de login. El hilo de fondo (si había uno) no
    /// se cancela activamente: al soltar `event_rx`/`event_tx` sus envíos
    /// simplemente van a un canal sin receptor y se ignoran, así que
    /// termina solo en el próximo paso que intente mandar algo.
    pub fn cancel_sign_in(&mut self) {
        self.captcha.cancel_all();
        self.auth = AuthStatus::SignedOut;
        self.event_rx = None;
        self.event_tx = None;
    }

    /// "Probar de nuevo" de la pantalla de error. Si todavía tenemos un
    /// token (el error fue de red/Discord, no de la sesión) se reconecta
    /// con ESE token — no tiene sentido pedir un QR si la cuenta ya está
    /// autenticada. Solo sin token guardado se arranca el login por QR.
    pub fn retry_sign_in(&mut self) {
        match self.discord_token.clone() {
            Some(token) => self.reconnect_with_token(token),
            None => self.start_sign_in(),
        }
    }

    /// Conecta el Gateway con el token de la sesión guardada, saltándose el
    /// QR. Se llama al arrancar o, si había un aviso de seguridad pendiente,
    /// cuando la persona lo acepta (ver `ui::security_warning`).
    pub fn resume_saved_session(&mut self, token: String) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.discord_token = Some(token.clone());
        self.event_tx = Some(tx.clone());
        self.event_rx = Some(rx);
        self.auth = AuthStatus::Connecting;
        self.connection_steps.clear();
        crate::discord::spawn_gateway_with_token(token, tx);
    }

    /// Reconecta con un token que ya tenemos, saltándose el QR (mismo
    /// camino que el arranque con sesión guardada, ver `App::default`).
    fn reconnect_with_token(&mut self, token: String) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.event_tx = Some(tx.clone());
        self.event_rx = Some(rx);
        self.login_error = None;
        self.connection_steps.clear();
        self.auth = AuthStatus::Connecting;
        crate::discord::spawn_gateway_with_token(token, tx);
    }

    /// Anota la acción que está haciendo la conexión (la ve la pantalla de
    /// "Conectando…"). Ignora repeticiones y se queda con las últimas.
    fn push_connection_step(&mut self, text: String) {
        if self.connection_steps.last() == Some(&text) {
            return;
        }
        self.connection_steps.push(text);
        let excess = self.connection_steps.len().saturating_sub(5);
        self.connection_steps.drain(..excess);
    }

    /// ¿Hay un token guardado con el que "Probar de nuevo" reconectaría?
    pub fn has_saved_session(&self) -> bool {
        self.discord_token.is_some()
    }

    /// Vuelve al selector de cuentas ("Elegir otra cuenta", "Agregar cuenta").
    /// NO borra ningún token: la cuenta que estaba abierta sigue guardada y se
    /// puede volver a elegir. Corta la conexión actual y limpia sus datos.
    pub fn show_account_picker(&mut self) {
        self.settings_open = false;
        self.reset_session_data();
        self.login_error = None;
        self.auth = AuthStatus::SignedOut;
    }

    /// Conecta con el token de una cuenta guardada. Con el aviso de seguridad
    /// aplicable (modo mock / TLS sin verificar) el token no sale hasta que la
    /// persona lo acepte, igual que al arrancar.
    fn connect_saved_token(&mut self, token: String) {
        if let Some(dialog) = crate::ui::security_warning::detect() {
            self.pending_resume_token = Some(token);
            // Solo suena si el diálogo realmente se va a mostrar (si ya hay
            // uno abierto, `open_dialog` lo ignora).
            if !self.dialogs.iter().any(|d| d.id == dialog.id) {
                crate::ui::security_warning::play_alert();
            }
            self.open_dialog(dialog);
        } else {
            self.resume_saved_session(token);
        }
    }

    /// Elegir una cuenta del selector. Con token → se conecta; sin token (la
    /// sesión se cerró o expiró) → se queda en el login avisando que hay que
    /// iniciar sesión de nuevo (QR o contraseña).
    pub fn sign_in_saved_account(&mut self, user_id: &str) {
        let Some(account) = self.accounts.get(user_id).cloned() else {
            return;
        };
        self.login_error = None;
        match account.token {
            Some(token) => self.connect_saved_token(token),
            None => {
                self.login_error = Some(format!(
                    "La sesión de {} está cerrada o expiró. Iniciá sesión de nuevo para usarla.",
                    account.label()
                ));
                self.auth = AuthStatus::SignedOut;
            }
        }
    }

    /// Cambiar de cuenta estando adentro (Ajustes → Cuenta): se cierra la
    /// conexión actual SIN cerrar su sesión y se entra con la otra.
    pub fn switch_account(&mut self, user_id: &str) {
        self.settings_open = false;
        if self.me.as_ref().is_some_and(|me| me.id == user_id) {
            return;
        }
        self.reset_session_data();
        self.auth = AuthStatus::SignedOut;
        self.sign_in_saved_account(user_id);
    }

    /// Pide confirmación para cerrar la sesión de la cuenta abierta.
    pub fn ask_log_out(&mut self) {
        use crate::ui::dialog::{Dialog, DialogKind};
        let name = self
            .me
            .as_ref()
            .map(|me| me.display_name().to_owned())
            .unwrap_or_else(|| "esta cuenta".to_owned());
        self.open_dialog(
            Dialog::confirm(
                "confirm_logout",
                DialogKind::Danger,
                "Cerrar sesión",
                format!(
                    "Se cierra la sesión de {name} en este equipo y su token deja de valer \
                     en Discord. La cuenta queda en la lista: para volver a usarla hay que \
                     iniciar sesión otra vez (QR o contraseña)."
                ),
                "Cerrar sesión",
            )
            .on_result(|app, _ctx, result| {
                if result.is("confirm") {
                    app.log_out_current();
                }
            }),
        );
    }

    /// Cierra la sesión de la cuenta abierta: invalida el token en Discord, lo
    /// borra de este equipo (la cuenta queda en la lista, marcada como "sesión
    /// cerrada") y vuelve al selector de cuentas.
    pub fn log_out_current(&mut self) {
        if let Some(token) = self.discord_token.clone() {
            self.accounts.sign_out_token(&token);
            self.accounts.save();
            crate::discord::spawn_logout(token);
        }
        self.settings_open = false;
        self.reset_session_data();
        self.login_error = None;
        self.auth = AuthStatus::SignedOut;
    }

    /// Pide confirmación para quitar una cuenta de la lista.
    pub fn ask_remove_account(&mut self, user_id: &str) {
        use crate::ui::dialog::{Dialog, DialogKind};
        let Some(account) = self.accounts.get(user_id).cloned() else {
            return;
        };
        let id = account.user_id.clone();
        self.open_dialog(
            Dialog::confirm(
                "confirm_remove_account",
                DialogKind::Danger,
                "Quitar cuenta",
                format!(
                    "{} se quita de este equipo y, si tenía la sesión abierta, se invalida su \
                     token en Discord. Para usarla de nuevo hay que agregarla otra vez.",
                    account.label()
                ),
                "Quitar cuenta",
            )
            .on_result(move |app, _ctx, result| {
                if result.is("confirm") {
                    app.remove_account(&id);
                }
            }),
        );
    }

    /// Saca una cuenta de la lista (e invalida su token si tenía uno). Si era
    /// la que estaba abierta, también se cierra la conexión.
    pub fn remove_account(&mut self, user_id: &str) {
        let is_current = self.me.as_ref().is_some_and(|me| me.id == user_id);
        if let Some(token) = self.accounts.remove(user_id) {
            crate::discord::spawn_logout(token);
        }
        self.accounts.save();
        if is_current {
            self.settings_open = false;
            self.reset_session_data();
            self.login_error = None;
            self.auth = AuthStatus::SignedOut;
        }
    }

    /// Guarda el token de un login recién hecho en la lista de cuentas.
    fn remember_login(&mut self, token: &str) {
        self.accounts.upsert_token(token);
        self.accounts.save();
    }

    /// Corta la conexión y deja `App` sin ningún dato de la cuenta que estaba
    /// abierta (amigos, servers, DMs, no leídos, popups...), para que al
    /// entrar con otra cuenta no se mezcle nada. Las preferencias de la app
    /// (tema, audio, notificaciones) se conservan.
    fn reset_session_data(&mut self) {
        // Favoritos/frecency son de la cuenta: lo que quedó sin mandar sale
        // ahora con el token de ESA cuenta (en un hilo, sin esperar) y recién
        // después se olvida todo, así no se mezcla con la próxima cuenta.
        if let Some(token) = self.discord_token.as_deref() {
            crate::discord::frecency::tick(token, true);
        }
        // Idem para el volumen/silencio por persona: sale con el token de ESTA
        // cuenta antes de olvidarlo todo.
        self.flush_participant_audio(true);
        self.participant_playback.clear();
        self.participant_audio_dirty.clear();
        self.participant_audio_dirty_at = None;
        self.stream_playback.clear();
        self.stream_audio_dirty.clear();
        crate::discord::frecency::reset();
        self.drop_connection();
        self.discord_token = None;
        self.me = None;
        self.discord_settings = None;
        self.open_guild_folders.clear();
        self.friends.clear();
        self.dms.clear();
        self.activity.clear();
        self.servers.clear();
        self.current_channel = (0, 0);
        self.friends_search.clear();
        self.friends_tab = 0;
        self.compose_text.clear();
        self.reply_target = None;
        self.pending_scroll_anchor = None;
        self.pending_jump = None;
        self.inbox_open = false;
        self.thread_panel = None;
        self.component_modal = None;
        self.top_search.clear();
        self.unread_mentions.clear();
        self.guild_mentions.clear();
        self.acked_messages.clear();
        self.ack_sent_at.clear();
        self.pending_mentions.clear();
        self.presences.clear();
        self.user_activities.clear();
        self.custom_statuses.clear();
        self.own_activities.clear();
        self.affinity_users = Default::default();
        self.affinity_guilds = Default::default();
        self.home_scroll_req = [None; 2];
        self.in_app_notifications.clear();
        self.mute_rules = crate::lib::notifications::MuteRules::default();
        self.shown_title_count = None;
        self.modal = None;
        self.profile_popup = None;
        self.role_popup = None;
        self.dm_profile = None;
        self.profile_full = None;
        self.watching_stream = None;
        self.stream_sessions.clear();
        self.voice_target = None;
        // El estado de voz es de la cuenta, pero el audio elegido es una
        // preferencia de la persona: se conserva.
        let audio = std::mem::take(&mut self.voice.audio);
        let audio_sources = std::mem::take(&mut self.voice.audio_sources);
        self.voice = VoiceCache { audio, audio_sources, ..VoiceCache::default() };
    }

    /// Corta lo que quede de la conexión actual y vuelve a la pantalla de
    /// login. No toca el token guardado (eso lo decide quien llama).
    fn drop_connection(&mut self) {
        self.captcha.cancel_all();
        // Primero se sale de la voz (corta también un stream que se esté
        // viendo) mientras todavía existen los canales; recién después se
        // sueltan.
        self.leave_voice();
        self.voice_runtime_tx = None;
        self.voice_connection_status = None;
        self.voice_connection_message = None;
        // Si el Gateway sigue vivo (p. ej. un 401 de REST con el socket
        // todavía abierto) hay que pedirle que cierre, si no seguiría
        // conectado en segundo plano sin nadie escuchándolo.
        if let Some(commands) = self.gateway_commands.take() {
            let _ = commands.send(crate::discord::gateway::GatewayCommand::Shutdown);
        }
        self.member_subscription = None;
        self.pending_member_subscription = None;
        self.gateway_session_id.clear();
        self.slash_catalogs.clear();
        self.slash_pending.clear();
        self.connection_steps.clear();
        self.event_rx = None;
        self.event_tx = None;
        // Sin conexión que reintentar (vuelve al login): nada que avisar.
        self.gateway_disconnected = false;
        self.screen = Screen::Login;
    }

    /// Discord ya no acepta el token: se borra el guardado y se vuelve al
    /// login con un aviso. No se lanza el QR solo; la persona elige cómo
    /// volver a entrar.
    fn invalidate_session(&mut self, message: String) {
        log::warn!("La sesión ya no es válida; la cuenta queda con la sesión cerrada");
        let name = self.me.as_ref().map(|me| me.display_name().to_owned());
        // La cuenta NO se borra de la lista: queda marcada para que el
        // selector avise que hay que volver a iniciar sesión.
        if let Some(token) = self.discord_token.clone() {
            self.accounts.sign_out_token(&token);
            self.accounts.save();
        }
        self.reset_session_data();
        self.login_error = Some(match name {
            Some(name) => format!("La sesión de {name} ya no es válida. Iniciá sesión de nuevo."),
            None => message,
        });
        self.auth = AuthStatus::SignedOut;
    }

    /// Se perdió el Gateway por algo que NO es la sesión (red caída tras
    /// agotar los reintentos, Discord rechazó la conexión...). Vuelve al
    /// login, pero conserva el token: "Probar de nuevo" reconecta con él.
    fn connection_lost(&mut self, message: String) {
        log::warn!("Se perdió la conexión con Discord: {message}");
        self.drop_connection();
        self.auth = AuthStatus::Failed(message);
    }

    /// Pasa de la pantalla de QR a la de usuario/contraseña (link "Usar
    /// usuario y contraseña" en `ui::login`).
    pub fn switch_to_password_form(&mut self) {
        self.login_error = None;
        self.auth = AuthStatus::PasswordForm;
    }

    /// Vuelve de la pantalla de contraseña (o de 2FA) a la de QR, y
    /// limpia lo tipeado — igual que `cancel_sign_in`, no cancela
    /// activamente un pedido en vuelo, solo deja de escuchar la
    /// respuesta.
    pub fn switch_to_qr_form(&mut self) {
        self.captcha.cancel_all();
        self.login_email.clear();
        self.login_password.clear();
        self.login_mfa_code.clear();
        self.login_sms_sent = false;
        self.login_error = None;
        self.pending_mfa = None;
        self.auth = AuthStatus::SignedOut;
    }

    /// Manda el formulario de usuario/contraseña. No valida nada de
    /// forma local (ni longitud ni formato de mail/teléfono): que lo
    /// rechace Discord y mostrar su mensaje es más confiable que
    /// adivinar sus reglas acá.
    pub fn submit_password_login(&mut self) {
        if self.login_email.trim().is_empty() || self.login_password.is_empty() {
            self.login_error = Some("Completá usuario y contraseña".to_owned());
            return;
        }
        self.login_error = None;
        self.auth = AuthStatus::PasswordSubmitting;
        let (tx, rx) = std::sync::mpsc::channel();
        self.event_tx = Some(tx.clone());
        self.event_rx = Some(rx);
        crate::discord::password_auth::spawn(self.login_email.clone(), self.login_password.clone(), tx);
    }

    /// Manda el código de 2FA que se acaba de tipear (`App::login_mfa_code`)
    /// para el método `method` (TOTP salvo que el usuario haya pedido
    /// SMS).
    pub fn submit_mfa_code(&mut self, method: crate::discord::password_auth::MfaMethod) {
        let Some(challenge) = self.pending_mfa.clone() else { return };
        if self.login_mfa_code.trim().is_empty() {
            self.login_error = Some("Escribí el código de verificación".to_owned());
            return;
        }
        self.login_error = None;
        self.auth = AuthStatus::PasswordMfaSubmitting;
        let Some(tx) = self.event_tx.clone() else { return };
        crate::discord::password_auth::spawn_mfa_verify(
            method,
            self.login_mfa_code.clone(),
            challenge.ticket,
            challenge.login_instance_id,
            tx,
        );
    }

    /// Pide que Discord mande el código de 2FA por SMS (botón "Enviar
    /// SMS"/"Reenviar SMS" de `ui::login`).
    pub fn request_mfa_sms(&mut self) {
        let Some(challenge) = self.pending_mfa.clone() else { return };
        let Some(tx) = self.event_tx.clone() else { return };
        crate::discord::password_auth::spawn_mfa_sms(challenge.ticket, tx);
    }

    /// Vuelve a la lista de amigos (ícono de mensajes directos en el rail,
    /// o botón "Amigos" en el panel de navegación).
    pub fn go_home(&mut self) {
        self.screen = Screen::Home;
        self.compose_text.clear();
        self.reply_target = None;
    }

    /// Pide por REST los datos de un usuario que apareció en un estado de
    /// voz sin `member`/`user` embebido (ver `VoiceState::known`), para
    /// dejar de mostrarlo como "Usuario desconocido". Sin sesión activa
    /// (demo) simplemente no hace nada.
    fn fetch_unknown_voice_user(&self, user_id: &str) {
        if user_id.is_empty() {
            return;
        }
        if let (Some(token), Some(tx)) = (&self.discord_token, &self.event_tx) {
            crate::discord::spawn_fetch_user(token.clone(), user_id.to_string(), tx.clone());
        }
    }

    /// Abre la conversación directa con `friends[index]`. Si todavía no
    /// se pidió el historial de este DM, lo pide por REST en el fondo:
    /// - si ya sabemos el `dm_channel_id` (vino de `self.dms`, la lista de
    ///   conversaciones recientes), pedimos directo los mensajes de ese
    ///   canal (`AppEvent::ChannelMessages`).
    /// - si no (amigo de `relationships` con el que nunca se abrió un DM
    ///   en esta sesión), primero hay que abrir/crear el canal por
    ///   `user_id` (`AppEvent::DmOpened`, que además ya trae los
    ///   mensajes).
    pub fn open_dm(&mut self, index: usize) {
        if index >= self.friends.len() {
            return;
        }
        self.screen = Screen::Dm(index);
        self.compose_text.clear();
        self.reply_target = None;
        self.pending_jump = None;

        let friend = &mut self.friends[index];
        // La ventana cargada quedó en el medio del historial (se saltó a un
        // mensaje con `around`): al abrir el DM a secas se vuelve al final.
        if friend.has_newer {
            friend.messages.clear();
            friend.loaded = false;
            friend.has_newer = false;
            friend.loading_newer = false;
            friend.has_more = true;
        }
        if friend.loaded || friend.user_id.is_empty() {
            return;
        }
        friend.loaded = true; // optimista, para no repetir el pedido
        friend.loading = true;

        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        match self.friends[index].dm_channel_id.clone() {
            Some(channel_id) => crate::discord::spawn_fetch_channel_messages(token, channel_id, tx),
            None => crate::discord::spawn_fetch_dm(token, self.friends[index].user_id.clone(), tx),
        }
    }

    /// Abre una conversación de la lista de DMs recientes (`self.dms[i]`,
    /// lo que se ve y se clickea en el panel de la izquierda). `Screen::Dm`
    /// indexa `self.friends`, no `self.dms` — son dos listas distintas
    /// (podés tener un DM abierto con alguien que no es "amigo" mutuo) —
    /// así que acá buscamos (o creamos) la entrada de `friends` que
    /// corresponde a ese DM por `user_id`, y recién ahí delegamos a
    /// `open_dm`. Sin este puente, clickear un DM de la lista terminaba
    /// abriendo el amigo que casualmente tuviera ese mismo índice en la
    /// otra lista (o ninguno, si `self.dms` tenía más filas que
    /// `self.friends`).
    pub fn open_dm_from_channel(&mut self, dm_index: usize) {
        let Some(dm) = self.dms.get(dm_index) else { return };
        let Some(user_id) = dm.recipients.first().map(|r| r.id.clone()) else { return };
        let channel_id = dm.id.clone();

        let existing = self.friends.iter().position(|f| f.user_id == user_id);
        let index = match existing {
            Some(i) => {
                // Ya lo teníamos de la lista de amigos; nos aseguramos de
                // que tenga el id de canal (por si el amigo se cargó antes
                // que la lista de DMs y todavía no lo sabía).
                if self.friends[i].dm_channel_id.is_none() {
                    self.friends[i].dm_channel_id = Some(channel_id);
                }
                i
            }
            None => {
                let Some(mut friend) = Friend::from_dm_channel(dm) else { return };
                if let Some((status, subtitle)) = self.presences.get(&friend.user_id) {
                    friend.status = *status;
                    friend.subtitle = subtitle.clone();
                }
                self.friends.push(friend);
                self.friends.len() - 1
            }
        };
        self.open_dm(index);
    }

    /// Token + id real de canal/DM (Discord) + id del server (`None` en un
    /// DM) para el canal o DM actualmente abierto — si existen, `ui::chat`
    /// los usa para además mandar el mensaje de verdad por REST, no solo
    /// mostrarlo local. El server hace falta para el `Referer` del pedido.
    pub fn current_send_target(&self) -> Option<(String, String, Option<String>)> {
        let token = self.discord_token.clone()?;
        match self.screen {
            Screen::Dm(index) => {
                let channel_id = self.friends.get(index)?.dm_channel_id.clone()?;
                Some((token, channel_id, None))
            }
            Screen::Server(index) => {
                let (cat, chan) = self.current_channel;
                let server = self.servers.get(index)?;
                let channel_id = server.channel(cat, chan)?.channel_id.clone()?;
                let guild_id = (!server.guild_id.is_empty()).then(|| server.guild_id.clone());
                Some((token, channel_id, guild_id))
            }
            _ => None,
        }
    }

    /// Abre un servidor en su primer canal. Si es un server real y
    /// todavía no tiene canales cargados, los pide por REST; llegan como
    /// `AppEvent::GuildChannels`.
    pub fn open_server(&mut self, index: usize) {
        let Some(server) = self.servers.get(index) else { return };
        self.current_channel = server.first_channel();
        self.screen = Screen::Server(index);
        self.compose_text.clear();
        self.reply_target = None;
        self.thread_panel = None;

        if !server.guild_id.is_empty() && server.categories.is_empty() {
            if let (Some(token), Some(tx)) = (&self.discord_token, &self.event_tx) {
                crate::discord::spawn_fetch_guild_channels(
                    token.clone(),
                    server.guild_id.clone(),
                    tx.clone(),
                );
            }
        }
        self.subscribe_member_list();
        // Si el canal por defecto es un foro que ya tenía sus canales
        // cargados, hay que pedir sus posts (si no, `open_channel` no corre).
        self.load_forum_posts_if_needed();
    }

    /// Si el canal abierto en un server quedó oculto (llegaron los roles y
    /// resultó que ya no se puede ver, o te sacaron un rol), pasa al primer
    /// canal visible en vez de dejar abierto uno que no se muestra.
    fn fix_hidden_current_channel(&mut self) {
        let Screen::Server(index) = self.screen else { return };
        let Some(server) = self.servers.get(index) else { return };
        let (cat, chan) = self.current_channel;
        if server.channel_visible(cat, chan) {
            return;
        }
        let first = server.first_channel();
        if first != (cat, chan) {
            self.open_channel(first.0, first.1);
        }
    }

    /// (guild, canal) de texto abierto ahora mismo, si es un server real.
    fn current_member_list_key(&self) -> Option<(String, String)> {
        let Screen::Server(index) = self.screen else { return None };
        let server = self.servers.get(index)?;
        if server.guild_id.is_empty() {
            return None;
        }
        let (category, channel) = self.current_channel;
        let channel = server.channel(category, channel)?;
        if channel.is_voice {
            return None;
        }
        // Un post de foro no tiene lista de miembros propia: se usa la de
        // su foro.
        let id = if channel.is_thread {
            channel.parent_id.clone()?
        } else {
            channel.channel_id.clone()?
        };
        Some((server.guild_id.clone(), id))
    }

    /// Decide si hay que pedirle al Gateway la lista de miembros (con roles
    /// y presencias) del canal abierto. Discord no la manda sola a las
    /// cuentas de usuario: hay que suscribirse (opcode 37) y después llegan
    /// los `GUILD_MEMBER_LIST_UPDATE`.
    ///
    /// Se evita mandar pedidos de más:
    /// * si es el mismo canal que ya se pidió, no se manda nada;
    /// * si ya se sabe que este canal comparte la lista que se está viendo
    ///   (`Server::channel_shares_active_list`), tampoco: la suscripción
    ///   vigente ya la mantiene al día;
    /// * el resto queda "pendiente" unos milisegundos
    ///   (`flush_member_subscription`), así que cambiar de canal varias
    ///   veces seguidas manda un solo pedido, el del canal donde se termina.
    pub fn subscribe_member_list(&mut self) {
        let Some(key) = self.current_member_list_key() else { return };
        if self.member_subscription.as_ref() == Some(&key) {
            self.pending_member_subscription = None;
            return;
        }
        // Solo vale si en ESTA conexión ya hay una suscripción viva: en una
        // conexión nueva no queda ninguna, aunque el server recuerde listas.
        if self.member_subscription.is_some() {
            let shares = self
                .servers
                .iter()
                .find(|s| s.guild_id == key.0)
                .is_some_and(|s| s.channel_shares_active_list(&key.1));
            if shares {
                self.pending_member_subscription = None;
                return;
            }
        }
        self.pending_member_subscription = Some((key.0, key.1, Instant::now()));
    }

    /// Manda la suscripción pendiente cuando ya pasó el debounce. Se llama
    /// una vez por frame desde `poll_discord_events`.
    fn flush_member_subscription(&mut self, ctx: &egui::Context) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
        let elapsed = match &self.pending_member_subscription {
            Some((_, _, since)) => since.elapsed(),
            None => return,
        };
        if elapsed < DEBOUNCE {
            // No hay eventos que despierten la UI si el usuario se queda
            // quieto: hay que pedir un frame para cuando venza el plazo.
            ctx.request_repaint_after(DEBOUNCE - elapsed);
            return;
        }
        let Some((guild_id, channel_id, _)) = self.pending_member_subscription.take() else {
            return;
        };
        // Si mientras esperaba el usuario ya se fue a otro lado, no se manda.
        if self.current_member_list_key().as_ref() != Some(&(guild_id.clone(), channel_id.clone())) {
            return;
        }
        let Some(commands) = self.gateway_commands.clone() else { return };
        let command = crate::discord::gateway::GatewayCommand::SubscribeMemberList {
            guild_id: guild_id.clone(),
            channel_id: channel_id.clone(),
        };
        if commands.send(command).is_ok() {
            // La respuesta (`SYNC`) le asigna su lista a este canal.
            if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                server.pending_list_channel = Some(channel_id.clone());
            }
            self.member_subscription = Some((guild_id, channel_id));
        }
    }

    /// Aplica un cambio de presencia: al amigo (si lo es) y, si viene de un
    /// server, al miembro dentro de la lista de ese server.
    fn apply_presence_event(&mut self, event: &crate::discord::models::PresenceEvent) {
        let Some(user_id) = event.user_id() else { return };
        let subtitle = event.subtitle();
        // Se guarda para todos (no solo amigos): la lista de DMs también
        // muestra a gente que no está en `friends`.
        self.presences.insert(
            user_id.to_string(),
            (crate::lib::data::Status::from_gateway(&event.status), subtitle.clone()),
        );
        // Actividades reales (sin el estado personalizado) para "Activo ahora".
        let activities: Vec<crate::discord::models::PresenceActivity> = event
            .activities
            .iter()
            .filter(|a| a.kind != 4 && !a.name.is_empty())
            .cloned()
            .collect();
        if activities.is_empty() || crate::lib::data::Status::from_gateway(&event.status) == crate::lib::data::Status::Offline {
            // Con `guild_id` la presencia es de un miembro de un server: no
            // borra lo que ya sabíamos por la de amigo si esta viene vacía.
            if event.guild_id.is_none() {
                self.user_activities.remove(user_id);
            }
        } else {
            self.user_activities.insert(user_id.to_string(), activities);
        }
        // Estado personalizado (actividad de tipo 4) para el globito del perfil.
        let custom = event
            .activities
            .iter()
            .find(|a| a.kind == 4)
            .and_then(|a| a.state.clone())
            .filter(|text| !text.trim().is_empty());
        match custom {
            Some(text) => {
                self.custom_statuses.insert(user_id.to_string(), text);
            }
            None => {
                if event.guild_id.is_none() {
                    self.custom_statuses.remove(user_id);
                }
            }
        }
        if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
            friend.apply_presence(&event.status, subtitle);
        }
        if let Some(guild_id) = event.guild_id.as_deref() {
            if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                server.apply_presence(user_id, &event.status, &event.activities);
            }
        }
    }

    /// Libera el historial de los chats que NO se están mirando.
    ///
    /// Cada canal/DM abierto deja su `Vec<ChatMessage>` (texto, embeds,
    /// adjuntos, reacciones, stickers...) en RAM para siempre: con varios
    /// canales visitados la memoria solo sube. Acá, cada pocos segundos, los
    /// chats inactivos se recortan a los últimos `KEEP_INACTIVE` mensajes y
    /// se marca `has_more` para que el scroll hacia arriba vuelva a pedir lo
    /// viejo por REST. El chat abierto (y el hilo del panel lateral) no se
    /// tocan, así no se pierde el punto de scroll.
    fn trim_inactive_history(&mut self, now: f64) {
        const EVERY: f64 = 10.0;
        const KEEP_INACTIVE: usize = 50;
        // Margen para no recortar por 2-3 mensajes nuevos cada vez.
        const SLACK: usize = 30;
        if now - self.last_history_trim < EVERY {
            return;
        }
        self.last_history_trim = now;

        let open_dm = match self.screen {
            Screen::Dm(i) => Some(i),
            _ => None,
        };
        let open_server = match self.screen {
            Screen::Server(i) => Some(i),
            _ => None,
        };
        let open_channel = self.current_channel;
        let thread_id = self.thread_panel.as_ref().map(|t| t.thread_id.clone());

        for (i, friend) in self.friends.iter_mut().enumerate() {
            if Some(i) == open_dm || friend.dm_channel_id.is_none() {
                continue; // (sin canal real = demo: no hay de dónde recargar)
            }
            if friend.messages.len() > KEEP_INACTIVE + SLACK {
                let cut = friend.messages.len() - KEEP_INACTIVE;
                friend.messages.drain(..cut);
                friend.messages.shrink_to_fit();
                friend.has_more = true;
            }
        }
        for (si, server) in self.servers.iter_mut().enumerate() {
            for (ci, category) in server.categories.iter_mut().enumerate() {
                for (hi, channel) in category.channels.iter_mut().enumerate() {
                    let is_open = Some(si) == open_server && open_channel == (ci, hi);
                    let is_thread = channel.channel_id.is_some()
                        && channel.channel_id == thread_id;
                    if is_open || is_thread || channel.channel_id.is_none() {
                        continue;
                    }
                    if channel.messages.len() > KEEP_INACTIVE + SLACK {
                        let cut = channel.messages.len() - KEEP_INACTIVE;
                        channel.messages.drain(..cut);
                        channel.messages.shrink_to_fit();
                        channel.has_more = true;
                    }
                }
            }
        }
    }

    /// Va al canal con ese id real de Discord dentro del server abierto (clic
    /// en una mención `#canal`). No hace nada si el canal no es de este server
    /// o si la cuenta no puede verlo.
    pub fn open_channel_by_id(&mut self, channel_id: &str) {
        let Screen::Server(index) = self.screen else { return };
        let Some(server) = self.servers.get(index) else { return };
        let Some((category, channel)) = server.channel_position_by_id(channel_id) else { return };
        if !server.channel(category, channel).is_some_and(|c| c.is_thread || c.access.can_view) {
            return;
        }
        self.open_channel(category, channel);
    }

    /// Cambia de canal dentro del servidor actualmente abierto. Si es un
    /// canal de texto real que todavía no cargó su historial, lo pide por
    /// REST (llega como `AppEvent::ChannelMessages`); los canales de voz
    /// no tienen mensajes que pedir (`Channel::from_discord` ya los marca
    /// `loaded` de entrada).
    pub fn open_channel(&mut self, category: usize, channel: usize) {
        self.open_channel_with(category, channel, true);
    }

    /// `open_channel`, pero con `fetch_latest = false` no pide ni resetea el
    /// historial: lo usa `open_channel_at`, que carga la ventana de mensajes
    /// por su cuenta (`jump_to_message`).
    fn open_channel_with(&mut self, category: usize, channel: usize, fetch_latest: bool) {
        self.current_channel = (category, channel);
        self.compose_text.clear();
        self.reply_target = None;
        self.thread_panel = None;
        if fetch_latest {
            self.pending_jump = None;
        }
        self.subscribe_member_list();
        self.load_forum_posts_if_needed();
        if !fetch_latest {
            return;
        }

        let Screen::Server(server_index) = self.screen else { return };
        let Some(server) = self.servers.get_mut(server_index) else { return };
        let Some(ch) = server.channel_mut(category, channel) else { return };
        // La ventana cargada quedó en el medio del historial (se saltó a un
        // mensaje con `around`): al abrir el canal a secas se vuelve al final.
        if ch.has_newer {
            ch.messages.clear();
            ch.loaded = false;
            ch.has_newer = false;
            ch.loading_newer = false;
            ch.has_more = true;
        }
        if ch.loaded {
            return;
        }
        let Some(channel_id) = ch.channel_id.clone() else { return };
        // Se marca `loaded` de una, optimistamente, para no disparar el
        // mismo pedido de nuevo si el usuario cambia de canal y vuelve
        // antes de que responda el REST.
        ch.loaded = true;
        ch.loading = true;

        if let (Some(token), Some(tx)) = (&self.discord_token, &self.event_tx) {
            crate::discord::spawn_fetch_channel_messages(token.clone(), channel_id, tx.clone());
        }
    }

    /// Si el canal abierto es un foro y todavía no se pidieron sus posts, los
    /// pide (llegan como `AppEvent::ForumPosts`).
    fn load_forum_posts_if_needed(&mut self) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let needs = self
            .servers
            .get(index)
            .and_then(|s| s.channel(category, channel))
            .is_some_and(|c| c.is_forum && !c.forum.loaded && !c.forum.loading);
        if needs {
            self.load_forum_posts(true);
        }
    }

    /// Pide posts del foro abierto: la primera página (`reset`) o la
    /// siguiente a las que ya se tienen.
    pub fn load_forum_posts(&mut self, reset: bool) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let Some(channel) = self
            .servers
            .get_mut(index)
            .and_then(|s| s.channel_mut(category, channel))
        else {
            return;
        };
        if !channel.is_forum || channel.forum.loading {
            return;
        }
        if !reset && !channel.forum.has_more {
            return;
        }
        let Some(channel_id) = channel.channel_id.clone() else { return };
        let offset = if reset { 0 } else { channel.forum.next_offset };
        channel.forum.loading = true;
        crate::discord::spawn_fetch_forum_posts(
            token,
            channel_id,
            offset,
            channel.forum.by_creation,
            tx,
        );
    }

    /// Cambió el orden de los posts del foro abierto: se vacía la lista y se
    /// vuelve a pedir desde el principio con el orden nuevo.
    pub fn resort_forum(&mut self) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        if let Some(channel) = self
            .servers
            .get_mut(index)
            .and_then(|s| s.channel_mut(category, channel))
        {
            channel.forum.posts.clear();
            channel.forum.loaded = false;
            channel.forum.loading = false;
            channel.forum.has_more = false;
            channel.forum.next_offset = 0;
        }
        self.load_forum_posts(true);
    }

    /// Abre un post del foro actual como si fuera un canal (sus mensajes son
    /// las respuestas).
    pub fn open_forum_post(&mut self, thread_id: &str, title: &str) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let Some(server) = self.servers.get_mut(index) else { return };
        let forum_id = server.channel(category, channel).and_then(|c| c.channel_id.clone());
        let (thread_category, thread_channel) = server.open_thread(thread_id, title, forum_id);
        self.open_channel(thread_category, thread_channel);
    }

    /// Abre un hilo en el panel lateral (a la derecha del chat). Sus
    /// mensajes se piden por REST la primera vez; el canal del hilo queda
    /// guardado en el server, así que reabrirlo no vuelve a pedirlos.
    pub fn open_thread_panel(&mut self, thread_id: &str, title: &str, owner_id: Option<String>) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let Some(server) = self.servers.get_mut(index) else { return };
        let parent_id = server.channel(category, channel).and_then(|c| c.channel_id.clone());
        let (thread_category, thread_channel) = server.open_thread(thread_id, title, parent_id);
        if let Some(ch) = server.channel_mut(thread_category, thread_channel) {
            ch.name = title.to_string();
            if !ch.loaded {
                ch.loaded = true;
                ch.loading = true;
                if let (Some(token), Some(tx)) = (&self.discord_token, &self.event_tx) {
                    crate::discord::spawn_fetch_channel_messages(token.clone(), thread_id.to_string(), tx.clone());
                }
            }
        }
        self.thread_panel = Some(ThreadPanel {
            thread_id: thread_id.to_string(),
            title: title.to_string(),
            owner_id,
            compose_text: String::new(),
            reply_target: None,
            scroll_anchor: None,
        });
    }

    /// Cierra el panel del hilo.
    pub fn close_thread_panel(&mut self) {
        self.thread_panel = None;
    }

    /// Crea un hilo a partir de un mensaje del canal abierto; cuando
    /// Discord lo confirma (`AppEvent::ThreadOpened`) se abre el panel.
    pub fn create_thread_from_message(&mut self, channel_id: &str, message_id: &str, name: &str) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let name: String = name.chars().take(100).collect();
        let name = if name.trim().is_empty() { "Hilo".to_string() } else { name };
        crate::discord::spawn_create_thread(token, channel_id.to_string(), message_id.to_string(), name, tx);
    }

    /// (token, id del hilo, id del server) para mandar mensajes desde el
    /// panel del hilo.
    pub fn thread_send_target(&self) -> Option<(String, String, Option<String>)> {
        let token = self.discord_token.clone()?;
        let panel = self.thread_panel.as_ref()?;
        Some((token, panel.thread_id.clone(), self.current_guild_id()))
    }

    /// Publica el catálogo de slash commands del canal abierto para el
    /// compositor (`ui::slash`) y, si el compositor está escribiendo un `/...`
    /// y falta (o venció), lo pide a Discord. Mientras llega uno nuevo se sigue
    /// mostrando el viejo.
    fn publish_slash_catalog(&mut self, ctx: &egui::Context) {
        use crate::ui::slash;

        let wanted = slash::take_wanted(ctx);
        let Some((token, channel_id, guild_id)) = self.current_send_target() else {
            slash::publish_catalog(ctx, None);
            return;
        };
        // Los comandos son del server (o del DM): el hilo comparte los del padre.
        let key = guild_id.clone().unwrap_or_else(|| channel_id.clone());
        let (expired, catalog) = match self.slash_catalogs.get(&key) {
            Some(cached) => (cached.expired(), Some(cached.catalog.clone())),
            None => (true, None),
        };
        if wanted && expired && !self.slash_pending.contains(&key) {
            if let Some(tx) = self.event_tx.clone() {
                self.slash_pending.insert(key.clone());
                crate::discord::spawn_fetch_slash_catalog(token, key, channel_id, guild_id, tx);
            }
        }
        slash::publish_catalog(ctx, catalog);
    }

    /// Ejecuta el slash command que el compositor dejó listo
    /// (`ui::slash::take_invocation`).
    fn run_slash_invocation(&mut self, invocation: crate::ui::slash::SlashInvocation) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let application_id = invocation.command.application_id.clone();
        let Some(ctx) = self.interaction_context(&invocation.channel_id, &application_id, invocation.guild_id.clone())
        else {
            self.push_toast(
                ToastKind::Warning,
                "No se pudo ejecutar el comando",
                "Todavía no hay conexión con Discord.",
            );
            return;
        };
        // Para "usados frecuentemente" del menú de comandos: el id del comando
        // (con `\0subcomando[:server]` si se usó uno) y la app.
        let key = match invocation.path.last().filter(|_| invocation.path.len() > 1) {
            None => invocation.command.id.clone(),
            Some(leaf) => match invocation.guild_id.as_deref().filter(|g| !g.is_empty()) {
                Some(guild) => format!("{}\0{leaf}:{guild}", invocation.command.id),
                None => format!("{}\0{leaf}", invocation.command.id),
            },
        };
        crate::discord::frecency::record_command_use(&key);
        crate::discord::frecency::record_application_use(&application_id);
        crate::discord::spawn_run_slash_command(token, ctx, invocation.command, invocation.options, tx);
    }

    /// Datos comunes de una interacción con el canal `channel_id` (vacío =
    /// el canal abierto) y la app `application_id`.
    fn interaction_context(
        &self,
        channel_id: &str,
        application_id: &str,
        guild_id: Option<String>,
    ) -> Option<crate::discord::InteractionContext> {
        if self.gateway_session_id.is_empty() || application_id.is_empty() {
            return None;
        }
        let channel_id = if channel_id.is_empty() {
            self.current_send_target().map(|(_, id, _)| id)?
        } else {
            channel_id.to_string()
        };
        Some(crate::discord::InteractionContext {
            session_id: self.gateway_session_id.clone(),
            application_id: application_id.to_string(),
            guild_id: guild_id.or_else(|| self.current_guild_id()).filter(|g| !g.is_empty()),
            channel_id,
        })
    }

    /// Se apretó un botón de un mensaje: se le manda la interacción a la
    /// app dueña. Lo que conteste (mensaje, edición, formulario) llega por
    /// el Gateway.
    pub fn press_component(&mut self, click: crate::lib::data::ComponentClick) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        // Doble clic en un botón que ya está cargando: no se manda la
        // interacción dos veces.
        if self
            .pending_buttons
            .iter()
            .any(|p| p.message_id == click.message_id && p.custom_id == click.custom_id)
        {
            return;
        }
        let application_id = click.application_id.clone().unwrap_or_default();
        let Some(ctx) = self.interaction_context(&click.channel_id, &application_id, None) else {
            self.push_toast(
                ToastKind::Warning,
                "Botón",
                "Todavía no se puede usar este botón (falta la sesión o la aplicación dueña).",
            );
            return;
        };
        // El botón muestra su spinner hasta que el bot conteste (ver
        // `handle_discord_event`) o venza `tick_pending_buttons`.
        self.pending_buttons.push(crate::lib::data::PendingButton {
            message_id: click.message_id.clone(),
            custom_id: click.custom_id.clone(),
            since: Instant::now(),
        });
        crate::discord::spawn_press_button(token, ctx, click.message_id, click.flags, click.custom_id, tx);
    }

    /// Vence los botones que esperan respuesta hace demasiado y deja la
    /// lista vigente a mano de la UI de los mensajes. Una vez por frame.
    fn tick_pending_buttons(&mut self, ctx: &egui::Context) {
        // Respaldo por si nunca llega `INTERACTION_SUCCESS`/`FAILURE`
        // (Discord le da 3 s a la app para contestar).
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
        self.pending_buttons.retain(|p| p.since.elapsed() < TIMEOUT);
        crate::lib::data::publish_pending_buttons(ctx, &self.pending_buttons);
        if !self.pending_buttons.is_empty() {
            // El spinner ya pide repintar mientras se ve; esto cubre el caso
            // en que el mensaje quedó fuera de pantalla, para que igual venza.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Manda el formulario que pidió un bot. Si algún campo no cumple lo
    /// pedido queda el aviso en el propio modal y no se manda nada.
    pub fn submit_component_modal(&mut self) {
        let Some(modal) = self.component_modal.as_mut() else { return };
        if !modal.validate() {
            return;
        }
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let request = modal.request.clone();
        let fields: Vec<(String, String)> = modal
            .fields
            .iter()
            .map(|f| (f.custom_id.clone(), f.value.trim().to_string()))
            .collect();
        let Some(ctx) = self.interaction_context(&request.channel_id, &request.application_id, request.guild_id.clone())
        else {
            self.component_modal = None;
            self.push_toast(ToastKind::Warning, "Formulario", "No se pudo enviar: falta la sesión de Discord.");
            return;
        };
        self.component_modal = None;
        crate::discord::spawn_submit_modal(
            token,
            ctx,
            request.interaction_id,
            request.custom_id,
            request.components,
            fields,
            tx,
        );
    }

    /// Desde un post abierto, vuelve a la lista de posts de su foro.
    pub fn leave_thread(&mut self) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let target = self.servers.get(index).and_then(|server| {
            let forum_id = server
                .channel(category, channel)
                .filter(|c| c.is_thread)
                .and_then(|c| c.parent_id.clone())?;
            server.channel_position_by_id(&forum_id)
        });
        if let Some((forum_category, forum_channel)) = target {
            self.open_channel(forum_category, forum_channel);
        }
    }

    /// Publica el post que se está escribiendo en el foro abierto.
    pub fn create_forum_post(&mut self) {
        let Screen::Server(index) = self.screen else { return };
        let (category, channel) = self.current_channel;
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let draft = self
            .servers
            .get(index)
            .and_then(|s| s.channel(category, channel))
            .filter(|c| c.is_forum && !c.forum.creating)
            .and_then(|c| {
                Some((
                    c.channel_id.clone()?,
                    c.forum.draft_title.trim().to_string(),
                    c.forum.draft_body.trim().to_string(),
                    c.forum.draft_tags.clone(),
                ))
            });
        let Some((forum_id, title, body, tags)) = draft else { return };
        if title.is_empty() || body.is_empty() {
            self.push_toast(
                ToastKind::Warning,
                "Nueva publicación",
                "Escribí un título y un mensaje para publicar.",
            );
            return;
        }
        if let Some(channel) = self
            .servers
            .get_mut(index)
            .and_then(|s| s.channel_mut(category, channel))
        {
            channel.forum.creating = true;
        }
        crate::discord::spawn_create_forum_post(token, forum_id, title, body, tags, tx);
    }

    /// Id real de Discord del canal abierto ahora mismo, si es un server.
    fn current_channel_id(&self) -> Option<String> {
        let Screen::Server(index) = self.screen else { return None };
        let (category, channel) = self.current_channel;
        self.servers.get(index)?.channel(category, channel)?.channel_id.clone()
    }

    /// Después de que los canales de `guild_id` se rearmaron (los índices
    /// `(categoría, canal)` pueden haberse corrido), vuelve a ubicar el canal
    /// que se estaba mirando por su id. Si ya no existe, pasa al primero
    /// visible.
    fn restore_current_channel(&mut self, guild_id: &str, previous: Option<String>) {
        if guild_id.is_empty() {
            return;
        }
        let Screen::Server(index) = self.screen else { return };
        let Some(server) = self.servers.get(index) else { return };
        if server.guild_id != guild_id {
            return;
        }
        let same = previous.as_deref().and_then(|id| server.channel_position_by_id(id));
        let first = server.first_channel();
        match same {
            Some(position) => self.current_channel = position,
            None => self.open_channel(first.0, first.1),
        }
        // Puede que, con los permisos nuevos, el canal ya no se pueda ver.
        self.fix_hidden_current_channel();
    }

    /// Pide la página de mensajes más vieja para el canal/DM actualmente
    /// abierto — llamado desde `ui::dm`/`ui::server` cuando `chat::show`
    /// devuelve `ChatEvent::LoadMoreRequested` (el usuario llegó arriba
    /// del todo de lo ya cargado). No hace nada si ya hay un pedido de
    /// "más" en curso, si no queda más historial (`has_more`), o si es
    /// una conversación de demo (mensajes sin `id` real de Discord — no
    /// hay `before` válido que mandar).
    pub fn load_more_messages(&mut self) {
        println!("load_more_messages invoked");
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        match self.screen {
            Screen::Dm(index) => {
                let Some(friend) = self.friends.get_mut(index) else { return };
                if friend.loading_more || friend.loading || !friend.has_more {
                    return;
                }
                let Some(channel_id) = friend.dm_channel_id.clone() else { return };
                let Some(oldest_id) = friend
                    .messages
                    .first()
                    .map(|m| m.id.clone())
                    .filter(|id| !id.is_empty())
                else {
                    return;
                };
                friend.loading_more = true;
                self.pending_scroll_anchor = Some(oldest_id.clone());
                crate::discord::spawn_fetch_more_channel_messages(token, channel_id, oldest_id, tx);
            }
            Screen::Server(server_index) => {
                println!("load_more_messages");
                let (cat, chan) = self.current_channel;
                let Some(server) = self.servers.get_mut(server_index) else { return };
                let Some(channel) = server.channel_mut(cat, chan) else { return };
                println!("{:?}", channel.loading_more);
                println!("{:?}", channel.loading);
                println!("{:?}", channel.has_more);
                if channel.loading_more || channel.loading || !channel.has_more {
                    return;
                }
                let Some(channel_id) = channel.channel_id.clone() else { return };
                let Some(oldest_id) = channel
                    .messages
                    .first()
                    .map(|m| m.id.clone())
                    .filter(|id| !id.is_empty())
                else {
                    return;
                };
                channel.loading_more = true;
                self.pending_scroll_anchor = Some(oldest_id.clone());
                crate::discord::spawn_fetch_more_channel_messages(token, channel_id, oldest_id, tx);
            }
            _ => {}
        }
    }

    /// Último mensaje que Discord tiene como leído en `channel_id` (el que
    /// trajo el `READY` o el último ack). Sirve para abrir/previsualizar un
    /// canal justo donde se había dejado.
    pub fn last_read_message(&self, channel_id: &str) -> Option<&str> {
        self.acked_messages
            .get(channel_id)
            .map(String::as_str)
            .filter(|id| !id.is_empty() && *id != "0")
    }

    /// Va a un mensaje puntual de un canal/DM ya conocido. Si el mensaje ya
    /// está en la ventana cargada solo se mueve el scroll hasta él; si no,
    /// se piden los mensajes ALREDEDOR (`?limit=30&around=<id>`) y la lista
    /// pasa a ser esa ventana (`has_newer` queda en `true`: para ver lo más
    /// nuevo se sigue bajando con `after`, ver `load_newer_messages`).
    pub fn jump_to_message(&mut self, channel_id: &str, message_id: &str) {
        if channel_id.is_empty() || message_id.is_empty() {
            return;
        }
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };

        if let Some(friend) = self
            .friends
            .iter_mut()
            .find(|f| f.dm_channel_id.as_deref() == Some(channel_id))
        {
            if friend.messages.iter().any(|m| m.id == message_id) {
                self.pending_jump = Some(message_id.to_string());
                return;
            }
            // Los mensajes de ahora se quedan hasta que llegue la ventana
            // nueva (no hay parpadeo); `loading` frena otros pedidos.
            friend.loaded = true;
            friend.loading = true;
            friend.loading_more = false;
            friend.loading_newer = false;
            self.pending_jump = None;
            crate::discord::spawn_fetch_messages_around(
                token,
                channel_id.to_string(),
                message_id.to_string(),
                true,
                tx,
            );
            return;
        }

        let Some(channel) = self
            .servers
            .iter_mut()
            .find_map(|s| s.channel_by_id_mut(channel_id))
        else {
            return;
        };
        if channel.messages.iter().any(|m| m.id == message_id) {
            self.pending_jump = Some(message_id.to_string());
            return;
        }
        channel.loaded = true;
        channel.loading = true;
        channel.loading_more = false;
        channel.loading_newer = false;
        self.pending_jump = None;
        crate::discord::spawn_fetch_messages_around(
            token,
            channel_id.to_string(),
            message_id.to_string(),
            true,
            tx,
        );
    }

    /// Abre `servers[server].categories[category].channels[channel]` en un
    /// mensaje puntual (o a secas si `message_id` es `None`). Es lo que usan
    /// los canales recientes del Inicio para abrir un canal donde se había
    /// quedado la lectura.
    pub fn open_channel_at(
        &mut self,
        server: usize,
        category: usize,
        channel: usize,
        message_id: Option<String>,
    ) {
        self.open_server(server);
        let Some(message_id) = message_id else {
            self.open_channel(category, channel);
            return;
        };
        self.open_channel_with(category, channel, false);
        let channel_id = self
            .servers
            .get(server)
            .and_then(|s| s.channel(category, channel))
            .and_then(|c| c.channel_id.clone());
        if let Some(channel_id) = channel_id {
            self.jump_to_message(&channel_id, &message_id);
        }
    }

    /// Vista previa de un canal reciente del Inicio: si todavía no tiene
    /// mensajes cargados, pide la ventana alrededor del último mensaje leído
    /// (`around`), así se ve lo que quedó sin leer; sin marca de lectura
    /// pide lo más nuevo. No mueve el scroll del chat (`jump = false`).
    pub fn load_recent_preview(&mut self, server: usize, category: usize, channel: usize) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let Some(channel_id) = self
            .servers
            .get(server)
            .and_then(|s| s.channel(category, channel))
            .filter(|c| !c.loaded && !c.loading && !c.is_forum)
            .and_then(|c| c.channel_id.clone())
        else {
            return;
        };
        let last_read = self.last_read_message(&channel_id).map(str::to_string);
        if let Some(ch) = self
            .servers
            .get_mut(server)
            .and_then(|s| s.channel_mut(category, channel))
        {
            ch.loaded = true;
            ch.loading = true;
        }
        match last_read {
            Some(id) => crate::discord::spawn_fetch_messages_around(token, channel_id, id, false, tx),
            None => crate::discord::spawn_fetch_channel_messages(token, channel_id, tx),
        }
    }

    /// Pide la página POSTERIOR al último mensaje cargado del canal/DM que
    /// se está mirando (`?limit=30&after=<id>`). Solo corresponde cuando la
    /// ventana no llega hasta el final del canal (`has_newer`).
    pub fn load_newer_messages(&mut self) {
        let Some(channel_id) = self.viewed_channel_id() else { return };
        self.load_newer_for_channel(&channel_id);
    }

    /// Lo mismo que `load_newer_messages` para un canal puntual (la vista
    /// previa de los canales recientes del Inicio no es el chat abierto).
    pub fn load_newer_for_channel(&mut self, channel_id: &str) {
        if self.newer_retry_after.is_some_and(|until| Instant::now() < until) {
            return;
        }
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            return;
        };
        let newest = |messages: &[ChatMessage]| {
            messages
                .iter()
                .rev()
                .find(|m| !m.id.is_empty())
                .map(|m| m.id.clone())
        };
        if let Some(friend) = self
            .friends
            .iter_mut()
            .find(|f| f.dm_channel_id.as_deref() == Some(channel_id))
        {
            if !friend.has_newer || friend.loading_newer || friend.loading {
                return;
            }
            let Some(newest_id) = newest(&friend.messages) else { return };
            friend.loading_newer = true;
            crate::discord::spawn_fetch_newer_channel_messages(token, channel_id.to_string(), newest_id, tx);
            return;
        }
        let Some(channel) = self
            .servers
            .iter_mut()
            .find_map(|s| s.channel_by_id_mut(channel_id))
        else {
            return;
        };
        if !channel.has_newer || channel.loading_newer || channel.loading {
            return;
        }
        let Some(newest_id) = newest(&channel.messages) else { return };
        channel.loading_newer = true;
        crate::discord::spawn_fetch_newer_channel_messages(token, channel_id.to_string(), newest_id, tx);
    }

    /// Encola una notificación flotante en la esquina de la ventana.
    pub fn push_toast(&mut self, kind: ToastKind, title: impl Into<String>, message: impl Into<String>) {
        self.toasts.push(Toast {
            kind,
            title: title.into(),
            message: message.into(),
            created: Instant::now(),
        });
    }

    /// ¿La cuenta logueada tiene Discord Nitro? `None` si todavía no hay
    /// sesión o Discord no lo informó (ver `User::has_nitro`).
    pub fn has_nitro(&self) -> Option<bool> {
        self.me.as_ref().and_then(|u| u.has_nitro())
    }

    /// Abre un popup modal centrado. Reemplaza cualquier modal ya abierto.
    pub fn open_modal(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.modal = Some(Modal {
            title: title.into(),
            message: message.into(),
            confirm_label: "OK".to_string(),
        });
    }

    /// Cierra el popup modal actualmente abierto, si hay alguno.
    /// Encola un diálogo (ver `ui::dialog`). Si ya hay uno con el mismo `id`
    /// en la cola, se ignora para no apilar avisos repetidos.
    pub fn open_dialog(&mut self, dialog: crate::ui::dialog::Dialog) {
        if self.dialogs.iter().any(|d| d.id == dialog.id) {
            return;
        }
        self.dialogs.push_back(dialog);
        if let Some(ctx) = &self.egui_ctx {
            ctx.request_repaint();
        }
    }

    /// Abre el historial completo de novedades (Ajustes → Cuenta).
    pub fn open_changelog(&mut self) {
        self.open_dialog(crate::ui::changelog::dialog(usize::MAX));
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    // -- Tarjeta de perfil de usuario --

    /// Guild id del server actualmente abierto, si `self.screen` es uno
    /// real (no demo). `None` en Home/DM/un server de demo — la tarjeta
    /// de perfil que se abre ahí no tiene roles que mostrar.
    fn current_guild_id(&self) -> Option<String> {
        let Screen::Server(index) = self.screen else { return None };
        let server = self.servers.get(index)?;
        (!server.guild_id.is_empty()).then(|| server.guild_id.clone())
    }

    /// Abre la tarjeta de perfil de `user_id`, con lo que ya tengamos a
    /// mano (`fallback_*`) mientras se pide el resto por REST — ver
    /// `ui::profile_popup`. Reabrir el perfil de alguien que ya estaba
    /// abierto no relanza el pedido (evita spamear la API si el usuario
    /// clickea el mismo avatar varias veces seguidas).
    pub fn open_user_profile(
        &mut self,
        user_id: impl Into<String>,
        fallback_name: impl Into<String>,
        fallback_avatar_url: Option<String>,
        fallback_avatar_color: Color32,
    ) {
        let user_id = user_id.into();
        if user_id.is_empty() {
            // Usuario de demo sin id real: no hay nada que pedir por
            // REST, así que no tiene sentido abrir una tarjeta que se
            // quedaría cargando para siempre.
            return;
        }
        if let Some(existing) = &self.profile_popup {
            if existing.user_id == user_id {
                return;
            }
        }
        let guild_id = self.current_guild_id();
        self.profile_popup = Some(ProfilePopup {
            user_id: user_id.clone(),
            guild_id: guild_id.clone(),
            fallback_name: fallback_name.into(),
            fallback_avatar_url,
            fallback_avatar_color,
            loading: true,
            failed: false,
            profile: None,
            bio_expanded: false,
            quick_message: String::new(),
            anchor: None,
        });
        if let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) {
            crate::discord::spawn_fetch_user_profile(token, user_id, guild_id, tx);
        } else {
            // Sin sesión real (pantallas de demo): no hay a quién
            // pedirle el perfil completo, así que la tarjeta se queda
            // mostrando solo lo que ya sabíamos.
            if let Some(popup) = &mut self.profile_popup {
                popup.loading = false;
            }
        }
    }

    /// Deja cargado (una sola vez por persona) el perfil del panel derecho de
    /// un DM. Cambiar de DM lo reemplaza y relanza el pedido; volver a dibujar
    /// el mismo no hace nada.
    pub fn ensure_dm_profile(
        &mut self,
        user_id: &str,
        name: String,
        avatar_url: Option<String>,
        avatar_color: Color32,
    ) {
        if user_id.is_empty() || self.dm_profile.as_ref().is_some_and(|p| p.user_id == user_id) {
            return;
        }
        let has_session = self.discord_token.is_some() && self.event_tx.is_some();
        self.dm_profile = Some(ProfilePopup {
            user_id: user_id.to_string(),
            guild_id: None,
            fallback_name: name,
            fallback_avatar_url: avatar_url,
            fallback_avatar_color: avatar_color,
            loading: has_session,
            failed: false,
            profile: None,
            bio_expanded: false,
            quick_message: String::new(),
            anchor: None,
        });
        if let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) {
            crate::discord::spawn_fetch_user_profile(token, user_id.to_string(), None, tx);
        }
    }

    /// Abre el modal de perfil completo de `user_id` (su perfil ya tiene que
    /// estar cargado en `dm_profile` o `profile_popup`).
    pub fn open_full_profile(&mut self, user_id: impl Into<String>) {
        self.profile_full = Some(FullProfile { user_id: user_id.into(), tab: 0 });
    }

    /// Cierra la tarjeta de perfil actualmente abierta, si hay alguna.
    pub fn close_profile_popup(&mut self) {
        self.profile_popup = None;
    }

    /// Abre (o crea) la conversación de DM con `user_id` y cierra la
    /// tarjeta de perfil — llamado desde el ícono de mensaje y desde la
    /// cajita de mensaje rápido del pie de la tarjeta. Si `prefill` trae
    /// texto, queda ya cargado en el composer del DM (el usuario todavía
    /// tiene que apretar enviar); no lo mandamos solo desde acá para no
    /// mandar mensajes sin que la persona vea exactamente qué se fue.
    pub fn message_user_from_profile(
        &mut self,
        user_id: &str,
        display_name: &str,
        avatar_url: Option<String>,
        avatar_color: Color32,
        prefill: Option<String>,
    ) {
        let index = match self.friends.iter().position(|f| f.user_id == user_id) {
            Some(i) => i,
            None => {
                self.friends.push(Friend {
                    name: display_name.to_string(),
                    status: crate::lib::data::Status::Offline,
                    subtitle: None,
                    avatar_color,
                    avatar_url,
                    user_id: user_id.to_string(),
                    dm_channel_id: None,
                    handle: display_name.to_string(),
                    member_since: String::new(),
                    mutual_servers: 0,
                    messages: Vec::new(),
                    loaded: false,
                    loading: false,
                    loading_more: false,
                    has_more: true,
                    has_newer: false,
                    loading_newer: false,
                    // Solo se abre el DM: no es (ni pasa a ser) un amigo.
                    is_friend: false,
                });
                self.friends.len() - 1
            }
        };
        self.open_dm(index);
        if let Some(text) = prefill {
            self.compose_text = text;
        }
        self.close_profile_popup();
    }

    // -- Temas: modo activo, wallpaper dinámico, y temas del usuario --

    /// Recalcula `self.palette` a partir de `self.theme_mode`. No toca
    /// nada persistido; llamalo después de cambiar `theme_mode` a mano
    /// (aunque para eso normalmente conviene `set_theme_mode`, que
    /// además persiste el cambio).
    fn apply_theme_mode(&mut self) {
        // Matcheamos sobre una copia local (no `&self.theme_mode`) porque la
        // rama `Wallpaper` necesita llamar a `self.start_wallpaper_watch()`
        // (pide `&mut self` entero), y eso no se puede hacer con un borrow
        // de un campo de `self` todavía vivo por culpa del `match`.
        let mode = self.theme_mode.clone();
        // Solo los temas propios traen fondo; el resto vuelve al sólido opaco.
        self.backdrop = match &mode {
            ThemeMode::Custom(name) => self
                .custom_themes
                .iter()
                .find(|def| &def.name == name)
                .map(|def| def.backdrop.clone())
                .unwrap_or_default(),
            _ => Backdrop::default(),
        };
        self.palette = match &mode {
            ThemeMode::Dark => Palette::dark(),
            ThemeMode::Light => Palette::light(),
            ThemeMode::CreArts => Palette::crearts(),
            ThemeMode::Wallpaper => {
                self.start_wallpaper_watch();
                self.wallpaper_seed.map(Palette::from_seed).unwrap_or_else(Palette::dark)
            }
            ThemeMode::Custom(name) => self
                .custom_themes
                .iter()
                .find(|def| &def.name == name)
                .map(Palette::from_def)
                .unwrap_or_else(Palette::dark),
        };
    }

    /// Cambia el modo de imágenes (normal / solo estáticas / sin imágenes de
    /// perfil) y lo persiste. Ver `ui::anim::ImageMode`.
    pub fn set_image_mode(&mut self, mode: crate::ui::anim::ImageMode) {
        crate::ui::anim::set_image_mode(mode);
        let _ = web_local_storage_api::set_item("image_mode", &mode.to_u8().to_string());
    }

    /// Activa/desactiva la barra de llamada nueva (a todo el ancho, abajo) y lo
    /// persiste.
    pub fn set_new_call_ui(&mut self, enabled: bool) {
        self.new_call_ui = enabled;
        let _ = web_local_storage_api::set_item("new_call_ui", if enabled { "1" } else { "0" });
        crate::theme::set_modern(enabled);
        // Las pestañas del Inicio son distintas en cada interfaz: se vuelve a la
        // primera para no quedar en un índice que significa otra cosa.
        self.friends_tab = 0;
        self.home_scroll_req = [None; 2];
    }

    /// Cambia el tamaño de toda la interfaz (zoom de egui) y lo persiste. El
    /// valor se acota a `UI_SCALE_MIN..=UI_SCALE_MAX` y se aplica en el
    /// próximo frame (ver `App::ui`).
    pub fn set_ui_scale(&mut self, scale: f32) {
        let scale = (scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX) * 100.0).round() / 100.0;
        self.ui_scale = scale;
        let _ = web_local_storage_api::set_item("ui_scale", &format!("{scale:.2}"));
    }

    /// Lleva a la pantalla de la llamada en curso: el canal de voz del server
    /// o el DM. Botón de "ir a la llamada" de la barra nueva.
    pub fn go_to_voice_call(&mut self) {
        let Some(target) = self.voice_target.as_ref() else { return };
        let channel_id = target.channel_id.to_string();
        let guild_id = match target.scope {
            crate::discord::VoiceScope::Guild(guild_id) => Some(guild_id.to_string()),
            crate::discord::VoiceScope::Private(_) => None,
        };
        match guild_id {
            Some(guild_id) => {
                let Some(index) = self.servers.iter().position(|s| s.guild_id == guild_id) else {
                    return;
                };
                if !matches!(self.screen, Screen::Server(current) if current == index) {
                    self.open_server(index);
                }
                self.open_channel_by_id(&channel_id);
            }
            None => {
                let Some(index) = self
                    .friends
                    .iter()
                    .position(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                else {
                    return;
                };
                self.open_dm(index);
            }
        }
    }

    /// Elige de qué lado de la ventana aparecen las notificaciones y lo
    /// persiste.
    pub fn set_notification_side(&mut self, side: NotificationSide) {
        self.notification_side = side;
        let _ = web_local_storage_api::set_item("notification_side", side.as_str());
    }

    /// Cambia el modo de tema activo (oscuro/claro/wallpaper/uno
    /// guardado), recalcula la paleta ya mismo y lo persiste para la
    /// próxima vez que se abra la app.
    pub fn set_theme_mode(&mut self, mode: ThemeMode) {
        self.theme_mode = mode;
        self.apply_theme_mode();
        if let Ok(json) = serde_json::to_string(&self.theme_mode) {
            let _ = web_local_storage_api::set_item("theme_mode", &json);
        }
    }

    /// Arranca (una sola vez) el hilo de fondo que vigila el wallpaper
    /// del sistema: lo revisa cada pocos segundos y, si cambió, decodifica
    /// la imagen nueva y manda su color dominante por el canal. Vive
    /// mientras dure la app; no hace falta pararlo al salir de
    /// `ThemeMode::Wallpaper`, porque no manda nada mientras nadie lo
    /// escuche del lado de la UI y el costo de chequear es mínimo.
    fn start_wallpaper_watch(&mut self) {
        if self.wallpaper_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.wallpaper_rx = Some(rx);
        std::thread::spawn(move || {
            let mut last_path: Option<String> = None;
            loop {
                if let Ok(path) = wallpaper::get() {
                    if last_path.as_deref() != Some(path.as_str()) {
                        if let Some(seed) = crate::theme::sample_wallpaper_color_from_path(&path) {
                            if tx.send(seed).is_err() {
                                return;
                            }
                        }
                        last_path = Some(path);
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        });
    }

    /// Recibe los colores que haya mandado el hilo de `start_wallpaper_watch`
    /// desde la última vez, y si estamos en `ThemeMode::Wallpaper`
    /// recalcula la paleta con el más reciente. Se llama una vez por
    /// frame desde `App::ui`, igual que `poll_discord_events`.
    pub fn poll_wallpaper_updates(&mut self) {
        let Some(rx) = &self.wallpaper_rx else { return };
        let mut latest = None;
        while let Ok(seed) = rx.try_recv() {
            latest = Some(seed);
        }
        let Some(seed) = latest else { return };
        self.wallpaper_seed = Some(seed);
        if matches!(self.theme_mode, ThemeMode::Wallpaper) {
            self.palette = Palette::from_seed(seed);
        }
    }

    /// El color más reciente que se sacó del wallpaper (o `None` si
    /// todavía no se activó `ThemeMode::Wallpaper` ninguna vez, o el
    /// hilo de fondo no encontró nada usable). Lo usa `ui::settings` para
    /// la muestra de color y para arrancar el editor de temas con ese
    /// color como base.
    pub fn wallpaper_seed(&self) -> Option<[u8; 3]> {
        self.wallpaper_seed
    }

    /// Guarda (o reemplaza, si ya existía uno con el mismo nombre) un
    /// tema creado por el usuario, y lo persiste. Si es el tema
    /// activo en este momento, refresca la paleta con los colores
    /// nuevos.
    pub fn save_custom_theme(&mut self, def: ThemeDef) {
        if let Some(existing) = self.custom_themes.iter_mut().find(|t| t.name == def.name) {
            *existing = def.clone();
        } else {
            self.custom_themes.push(def.clone());
        }
        self.persist_custom_themes();
        if matches!(&self.theme_mode, ThemeMode::Custom(name) if name == &def.name) {
            self.palette = Palette::from_def(&def);
            self.backdrop = def.backdrop.clone();
        }
    }

    /// Borra un tema guardado. Si era el activo, vuelve a oscuro.
    pub fn delete_custom_theme(&mut self, name: &str) {
        self.custom_themes.retain(|t| t.name != name);
        self.persist_custom_themes();
        if matches!(&self.theme_mode, ThemeMode::Custom(n) if n == name) {
            self.set_theme_mode(ThemeMode::Dark);
        }
    }

    fn persist_custom_themes(&self) {
        if let Ok(json) = serde_json::to_string(&self.custom_themes) {
            let _ = web_local_storage_api::set_item("custom_themes", &json);
        }
    }

    /// Abre el editor de temas para crear uno nuevo, arrancando de la
    /// paleta activa (o del color del wallpaper, si ya se sacó alguno:
    /// suele ser un punto de partida más lindo que oscuro liso).
    pub fn open_theme_editor_new(&mut self) {
        // `self.palette` puede traer la translucidez del tema activo (alfa
        // premultiplicado), que un color picker opaco no sabe editar: si el
        // activo es un tema propio se parte de sus colores originales.
        let base = self.wallpaper_seed.map(Palette::from_seed).unwrap_or_else(|| {
            match &self.theme_mode {
                ThemeMode::Custom(name) => self
                    .custom_themes
                    .iter()
                    .find(|d| &d.name == name)
                    .map(Palette::from_def_opaque)
                    .unwrap_or(self.palette),
                _ => self.palette,
            }
        });
        let mut editor = ThemeEditor::from_palette(&base, None, String::new());
        // Un tema nuevo hereda el fondo del activo.
        editor.backdrop = self.backdrop.clone();
        self.theme_editor = Some(editor);
    }

    /// Abre el editor sobre un tema ya guardado, para modificarlo.
    pub fn open_theme_editor_existing(&mut self, name: &str) {
        let Some(def) = self.custom_themes.iter().find(|t| t.name == name).cloned() else {
            return;
        };
        self.theme_editor = Some(ThemeEditor::from_def(&def));
    }

    /// Guarda lo que haya en el editor de temas abierto y lo activa. Si
    /// el nombre está vacío, no guarda nada (y avisa con un toast) para
    /// no dejar temas sin nombre en la lista.
    pub fn save_theme_editor(&mut self) {
        let Some(editor) = self.theme_editor.take() else { return };
        if editor.name.trim().is_empty() {
            self.theme_editor = Some(editor);
            self.push_toast(
                ToastKind::Warning,
                "Falta un nombre",
                "Ponele un nombre al tema antes de guardarlo.",
            );
            return;
        }
        let previous_name = editor.editing_name.clone();
        let def = editor.to_def();
        // Si veníamos editando un tema y le cambiaron el nombre, sacamos
        // la entrada vieja para no dejar un duplicado.
        if let Some(old) = &previous_name {
            if old != &def.name {
                self.custom_themes.retain(|t| &t.name != old);
            }
        }
        let mode = ThemeMode::Custom(def.name.clone());
        self.save_custom_theme(def);
        self.set_theme_mode(mode);
    }

    /// Importa temas desde un JSON: una lista (`[ {...}, {...} ]`, lo que
    /// exporta el editor de temas externo) o un único objeto. Cada tema se
    /// guarda con un nombre libre (si ya existe uno igual se le agrega
    /// « (2)», etc., para no pisar el tuyo) y el último importado queda
    /// activo. Devuelve cuántos temas entraron, o un mensaje de error.
    pub fn import_themes(&mut self, json: &str) -> Result<usize, String> {
        let json = json.trim().trim_start_matches('\u{feff}');
        if json.is_empty() {
            return Err("No hay nada para importar.".into());
        }
        let mut defs: Vec<ThemeDef> = match serde_json::from_str::<Vec<ThemeDef>>(json) {
            Ok(list) => list,
            Err(list_err) => match serde_json::from_str::<ThemeDef>(json) {
                Ok(one) => vec![one],
                // Si parece una lista, el error de la lista es el útil.
                Err(one_err) => {
                    let err = if json.starts_with('[') { list_err } else { one_err };
                    return Err(format!("El JSON no es un tema válido: {err}"));
                }
            },
        };
        if defs.is_empty() {
            return Err("La lista de temas está vacía.".into());
        }
        let imported = defs.len();
        let mut last = None;
        for mut def in defs.drain(..) {
            let base = match def.name.trim() {
                "" => "Tema importado".to_owned(),
                name => name.to_owned(),
            };
            let mut name = base.clone();
            let mut n = 2;
            while self.custom_themes.iter().any(|t| t.name == name) {
                name = format!("{base} ({n})");
                n += 1;
            }
            def.name = name.clone();
            // Datos de un archivo de afuera: se limpian los valores que
            // romperían el dibujado (posiciones fuera de rango, demasiados
            // colores, valores no numéricos).
            let b = &mut def.backdrop;
            b.stops.truncate(5);
            for s in &mut b.stops {
                s.pos = if s.pos.is_finite() { s.pos.clamp(0.0, 1.0) } else { 0.0 };
            }
            for v in b.center.iter_mut() {
                if !v.is_finite() {
                    *v = 0.5;
                }
            }
            for v in [
                &mut b.angle,
                &mut b.radius,
                &mut b.image_blur,
                &mut b.image_dim,
                &mut b.background_opacity,
                &mut b.panel_opacity,
                &mut b.surface_opacity,
            ] {
                if !v.is_finite() {
                    *v = 0.0;
                }
            }
            b.background_opacity = b.background_opacity.clamp(0.2, 1.0);
            b.panel_opacity = b.panel_opacity.clamp(0.0, 1.0);
            b.surface_opacity = b.surface_opacity.clamp(0.1, 1.0);
            self.custom_themes.push(def);
            last = Some(name);
        }
        self.persist_custom_themes();
        if let Some(name) = last {
            self.set_theme_mode(ThemeMode::Custom(name));
        }
        Ok(imported)
    }

    /// Cierra el editor de temas sin guardar nada.
    pub fn cancel_theme_editor(&mut self) {
        self.theme_editor = None;
    }

    /// Saca todos los eventos que hayan llegado del hilo de Discord desde
    /// el frame anterior y actualiza el estado. Se llama una vez por
    /// frame, antes de dibujar. Mientras el login/la conexión están en
    /// curso pedimos repintar seguido para que el spinner se vea fluido
    /// aunque no haya otra interacción del usuario.
    fn poll_discord_events(&mut self, ctx: &egui::Context) {
        if self.egui_ctx.is_none() {
            self.egui_ctx = Some(ctx.clone());
            // Los pedidos de captcha nacen en hilos de fondo: necesitan poder
            // despertar a la UI aunque esté quieta.
            crate::discord::captcha::set_repaint_context(ctx);
            // Idem para el spinner de la barra superior: las requests REST
            // empiezan y terminan en hilos de fondo.
            crate::discord::activity::set_repaint_context(ctx);
        }
        for notice in self.captcha.poll() {
            match notice {
                crate::discord::captcha::CaptchaNotice::Opened => self.push_toast(
                    ToastKind::Info,
                    "Verificación",
                    "Discord pidió un captcha. Resolvelo en la ventana que se abrió.",
                ),
                crate::discord::captcha::CaptchaNotice::OpenFailed(message) => {
                    self.push_toast(ToastKind::Warning, "Captcha", message)
                }
            }
        }
        crate::lib::data::publish_guild_directory(ctx, &self.servers);
        self.flush_member_subscription(ctx);
        // Favoritos/frecency: manda lo pendiente cuando toca (o ya, si la
        // ventana perdió el foco: cerrar la app no avisa). Si queda algo sin
        // mandar, se pide un repaint para que el temporizador siga corriendo
        // aunque la UI esté quieta.
        if let Some(token) = self.discord_token.as_deref()
            && crate::discord::frecency::tick(token, !self.window_focused)
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
        // Volumen/silencio por persona: lo mismo, con un temporizador corto
        // para que soltar el slider se guarde enseguida.
        if self.flush_participant_audio(!self.window_focused) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let Some(rx) = &self.event_rx else { return };
        let pending: Vec<AppEvent> = rx.try_iter().collect();
        for event in pending {
            self.handle_discord_event(event);
            // Si ese evento tiró la conexión (sesión inválida, Gateway
            // perdido) lo que queda en la cola es de la conexión muerta.
            if self.event_rx.is_none() {
                break;
            }
        }

        if matches!(
            self.auth,
            AuthStatus::Starting
                | AuthStatus::AwaitingScan { .. }
                | AuthStatus::Confirming { .. }
                | AuthStatus::Connecting
                | AuthStatus::PasswordSubmitting
                | AuthStatus::PasswordMfaSubmitting
        ) {
            ctx.request_repaint();
        }
    }

    fn handle_discord_event(&mut self, event: AppEvent) {
        // Todo evento de voz "de cable" pasa primero por acá: alimenta el
        // cache de participantes/habla (`self.voice`, lado sync) y, si el
        // hilo del runtime ya está arriba, también se lo reenvía a él (lado
        // async — el que de verdad abre el socket de voz). Los dos caminos
        // leen el mismo `AppEvent`, así que conviene hacerlo una sola vez
        // acá arriba en vez de duplicarlo en cada `match` de abajo.
        self.apply_voice_wire_event(&event);
        // El runtime necesita enterarse del `Ready` (trae el user id propio,
        // vía `VoiceRuntimeEvent::CurrentUserReady`) para que más adelante
        // `record_voice_state` pueda reconocer los `VOICE_STATE_UPDATE`
        // propios. Antes esto solo arrancaba el runtime al primer intento de
        // unirse a un canal (`sync_voice_target`), que pasa DESPUÉS de este
        // único `Ready` — el runtime nunca se enteraba de su propio user id
        // y la conexión de voz se quedaba esperando para siempre ("Conectando…"
        // sin fin, sin ningún log, porque ni se llegaba a intentar abrir el
        // websocket de voz). Arrancarlo acá también es idempotente/seguro.
        if matches!(event, AppEvent::Ready(_)) {
            self.ensure_voice_runtime_started();
        }
        if let Some(tx) = &self.voice_runtime_tx {
            crate::discord::voice::forward_app_event(tx, &event);
        }
        match event {
            AppEvent::QrCode(url) => {
                self.auth = AuthStatus::AwaitingScan { url };
            }
            AppEvent::QrConfirming { username } => {
                self.auth = AuthStatus::Confirming { username };
            }
            AppEvent::LoggedIn { token } => {
                self.auth = AuthStatus::Connecting;
                self.remember_login(&token);
                self.discord_token = Some(token);
                self.push_connection_step("Descargando ajustes…".to_owned());
                if let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) {
                    crate::discord::spawn_fetch_frecency_settings(token.clone(), tx.clone());
                    crate::discord::spawn_fetch_user_settings(token, tx);
                }
            }
            AppEvent::ConnectionStep(text) => {
                // Solo importa mientras se ve "Conectando…"; una vez dentro
                // (o en cualquier reconexión posterior) se descartan.
                if matches!(self.auth, AuthStatus::Connecting) {
                    self.push_connection_step(text);
                }
            }
            AppEvent::UserSettings(settings) => {
                if matches!(self.auth, AuthStatus::Connecting) {
                    self.push_connection_step("Ajustes descargados".to_owned());
                }
                // Se aplica una sola vez, apenas llega (justo después del
                // login): si el usuario cambia el tema a mano desde acá
                // más tarde, eso pasa por `set_theme_mode` (ver
                // `ui::settings`) y ya no tiene nada que ver con este
                // evento, así que no hace falta lógica extra para no
                // pisarlo después.
                // Un tema propio (con su fondo) elegido en esta app no se pisa
                // con el oscuro/claro de la cuenta: si no, cada inicio de
                // sesión lo tiraría a `Dark` y el fondo personalizado se
                // perdería.
                if !matches!(self.theme_mode, ThemeMode::Custom(_)) {
                    if let Some(mode) = crate::discord::user_settings::theme_mode_from_settings(&settings) {
                        self.theme_mode = mode;
                        self.apply_theme_mode();
                    }
                }
                let participant_audio =
                    crate::discord::user_settings::participant_playback_from_settings(&settings);
                let stream_audio =
                    crate::discord::user_settings::stream_playback_from_settings(&settings);
                self.discord_settings = Some(*settings);
                self.ingest_stream_audio(stream_audio, true);
                // Volumen/silencio por persona: la cuenta manda (salvo lo que
                // se cambió acá y todavía no salió).
                self.ingest_participant_audio(participant_audio, true);
            }
            AppEvent::FrecencySettings(settings) => {
                crate::discord::frecency::ingest_initial(*settings);
            }
            AppEvent::FrecencyUpdate { settings, partial } => {
                crate::discord::frecency::ingest_update(*settings, partial);
            }
            AppEvent::Affinities { users, guilds } => {
                if let Some(users) = users {
                    self.affinity_users = users;
                }
                if let Some(guilds) = guilds {
                    self.affinity_guilds = guilds;
                }
            }
            AppEvent::UserSettingsUpdate { settings, partial } => {
                // Un cambio hecho en otro dispositivo (sobre todo el orden y
                // las carpetas de servers). `guild_folders` se REEMPLAZA
                // entero, nunca se mergea: es una lista, y mergear
                // concatenaría y duplicaría los servers en la barra.
                let settings = *settings;
                // Volumen/silencio por persona cambiado desde otro dispositivo
                // (o el eco de nuestro propio guardado). En un update parcial
                // vienen solo las personas que cambiaron.
                let participant_audio =
                    crate::discord::user_settings::participant_playback_from_settings(&settings);
                self.ingest_participant_audio(participant_audio, !partial);
                let stream_audio =
                    crate::discord::user_settings::stream_playback_from_settings(&settings);
                self.ingest_stream_audio(stream_audio, !partial);
                let mut merged = false;
                if partial {
                    if let Some(current) = self.discord_settings.as_mut() {
                        if settings.guild_folders.is_some() {
                            current.guild_folders = settings.guild_folders.clone();
                        }
                        merged = true;
                    }
                }
                if !merged {
                    self.discord_settings = Some(settings);
                }
            }
            AppEvent::Ready(ready) => {
                self.gateway_session_id = ready.session_id.clone();
                crate::discord::invites::set_session_id(&ready.session_id);
                let display_name = ready.user.display_name().to_string();
                // Perfil de la cuenta para el selector (nombre, avatar, id).
                if let Some(token) = self.discord_token.clone() {
                    self.accounts.attach_profile(&token, &ready.user);
                    self.accounts.save();
                }
                self.me = Some(ready.user);
                self.friends = ready
                    .relationships
                    .iter()
                    .filter(|r| r.kind == 1) // 1 = amistad confirmada
                    .map(Friend::from_relationship)
                    .collect();
                self.relationship_kinds = ready
                    .relationships
                    .iter()
                    .map(|r| (r.id.clone(), r.kind))
                    .collect();
                // Solicitudes de amistad entrantes (3) y salientes (4).
                self.pending_requests = ready
                    .relationships
                    .iter()
                    .filter_map(crate::lib::data::PendingRequest::from_relationship)
                    .collect();
                self.ignored_users = ready
                    .relationships
                    .iter()
                    .filter(|r| r.user_ignored)
                    .map(|r| r.id.clone())
                    .collect();
                self.menu_ctx_rev += 1;
                self.servers = ready.guilds.iter().map(Server::from_guild).collect();
                // Para la tarjeta de invitación ("Ir al servidor" vs "Unirse").
                crate::discord::invites::set_joined_guilds(
                    self.servers.iter().map(|s| s.guild_id.clone()),
                );
                // Afinidades (amigos y servers) para ordenar el Inicio nuevo.
                if let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) {
                    crate::discord::spawn_fetch_affinities(token, tx);
                }
                self.dms = ready.private_channels.iter().map(PrivateChannel::from_dm).collect();
                // El `READY` no manda los DMs en orden: se ordenan por el
                // último mensaje, como el cliente oficial.
                self.sort_dms();
                self.mute_rules =
                    crate::lib::notifications::MuteRules::from_ready(&ready.user_guild_settings);
                // No leídos reales de la cuenta (guardados por Discord): así
                // sobreviven a reiniciar el cliente.
                self.apply_ready_read_state(&ready.read_state);
                self.own_activities = crate::discord::models::parse_session_activities(&ready.sessions);
                self.user_activities.clear();
                self.custom_statuses.clear();
                // Guilds grandes a veces mandan los `voice_states` del
                // `READY` sin `member` embebido (para no inflar un
                // payload que ya es enorme) — completamos esos por REST
                // en vez de dejarlos como "Usuario desconocido" para
                // siempre.
                let mut unknown_ids = std::collections::HashSet::new();
                for server in &self.servers {
                    for category in &server.categories {
                        for channel in &category.channels {
                            for occupant in &channel.voice_members {
                                if !occupant.known {
                                    unknown_ids.insert(occupant.user_id.clone());
                                }
                            }
                        }
                    }
                }
                for user_id in unknown_ids {
                    self.fetch_unknown_voice_user(&user_id);
                }
                // Sin rich-presence real todavía (ver nota en
                // `ActivityCard`/`discord::gateway`): el panel "Activo
                // ahora" queda vacío en vez de mostrar datos de demo que
                // no corresponden a la cuenta real.
                self.activity = Vec::new();
                self.auth = AuthStatus::Connected;
                self.go_home();
                self.push_toast(
                    ToastKind::Success,
                    "Sesión iniciada",
                    format!("Conectado como {display_name}."),
                );
            }
            AppEvent::MessageCreate(msg) => self.handle_incoming_message(*msg),
            AppEvent::MessageUpdate(update) => {
                // Un bot que contesta a un botón editando su propio mensaje
                // (paginadores, menús...) ya respondió.
                self.pending_buttons.retain(|p| p.message_id != update.id);
                if let Some(msg) = self.find_message_mut(&update.channel_id, &update.id) {
                    msg.apply_update(&update);
                }
            }
            AppEvent::DmOpened {
                user_id,
                channel_id,
                messages,
            } => {
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
                    friend.dm_channel_id = Some(channel_id.clone());
                    // El endpoint de mensajes los devuelve del más nuevo
                    // al más viejo; los damos vuelta para mostrar el
                    // historial en orden cronológico.
                    let has_more = messages.len() >= crate::discord::rest::MESSAGES_PAGE_SIZE as usize;
                    friend.messages = messages
                        .iter()
                        .rev()
                        .map(|m| ChatMessage::from_discord(m, &my_id))
                        .collect();
                    friend.loading = false;
                    friend.has_more = has_more;
                }
                // "Iniciar una llamada" desde el menú de clic derecho esperaba
                // este canal.
                if self.pending_call_user.as_deref() == Some(user_id.as_str()) {
                    self.pending_call_user = None;
                    self.start_dm_call(&channel_id);
                }
            }
            AppEvent::GuildChannels { guild_id, channels } => {
                let unknown_voice: Vec<String> =
                    if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                        server.apply_channels(channels);
                        // Estados de voz que esperaban a estos canales pueden
                        // haber quedado sin nombre: se piden por REST.
                        server.unknown_voice_user_ids().into_iter().collect()
                    } else {
                        Vec::new()
                    };
                for user_id in unknown_voice {
                    self.fetch_unknown_voice_user(&user_id);
                }
                // Ahora que se conocen los canales de este server, pueden
                // asignarse las menciones sin leer que trajo el `READY`.
                self.apply_pending_mentions();
                // Si el server recién llegado es el que está abierto ahora
                // mismo, el canal seleccionado por defecto (el primero) no
                // existía todavía cuando `open_server` corrió `open_channel`
                // (las categorías estaban vacías), así que no se disparó el
                // pedido de su historial. Lo intentamos de nuevo acá.
                if let Screen::Server(index) = self.screen {
                    if self.servers.get(index).map(|s| s.guild_id.as_str()) == Some(guild_id.as_str())
                    {
                        let (mut cat, mut chan) = self.current_channel;
                        // Si el canal por defecto (0, 0) resultó ser uno que
                        // no se puede ver, se abre el primero que sí.
                        if let Some(server) = self.servers.get(index) {
                            if !server.channel_visible(cat, chan) {
                                (cat, chan) = server.first_channel();
                            }
                        }
                        self.open_channel(cat, chan);
                    }
                }
            }
            AppEvent::ChannelMessages { channel_id, messages } => {
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let has_more = messages.len() >= crate::discord::rest::MESSAGES_PAGE_SIZE as usize;
                if let Some(friend) = self
                    .friends
                    .iter_mut()
                    .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                {
                    friend.messages = messages
                        .iter()
                        .rev()
                        .map(|m| ChatMessage::from_discord(m, &my_id))
                        .collect();
                    friend.loaded = true;
                    friend.loading = false;
                    friend.has_more = has_more;
                    return;
                }
                // Canal de server: los autores salen con su apodo del server
                // y el color de su rol (lo que ya sepamos), y de los que
                // falten se piden apodo y roles al Gateway.
                let mut to_request: Option<(String, Vec<String>)> = None;
                'outer: for server in &mut self.servers {
                    let names = NameResolver::new(&server.member_info, &server.roles);
                    for category in &mut server.categories {
                        for channel in &mut category.channels {
                            if channel.channel_id.as_deref() == Some(channel_id.as_str()) {
                                channel.messages = messages
                                    .iter()
                                    .rev()
                                    .map(|m| ChatMessage::from_discord_in(m, &my_id, Some(names)))
                                    .collect();
                                channel.loaded = true;
                                channel.loading = false;
                                channel.has_more = has_more;
                                to_request = Some((server.guild_id.clone(), message_author_ids(&messages)));
                                break 'outer;
                            }
                        }
                    }
                }
                if let Some((guild_id, user_ids)) = to_request {
                    self.request_members(&guild_id, user_ids);
                }
            }
            AppEvent::MoreChannelMessages { channel_id, messages } => {
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let has_more = messages.len() >= crate::discord::rest::MESSAGES_PAGE_SIZE as usize;
                // Igual que en `ChannelMessages`: la API los devuelve del
                // más nuevo al más viejo, los damos vuelta para que
                // queden en orden cronológico antes de anteponerlos a lo
                // que ya teníamos.
                if let Some(friend) = self
                    .friends
                    .iter_mut()
                    .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                {
                    let mut older: Vec<ChatMessage> = messages
                        .iter()
                        .rev()
                        .map(|m| ChatMessage::from_discord(m, &my_id))
                        .collect();
                    older.append(&mut friend.messages);
                    friend.messages = older;
                    friend.loading_more = false;
                    friend.has_more = has_more;
                    return;
                }
                let mut to_request: Option<(String, Vec<String>)> = None;
                'outer: for server in &mut self.servers {
                    let names = NameResolver::new(&server.member_info, &server.roles);
                    for category in &mut server.categories {
                        for channel in &mut category.channels {
                            if channel.channel_id.as_deref() == Some(channel_id.as_str()) {
                                let mut older: Vec<ChatMessage> = messages
                                    .iter()
                                    .rev()
                                    .map(|m| ChatMessage::from_discord_in(m, &my_id, Some(names)))
                                    .collect();
                                older.append(&mut channel.messages);
                                channel.messages = older;
                                channel.loading_more = false;
                                channel.has_more = has_more;
                                to_request = Some((server.guild_id.clone(), message_author_ids(&messages)));
                                break 'outer;
                            }
                        }
                    }
                }
                if let Some((guild_id, user_ids)) = to_request {
                    self.request_members(&guild_id, user_ids);
                }
            }
            AppEvent::AroundChannelMessages { channel_id, target_id, jump, messages } => {
                use crate::lib::notifications::{dm_sort_key, snowflake};
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                // Chronological (la API los manda del más nuevo al más viejo).
                let mut sorted: Vec<&crate::discord::models::GatewayMessage> = messages.iter().collect();
                sorted.sort_by_key(|m| snowflake(&m.id));
                let target_n = snowflake(&target_id);
                let older = sorted.iter().filter(|m| snowflake(&m.id) < target_n).count();
                let newer = sorted.iter().filter(|m| snowflake(&m.id) > target_n).count();
                let newest_n = sorted.last().map(|m| snowflake(&m.id)).unwrap_or(0);
                // Si no hay nada más viejo que el mensaje pedido, ya se llegó al
                // principio del canal.
                let has_more = older > 0;
                if jump {
                    self.pending_jump = Some(target_id.clone());
                }
                if let Some(friend) = self
                    .friends
                    .iter_mut()
                    .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                {
                    // En un DM se sabe cuál es el último mensaje: si la ventana
                    // ya lo incluye, no hay nada más abajo que pedir.
                    let latest_known = self
                        .dms
                        .iter()
                        .find(|dm| dm.id == channel_id)
                        .map(dm_sort_key)
                        .unwrap_or(0);
                    let at_latest = latest_known != 0 && newest_n >= latest_known;
                    friend.messages = sorted
                        .iter()
                        .map(|m| ChatMessage::from_discord(m, &my_id))
                        .collect();
                    friend.loaded = true;
                    friend.loading = false;
                    friend.loading_more = false;
                    friend.loading_newer = false;
                    friend.has_more = has_more;
                    friend.has_newer = newer > 0 && !at_latest;
                    return;
                }
                let mut to_request: Option<(String, Vec<String>)> = None;
                'outer: for server in &mut self.servers {
                    let names = NameResolver::new(&server.member_info, &server.roles);
                    for category in &mut server.categories {
                        for channel in &mut category.channels {
                            if channel.channel_id.as_deref() == Some(channel_id.as_str()) {
                                channel.messages = sorted
                                    .iter()
                                    .map(|m| ChatMessage::from_discord_in(m, &my_id, Some(names)))
                                    .collect();
                                channel.loaded = true;
                                channel.loading = false;
                                channel.loading_more = false;
                                channel.loading_newer = false;
                                channel.has_more = has_more;
                                // En un canal de server no sabemos cuál es el último
                                // mensaje: si hubo algo más nuevo, se sigue con `after`
                                // (si ya estábamos al final, ese pedido vuelve vacío y
                                // apaga la bandera).
                                channel.has_newer = newer > 0;
                                to_request = Some((server.guild_id.clone(), message_author_ids(&messages)));
                                break 'outer;
                            }
                        }
                    }
                }
                if let Some((guild_id, user_ids)) = to_request {
                    self.request_members(&guild_id, user_ids);
                }
            }
            AppEvent::NewerChannelMessages { channel_id, messages } => {
                use crate::lib::notifications::snowflake;
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let mut sorted: Vec<&crate::discord::models::GatewayMessage> = messages.iter().collect();
                sorted.sort_by_key(|m| snowflake(&m.id));
                // Una página completa = puede haber más; una corta = ya se llegó
                // al mensaje más nuevo del canal.
                let has_newer = messages.len() >= crate::discord::uwu_rest::MESSAGES_WINDOW_SIZE as usize;
                if let Some(friend) = self
                    .friends
                    .iter_mut()
                    .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                {
                    for m in &sorted {
                        if friend.messages.iter().any(|x| !x.id.is_empty() && x.id == m.id) {
                            continue;
                        }
                        reconcile_or_push(&mut friend.messages, ChatMessage::from_discord(m, &my_id));
                    }
                    friend.loading_newer = false;
                    friend.has_newer = has_newer;
                    return;
                }
                let mut to_request: Option<(String, Vec<String>)> = None;
                'outer: for server in &mut self.servers {
                    let names = NameResolver::new(&server.member_info, &server.roles);
                    for category in &mut server.categories {
                        for channel in &mut category.channels {
                            if channel.channel_id.as_deref() == Some(channel_id.as_str()) {
                                for m in &sorted {
                                    if channel.messages.iter().any(|x| !x.id.is_empty() && x.id == m.id) {
                                        continue;
                                    }
                                    reconcile_or_push(
                                        &mut channel.messages,
                                        ChatMessage::from_discord_in(m, &my_id, Some(names)),
                                    );
                                }
                                channel.loading_newer = false;
                                channel.has_newer = has_newer;
                                to_request = Some((server.guild_id.clone(), message_author_ids(&messages)));
                                break 'outer;
                            }
                        }
                    }
                }
                if let Some((guild_id, user_ids)) = to_request {
                    self.request_members(&guild_id, user_ids);
                }
            }
            AppEvent::MessageWindowFailed { channel_id } => {
                self.pending_jump = None;
                self.newer_retry_after = Some(Instant::now() + std::time::Duration::from_secs(5));
                if let Some(friend) = self
                    .friends
                    .iter_mut()
                    .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                {
                    friend.loading = false;
                    friend.loading_newer = false;
                } else if let Some(channel) = self
                    .servers
                    .iter_mut()
                    .find_map(|s| s.channel_by_id_mut(&channel_id))
                {
                    channel.loading = false;
                    channel.loading_newer = false;
                }
            }
            AppEvent::GuildVoiceStates { guild_id, states } => {
                // Foto completa (READY_SUPPLEMENTAL / GUILD_CREATE): reemplaza
                // lo que hubiera, así llegue por dos caminos no se duplica.
                let to_fetch: Vec<String> =
                    if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                        // Los que ya estaban sin nombre vienen del `READY` y ya
                        // tienen su pedido REST en camino: no se repite.
                        let in_flight = server.unknown_voice_user_ids();
                        server.apply_voice_snapshot(&states);
                        server
                            .unknown_voice_user_ids()
                            .into_iter()
                            .filter(|id| !in_flight.contains(id))
                            .collect()
                    } else {
                        Vec::new()
                    };
                for user_id in to_fetch {
                    self.fetch_unknown_voice_user(&user_id);
                }
            }
            AppEvent::VoiceStateUpdate(state) => {
                let Some(guild_id) = state.guild_id.clone() else { return };
                let mut needs_fetch = false;
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    // Primero lo sacamos de dondequiera que estuviera
                    // conectado antes (si estaba), y después, si sigue en
                    // voz (`channel_id` viene `Some`), lo agregamos al
                    // canal nuevo. El conectado se arma con lo que ya
                    // sabemos del miembro (apodo, nombre, avatar): solo si
                    // ni así hay nombre se va a buscar por REST.
                    server.remove_voice_member(&state.user_id);
                    if let Some(channel_id) = state.channel_id.clone() {
                        let occupant = server.resolved_voice_occupant(&state);
                        needs_fetch = !occupant.known;
                        if let Some(channel) = server.channel_by_id_mut(&channel_id) {
                            channel.voice_members.push(occupant);
                        }
                    }
                }
                if needs_fetch {
                    self.fetch_unknown_voice_user(&state.user_id);
                }
            }
            AppEvent::UserFetched(user) => {
                for server in &mut self.servers {
                    // El pedido REST trae el nombre global; si en este server
                    // la persona tiene apodo, ese es el que se muestra.
                    let name = server
                        .member_info
                        .get(&user.id)
                        .and_then(|info| info.nick.clone())
                        .unwrap_or_else(|| user.display_name().to_string());
                    for category in &mut server.categories {
                        for channel in &mut category.channels {
                            for occupant in &mut channel.voice_members {
                                if occupant.user_id == user.id && !occupant.known {
                                    occupant.name = name.clone();
                                    occupant.avatar_url = user.avatar_url();
                                    occupant.known = true;
                                }
                            }
                        }
                    }
                }
            }
            AppEvent::OwnActivities(activities) => {
                self.own_activities = activities;
            }
            AppEvent::MessageAck { channel_id, message_id, mention_count } => {
                self.apply_remote_ack(&channel_id, &message_id, mention_count);
            }
            AppEvent::ReactionAdd {
                channel_id,
                message_id,
                user_id,
                emoji,
            } => {
                let is_me = self.me.as_ref().is_some_and(|m| m.id == user_id);
                if let Some(msg) = self.find_message_mut(&channel_id, &message_id) {
                    apply_remote_reaction(msg, emoji.kind(), is_me, true);
                }
            }
            AppEvent::ReactionRemove {
                channel_id,
                message_id,
                user_id,
                emoji,
            } => {
                let is_me = self.me.as_ref().is_some_and(|m| m.id == user_id);
                if let Some(msg) = self.find_message_mut(&channel_id, &message_id) {
                    apply_remote_reaction(msg, emoji.kind(), is_me, false);
                }
            }
            AppEvent::UserActionDone { user_id, action, error, needs_stranger_confirm } => {
                use crate::discord::UserAction;
                match error {
                    Some(error) => {
                        // Solo si Discord contestó 400 / código 80013 al aceptar con
                        // `confirm_stranger_request: false`: se pide confirmación y,
                        // si la dan, se reintenta con `true`. Cualquier otro error
                        // sale como toast normal.
                        if needs_stranger_confirm
                            && matches!(action, UserAction::AcceptFriend { confirm: false })
                        {
                            self.confirm_accept_friend(user_id.clone(), error);
                        } else {
                            self.push_toast(
                                ToastKind::Warning,
                                "No se pudo completar la acción",
                                error,
                            );
                        }
                    }
                    None => {
                        let (title, text): (&str, String) = match &action {
                            UserAction::AddFriend | UserAction::AcceptFriend { .. } => {
                                // Si ella ya había mandado la solicitud, ahora son amigos.
                                let became_friends =
                                    self.relationship_kinds.get(&user_id) == Some(&3);
                                let kind = if became_friends { 1 } else { 4 };
                                self.relationship_kinds.insert(user_id.clone(), kind);
                                if became_friends {
                                    // La solicitud pasa de "Pendiente" a la lista de
                                    // amigos sin esperar al `RELATIONSHIP_ADD`.
                                    let accepted = self.take_pending(&user_id);
                                    if !self.friends.iter().any(|f| f.user_id == user_id) {
                                        if let Some(request) = accepted {
                                            self.friends.push(Friend::from_user(&request.user));
                                        }
                                    }
                                    self.set_friend_flag(&user_id, true);
                                    ("Amigos", "Ahora son amigos.".to_string())
                                } else {
                                    ("Amigos", "Solicitud de amistad enviada.".to_string())
                                }
                            }
                            UserAction::RemoveFriend => {
                                let previous = self.relationship_kinds.remove(&user_id);
                                self.pending_requests.retain(|p| p.user.id != user_id);
                                self.set_friend_flag(&user_id, false);
                                let text = match previous {
                                    Some(3) => "Solicitud rechazada.",
                                    Some(4) => "Solicitud cancelada.",
                                    _ => "Listo.",
                                };
                                ("Amigos", text.to_string())
                            }
                            UserAction::Block => {
                                self.relationship_kinds.insert(user_id.clone(), 2);
                                self.pending_requests.retain(|p| p.user.id != user_id);
                                self.set_friend_flag(&user_id, false);
                                ("Bloqueado", "Ya no puede escribirte ni llamarte.".to_string())
                            }
                            UserAction::Unblock => {
                                self.relationship_kinds.remove(&user_id);
                                ("Desbloqueado", "Listo.".to_string())
                            }
                            UserAction::Ignore => {
                                self.ignored_users.insert(user_id.clone());
                                ("Ignorado", "Sus mensajes quedan ocultos.".to_string())
                            }
                            UserAction::Unignore => {
                                self.ignored_users.remove(&user_id);
                                ("Ignorado", "Dejaste de ignorarlo.".to_string())
                            }
                            UserAction::SetNote(note) => (
                                "Nota",
                                if note.is_empty() {
                                    "Nota borrada.".to_string()
                                } else {
                                    "Nota guardada.".to_string()
                                },
                            ),
                            UserAction::InviteToServer { .. } => {
                                ("Invitación", "Invitación enviada por mensaje directo.".to_string())
                            }
                        };
                        self.push_toast(ToastKind::Success, title, text);
                    }
                }
                self.menu_ctx_rev += 1;
            }
            AppEvent::FriendRequestSent { username, error } => {
                self.add_friend_busy = false;
                self.add_friend_status = Some(match error {
                    None => {
                        self.add_friend_input.clear();
                        (true, format!("¡Listo! Tu solicitud de amistad para {username} fue enviada."))
                    }
                    Some(error) => (false, error),
                });
            }
            AppEvent::RelationshipAdd(relationship) => {
                self.apply_relationship_add(*relationship);
            }
            AppEvent::RelationshipRemove { user_id } => {
                self.relationship_kinds.remove(&user_id);
                self.pending_requests.retain(|p| p.user.id != user_id);
                self.set_friend_flag(&user_id, false);
                self.menu_ctx_rev += 1;
            }
            AppEvent::UserNote { user_id, note } => {
                if let Some((id, name)) = self.pending_note.take() {
                    if id == user_id {
                        self.show_note_dialog(id, name, note);
                    } else {
                        self.pending_note = Some((id, name));
                    }
                }
            }
            AppEvent::Error(message) => {
                // Un 401 de cualquier pedido REST llega acá envuelto en el
                // texto del error: es la sesión, no un fallo común.
                if message.contains(crate::discord::INVALID_SESSION_MESSAGE) {
                    self.invalidate_session(crate::discord::INVALID_SESSION_MESSAGE.to_owned());
                    return;
                }
                // Si todavía no habíamos llegado a `Connected`, el error es
                // del login/la conexión inicial y va a la pantalla de
                // login; si ya estábamos adentro, es solo un toast (por
                // ejemplo un pedido de REST puntual que falló) y no hace
                // falta patearlo de vuelta a Login.
                if matches!(self.auth, AuthStatus::Connected) {
                    // No sabemos a qué canal/DM correspondía el pedido que
                    // falló (el evento solo trae un mensaje de texto), así
                    // que como salvavidas apagamos cualquier spinner de
                    // carga que hubiera quedado prendido en cualquier lado
                    // — mejor un placeholder de "no hay mensajes" de más
                    // que un spinner girando para siempre por un pedido
                    // que ya no va a volver.
                    for friend in &mut self.friends {
                        friend.loading = false;
                        friend.loading_more = false;
                    }
                    for server in &mut self.servers {
                        for category in &mut server.categories {
                            for channel in &mut category.channels {
                                channel.loading = false;
                                channel.loading_more = false;
                            }
                        }
                    }
                    self.push_toast(ToastKind::Warning, "Discord", message);
                } else {
                    self.auth = AuthStatus::Failed(message);
                }
            }
            AppEvent::GatewayDisconnected(reason) => {
                // No fatal — `gateway::run` ya está reintentando solo en
                // segundo plano. Esto es solo para que se note, en vez de
                // que la UI se quede muda mientras reconecta (que era
                // exactamente el problema: antes esto no mandaba nada).
                let mut chars = reason.chars();
                let reason = match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => reason,
                };
                self.gateway_disconnected = true;
                self.push_toast(ToastKind::Warning, "Discord", format!("{reason}. Reconectando…"));
            }
            AppEvent::GatewayReconnected => {
                self.gateway_disconnected = false;
                self.push_toast(ToastKind::Success, "Discord", "Conexión restablecida");
            }
            AppEvent::SessionInvalid(message) => self.invalidate_session(message),
            AppEvent::GatewayLost(message) => self.connection_lost(message),
            AppEvent::GatewayCommands(commands) => {
                self.gateway_commands = Some(commands);
                // Conexión nueva: no hereda ninguna suscripción anterior.
                self.member_subscription = None;
                self.pending_member_subscription = None;
                self.subscribe_member_list();
            }
            AppEvent::GuildRoles { guild_id, roles } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.apply_roles(roles);
                }
                self.fix_hidden_current_channel();
            }
            AppEvent::SelfGuildRoles { guild_id, roles } => {
                let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.access_ctx.my_id = my_id;
                    server.set_my_roles(roles);
                }
                self.fix_hidden_current_channel();
            }
            AppEvent::GuildMembers { guild_id, members } => {
                let own_id = self.me.as_ref().map(|m| m.id.clone());
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    let mut changed = false;
                    // Si es un cambio de roles de la PROPIA cuenta (te
                    // dieron o sacaron un rol), cambia qué canales ves.
                    if let Some(me) = own_id.as_deref() {
                        if let Some(mine) = members.iter().find(|m| m.user.id == me) {
                            server.access_ctx.my_id = me.to_owned();
                            server.set_my_roles(mine.roles.clone());
                        }
                    }
                    for member in &members {
                        // Apodo/roles para el chat, y nombre/avatar para los
                        // conectados a voz que solo llegaron como `user_id`.
                        changed |= server.learn_member(&member.user, member.nick.as_deref(), &member.roles);
                    }
                    // Los mensajes ya dibujados se quedaron con el nombre
                    // global: ahora que llegó el apodo/rol, se actualizan.
                    if changed {
                        server.refresh_author_names();
                    }
                }
            }
            // Un admin creó/modificó un canal: se rearman las categorías. Los
            // índices pueden correrse, así que se vuelve a ubicar el canal
            // abierto por su id.
            AppEvent::ChannelUpsert { guild_id, channel } => {
                let previous = self.current_channel_id();
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.upsert_channel(channel);
                }
                self.restore_current_channel(&guild_id, previous);
            }
            AppEvent::ChannelDelete { guild_id, channel_id } => {
                let previous = self.current_channel_id();
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.remove_channel(&channel_id);
                }
                self.restore_current_channel(&guild_id, previous);
            }
            // Cambió un rol (sobre todo sus permisos): cambia qué canales se
            // ven, y el color de los nombres.
            AppEvent::GuildRoleUpsert { guild_id, role } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.upsert_role(role);
                }
                self.fix_hidden_current_channel();
            }
            AppEvent::GuildRoleDelete { guild_id, role_id } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                    server.remove_role(&role_id);
                }
                self.fix_hidden_current_channel();
            }
            AppEvent::ForumPosts { channel_id, offset, by_creation, page } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.has_channel(&channel_id)) {
                    server.apply_forum_page(&channel_id, offset, by_creation, page);
                }
            }
            AppEvent::ForumPostsFailed { channel_id } => {
                for server in &mut self.servers {
                    if let Some(channel) = server.channel_by_id_mut(&channel_id) {
                        channel.forum.loading = false;
                    }
                }
            }
            AppEvent::ForumPostCreateFailed { forum_id } => {
                for server in &mut self.servers {
                    if let Some(channel) = server.channel_by_id_mut(&forum_id) {
                        channel.forum.creating = false;
                    }
                }
            }
            AppEvent::ForumPostCreated { forum_id, thread } => {
                let Some(server_index) = self.servers.iter().position(|s| s.has_channel(&forum_id))
                else {
                    return;
                };
                let title = thread.name.clone().unwrap_or_else(|| "Publicación".to_string());
                let viewing = matches!(self.screen, Screen::Server(i) if i == server_index);
                let (thread_category, thread_channel) = {
                    let server = &mut self.servers[server_index];
                    if let Some(forum_channel) = server.channel_by_id_mut(&forum_id) {
                        let forum = &mut forum_channel.forum;
                        forum.creating = false;
                        forum.composing = false;
                        forum.draft_title.clear();
                        forum.draft_body.clear();
                        forum.draft_tags.clear();
                    }
                    server.upsert_thread(&thread);
                    server.open_thread(&thread.id, &title, Some(forum_id.clone()))
                };
                // Se abre el post recién creado, como el cliente oficial.
                if viewing {
                    self.open_channel(thread_category, thread_channel);
                }
            }
            AppEvent::ThreadUpsert(thread) => {
                let Some(parent) = thread.parent_id.clone() else { return };
                if let Some(server) = self.servers.iter_mut().find(|s| s.has_channel(&parent)) {
                    server.upsert_thread(&thread);
                }
            }
            AppEvent::ThreadDelete { parent_id, thread_id } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.has_channel(&parent_id)) {
                    server.remove_thread(&parent_id, &thread_id);
                }
            }
            AppEvent::ModalCreate(request) => {
                self.pending_buttons.clear();
                self.component_modal = Some(ComponentModal::new(*request));
            }
            AppEvent::InteractionFailed { message } => {
                self.pending_buttons.clear();
                self.push_toast(ToastKind::Warning, "No se pudo completar la acción", message);
            }
            AppEvent::InteractionSucceeded => {
                self.pending_buttons.clear();
            }
            AppEvent::ThreadOpened { thread } => {
                let title = thread.name.clone().unwrap_or_else(|| "Hilo".to_string());
                let owner = thread.owner_id.clone();
                if let Some(parent) = thread.parent_id.clone() {
                    if let Some(server) = self.servers.iter_mut().find(|s| s.has_channel(&parent)) {
                        server.upsert_thread(&thread);
                    }
                }
                self.open_thread_panel(&thread.id, &title, owner);
            }
            AppEvent::SlashCatalogLoaded { key, catalog } => {
                self.slash_pending.remove(&key);
                self.slash_catalogs.insert(key, crate::discord::slash::CachedCatalog::new(catalog));
                // Pocos catálogos en memoria: el de los servers que se dejaron
                // de visitar se va (se vuelve a pedir si hace falta).
                while self.slash_catalogs.len() > 12 {
                    let oldest = self
                        .slash_catalogs
                        .iter()
                        .min_by_key(|(_, cached)| cached.fetched)
                        .map(|(key, _)| key.clone());
                    match oldest {
                        Some(key) => self.slash_catalogs.remove(&key),
                        None => break,
                    };
                }
            }
            AppEvent::SlashCatalogFailed { key } => {
                self.slash_pending.remove(&key);
                self.slash_catalogs.insert(key, crate::discord::slash::CachedCatalog::failed());
            }
            AppEvent::PresenceUpdate(event) => self.apply_presence_event(&event),
            AppEvent::PresenceSnapshot(events) => {
                for event in &events {
                    self.apply_presence_event(event);
                }
            }
            AppEvent::MemberListUpdate(update) => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == update.guild_id) {
                    server.apply_member_list_update(&update);
                }
            }
            AppEvent::PasswordLoginToken(token) => {
                // Mismo bookkeeping que `AppEvent::LoggedIn` (persistir el
                // token para saltarse el login la próxima vez) más, a
                // diferencia de ese caso, arrancar el Gateway acá mismo:
                // el login por QR ya lo hace en el propio hilo de fondo
                // (`spawn_login_flow`) apenas consigue el token, pero acá
                // `password_auth::spawn` puede quedar esperando un 2FA en
                // el medio, así que quien sigue con el Gateway es este
                // handler, una vez que el token está confirmado de
                // verdad.
                self.auth = AuthStatus::Connecting;
                self.connection_steps.clear();
                self.remember_login(&token);
                self.discord_token = Some(token.clone());
                self.login_email.clear();
                self.login_password.clear();
                self.login_mfa_code.clear();
                self.login_sms_sent = false;
                self.pending_mfa = None;
                if let Some(tx) = self.event_tx.clone() {
                    crate::discord::spawn_gateway_with_token(token, tx);
                }
            }
            AppEvent::PasswordMfaRequired(challenge) => {
                self.pending_mfa = Some(challenge);
                self.login_sms_sent = false;
                self.login_mfa_code.clear();
                self.login_error = None;
                self.auth = AuthStatus::PasswordMfaRequired;
            }
            AppEvent::PasswordMfaSmsSent(_phone) => {
                self.login_sms_sent = true;
            }
            AppEvent::PasswordLoginBlocked(actions) => {
                let list = actions.join(", ");
                self.login_error = Some(format!(
                    "Discord pide resolver esto antes de dejar entrar: {list}. Hacelo desde discord.com en un navegador y probá de nuevo."
                ));
                self.pending_mfa = None;
                self.auth = AuthStatus::PasswordForm;
            }
            AppEvent::PasswordLoginFailed(message) => {
                self.login_error = Some(message);
                // Si falló en el paso de 2FA (el ticket sigue siendo
                // válido), volver ahí en vez de mandar a escribir
                // usuario/contraseña de nuevo desde cero.
                self.auth = if self.pending_mfa.is_some() {
                    AuthStatus::PasswordMfaRequired
                } else {
                    AuthStatus::PasswordForm
                };
            }
            AppEvent::UserProfileFetched { user_id, profile } => {
                // Si mientras tanto se cerró la tarjeta, o se abrió la de
                // otra persona, esta respuesta ya llegó tarde — se
                // descarta sin tocar nada.
                if let Some(popup) = &mut self.dm_profile {
                    if popup.user_id == user_id {
                        popup.loading = false;
                        popup.failed = false;
                        popup.profile = Some(profile.clone());
                    }
                }
                if let Some(popup) = &mut self.profile_popup {
                    if popup.user_id == user_id {
                        popup.loading = false;
                        popup.failed = false;
                        popup.profile = Some(profile);
                    }
                }
            }
            AppEvent::UserProfileFailed { user_id } => {
                if let Some(popup) = &mut self.dm_profile {
                    if popup.user_id == user_id {
                        popup.loading = false;
                        popup.failed = true;
                    }
                }
                if let Some(popup) = &mut self.profile_popup {
                    if popup.user_id == user_id {
                        popup.loading = false;
                        popup.failed = true;
                    }
                }
            }
            // Ya se lo reenviamos al runtime arriba del `match`
            // (`forward_app_event`); acá no hace falta tocar nada más de
            // `App` — el endpoint/token de voz no se muestran en ningún
            // lado de la UI.
            AppEvent::VoiceServerUpdate(_) => {}
            // Go Live: los tres datos para conectarse al servidor de media
            // del stream llegan en dos eventos, en cualquier orden.
            AppEvent::StreamCreate(create) => {
                let key = create.stream_key.clone();
                if self.watching_stream.as_ref().is_some_and(|w| w.stream_key == key) {
                    self.stream_sessions.entry(key.clone()).or_default().rtc_server_id =
                        create.rtc_server_id.clone();
                    self.try_start_stream_watch(&key);
                }
            }
            AppEvent::StreamServerUpdate(update) => {
                let key = update.stream_key.clone();
                if self.watching_stream.as_ref().is_some_and(|w| w.stream_key == key) {
                    let pending = self.stream_sessions.entry(key.clone()).or_default();
                    pending.endpoint = update.endpoint.clone();
                    pending.token = update.token.clone();
                    self.try_start_stream_watch(&key);
                }
            }
            AppEvent::StreamUpdate(_) => {}
            AppEvent::StreamDelete(delete) => {
                self.stream_sessions.remove(&delete.stream_key);
                if self.watching_stream.as_ref().is_some_and(|w| w.stream_key == delete.stream_key) {
                    let name = self
                        .watching_stream
                        .as_ref()
                        .map(|w| w.owner_name.clone())
                        .unwrap_or_default();
                    // El stream ya no existe: no hace falta avisarle nada al
                    // Gateway, solo soltar la conexión y la textura.
                    self.watching_stream = None;
                    self.push_toast(
                        ToastKind::Info,
                        "Stream terminado",
                        format!("{name} dejó de transmitir"),
                    );
                }
            }
            AppEvent::StreamWatchStatus { stream_key, status, message } => {
                if let Some(watched) = self.watching_stream.as_mut()
                    && watched.stream_key == stream_key
                {
                    watched.status = status;
                    watched.message = message.clone();
                    if status == StreamWatchStatus::Failed {
                        // Se suelta la conexión (ya murió) pero el visor queda
                        // mostrando el error hasta que se cierre.
                        watched.handle = None;
                        self.push_toast(
                            ToastKind::Warning,
                            "No se pudo ver el stream",
                            message.unwrap_or_else(|| "Error de conexión".to_string()),
                        );
                    }
                }
            }
            AppEvent::VoiceConnectionStatusChanged { status, message, .. } => {
                // Al cortar la conexión (`Disconnected`/`Failed`) limpiamos
                // también lo que habíamos pedido, para que la barra de
                // llamada y el botón "Unirse" vuelvan a su estado normal en
                // vez de quedarse mostrando una llamada que ya no existe.
                if matches!(status, VoiceConnectionStatus::Disconnected | VoiceConnectionStatus::Failed) {
                    self.voice_target = None;
                }
                if status == VoiceConnectionStatus::Failed
                    && let Some(message) = &message
                {
                    self.push_toast(ToastKind::Warning, "No se pudo conectar a voz", message.clone());
                }
                self.voice_connection_status = Some(status);
                self.voice_connection_message = message;
            }
            // Ya actualizado en `apply_voice_wire_event` arriba del
            // `match` (afecta a `self.voice.states`, no a nada más de
            // `App`).
            AppEvent::VoiceSpeakingUpdate { .. } => {}
            AppEvent::VoiceAudioSourcesApplyFailed { message, .. } => {
                self.push_toast(
                    ToastKind::Warning,
                    "Dispositivo de audio no disponible",
                    message,
                );
            }
        }
    }

    // ---- Notificaciones: orden de DMs, contador de menciones y avisos ----

    /// Ordena `self.dms` como el cliente oficial: arriba la conversación con
    /// el mensaje más reciente (los DMs sin mensajes, por fecha de creación).
    /// Es seguro reordenar en cualquier momento: `Screen::Dm` indexa
    /// `self.friends`, no `self.dms`.
    fn sort_dms(&mut self) {
        self.dms
            .sort_by_key(|dm| std::cmp::Reverse(crate::lib::notifications::dm_sort_key(dm)));
    }

    /// Si `msg` es de un DM de la lista, actualiza su último mensaje y lo
    /// sube al tope. Corre también con los mensajes propios (el eco del
    /// Gateway), así mandar un mensaje también sube la conversación.
    fn bump_dm_order(&mut self, msg: &crate::discord::models::GatewayMessage) {
        let Some(dm) = self.dms.iter_mut().find(|dm| dm.id == msg.channel_id) else { return };
        let newest = crate::lib::notifications::snowflake(&msg.id);
        if newest >= crate::lib::notifications::dm_sort_key(dm) {
            dm.last_message_id = Some(msg.id.clone());
        }
        self.sort_dms();
    }

    /// Menciones sin leer de todo (DMs + servers).
    pub fn total_mentions(&self) -> u32 {
        self.unread_mentions.values().sum()
    }

    /// Menciones sin leer en los DMs (badge del logo/Home en el rail).
    pub fn dm_mentions_total(&self) -> u32 {
        self.dms
            .iter()
            .map(|dm| self.unread_mentions.get(&dm.id).copied().unwrap_or(0))
            .sum::<u32>()
    }

    /// Menciones sin leer de `servers[server_index]`.
    pub fn server_mentions(&self, server_index: usize) -> u32 {
        self.servers
            .get(server_index)
            .map(|s| self.guild_mentions.get(&s.guild_id).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Id real del canal/DM que se está mirando ahora mismo, si hay uno.
    fn viewed_channel_id(&self) -> Option<String> {
        match self.screen {
            Screen::Dm(i) => self.friends.get(i)?.dm_channel_id.clone(),
            Screen::Server(i) => {
                let (category, channel) = self.current_channel;
                self.servers.get(i)?.channel(category, channel)?.channel_id.clone()
            }
            _ => None,
        }
    }

    fn guild_of_channel(&self, channel_id: &str) -> Option<String> {
        self.servers
            .iter()
            .find(|s| s.has_channel(channel_id))
            .map(|s| s.guild_id.clone())
            .filter(|g| !g.is_empty())
    }

    /// Marca como leído `channel_id`: baja su contador y el del server, y saca
    /// sus tarjetas de notificación.
    fn clear_mentions(&mut self, channel_id: &str) {
        self.pending_mentions.remove(channel_id);
        if let Some(count) = self.unread_mentions.remove(channel_id) {
            if let Some(guild_id) = self.guild_of_channel(channel_id) {
                let remaining = self
                    .guild_mentions
                    .get(&guild_id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(count);
                if remaining == 0 {
                    self.guild_mentions.remove(&guild_id);
                } else {
                    self.guild_mentions.insert(guild_id, remaining);
                }
            }
        }
        self.in_app_notifications
            .retain(|n| n.target.channel_id() != channel_id);
    }

    /// Cada frame: lo que se está mirando con la ventana en foco cuenta como
    /// leído. Hacerlo acá (y no en cada función que cambia de pantalla/canal)
    /// cubre todos los caminos: click en la lista, notificación, teclado...
    fn clear_viewed_mentions(&mut self) {
        if !self.window_focused {
            return;
        }
        let Some(channel_id) = self.viewed_channel_id() else { return };
        self.clear_mentions(&channel_id);
    }

    fn note_mention(&mut self, channel_id: &str, guild_id: Option<&str>) {
        *self.unread_mentions.entry(channel_id.to_string()).or_insert(0) += 1;
        if let Some(guild_id) = guild_id.filter(|g| !g.is_empty()) {
            *self.guild_mentions.entry(guild_id.to_string()).or_insert(0) += 1;
        }
    }

    /// Deja `channel_id` con exactamente `count` menciones sin leer (y
    /// ajusta el total del server). `0` lo marca como leído del todo.
    fn set_mention_count(&mut self, channel_id: &str, count: u32) {
        if count == 0 {
            self.clear_mentions(channel_id);
            return;
        }
        let old = self.unread_mentions.insert(channel_id.to_string(), count).unwrap_or(0);
        if let Some(guild_id) = self.guild_of_channel(channel_id) {
            let total = self.guild_mentions.get(&guild_id).copied().unwrap_or(0);
            let total = total.saturating_sub(old) + count;
            self.guild_mentions.insert(guild_id, total);
        }
    }

    /// Reemplaza los no leídos locales por los que Discord tiene guardados
    /// en el `READY` (`read_state`). Es lo que hace los no leídos
    /// persistentes: el servidor los recuerda entre reinicios mientras este
    /// cliente mande el ack al leer (ver `ack_viewed_channel`).
    ///
    /// Sin `read_state` en el payload (formato inesperado) no se toca nada,
    /// para no borrar contadores válidos de una sesión ya en marcha.
    fn apply_ready_read_state(&mut self, raw: &serde_json::Value) {
        let entries = crate::discord::models::parse_read_state(raw);
        if entries.is_empty() && raw.is_null() {
            return;
        }
        self.unread_mentions.clear();
        self.guild_mentions.clear();
        self.pending_mentions.clear();
        self.acked_messages.clear();
        for entry in entries {
            if let Some(last) = entry.last_message_id {
                self.acked_messages.insert(entry.channel_id.clone(), last);
            }
            if entry.mention_count > 0 {
                self.pending_mentions.insert(entry.channel_id, entry.mention_count);
            }
        }
        self.apply_pending_mentions();
    }

    /// Pasa a `unread_mentions`/`guild_mentions` las menciones pendientes
    /// cuyo canal ya se conoce (un DM, o un canal de un server ya cargado).
    /// Respeta lo silenciado y las solicitudes de mensajes/spam, igual que
    /// `notify_incoming` con los mensajes en vivo. Las de canales todavía
    /// desconocidos quedan pendientes.
    fn apply_pending_mentions(&mut self) {
        if self.pending_mentions.is_empty() {
            return;
        }
        let pending: Vec<(String, u32)> = self
            .pending_mentions
            .iter()
            .map(|(channel, count)| (channel.clone(), *count))
            .collect();
        for (channel_id, count) in pending {
            // `Some(oculto)` si es un DM (oculto = solicitud de mensajes o
            // spam). Se calcula aparte para no dejar `self.dms` prestado
            // mientras se llama a métodos que toman `&mut self`.
            let dm_hidden = self
                .dms
                .iter()
                .find(|dm| dm.id == channel_id)
                .map(|dm| dm.is_message_request || dm.is_spam);
            if let Some(hidden) = dm_hidden {
                let allowed = self.mute_rules.passes(
                    None,
                    &channel_id,
                    crate::lib::notifications::Mention::default(),
                );
                self.pending_mentions.remove(&channel_id);
                if !hidden && allowed {
                    self.set_mention_count(&channel_id, count);
                }
                continue;
            }
            let Some(guild_id) = self.guild_of_channel(&channel_id) else { continue };
            // Discord solo cuenta menciones reales en los canales de server,
            // así que alcanza con tratarlo como mención directa.
            let allowed = self.mute_rules.passes(
                Some(&guild_id),
                &channel_id,
                crate::lib::notifications::Mention { direct: true, ..Default::default() },
            );
            self.pending_mentions.remove(&channel_id);
            if allowed {
                self.set_mention_count(&channel_id, count);
            }
        }
    }

    /// Canal abierto ahora mismo y su mensaje más nuevo ya cargado. `None`
    /// si no hay un canal real a la vista, si todavía está cargando el
    /// historial, o si no tiene mensajes con id real (demo).
    fn viewed_latest_message(&self) -> Option<(String, String)> {
        let (channel_id, messages, loading) = match self.screen {
            Screen::Dm(i) => {
                let friend = self.friends.get(i)?;
                (friend.dm_channel_id.clone()?, &friend.messages, friend.loading || friend.has_newer)
            }
            Screen::Server(i) => {
                let (category, channel) = self.current_channel;
                let channel = self.servers.get(i)?.channel(category, channel)?;
                (channel.channel_id.clone()?, &channel.messages, channel.loading || channel.has_newer)
            }
            _ => return None,
        };
        // Con `has_newer` la lista termina en el medio del historial: no se
        // marca como leído hasta llegar de verdad al último mensaje.
        if loading {
            return None;
        }
        let latest = messages.last()?.id.clone();
        (!latest.is_empty()).then_some((channel_id, latest))
    }

    /// Cada frame: si se está mirando un canal con la ventana en foco y hay
    /// un mensaje más nuevo que el último que Discord tiene como leído, le
    /// manda el ack (`POST .../ack`). Sin esto el cliente solo borraba el
    /// contador en memoria y, al reiniciar, Discord seguía contando esos
    /// mensajes como no leídos.
    ///
    /// Cubre también los mensajes que llegan con el chat abierto (no suman
    /// mención, pero sí hay que marcarlos leídos). Se espera un poco entre
    /// acks del mismo canal para no saturar la API si llegan muchos seguidos.
    fn ack_viewed_channel(&mut self) {
        use crate::lib::notifications::snowflake;
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

        if !self.window_focused {
            return;
        }
        let Some(token) = self.discord_token.clone() else { return };
        let Some((channel_id, latest)) = self.viewed_latest_message() else { return };
        let latest_n = snowflake(&latest);
        if latest_n == 0 {
            return;
        }
        let already = self
            .acked_messages
            .get(&channel_id)
            .map(|id| snowflake(id))
            .unwrap_or(0);
        if already >= latest_n {
            return;
        }
        if self
            .ack_sent_at
            .get(&channel_id)
            .is_some_and(|sent| sent.elapsed() < MIN_INTERVAL)
        {
            return;
        }
        self.acked_messages.insert(channel_id.clone(), latest.clone());
        self.ack_sent_at.insert(channel_id.clone(), Instant::now());
        crate::discord::spawn_ack_message(token, channel_id, latest);
    }

    /// `MESSAGE_ACK`: otro dispositivo (o este mismo cliente) marcó un canal
    /// como leído. Se refleja acá con lo que Discord dice que queda sin leer.
    fn apply_remote_ack(&mut self, channel_id: &str, message_id: &str, mention_count: u32) {
        use crate::lib::notifications::snowflake;
        let acked_n = snowflake(message_id);
        let known = self.acked_messages.get(channel_id).map(|id| snowflake(id)).unwrap_or(0);
        if acked_n > known {
            self.acked_messages.insert(channel_id.to_string(), message_id.to_string());
        }
        // El eco de un ack viejo no debe borrar una mención que llegó
        // después: en un DM se sabe cuál es el mensaje más nuevo.
        let newer_exists = self
            .dms
            .iter()
            .find(|dm| dm.id == channel_id)
            .and_then(|dm| dm.last_message_id.as_deref())
            .is_some_and(|last| snowflake(last) > acked_n);
        if mention_count == 0 && newer_exists {
            return;
        }
        self.pending_mentions.remove(channel_id);
        self.set_mention_count(channel_id, mention_count);
    }

    /// Tarjetas del panel "Activo ahora": primero lo que hacés vos (si hay),
    /// después los amigos que están jugando o escuchando algo.
    /// Primera actividad de la propia cuenta (la que muestra la tarjeta de
    /// "escuchando / jugando" de la interfaz nueva), si hay alguna.
    pub fn own_activity(&self) -> Option<&crate::discord::models::PresenceActivity> {
        self.own_activities.first()
    }

    pub fn active_now_cards(&self) -> Vec<crate::lib::data::ActiveNowCard> {
        use crate::lib::data::{ActiveNowCard, Status};
        let mut cards = Vec::new();
        if let Some(me) = self.me.as_ref() {
            for activity in self.own_activities.iter().take(3) {
                cards.push(ActiveNowCard {
                    user_id: me.id.clone(),
                    name: me.display_name().to_string(),
                    avatar_url: me.avatar_url(),
                    avatar_color: self.palette.accent,
                    status: Status::Online,
                    is_me: true,
                    activity: activity.clone(),
                });
            }
        }
        for friend in self.friends.iter().filter(|f| f.is_friend) {
            let Some(activities) = self.user_activities.get(&friend.user_id) else { continue };
            // Un tarjeta por amigo: la primera actividad (Discord ordena la
            // más relevante primero).
            let Some(activity) = activities.first() else { continue };
            cards.push(ActiveNowCard {
                user_id: friend.user_id.clone(),
                name: friend.name.clone(),
                avatar_url: friend.avatar_url.clone(),
                avatar_color: friend.avatar_color,
                status: friend.status,
                is_me: false,
                activity: activity.clone(),
            });
        }
        cards
    }

    /// Texto corto de lo que hace `user_id` ahora (juego, canción...), sin el
    /// estado personalizado. `None` si no está haciendo nada.
    /// Título ("Jugando X") y detalle de la primera actividad de `user_id`.
    fn activity_texts_of(&self, user_id: &str) -> (Option<String>, Option<String>) {
        let Some(a) = self.user_activities.get(user_id).and_then(|v| v.iter().find(|a| a.kind != 4)) else {
            return (None, None);
        };
        let verb = match a.kind {
            0 => "Jugando",
            1 => "Transmitiendo",
            2 => "Escuchando",
            3 => "Viendo",
            5 => "Compitiendo en",
            _ => "",
        };
        let title = if verb.is_empty() { a.name.clone() } else { format!("{verb} {}", a.name) };
        let parts: Vec<&str> = [a.details.as_deref(), a.state.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        (Some(title), (!parts.is_empty()).then(|| parts.join(" · ")))
    }

    fn short_activity_of(&self, user_id: &str) -> Option<String> {
        let activity = self.user_activities.get(user_id)?.iter().find(|a| a.kind != 4)?;
        let pick = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        // Escuchando: la canción; el resto: el nombre del juego/app.
        let song = if activity.kind == 2 { pick(&activity.details) } else { None };
        let text = song.unwrap_or_else(|| activity.name.clone());
        (!text.trim().is_empty()).then_some(text)
    }

    /// Amigos para el carrusel del Inicio nuevo, en orden de prioridad:
    /// primero los que están haciendo algo, después los conectados y al final
    /// los desconectados. Dentro de cada grupo manda la afinidad de Discord
    /// (mayor % primero); si empatan o no hay dato, por nombre.
    pub fn home_friends_sorted(&self) -> Vec<crate::lib::data::HomeFriend> {
        use crate::lib::data::{HomeFriend, Status};
        let mut out: Vec<HomeFriend> = self
            .friends
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_friend)
            .map(|(index, f)| HomeFriend {
                index,
                name: f.name.clone(),
                initial: f.initial(),
                avatar_url: f.avatar_url.clone(),
                avatar_color: f.avatar_color,
                status: f.status,
                activity: if f.status == Status::Offline { None } else { self.short_activity_of(&f.user_id) },
                affinity: self.affinity_users.pct(&f.user_id),
                user_id: f.user_id.clone(),
                handle: {
                    let h = f.handle.trim_end_matches("#0000").trim_end_matches("#0");
                    if h.contains('#') { h.to_owned() } else { format!("@{h}") }
                },
                activity_title: None,
                activity_detail: None,
                ranks: Vec::new(),
            })
            .collect();
        // Detalle de la actividad (solo de quienes están haciendo algo).
        for f in out.iter_mut().filter(|f| f.activity.is_some()) {
            let (title, detail) = self.activity_texts_of(&f.user_id);
            f.activity_title = title;
            f.activity_detail = detail;
        }
        // Puestos de afinidad: global y dentro de cada grupo.
        let rank_where = |pred: &dyn Fn(&HomeFriend) -> bool| -> std::collections::HashMap<usize, (usize, usize)> {
            let mut v: Vec<(usize, f32)> = out
                .iter()
                .filter(|f| pred(f))
                .filter_map(|f| f.affinity.map(|p| (f.index, p)))
                .collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            let total = v.len();
            v.into_iter().enumerate().map(|(i, (idx, _))| (idx, (i + 1, total))).collect()
        };
        let all = rank_where(&|_| true);
        let doing = rank_where(&|f| f.activity.is_some());
        let on = rank_where(&|f| f.status != Status::Offline);
        let off = rank_where(&|f| f.status == Status::Offline);
        for f in out.iter_mut() {
            if let Some(&(r, n)) = all.get(&f.index) {
                f.ranks.push(("Entre todos tus amigos", r, n));
            }
            if let Some(&(r, n)) = doing.get(&f.index) {
                f.ranks.push(("Entre los que hacen algo", r, n));
            }
            if let Some(&(r, n)) = on.get(&f.index) {
                f.ranks.push(("Entre los conectados", r, n));
            }
            if let Some(&(r, n)) = off.get(&f.index) {
                f.ranks.push(("Entre los desconectados", r, n));
            }
        }
        let group = |f: &HomeFriend| -> u8 {
            if f.activity.is_some() {
                0
            } else if f.status != Status::Offline {
                1
            } else {
                2
            }
        };
        out.sort_by(|a, b| {
            group(a)
                .cmp(&group(b))
                .then_with(|| b.affinity.unwrap_or(-1.0).total_cmp(&a.affinity.unwrap_or(-1.0)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    /// Servers para "Servidores frecuentes" del Inicio nuevo: los de mayor
    /// afinidad primero. Los que Discord no puntuó van al final en el orden
    /// de siempre (y si todavía no llegaron las afinidades, todos quedan así).
    pub fn home_servers_sorted(&self) -> Vec<crate::lib::data::HomeServer> {
        let mut out: Vec<crate::lib::data::HomeServer> = self
            .servers
            .iter()
            .enumerate()
            .map(|(index, s)| crate::lib::data::HomeServer {
                index,
                name: s.name.clone(),
                initial: s.icon_initial.clone(),
                icon_url: s.icon_url.clone(),
                icon_color: s.icon_color,
                online_count: s.online_count,
                affinity: self.affinity_guilds.pct(&s.guild_id),
            })
            .collect();
        // `sort_by` es estable: los empatados conservan el orden original.
        out.sort_by(|a, b| b.affinity.unwrap_or(-1.0).total_cmp(&a.affinity.unwrap_or(-1.0)));
        out
    }

    /// Estado de presencia conocido de `user_id`, para el punto de estado del
    /// avatar en la tarjeta de perfil. `None` = no sabemos nada de esa persona.
    pub fn presence_status_of(&self, user_id: &str) -> Option<crate::lib::data::Status> {
        if self.me.as_ref().is_some_and(|me| me.id == user_id) {
            // No nos llega presencia propia por el Gateway: si estamos
            // conectados, estamos en línea (igual que `active_now_cards`).
            return Some(crate::lib::data::Status::Online);
        }
        if let Some((status, _)) = self.presences.get(user_id) {
            return Some(*status);
        }
        self.friends.iter().find(|f| f.user_id == user_id).map(|f| f.status)
    }

    /// Actividades reales (juego, música...) de `user_id`, sin el estado
    /// personalizado — para el bloque "Jugando" de la tarjeta de perfil.
    pub fn activities_of(&self, user_id: &str) -> Vec<crate::discord::models::PresenceActivity> {
        if self.me.as_ref().is_some_and(|me| me.id == user_id) {
            return self.own_activities.clone();
        }
        self.user_activities.get(user_id).cloned().unwrap_or_default()
    }

    /// Texto del estado personalizado de `user_id`, si tiene uno.
    pub fn custom_status_of(&self, user_id: &str) -> Option<String> {
        self.custom_statuses.get(user_id).cloned()
    }

    /// Estado y texto de actividad de quien está del otro lado de un DM.
    /// Primero lo último que llegó por el Gateway; si no hay nada, el estado
    /// del amigo (si lo es); si tampoco, desconectado.
    pub fn dm_presence(&self, dm: &PrivateChannel) -> (crate::lib::data::Status, Option<String>) {
        let Some(user_id) = dm.recipients.first().map(|r| r.id.as_str()) else {
            return (crate::lib::data::Status::Offline, None);
        };
        if let Some((status, subtitle)) = self.presences.get(user_id) {
            return (*status, subtitle.clone());
        }
        self.friends
            .iter()
            .find(|f| f.user_id == user_id)
            .map(|f| (f.status, f.subtitle.clone()))
            .unwrap_or((crate::lib::data::Status::Offline, None))
    }

    /// Decide qué hacer con un mensaje nuevo ajeno:
    ///
    /// 1. Si no aplica (es mío, DM en "solicitudes"/spam, server/canal
    ///    silenciado, server sin mención, o es el canal que estoy mirando con
    ///    la ventana en foco) no hace nada.
    /// 2. Si aplica, suma al contador de menciones (siempre).
    /// 3. Y avisa según `lib::notifications::policy` (ajustes de la cuenta):
    ///    con la ventana en foco, una tarjeta dentro de la app; sin foco, una
    ///    notificación del escritorio.
    fn notify_incoming(&mut self, msg: &crate::discord::models::GatewayMessage) {
        use crate::lib::notifications as notif;

        let Some(my_id) = self.me.as_ref().map(|m| m.id.clone()) else { return };
        if msg.author.id == my_id {
            return;
        }

        let author = notif::author_name(msg);
        let (guild_id, mention, title) =
            if let Some(dm) = self.dms.iter().find(|dm| dm.id == msg.channel_id) {
                if dm.is_message_request || dm.is_spam {
                    return;
                }
                (None, notif::Mention::default(), author)
            } else {
                let Some(server) = self.servers.iter().find(|s| s.has_channel(&msg.channel_id))
                else {
                    return;
                };
                let my_roles = server.access_ctx.my_roles.as_deref().unwrap_or(&[]);
                let mention = notif::Mention {
                    direct: msg.mentions.iter().any(|u| u.id == my_id),
                    everyone: msg.mention_everyone,
                    role: msg.mention_roles.iter().any(|r| my_roles.contains(r)),
                };
                let channel_name = server
                    .categories
                    .iter()
                    .flat_map(|c| c.channels.iter())
                    .find(|ch| ch.channel_id.as_deref() == Some(msg.channel_id.as_str()))
                    .map(|ch| ch.name.as_str())
                    .unwrap_or("canal");
                let title = format!("{author} · #{channel_name} ({})", server.name);
                (Some(server.guild_id.clone()), mention, title)
            };

        if !self
            .mute_rules
            .passes(guild_id.as_deref(), &msg.channel_id, mention)
        {
            return;
        }

        let viewing = self.window_focused
            && self.viewed_channel_id().as_deref() == Some(msg.channel_id.as_str());
        if viewing {
            return;
        }

        self.note_mention(&msg.channel_id, guild_id.as_deref());

        let policy = notif::policy(self.discord_settings.as_ref());
        let body = notif::preview(msg);
        if self.window_focused {
            if policy.in_app {
                let target = match guild_id {
                    Some(guild_id) => NotificationTarget::Channel {
                        guild_id,
                        channel_id: msg.channel_id.clone(),
                    },
                    None => NotificationTarget::Dm { channel_id: msg.channel_id.clone() },
                };
                let avatar_url = msg.author.avatar_url();
                let initial = author_initial(&notif::author_name(msg));
                self.push_in_app_notification(title, body, avatar_url, initial, target);
            }
        } else if policy.desktop {
            notif::send_desktop(title, body);
        }
    }

    /// Agrega una tarjeta de notificación dentro de la app. Si ya hay una de
    /// la misma conversación se actualiza (y cuenta cuántos mensajes junta)
    /// en vez de apilar una nueva por cada mensaje.
    fn push_in_app_notification(
        &mut self,
        title: String,
        body: String,
        avatar_url: Option<String>,
        initial: String,
        target: NotificationTarget,
    ) {
        const MAX_STACK: usize = 4;
        let channel_id = target.channel_id().to_string();
        if let Some(existing) = self
            .in_app_notifications
            .iter_mut()
            .find(|n| n.target.channel_id() == channel_id)
        {
            existing.title = title;
            existing.body = body;
            existing.avatar_url = avatar_url;
            existing.initial = initial;
            existing.count += 1;
            existing.created = Instant::now();
            return;
        }
        self.in_app_notifications.push(InAppNotification {
            title,
            body,
            avatar_url,
            initial,
            target,
            count: 1,
            created: Instant::now(),
        });
        while self.in_app_notifications.len() > MAX_STACK {
            self.in_app_notifications.remove(0);
        }
    }

    /// Abre la conversación/canal al que apunta una notificación (click en
    /// la tarjeta).
    pub fn open_notification_target(&mut self, target: &NotificationTarget) {
        match target {
            NotificationTarget::Dm { channel_id } => {
                if let Some(i) = self.dms.iter().position(|dm| &dm.id == channel_id) {
                    self.open_dm_from_channel(i);
                }
            }
            NotificationTarget::Channel { guild_id, channel_id } => {
                let Some(server_index) = self.servers.iter().position(|s| &s.guild_id == guild_id)
                else {
                    return;
                };
                self.open_server(server_index);
                let position = self.servers[server_index]
                    .categories
                    .iter()
                    .enumerate()
                    .find_map(|(category, cat)| {
                        cat.channels
                            .iter()
                            .position(|ch| ch.channel_id.as_deref() == Some(channel_id.as_str()))
                            .map(|channel| (category, channel))
                    });
                if let Some((category, channel)) = position {
                    self.open_channel(category, channel);
                }
            }
        }
    }

    /// Pone el total de menciones en el título de la ventana (`(3) eCord`);
    /// se ve en la barra de tareas aunque la ventana esté minimizada. Solo
    /// manda el comando cuando el número cambia.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let total = self.total_mentions();
        if self.shown_title_count == Some(total) {
            return;
        }
        self.shown_title_count = Some(total);
        let title = if total > 0 { format!("({total}) eCord") } else { "eCord".to_string() };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn handle_incoming_message(&mut self, msg: crate::discord::models::GatewayMessage) {
        // Antes que nada (el resto puede salir temprano con `return`): sube
        // el DM al tope de la lista y avisa si corresponde.
        self.bump_dm_order(&msg);
        self.notify_incoming(&msg);

        let my_id = self.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();

        // DMs: no hay apodos ni roles.
        if let Some(friend) = self
            .friends
            .iter_mut()
            .find(|f| f.dm_channel_id.as_deref() == Some(msg.channel_id.as_str()))
        {
            // Ventana en el medio del historial (`has_newer`): este mensaje
            // se trae con `after` al bajar; agregarlo ahora dejaría un hueco.
            if !friend.has_newer {
                reconcile_or_push(&mut friend.messages, ChatMessage::from_discord(&msg, &my_id));
            }
            return;
        }

        let Some(server) = self.servers.iter_mut().find(|s| s.has_channel(&msg.channel_id)) else {
            return;
        };
        // Discord manda el apodo y los roles del autor embebidos en el propio
        // mensaje: se guardan primero, así el mensaje ya sale bien resuelto.
        let mut names_changed = false;
        if let Some(member) = &msg.member {
            names_changed = server.learn_member(&msg.author, member.nick.as_deref(), &member.roles);
        }
        let chat_msg = ChatMessage::from_discord_in(
            &msg,
            &my_id,
            Some(NameResolver::new(&server.member_info, &server.roles)),
        );
        let mut in_thread = false;
        if let Some(channel) = server.channel_by_id_mut(&msg.channel_id) {
            in_thread = channel.is_thread;
            // Ver `Friend::has_newer`: con la ventana en el medio no se agrega.
            if !channel.has_newer {
                reconcile_or_push(&mut channel.messages, chat_msg);
            }
        }
        // Un mensaje dentro de un hilo sube el contador de la tarjeta que
        // está debajo del mensaje original (el mensaje inicial del hilo,
        // tipo 21, no cuenta).
        if in_thread && msg.kind != 21 {
            server.note_thread_message(&msg.channel_id, &msg.id);
        }
        // Si alguien cambió de apodo/rol, sus mensajes anteriores también.
        if names_changed {
            server.refresh_author_names();
        }
        let guild_id = server.guild_id.clone();
        self.request_members(&guild_id, message_author_ids(std::slice::from_ref(&msg)));
    }

    /// Le pide al Gateway el apodo y los roles de los `user_ids` que este
    /// server todavía no conoce (los que ya se conocen, o ya se pidieron,
    /// se saltean). La respuesta llega como `AppEvent::GuildMembers`.
    fn request_members(&mut self, guild_id: &str, user_ids: Vec<String>) {
        if user_ids.is_empty() {
            return;
        }
        // Sin conexión no se marca nada como pedido.
        let Some(commands) = self.gateway_commands.clone() else { return };
        let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) else {
            return;
        };
        let missing = server.take_unresolved_members(user_ids.iter().map(String::as_str));
        // El Gateway acepta hasta 100 ids por pedido.
        for chunk in missing.chunks(100) {
            let _ = commands.send(crate::discord::gateway::GatewayCommand::RequestGuildMembers {
                guild_id: guild_id.to_string(),
                user_ids: chunk.to_vec(),
            });
        }
    }

    /// Arma los candidatos del menú de menciones (`@`) para la consulta que el
    /// compositor está tipeando ahora (ver `ui::compose_menus`) y los publica.
    /// No hace nada si no hay una mención en curso.
    ///
    /// Orden: primero quienes hablaron hace poco en el canal abierto, después
    /// la lista de miembros cargada y al final cualquier otro usuario ya visto
    /// (incluye lo que trajo una búsqueda al Gateway). Dentro de cada grado de
    /// coincidencia (empieza igual / una palabra empieza igual / contiene) se
    /// respeta ese orden. En un DM son solo los dos participantes.
    fn publish_mention_results(&mut self, ctx: &egui::Context) {
        use crate::ui::compose_menus::{self as menus, MentionCandidate, MentionKind};

        let Some(query) = menus::active_query(ctx) else { return };
        let needle = menus::fold(&query);
        let fallback_bg = self.palette.surface_active;

        // (grado de coincidencia, orden de prioridad, candidato)
        let mut users: Vec<(u8, usize, MentionCandidate)> = Vec::new();
        let mut checked: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut order = 0usize;
        let mut add_user = |id: &str, name: &str, avatar_url: Option<&str>, avatar_color: Color32, color: Option<Color32>| {
            order += 1;
            if id.is_empty() || name.is_empty() || !checked.insert(id.to_string()) {
                return;
            }
            if let Some(rank) = menus::match_rank(name, &needle) {
                users.push((
                    rank,
                    order,
                    MentionCandidate {
                        kind: MentionKind::User,
                        id: id.to_string(),
                        name: name.to_string(),
                        avatar_url: avatar_url.map(str::to_string),
                        avatar_color,
                        color,
                    },
                ));
            }
        };

        let mut roles_out: Vec<MentionCandidate> = Vec::new();
        let mut special: Vec<MentionCandidate> = Vec::new();

        match self.screen {
            Screen::Server(index) => {
                if let Some(server) = self.servers.get(index) {
                    // 1) Quienes hablaron hace poco en este canal, el más reciente primero.
                    let (category, channel) = self.current_channel;
                    if let Some(channel) = server.channel(category, channel) {
                        for msg in channel.messages.iter().rev() {
                            add_user(&msg.author_id, &msg.author, msg.avatar_url.as_deref(), msg.avatar_color, msg.author_color);
                        }
                    }
                    // 2) La lista de miembros que ya llegó.
                    for group in &server.member_groups {
                        for member in &group.members {
                            add_user(&member.user_id, &member.name, member.avatar_url.as_deref(), member.avatar_color, member.name_color);
                        }
                    }
                    // 3) Cualquier otro usuario ya visto. Se ordena por nombre para que
                    //    la lista no cambie de lugar entre frames (un `HashMap` no tiene orden).
                    let mut known: Vec<(String, &String, Option<&str>)> = server
                        .known_users
                        .iter()
                        .map(|(id, user)| {
                            let nick = server.member_info.get(id).and_then(|info| info.nick.as_deref());
                            (nick.unwrap_or(user.name.as_str()).to_string(), id, user.avatar_url.as_deref())
                        })
                        .filter(|(name, _, _)| menus::match_rank(name, &needle).is_some())
                        .collect();
                    known.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
                    for (name, id, avatar) in known {
                        add_user(id, &name, avatar, fallback_bg, None);
                    }

                    // Roles y @everyone / @here: solo si la cuenta puede mencionarlos
                    // (los roles "mencionables" los puede mencionar cualquiera).
                    let can_everyone = crate::lib::permissions::can_mention_everyone(
                        &server.guild_id,
                        &server.access_ctx,
                        &server.roles,
                    );
                    let mut roles: Vec<&crate::discord::models::Role> = server
                        .roles
                        .iter()
                        .filter(|r| r.id != server.guild_id && !r.name.is_empty() && (r.mentionable || can_everyone))
                        .collect();
                    roles.sort_by(|a, b| b.position.cmp(&a.position));
                    for role in roles {
                        if menus::match_rank(&role.name, &needle).is_some() {
                            roles_out.push(MentionCandidate {
                                kind: MentionKind::Role,
                                id: role.id.clone(),
                                name: role.name.clone(),
                                avatar_url: None,
                                avatar_color: fallback_bg,
                                color: (role.color != 0).then(|| {
                                    Color32::from_rgb((role.color >> 16) as u8, (role.color >> 8) as u8, role.color as u8)
                                }),
                            });
                        }
                    }
                    if can_everyone {
                        for (kind, name) in [(MentionKind::Everyone, "everyone"), (MentionKind::Here, "here")] {
                            if menus::match_rank(name, &needle).is_some() {
                                special.push(MentionCandidate {
                                    kind,
                                    id: String::new(),
                                    name: name.to_string(),
                                    avatar_url: None,
                                    avatar_color: fallback_bg,
                                    color: None,
                                });
                            }
                        }
                    }
                }
            }
            Screen::Dm(index) => {
                if let Some(friend) = self.friends.get(index) {
                    add_user(&friend.user_id, &friend.name, friend.avatar_url.as_deref(), friend.avatar_color, None);
                }
                if let Some(me) = self.me.as_ref() {
                    let avatar = me.avatar_url();
                    add_user(&me.id, me.display_name(), avatar.as_deref(), fallback_bg, None);
                }
            }
            _ => {}
        }

        users.sort_by_key(|(rank, order, _)| (*rank, *order));
        let mut items: Vec<MentionCandidate> =
            users.into_iter().map(|(_, _, cand)| cand).take(menus::MAX_USERS).collect();
        items.extend(roles_out.into_iter().take(menus::MAX_ROLES));
        items.extend(special);
        menus::publish_results(ctx, query, items);
    }

    /// Arma los canales que coinciden con lo que el formulario de un slash
    /// command está buscando en una opción de tipo canal (ver `ui::slash`).
    /// Solo canales visibles del server abierto (sin los hilos de foro).
    fn publish_channel_results(&mut self, ctx: &egui::Context) {
        use crate::ui::compose_menus as menus;
        use crate::ui::slash::{self, ChannelCandidate};

        let Some(query) = slash::take_channel_query(ctx) else { return };
        let needle = menus::fold(&query);
        let mut found: Vec<(u8, usize, ChannelCandidate)> = Vec::new();
        if let Screen::Server(index) = self.screen {
            if let Some(server) = self.servers.get(index) {
                let mut order = 0usize;
                for category in &server.categories {
                    for channel in &category.channels {
                        order += 1;
                        if channel.is_thread || !channel.access.can_view {
                            continue;
                        }
                        let Some(id) = channel.channel_id.clone() else { continue };
                        let Some(rank) = menus::match_rank(&channel.name, &needle) else { continue };
                        found.push((
                            rank,
                            order,
                            ChannelCandidate { id, name: channel.name.clone(), category: category.name.clone() },
                        ));
                    }
                }
            }
        }
        found.sort_by_key(|(rank, order, _)| (*rank, *order));
        let items = found.into_iter().map(|(_, _, cand)| cand).take(25).collect();
        slash::publish_channel_results(ctx, query, items);
    }

    /// Le pide al Gateway los miembros del server abierto cuyo nombre empieza
    /// con `query` (la respuesta llega como `AppEvent::GuildMembers` y deja a
    /// esos usuarios disponibles para el menú de menciones). No aplica en DMs.
    fn search_guild_members(&mut self, query: &str) {
        let Screen::Server(index) = self.screen else { return };
        let Some(commands) = self.gateway_commands.clone() else { return };
        let Some(server) = self.servers.get(index) else { return };
        if server.guild_id.is_empty() {
            return;
        }
        let _ = commands.send(crate::discord::gateway::GatewayCommand::SearchGuildMembers {
            guild_id: server.guild_id.clone(),
            query: query.to_string(),
        });
    }

    /// Busca un mensaje por `channel_id` + `message_id` en cualquier DM o
    /// canal de server (el mismo recorrido que `handle_incoming_message`,
    /// pero además filtrando por id de mensaje). Se usa para aplicar
    /// `MESSAGE_REACTION_ADD`/`REMOVE`, que llegan sueltos y no traen el
    /// mensaje completo.
    fn find_message_mut(&mut self, channel_id: &str, message_id: &str) -> Option<&mut ChatMessage> {
        if let Some(friend) = self
            .friends
            .iter_mut()
            .find(|f| f.dm_channel_id.as_deref() == Some(channel_id))
        {
            if let Some(msg) = friend.messages.iter_mut().find(|m| m.id == message_id) {
                return Some(msg);
            }
        }

        for server in &mut self.servers {
            for category in &mut server.categories {
                for channel in &mut category.channels {
                    if channel.channel_id.as_deref() == Some(channel_id) {
                        return channel.messages.iter_mut().find(|m| m.id == message_id);
                    }
                }
            }
        }
        None
    }

    // ---- Voz: arranque del runtime y acciones de la UI --------------

    // -----------------------------------------------------------------
    // Volumen / silencio por persona (clic derecho en la llamada)
    // -----------------------------------------------------------------

    /// Ajuste local de una persona de las llamadas. Por defecto (nunca se tocó):
    /// 100 % y sin silenciar. Solo afecta lo que ESTE cliente reproduce de esa
    /// persona; ella no se entera.
    pub fn voice_participant_playback(
        &self,
        user_id: &str,
    ) -> crate::discord::VoiceParticipantPlaybackSettings {
        user_id
            .parse::<u64>()
            .ok()
            .and_then(|id| self.participant_playback.get(&id).copied())
            .unwrap_or_default()
    }

    /// Volumen de una persona, en % (0..=200, 100 = normal).
    pub fn set_voice_participant_volume(&mut self, user_id: &str, percent: u16) {
        let mut playback = self.voice_participant_playback(user_id);
        playback.volume = crate::discord::VoiceParticipantVolumePercent::new(percent);
        self.apply_participant_playback(user_id, playback);
    }

    /// Silencia (o deja de silenciar) a una persona sin tocar su volumen.
    pub fn set_voice_participant_muted(&mut self, user_id: &str, muted: bool) {
        let mut playback = self.voice_participant_playback(user_id);
        playback.muted = muted;
        self.apply_participant_playback(user_id, playback);
    }

    /// Vuelve a una persona a 100 % y sin silenciar.
    pub fn reset_voice_participant_audio(&mut self, user_id: &str) {
        self.apply_participant_playback(
            user_id,
            crate::discord::VoiceParticipantPlaybackSettings::default(),
        );
    }

    /// Aplica el cambio ya (el runtime lo toma en el siguiente bloque de audio)
    /// y deja marcado que falta guardarlo en la cuenta.
    fn apply_participant_playback(
        &mut self,
        user_id: &str,
        playback: crate::discord::VoiceParticipantPlaybackSettings,
    ) {
        let Some(raw) = user_id.parse::<u64>().ok() else { return };
        let Some(id) = crate::discord::ids::Id::<crate::discord::ids::marker::UserMarker>::new_checked(raw)
        else {
            return;
        };
        if self.voice_participant_playback(user_id) == playback {
            return;
        }
        if playback == crate::discord::VoiceParticipantPlaybackSettings::default() {
            self.participant_playback.remove(&raw);
        } else {
            self.participant_playback.insert(raw, playback);
        }
        if let Some(tx) = &self.voice_runtime_tx {
            let _ = tx.send(VoiceRuntimeEvent::UpdateParticipantPlaybackSettings {
                user_id: id,
                settings: playback,
            });
        }
        self.participant_audio_dirty.insert(raw);
        self.participant_audio_dirty_at = Some(Instant::now());
    }

    // ---- Audio de los streams (volumen / silencio del stream que mirás) ----

    /// Ajuste local del audio del stream de `owner_id` (por defecto: 100 %, sin
    /// silenciar). Es independiente del volumen de esa persona en la llamada.
    pub fn voice_stream_playback(
        &self,
        owner_id: &str,
    ) -> crate::discord::VoiceParticipantPlaybackSettings {
        owner_id
            .parse::<u64>()
            .ok()
            .and_then(|id| self.stream_playback.get(&id).copied())
            .unwrap_or_default()
    }

    pub fn set_voice_stream_volume(&mut self, owner_id: &str, percent: u16) {
        let mut playback = self.voice_stream_playback(owner_id);
        playback.volume = crate::discord::VoiceParticipantVolumePercent::new(percent);
        self.apply_stream_playback(owner_id, playback);
    }

    pub fn set_voice_stream_muted(&mut self, owner_id: &str, muted: bool) {
        let mut playback = self.voice_stream_playback(owner_id);
        playback.muted = muted;
        self.apply_stream_playback(owner_id, playback);
    }

    pub fn reset_voice_stream_audio(&mut self, owner_id: &str) {
        self.apply_stream_playback(
            owner_id,
            crate::discord::VoiceParticipantPlaybackSettings::default(),
        );
    }

    /// Guarda el cambio y deja marcado que falta mandarlo a la cuenta. El audio
    /// lo toma `pump_stream_frames` en el siguiente frame de UI.
    fn apply_stream_playback(
        &mut self,
        owner_id: &str,
        playback: crate::discord::VoiceParticipantPlaybackSettings,
    ) {
        let Some(raw) = owner_id.parse::<u64>().ok().filter(|id| *id != 0) else { return };
        if self.voice_stream_playback(owner_id) == playback {
            return;
        }
        if playback == crate::discord::VoiceParticipantPlaybackSettings::default() {
            self.stream_playback.remove(&raw);
        } else {
            self.stream_playback.insert(raw, playback);
        }
        self.stream_audio_dirty.insert(raw);
        self.participant_audio_dirty_at = Some(Instant::now());
    }

    /// Incorpora lo que dice la cuenta sobre el audio de los streams (mismas
    /// reglas que `ingest_participant_audio`).
    fn ingest_stream_audio(
        &mut self,
        entries: Vec<(
            crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
            crate::discord::VoiceParticipantPlaybackSettings,
        )>,
        replace: bool,
    ) {
        let next =
            merged_playback(&self.stream_playback, &self.stream_audio_dirty, entries, replace);
        if next != self.stream_playback {
            self.stream_playback = next;
        }
    }

    /// Le manda al runtime la lista completa (al arrancar, o cuando la cuenta
    /// trae cambios de otro dispositivo).
    fn publish_participant_playback(&self) {
        let Some(tx) = &self.voice_runtime_tx else { return };
        let list = self
            .participant_playback
            .iter()
            .filter_map(|(raw, playback)| {
                let id = crate::discord::ids::Id::<crate::discord::ids::marker::UserMarker>::new_checked(*raw)?;
                Some((id, *playback))
            })
            .collect();
        let _ = tx.send(VoiceRuntimeEvent::ReplaceParticipantPlaybackSettings(list));
    }

    /// Incorpora lo que dice la cuenta sobre volumen/silencio por persona.
    /// `replace`: `entries` es la lista completa (carga inicial o update no
    /// parcial); si no, solo trae a las personas que cambiaron. Lo que se tocó
    /// acá y todavía no se mandó gana sobre lo que llegue.
    fn ingest_participant_audio(
        &mut self,
        entries: Vec<(
            crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
            crate::discord::VoiceParticipantPlaybackSettings,
        )>,
        replace: bool,
    ) {
        let next = merged_playback(
            &self.participant_playback,
            &self.participant_audio_dirty,
            entries,
            replace,
        );
        if next == self.participant_playback {
            return;
        }
        self.participant_playback = next;
        self.publish_participant_playback();
    }

    /// Guarda en la cuenta (`audio_context_settings`) lo que cambió, para que
    /// el cliente oficial y los demás dispositivos lo vean igual. Devuelve
    /// `true` mientras quede algo esperando el temporizador.
    fn flush_participant_audio(&mut self, force: bool) -> bool {
        let Some(at) = self.participant_audio_dirty_at else { return false };
        if !force && at.elapsed() < PARTICIPANT_AUDIO_SYNC_DELAY {
            return true;
        }
        let Some(token) = self.discord_token.clone() else {
            // Sin sesión real (pantalla demo) no hay a dónde mandarlo.
            self.participant_audio_dirty.clear();
            self.stream_audio_dirty.clear();
            self.participant_audio_dirty_at = None;
            return false;
        };
        let entries: Vec<(u64, crate::discord::VoiceParticipantPlaybackSettings)> = self
            .participant_audio_dirty
            .drain()
            .map(|raw| (raw, self.participant_playback.get(&raw).copied().unwrap_or_default()))
            .collect();
        let stream_entries: Vec<(u64, crate::discord::VoiceParticipantPlaybackSettings)> = self
            .stream_audio_dirty
            .drain()
            .map(|raw| (raw, self.stream_playback.get(&raw).copied().unwrap_or_default()))
            .collect();
        self.participant_audio_dirty_at = None;
        if entries.is_empty() && stream_entries.is_empty() {
            return false;
        }
        let modified_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let partial = crate::discord::user_settings::partial_with_participant_audio(
            &entries,
            &stream_entries,
            modified_at_ms,
        );
        crate::discord::spawn_patch_user_settings(token, partial);
        false
    }

    /// Arranca, si todavía no está arriba, el hilo de SO con su propio
    /// runtime de tokio que corre `voice::run_voice_runtime` — el que abre
    /// de verdad el WebSocket de voz y captura/reproduce audio. Idempotente
    /// a propósito: se puede llamar desde cualquier acción (`join_voice`,
    /// `start_dm_call`, etc.) sin preocuparse de si ya se llamó antes.
    ///
    /// Necesita `self.event_tx` (para que el runtime pueda mandar
    /// `AppEvent::VoiceConnectionStatusChanged`/`VoiceSpeakingUpdate`/etc.
    /// de vuelta a la UI) — sin sesión iniciada todavía no hay ninguno, así
    /// que en ese caso no hace nada (no debería llamarse antes del login de
    /// todos modos: no hay ningún canal de voz que mostrar sin sesión).
    fn ensure_voice_runtime_started(&mut self) {
        if self.voice_runtime_tx.is_some() {
            return;
        }
        let Some(event_tx) = self.event_tx.clone() else { return };
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        // El runtime manda comandos de vuelta al Gateway principal (opcode
        // 4, `UpdateVoiceState`) a través del mismo canal que ya usa el
        // resto de `App` — si todavía no hay conexión (`gateway_commands`
        // sigue en `None`) se manda un sender que descarta todo; en cuanto
        // llegue `AppEvent::GatewayCommands` el runtime ya está recibiendo
        // eventos por `events_tx` igual, así que no hace falta reiniciarlo.
        let gateway_commands_tx = self.gateway_commands.clone().unwrap_or_else(|| {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            tx
        });
        let status_publisher = crate::discord::voice::VoiceStatusPublisher::new(event_tx);
        self.voice_runtime_tx = Some(events_tx.clone());
        // El runtime arranca sabiendo el volumen/silencio de cada persona.
        self.publish_participant_playback();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            rt.block_on(crate::discord::voice::run_voice_runtime(
                events_rx,
                events_tx,
                gateway_commands_tx,
                status_publisher,
            ));
        });
    }

    /// Ajustes por default con los que se pide una conexión de voz nueva:
    /// los de `self.voice.audio` (persistidos, ver `ui::settings`) más
    /// mute/deaf según lo que el usuario haya dejado pre-seteado con los
    /// botones de la barra de usuario (`self.self_mute`/`self.self_deaf`)
    /// antes de unirse — igual que el cliente real, si ya te silenciaste
    /// de antemano entrás muteado a la llamada.
    fn requested_voice_state(
        &self,
        scope: crate::discord::VoiceScope,
        channel_id: crate::discord::ids::Id<crate::discord::ids::marker::ChannelMarker>,
    ) -> CurrentVoiceConnectionState {
        let settings = self.voice.audio.clone();
        CurrentVoiceConnectionState {
            scope,
            channel_id,
            self_mute: self.self_mute,
            self_deaf: self.self_deaf,
            allow_microphone_transmit: settings.allow_microphone_transmit,
            noise_suppression: settings.noise_suppression,
            microphone_buffer_ms: settings.microphone_buffer_ms,
            microphone_sensitivity: settings.microphone_sensitivity,
            microphone_volume: settings.microphone_volume,
            voice_output_volume: settings.voice_output_volume,
        }
    }

    /// Manda el estado de voz vigente (`self.voice_target`, o "salí de
    /// voz" si es `None`) tanto al runtime (para que abra/cierre el socket
    /// y arranque/pare de capturar audio) como al Gateway principal
    /// (opcode 4 — sin esto Discord ni se entera de que nos unimos, y el
    /// runtime nunca recibe el `VOICE_SERVER_UPDATE` que necesita).
    fn sync_voice_target(&mut self) {
        self.ensure_voice_runtime_started();
        if let Some(tx) = &self.voice_runtime_tx {
            let _ = tx.send(VoiceRuntimeEvent::Requested(self.voice_target.clone()));
        }
        let Some(commands) = self.gateway_commands.clone() else { return };
        let command = match &self.voice_target {
            Some(target) => crate::discord::gateway::GatewayCommand::UpdateVoiceState {
                guild_id: target.scope.guild_id().map(|id| id.to_string()),
                channel_id: Some(target.channel_id.to_string()),
                self_mute: target.self_mute,
                self_deaf: target.self_deaf,
            },
            None => crate::discord::gateway::GatewayCommand::UpdateVoiceState {
                guild_id: None,
                channel_id: None,
                self_mute: false,
                self_deaf: false,
            },
        };
        // TEMPORAL: para diagnosticar el spam de opcode 4 — borrar una vez
        // encontrado el llamador repetido. `n` es un contador global (no
        // por-instancia) para poder ver en el log cuántas veces se llamó
        // `sync_voice_target` en total y con qué frecuencia.
        static SYNC_VOICE_TARGET_CALLS: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let n = SYNC_VOICE_TARGET_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        crate::logging::debug(
            "voice",
            format!(
                "sync_voice_target call #{n}: target={:?}",
                self.voice_target
            ),
        );
        let _ = commands.send(command);
    }

    /// Se une (o cambia) al canal de voz `channel_id` de un server.
    /// Llamado al tocar un canal de voz en `ui::server`.
    pub fn join_voice_channel(&mut self, guild_id: &str, channel_id: &str) {
        let (Some(guild_id), Some(channel_id)) = (
            crate::discord::voice::parse_guild_id(guild_id),
            crate::discord::voice::parse_channel_id(channel_id),
        ) else {
            return;
        };
        let scope = crate::discord::VoiceScope::Guild(guild_id);
        self.voice_target = Some(self.requested_voice_state(scope, channel_id));
        self.sync_voice_target();
    }

    // -----------------------------------------------------------------
    // Menú de clic derecho sobre una persona (ver `ui::audio_menu`)
    // -----------------------------------------------------------------

    /// Marca (o desmarca) como amigo a la entrada de `friends`, si existe, sin
    /// borrar el DM.
    fn set_friend_flag(&mut self, user_id: &str, is_friend: bool) {
        if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
            friend.is_friend = is_friend;
        }
    }

    /// Cuántas solicitudes de amistad recibidas esperan respuesta (globito rojo
    /// de la pestaña "Pendiente").
    pub fn pending_incoming_count(&self) -> usize {
        self.pending_requests.iter().filter(|p| p.incoming).count()
    }

    /// "Enviar solicitud de amistad" de la pantalla "Añadir amigo": el resultado
    /// llega como `AppEvent::FriendRequestSent` y se muestra bajo el campo.
    pub fn send_friend_request(&mut self, username: String) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            self.add_friend_status =
                Some((false, "Necesitas iniciar sesión en Discord para añadir amigos.".to_string()));
            return;
        };
        self.add_friend_busy = true;
        self.add_friend_status = None;
        crate::discord::spawn_send_friend_request(token, username, tx);
    }

    /// Saca de `pending_requests` la solicitud de `user_id`, si está.
    fn take_pending(&mut self, user_id: &str) -> Option<crate::lib::data::PendingRequest> {
        let index = self.pending_requests.iter().position(|p| p.user.id == user_id)?;
        Some(self.pending_requests.remove(index))
    }

    /// `RELATIONSHIP_ADD`: deja `relationship_kinds`, la lista de amigos y las
    /// solicitudes pendientes de acuerdo con la relación nueva.
    fn apply_relationship_add(&mut self, relationship: crate::discord::models::Relationship) {
        let user_id = relationship.id.clone();
        self.relationship_kinds.insert(user_id.clone(), relationship.kind);
        self.pending_requests.retain(|p| p.user.id != user_id);
        if relationship.user_ignored {
            self.ignored_users.insert(user_id.clone());
        }
        match relationship.kind {
            // Amistad confirmada (aceptaron la nuestra, o aceptamos desde otro
            // dispositivo).
            1 => {
                if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
                    friend.is_friend = true;
                } else {
                    self.friends.push(Friend::from_relationship(&relationship));
                }
            }
            3 | 4 => {
                if let Some(request) = crate::lib::data::PendingRequest::from_relationship(&relationship) {
                    if request.incoming {
                        self.push_toast(
                            ToastKind::Info,
                            "Solicitud de amistad",
                            format!("{} quiere ser tu amigo.", request.name),
                        );
                    }
                    self.pending_requests.push(request);
                }
            }
            // Bloqueado: deja de ser amigo.
            _ => self.set_friend_flag(&user_id, false),
        }
        self.menu_ctx_rev += 1;
    }

    /// Lanza una acción de red sobre una persona (amistad, ignorar, bloquear,
    /// nota, invitación). El resultado llega como `AppEvent::UserActionDone`.
    pub fn run_user_action(&mut self, user_id: String, action: crate::discord::UserAction) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            self.push_toast(
                ToastKind::Warning,
                "Sin sesión",
                "Necesitás iniciar sesión en Discord para hacer esto.",
            );
            return;
        };
        crate::discord::spawn_user_action(token, user_id, action, tx);
    }

    /// Pide la nota actual y, cuando llega (`AppEvent::UserNote`), abre el
    /// editor con el texto precargado.
    fn open_note_editor(&mut self, user_id: String, name: String) {
        let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) else {
            self.push_toast(
                ToastKind::Warning,
                "Sin sesión",
                "Necesitás iniciar sesión en Discord para guardar notas.",
            );
            return;
        };
        self.pending_note = Some((user_id.clone(), name));
        crate::discord::spawn_fetch_note(token, user_id, tx);
    }

    fn show_note_dialog(&mut self, user_id: String, name: String, note: Option<String>) {
        use crate::ui::dialog::{ButtonStyle, Dialog, DialogKind, FormField};
        self.open_dialog(
            Dialog::new("user_note", DialogKind::Question, format!("Nota sobre {name}"))
                .subtitle("Solo visible para ti")
                .field(
                    FormField::new("note", "Nota")
                        .placeholder("Haz clic para añadir una nota")
                        .value(note.unwrap_or_default())
                        .multiline(true)
                        .rows(4)
                        .max_len(256),
                )
                .button("cancel", "Cancelar", ButtonStyle::Secondary)
                .button("save", "Guardar", ButtonStyle::Primary)
                .dismissable()
                .on_result(move |app, _ctx, result| {
                    if result.is("save") {
                        let note = result.value("note").unwrap_or_default().trim().to_string();
                        app.run_user_action(user_id, crate::discord::UserAction::SetNote(note));
                    }
                }),
        );
    }

    /// Popup para aceptar una solicitud que Discord no dejó aceptar sin
    /// confirmación (`confirm_stranger_request`). Al confirmar se reenvía con
    /// `true`.
    fn confirm_accept_friend(&mut self, user_id: String, reason: String) {
        use crate::ui::dialog::{Dialog, DialogKind};
        let name = self
            .friends
            .iter()
            .find(|f| f.user_id == user_id)
            .map(|f| f.name.clone())
            .or_else(|| {
                self.dms
                    .iter()
                    .flat_map(|d| d.recipients.iter())
                    .find(|r| r.id == user_id)
                    .map(|r| r.display_name().to_string())
            })
            .unwrap_or_else(|| "esta persona".to_string());
        self.open_dialog(
            Dialog::confirm(
                format!("accept_friend_{user_id}"),
                DialogKind::Warning,
                "¿Aceptar solicitud de amistad?",
                format!(
                    "Discord pidió confirmación para aceptar la solicitud de {name}. \
                     Puede ser alguien con quien no tienes amigos ni servidores en común."
                ),
                "Aceptar",
            )
            .note(format!("Respuesta de Discord: {reason}"))
            .on_result(move |app, _ctx, result| {
                if result.is("confirm") {
                    app.run_user_action(
                        user_id,
                        crate::discord::UserAction::AcceptFriend { confirm: true },
                    );
                }
            }),
        );
    }

    /// "Iniciar una llamada": abre el DM y llama. Si todavía no se conoce el
    /// canal, la llamada sale cuando llega (`AppEvent::DmOpened`).
    fn call_user(&mut self, user_id: &str, name: &str) {
        let known = self
            .friends
            .iter()
            .find(|f| f.user_id == user_id)
            .and_then(|f| f.dm_channel_id.clone())
            .or_else(|| {
                self.dms
                    .iter()
                    .find(|d| d.recipients.len() == 1 && d.recipients[0].id == user_id)
                    .map(|d| d.id.clone())
            });
        self.message_user_from_profile(
            user_id,
            name,
            None,
            crate::lib::data::color_from_id(user_id),
            None,
        );
        match known {
            Some(channel_id) => self.start_dm_call(&channel_id),
            None => self.pending_call_user = Some(user_id.to_string()),
        }
    }

    /// Publica lo que el menú de clic derecho necesita ver de `App` (relaciones,
    /// ignorados, servers) cuando cambió.
    fn publish_menu_context(&mut self, ctx: &egui::Context) {
        let fingerprint = (self.menu_ctx_rev, self.servers.len());
        if self.menu_ctx_published == Some(fingerprint) {
            return;
        }
        self.menu_ctx_published = Some(fingerprint);
        crate::ui::audio_menu::publish_context(
            ctx,
            crate::ui::audio_menu::MenuContext {
                relationships: self.relationship_kinds.clone(),
                ignored: self.ignored_users.clone(),
                video_disabled: self.video_disabled_users.clone(),
                guilds: self
                    .servers
                    .iter()
                    .filter(|s| !s.guild_id.is_empty())
                    .map(|s| (s.guild_id.clone(), s.name.clone()))
                    .collect(),
            },
        );
    }

    /// "Silenciar panel de sonidos" de una persona (se guarda en la cuenta).
    pub fn set_voice_participant_soundboard_muted(&mut self, user_id: &str, muted: bool) {
        let mut playback = self.voice_participant_playback(user_id);
        playback.soundboard_muted = muted;
        self.apply_participant_playback(user_id, playback);
    }

    /// Arranca (o se une a) una llamada de voz de un DM/grupo. A diferencia
    /// de un canal de server, acá no hay `guild_id`: Discord indexa la
    /// llamada por el `channel_id` del propio DM (ver `VoiceScope::Private`).
    pub fn start_dm_call(&mut self, channel_id: &str) {
        let Some(id) = crate::discord::voice::parse_channel_id(channel_id) else { return };
        let scope = crate::discord::VoiceScope::Private(id);
        self.voice_target = Some(self.requested_voice_state(scope, id));
        self.sync_voice_target();
    }

    /// Corta la conexión de voz actual, si hay una (canal de server o
    /// llamada de DM). Botón de colgar de la barra de llamada.
    pub fn leave_voice(&mut self) {
        // Un stream se ve estando en el canal: al salir se corta también.
        self.stop_watching_stream();
        if self.voice_target.is_none() {
            return;
        }
        self.voice_target = None;
        self.sync_voice_target();
    }

    /// Prende/apaga el propio micrófono. Silenciarse no toca `self_deaf`;
    /// ensordecerse si apaga el micrófono también (ver
    /// `toggle_self_deafen`), como en el cliente real de Discord.
    ///
    /// A diferencia de antes, esto ya NO depende de estar en una llamada:
    /// `self.self_mute` es la preferencia persistida que se usa tanto para
    /// pre-silenciarte antes de unirte (ver `requested_voice_state`) como
    /// para el botón de la barra de usuario (`ui::friends_panel::user_bar`)
    /// estando fuera de una llamada. Si hay una llamada en curso
    /// (`voice_target`), además se actualiza en vivo y se sincroniza con
    /// el Gateway/runtime, igual que hacía la barra de llamada.
    pub fn toggle_self_mute(&mut self) {
        self.self_mute = !self.self_mute;
        if !self.self_mute {
            self.self_deaf = false;
        }
        if let Some(target) = &mut self.voice_target {
            target.self_mute = self.self_mute;
            target.self_deaf = self.self_deaf;
            self.sync_voice_target();
        }
    }

    /// Prende/apaga "ensordecerse" (no escuchar a nadie). Al ensordecerse
    /// también nos silencia, como en Discord; al desensordecerse el
    /// micrófono no se toca especial, solo deja de forzarse `self_mute`.
    /// Misma idea que `toggle_self_mute`: funciona con o sin llamada activa.
    pub fn toggle_self_deafen(&mut self) {
        self.self_deaf = !self.self_deaf;
        if self.self_deaf {
            self.self_mute = true;
        }
        if let Some(target) = &mut self.voice_target {
            target.self_mute = self.self_mute;
            target.self_deaf = self.self_deaf;
            self.sync_voice_target();
        }
    }

    /// Nombre para mostrar en la barra de llamada (`ui::call_bar`): el
    /// canal de voz si es una llamada de server, o el nombre del
    /// amigo/DM si es privada. `None` si todavía no encontramos el
    /// canal/amigo correspondiente (por ejemplo mientras `self.servers`
    /// recién está cargando) — la barra cae a un texto genérico en ese
    /// caso.
    pub fn current_voice_channel_label(&self) -> Option<String> {
        let target = self.voice_target.as_ref()?;
        let channel_id = target.channel_id.to_string();
        match target.scope {
            crate::discord::VoiceScope::Guild(guild_id) => {
                let guild_id = guild_id.to_string();
                self.servers
                    .iter()
                    .find(|s| s.guild_id == guild_id)
                    .and_then(|server| {
                        server
                            .categories
                            .iter()
                            .flat_map(|category| category.channels.iter())
                            .find(|c| c.channel_id.as_deref() == Some(channel_id.as_str()))
                            .map(|c| c.name.clone())
                    })
            }
            crate::discord::VoiceScope::Private(_) => self
                .friends
                .iter()
                .find(|f| f.dm_channel_id.as_deref() == Some(channel_id.as_str()))
                .map(|f| f.name.clone()),
        }
    }

    // ---- Voz: ajustes de audio persistidos (ui::settings, pestaña "Voz y audio") ----

    /// Vuelve a enumerar los dispositivos de entrada/salida del sistema
    /// para el picker de `ui::settings`. Se llama al abrir la pestaña (no
    /// en cada frame: `cpal` puede tardar, sobre todo con ALSA/PulseAudio
    /// reiniciando) — ver `SettingsTab::Voice` en `ui::settings::show`.
    pub fn refresh_voice_audio_sources(&mut self) {
        match list_voice_audio_sources() {
            Ok(options) => self.voice_audio_source_options = options,
            Err(error) => {
                crate::logging::debug(
                    "voice",
                    format!("voice audio source enumeration failed: {error}"),
                );
            }
        }
    }

    /// Elige `device_id` (o `None` para "predeterminado del sistema") como
    /// micrófono. `device_id` viene tal cual de
    /// `voice_audio_source_options.inputs()` (el `ComboBox` de
    /// `ui::settings`), así que no hace falta re-clampear nada acá.
    pub fn set_voice_input_source(&mut self, device_id: Option<String>) {
        if self.voice.audio_sources.input == device_id {
            return;
        }
        self.voice.audio_sources.input = device_id;
        self.persist_voice_audio_sources();
        self.publish_voice_audio_sources();
    }

    /// Igual que [`Self::set_voice_input_source`] pero para la salida.
    pub fn set_voice_output_source(&mut self, device_id: Option<String>) {
        if self.voice.audio_sources.output == device_id {
            return;
        }
        self.voice.audio_sources.output = device_id;
        self.persist_voice_audio_sources();
        self.publish_voice_audio_sources();
    }

    fn persist_voice_audio_sources(&self) {
        if let Ok(json) = serde_json::to_string(&self.voice.audio_sources) {
            let _ = web_local_storage_api::set_item("voice_audio_sources", &json);
        }
    }

    /// El id de dispositivo elegido no depende de estar conectado a un
    /// canal (a diferencia de `sync_voice_target`, que solo tiene sentido
    /// con `voice_target: Some`): el runtime lo guarda apenas arranca y lo
    /// usa la próxima vez que abre el micrófono/parlante, estemos en una
    /// llamada ahora mismo o no. Por eso esto siempre arranca el runtime y
    /// manda el evento, sin chequear `voice_target`.
    fn publish_voice_audio_sources(&mut self) {
        self.ensure_voice_runtime_started();
        if let Some(tx) = &self.voice_runtime_tx {
            let _ = tx.send(VoiceRuntimeEvent::AudioSourcesChanged(
                self.voice.audio_sources.clone(),
            ));
        }
    }

    /// Prende/apaga si el micrófono puede transmitir. Arranca en `false`
    /// (ver `VoiceAudioSettings::default`) a propósito, como opt-in
    /// explícito: hasta que no lo tocás acá una vez, unirte a un canal de
    /// voz no manda audio, aunque no estés silenciado.
    pub fn set_voice_allow_microphone_transmit(&mut self, allow: bool) {
        if self.voice.audio.allow_microphone_transmit == allow {
            return;
        }
        self.voice.audio.allow_microphone_transmit = allow;
        self.persist_voice_audio_settings();
        self.sync_voice_audio_settings_to_target();
    }

    pub fn set_voice_noise_suppression(&mut self, enabled: bool) {
        if self.voice.audio.noise_suppression == enabled {
            return;
        }
        self.voice.audio.noise_suppression = enabled;
        self.persist_voice_audio_settings();
        self.sync_voice_audio_settings_to_target();
    }

    /// `db` ya viene clampeado por `MicrophoneSensitivityDb::new` (-100..0).
    pub fn set_voice_microphone_sensitivity(&mut self, db: i8) {
        let value = crate::discord::MicrophoneSensitivityDb::new(db);
        if self.voice.audio.microphone_sensitivity == value {
            return;
        }
        self.voice.audio.microphone_sensitivity = value;
        self.persist_voice_audio_settings();
        self.sync_voice_audio_settings_to_target();
    }

    /// `percent` ya viene clampeado por `VoiceVolumePercent::new` (0..200).
    pub fn set_voice_microphone_volume(&mut self, percent: u8) {
        let value = crate::discord::VoiceVolumePercent::new(percent);
        if self.voice.audio.microphone_volume == value {
            return;
        }
        self.voice.audio.microphone_volume = value;
        self.persist_voice_audio_settings();
        self.sync_voice_audio_settings_to_target();
    }

    /// Igual que [`Self::set_voice_microphone_volume`] pero para el
    /// volumen de salida (lo que se escucha de los demás).
    pub fn set_voice_output_volume(&mut self, percent: u8) {
        let value = crate::discord::VoiceVolumePercent::new(percent);
        if self.voice.audio.voice_output_volume == value {
            return;
        }
        self.voice.audio.voice_output_volume = value;
        self.persist_voice_audio_settings();
        self.sync_voice_audio_settings_to_target();
    }

    fn persist_voice_audio_settings(&self) {
        if let Ok(json) = serde_json::to_string(&self.voice.audio) {
            let _ = web_local_storage_api::set_item("voice_audio_settings", &json);
        }
    }

    /// Si ya estamos conectados a una llamada, empuja los ajustes recién
    /// tocados (sensibilidad, volumen, supresión de ruido, si el mic puede
    /// transmitir) al runtime en caliente. Sin esto quedarían "congelados"
    /// con los valores que había al momento de `join_voice_channel`
    /// (`requested_voice_state` los copia una sola vez, ahí) hasta la
    /// próxima reconexión. `self_mute`/`self_deaf`/canal no se tocan acá:
    /// esos los maneja la barra de llamada, no esta pestaña.
    fn sync_voice_audio_settings_to_target(&mut self) {
        let Some(target) = &mut self.voice_target else { return };
        let settings = self.voice.audio.clone();
        target.allow_microphone_transmit = settings.allow_microphone_transmit;
        target.noise_suppression = settings.noise_suppression;
        target.microphone_buffer_ms = settings.microphone_buffer_ms;
        target.microphone_sensitivity = settings.microphone_sensitivity;
        target.microphone_volume = settings.microphone_volume;
        target.voice_output_volume = settings.voice_output_volume;
        self.sync_voice_target();
    }
}

/// Ids (sin repetir) de los autores de `messages` y de los autores a los que
/// responden — todos los que hace falta poder mostrar con su apodo.
fn message_author_ids(messages: &[crate::discord::models::GatewayMessage]) -> Vec<String> {
    let mut ids = std::collections::BTreeSet::new();
    for message in messages {
        ids.insert(message.author.id.clone());
        if let Some(original) = &message.referenced_message {
            ids.insert(original.author.id.clone());
        }
    }
    ids.into_iter().collect()
}

/// Si `chat_msg` es un mensaje propio y ya había un eco local optimista
/// esperando confirmación (`ChatMessage::own`, empujado directo por
/// `ui::chat::composer` al apretar Enter, ANTES de que la respuesta real
/// del Gateway/REST llegara — mismo contenido, `id` todavía vacío), lo
/// reemplaza in-place por la versión real en vez de agregar una fila
/// nueva. Esto es lo que evita el bug de "el mensaje aparece dos veces
/// hasta reiniciar": antes, el eco local optimista y el `MESSAGE_CREATE`
/// que Discord manda de vuelta por el propio mensaje que mandaste
/// quedaban los dos en la lista para siempre.
///
/// Si no encuentra ningún eco pendiente (mensaje ajeno, o el propio pero
/// mandado desde otro cliente/dispositivo mientras este estaba abierto),
/// lo agrega como una fila nueva, como siempre. Busca desde el final
/// porque el eco más reciente es el que más probablemente corresponde a
/// este mensaje.
fn reconcile_or_push(messages: &mut Vec<ChatMessage>, chat_msg: ChatMessage) {
    if chat_msg.is_own {
        if let Some(echo) = messages
            .iter_mut()
            .rev()
            .find(|m| m.is_own && m.id.is_empty() && m.content == chat_msg.content)
        {
            *echo = chat_msg;
            return;
        }
    }
    messages.push(chat_msg);
}

/// Aplica un `MESSAGE_REACTION_ADD`/`REMOVE` en vivo a un mensaje ya
/// encontrado. A diferencia de `ChatMessage::toggle_reaction` (que asume
/// "la cuenta logueada clickeó esto ahora"), acá el evento puede ser de
/// cualquier usuario — el contador siempre sube/baja, pero `reacted_by_me`
/// solo cambia si el evento es sobre la propia cuenta (`is_me`).
fn apply_remote_reaction(msg: &mut ChatMessage, emoji: crate::lib::data::ReactionKind, is_me: bool, added: bool) {
    if added {
        if let Some(existing) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
            // El propio click ya se aplicó optimistamente en
            // `ChatMessage::toggle_reaction` (ver `ui::chat::show`, donde
            // se llama antes de avisarle a Discord por REST). Si esto es
            // el eco de gateway de esa misma acción (`reacted_by_me` ya
            // está en `true`), no hay que volver a sumar el contador — si
            // lo hiciéramos, cada reacción propia terminaría contando
            // doble hasta reiniciar el cliente. Si en cambio llega un
            // `add` propio sin que `reacted_by_me` ya estuviera puesto
            // (reaccionaste desde el cliente real u otro dispositivo
            // mientras este estaba abierto), sí corresponde aplicarlo.
            if is_me && existing.reacted_by_me {
                return;
            }
            existing.count += 1;
            if is_me {
                existing.reacted_by_me = true;
            }
        } else {
            msg.reactions.push(crate::lib::data::Reaction {
                emoji,
                count: 1,
                reacted_by_me: is_me,
            });
        }
    } else if let Some(existing) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
        // Mismo criterio que en el `if added` de arriba, en espejo: si el
        // propio click de "sacar la reacción" ya se aplicó optimistamente
        // (`reacted_by_me` ya está en `false`), el eco del gateway no
        // resta de nuevo.
        if is_me && !existing.reacted_by_me {
            return;
        }
        existing.count = existing.count.saturating_sub(1);
        if is_me {
            existing.reacted_by_me = false;
        }
        if existing.count == 0 {
            msg.reactions.retain(|r| r.emoji != emoji);
        }
    }
}

// ---- Go Live: ver el stream de otra persona ----
impl App {
    /// Pide ver el stream de `owner_user_id` en el canal de voz `channel_id`
    /// del server `guild_id` (opcode 20). Discord responde con
    /// `STREAM_CREATE` + `STREAM_SERVER_UPDATE`, y recién ahí
    /// (`try_start_stream_watch`) se abre la conexión de media.
    ///
    /// Hay que estar conectado a ese canal de voz: la UI solo ofrece el botón
    /// en ese caso.
    pub fn watch_stream(&mut self, guild_id: &str, channel_id: &str, owner_user_id: &str, owner_name: &str) {
        let stream_key = format!("guild:{guild_id}:{channel_id}:{owner_user_id}");
        if self.watching_stream.as_ref().is_some_and(|w| w.stream_key == stream_key) {
            return;
        }
        // Solo se ve un stream a la vez: se deja el anterior primero.
        self.stop_watching_stream();
        let Some(commands) = self.gateway_commands.clone() else {
            self.push_toast(ToastKind::Warning, "Sin conexión", "Todavía no hay conexión con Discord");
            return;
        };
        self.stream_sessions.remove(&stream_key);
        self.watching_stream = Some(WatchedStream {
            stream_key: stream_key.clone(),
            channel_id: channel_id.to_string(),
            owner_name: owner_name.to_string(),
            status: StreamWatchStatus::Connecting,
            message: None,
            handle: None,
            texture: None,
            frame_size: [0, 0],
            frames_shown: 0,
        });
        let _ = commands.send(crate::discord::gateway::GatewayCommand::StreamWatch { stream_key });
    }

    /// Deja de ver el stream actual (botón de cerrar del visor, o al salir de
    /// la llamada): le avisa al Gateway (opcode 19) y corta la conexión.
    pub fn stop_watching_stream(&mut self) {
        let Some(watched) = self.watching_stream.take() else { return };
        self.stream_sessions.remove(&watched.stream_key);
        if let Some(commands) = self.gateway_commands.clone() {
            let _ = commands.send(crate::discord::gateway::GatewayCommand::StreamDelete {
                stream_key: watched.stream_key.clone(),
            });
        }
        // `watched` se suelta acá: `StreamWatchHandle::drop` corta la conexión.
    }

    /// Si ya se juntaron `rtc_server_id`, endpoint y token del stream que se
    /// quiere ver, abre la conexión de media.
    fn try_start_stream_watch(&mut self, stream_key: &str) {
        let Some(pending) = self.stream_sessions.get(stream_key) else { return };
        let (Some(rtc_server_id), Some(endpoint), Some(token)) =
            (pending.rtc_server_id.clone(), pending.endpoint.clone(), pending.token.clone())
        else {
            return;
        };
        let Some(watched) = self.watching_stream.as_mut() else { return };
        if watched.stream_key != stream_key || watched.handle.is_some() {
            return;
        }
        // stream_key = "guild:<guild>:<canal>:<usuario>" o "call:<canal>:<usuario>":
        // el último tramo es siempre quien transmite.
        let owner_user_id = stream_key
            .rsplit(':')
            .next()
            .and_then(crate::discord::voice::parse_user_id);
        let user_id = self.me.as_ref().and_then(|me| crate::discord::voice::parse_user_id(&me.id));
        let (Some(owner_user_id), Some(user_id)) = (owner_user_id, user_id) else {
            watched.status = StreamWatchStatus::Failed;
            watched.message = Some("No se pudo identificar al usuario del stream".to_string());
            return;
        };
        let Some(event_tx) = self.event_tx.clone() else { return };
        if self.gateway_session_id.is_empty() {
            return;
        }
        let repaint_ctx = self.egui_ctx.clone();
        let repaint: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
            if let Some(ctx) = &repaint_ctx {
                ctx.request_repaint();
            }
        });
        let params = StreamWatchParams {
            stream_key: stream_key.to_string(),
            rtc_server_id,
            endpoint,
            token,
            user_id,
            session_id: self.gateway_session_id.clone(),
            owner_user_id,
            output_source: self.voice.audio_sources.output.clone(),
        };
        watched.handle = Some(spawn_stream_watch(params, event_tx, repaint));
    }

    /// Pasa el último frame decodificado (si llegó uno) a la textura del
    /// visor. Se llama una vez por frame de UI.
    fn pump_stream_frames(&mut self, ctx: &egui::Context) {
        // Ensordecerte también calla el audio del stream.
        let deaf = self.voice_target.as_ref().map(|t| t.self_deaf).unwrap_or(self.self_deaf);
        let Some(watched) = self.watching_stream.as_mut() else { return };
        // Respaldo: aunque el hilo de video no logre despertar la UI, mientras
        // se mira un stream se repinta unas 10 veces por segundo.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let Some(handle) = watched.handle.as_ref() else { return };
        // Volumen/silencio del audio de ESTE stream (clave: quien transmite).
        let playback = watched
            .stream_key
            .rsplit(':')
            .next()
            .and_then(|id| id.parse::<u64>().ok())
            .and_then(|id| self.stream_playback.get(&id).copied())
            .unwrap_or_default();
        handle
            .audio
            .set(playback.volume.value().min(200) as u8, playback.muted || deaf);
        let Some(frame) = handle.frames.take_new() else { return };
        let size = [frame.width as usize, frame.height as usize];
        if size[0] == 0 || size[1] == 0 || frame.rgba.len() != size[0] * size[1] * 4 {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
        match watched.texture.as_mut() {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                watched.texture = Some(ctx.load_texture("stream_view", image, egui::TextureOptions::LINEAR));
            }
        }
        watched.frame_size = size;
        watched.frames_shown += 1;
        if watched.frames_shown == 1 {
            crate::logging::debug(
                "stream",
                format!("first stream frame uploaded to the viewer texture: {}x{}", size[0], size[1]),
            );
        }
    }
}

impl eframe::App for App {
    /// Framebuffer transparente: la ventana se crea con `with_transparent(true)`
    /// (ver `main.rs`) y es `Backdrop::paint` el que decide, cada frame, cuánto
    /// tapa. Con un tema opaco el fondo se pinta al 100 % y no se nota nada.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// eframe 0.34+ entrega directamente un `Ui` (no un `Context`), así que
    /// cada pantalla recibe `ui: &mut egui::Ui` y arma sus paneles con
    /// `egui::Panel::left/right(...).show(ui, ...)` en cascada.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Devuelve al sistema la memoria libre del allocator (cada ~20 s).
        crate::support::mem_report::release_free_memory_periodic();

        // Tamaño de la interfaz (zoom). Si egui ya no tiene el zoom que le
        // pusimos, lo cambió el usuario con Ctrl + / Ctrl - y se adopta; si no,
        // se le aplica el elegido en Ajustes.
        {
            let ctx = ui.ctx().clone();
            let current = ctx.zoom_factor();
            if (current - self.ui_scale_applied).abs() > 0.001 {
                self.set_ui_scale(current);
                self.ui_scale_applied = self.ui_scale;
                if (self.ui_scale - current).abs() > 0.001 {
                    ctx.set_zoom_factor(self.ui_scale);
                }
            } else if (self.ui_scale - current).abs() > 0.001 {
                ctx.set_zoom_factor(self.ui_scale);
                self.ui_scale_applied = self.ui_scale;
            }
            if self.ui_scale_applied.is_nan() {
                ctx.set_zoom_factor(self.ui_scale);
                self.ui_scale_applied = self.ui_scale;
            }
        }

        // Reaplica el estilo cada frame (barato) para poder alternar
        // Palette::dark()/light() en caliente si más adelante agregás un
        // toggle de tema.
        crate::theme::apply(ui.ctx(), &self.palette);

        // Con un diálogo abierto, la app de atrás queda congelada: el fondo
        // oscuro ya frena los clics, pero el login lee Enter y el texto
        // directo del teclado. Se quita el foco y se aparta el teclado ANTES
        // de dibujar cualquier pantalla (el diálogo lo recupera al dibujarse).
        crate::ui::dialog::begin_frame(ui.ctx(), self);

        // eframe limpia el framebuffer de verdad a transparente (no hay
        // `with_transparent(true)` en el viewport, así que ese
        // "transparente" se ve directamente como negro puro). Cualquier
        // huequito que quede sin pintar por algún panel/frame de acá abajo
        // — un `item_spacing` por defecto entre dos columnas, un borde sin
        // relleno, lo que sea — deja ver ese negro de fondo en vez del gris
        // oscuro del tema. Pintamos TODA la ventana con el color base
        // antes de dibujar cualquier otra cosa para que ninguna
        // separación quede negra pase lo que pase en el resto del árbol
        // de UI.
        //
        // Ahora ese "pintar todo con el color base" lo hace el fondo del tema
        // (sólido, degradado, imagen o wallpaper; con su propia opacidad, ver
        // `theme::Backdrop`). Con `background_opacity < 1` deja este mismo
        // hueco transparente a propósito para que se vea el escritorio.
        let full_rect = ui.max_rect();
        self.backdrop_rt.paint(ui.ctx(), ui.painter(), full_rect, &self.backdrop, &self.palette);

        // Foco de la ventana ANTES de procesar eventos: decide si un mensaje
        // nuevo se avisa dentro de la app o en el escritorio.
        self.window_focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
        self.poll_discord_events(ui.ctx());
        self.tick_pending_buttons(ui.ctx());
        self.pump_stream_frames(ui.ctx());
        self.clear_viewed_mentions();
        self.ack_viewed_channel();
        self.sync_window_title(ui.ctx());
        // El hilo de Discord no despierta la UI al llegar un evento, y sin
        // input/animaciones eframe no repinta: con la ventana en segundo
        // plano los mensajes se quedarían sin procesar (y sin notificar)
        // hasta que vuelvas a tocarla. Un repintado suave lo evita.
        if matches!(self.auth, AuthStatus::Connected) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.poll_wallpaper_updates();
        // ¿Lo que se tipee sin ningún campo enfocado puede ir al compositor del
        // chat? No si hay un diálogo, popup, ajustes, visor o video a pantalla
        // completa encima (ver `ui::chat::set_type_to_focus_allowed`).
        {
            let overlay_open = !self.dialogs.is_empty()
                || self.modal.is_some()
                || self.component_modal.is_some()
                || self.profile_popup.is_some()
                || self.role_popup.is_some()
                || self.profile_full.is_some()
                || self.settings_open
                || self.theme_editor.is_some()
                || crate::ui::media_viewer::is_open()
                || crate::ui::video_player::is_fullscreen();
            crate::ui::chat::set_type_to_focus_allowed(ui.ctx(), !overlay_open);
        }
        // Recorta las imágenes decodificadas si se pasan de su tope.
        crate::ui::anim::maintain(ui.ctx());
        // Al cambiar de chat, suelta las imágenes decodificadas del anterior.
        let view_key = match self.screen {
            Screen::Dm(i) => 2 + ((i as u64) << 8),
            Screen::Server(i) => {
                let (c, h) = self.current_channel;
                3 + ((i as u64) << 8) + ((c as u64) << 24) + ((h as u64) << 40)
            }
            _ => 1,
        };
        crate::ui::anim::on_view_change(ui.ctx(), view_key);
        self.trim_inactive_history(ui.ctx().input(|i| i.time));
        // Roles del server abierto para las menciones `@rol` del chat, y
        // clics en menciones de canal/rol (ver `ui::markdown`).
        {
            let ctx = ui.ctx().clone();
            let roles: &[crate::discord::models::Role] = match self.screen {
                Screen::Server(index) => self.servers.get(index).map(|s| s.roles.as_slice()).unwrap_or(&[]),
                _ => &[],
            };
            crate::ui::markdown::publish_roles(&ctx, roles);
            // Menú de menciones (`@`) del compositor: candidatos para lo que se
            // está tipeando y, si hacen falta más, búsqueda en el Gateway.
            self.publish_mention_results(&ctx);
            // Opciones de tipo canal de un slash command: canales del server abierto.
            self.publish_channel_results(&ctx);
            // Slash commands (`/`): catálogo del canal abierto y, si el
            // compositor dejó uno listo, ejecutarlo.
            self.publish_slash_catalog(&ctx);
            if let Some(invocation) = crate::ui::slash::take_invocation(&ctx) {
                self.run_slash_invocation(invocation);
            }
            if let Some(query) = crate::ui::compose_menus::take_member_search(&ctx) {
                self.search_guild_members(&query);
            }
            if let Some(channel_id) = crate::ui::markdown::take_channel_request(&ctx) {
                self.open_channel_by_id(&channel_id);
            }
            if let Some(click) = crate::ui::markdown::take_role_request(&ctx) {
                if matches!(self.screen, Screen::Server(_)) {
                    self.role_popup = Some(RolePopup {
                        role_id: click.id,
                        name: click.name,
                        color: click.color,
                        anchor: click.anchor,
                    });
                }
            }
        }
        // Alguien clickeó un avatar/nombre en algún lado (chat, DM, panel
        // de amigos) — ver `ui::profile_popup::request_open`.
        // Menú de clic derecho de volumen por persona (ver `ui::audio_menu`).
        self.publish_menu_context(ui.ctx());
        for change in crate::ui::audio_menu::take_changes(ui.ctx()) {
            match change {
                crate::ui::audio_menu::AudioChange::Volume(id, percent) => {
                    self.set_voice_participant_volume(&id, percent)
                }
                crate::ui::audio_menu::AudioChange::Muted(id, muted) => {
                    self.set_voice_participant_muted(&id, muted)
                }
                crate::ui::audio_menu::AudioChange::Reset(id) => {
                    self.reset_voice_participant_audio(&id)
                }
                crate::ui::audio_menu::AudioChange::SoundboardMuted(id, muted) => {
                    self.set_voice_participant_soundboard_muted(&id, muted)
                }
                crate::ui::audio_menu::AudioChange::VideoDisabled(id, disabled) => {
                    if disabled {
                        self.video_disabled_users.insert(id);
                    } else {
                        self.video_disabled_users.remove(&id);
                    }
                    self.menu_ctx_rev += 1;
                }
                crate::ui::audio_menu::AudioChange::Profile(id, name) => {
                    let color = crate::lib::data::color_from_id(&id);
                    self.open_user_profile(id, name, None, color);
                }
                crate::ui::audio_menu::AudioChange::Mention(id) => {
                    self.compose_text.push_str(&format!("<@{id}> "));
                }
                crate::ui::audio_menu::AudioChange::Message(id, name) => {
                    let color = crate::lib::data::color_from_id(&id);
                    self.message_user_from_profile(&id, &name, None, color, None);
                }
                crate::ui::audio_menu::AudioChange::Call(id, name) => self.call_user(&id, &name),
                crate::ui::audio_menu::AudioChange::Note(id, name) => self.open_note_editor(id, name),
                crate::ui::audio_menu::AudioChange::VerificationCode(_) => {
                    self.open_dialog(crate::ui::dialog::Dialog::info(
                        "verification_code",
                        "Código de verificación",
                        "Esta función todavía no está disponible en ecord.",
                    ));
                }
                crate::ui::audio_menu::AudioChange::Api(id, action) => self.run_user_action(id, action),
            }
        }
        if let Some(request) = crate::ui::profile_popup::take_requested(ui.ctx()) {
            let anchor = request.anchor;
            self.open_user_profile(request.user_id, request.name, request.avatar_url, request.avatar_color);
            if let Some(popup) = &mut self.profile_popup {
                popup.anchor = anchor;
            }
        }

        // Barra superior propia (arrastre + buscador + estado + controles
        // de ventana): ocupa todo el ancho, por encima de rail/paneles/
        // central. En Login (que tiene su propia pantalla centrada) se
        // muestra una versión mínima: solo arrastre y controles de ventana,
        // porque la ventana no tiene decoraciones nativas.
        if matches!(self.screen, Screen::Login) {
            crate::ui::topbar::show_login(self, ui);
        }
        if !matches!(self.screen, Screen::Login) {
            crate::ui::topbar::show(self, ui);
            // Interfaz nueva: franja vacía en el borde derecho para que la
            // última tarjeta no quede pegada a la ventana (GAP/2 de la
            // tarjeta + esta franja = GAP, igual que a la izquierda).
            if crate::theme::is_modern() {
                egui::Panel::right("modern_edge_right")
                    .exact_size((crate::theme::GAP / 2) as f32)
                    .resizable(false)
                    .frame(egui::Frame::NONE)
                    .show(ui, |_ui| {});
            }
        }

        // Barra de llamada nueva: a todo el ancho, abajo. Va ANTES de las
        // pantallas para que su panel reserve el borde inferior y los paneles
        // laterales y el central se repartan lo que queda.
        if self.new_call_ui && self.voice_target.is_some() && !matches!(self.screen, Screen::Login) {
            crate::ui::call_bar::show_bottom(self, ui);
        }

        match self.screen {
            Screen::Login => crate::ui::login::show(self, ui),
            Screen::Home => crate::ui::home::show(self, ui),
            Screen::Dm(_) => crate::ui::dm::show(self, ui),
            Screen::Server(_) => crate::ui::server::show(self, ui),
        }

        // Popup del stream que se está viendo cuando no estás en la vista de la llamada.
        crate::ui::call_view::show_popup(self, ui);
        // Overlays flotantes, siempre por encima de todo lo demás:
        // notificaciones apiladas en una esquina y el popup modal actual.
        crate::ui::overlay::show_toasts(self, ui);
        crate::ui::notifications::show_in_app(self, ui);
        crate::ui::overlay::show_modal(self, ui);
        crate::ui::overlay::show_component_modal(self, ui);
        crate::ui::profile_popup::show(self, ui);
        crate::ui::role_popup::show(self, ui);
        crate::ui::profile_popup::show_full(self, ui);
        crate::ui::settings::show(self, ui);
        // Cola de diálogos (aviso de seguridad, novedades, confirmaciones...):
        // va al final para quedar por encima de todo, incluida la pantalla de
        // login (donde se escribe el token).
        crate::ui::dialog::show(self, ui);
        // Visor multimedia (imágenes/GIFs a pantalla completa).
        crate::ui::media_viewer::show(self, ui);
        crate::ui::video_player::show_fullscreen(ui.ctx());
        // Suelta los players de GIF (embeds `gifv`) que ya no se dibujan.
        crate::ui::video_player::end_frame(ui.ctx());
        // Si la vista de llamada quedó en pantalla completa pero ya no se dibuja,
        // saca la ventana de pantalla completa.
        crate::ui::call_view::end_frame(ui.ctx());
    }
}

/// Primera letra del nombre en mayúscula, para el avatar de reemplazo de las
/// notificaciones (`?` si el nombre está vacío).
fn author_initial(name: &str) -> String {
    name.chars()
        .find(|c| !c.is_whitespace())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_string())
}
