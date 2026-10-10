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
//! Todavía no transmite audio del sistema (solo video).

use super::dave::{VoiceDaveOutboundPayload, handles_gateway_json_op};
use super::gateway::{
    choose_encryption_mode, discover_voice_udp_address, parse_voice_binary_frame,
    parse_voice_ready_payload, parse_voice_session_description, run_voice_heartbeat,
    run_voice_udp_ping, send_voice_text, voice_gateway_opcode, voice_gateway_url,
};
use super::h264::{
    EncodedFrame, RTP_VIDEO_CLOCK_HZ, RTP_VIDEO_MAX_PAYLOAD, build_video_rtp_packet,
    packetize_h264_access_unit,
};
use super::rtp::VoiceRtpDecryptor;
use super::screen_capture::{
    KeyframeRequest, ScreenCaptureConfig, ScreenEncoder, start_screen_encoder,
};
use super::*;

/// Payload types: los mismos que anuncia el visor y los clientes oficiales.
const PUBLISH_OPUS_PAYLOAD_TYPE: u8 = 120;
const PUBLISH_H264_PAYLOAD_TYPE: u8 = 101;
const PUBLISH_H264_RTX_PAYLOAD_TYPE: u8 = 102;

/// Frames codificados esperando a salir. Con GOP de 1 s, 8 son ~0,27 s a 30 fps.
const PUBLISH_QUEUE: usize = 8;

/// Para no volcar de golpe los ~25 paquetes de una IDR (se pierden en la red),
/// se hace una pausa corta cada tantos paquetes.
const PACING_EVERY_PACKETS: usize = 8;
const PACING_DELAY: Duration = Duration::from_micros(400);

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
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
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
    StreamPublishHandle {
        stream_key,
        stop_tx,
    }
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

async fn run_stream_publish(
    params: StreamPublishParams,
    events: std::sync::mpsc::Sender<AppEvent>,
    stop_rx: watch::Receiver<bool>,
) {
    logging::debug("stream", format!("stream publish starting: {params:?}"));
    publish_status(
        &events,
        &params.stream_key,
        StreamPublishStatus::Connecting,
        None,
    );
    match publish_stream_session(&params, &events, stop_rx).await {
        Ok(()) => {
            logging::debug("stream", "stream publish ended");
            publish_status(&events, &params.stream_key, StreamPublishStatus::Ended, None);
        }
        Err(error) => {
            logging::error("stream", &error);
            publish_status(
                &events,
                &params.stream_key,
                StreamPublishStatus::Failed,
                Some(error),
            );
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
    {
        let capture = params.capture.clone();
        let fatal_tx = fatal_tx.clone();
        let keyframe_request = keyframe_request.clone();
        let preview = params.preview.clone();
        tasks.push(tokio::spawn(async move {
            let started = tokio::task::spawn_blocking(move || {
                start_screen_encoder(&capture, frames_tx, keyframe_request, preview)
            })
            .await;
            match started {
                Ok(Ok(encoder)) => hold_encoder(encoder).await,
                Ok(Err(error)) => {
                    let _ = fatal_tx.send(format!("no se pudo iniciar la captura: {error}"));
                }
                Err(error) => {
                    let _ = fatal_tx.send(format!("la tarea de captura falló: {error}"));
                }
            }
        }));
    }
    let mut frames_rx = Some(frames_rx);

    let url = voice_gateway_url(&params.endpoint)?;
    logging::debug("stream", format!("connecting stream websocket: {url}"));
    let ws = timeout(VOICE_WEBSOCKET_CONNECT_TIMEOUT, websocket::connect(&url))
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
    let (heartbeat_timeout_tx, mut heartbeat_timeout_rx) =
        mpsc::unbounded_channel::<VoiceHeartbeatTimeout>();

    let mut udp_socket: Option<Arc<UdpSocket>> = None;
    // (audio, video, rtx) — se completan en READY.
    let mut ssrcs: Option<(u32, u32, u32)> = None;
    let mut sending = false;

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
                        let mode = choose_encryption_mode(&ready.modes)?;
                        let (socket, discovered) = discover_voice_udp_address(&ready).await?;
                        send_voice_text(
                            &writer,
                            publish_select_protocol_payload(&discovered, &mode),
                        )
                        .await?;
                        tasks.push(tokio::spawn(run_voice_udp_ping(Arc::clone(&socket))));
                        udp_socket = Some(socket);
                    }
                    VOICE_OP_SESSION_DESCRIPTION => {
                        let description = parse_voice_session_description(&value)?;
                        logging::debug(
                            "stream",
                            format!("stream publish session description: {description:?}"),
                        );
                        dave_state
                            .lock()
                            .await
                            .apply_protocol_version(description.dave_protocol_version)?;
                        let (Some(socket), Some((audio_ssrc, video_ssrc, rtx_ssrc))) =
                            (udp_socket.as_ref(), ssrcs)
                        else {
                            return Err(
                                "stream session description arrived before the UDP transport"
                                    .to_owned(),
                            );
                        };
                        // Un session description repetido no reinicia el envío.
                        if sending {
                            continue;
                        }
                        sending = true;
                        let encryptor =
                            VoiceRtpEncryptor::new(&description.mode, &description.secret_key)?;

                        // "Voy a mandar video con estos SSRC y esta resolución."
                        send_voice_text(&writer, publish_speaking_payload(audio_ssrc)).await?;
                        send_voice_text(
                            &writer,
                            publish_video_payload(
                                audio_ssrc,
                                video_ssrc,
                                rtx_ssrc,
                                &params.capture,
                            ),
                        )
                        .await?;

                        let Some(frames_rx) = frames_rx.take() else {
                            return Err("stream frame queue already taken".to_owned());
                        };
                        // Lo que llega por el UDP: respuestas al ping y RTCP de los
                        // espectadores (un PLI/FIR fuerza una IDR).
                        let decryptor = VoiceRtpDecryptor::new(
                            &description.mode,
                            &description.secret_key,
                        )?;
                        tasks.push(tokio::spawn(run_feedback(
                            Arc::clone(socket),
                            decryptor,
                            keyframe_request.clone(),
                        )));
                        tasks.push(tokio::spawn(run_video_send(VideoSender {
                            socket: Arc::clone(socket),
                            encryptor,
                            dave_state: Arc::clone(&dave_state),
                            ssrc: video_ssrc,
                            frames_rx,
                            fatal_tx: fatal_tx.clone(),
                            events: events.clone(),
                            stream_key: params.stream_key.clone(),
                        })));
                        publish_status(
                            events,
                            &params.stream_key,
                            StreamPublishStatus::Connected,
                            None,
                        );
                    }
                    VOICE_OP_VIDEO => {
                        logging::debug("stream", format!("stream video op: {value}"));
                    }
                    other => {
                        if handles_gateway_json_op(other) {
                            dave_state
                                .lock()
                                .await
                                .handle_json_op(&writer, other, &value)
                                .await?;
                        } else {
                            logging::debug("stream", format!("unhandled stream gateway op={other}"));
                        }
                    }
                }
            }
            WsMessage::Binary(payload) => {
                let frame = parse_voice_binary_frame(&payload)?;
                *last_sequence.lock().await = Some(frame.sequence);
                dave_state
                    .lock()
                    .await
                    .handle_binary_frame(&writer, frame)
                    .await?;
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

/// Mantiene vivo el codificador mientras dure la sesión. Se aborta con el resto
/// de las tareas, y al soltarse el `ScreenEncoder` se detiene su hilo.
async fn hold_encoder(encoder: ScreenEncoder) {
    logging::debug(
        "stream",
        format!("holding screen encoder {}", encoder.encoder_name()),
    );
    std::future::pending::<()>().await;
    drop(encoder);
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
fn publish_video_payload(
    audio_ssrc: u32,
    video_ssrc: u32,
    rtx_ssrc: u32,
    capture: &ScreenCaptureConfig,
) -> String {
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
const RTCP_FMT_FIR: u8 = 4;

/// ¿El paquete RTCP (compuesto, ya descifrado) pide una imagen completa?
/// Recorre los paquetes de la cabecera común (V/P/FMT, PT, longitud en
/// palabras de 32 bits menos uno) y busca un PLI o un FIR. Hay un solo stream
/// de video, así que no se mira a qué SSRC apunta.
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
        if kind == RTCP_PAYLOAD_SPECIFIC_FEEDBACK
            && (format == RTCP_FMT_PLI || format == RTCP_FMT_FIR)
        {
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
) {
    let mut packet = vec![0u8; 2048];
    let mut requests = 0u64;
    loop {
        match socket.recv(&mut packet).await {
            Ok(len) => {
                let Ok(plain) = decryptor.decrypt_rtcp_feedback(&packet[..len]) else {
                    // Respuesta al ping, RTP ajeno o RTCP que no es de feedback
                    // cifrado: no interesa.
                    continue;
                };
                if rtcp_requests_keyframe(&plain) {
                    keyframe.request();
                    requests += 1;
                    if requests == 1 || requests % 20 == 0 {
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
    encryptor: VoiceRtpEncryptor,
    dave_state: Arc<Mutex<VoiceDaveState>>,
    ssrc: u32,
    frames_rx: mpsc::Receiver<EncodedFrame>,
    fatal_tx: mpsc::UnboundedSender<String>,
    events: std::sync::mpsc::Sender<AppEvent>,
    stream_key: String,
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
    let mut nonce_suffix: u32 = 0;
    let started = Instant::now();

    // Se arranca esperando una IDR: un espectador no puede decodificar un
    // frame P sin su referencia, y tras cualquier descarte pasa lo mismo.
    let mut waiting_for_keyframe = true;
    let mut frames_sent = 0u64;
    let mut frames_blocked = 0u64;

    while let Some(frame) = sender.frames_rx.recv().await {
        if waiting_for_keyframe && !frame.keyframe {
            continue;
        }

        let outbound = sender
            .dave_state
            .lock()
            .await
            .prepare_outbound_h264(&frame.data);
        let access_unit = match outbound {
            VoiceDaveOutboundPayload::Plain(data) | VoiceDaveOutboundPayload::Encrypted(data) => {
                data
            }
            VoiceDaveOutboundPayload::Blocked(reason) => {
                // Típico al arrancar: el handshake MLS de DAVE todavía no terminó.
                waiting_for_keyframe = true;
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

        let timestamp = base_timestamp.wrapping_add(
            (started.elapsed().as_micros() as u64 * RTP_VIDEO_CLOCK_HZ / 1_000_000) as u32,
        );
        let payloads = packetize_h264_access_unit(&access_unit, RTP_VIDEO_MAX_PAYLOAD);
        let count = payloads.len();
        for (index, payload) in payloads.iter().enumerate() {
            let packet = build_video_rtp_packet(
                sequence,
                timestamp,
                sender.ssrc,
                PUBLISH_H264_PAYLOAD_TYPE,
                index + 1 == count,
                payload,
            );
            let encrypted = sender
                .encryptor
                .encrypt_media_packet(&packet, nonce_suffix.to_be_bytes())?;
            sender
                .socket
                .send(&encrypted)
                .await
                .map_err(|error| format!("stream UDP send failed: {error}"))?;
            sequence = sequence.wrapping_add(1);
            nonce_suffix = nonce_suffix
                .checked_add(1)
                .ok_or_else(|| "stream RTP nonce suffix exhausted".to_owned())?;
            if (index + 1) % PACING_EVERY_PACKETS == 0 && index + 1 < count {
                tokio::time::sleep(PACING_DELAY).await;
            }
        }

        waiting_for_keyframe = false;
        frames_sent = frames_sent.saturating_add(1);
        if frames_sent == 1 {
            logging::debug(
                "stream",
                format!(
                    "first stream frame sent: keyframe={} bytes={} packets={count}",
                    frame.keyframe,
                    access_unit.len()
                ),
            );
            publish_status(
                &sender.events,
                &sender.stream_key,
                StreamPublishStatus::Live,
                None,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pli_and_fir_request_a_keyframe() {
        // PLI: V=2, FMT=1, PT=206, longitud 2 (3 palabras), SSRC emisor y de media.
        let pli = [0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        assert!(rtcp_requests_keyframe(&pli));
        // FIR: FMT=4, con una entrada FCI de 8 bytes.
        let fir = [
            0x84, 206, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0, 0,
        ];
        assert!(rtcp_requests_keyframe(&fir));
        // Receiver report (PT 201) seguido de un PLI: se encuentra en el compuesto.
        let mut compound = vec![0x80, 201, 0, 1, 0, 0, 0, 1];
        compound.extend_from_slice(&pli);
        assert!(rtcp_requests_keyframe(&compound));
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
        let payload: Value =
            serde_json::from_str(&publish_video_payload(1, 2, 3, &capture)).unwrap();
        let stream = &payload["d"]["streams"][0];
        assert_eq!(payload["d"]["video_ssrc"], 2);
        assert_eq!(stream["max_bitrate"], 2_500_000);
        assert_eq!(stream["max_resolution"]["width"], 1280);
        assert_eq!(stream["max_resolution"]["height"], 720);
    }
}
