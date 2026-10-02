//! Todo lo que habla con los servidores reales de Discord vive acá,
//! separado de `ui::*`/`lib::state`, que siguen siendo sync (los necesita
//! `eframe`). La comunicación entre los dos mundos es un
//! `std::sync::mpsc::Sender<AppEvent>` / `Receiver<AppEvent>`: este módulo
//! corre en un hilo aparte con su propio runtime de tokio y va empujando
//! eventos; `App` los va sacando del receiver una vez por frame
//! (`App::poll_discord_events`, en `lib/state.rs`) y actualiza su estado.

pub mod gateway;
pub mod models;
pub mod remote_auth;
pub mod rest;
pub mod fingerprint;
pub mod password_auth;
pub mod captcha;
pub mod user_settings;
pub mod uwu_rest;
pub mod voice;
mod auth_http;
pub(crate) mod ids;

pub use voice::{
    CurrentVoiceConnectionState, MemberInfo, MicrophoneBufferMs, MicrophoneSensitivityDb,
    StreamWatchStatus, VoiceAudioSettings, VoiceCache, VoiceConnectionStatus,
    VoiceParticipantState, VoiceScope, VoiceServerInfo, VoiceSoundKind, VoiceStateInfo,
    VoiceParticipantPlaybackSettings, VoiceParticipantVolumePercent, VoiceVolumePercent,
};
pub use models::{
    Channel, Emoji, ForumPage, GatewayMessage, ThreadChannel, MemberListMember, MemberListUpdate, MessageUpdate,
    PresenceEvent, ReadyPayload, Role, User, UserProfileResponse, VoiceState,
};

/// `VOICE_SERVER_UPDATE`, tal cual llega del Gateway: sin ids tipados, como
/// el resto de los eventos "de cable" de `ecord` (se convierte a
/// `voice::VoiceServerInfo` recién en `voice::forward_app_event`).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct VoiceServerUpdate {
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    pub endpoint: Option<String>,
    pub token: String,
}

/// `STREAM_CREATE`: alguien empezó a transmitir, o Discord confirma que
/// pedimos ver un stream (`GatewayCommand::StreamWatch`). `rtc_server_id` es
/// el id con el que hay que identificarse en el WebSocket de voz del stream.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct StreamCreate {
    pub stream_key: String,
    #[serde(default)]
    pub rtc_server_id: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub viewer_ids: Vec<String>,
    #[serde(default)]
    pub paused: bool,
}

/// `STREAM_SERVER_UPDATE`: endpoint y token del servidor de media del stream
/// (equivalente a `VOICE_SERVER_UPDATE`, pero para el Go Live).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct StreamServerUpdate {
    pub stream_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

/// `STREAM_UPDATE`: cambió la lista de espectadores o el estado de pausa.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct StreamUpdate {
    pub stream_key: String,
    #[serde(default)]
    pub viewer_ids: Vec<String>,
    #[serde(default)]
    pub paused: bool,
}

/// `STREAM_DELETE`: el stream terminó (o dejó de estar disponible).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct StreamDelete {
    pub stream_key: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
}

/// Todo lo que el hilo de Discord le puede avisar a la UI.
pub enum AppEvent {
    /// Código QR listo para mostrar (ya es la URL completa, `App` solo
    /// tiene que renderizarla como QR).
    QrCode(String),
    /// El usuario escaneó el QR con el celular; falta que confirme ahí.
    QrConfirming { username: String },
    /// Login terminado: tenemos un token de sesión utilizable.
    LoggedIn { token: String },
    /// Primer payload grande del Gateway: usuario, servidores y amigos.
    Ready(Box<ReadyPayload>),
    /// `PreloadedUserSettings` real de la cuenta (tema, locale, status,
    /// notificaciones, etc. — ver `discord::user_settings`), traído de
    /// `GET /users/@me/settings-proto/1` justo después de loguearse. Si
    /// el pedido falla (cuenta nueva sin blob todavía, error de red) no
    /// se manda nada y el cliente se queda con los defaults locales.
    UserSettings(Box<user_settings::PreloadedUserSettings>),
    /// `USER_SETTINGS_PROTO_UPDATE` del Gateway: la cuenta cambió sus ajustes
    /// (típicamente el orden/carpetas de servers) desde otro dispositivo.
    /// `partial` = solo trae lo que cambió.
    UserSettingsUpdate {
        settings: Box<user_settings::PreloadedUserSettings>,
        partial: bool,
    },
    /// Mensaje nuevo (server o DM) recibido en vivo.
    MessageCreate(Box<GatewayMessage>),
    /// Mensaje ya existente que cambió (edición, o embeds de un link que
    /// Discord terminó de generar) — `MESSAGE_UPDATE`, payload parcial.
    MessageUpdate(Box<MessageUpdate>),
    /// Se abrió (o ya existía) el canal de DM con `user_id`, con su
    /// historial reciente ya cargado.
    DmOpened {
        user_id: String,
        channel_id: String,
        messages: Vec<GatewayMessage>,
    },
    /// Canales de un server, pedidos por REST al abrirlo por primera vez.
    GuildChannels {
        guild_id: String,
        channels: Vec<Channel>,
    },
    /// Historial de un canal de texto de server, pedido por REST la
    /// primera vez que se abre (`App::open_channel`).
    ChannelMessages {
        channel_id: String,
        messages: Vec<GatewayMessage>,
    },
    /// Página más vieja del historial de un canal/DM, pedida por
    /// `App::load_more_messages` cuando el usuario llega arriba del todo
    /// de los mensajes ya cargados (ver `ui::chat::ChatEvent::LoadMoreRequested`).
    MoreChannelMessages {
        channel_id: String,
        messages: Vec<GatewayMessage>,
    },
    /// Foto de "quién está en qué canal de voz" de un guild. Llega por
    /// `READY_SUPPLEMENTAL` (un `{ id, voice_states }` por guild, justo
    /// después del `READY`) y por `GUILD_CREATE`. Es una foto COMPLETA: la
    /// UI reemplaza lo que había, no lo suma.
    GuildVoiceStates {
        guild_id: String,
        states: Vec<VoiceState>,
    },
    /// Cambio de voz en vivo (`VOICE_STATE_UPDATE`): alguien se conectó, se
    /// desconectó o cambió de canal de voz.
    VoiceStateUpdate(VoiceState),
    /// `VOICE_SERVER_UPDATE`: el endpoint y token para abrir el WebSocket de
    /// voz de la conexión que se acaba de pedir con `GatewayCommand::UpdateVoiceState`.
    VoiceServerUpdate(VoiceServerUpdate),
    /// Go Live: alguien (o nosotros, al pedir verlo) creó un stream. Trae el
    /// `rtc_server_id` que hace falta para conectarse al servidor de media.
    StreamCreate(StreamCreate),
    /// Endpoint + token del servidor de media de un stream que estamos viendo.
    StreamServerUpdate(StreamServerUpdate),
    /// Cambió la lista de espectadores / pausa de un stream.
    StreamUpdate(StreamUpdate),
    /// Un stream terminó.
    StreamDelete(StreamDelete),
    /// Progreso de la conexión de media del stream que estamos viendo
    /// (`voice::spawn_stream_watch`): conectando, recibiendo video, falló...
    StreamWatchStatus {
        stream_key: String,
        status: StreamWatchStatus,
        message: Option<String>,
    },
    /// Datos de un usuario pedidos por REST como respaldo, cuando un
    /// estado de voz llegó sin `member`/`user` embebido (ver
    /// `spawn_fetch_user`).
    UserFetched(User),
    /// Alguien puso una reacción en vivo (`MESSAGE_REACTION_ADD`), en
    /// cualquier mensaje al que tengamos acceso — no solo el nuestro.
    ReactionAdd {
        channel_id: String,
        message_id: String,
        user_id: String,
        emoji: Emoji,
    },
    /// `SESSIONS_REPLACE`: cambió lo que jugás/escuchás en alguna de tus
    /// sesiones (por ejemplo la app oficial de Discord en tu PC). Ya viene
    /// filtrado (ver `models::parse_session_activities`).
    OwnActivities(Vec<models::PresenceActivity>),
    /// `MESSAGE_ACK`: alguien (otro dispositivo tuyo, o este mismo cliente)
    /// marcó un canal como leído hasta `message_id`. `mention_count` es lo
    /// que le queda sin leer a ese canal según Discord.
    MessageAck {
        channel_id: String,
        message_id: String,
        mention_count: u32,
    },
    /// Igual que `ReactionAdd` pero para `MESSAGE_REACTION_REMOVE`.
    ReactionRemove {
        channel_id: String,
        message_id: String,
        user_id: String,
        emoji: Emoji,
    },
    /// Algo se rompió (login o Gateway); mensaje ya listo para mostrar.
    Error(String),
    /// Se cortó la conexión con Discord (se cerró el socket, dejó de
    /// contestar los heartbeats, etc.) y `gateway::run` va a intentar
    /// reconectar solo. A diferencia de `Error`, esto NO es fatal — es
    /// solo para que la UI le avise al usuario que algo pasó en vez de
    /// quedarse callada mientras reconecta en segundo plano.
    GatewayDisconnected(String),
    /// La reconexión automática después de un `GatewayDisconnected`
    /// funcionó.
    GatewayReconnected,
    /// El extremo de envío para mandarle comandos al Gateway (suscribirse
    /// a la lista de miembros de un canal). Llega una vez por cada
    /// conexión nueva que arranca (`spawn_login_flow` /
    /// `spawn_gateway_with_token`); `App` se queda con el último.
    GatewayCommands(tokio::sync::mpsc::UnboundedSender<gateway::GatewayCommand>),
    /// Roles de un server (`GUILD_CREATE`), para nombrar y colorear los
    /// grupos de la lista de miembros.
    GuildRoles { guild_id: String, roles: Vec<Role> },
    /// Roles de la PROPIA cuenta en un server (`merged_members` del `READY`
    /// / `READY_SUPPLEMENTAL`). Junto con los permisos de los roles y los
    /// overwrites de cada canal, permiten esconder los canales que no se
    /// pueden ver y marcar con candado los privados (ver
    /// `lib::permissions`).
    SelfGuildRoles { guild_id: String, roles: Vec<String> },
    /// Apodo y roles de uno o más miembros de un server, para mostrar en
    /// el chat el nombre del server y el color de su rol. Llega como
    /// respuesta a un `RequestGuildMembers` (`GUILD_MEMBERS_CHUNK`) y
    /// cuando alguien cambia su apodo/roles (`GUILD_MEMBER_UPDATE`).
    GuildMembers { guild_id: String, members: Vec<MemberListMember> },
    /// Cambio de presencia en vivo (`PRESENCE_UPDATE`).
    PresenceUpdate(Box<PresenceEvent>),
    /// Presencias iniciales de los amigos conectados
    /// (`READY_SUPPLEMENTAL.merged_presences.friends`).
    PresenceSnapshot(Vec<PresenceEvent>),
    /// Novedades de la lista de miembros de un server
    /// (`GUILD_MEMBER_LIST_UPDATE`).
    MemberListUpdate(Box<MemberListUpdate>),
    /// Login con usuario/contraseña resuelto sin 2FA (o el 2FA ya
    /// verificado): tenemos un token de sesión. Igual que con el QR, esto
    /// no arranca el Gateway solo — `App` reacciona mandando
    /// `LoggedIn`/`spawn_gateway_with_token` (ver `poll_discord_events`).
    PasswordLoginToken(String),
    /// Discord pide 2FA antes de terminar el login por contraseña.
    PasswordMfaRequired(password_auth::MfaChallenge),
    /// Se mandó el SMS de 2FA pedido (`password_auth::spawn_mfa_sms`).
    /// El teléfono es el que Discord devuelve enmascarado
    /// (`+54 9 11 **** **67`, por ejemplo), si lo manda.
    PasswordMfaSmsSent(Option<String>),
    /// Discord pide completar algo más (cambiar la contraseña, verificar
    /// el mail...) antes de dejar terminar el login. No hay forma de
    /// resolver eso desde acá (haría falta abrir un navegador), así que
    /// por ahora se muestra como una lista de qué hace falta.
    PasswordLoginBlocked(Vec<String>),
    /// Perfil completo de un usuario, pedido al clickearlo (ver
    /// `App::open_user_profile` / `spawn_fetch_user_profile`): bio,
    /// banner, servers y amigos en común, y roles en el server desde el
    /// que se abrió. `user_id` es el que se pidió (para poder ignorar la
    /// respuesta si mientras tanto se abrió el perfil de otra persona).
    UserProfileFetched {
        user_id: String,
        profile: UserProfileResponse,
    },
    /// El pedido de perfil (`spawn_fetch_user_profile`) falló — se
    /// distingue de `Error` porque no queremos tirar un toast genérico
    /// por esto, solo sacar el spinner de la tarjeta y mostrar un aviso
    /// puntual ahí adentro (ver `ui::profile_popup`).
    UserProfileFailed { user_id: String },
    /// El login (o la verificación de 2FA) por contraseña falló. Aparte
    /// de `Error` porque hay que volver a mostrar el formulario de
    /// contraseña con el error, no la pantalla de login entera.
    PasswordLoginFailed(String),
    /// Estado de la conexión de voz propia (`voice::VoiceStatusPublisher`):
    /// conectando/conectado/caído. `scope` es el guild id o, en una
    /// llamada de DM/grupo, el channel id (ver `VoiceScope::server_id_string`).
    VoiceConnectionStatusChanged {
        scope: String,
        channel_id: Option<String>,
        status: voice::VoiceConnectionStatus,
        message: Option<String>,
    },
    /// Alguien empezó/dejó de hablar en el canal de voz que tenemos
    /// abierto (detectado del propio audio RTP entrante, no un evento de
    /// Discord).
    VoiceSpeakingUpdate {
        channel_id: String,
        user_id: String,
        speaking: bool,
    },
    /// El hilo de voz no pudo aplicar el dispositivo de entrada/salida
    /// pedido (se abrió otro por default, `active_*_source`) y quedó
    /// usando el que ya tenía.
    VoiceAudioSourcesApplyFailed {
        requested_input_source: Option<String>,
        requested_output_source: Option<String>,
        active_input_source: Option<String>,
        active_output_source: Option<String>,
        message: String,
    },
    /// `CHANNEL_CREATE` / `CHANNEL_UPDATE` de un server: un canal nuevo, o
    /// uno existente que cambió de nombre, posición, categoría o permisos.
    ChannelUpsert { guild_id: String, channel: Channel },
    /// `CHANNEL_DELETE` de un server.
    ChannelDelete { guild_id: String, channel_id: String },
    /// `GUILD_ROLE_CREATE` / `GUILD_ROLE_UPDATE`: rol nuevo o modificado
    /// (nombre, color, permisos...). Cambiar sus permisos cambia qué
    /// canales se ven.
    GuildRoleUpsert { guild_id: String, role: Role },
    /// `GUILD_ROLE_DELETE`.
    GuildRoleDelete { guild_id: String, role_id: String },
    /// Página de posts de un foro (`spawn_fetch_forum_posts`). `offset` y
    /// `by_creation` son los del pedido, para descartar respuestas viejas.
    ForumPosts {
        channel_id: String,
        offset: u32,
        by_creation: bool,
        page: ForumPage,
    },
    /// El pedido de posts de un foro falló (se saca el "cargando").
    ForumPostsFailed { channel_id: String },
    /// Se publicó un post nuevo desde este cliente
    /// (`spawn_create_forum_post`).
    ForumPostCreated { forum_id: String, thread: ThreadChannel },
    /// Falló la publicación de un post (se reactiva el formulario).
    ForumPostCreateFailed { forum_id: String },
    /// `THREAD_CREATE` / `THREAD_UPDATE`: post de foro nuevo o modificado
    /// (título, etiquetas, fijado...).
    ThreadUpsert(ThreadChannel),
    /// `THREAD_DELETE`.
    ThreadDelete { parent_id: String, thread_id: String },
    /// `INTERACTION_MODAL_CREATE`: un bot contestó a un botón pidiendo un
    /// formulario (ver `ModalRequest`).
    ModalCreate(Box<models::ModalRequest>),
    /// `INTERACTION_FAILURE`, o el `POST /interactions` falló: el botón no
    /// hizo nada. Se avisa con un toast.
    InteractionFailed { message: String },
    /// Se pudo crear un hilo desde un mensaje (`spawn_create_thread`); el
    /// panel lo abre enseguida.
    ThreadOpened { thread: ThreadChannel },
}

/// Arranca todo el flujo en un hilo de SO nuevo con su propio runtime de
/// tokio (así `eframe`, que es sync, no se entera de que existe async por
/// debajo). Devuelve enseguida; los resultados llegan por `tx`.
pub fn spawn_login_flow(tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(AppEvent::Error(format!(
                    "No se pudo iniciar el runtime async: {e}"
                )));
                return;
            }
        };

        rt.block_on(async move {
            let fingerprint = init_fingerprint().await;
            match remote_auth::run(fingerprint.clone(), tx.clone()).await {
                Ok(token) => {
                    // Validamos el token contra Discord a través del REST de
                    // UwUDev (impersonación TLS de Chrome) antes de darlo
                    // por bueno. Si el QR devolvió algo que Discord no
                    // acepta, nos enteramos acá en vez de recién al fallar
                    // el gateway más adelante.
                    if let Err(e) = uwu_rest::UwuRest::for_token(token.clone()).await {
                        let _ = tx.send(AppEvent::Error(format!(
                            "El token que dio el QR no pasó la validación: {e}"
                        )));
                        return;
                    }

                    let _ = tx.send(AppEvent::LoggedIn {
                        token: token.clone(),
                    });
                    let (commands_tx, commands_rx) = tokio::sync::mpsc::unbounded_channel();
                    let _ = tx.send(AppEvent::GatewayCommands(commands_tx));
                    if let Err(e) = gateway::run(token, fingerprint, tx.clone(), commands_rx).await {
                        let _ = tx.send(AppEvent::Error(format!(
                            "Se cortó la conexión con Discord: {e}"
                        )));
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("No se pudo iniciar sesión: {e}")));
                }
            }
        });
    });
}

/// Lee el build number vigente de Discord Web y los identificadores de
/// sesión, deja ese fingerprint como el compartido por todos los
/// `RestClient` (ver `rest::set_shared_fingerprint`) y lo devuelve para
/// que el Gateway (`gateway::run`) también lo use en el `IDENTIFY` — antes
/// mandaba un `properties` inventado sin relación con lo que ya usa REST.
/// Si no hay red o tarda demasiado se sigue con el fingerprint de
/// respaldo (`ClientFingerprint::new` con el build number harcodeado):
/// nunca bloquea ni rompe el inicio de sesión.
async fn init_fingerprint() -> std::sync::Arc<fingerprint::ClientFingerprint> {
    let loading = fingerprint::load_client_fingerprint_and_http();
    let (fingerprint, _http) =
        match tokio::time::timeout(std::time::Duration::from_secs(10), loading).await {
            Ok(pair) => pair,
            Err(_) => {
                log::warn!("No se pudo cargar el fingerprint a tiempo; se usa el de respaldo");
                (
                    std::sync::Arc::new(fingerprint::ClientFingerprint::new(fingerprint::CLIENT_BUILD_NUMBER)),
                    reqwest::Client::new(),
                )
            }
        };
    rest::set_shared_fingerprint(fingerprint.clone());
    uwu_rest::set_shared_fingerprint(fingerprint.clone());
    fingerprint
}

/// Pide (o abre) el canal de DM con `user_id` y trae su historial reciente.
/// Se llama al abrir una conversación por primera vez (`App::open_dm`); el
/// resultado llega como `AppEvent::DmOpened`.
pub fn spawn_fetch_dm(token: String, user_id: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("No se pudo abrir el DM: {e}")));
                    return;
                }
            };
            match rest.open_dm(&user_id).await {
                Ok(channel_id) => {
                    let messages = rest
                        .channel_messages(&channel_id, uwu_rest::MESSAGES_PAGE_SIZE)
                        .await
                        .unwrap_or_default();
                    let _ = tx.send(AppEvent::DmOpened {
                        user_id,
                        channel_id,
                        messages,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("No se pudo abrir el DM: {e}")));
                }
            }
        });
    });
}

/// Pide los canales de texto de un server. Se llama al abrirlo por primera
/// vez (`App::open_server`); el resultado llega como
/// `AppEvent::GuildChannels`.
pub fn spawn_fetch_guild_channels(
    token: String,
    guild_id: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudieron traer los canales del server: {e}"
                    )));
                    return;
                }
            };
            match rest.guild_channels(&guild_id).await {
                Ok(channels) => {
                    let _ = tx.send(AppEvent::GuildChannels { guild_id, channels });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudieron traer los canales del server: {e}"
                    )));
                }
            }
        });
    });
}

/// Pide el historial reciente de un canal de texto de server. Se llama al
/// abrirlo por primera vez (`App::open_channel`); el resultado llega como
/// `AppEvent::ChannelMessages`.
pub fn spawn_fetch_channel_messages(
    token: String,
    channel_id: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudo traer el historial del canal: {e}"
                    )));
                    return;
                }
            };
            match rest.channel_messages(&channel_id, uwu_rest::MESSAGES_PAGE_SIZE).await {
                Ok(messages) => {
                    let _ = tx.send(AppEvent::ChannelMessages { channel_id, messages });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudo traer el historial del canal: {e}"
                    )));
                }
            }
        });
    });
}

/// Pide una página de posts de un canal de foro (`offset` = cuántos ya se
/// tienen). El resultado llega como `AppEvent::ForumPosts`.
pub fn spawn_fetch_forum_posts(
    token: String,
    channel_id: String,
    offset: u32,
    by_creation: bool,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::ForumPostsFailed { channel_id });
            return;
        };
        rt.block_on(async move {
            let result = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest.forum_threads(&channel_id, offset, by_creation).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(page) => {
                    let _ = tx.send(AppEvent::ForumPosts { channel_id, offset, by_creation, page });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ForumPostsFailed { channel_id });
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudieron traer las publicaciones del foro: {e}"
                    )));
                }
            }
        });
    });
}

/// Publica un post nuevo en un foro. El resultado llega como
/// `AppEvent::ForumPostCreated` (o `ForumPostCreateFailed` + `Error`).
pub fn spawn_create_forum_post(
    token: String,
    forum_id: String,
    title: String,
    content: String,
    tag_ids: Vec<String>,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::ForumPostCreateFailed { forum_id });
            return;
        };
        rt.block_on(async move {
            let result = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest.create_forum_post(&forum_id, &title, &content, &tag_ids).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(thread) => {
                    let _ = tx.send(AppEvent::ForumPostCreated { forum_id, thread });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ForumPostCreateFailed { forum_id });
                    let _ = tx.send(AppEvent::Error(format!("No se pudo publicar el post: {e}")));
                }
            }
        });
    });
}

/// Pide la página de mensajes anterior a `before_message_id` de un canal
/// o DM ya abierto — "cargar más" al llegar arriba del todo del historial
/// visible (ver `ui::chat::ChatEvent::LoadMoreRequested` y
/// `App::load_more_messages`). El resultado llega como
/// `AppEvent::MoreChannelMessages`.
pub fn spawn_fetch_more_channel_messages(
    token: String,
    channel_id: String,
    before_message_id: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    println!("load_more_messages");
    println!("{:?}", channel_id);
    println!("{:?}", before_message_id);
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudieron cargar mensajes más viejos: {e}"
                    )));
                    return;
                }
            };
            match rest
                .channel_messages_before(&channel_id, uwu_rest::MESSAGES_PAGE_SIZE, &before_message_id)
                .await
            {
                Ok(messages) => {
                    let _ = tx.send(AppEvent::MoreChannelMessages { channel_id, messages });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!(
                        "No se pudieron cargar mensajes más viejos: {e}"
                    )));
                }
            }
        });
    });
}

/// Datos de un usuario por id, para completar un estado de voz que llegó
/// sin `member`/`user` (ver `AppEvent::UserFetched`). Si falla, no
/// molestamos con un toast de error — el peor caso es que esa persona
/// se quede como "Usuario desconocido" en la lista de voz, no algo que
/// justifique interrumpir al usuario.
/// Trae el `PreloadedUserSettings` real de la cuenta (mismo blob que
/// sincronizan entre sí el cliente oficial de escritorio/web/mobile) y lo
/// manda como `AppEvent::UserSettings`. Se llama una vez, justo después
/// de `LoggedIn`/`Ready` (ver `App::poll_discord_events`) — no hay push
/// en vivo de esto por el Gateway en este cliente, así que si el usuario
/// cambia algo desde otro dispositivo mientras `ecord` está abierto no se
/// entera hasta el próximo login.
pub fn spawn_fetch_user_settings(token: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            // De estos ajustes sale el ORDEN de la barra de servers: si el
            // pedido falla una vez (red, rate limit, bootstrap de la
            // sesión) no se puede quedar sin reintentar, o los servers
            // salen en el orden crudo del READY para toda la sesión.
            const DELAYS_SECS: [u64; 5] = [0, 2, 5, 10, 30];
            for delay in DELAYS_SECS {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
                let Ok(rest) = uwu_rest::UwuRest::for_token(token.clone()).await else {
                    continue;
                };
                if let Ok(settings) = rest.get_user_settings().await {
                    let _ = tx.send(AppEvent::UserSettings(Box::new(settings)));
                    return;
                }
            }
        });
    });
}

pub fn spawn_fetch_user(token: String, user_id: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let Ok(rest) = uwu_rest::UwuRest::for_token(token).await else {
                return;
            };
            if let Ok(user) = rest.get_user(&user_id).await {
                let _ = tx.send(AppEvent::UserFetched(user));
            }
        });
    });
}

/// Cuánto valen en disco el catálogo de efectos y los productos de
/// coleccionables (`support::http_cache::json_*`).
const CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Cuánto vale un perfil de usuario ya pedido antes de volver a la API.
const PROFILE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Tope de perfiles guardados en memoria.
const PROFILE_CACHE_MAX: usize = 256;

type ProfileKey = (String, Option<String>);

/// Perfiles ya pedidos (`GET /users/{id}/profile`), por (usuario, server).
static PROFILE_CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<ProfileKey, (std::time::Instant, UserProfileResponse)>>,
> = std::sync::Mutex::new(None);

/// `(perfil, ¿todavía fresco?)`. Uno vencido se devuelve igual (`false`): sirve
/// de respaldo si el pedido nuevo falla.
fn profile_cache_get(user_id: &str, guild_id: Option<&str>) -> Option<(UserProfileResponse, bool)> {
    let guard = PROFILE_CACHE.lock().ok()?;
    let (at, profile) = guard.as_ref()?.get(&(user_id.to_string(), guild_id.map(str::to_string)))?;
    Some((profile.clone(), at.elapsed() < PROFILE_TTL))
}

fn profile_cache_put(user_id: &str, guild_id: Option<&str>, profile: &UserProfileResponse) {
    let Ok(mut guard) = PROFILE_CACHE.lock() else { return };
    let map = guard.get_or_insert_with(Default::default);
    if map.len() >= PROFILE_CACHE_MAX {
        // Se tiran los de más de 10 minutos; si sigue lleno, todo.
        map.retain(|_, (at, _)| at.elapsed() < PROFILE_TTL * 10);
        if map.len() >= PROFILE_CACHE_MAX {
            map.clear();
        }
    }
    map.insert(
        (user_id.to_string(), guild_id.map(str::to_string)),
        (std::time::Instant::now(), profile.clone()),
    );
}

/// Catálogo de efectos de perfil (`GET /user-profile-effects`), cacheado: es
/// el mismo para todos los usuarios y bastante pesado, así que se pide una
/// sola vez por sesión.
static PROFILE_EFFECT_CONFIGS: std::sync::Mutex<Option<serde_json::Value>> =
    std::sync::Mutex::new(None);

/// Resuelve las capas del efecto `effect_id` (ver `ProfileEffect`).
async fn fetch_profile_effect(
    rest: &uwu_rest::UwuRest,
    effect_id: &str,
) -> Option<models::ProfileEffect> {
    let cached = PROFILE_EFFECT_CONFIGS.lock().ok().and_then(|guard| guard.clone());
    // Memoria → disco (1 día) → red.
    let cached = cached.or_else(|| {
        crate::support::http_cache::json_read("profile_effects", CATALOG_TTL)
    });
    let configs = match cached {
        Some(configs) => {
            if let Ok(mut guard) = PROFILE_EFFECT_CONFIGS.lock() {
                guard.get_or_insert_with(|| configs.clone());
            }
            configs
        }
        None => {
            let configs = match rest.profile_effects().await {
                Ok(configs) => configs,
                Err(e) => {
                    log::warn!("No se pudo traer el catálogo de efectos de perfil: {e}");
                    return None;
                }
            };
            crate::support::http_cache::json_write("profile_effects", &configs);
            if let Ok(mut guard) = PROFILE_EFFECT_CONFIGS.lock() {
                *guard = Some(configs.clone());
            }
            configs
        }
    };
    let effect = models::ProfileEffect::from_configs(&configs, effect_id);
    match &effect {
        Some(e) => log::debug!("efecto de perfil {effect_id}: {} capas", e.layers.len()),
        None => log::warn!(
            "efecto de perfil {effect_id}: no está en el catálogo o no se pudieron leer sus capas \
             (¿cambió el formato de /user-profile-effects?)"
        ),
    }
    effect
}

/// Productos de coleccionables ya pedidos (`GET /collectibles-products/{sku}`),
/// por `sku_id`: no cambian, así que se piden una sola vez por sesión.
static COLLECTIBLE_PRODUCTS: std::sync::Mutex<
    Option<std::collections::HashMap<String, serde_json::Value>>,
> = std::sync::Mutex::new(None);

async fn fetch_collectible_product(
    rest: &uwu_rest::UwuRest,
    sku_id: &str,
) -> Option<serde_json::Value> {
    let cached = COLLECTIBLE_PRODUCTS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|map| map.get(sku_id).cloned()));
    if cached.is_some() {
        return cached;
    }
    // Disco (los productos casi no cambian) antes de pedirlo a la API.
    let disk_key = format!("collectible_{sku_id}");
    if let Some(product) = crate::support::http_cache::json_read(&disk_key, CATALOG_TTL) {
        if let Ok(mut guard) = COLLECTIBLE_PRODUCTS.lock() {
            guard.get_or_insert_with(Default::default).insert(sku_id.to_string(), product.clone());
        }
        return Some(product);
    }
    match rest.collectible_product(sku_id).await {
        Ok(product) => {
            crate::support::http_cache::json_write(&disk_key, &product);
            if let Ok(mut guard) = COLLECTIBLE_PRODUCTS.lock() {
                guard.get_or_insert_with(Default::default).insert(sku_id.to_string(), product.clone());
            }
            Some(product)
        }
        Err(e) => {
            log::warn!("No se pudo traer el coleccionable {sku_id}: {e}");
            None
        }
    }
}

/// Efecto de perfil desde su producto (`items[]` con `type` 1).
async fn fetch_profile_effect_by_sku(
    rest: &uwu_rest::UwuRest,
    sku_id: &str,
) -> Option<models::ProfileEffect> {
    let product = fetch_collectible_product(rest, sku_id).await?;
    let item = models::collectible_item(&product, models::COLLECTIBLE_PROFILE_EFFECT)?;
    let effect = models::ProfileEffect::from_entry(item);
    if effect.is_none() {
        log::warn!("efecto de perfil {sku_id}: el producto no trae capas legibles");
    }
    effect
}

/// Profile frame desde su producto (`items[]` con `type` 3).
async fn fetch_profile_frame(rest: &uwu_rest::UwuRest, sku_id: &str) -> Option<models::ProfileFrame> {
    let product = fetch_collectible_product(rest, sku_id).await?;
    let item = models::collectible_item(&product, models::COLLECTIBLE_PROFILE_FRAME)?;
    let frame = models::ProfileFrame::from_item(item);
    match &frame {
        Some(f) => log::debug!(
            "profile frame {sku_id}: {} capas, inner_width={}, overflow arriba={} abajo={} lados={}",
            f.layers.len(),
            f.inner_width,
            f.overflow_top,
            f.overflow_bottom,
            f.overflow_horizontal
        ),
        None => log::warn!("profile frame {sku_id}: el producto no trae capas legibles"),
    }
    frame
}

/// Trae el perfil completo de `user_id` (bio, banner, servers/amigos en
/// común, roles en `guild_id` si se pasa uno) para la tarjeta que se abre
/// al clickearlo. Se llama desde `App::open_user_profile`; el resultado
/// llega como `AppEvent::UserProfileFetched` (o `UserProfileFailed` si el
/// pedido falla — a diferencia de `spawn_fetch_user`, acá sí nos importa
/// avisarle a la tarjeta para que saque el spinner).
pub fn spawn_fetch_user_profile(
    token: String,
    user_id: String,
    guild_id: Option<String>,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    // Perfil pedido hace menos de `PROFILE_TTL`: se reusa sin tocar la API.
    let stale = match profile_cache_get(&user_id, guild_id.as_deref()) {
        Some((profile, true)) => {
            let _ = tx.send(AppEvent::UserProfileFetched { user_id, profile });
            return;
        }
        Some((profile, false)) => Some(profile),
        None => None,
    };
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::UserProfileFailed { user_id });
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    log::warn!("No se pudo loguear el REST para el perfil de {user_id}: {e}");
                    let _ = tx.send(match stale {
                        Some(profile) => AppEvent::UserProfileFetched { user_id, profile },
                        None => AppEvent::UserProfileFailed { user_id },
                    });
                    return;
                }
            };
            match rest.user_profile(&user_id, guild_id.as_deref()).await {
                Ok(mut profile) => {
                    // Si tiene un efecto de perfil equipado, se resuelve acá
                    // (segundo pedido, con caché) para que la tarjeta ya
                    // llegue completa. Si falla, la tarjeta se ve igual,
                    // solo sin el efecto.
                    // Efecto: primero por el producto del coleccionable (`type` 1);
                    // si no viene así, por el catálogo (`profile_effect.id`).
                    let effect_by_sku = match profile.profile_effect_sku() {
                        Some(sku) => fetch_profile_effect_by_sku(&rest, &sku).await,
                        None => None,
                    };
                    profile.effect = match effect_by_sku {
                        Some(effect) => Some(effect),
                        None => match profile.profile_effect_id() {
                            Some(effect_id) => fetch_profile_effect(&rest, &effect_id).await,
                            None => None,
                        },
                    };
                    // Frame (`type` 3). El array `collectibles` puede traer ambos
                    // tipos a la vez; cada uno se busca por su `type`.
                    if let Some(sku) = profile.profile_frame_sku() {
                        profile.frame = fetch_profile_frame(&rest, &sku).await;
                    }
                    profile_cache_put(&user_id, guild_id.as_deref(), &profile);
                    let _ = tx.send(AppEvent::UserProfileFetched { user_id, profile });
                }
                Err(e) => {
                    log::warn!("No se pudo traer el perfil de {user_id}: {e}");
                    // Si había uno viejo en caché, mejor mostrarlo que un error.
                    let _ = tx.send(match stale {
                        Some(profile) => AppEvent::UserProfileFetched { user_id, profile },
                        None => AppEvent::UserProfileFailed { user_id },
                    });
                }
            }
        });
    });
}

/// Manda un mensaje por REST sin esperar la respuesta (no bloquea el
/// frame). Si falla, por ahora se pierde en silencio — llamado desde
/// `ui::chat::composer` cuando el canal/DM abierto es uno real.
/// `reply_to`: id del mensaje al que se está respondiendo (banner
/// "Respondiendo a..." del composer), si había uno puesto al mandar.
/// `guild_id`: server del canal (`None` en un DM), para mandar el `Referer`
/// correcto (ver `UwuRest::send_message`).
pub fn spawn_send_message(
    token: String,
    channel_id: String,
    guild_id: Option<String>,
    content: String,
    reply_to: Option<String>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let Ok(rest) = uwu_rest::UwuRest::for_token(token).await else {
                return;
            };
            if let Err(e) = rest
                .send_message(&channel_id, guild_id.as_deref(), &content, reply_to.as_deref())
                .await
            {
                log::warn!("No se pudo mandar el mensaje: {e}");
            }
        });
    });
}

/// Pone o saca la reacción propia (`emoji`) en un mensaje ya mandado,
/// sin bloquear el frame. Llamado desde `ui::chat` cuando el usuario
/// clickea una reacción (o el botón de "agregar reacción") en un mensaje
/// real (con `send_target` y `id` — los mensajes de demo no llaman esto).
/// Si falla, por ahora se pierde en silencio, como `spawn_send_message`:
/// el eco local ya se aplicó optimistamente en `ChatMessage::toggle_reaction`.
pub fn spawn_toggle_reaction(
    token: String,
    channel_id: String,
    message_id: String,
    emoji: String,
    adding: bool,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    log::warn!("No se pudo loguear el REST para la reacción: {e}");
                    return;
                }
            };
            let result = if adding {
                rest.add_reaction(&channel_id, &message_id, &emoji).await
            } else {
                rest.remove_reaction(&channel_id, &message_id, &emoji).await
            };
            if let Err(e) = result {
                log::warn!("No se pudo actualizar la reacción: {e}");
            }
        });
    });
}

/// Le avisa a Discord que `channel_id` está leído hasta `message_id`
/// (`POST /channels/{id}/messages/{id}/ack`). Sin esto, Discord sigue
/// contando esos mensajes como no leídos y al reiniciar el cliente
/// vuelven a aparecer. Si falla se registra y se sigue: no vale la pena
/// interrumpir al usuario con un toast por esto.
pub fn spawn_ack_message(token: String, channel_id: String, message_id: String) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    log::warn!("No se pudo loguear el REST para marcar como leído: {e}");
                    return;
                }
            };
            if let Err(e) = rest.ack_message(&channel_id, &message_id).await {
                log::warn!("No se pudo marcar el canal {channel_id} como leído: {e}");
            }
        });
    });
}

/// Igual que `spawn_login_flow`, pero saltando el QR: usa un token que ya
/// tenemos (guardado de una sesión anterior en `localStorage`, ver
/// `lib::state::UserDB`) y va directo al Gateway.
pub fn spawn_gateway_with_token(token: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::Error("No se pudo iniciar el runtime async".into()));
            return;
        };
        rt.block_on(async move {
            let fingerprint = init_fingerprint().await;

            if let Err(e) = uwu_rest::UwuRest::for_token(token.clone()).await {
                let _ = tx.send(AppEvent::Error(format!(
                    "El token guardado ya no es válido: {e}"
                )));
                return;
            }

            let _ = tx.send(AppEvent::LoggedIn { token: token.clone() });
            let (commands_tx, commands_rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(AppEvent::GatewayCommands(commands_tx));
            if let Err(e) = gateway::run(token, fingerprint, tx.clone(), commands_rx).await {
                let _ = tx.send(AppEvent::Error(format!(
                    "Se cortó la conexión con Discord: {e}"
                )));
            }
        });
    });
}

/// Datos comunes de una interacción de usuario (`POST /interactions`).
#[derive(Clone, Debug)]
pub struct InteractionContext {
    pub session_id: String,
    pub application_id: String,
    pub guild_id: Option<String>,
    pub channel_id: String,
}

/// Aprieta un botón de un mensaje (interacción tipo 3, `MESSAGE_COMPONENT`).
/// La respuesta del bot llega después por el Gateway (un mensaje nuevo, una
/// edición o un modal); si el POST falla se avisa con
/// `AppEvent::InteractionFailed`.
pub fn spawn_press_button(
    token: String,
    ctx: InteractionContext,
    message_id: String,
    message_flags: u64,
    custom_id: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let result = async {
                let rest = uwu_rest::UwuRest::for_token(token).await?;
                rest.press_button(&ctx, &message_id, message_flags, &custom_id).await
            }
            .await;
            if let Err(e) = result {
                log::warn!("No se pudo apretar el botón {custom_id}: {e}");
                let _ = tx.send(AppEvent::InteractionFailed {
                    message: "La aplicación no respondió a ese botón.".to_string(),
                });
            }
        });
    });
}

/// Manda un modal completado (interacción tipo 5, `MODAL_SUBMIT`).
/// `fields` son `(custom_id del campo, texto escrito)`.
pub fn spawn_submit_modal(
    token: String,
    ctx: InteractionContext,
    modal_interaction_id: String,
    modal_custom_id: String,
    components: Vec<models::Component>,
    fields: Vec<(String, String)>,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let result = async {
                let rest = uwu_rest::UwuRest::for_token(token).await?;
                rest.submit_modal(&ctx, &modal_interaction_id, &modal_custom_id, &components, &fields).await
            }
            .await;
            if let Err(e) = result {
                log::warn!("No se pudo mandar el formulario {modal_custom_id}: {e}");
                let _ = tx.send(AppEvent::InteractionFailed {
                    message: "No se pudo enviar el formulario.".to_string(),
                });
            }
        });
    });
}

/// Crea un hilo a partir de un mensaje (`POST /channels/{id}/messages/{id}/threads`)
/// y avisa con `AppEvent::ThreadOpened` para abrirlo.
pub fn spawn_create_thread(
    token: String,
    channel_id: String,
    message_id: String,
    name: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let result = async {
                let rest = uwu_rest::UwuRest::for_token(token).await?;
                rest.create_thread_from_message(&channel_id, &message_id, &name).await
            }
            .await;
            match result {
                Ok(thread) => {
                    let _ = tx.send(AppEvent::ThreadOpened { thread });
                }
                Err(e) => {
                    log::warn!("No se pudo crear el hilo: {e}");
                    let _ = tx.send(AppEvent::InteractionFailed {
                        message: "No se pudo crear el hilo.".to_string(),
                    });
                }
            }
        });
    });
}
