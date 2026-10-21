//! Transmitir nuestra pantalla (Go Live propio).
//!
//! Espejo de `stream_watch`: el stream tiene su propia conexión de media,
//! separada de la de voz del canal. El flujo completo es:
//!
//! 1. `GatewayCommand::StreamCreate` (opcode 18 del Gateway principal). Discord
//!    responde con `STREAM_CREATE` (con el `rtc_server_id`) y
//!    `STREAM_SERVER_UPDATE` (endpoint + token), y `App` abre este módulo
//!    cuando tiene los tres datos (`App::try_start_stream_publish`).
//! 2. WebSocket de media: IDENTIFY con `video: true` y un stream `screen`,
//!    READY (SSRC de audio, de video y de RTX), SELECT_PROTOCOL con H.264
//!    marcado como `encode: true`, SESSION_DESCRIPTION y el handshake MLS de
//!    DAVE (el mismo `VoiceDaveState` que usa el visor).
//! 3. Opcode 12 (VIDEO) anunciando el SSRC de video y la resolución.
//! 4. Cada access unit que sale de `screen_capture` se cifra con DAVE, se
//!    paquetiza en RTP (`h264`), se cifra con el AEAD del transporte y se manda
//!    por UDP.
//!
//! Como el visor, NO pasa por la máquina de estados de `runtime.rs`: corre en
//! su propio hilo con su propio runtime de tokio.
//!
//! El audio de la ventana/pantalla solo se transmite por WebRTC (`stream_audio.rs`,
//! Windows); el camino UDP clásico sigue siendo solo video.

use super::dave::{VoiceDaveOutboundPayload, handles_gateway_json_op};
use super::gateway::{
    choose_encryption_mode, discover_voice_udp_address, parse_voice_binary_frame, parse_voice_ready_payload,
    parse_voice_session_description, run_voice_heartbeat, run_voice_udp_ping, send_voice_text, voice_gateway_opcode,
    voice_gateway_url,
};
use super::loss_control::LossController;
use super::h264::{
    EncodedFrame, PlayoutDelay, RTP_VIDEO_CLOCK_HZ, RTP_VIDEO_MAX_PAYLOAD, build_video_rtp_packet, packetize_h264_access_unit,
    playout_delay, split_annexb_nals,
};
use super::rtp::{VoiceRtpDecryptor, looks_like_rtcp_packet};
use super::screen_capture::{KeyframeRequest, ScreenCaptureConfig, encoder_settings_generation, start_screen_encoder};
use super::*;

// Transporte WebRTC opt-in (devtools o `ECORD_STREAM_TRANSPORT=webrtc`).
mod webrtc;

/// Cómo viaja el video de la transmisión propia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamTransport {
    /// UDP con cifrado AEAD (el camino clásico; `ECORD_STREAM_TRANSPORT=udp`).
    Udp,
    /// WebRTC (DTLS-SRTP), el predeterminado; como la librería de Node `Discord-video-stream`.
    WebRtc,
}

impl StreamTransport {
    pub(crate) const CHOICES: [Self; 2] = [Self::Udp, Self::WebRtc];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Udp => "UDP (AEAD)",
            Self::WebRtc => "WebRTC (DTLS-SRTP)",
        }
    }
}

/// Lo que eligen las devtools en esta sesión: gana sobre `ECORD_STREAM_TRANSPORT`.
static STREAM_TRANSPORT_OVERRIDE: std::sync::Mutex<Option<StreamTransport>> = std::sync::Mutex::new(None);

pub(crate) fn stream_transport_override() -> Option<StreamTransport> {
    *STREAM_TRANSPORT_OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `None` vuelve a `ECORD_STREAM_TRANSPORT` (o a WebRTC).
pub(crate) fn set_stream_transport_override(value: Option<StreamTransport>) {
    *STREAM_TRANSPORT_OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// El transporte que usaría una transmisión que arrancara ahora. Cada sesión lo lee
/// una sola vez al empezar: cambiarlo con una transmisión en curso no la toca.
pub(crate) fn stream_transport() -> StreamTransport {
    stream_transport_override().unwrap_or_else(|| {
        // WebRTC es el predeterminado; `ECORD_STREAM_TRANSPORT=udp` vuelve al camino clásico.
        if std::env::var("ECORD_STREAM_TRANSPORT").is_ok_and(|value| value.trim().eq_ignore_ascii_case("udp")) {
            StreamTransport::Udp
        } else {
            StreamTransport::WebRtc
        }
    })
}

/// Payload types: los mismos que anuncia el visor y los clientes oficiales.
const PUBLISH_OPUS_PAYLOAD_TYPE: u8 = 120;
const PUBLISH_H264_PAYLOAD_TYPE: u8 = 101;
const PUBLISH_H264_RTX_PAYLOAD_TYPE: u8 = 102;

/// Frames codificados esperando a salir. 15 son ~0,5 s a 30 fps.
const PUBLISH_QUEUE: usize = 15;

/// Pacing por reloj: los paquetes de un frame (sobre todo los de una IDR) no se
/// vuelcan de golpe a la red, pero tampoco se duerme "un poquito" entre grupos.
/// En Windows `sleep(400µs)` dura 1-15 ms según la resolución del timer, y con
/// ~25-300 paquetes por IDR esas pausas se sumaban hasta llenar la cola de
/// salida: se tiraban frames, se esperaba otra IDR (más pesada) y la imagen
/// quedaba congelada varios segundos. Ahora cada paquete tiene una hora de
/// salida calculada (`bytes acumulados / velocidad`) y solo se duerme si se va
/// adelantado más de `PACING_MIN_SLEEP`; si un sleep se pasa, los paquetes
/// siguientes salen sin esperar hasta recuperar el atraso.
///
/// La velocidad es adaptativa, como el `PacingHandler` de libdatachannel que usa
/// la implementación en Node: `1,25 ×` el bitrate realmente enviado en los
/// últimos 250 ms, con un piso de 1 Mbps. Antes era fija (6 × el bitrate
/// nominal): los frames P salían en ráfagas de ~4 ms y la IDR a ~27 Mbps, y un
/// router/Wi-Fi con poco upload perdía la cola; como cada paquete perdido
/// congela al espectador hasta la próxima IDR, la imagen se congelaba casi la
/// mitad del tiempo.
const PACING_WINDOW: Duration = Duration::from_millis(250);
/// Margen sobre el bitrate medido (en %).
const PACING_HEADROOM_PERCENT: u64 = 125;
/// Piso absoluto de la velocidad del pacer (bytes/s, 1 Mbps).
const PACING_MIN_BYTES_PER_SEC: u64 = 125_000;
/// Piso relativo al bitrate configurado (en %). Sin él, tras unos instantes de
/// imagen quieta el bitrate medido es casi 0 y un frame P de 8-13 KB salía a
/// 1 Mbps (35-45 ms, más que un intervalo de frame) porque, a diferencia de
/// libdatachannel, acá cada frame se envía de forma secuencial y bloquea al
/// siguiente.
const PACING_FLOOR_PERCENT: u64 = 150;
/// Tiempo máximo en que se reparte un frame, aunque la velocidad adaptativa
/// pida más: una IDR de ~100 KB tardaría ~130 ms y retrasaría los frames
/// siguientes. Un freeze solo se nota pasados ~150 ms.
const PACING_MAX_SPREAD: Duration = Duration::from_millis(100);
const PACING_MIN_SLEEP: Duration = Duration::from_millis(2);
/// Tiempo máximo para repartir una IDR. Antes se repartía en hasta 100 ms
/// (a 2,1 ms por paquete del piso, ~70 ms), que a 60 fps bloqueaba ~4 frames.
const PACING_KEYFRAME_SPREAD: Duration = Duration::from_millis(35);
/// Un frame P debe salir como mucho en esta fracción (en %) de su intervalo, para
/// que el envío secuencial no retrase al frame siguiente.
const PACING_P_FRAME_INTERVAL_PERCENT: u64 = 75;
/// Mínimo absoluto del reparto de un frame P.
const PACING_P_FRAME_MIN_SPREAD: Duration = Duration::from_millis(8);
/// Qué parte (en %) del REMB del servidor se usa como bitrate de video. El resto
/// es margen para el audio, los reenvíos y las ráfagas.
const REMB_USE_PERCENT: u64 = 85;
/// Solo se baja el bitrate si el tope del REMB es menor que este % del actual
/// (evita reiniciar el codificador por variaciones pequeñas).
const REMB_RESTART_BELOW_PERCENT: u64 = 90;
/// Separación mínima entre dos reinicios del codificador por el REMB.
const REMB_RESTART_MIN_INTERVAL: Duration = Duration::from_secs(15);
/// Piso del bitrate de video al adaptarse al REMB (kbps).
const REMB_MIN_KBPS: u64 = 800;
/// Si enviar un frame tarda más que esto se deja una línea en el log.
const SLOW_FRAME_LOG: Duration = Duration::from_millis(35);

/// Ajustes de Windows para que la transmisión no sufra con la ventana en segundo
/// plano (es lo normal: se transmite un juego y ecord queda detrás):
///
/// * `timeBeginPeriod(1)`: sin esto los `sleep` y los timers duran 1-15 ms de más.
/// * Sin "power throttling" (EcoQoS): Windows 10/11 baja la prioridad y estira
///   los timers de los procesos que considera de fondo; el codificador llegaba a
///   tardar 150 ms en un frame y se veían envíos de 7 paquetes que demoraban 60 ms.
/// * Hilos del codificador y del envío con prioridad algo por encima de la normal.
pub(super) mod win_perf {
    pub(crate) struct Guard;

    #[cfg(windows)]
    impl Guard {
        pub(crate) fn new() -> Self {
            use windows_sys::Win32::Media::timeBeginPeriod;
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
                ProcessPowerThrottling, SetProcessInformation,
            };
            // SAFETY: llamadas simples a la API de Windows con estructuras propias.
            unsafe {
                timeBeginPeriod(1);
                // ControlMask con el bit y StateMask en 0 = "no limitar nunca".
                let state = PROCESS_POWER_THROTTLING_STATE {
                    Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                    ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                        | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
                    StateMask: 0,
                };
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&raw const state).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                );
            }
            boost_current_thread();
            Self
        }
    }

    #[cfg(windows)]
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: par de `timeBeginPeriod(1)`.
            unsafe {
                windows_sys::Win32::Media::timeEndPeriod(1);
            }
        }
    }

    #[cfg(not(windows))]
    impl Guard {
        pub(crate) fn new() -> Self {
            Self
        }
    }

    /// Prioridad algo mayor a la normal para el hilo actual.
    pub(crate) fn boost_current_thread() {
        #[cfg(windows)]
        // SAFETY: afecta solo al hilo que llama.
        unsafe {
            use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
        }
    }
}

/// Cuántos paquetes enviados se guardan para responder a los NACK (~0,7 s con
/// frames de 100 paquetes a 30 fps).
const RTX_HISTORY_PACKETS: usize = 2048;
/// Si un paquete ya reenviado vuelve a pedirse dentro de esta ventana, el
/// reenvío no llegó a servir.
const NACK_REPEAT_WINDOW: Duration = Duration::from_secs(2);
/// Tope de paquetes que se reenvían por cada RTCP de NACK.
const RTX_MAX_PER_RTCP: usize = 160;
/// El visor repite el NACK cada ~RTT hasta que le llega el reenvío: un paquete
/// que ya reenviamos hace menos de esto no se vuelve a mandar.
const RTX_MIN_RESEND_INTERVAL: Duration = Duration::from_millis(150);
/// Un NACK repetido solo cuenta como "el reenvío no sirvió" si pasó al menos
/// esto desde que lo reenviamos (si no, el reenvío todavía puede estar viajando
/// y pedir una IDR solo agrega una ráfaga más).
const NACK_REPEAT_MIN_AGE: Duration = Duration::from_millis(400);
/// Velocidad de los reenvíos (bytes/s, ~8 Mbps): antes salían 160 paquetes
/// seguidos de golpe.
const RTX_PACING_BYTES_PER_SEC: u64 = 1_000_000;

/// Un paquete de video ya enviado (el payload ya viene cifrado con DAVE).
struct SentPacket {
    sequence: u16,
    timestamp: u32,
    marker: bool,
    payload: Vec<u8>,
}

/// Ventana de los últimos paquetes enviados, con secuencias consecutivas.
#[derive(Default)]
struct PacketHistory {
    packets: std::collections::VecDeque<SentPacket>,
}

impl PacketHistory {
    fn push(&mut self, packet: SentPacket) {
        if self.packets.len() >= RTX_HISTORY_PACKETS {
            self.packets.pop_front();
        }
        self.packets.push_back(packet);
    }

    fn get(&self, sequence: u16) -> Option<&SentPacket> {
        let first = self.packets.front()?.sequence;
        let index = usize::from(sequence.wrapping_sub(first));
        self.packets.get(index).filter(|packet| packet.sequence == sequence)
    }
}

/// Reenvío (RTX, RFC 4588) de los paquetes que un espectador pidió con NACK.
/// Sin esto, un solo paquete perdido congelaba la imagen hasta la próxima IDR.
struct RtxSender {
    socket: Arc<UdpSocket>,
    encryptor: Arc<VoiceRtpEncryptor>,
    ssrc: u32,
    sequence: u16,
    history: Arc<std::sync::Mutex<PacketHistory>>,
    nonce: Arc<std::sync::atomic::AtomicU32>,
}

impl RtxSender {
    /// Devuelve `(reenviados, ya_no_estaban)`.
    async fn resend(&mut self, wanted: &[u16]) -> (u64, u64) {
        let mut found = Vec::new();
        let mut missing = 0u64;
        if let Ok(history) = self.history.lock() {
            for &sequence in wanted.iter().take(RTX_MAX_PER_RTCP) {
                match history.get(sequence) {
                    Some(packet) => found.push((
                        packet.sequence,
                        packet.timestamp,
                        packet.marker,
                        packet.payload.clone(),
                    )),
                    None => missing += 1,
                }
            }
        }
        let mut resent = 0u64;
        let started = Instant::now();
        let mut paced_bytes: u64 = 0;
        for (original_sequence, timestamp, marker, payload) in found {
            // Pacing por reloj, igual que el envío normal.
            let due = started + Duration::from_micros(paced_bytes * 1_000_000 / RTX_PACING_BYTES_PER_SEC);
            let ahead = due.saturating_duration_since(Instant::now());
            if ahead >= Duration::from_millis(2) {
                tokio::time::sleep(ahead).await;
            }
            // Payload RTX: número de secuencia original (OSN) + payload original.
            let mut body = Vec::with_capacity(2 + payload.len());
            body.extend_from_slice(&original_sequence.to_be_bytes());
            body.extend_from_slice(&payload);
            let packet = build_video_rtp_packet(
                self.sequence,
                timestamp,
                self.ssrc,
                PUBLISH_H264_RTX_PAYLOAD_TYPE,
                marker,
                &body,
            );
            let nonce = self.nonce.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Ok(encrypted) = self.encryptor.encrypt_media_packet(&packet, nonce.to_be_bytes()) else {
                continue;
            };
            if self.socket.send(&encrypted).await.is_err() {
                break;
            }
            paced_bytes += encrypted.len() as u64;
            self.sequence = self.sequence.wrapping_add(1);
            resent += 1;
        }
        (resent, missing)
    }
}

/// Estado de nuestra transmisión.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamPublishStatus {
    /// Abriendo el WebSocket / negociando UDP / arrancando la captura.
    Connecting,
    /// Transporte negociado; falta que salga el primer frame.
    Connected,
    /// Ya se está mandando video.
    Live,
    /// La conexión se cortó por un error (ver el mensaje).
    Failed,
    /// Se dejó de transmitir.
    Ended,
}

/// Datos que hacen falta para conectarse al servidor de media de nuestro stream.
#[derive(Clone)]
pub(crate) struct StreamPublishParams {
    pub(crate) stream_key: String,
    /// `rtc_server_id` de `STREAM_CREATE`: hace de `server_id` en el IDENTIFY.
    pub(crate) rtc_server_id: String,
    /// `endpoint` de `STREAM_SERVER_UPDATE`.
    pub(crate) endpoint: String,
    /// `token` de `STREAM_SERVER_UPDATE`.
    pub(crate) token: String,
    /// El propio usuario.
    pub(crate) user_id: Id<UserMarker>,
    /// `session_id` del Gateway principal.
    pub(crate) session_id: String,
    /// Qué se captura y a qué calidad.
    pub(crate) capture: ScreenCaptureConfig,
    /// Donde el codificador deja la vista previa de lo que se transmite (la
    /// UI la lee para el tile propio) y por donde se la pausa.
    pub(crate) preview: super::stream_watch::StreamFrameSlot,
}

impl fmt::Debug for StreamPublishParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamPublishParams")
            .field("stream_key", &self.stream_key)
            .field("rtc_server_id", &self.rtc_server_id)
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .field("user_id", &self.user_id)
            .field("session_id", &"<redacted>")
            .field("capture", &self.capture)
            .finish()
    }
}

/// Mango de una transmisión en curso. Al soltarlo (`Drop`) o al llamar a
/// `stop`, la conexión se cierra y se mata el codificador.
pub(crate) struct StreamPublishHandle {
    pub(crate) stream_key: String,
    stop_tx: watch::Sender<bool>,
}

impl StreamPublishHandle {
    pub(crate) fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

impl Drop for StreamPublishHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Abre la conexión de media de la transmisión en un hilo aparte. Los cambios
/// de estado llegan como `AppEvent::StreamPublishStatus`.
pub(crate) fn spawn_stream_publish(
    params: StreamPublishParams,
    events: std::sync::mpsc::Sender<AppEvent>,
) -> StreamPublishHandle {
    let (stop_tx, stop_rx) = watch::channel(false);
    let stream_key = params.stream_key.clone();
    let thread_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("stream-publish".to_owned())
        .spawn(move || {
            let _perf = win_perf::Guard::new();
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    publish_status(
                        &thread_events,
                        &params.stream_key,
                        StreamPublishStatus::Failed,
                        Some(format!("could not start the stream runtime: {error}")),
                    );
                    return;
                }
            };
            runtime.block_on(run_stream_publish(params, thread_events, stop_rx));
        });
    if let Err(error) = spawned {
        publish_status(
            &events,
            &stream_key,
            StreamPublishStatus::Failed,
            Some(format!("could not start the stream thread: {error}")),
        );
    }
    StreamPublishHandle { stream_key, stop_tx }
}

fn publish_status(
    events: &std::sync::mpsc::Sender<AppEvent>,
    stream_key: &str,
    status: StreamPublishStatus,
    message: Option<String>,
) {
    let _ = events.send(AppEvent::StreamPublishStatus {
        stream_key: stream_key.to_owned(),
        status,
        message,
    });
}

/// Tiempo máximo que se sigue reintentando después de perder la conexión (corte de
/// internet, cambio de red, el servidor de media que cierra) antes de dar la
/// transmisión por fallida.
const RECONNECT_GIVE_UP: Duration = Duration::from_secs(5 * 60);
/// Espera entre reintentos: 1 s, 2 s, 4 s… hasta este máximo.
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(8);
/// Una sesión que llegó a durar esto estaba funcionando: el plazo de reintentos
/// empieza de cero.
const RECONNECT_HEALTHY_AFTER: Duration = Duration::from_secs(15);

async fn run_stream_publish(
    params: StreamPublishParams,
    events: std::sync::mpsc::Sender<AppEvent>,
    mut stop_rx: watch::Receiver<bool>,
) {
    logging::debug("stream", format!("stream publish starting: {params:?}"));
    publish_status(&events, &params.stream_key, StreamPublishStatus::Connecting, None);
    // Desde cuándo se está fallando sin parar (None = la última sesión funcionó).
    let mut failing_since: Option<Instant> = None;
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match publish_stream_session(&params, &events, stop_rx.clone()).await {
            Ok(()) => {
                logging::debug("stream", "stream publish ended");
                publish_status(&events, &params.stream_key, StreamPublishStatus::Ended, None);
                return;
            }
            Err(error) => {
                if *stop_rx.borrow() {
                    publish_status(&events, &params.stream_key, StreamPublishStatus::Ended, None);
                    return;
                }
                if started.elapsed() >= RECONNECT_HEALTHY_AFTER {
                    failing_since = None;
                    backoff = Duration::from_secs(1);
                }
                let since = *failing_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= RECONNECT_GIVE_UP {
                    logging::error("stream", format!("{error} (se dejó de reintentar tras {RECONNECT_GIVE_UP:?})"));
                    publish_status(&events, &params.stream_key, StreamPublishStatus::Failed, Some(error));
                    return;
                }
                logging::error("stream", format!("{error}: reintentando en {backoff:?}"));
                publish_status(
                    &events,
                    &params.stream_key,
                    StreamPublishStatus::Connecting,
                    Some(format!("Reconectando… ({error})")),
                );
                // El sleep se corta si se pide parar la transmisión.
                tokio::select! {
                    () = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => {
                        publish_status(&events, &params.stream_key, StreamPublishStatus::Ended, None);
                        return;
                    }
                }
                backoff = (backoff * 2).min(RECONNECT_MAX_BACKOFF);
            }
        }
    }
}

/// Aborta las tareas hijas cuando la sesión termina (por error o no). Una de
/// ellas es dueña del `ScreenEncoder`: al abortarla se detiene la captura.
#[derive(Default)]
struct PublishTasks {
    handles: Vec<JoinHandle<()>>,
}

impl PublishTasks {
    fn push(&mut self, handle: JoinHandle<()>) {
        self.handles.push(handle);
    }
}

impl Drop for PublishTasks {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

async fn publish_stream_session(
    params: &StreamPublishParams,
    events: &std::sync::mpsc::Sender<AppEvent>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), String> {
    let mut tasks = PublishTasks::default();
    // Errores de las tareas hijas (codificador, envío) que tienen que terminar
    // la sesión.
    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel::<String>();

    // La captura arranca YA, en paralelo con el WebSocket: así, cuando termina
    // la negociación el primer frame suele estar listo. Hasta entonces los
    // frames se quedan en la cola (`PUBLISH_QUEUE`) y el codificador espera a la
    // próxima IDR si se llena.
    let (frames_tx, frames_rx) = mpsc::channel::<EncodedFrame>(PUBLISH_QUEUE);
    // Pedido de IDR compartido: lo activan los PLI/FIR de los espectadores
    // (`run_feedback`) y lo atiende el hilo del codificador.
    let keyframe_request = KeyframeRequest::default();
    // Bitrate de video objetivo (kbps). `run_feedback` lo baja cuando el REMB del
    // servidor dice que se está mandando más de lo que admite o cuando la pérdida
    // reportada es alta y sostenida (y lo vuelve a subir si se normaliza); la tarea
    // de captura reinicia el codificador con el valor nuevo.
    let (bitrate_tx, mut bitrate_rx) = watch::channel(params.capture.bitrate_kbps);
    {
        let mut capture = params.capture.clone();
        let fatal_tx = fatal_tx.clone();
        let keyframe_request = keyframe_request.clone();
        let preview = params.preview.clone();
        tasks.push(tokio::spawn(async move {
            // Referencia débil: no mantiene abierta la cola, así que si el hilo del
            // codificador muere sola se sigue detectando (`recv()` devuelve `None`).
            let weak_tx = frames_tx.downgrade();
            let mut next_tx = Some(frames_tx);
            loop {
                let Some(tx) = next_tx.take() else { return };
                // Antes de arrancar: un cambio de las devtools durante el arranque no se pierde.
                let settings_seen = encoder_settings_generation();
                let config = capture.clone();
                let (keyframe, preview) = (keyframe_request.clone(), preview.clone());
                let started = tokio::task::spawn_blocking(move || {
                    start_screen_encoder(&config, tx, keyframe, preview)
                })
                .await;
                let encoder = match started {
                    Ok(Ok(encoder)) => encoder,
                    Ok(Err(error)) => {
                        let _ = fatal_tx.send(format!("no se pudo iniciar la captura: {error}"));
                        return;
                    }
                    Err(error) => {
                        let _ = fatal_tx.send(format!("la tarea de captura falló: {error}"));
                        return;
                    }
                };
                logging::debug(
                    "stream",
                    format!(
                        "holding screen encoder {} ({} kbps)",
                        encoder.encoder_name(),
                        capture.bitrate_kbps
                    ),
                );
                // Espera un cambio de bitrate (REMB, pérdida) o de ajustes del codificador
                // (devtools); mientras tanto el codificador sigue vivo. En ambos casos se
                // reinicia con la configuración nueva.
                let mut settings_poll = tokio::time::interval(Duration::from_millis(250));
                settings_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut bitrate_closed = false;
                loop {
                    tokio::select! {
                        changed = bitrate_rx.changed(), if !bitrate_closed => {
                            if changed.is_err() {
                                // Nadie va a pedir cambios de bitrate; los ajustes siguen vigilándose.
                                bitrate_closed = true;
                                continue;
                            }
                            let kbps = *bitrate_rx.borrow_and_update();
                            // Baja (REMB, pérdida) o sube (la pérdida volvió a ser baja).
                            if kbps != capture.bitrate_kbps {
                                capture.bitrate_kbps = kbps;
                                break;
                            }
                        }
                        _ = settings_poll.tick() => {
                            if encoder_settings_generation() != settings_seen {
                                // Varios cambios seguidos (dos listas de las devtools) reinician una sola vez.
                                loop {
                                    let before = encoder_settings_generation();
                                    tokio::time::sleep(Duration::from_millis(400)).await;
                                    if encoder_settings_generation() == before {
                                        break;
                                    }
                                }
                                logging::debug("stream", "ajustes del codificador cambiados en las devtools: se reinicia el codificador");
                                break;
                            }
                        }
                    }
                }
                // Se toma un sender nuevo ANTES de soltar el codificador, para que la
                // cola no se cierre durante el reinicio.
                let Some(tx) = weak_tx.upgrade() else { return };
                next_tx = Some(tx);
                // Soltarlo espera al hilo del codificador (como mucho un intervalo
                // de captura): fuera del runtime asíncrono.
                let _ = tokio::task::spawn_blocking(move || drop(encoder)).await;
            }
        }));
    }
    let mut frames_rx = Some(frames_rx);

    let url = voice_gateway_url(&params.endpoint)?;
    logging::debug("stream", format!("connecting stream websocket: {url}"));
    let ws = timeout(
        VOICE_WEBSOCKET_CONNECT_TIMEOUT,
        websocket::connect(&url, websocket::NetKind::StreamPublish, "voice::stream_publish (emitir un stream)"),
    )
        .await
        .map_err(|_| "stream websocket connect timed out after 10s".to_owned())?
        .map_err(|error| format!("stream websocket connect failed: {error}"))?;
    logging::debug("stream", "stream websocket connected");

    let (writer, mut reader) = ws.split();
    let writer: VoiceWriter = Arc::new(Mutex::new(writer));
    send_voice_text(&writer, publish_identify_payload(params)).await?;

    let dave_state = Arc::new(Mutex::new(VoiceDaveState::new_for_stream(
        params.user_id,
        &params.rtc_server_id,
    )?));
    let last_sequence: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let heartbeat_ack = Arc::new(Mutex::new(VoiceHeartbeatAckState::default()));
    let (heartbeat_timeout_tx, mut heartbeat_timeout_rx) = mpsc::unbounded_channel::<VoiceHeartbeatTimeout>();

    let mut udp_socket: Option<Arc<UdpSocket>> = None;
    // (audio, video, rtx) — se completan en READY.
    let mut ssrcs: Option<(u32, u32, u32)> = None;
    let mut sending = false;
    // Solo con `ECORD_STREAM_TRANSPORT=webrtc`: lo que queda listo en READY y se usa
    // cuando llega la respuesta de Discord.
    let mut webrtc_prepared: Option<webrtc::PreparedTransport> = None;
    // Se decide una vez por sesión: cambiar la opción de las devtools a mitad de la
    // negociación no mezcla los dos transportes.
    let use_webrtc = stream_transport() == StreamTransport::WebRtc;

    loop {
        let frame = tokio::select! {
            _ = stop_rx.changed() => return Ok(()),
            error = fatal_rx.recv() => {
                return Err(error.unwrap_or_else(|| "stream task channel closed".to_owned()));
            }
            timed_out = heartbeat_timeout_rx.recv() => {
                if timed_out.is_some() {
                    return Err("stream heartbeat was not acknowledged".to_owned());
                }
                continue;
            }
            frame = reader.next() => frame,
        };
        let Some(frame) = frame else {
            return Err("stream websocket closed by the server".to_owned());
        };
        let frame = frame.map_err(|error| format!("stream websocket read failed: {error}"))?;

        match frame {
            WsMessage::Text(text) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("stream websocket JSON parse failed: {error}"))?;
                if let Some(sequence) = value.get("seq").and_then(Value::as_i64) {
                    *last_sequence.lock().await = Some(sequence);
                }
                let Some(opcode) = voice_gateway_opcode(&value) else {
                    continue;
                };
                match opcode {
                    VOICE_OP_HELLO => {
                        let interval = value
                            .get("d")
                            .and_then(|data| data.get("heartbeat_interval"))
                            .and_then(Value::as_u64)
                            .map(Duration::from_millis)
                            .ok_or_else(|| "stream hello missing heartbeat interval".to_owned())?;
                        heartbeat_ack.lock().await.reset();
                        tasks.push(tokio::spawn(run_voice_heartbeat(
                            Arc::clone(&writer),
                            interval,
                            Arc::clone(&last_sequence),
                            Arc::clone(&heartbeat_ack),
                            heartbeat_timeout_tx.clone(),
                            0,
                        )));
                    }
                    VOICE_OP_HEARTBEAT_ACK => {
                        heartbeat_ack.lock().await.mark_acknowledged();
                    }
                    VOICE_OP_READY => {
                        let ready = parse_voice_ready_payload(&value)?;
                        let (video_ssrc, rtx_ssrc) = ready_video_ssrcs(&value, ready.ssrc);
                        logging::debug(
                            "stream",
                            format!(
                                "stream publish ready: audio_ssrc={} video_ssrc={video_ssrc} \
                                 rtx_ssrc={rtx_ssrc} udp={}:{} modes={}",
                                ready.ssrc,
                                ready.ip,
                                ready.port,
                                ready.modes.len()
                            ),
                        );
                        ssrcs = Some((ready.ssrc, video_ssrc, rtx_ssrc));
                        if use_webrtc {
                            // WebRTC: sin descubrimiento de IP ni modo de cifrado. Se manda
                            // un SDP local y Discord contesta con el suyo.
                            let prepared = webrtc::PreparedTransport::new(ready.ssrc, video_ssrc, rtx_ssrc, keyframe_request.clone())?;
                            let offer = webrtc::build_offer_sdp(prepared.creds(), ready.ssrc, video_ssrc, rtx_ssrc);
                            logging::debug("stream", format!("webrtc: oferta SDP local:\n{offer}"));
                            send_voice_text(
                                &writer,
                                webrtc::select_protocol_payload(&offer, &uuid::Uuid::new_v4().to_string()),
                            )
                            .await?;
                            webrtc_prepared = Some(prepared);
                            continue;
                        }
                        let mode = choose_encryption_mode(&ready.modes)?;
                        let (socket, discovered) = discover_voice_udp_address(&ready).await?;
                        send_voice_text(&writer, publish_select_protocol_payload(&discovered, &mode)).await?;
                        tasks.push(tokio::spawn(run_voice_udp_ping(Arc::clone(&socket))));
                        udp_socket = Some(socket);
                    }
                    VOICE_OP_SESSION_DESCRIPTION if use_webrtc => {
                        let Some(prepared) = webrtc_prepared.take() else {
                            // Un session description repetido no reinicia el envío.
                            continue;
                        };
                        let description = webrtc::parse_session_description(&value)?;
                        logging::debug("stream", format!("webrtc: sdp de Discord:\n{}", description.sdp));
                        dave_state
                            .lock()
                            .await
                            .apply_protocol_version(description.dave_protocol_version)?;
                        let Some((audio_ssrc, video_ssrc, rtx_ssrc)) = ssrcs else {
                            return Err("stream session description arrived before READY".to_owned());
                        };
                        let remote = webrtc::parse_remote_sdp(&description.sdp)?;
                        let transport = prepared.start(remote, &mut tasks).await?;
                        sending = true;

                        send_voice_text(&writer, publish_speaking_payload(audio_ssrc)).await?;
                        send_voice_text(
                            &writer,
                            publish_video_payload(audio_ssrc, video_ssrc, rtx_ssrc, &params.capture),
                        )
                        .await?;

                        let Some(frames_rx) = frames_rx.take() else {
                            return Err("stream frame queue already taken".to_owned());
                        };
                        // Audio de la ventana/pantalla (opcional: si falla, sigue solo el video).
                        tasks.push(tokio::spawn(webrtc::run_audio_send(webrtc::WebRtcAudioSender {
                            transport: transport.clone(),
                            dave_state: Arc::clone(&dave_state),
                            target: params.capture.target.clone(),
                        })));
                        tasks.push(tokio::spawn(webrtc::run_video_send(webrtc::WebRtcVideoSender {
                            transport,
                            dave_state: Arc::clone(&dave_state),
                            frames_rx,
                            fatal_tx: fatal_tx.clone(),
                            events: events.clone(),
                            stream_key: params.stream_key.clone(),
                            pacing_floor_bytes_per_sec: (u64::from(params.capture.bitrate_kbps)
                                * 125
                                * webrtc::PACER_FLOOR_PERCENT
                                / 100)
                                .max(PACING_MIN_BYTES_PER_SEC),
                            fps: params.capture.fps.max(1),
                            keyframe: keyframe_request.clone(),
                        })));
                        publish_status(events, &params.stream_key, StreamPublishStatus::Connected, None);
                    }
                    VOICE_OP_SESSION_DESCRIPTION => {
                        let description = parse_voice_session_description(&value)?;
                        logging::debug("stream", format!("stream publish session description: {description:?}"));
                        dave_state
                            .lock()
                            .await
                            .apply_protocol_version(description.dave_protocol_version)?;
                        let (Some(socket), Some((audio_ssrc, video_ssrc, rtx_ssrc))) = (udp_socket.as_ref(), ssrcs)
                        else {
                            return Err("stream session description arrived before the UDP transport".to_owned());
                        };
                        // Un session description repetido no reinicia el envío.
                        if sending {
                            continue;
                        }
                        sending = true;
                        let encryptor = Arc::new(VoiceRtpEncryptor::new(&description.mode, &description.secret_key)?);
                        // El buffer de envío por omisión de Windows es de 64 KB: una IDR de
                        // ~100 KB bloqueaba el `send` hasta que la red lo vaciaba.
                        let _ = socket2::SockRef::from(&**socket).set_send_buffer_size(2 * 1024 * 1024);
                        let nonce = Arc::new(std::sync::atomic::AtomicU32::new(0));
                        let history = Arc::new(std::sync::Mutex::new(PacketHistory::default()));

                        // "Voy a mandar video con estos SSRC y esta resolución."
                        send_voice_text(&writer, publish_speaking_payload(audio_ssrc)).await?;
                        send_voice_text(
                            &writer,
                            publish_video_payload(audio_ssrc, video_ssrc, rtx_ssrc, &params.capture),
                        )
                        .await?;

                        let Some(frames_rx) = frames_rx.take() else {
                            return Err("stream frame queue already taken".to_owned());
                        };
                        // Lo que llega por el UDP: respuestas al ping y RTCP de los
                        // espectadores (un PLI/FIR fuerza una IDR).
                        let decryptor = VoiceRtpDecryptor::new(&description.mode, &description.secret_key)?;
                        tasks.push(tokio::spawn(run_feedback(
                            Arc::clone(socket),
                            decryptor,
                            keyframe_request.clone(),
                            bitrate_tx.clone(),
                            params.capture.bitrate_kbps,
                            video_ssrc,
                            RtxSender {
                                socket: Arc::clone(socket),
                                encryptor: Arc::clone(&encryptor),
                                ssrc: rtx_ssrc,
                                sequence: rand::random(),
                                history: Arc::clone(&history),
                                nonce: Arc::clone(&nonce),
                            },
                        )));
                        tasks.push(tokio::spawn(run_video_send(VideoSender {
                            socket: Arc::clone(socket),
                            encryptor,
                            nonce,
                            history,
                            dave_state: Arc::clone(&dave_state),
                            ssrc: video_ssrc,
                            frames_rx,
                            fatal_tx: fatal_tx.clone(),
                            events: events.clone(),
                            stream_key: params.stream_key.clone(),
                            pacing_floor_bytes_per_sec: (u64::from(params.capture.bitrate_kbps)
                                * 125
                                * PACING_FLOOR_PERCENT
                                / 100)
                                .max(PACING_MIN_BYTES_PER_SEC),
                            fps: params.capture.fps.max(1),
                            keyframe: keyframe_request.clone(),
                        })));
                        publish_status(events, &params.stream_key, StreamPublishStatus::Connected, None);
                    }
                    VOICE_OP_VIDEO => {
                        logging::debug("stream", format!("stream video op: {value}"));
                    }
                    other => {
                        if handles_gateway_json_op(other) {
                            dave_state.lock().await.handle_json_op(&writer, other, &value).await?;
                        } else {
                            logging::debug("stream", format!("unhandled stream gateway op={other}"));
                        }
                    }
                }
            }
            WsMessage::Binary(payload) => {
                let frame = parse_voice_binary_frame(&payload)?;
                *last_sequence.lock().await = Some(frame.sequence);
                dave_state.lock().await.handle_binary_frame(&writer, frame).await?;
            }
            WsMessage::Ping(payload) => {
                let mut writer = writer.lock().await;
                writer
                    .send(WsMessage::Pong(payload))
                    .await
                    .map_err(|error| format!("stream websocket pong failed: {error}"))?;
            }
            WsMessage::Close(frame) => {
                let detail = frame
                    .map(|frame| {
                        let reason = frame.reason.to_string();
                        format!("code={} reason={reason}", u16::from(frame.code))
                    })
                    .unwrap_or_else(|| "no close frame".to_owned());
                return Err(format!("stream websocket closed: {detail}"));
            }
            _ => {}
        }
    }
}

/// SSRC de video y de RTX que asignó el servidor (`streams[0]` del READY). Si no
/// vienen, la convención de Discord es audio+1 y audio+2.
fn ready_video_ssrcs(value: &Value, audio_ssrc: u32) -> (u32, u32) {
    let stream = value
        .get("d")
        .and_then(|data| data.get("streams"))
        .and_then(Value::as_array)
        .and_then(|streams| streams.first());
    let field = |name: &str| {
        stream
            .and_then(|stream| stream.get(name))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    };
    (
        field("ssrc").unwrap_or(audio_ssrc.wrapping_add(1)),
        field("rtx_ssrc").unwrap_or(audio_ssrc.wrapping_add(2)),
    )
}

/// IDENTIFY (opcode 0) del WebSocket de media del stream propio. `server_id` es
/// el `rtc_server_id`; `streams` declara que vamos a mandar una pantalla.
fn publish_identify_payload(params: &StreamPublishParams) -> String {
    json!({
        "op": 0,
        "d": {
            "server_id": params.rtc_server_id,
            "user_id": params.user_id.to_string(),
            "session_id": params.session_id,
            "token": params.token,
            "video": true,
            "streams": [
                { "type": "screen", "rid": "100", "quality": 100 },
            ],
            "max_dave_protocol_version": davey::DAVE_PROTOCOL_VERSION,
        },
    })
    .to_string()
}

/// SELECT_PROTOCOL (opcode 1) de quien transmite: igual al del espectador pero
/// con H.264 marcado como `encode: true`.
fn publish_select_protocol_payload(discovered: &DiscoveredVoiceAddress, mode: &str) -> String {
    json!({
        "op": 1,
        "d": {
            "protocol": "udp",
            "data": {
                "address": discovered.address,
                "port": discovered.port,
                "mode": mode,
            },
            "address": discovered.address,
            "port": discovered.port,
            "mode": mode,
            "codecs": [
                {
                    "name": "opus",
                    "type": "audio",
                    "priority": 1000,
                    "payload_type": PUBLISH_OPUS_PAYLOAD_TYPE,
                },
                {
                    "name": "H264",
                    "type": "video",
                    "priority": 1000,
                    "payload_type": PUBLISH_H264_PAYLOAD_TYPE,
                    "rtx_payload_type": PUBLISH_H264_RTX_PAYLOAD_TYPE,
                    "encode": true,
                    "decode": false,
                },
            ],
        },
    })
    .to_string()
}

/// SPEAKING (opcode 5) con el flag de "sonido compartido" (2): es lo que manda
/// la conexión de un stream antes del video.
fn publish_speaking_payload(audio_ssrc: u32) -> String {
    json!({
        "op": VOICE_OP_SPEAKING,
        "d": { "speaking": 2, "delay": 0, "ssrc": audio_ssrc },
    })
    .to_string()
}

/// VIDEO (opcode 12): SSRC de video y de RTX, y la resolución/bitrate/fps que
/// vamos a mandar.
fn publish_video_payload(audio_ssrc: u32, video_ssrc: u32, rtx_ssrc: u32, capture: &ScreenCaptureConfig) -> String {
    json!({
        "op": VOICE_OP_VIDEO,
        "d": {
            "audio_ssrc": audio_ssrc,
            "video_ssrc": video_ssrc,
            "rtx_ssrc": rtx_ssrc,
            "streams": [
                {
                    "type": "screen",
                    "rid": "100",
                    "ssrc": video_ssrc,
                    "rtx_ssrc": rtx_ssrc,
                    "active": true,
                    "quality": 100,
                    "max_bitrate": u64::from(capture.bitrate_kbps) * 1000,
                    "max_framerate": capture.fps,
                    "max_resolution": {
                        "type": "fixed",
                        "width": capture.width & !1,
                        "height": capture.height & !1,
                    },
                },
            ],
        },
    })
    .to_string()
}

/// Tipo de paquete RTCP de feedback específico del payload (RFC 4585, PSFB).
const RTCP_PAYLOAD_SPECIFIC_FEEDBACK: u8 = 206;
/// FMT de PSFB: Picture Loss Indication (RFC 4585 §6.3.1) y Full Intra Request
/// (RFC 5104 §4.3.1).
const RTCP_FMT_PLI: u8 = 1;
/// RTCP de feedback de transporte (RTPFB) y su formato NACK genérico.
const RTCP_TRANSPORT_FEEDBACK: u8 = 205;
const RTCP_FMT_NACK: u8 = 1;
const RTCP_FMT_FIR: u8 = 4;

/// ¿El paquete RTCP (compuesto, ya descifrado) pide una imagen completa?
/// Recorre los paquetes de la cabecera común (V/P/FMT, PT, longitud en
/// palabras de 32 bits menos uno) y busca un PLI o un FIR. Hay un solo stream
/// de video, así que no se mira a qué SSRC apunta.
/// Un bloque de reporte de un RR/SR (RFC 3550): lo que el servidor dice haber
/// recibido de nuestro SSRC.
struct RtcpReportBlock {
    ssrc: u32,
    /// Fracción perdida desde el reporte anterior, en 1/256.
    fraction_lost: u8,
    cumulative_lost: u32,
    jitter: u32,
}

/// Recorre un RTCP compuesto y devuelve `(tipo, formato, paquete completo)` de
/// cada paquete. `formato` es el FMT de los feedback (205/206) y 0 en el resto.
fn rtcp_packets(packet: &[u8]) -> Vec<(u8, u8, &[u8])> {
    let mut found = Vec::new();
    let mut rest = packet;
    while rest.len() >= 4 {
        if rest[0] >> 6 != 2 {
            break;
        }
        let length = (usize::from(u16::from_be_bytes([rest[2], rest[3]])) + 1) * 4;
        if length > rest.len() {
            break;
        }
        let kind = rest[1];
        let format = if kind == RTCP_TRANSPORT_FEEDBACK || kind == RTCP_PAYLOAD_SPECIFIC_FEEDBACK {
            rest[0] & 0x1F
        } else {
            0
        };
        found.push((kind, format, &rest[..length]));
        rest = &rest[length..];
    }
    found
}

/// Bloques de reporte de un SR (200) o RR (201); vacío para cualquier otro tipo.
fn rtcp_report_blocks(kind: u8, packet: &[u8]) -> Vec<RtcpReportBlock> {
    let offset = match kind {
        200 => 28,
        201 => 8,
        _ => return Vec::new(),
    };
    let count = usize::from(packet.first().copied().unwrap_or(0) & 0x1F);
    let mut blocks = Vec::new();
    for index in 0..count {
        let start = offset + index * 24;
        let Some(block) = packet.get(start..start + 24) else {
            break;
        };
        blocks.push(RtcpReportBlock {
            ssrc: u32::from_be_bytes([block[0], block[1], block[2], block[3]]),
            fraction_lost: block[4],
            cumulative_lost: u32::from_be_bytes([0, block[5], block[6], block[7]]),
            jitter: u32::from_be_bytes([block[12], block[13], block[14], block[15]]),
        });
    }
    blocks
}

/// Números de secuencia que piden reenviar los NACK genéricos (RFC 4585 §6.2.1)
/// de un RTCP compuesto: cada entrada trae un PID y una máscara de 16 pérdidas
/// siguientes (BLP).
fn rtcp_nacked_sequences(packet: &[u8]) -> Vec<u16> {
    let mut wanted = Vec::new();
    let mut rest = packet;
    while rest.len() >= 4 {
        if rest[0] >> 6 != 2 {
            break;
        }
        let format = rest[0] & 0x1F;
        let kind = rest[1];
        let length = (usize::from(u16::from_be_bytes([rest[2], rest[3]])) + 1) * 4;
        if length > rest.len() {
            break;
        }
        if kind == RTCP_TRANSPORT_FEEDBACK && format == RTCP_FMT_NACK && length > 12 {
            for entry in rest[12..length].chunks_exact(4) {
                let pid = u16::from_be_bytes([entry[0], entry[1]]);
                let blp = u16::from_be_bytes([entry[2], entry[3]]);
                wanted.push(pid);
                for bit in 0..16u16 {
                    if blp & (1 << bit) != 0 {
                        wanted.push(pid.wrapping_add(bit + 1));
                    }
                }
            }
        }
        rest = &rest[length..];
    }
    wanted
}

fn rtcp_requests_keyframe(packet: &[u8]) -> bool {
    let mut rest = packet;
    while rest.len() >= 4 {
        if rest[0] >> 6 != 2 {
            return false;
        }
        let format = rest[0] & 0x1F;
        let kind = rest[1];
        let length = (usize::from(u16::from_be_bytes([rest[2], rest[3]])) + 1) * 4;
        if length > rest.len() {
            return false;
        }
        if kind == RTCP_PAYLOAD_SPECIFIC_FEEDBACK && (format == RTCP_FMT_PLI || format == RTCP_FMT_FIR) {
            return true;
        }
        rest = &rest[length..];
    }
    false
}

/// Lee lo que llega por el UDP (respuestas al ping, RTCP). Si un espectador
/// manda un PLI/FIR (entró tarde o perdió paquetes), pide una IDR al
/// codificador; el resto (NACK, receiver reports) se descarta.
async fn run_feedback(
    socket: Arc<UdpSocket>,
    decryptor: VoiceRtpDecryptor,
    keyframe: KeyframeRequest,
    bitrate_tx: watch::Sender<u32>,
    initial_bitrate_kbps: u32,
    video_ssrc: u32,
    mut rtx: RtxSender,
) {
    let mut packet = vec![0u8; 2048];
    let mut requests = 0u64;
    let (mut nack_packets, mut resent_total, mut missing_total) = (0u64, 0u64, 0u64);
    // Paquetes que ya reenviamos y cuándo. Si el visor los vuelve a pedir, el
    // reenvío (RTX) no le sirvió: la imagen está rota hasta la próxima IDR, y el
    // visor oficial casi nunca manda PLI (2 en 160 s con 54 congelamientos), así
    // que se pide la IDR desde acá en vez de esperar al GOP.
    let mut resent_at: std::collections::HashMap<u16, Instant> = std::collections::HashMap::new();
    let mut nack_keyframes = 0u64;
    // Contadores del feedback del visor, para ver si sus RTCP llegan y se
    // descifran (el visor puede reportar cientos de NACK y no llegar ninguno).
    let mut last_report = Instant::now();
    let (mut rtcp_ok, mut rtcp_fail, mut nack_rtcp) = (0u64, 0u64, 0u64);
    // Qué tipos de RTCP manda el servidor, y el último reporte de recepción
    // (cuánto dice haber perdido de lo que le enviamos).
    let mut rtcp_kinds: std::collections::BTreeMap<(u8, u8), u64> = std::collections::BTreeMap::new();
    let mut last_block: Option<RtcpReportBlock> = None;
    // REMB del servidor (su estimación de ancho de banda de subida): último,
    // mínimo y máximo de los últimos 5 s.
    let mut remb_last: Option<u64> = None;
    let mut remb_range: Option<(u64, u64)> = None;
    // Bitrate de video vigente y cuándo se cambió por última vez.
    let mut current_kbps = u64::from(initial_bitrate_kbps);
    let mut last_bitrate_change: Option<Instant> = None;
    // Baja el bitrate si el servidor reporta pérdida alta y sostenida de nuestro
    // video (el REMB suele venir fijo y no lo avisa).
    let mut loss_control = LossController::default();
    let mut logged_foreign_report = false;
    loop {
        match socket.recv(&mut packet).await {
            Ok(len) => {
                if last_report.elapsed() >= Duration::from_secs(5) {
                    last_report = Instant::now();
                    let server_report = last_block.as_ref().map_or_else(
                        || "sin reporte".to_owned(),
                        |block| {
                            format!(
                                "ssrc={} perdido={:.1}% acumulado={} jitter={}",
                                block.ssrc,
                                f64::from(block.fraction_lost) * 100.0 / 256.0,
                                block.cumulative_lost,
                                block.jitter
                            )
                        },
                    );
                    let loss_smoothed = loss_control
                        .smoothed_percent()
                        .map_or_else(|| "n/d".to_owned(), |percent| format!("{percent:.1}%"));
                    let remb = match (remb_last, remb_range) {
                        (Some(last), Some((min, max))) => {
                            format!("ultimo={} kbps min={} kbps max={} kbps", last / 1000, min / 1000, max / 1000)
                        }
                        _ => "sin REMB".to_owned(),
                    };
                    logging::debug(
                        "stream",
                        format!(
                            "feedback 5s: rtcp_ok={rtcp_ok} rtcp_sin_descifrar={rtcp_fail} nack_rtcp={nack_rtcp} \
                             pedidos={missing_total}+{resent_total} (ya_no_estaban+reenviados) \
                             rtcp_tipos(tipo,fmt)={rtcp_kinds:?} reporte_servidor=[{server_report}] remb=[{remb}] \
                             bitrate={current_kbps} kbps perdida_suavizada={loss_smoothed}"
                        ),
                    );
                    rtcp_kinds.clear();
                    remb_range = None;
                }
                let plain = match decryptor.decrypt_rtcp_feedback(&packet[..len]) {
                    Ok(plain) => {
                        rtcp_ok += 1;
                        plain
                    }
                    Err(_) => {
                        // Respuesta al ping o RTP ajeno no interesan; un RTCP que
                        // no se descifra sí se cuenta.
                        if looks_like_rtcp_packet(&packet[..len]) {
                            rtcp_fail += 1;
                        }
                        continue;
                    }
                };
                for (kind, format, bytes) in rtcp_packets(&plain) {
                    *rtcp_kinds.entry((kind, format)).or_insert(0) += 1;
                    if let Some(bps) = rtcp_remb_bitrate(kind, format, bytes) {
                        remb_last = Some(bps);
                        remb_range = Some(remb_range.map_or((bps, bps), |(min, max)| (min.min(bps), max.max(bps))));
                        // El servidor dice cuánto admite: no se manda más que eso.
                        let cap_kbps = (bps / 1000 * REMB_USE_PERCENT / 100).max(REMB_MIN_KBPS);
                        if cap_kbps * 100 < current_kbps * REMB_RESTART_BELOW_PERCENT
                            && last_bitrate_change.is_none_or(|at| at.elapsed() >= REMB_RESTART_MIN_INTERVAL)
                        {
                            logging::debug(
                                "stream",
                                format!(
                                    "REMB {} kbps < bitrate {current_kbps} kbps: se reinicia el codificador a {cap_kbps} kbps",
                                    bps / 1000
                                ),
                            );
                            current_kbps = cap_kbps;
                            last_bitrate_change = Some(Instant::now());
                            loss_control.note_external_change(Instant::now());
                            let _ = bitrate_tx.send(u32::try_from(cap_kbps).unwrap_or(u32::MAX));
                        }
                    }
                    let blocks = rtcp_report_blocks(kind, bytes);
                    if let Some(video) = blocks.iter().find(|block| block.ssrc == video_ssrc) {
                        // Techo: el bitrate configurado, o menos si el REMB lo limita.
                        let ceiling_kbps = remb_last.map_or(u64::from(initial_bitrate_kbps), |bps| {
                            (bps / 1000 * REMB_USE_PERCENT / 100)
                                .max(REMB_MIN_KBPS)
                                .min(u64::from(initial_bitrate_kbps))
                        });
                        if let Some(decision) = loss_control.on_report(
                            Instant::now(),
                            video.fraction_lost,
                            current_kbps,
                            u64::from(initial_bitrate_kbps),
                            ceiling_kbps,
                        ) {
                            logging::debug("stream", format!("control de pérdida: {}", decision.reason));
                            current_kbps = decision.kbps;
                            last_bitrate_change = Some(Instant::now());
                            let _ = bitrate_tx.send(u32::try_from(decision.kbps).unwrap_or(u32::MAX));
                        }
                    } else if !blocks.is_empty() && !logged_foreign_report {
                        // Si el servidor reporta con otro SSRC, el control de pérdida
                        // no actúa: se deja dicho una vez.
                        logged_foreign_report = true;
                        let ssrcs: Vec<u32> = blocks.iter().map(|block| block.ssrc).collect();
                        logging::debug(
                            "stream",
                            format!("control de pérdida inactivo: reporte del servidor para ssrc {ssrcs:?}, el video es {video_ssrc}"),
                        );
                    }
                    if let Some(block) = blocks.into_iter().last() {
                        last_block = Some(block);
                    }
                }
                let nacked = rtcp_nacked_sequences(&plain);
                if !nacked.is_empty() {
                    nack_rtcp += 1;
                    let now = Instant::now();
                    // Sin reenviar lo que salió hace menos de ~1 RTT.
                    let fresh: Vec<u16> = nacked
                        .iter()
                        .copied()
                        .filter(|sequence| {
                            resent_at
                                .get(sequence)
                                .is_none_or(|at| now.duration_since(*at) >= RTX_MIN_RESEND_INTERVAL)
                        })
                        .collect();
                    // "Sin efecto" solo si pasó tiempo de sobra para que el
                    // reenvío llegara.
                    let repeated = nacked.iter().any(|sequence| {
                        resent_at.get(sequence).is_some_and(|at| {
                            let age = now.duration_since(*at);
                            age >= NACK_REPEAT_MIN_AGE && age < NACK_REPEAT_WINDOW
                        })
                    });
                    let (resent, missing) = rtx.resend(&fresh).await;
                    for sequence in fresh.iter().take(RTX_MAX_PER_RTCP) {
                        resent_at.insert(*sequence, now);
                    }
                    if resent_at.len() > 1024 {
                        resent_at.retain(|_, at| now.duration_since(*at) < NACK_REPEAT_WINDOW);
                    }
                    if repeated || missing > 0 {
                        keyframe.request();
                        nack_keyframes += 1;
                        if nack_keyframes <= 30 || nack_keyframes % 20 == 0 {
                            logging::debug(
                                "stream",
                                format!(
                                    "NACK sin efecto (repetido={repeated}, fuera_de_historial={missing}): \
                                     IDR pedida, count={nack_keyframes}"
                                ),
                            );
                        }
                    }
                    nack_packets += 1;
                    resent_total += resent;
                    missing_total += missing;
                    if nack_packets == 1 || nack_packets % 25 == 0 {
                        logging::debug(
                            "stream",
                            format!(
                                "viewer NACK: rtcp={nack_packets} pedidos={} reenviados={resent_total} \
                                 ya_no_estaban={missing_total}",
                                nacked.len()
                            ),
                        );
                    }
                }
                if rtcp_requests_keyframe(&plain) {
                    keyframe.request();
                    requests += 1;
                    if requests <= 30 || requests % 20 == 0 {
                        logging::debug(
                            "stream",
                            format!("viewer asked for a keyframe (PLI/FIR): count={requests}"),
                        );
                    }
                }
            }
            Err(error) => {
                logging::debug("stream", format!("stream UDP receive ended: {error}"));
                return;
            }
        }
    }
}

struct VideoSender {
    socket: Arc<UdpSocket>,
    encryptor: Arc<VoiceRtpEncryptor>,
    /// Compartidos con `RtxSender`: los reenvíos usan el mismo contador de nonce.
    nonce: Arc<std::sync::atomic::AtomicU32>,
    history: Arc<std::sync::Mutex<PacketHistory>>,
    dave_state: Arc<Mutex<VoiceDaveState>>,
    ssrc: u32,
    frames_rx: mpsc::Receiver<EncodedFrame>,
    fatal_tx: mpsc::UnboundedSender<String>,
    events: std::sync::mpsc::Sender<AppEvent>,
    stream_key: String,
    /// Velocidad mínima del pacer (bytes/s), derivada del bitrate configurado.
    pacing_floor_bytes_per_sec: u64,
    /// fps configurados: fijan cuánto puede tardar en salir un frame.
    fps: u32,
    /// Para pedir una IDR en cuanto DAVE desbloquea el envío (sin esperar al GOP).
    keyframe: KeyframeRequest,
}

/// Bitrate enviado en una ventana deslizante de `PACING_WINDOW` (mismo cálculo
/// que `BitrateCalculator` de la versión Node: bytes de la ventana / duración
/// fija de la ventana).
#[derive(Default)]
struct BitrateWindow {
    samples: std::collections::VecDeque<(Instant, u64)>,
    total_bytes: u64,
}

impl BitrateWindow {
    /// Suma un frame y devuelve el bitrate de la ventana en bits/s.
    fn add_sample(&mut self, now: Instant, bytes: u64) -> u64 {
        self.samples.push_back((now, bytes));
        self.total_bytes += bytes;
        while let Some(&(at, old_bytes)) = self.samples.front() {
            if now.saturating_duration_since(at) > PACING_WINDOW {
                self.total_bytes -= old_bytes;
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.total_bytes * 8 * 1000 / PACING_WINDOW.as_millis() as u64
    }
}

/// Velocidad (bytes/s) a la que se reparten los paquetes de un frame: la
/// adaptativa (`1,25 ×` el bitrate medido, mínimo 1 Mbps), pero lo bastante alta
/// para que el frame entero salga en `PACING_MAX_SPREAD` como mucho.
fn pacing_rate_bytes_per_sec(
    measured_bits_per_sec: u64,
    frame_bytes: u64,
    floor_bytes_per_sec: u64,
    max_spread: Duration,
) -> u64 {
    let adaptive = (measured_bits_per_sec * PACING_HEADROOM_PERCENT / 100 / 8).max(floor_bytes_per_sec);
    adaptive.max(frame_bytes * 1000 / (max_spread.as_millis() as u64).max(1))
}

/// Tiempo máximo en que debe salir un frame: una IDR en `PACING_KEYFRAME_SPREAD`;
/// un frame P en ~3/4 de su intervalo (nunca más de `PACING_MAX_SPREAD`).
fn frame_max_spread(keyframe: bool, fps: u32) -> Duration {
    if keyframe {
        return PACING_KEYFRAME_SPREAD;
    }
    let interval_ms = 1000 / u64::from(fps.max(1));
    Duration::from_millis(interval_ms * PACING_P_FRAME_INTERVAL_PERCENT / 100)
        .max(PACING_P_FRAME_MIN_SPREAD)
        .min(PACING_MAX_SPREAD)
}

/// Bitrate (bits/s) de un REMB (draft-alvestrand-rmcat-remb) dentro de un
/// PSFB FMT 15; `None` si el paquete no es un REMB.
fn rtcp_remb_bitrate(kind: u8, format: u8, packet: &[u8]) -> Option<u64> {
    if kind != RTCP_PAYLOAD_SPECIFIC_FEEDBACK || format != 15 || packet.get(12..16)? != b"REMB" {
        return None;
    }
    let exp = u32::from(packet.get(17)? >> 2);
    let mantissa = (u64::from(packet.get(17)? & 0x03) << 16) | (u64::from(*packet.get(18)?) << 8) | u64::from(*packet.get(19)?);
    Some(mantissa.checked_shl(exp)?)
}

/// Saca los frames de la cola, los cifra con DAVE, los paquetiza y los manda.
async fn run_video_send(mut sender: VideoSender) {
    let result = send_frames(&mut sender).await;
    let message = match result {
        Ok(()) => "el codificador de video terminó".to_owned(),
        Err(error) => error,
    };
    let _ = sender.fatal_tx.send(message);
}

async fn send_frames(sender: &mut VideoSender) -> Result<(), String> {
    // Números de secuencia y de timestamp con arranque aleatorio (RFC 3550).
    let mut sequence: u16 = rand::random();
    let base_timestamp: u32 = rand::random();
    // El nonce del AEAD no se puede repetir con la misma clave.
    let started = Instant::now();
    let mut bitrate_window = BitrateWindow::default();

    // Se arranca esperando una IDR: un espectador no puede decodificar un
    // frame P sin su referencia, y tras cualquier descarte pasa lo mismo.
    let mut waiting_for_keyframe = true;
    let mut frames_sent = 0u64;
    let mut frames_blocked = 0u64;
    let mut frames_skipped = 0u64;
    let mut keyframes_sent = 0u64;
    let mut bytes_sent = 0u64;
    let mut last_stats = Instant::now();
    // Playout delay con el que salen los paquetes: se deja en el log al arrancar y en cada cambio.
    let mut logged_playout: Option<PlayoutDelay> = None;

    while let Some(frame) = sender.frames_rx.recv().await {
        let playout = playout_delay();
        if logged_playout != Some(playout) {
            logged_playout = Some(playout);
            logging::debug("stream", format!("playout delay de los paquetes de video: {}", playout.label()));
        }
        // Un resumen cada 5 s mientras el emisor vive: si el espectador pide
        // keyframes sin parar, esto dice si de verdad se está enviando algo, o si
        // todo se queda esperando a DAVE o a una IDR.
        if last_stats.elapsed() >= Duration::from_secs(5) {
            last_stats = Instant::now();
            logging::debug(
                "stream",
                format!(
                    "stream stats: sent={frames_sent} keyframes={keyframes_sent} bytes={bytes_sent} \
                     blocked_by_dave={frames_blocked} skipped_waiting_keyframe={frames_skipped}"
                ),
            );
        }
        if waiting_for_keyframe && !frame.keyframe {
            // Antes esto era silencioso: si el codificador nunca entrega una IDR,
            // la transmisión queda en negro sin ningún rastro en el log.
            frames_skipped = frames_skipped.saturating_add(1);
            if frames_skipped == 1 || frames_skipped % 100 == 0 {
                logging::debug(
                    "stream",
                    format!("waiting for a keyframe: {frames_skipped} frames skipped so far"),
                );
            }
            continue;
        }

        let outbound = sender.dave_state.lock().await.prepare_outbound_h264(&frame.data);
        let (access_unit, encrypted_by_dave) = match outbound {
            VoiceDaveOutboundPayload::Plain(data) => (data, false),
            VoiceDaveOutboundPayload::Encrypted(data) => (data, true),
            VoiceDaveOutboundPayload::Blocked(reason) => {
                // Típico al arrancar: el handshake MLS de DAVE todavía no terminó.
                waiting_for_keyframe = true;
                // No esperar al próximo GOP (hasta 1 s): al terminar la
                // transición MLS hace falta una IDR cifrada con la época nueva.
                sender.keyframe.request();
                frames_blocked = frames_blocked.saturating_add(1);
                if frames_blocked == 1 || frames_blocked % 100 == 0 {
                    logging::debug(
                        "stream",
                        format!("stream frame blocked ({frames_blocked} so far): {reason:?}"),
                    );
                }
                continue;
            }
        };

        // Timestamp por la hora en que salió del codificador, no la de envío.
        let timestamp = base_timestamp.wrapping_add(
            (frame.captured.saturating_duration_since(started).as_micros() as u64 * RTP_VIDEO_CLOCK_HZ / 1_000_000)
                as u32,
        );
        let payloads = packetize_h264_access_unit(&access_unit, RTP_VIDEO_MAX_PAYLOAD);
        let count = payloads.len();
        let frame_started = Instant::now();
        let mut paced_bytes: u64 = 0;
        // La IDR (~100 KB, 10 veces un frame normal) no puede estirarse más de
        // ~PACING_MAX_SPREAD: si no, retrasa los frames siguientes cada segundo.
        let total_bytes: u64 = payloads.iter().map(|p| p.len() as u64 + 12).sum();
        let measured_bps = bitrate_window.add_sample(frame_started, access_unit.len() as u64);
        let pacing_rate = pacing_rate_bytes_per_sec(
            measured_bps,
            total_bytes,
            sender.pacing_floor_bytes_per_sec,
            frame_max_spread(frame.keyframe, sender.fps),
        );
        for (index, payload) in payloads.iter().enumerate() {
            if count > 1 {
                let due = frame_started + Duration::from_micros(paced_bytes * 1_000_000 / pacing_rate);
                let ahead = due.saturating_duration_since(Instant::now());
                if ahead >= PACING_MIN_SLEEP {
                    tokio::time::sleep(ahead).await;
                }
            }
            paced_bytes += payload.len() as u64 + 12;
            let packet = build_video_rtp_packet(
                sequence,
                timestamp,
                sender.ssrc,
                PUBLISH_H264_PAYLOAD_TYPE,
                index + 1 == count,
                payload,
            );
            let nonce = sender.nonce.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if nonce == u32::MAX {
                return Err("stream RTP nonce suffix exhausted".to_owned());
            }
            let encrypted = sender.encryptor.encrypt_media_packet(&packet, nonce.to_be_bytes())?;
            if let Ok(mut history) = sender.history.lock() {
                history.push(SentPacket {
                    sequence,
                    timestamp,
                    marker: index + 1 == count,
                    payload: payload.to_vec(),
                });
            }
            sender
                .socket
                .send(&encrypted)
                .await
                .map_err(|error| format!("stream UDP send failed: {error}"))?;
            sequence = sequence.wrapping_add(1);
        }
        let send_took = frame_started.elapsed();
        if frame.keyframe {
            logging::debug(
                "stream",
                format!(
                    "IDR enviada: bytes={} packets={count} tardó={} ms",
                    access_unit.len(),
                    send_took.as_millis()
                ),
            );
        }
        if send_took >= SLOW_FRAME_LOG {
            logging::debug(
                "stream",
                format!(
                    "slow frame send: {} ms, packets={count}, bytes={}, keyframe={}",
                    send_took.as_millis(),
                    access_unit.len(),
                    frame.keyframe
                ),
            );
        }

        waiting_for_keyframe = false;
        frames_sent = frames_sent.saturating_add(1);
        bytes_sent = bytes_sent.saturating_add(access_unit.len() as u64);
        if frame.keyframe {
            keyframes_sent = keyframes_sent.saturating_add(1);
        }
        if frames_sent == 1 {
            let nals: Vec<u8> = split_annexb_nals(&frame.data).iter().map(|nal| nal[0] & 0x1F).collect();
            logging::debug(
                "stream",
                format!(
                    "first stream frame sent: keyframe={} bytes={} packets={count} dave_encrypted={encrypted_by_dave} \
                     plain_bytes={} nal_types={nals:?} blocked_before={frames_blocked} skipped_before={frames_skipped}",
                    frame.keyframe,
                    access_unit.len(),
                    frame.data.len(),
                ),
            );
            publish_status(&sender.events, &sender.stream_key, StreamPublishStatus::Live, None);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtcp_receiver_report_blocks_are_parsed() {
        let mut rr = vec![0x81, 201, 0x00, 0x07, 0, 0, 0, 1];
        rr.extend_from_slice(&0x0102_0304u32.to_be_bytes()); // ssrc del bloque
        rr.extend_from_slice(&[26, 0x00, 0x01, 0x02]); // fracción perdida + acumulado
        rr.extend_from_slice(&[0; 4]); // highest seq
        rr.extend_from_slice(&77u32.to_be_bytes()); // jitter
        rr.extend_from_slice(&[0; 8]); // LSR + DLSR
        let packets = rtcp_packets(&rr);
        assert_eq!(packets.len(), 1);
        assert_eq!((packets[0].0, packets[0].1), (201, 0));
        let blocks = rtcp_report_blocks(packets[0].0, packets[0].2);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ssrc, 0x0102_0304);
        assert_eq!(blocks[0].fraction_lost, 26);
        assert_eq!(blocks[0].cumulative_lost, 0x0102);
        assert_eq!(blocks[0].jitter, 77);
    }

    #[test]
    fn bitrate_window_averages_over_250ms_and_drops_old_samples() {
        let t0 = Instant::now();
        let mut window = BitrateWindow::default();
        // 10 KB en la ventana de 250 ms = 320 kbps.
        assert_eq!(window.add_sample(t0, 10_000), 320_000);
        assert_eq!(window.add_sample(t0 + Duration::from_millis(100), 10_000), 640_000);
        // A los 300 ms la primera muestra ya salió de la ventana.
        assert_eq!(window.add_sample(t0 + Duration::from_millis(300), 10_000), 640_000);
        // Mucho después solo queda la última.
        assert_eq!(window.add_sample(t0 + Duration::from_secs(5), 10_000), 320_000);
    }

    #[test]
    fn pacing_rate_follows_measured_bitrate_with_floor_and_spread_cap() {
        // Frame chico con bitrate medido bajo: manda el piso.
        assert_eq!(
            pacing_rate_bytes_per_sec(100_000, 500, PACING_MIN_BYTES_PER_SEC, PACING_MAX_SPREAD),
            PACING_MIN_BYTES_PER_SEC
        );
        // Piso relativo a 2500 kbps (1,5 ×): 468 750 bytes/s aunque lo medido sea poco.
        assert_eq!(pacing_rate_bytes_per_sec(320_000, 9_000, 468_750, PACING_MAX_SPREAD), 468_750);
        // 2,4 Mbps medidos: 1,25 × = 3 Mbps = 375 000 bytes/s.
        assert_eq!(pacing_rate_bytes_per_sec(2_400_000, 7_000, PACING_MIN_BYTES_PER_SEC, PACING_MAX_SPREAD), 375_000);
        // IDR de 100 KB: la adaptativa (~0,75 MB/s) no alcanza para repartirla
        // en 100 ms, manda el tope de reparto (1 MB/s).
        assert_eq!(pacing_rate_bytes_per_sec(4_800_000, 100_000, PACING_MIN_BYTES_PER_SEC, PACING_MAX_SPREAD),
            1_000_000
        );
    }

    #[test]
    fn frames_are_spread_within_their_interval() {
        assert_eq!(frame_max_spread(true, 30), Duration::from_millis(35));
        assert_eq!(frame_max_spread(true, 60), Duration::from_millis(35));
        assert_eq!(frame_max_spread(false, 30), Duration::from_millis(24));
        assert_eq!(frame_max_spread(false, 60), Duration::from_millis(12));
        // Con fps muy altos nunca baja del mínimo.
        assert_eq!(frame_max_spread(false, 240), PACING_P_FRAME_MIN_SPREAD);
        // IDR de 34 KB en 35 ms: ~0,97 MB/s (~7,8 Mbps), no 3,75 Mbps.
        assert_eq!(
            pacing_rate_bytes_per_sec(1_800_000, 34_000, 468_750, frame_max_spread(true, 60)),
            971_428
        );
    }

    #[test]
    fn remb_bitrate_is_decoded() {
        // exp=2, mantissa=0x0F424 (62 500) => 250 000 bits/s.
        let mut packet = vec![0x8F, 206, 0, 5, 0, 0, 0, 1, 0, 0, 0, 0];
        packet.extend_from_slice(b"REMB");
        packet.extend_from_slice(&[1, (2 << 2) | 0x00, 0xF4, 0x24, 0, 0, 0, 7]);
        assert_eq!(rtcp_remb_bitrate(206, 15, &packet), Some(250_000));
        assert_eq!(rtcp_remb_bitrate(205, 15, &packet), None);
        assert_eq!(rtcp_remb_bitrate(206, 15, &packet[..14]), None);
    }

    #[test]
    fn pli_and_fir_request_a_keyframe() {
        // PLI: V=2, FMT=1, PT=206, longitud 2 (3 palabras), SSRC emisor y de media.
        let pli = [0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        assert!(rtcp_requests_keyframe(&pli));
        // FIR: FMT=4, con una entrada FCI de 8 bytes.
        let fir = [0x84, 206, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0, 0];
        assert!(rtcp_requests_keyframe(&fir));
        // Receiver report (PT 201) seguido de un PLI: se encuentra en el compuesto.
        let mut compound = vec![0x80, 201, 0, 1, 0, 0, 0, 1];
        compound.extend_from_slice(&pli);
        assert!(rtcp_requests_keyframe(&compound));
    }

    #[test]
    fn nack_lists_the_lost_sequences() {
        // PID 5, BLP con los bits 0 y 2: se perdieron 5, 6 y 8.
        let nack = [0x81, 205, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 5, 0, 5];
        assert_eq!(rtcp_nacked_sequences(&nack), vec![5, 6, 8]);
    }

    #[test]
    fn history_finds_packets_by_sequence() {
        let mut history = PacketHistory::default();
        for sequence in [65_534u16, 65_535, 0, 1] {
            history.push(SentPacket {
                sequence,
                timestamp: 1,
                marker: false,
                payload: vec![1],
            });
        }
        assert!(history.get(0).is_some());
        assert!(history.get(2).is_none());
    }

    #[test]
    fn other_rtcp_does_not_request_a_keyframe() {
        // NACK genérico (RTPFB, PT 205, FMT 1) y receiver report.
        let nack = [0x81, 205, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 5, 0, 0];
        assert!(!rtcp_requests_keyframe(&nack));
        let report = [0x80, 201, 0, 1, 0, 0, 0, 1];
        assert!(!rtcp_requests_keyframe(&report));
        // Truncado, versión errónea y vacío.
        assert!(!rtcp_requests_keyframe(&[0x81, 206, 0, 9, 0, 0]));
        assert!(!rtcp_requests_keyframe(&[0x41, 206, 0, 2, 0, 0, 0, 1]));
        assert!(!rtcp_requests_keyframe(&[]));
    }

    #[test]
    fn video_ssrcs_come_from_ready_or_follow_the_convention() {
        let with_streams = json!({
            "op": 2,
            "d": { "ssrc": 10, "streams": [{ "type": "screen", "ssrc": 77, "rtx_ssrc": 78 }] },
        });
        assert_eq!(ready_video_ssrcs(&with_streams, 10), (77, 78));
        let without = json!({ "op": 2, "d": { "ssrc": 10 } });
        assert_eq!(ready_video_ssrcs(&without, 10), (11, 12));
    }

    #[test]
    fn video_payload_announces_even_resolution_and_bitrate() {
        let capture = ScreenCaptureConfig {
            width: 1281,
            height: 721,
            fps: 30,
            bitrate_kbps: 2500,
            ..Default::default()
        };
        let payload: Value = serde_json::from_str(&publish_video_payload(1, 2, 3, &capture)).unwrap();
        let stream = &payload["d"]["streams"][0];
        assert_eq!(payload["d"]["video_ssrc"], 2);
        assert_eq!(stream["max_bitrate"], 2_500_000);
        assert_eq!(stream["max_resolution"]["width"], 1280);
        assert_eq!(stream["max_resolution"]["height"], 720);
    }
}