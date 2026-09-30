use std::time::Instant;

use egui::Color32;

use crate::discord::models::{PrivateChannel, User, UserProfileResponse};
use crate::discord::voice::VoiceRuntimeEvent;
use crate::discord::voice::{VoiceAudioSourceOptions, VoiceAudioSources, list_voice_audio_sources};
use crate::discord::{AppEvent, CurrentVoiceConnectionState, VoiceAudioSettings, VoiceCache, VoiceConnectionStatus};
use crate::lib::data::{demo_activity, demo_friends, demo_servers, ActivityCard, ChatMessage, Friend, NameResolver, Server};
use crate::theme::{Backdrop, BackdropRuntime, Palette, ThemeDef, ThemeEditor, ThemeMode};
use crate::ui::settings::SettingsTab;
use serde::{Deserialize, Serialize};
use web_local_storage_api;

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

/// Lo único que persistimos en `localStorage` entre sesiones: el token de
/// la cuenta ya logueada, para no tener que escanear el QR cada vez que se
/// abre la app.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct UserDB {
    logged_in: bool,
    token: Option<String>,
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

/// Notificación de mención/DM que se muestra DENTRO de la app (esquina
/// izquierda) cuando la ventana tiene el foco; con la ventana sin foco se
/// manda una del escritorio en su lugar. Ver `ui::notifications::show_in_app`
/// y `App::notify_incoming`.
pub struct InAppNotification {
    pub title: String,
    pub body: String,
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

pub struct App {
    pub screen: Screen,
    pub auth: AuthStatus,
    /// Usuario logueado (viene de `READY`). `None` mientras no haya sesión
    /// real (pantallas demo).
    pub me: Option<User>,
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
    /// Hilo abierto en el panel lateral del server, si hay alguno.
    pub thread_panel: Option<ThreadPanel>,
    /// Último momento (reloj de la UI, segundos) en que se recortó el
    /// historial de los chats inactivos (`App::trim_inactive_history`).
    pub last_history_trim: f64,
    /// Formulario pedido por un bot (tras apretar uno de sus botones).
    pub component_modal: Option<ComponentModal>,
    /// `session_id` del Gateway (del `READY`): las interacciones con
    /// botones de bots lo piden.
    pub gateway_session_id: String,
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
    /// Tarjetas de notificación dentro de la app, más recientes al final.
    pub in_app_notifications: Vec<InAppNotification>,
    /// Servers/canales/DMs silenciados y nivel de mensajes (vienen en el
    /// `READY`, ver `lib::notifications::MuteRules`).
    mute_rules: crate::lib::notifications::MuteRules,
    /// Último total de menciones puesto en el título de la ventana.
    shown_title_count: Option<u32>,
    /// Popup modal actualmente abierto, si hay alguno.
    pub modal: Option<Modal>,
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
    /// Desafío de 2FA que mandó Discord (`AuthStatus::PasswordMfaRequired`
    /// necesita esto para saber qué métodos ofrecer y con qué ticket
    /// contestar).
    pub pending_mfa: Option<crate::discord::password_auth::MfaChallenge>,
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
}

impl Default for App {
    fn default() -> Self {
        let user_db = match web_local_storage_api::get_item("current_user") {
            Ok(Some(json)) => serde_json::from_str::<UserDB>(&json).unwrap_or_default(),
            Ok(None) => UserDB::default(),
            Err(_) => UserDB::default(),
        };
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

        let mut app = Self {
            screen: Screen::Login,
            auth: AuthStatus::SignedOut,
            me: None,
            discord_token: None,
            discord_settings: None,
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
            thread_panel: None,
            last_history_trim: 0.0,
            component_modal: None,
            gateway_session_id: String::new(),
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
            in_app_notifications: Vec::new(),
            mute_rules: crate::lib::notifications::MuteRules::default(),
            shown_title_count: None,
            modal: None,
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
            pending_mfa: None,
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
        };
        // Con el modo ya cargado, calculamos la paleta que corresponde
        // (y, si es `Wallpaper`, arrancamos el hilo que lo vigila) antes
        // del primer frame.
        app.apply_theme_mode();

        // Si ya había una sesión guardada, nos saltamos el QR y vamos
        // directo al Gateway con ese token.
        if user_db.logged_in {
            if let Some(token) = user_db.token {
                let (tx, rx) = std::sync::mpsc::channel();
                app.discord_token = Some(token.clone());
                app.event_tx = Some(tx.clone());
                app.event_rx = Some(rx);
                app.auth = AuthStatus::Connecting;
                crate::discord::spawn_gateway_with_token(token, tx);
            }
        }

        app
    }
}

impl App {
    /// Arranca el login por QR: abre un canal nuevo hacia un hilo de fondo
    /// que hace todo el handshake de `remote_auth` y, si sale bien, sigue
    /// derecho a la conexión del Gateway.
    pub fn start_sign_in(&mut self) {
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
        self.auth = AuthStatus::SignedOut;
        self.event_rx = None;
        self.event_tx = None;
    }

    pub fn retry_sign_in(&mut self) {
        self.start_sign_in();
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

        let friend = &mut self.friends[index];
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
        self.current_channel = (category, channel);
        self.compose_text.clear();
        self.reply_target = None;
        self.thread_panel = None;
        self.subscribe_member_list();
        self.load_forum_posts_if_needed();

        let Screen::Server(server_index) = self.screen else { return };
        let Some(server) = self.servers.get_mut(server_index) else { return };
        let Some(ch) = server.channel_mut(category, channel) else { return };
        if ch.is_voice || ch.loaded {
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
        let application_id = click.application_id.clone().unwrap_or_default();
        let Some(ctx) = self.interaction_context(&click.channel_id, &application_id, None) else {
            self.push_toast(
                ToastKind::Warning,
                "Botón",
                "Todavía no se puede usar este botón (falta la sesión o la aplicación dueña).",
            );
            return;
        };
        crate::discord::spawn_press_button(token, ctx, click.message_id, click.flags, click.custom_id, tx);
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
        self.flush_member_subscription(ctx);
        let Some(rx) = &self.event_rx else { return };
        let pending: Vec<AppEvent> = rx.try_iter().collect();
        for event in pending {
            self.handle_discord_event(event);
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
                self.discord_token = Some(token.clone());
                let db = UserDB {
                    logged_in: true,
                    token: Some(token),
                };
                if let Ok(json) = serde_json::to_string(&db) {
                    let _ = web_local_storage_api::set_item("current_user", &json);
                }
                if let Some((token, tx)) = self.discord_token.clone().zip(self.event_tx.clone()) {
                    crate::discord::spawn_fetch_user_settings(token, tx);
                }
            }
            AppEvent::UserSettings(settings) => {
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
                self.discord_settings = Some(*settings);
            }
            AppEvent::Ready(ready) => {
                self.gateway_session_id = ready.session_id.clone();
                let display_name = ready.user.display_name().to_string();
                self.me = Some(ready.user);
                self.friends = ready
                    .relationships
                    .iter()
                    .filter(|r| r.kind == 1) // 1 = amistad confirmada
                    .map(Friend::from_relationship)
                    .collect();
                self.servers = ready.guilds.iter().map(Server::from_guild).collect();
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
                    friend.dm_channel_id = Some(channel_id);
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
            AppEvent::Error(message) => {
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
                self.push_toast(ToastKind::Warning, "Discord", format!("{reason}. Reconectando…"));
            }
            AppEvent::GatewayReconnected => {
                self.push_toast(ToastKind::Success, "Discord", "Conexión restablecida");
            }
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
                self.component_modal = Some(ComponentModal::new(*request));
            }
            AppEvent::InteractionFailed { message } => {
                self.push_toast(ToastKind::Warning, "No se pudo completar la acción", message);
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
                self.discord_token = Some(token.clone());
                self.login_email.clear();
                self.login_password.clear();
                self.login_mfa_code.clear();
                self.login_sms_sent = false;
                self.pending_mfa = None;
                let db = UserDB { logged_in: true, token: Some(token.clone()) };
                if let Ok(json) = serde_json::to_string(&db) {
                    let _ = web_local_storage_api::set_item("current_user", &json);
                }
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
                (friend.dm_channel_id.clone()?, &friend.messages, friend.loading)
            }
            Screen::Server(i) => {
                let (category, channel) = self.current_channel;
                let channel = self.servers.get(i)?.channel(category, channel)?;
                (channel.channel_id.clone()?, &channel.messages, channel.loading)
            }
            _ => return None,
        };
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
                self.push_in_app_notification(title, body, target);
            }
        } else if policy.desktop {
            notif::send_desktop(title, body);
        }
    }

    /// Agrega una tarjeta de notificación dentro de la app. Si ya hay una de
    /// la misma conversación se actualiza (y cuenta cuántos mensajes junta)
    /// en vez de apilar una nueva por cada mensaje.
    fn push_in_app_notification(&mut self, title: String, body: String, target: NotificationTarget) {
        const MAX_STACK: usize = 4;
        let channel_id = target.channel_id().to_string();
        if let Some(existing) = self
            .in_app_notifications
            .iter_mut()
            .find(|n| n.target.channel_id() == channel_id)
        {
            existing.title = title;
            existing.body = body;
            existing.count += 1;
            existing.created = Instant::now();
            return;
        }
        self.in_app_notifications.push(InAppNotification {
            title,
            body,
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
            reconcile_or_push(&mut friend.messages, ChatMessage::from_discord(&msg, &my_id));
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
            reconcile_or_push(&mut channel.messages, chat_msg);
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
        // Reaplica el estilo cada frame (barato) para poder alternar
        // Palette::dark()/light() en caliente si más adelante agregás un
        // toggle de tema.
        crate::theme::apply(ui.ctx(), &self.palette);

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
        // Recorta las imágenes decodificadas si se pasan de su tope.
        crate::ui::anim::maintain(ui.ctx());
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
        if let Some(request) = crate::ui::profile_popup::take_requested(ui.ctx()) {
            let anchor = request.anchor;
            self.open_user_profile(request.user_id, request.name, request.avatar_url, request.avatar_color);
            if let Some(popup) = &mut self.profile_popup {
                popup.anchor = anchor;
            }
        }

        // Barra superior propia (arrastre + buscador + estado + controles
        // de ventana): ocupa todo el ancho, por encima de rail/paneles/
        // central. No aplica en Login, que tiene su propia pantalla
        // centrada de inicio de sesión.
        if !matches!(self.screen, Screen::Login) {
            crate::ui::topbar::show(self, ui);
        }

        match self.screen {
            Screen::Login => crate::ui::login::show(self, ui),
            Screen::Home => crate::ui::home::show(self, ui),
            Screen::Dm(_) => crate::ui::dm::show(self, ui),
            Screen::Server(_) => crate::ui::server::show(self, ui),
        }

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
        crate::ui::video_player::show_fullscreen(ui.ctx());
        // Suelta los players de GIF (embeds `gifv`) que ya no se dibujan.
        crate::ui::video_player::end_frame(ui.ctx());
    }
}