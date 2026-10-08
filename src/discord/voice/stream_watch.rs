//! Ver el stream (Go Live) de otra persona.
//!
//! El stream tiene su propia conexión de media, separada de la de voz del
//! canal: `STREAM_WATCH` (opcode 20 del Gateway principal) hace que Discord
//! mande `STREAM_CREATE` (con el `rtc_server_id`) y `STREAM_SERVER_UPDATE`
//! (con endpoint + token). Con eso se abre un segundo WebSocket de voz, se
//! negocia UDP igual que en una llamada y se recibe video H.264 por RTP.
//!
//! Este módulo NO pasa por la máquina de estados de `runtime.rs` (pensada para
//! UNA conexión de voz con micrófono y parlantes): corre en su propio hilo con
//! su propio runtime de tokio y reutiliza solo las piezas de bajo nivel
//! (`VoiceDaveState`, `VoiceRtpDecryptor`, descubrimiento UDP, heartbeat).
//!
//! Video H.264 y audio Opus llegan por el mismo UDP. El audio se descifra
//! (transporte + DAVE), se decodifica y suena por su propia salida, con
//! volumen y silencio propios (`StreamAudioControl`).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};

use super::dave::handles_gateway_json_op;
use super::gateway::{
    choose_encryption_mode, discover_voice_udp_address, parse_udp_ping_response,
    parse_voice_binary_frame, parse_voice_ready_payload, parse_voice_session_description,
    run_voice_heartbeat, run_voice_udp_ping, send_voice_text, voice_gateway_opcode,
    voice_gateway_url,
};
use super::h264::{H264Depacketizer, RtpReorderBuffer, RtpVideoPacket};
use super::video_decode::H264Decoder;
use super::*;
#[cfg(feature = "voice-playback")]
use super::opus::VoiceDecodedAudioOutput;
#[cfg(feature = "voice-playback")]
use super::playback::VoiceAudioOutput;

/// Payload types que se anuncian en `SELECT_PROTOCOL`. Coinciden con los que
/// anuncian los clientes oficiales para Opus (120) y H.264 (101, RTX 102).
const STREAM_OPUS_PAYLOAD_TYPE: u8 = 120;
const STREAM_H264_PAYLOAD_TYPE: u8 = 101;
const STREAM_H264_RTX_PAYLOAD_TYPE: u8 = 102;

/// Access units en cola hacia el hilo de decodificación. Con ~60 fps son unos
/// 0,5 s de margen; si se llena, el decoder no da abasto y se descartan frames.
const STREAM_DECODE_QUEUE: usize = 32;

/// Cada cuánto se puede repetir un pedido de keyframe (PLI).
const PLI_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Estado de la conexión de media del stream que se está viendo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamWatchStatus {
    /// Abriendo el WebSocket / negociando UDP.
    Connecting,
    /// Transporte negociado; falta que llegue el primer frame decodificable.
    Connected,
    /// Ya se está recibiendo y mostrando video.
    Receiving,
    /// La conexión se cortó por un error (ver el mensaje).
    Failed,
    /// Se dejó de ver, o el stream terminó.
    Ended,
}

/// Último frame decodificado. El hilo de decodificación pisa el anterior: la UI
/// solo necesita el más reciente, así que no hay cola que se pueda atrasar.
#[derive(Clone, Default)]
pub struct StreamFrameSlot {
    inner: Arc<std::sync::Mutex<Option<StreamFrame>>>,
}

/// Un frame RGBA listo para subir a una textura.
pub struct StreamFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA sin premultiplicar.
    pub rgba: Vec<u8>,
}

impl StreamFrameSlot {
    fn store(&self, frame: StreamFrame) {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        *guard = Some(frame);
    }

    /// Saca el frame nuevo, si llegó uno desde la última vez.
    pub fn take_new(&self) -> Option<StreamFrame> {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        guard.take()
    }
}

/// Datos que hacen falta para conectarse al servidor de media de un stream.
#[derive(Clone)]
pub(crate) struct StreamWatchParams {
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
    /// Quién transmite: es de quien hay que descifrar el video con DAVE.
    pub(crate) owner_user_id: Id<UserMarker>,
    /// Dispositivo de salida elegido en Ajustes (`None` = el predeterminado)
    /// por donde suena el audio del stream.
    pub(crate) output_source: Option<String>,
}

impl fmt::Debug for StreamWatchParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamWatchParams")
            .field("stream_key", &self.stream_key)
            .field("rtc_server_id", &self.rtc_server_id)
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .field("user_id", &self.user_id)
            .field("session_id", &"<redacted>")
            .field("owner_user_id", &self.owner_user_id)
            .finish()
    }
}

/// Extremo por el que la tarea de recepción le pasa PCM a la salida de audio
/// del stream (`()` si el cliente se compiló sin reproducción de audio).
#[cfg(feature = "voice-playback")]
type StreamAudioSink = VoiceDecodedAudioOutput;
#[cfg(not(feature = "voice-playback"))]
type StreamAudioSink = ();

/// Mango de una sesión de visualización en curso. Al soltarlo (`Drop`) o al
/// llamar a `stop`, la conexión se cierra.
pub(crate) struct StreamWatchHandle {
    pub(crate) stream_key: String,
    pub(crate) frames: StreamFrameSlot,
    /// Volumen y silencio del audio del stream (se puede cambiar en vivo).
    pub(crate) audio: StreamAudioControl,
    stop_tx: watch::Sender<bool>,
}

/// Volumen y silencio del audio de un stream, compartidos con el callback de
/// salida: cambiarlos tiene efecto en el siguiente bloque de audio, sin
/// reconectar nada.
#[derive(Clone)]
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
pub(crate) struct StreamAudioControl {
    volume: Arc<AtomicU8>,
    enabled: Arc<AtomicBool>,
}

impl StreamAudioControl {
    fn new() -> Self {
        Self {
            volume: Arc::new(AtomicU8::new(100)),
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// `percent`: 0..=200 (100 = normal). `muted` corta el audio sin perder el
    /// volumen elegido.
    pub(crate) fn set(&self, percent: u8, muted: bool) {
        self.volume.store(percent.min(200), AtomicOrdering::Relaxed);
        self.enabled.store(!muted, AtomicOrdering::Relaxed);
    }
}

impl StreamWatchHandle {
    pub(crate) fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

impl Drop for StreamWatchHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Abre la conexión de media del stream en un hilo aparte. Los cambios de
/// estado llegan como `AppEvent::StreamWatchStatus`; los frames, por
/// `handle.frames`. `repaint` se llama cada vez que hay un frame nuevo (el hilo
/// de Discord no despierta la UI por sí solo).
pub(crate) fn spawn_stream_watch(
    params: StreamWatchParams,
    events: std::sync::mpsc::Sender<AppEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> StreamWatchHandle {
    let frames = StreamFrameSlot::default();
    let audio = StreamAudioControl::new();
    let (stop_tx, stop_rx) = watch::channel(false);
    let stream_key = params.stream_key.clone();

    let thread_frames = frames.clone();
    let thread_audio = audio.clone();
    let thread_events = events.clone();
    let spawned = std::thread::Builder::new()
        .name("stream-watch".to_owned())
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
                        StreamWatchStatus::Failed,
                        Some(format!("could not start the stream runtime: {error}")),
                    );
                    return;
                }
            };
            runtime.block_on(run_stream_watch(
                params,
                thread_events,
                thread_frames,
                thread_audio,
                repaint,
                stop_rx,
            ));
        });
    if let Err(error) = spawned {
        publish_status(
            &events,
            &stream_key,
            StreamWatchStatus::Failed,
            Some(format!("could not start the stream thread: {error}")),
        );
    }

    StreamWatchHandle {
        stream_key,
        frames,
        audio,
        stop_tx,
    }
}

fn publish_status(
    events: &std::sync::mpsc::Sender<AppEvent>,
    stream_key: &str,
    status: StreamWatchStatus,
    message: Option<String>,
) {
    let _ = events.send(AppEvent::StreamWatchStatus {
        stream_key: stream_key.to_owned(),
        status,
        message,
    });
}

async fn run_stream_watch(
    params: StreamWatchParams,
    events: std::sync::mpsc::Sender<AppEvent>,
    frames: StreamFrameSlot,
    audio: StreamAudioControl,
    repaint: Arc<dyn Fn() + Send + Sync>,
    stop_rx: watch::Receiver<bool>,
) {
    logging::debug("stream", format!("stream watch starting: {params:?}"));
    publish_status(
        &events,
        &params.stream_key,
        StreamWatchStatus::Connecting,
        None,
    );
    match watch_stream_session(&params, &events, &frames, &audio, &repaint, stop_rx).await {
        Ok(()) => {
            logging::debug("stream", "stream watch ended");
            publish_status(&events, &params.stream_key, StreamWatchStatus::Ended, None);
        }
        Err(error) => {
            logging::error("stream", &error);
            publish_status(
                &events,
                &params.stream_key,
                StreamWatchStatus::Failed,
                Some(error),
            );
        }
    }
}

/// Aborta las tareas hijas cuando la sesión termina (por error o no).
#[derive(Default)]
struct StreamTasks {
    handles: Vec<JoinHandle<()>>,
}

impl StreamTasks {
    fn push(&mut self, handle: JoinHandle<()>) {
        self.handles.push(handle);
    }
}

impl Drop for StreamTasks {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

async fn watch_stream_session(
    params: &StreamWatchParams,
    events: &std::sync::mpsc::Sender<AppEvent>,
    frames: &StreamFrameSlot,
    audio: &StreamAudioControl,
    repaint: &Arc<dyn Fn() + Send + Sync>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), String> {
    let url = voice_gateway_url(&params.endpoint)?;
    logging::debug("stream", format!("connecting stream websocket: {url}"));
    let (ws, response) = timeout(
        VOICE_WEBSOCKET_CONNECT_TIMEOUT,
        connect_async_tls_with_config(&url, None, false, None),
    )
    .await
    .map_err(|_| "stream websocket connect timed out after 10s".to_owned())?
    .map_err(|error| format!("stream websocket connect failed: {error}"))?;
    logging::debug(
        "stream",
        format!("stream websocket connected: status={}", response.status()),
    );

    let (writer, mut reader) = ws.split();
    let writer: VoiceWriter = Arc::new(Mutex::new(writer));
    send_voice_text(&writer, stream_identify_payload(params)).await?;

    let dave_state = Arc::new(Mutex::new(VoiceDaveState::new_for_stream(
        params.user_id,
        &params.rtc_server_id,
    )?));
    let last_sequence: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let heartbeat_ack = Arc::new(Mutex::new(VoiceHeartbeatAckState::default()));
    let (heartbeat_timeout_tx, mut heartbeat_timeout_rx) =
        mpsc::unbounded_channel::<VoiceHeartbeatTimeout>();

    // Decodificación en un hilo de SO aparte: es CPU pesado y no debe frenar
    // la lectura del socket. Termina sola cuando se sueltan todos los
    // `decode_tx` (la sesión termina y las tareas hijas se abortan).
    let (decode_tx, decode_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(STREAM_DECODE_QUEUE);
    {
        let frames = frames.clone();
        let repaint = Arc::clone(repaint);
        let events = events.clone();
        let stream_key = params.stream_key.clone();
        let _ = std::thread::Builder::new()
            .name("stream-decode".to_owned())
            .spawn(move || run_decode_thread(decode_rx, frames, repaint, events, stream_key));
    }

    // Salida de audio del stream. `_stream_audio_output` tiene que seguir vivo
    // mientras dure la sesión (si se suelta, se corta el sonido), y vive acá
    // —no en la tarea de recepción— porque el stream de cpal no es `Send`.
    #[cfg(feature = "voice-playback")]
    let (_stream_audio_output, stream_audio_sink) =
        open_stream_audio(audio, params.output_source.as_deref());
    #[cfg(not(feature = "voice-playback"))]
    let (stream_audio_sink, _) = ((), audio);

    let mut tasks = StreamTasks::default();
    let mut udp_socket: Option<Arc<UdpSocket>> = None;
    let mut our_ssrc = 0u32;
    let mut receiving = false;

    loop {
        let frame = tokio::select! {
            _ = stop_rx.changed() => return Ok(()),
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
                        logging::debug(
                            "stream",
                            format!(
                                "stream ready: ssrc={} udp={}:{} modes={}",
                                ready.ssrc,
                                ready.ip,
                                ready.port,
                                ready.modes.len()
                            ),
                        );
                        our_ssrc = ready.ssrc;
                        let mode = choose_encryption_mode(&ready.modes)?;
                        let (socket, discovered) = discover_voice_udp_address(&ready).await?;
                        send_voice_text(
                            &writer,
                            stream_select_protocol_payload(&discovered, &mode),
                        )
                        .await?;
                        tasks.push(tokio::spawn(run_voice_udp_ping(Arc::clone(&socket))));
                        udp_socket = Some(socket);
                    }
                    VOICE_OP_SESSION_DESCRIPTION => {
                        let description = parse_voice_session_description(&value)?;
                        logging::debug(
                            "stream",
                            format!("stream session description: {description:?}"),
                        );
                        dave_state
                            .lock()
                            .await
                            .apply_protocol_version(description.dave_protocol_version)?;
                        let Some(socket) = udp_socket.as_ref() else {
                            return Err(
                                "stream session description arrived before the UDP transport"
                                    .to_owned(),
                            );
                        };
                        // Un session description repetido no reinicia la
                        // recepción: solo el primero arranca la tarea.
                        if !receiving {
                            receiving = true;
                            tasks.push(tokio::spawn(run_stream_video_receive(
                                Arc::clone(socket),
                                description,
                                Arc::clone(&dave_state),
                                params.owner_user_id,
                                our_ssrc,
                                decode_tx.clone(),
                                stream_audio_sink.clone(),
                            )));
                            // "Quiero cualquier stream, calidad máxima."
                            send_voice_text(&writer, stream_sink_wants_payload()).await?;
                            publish_status(
                                events,
                                &params.stream_key,
                                StreamWatchStatus::Connected,
                                None,
                            );
                        }
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
                    .map(|frame| format!("code={} reason={}", frame.code, frame.reason))
                    .unwrap_or_else(|| "no close frame".to_owned());
                return Err(format!("stream websocket closed: {detail}"));
            }
            WsMessage::Pong(_) | WsMessage::Frame(_) => {}
        }
    }
}

/// IDENTIFY (opcode 0) del WebSocket de media del stream. `server_id` es el
/// `rtc_server_id`, no el id del server/canal.
fn stream_identify_payload(params: &StreamWatchParams) -> String {
    json!({
        "op": 0,
        "d": {
            "server_id": params.rtc_server_id,
            "user_id": params.user_id.to_string(),
            "session_id": params.session_id,
            "token": params.token,
            "video": true,
            "max_dave_protocol_version": davey::DAVE_PROTOCOL_VERSION,
        },
    })
    .to_string()
}

/// SELECT_PROTOCOL (opcode 1) para un espectador: además del modo de cifrado
/// anuncia qué codecs sabe DECODIFICAR (`decode: true`, `encode: false`), y
/// así el servidor le manda H.264.
fn stream_select_protocol_payload(discovered: &DiscoveredVoiceAddress, mode: &str) -> String {
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
                    "payload_type": STREAM_OPUS_PAYLOAD_TYPE,
                },
                {
                    "name": "H264",
                    "type": "video",
                    "priority": 1000,
                    "payload_type": STREAM_H264_PAYLOAD_TYPE,
                    "rtx_payload_type": STREAM_H264_RTX_PAYLOAD_TYPE,
                    "encode": false,
                    "decode": true,
                },
            ],
        },
    })
    .to_string()
}

/// MEDIA_SINK_WANTS (opcode 15): pide cualquier stream de video a calidad
/// máxima.
fn stream_sink_wants_payload() -> String {
    json!({
        "op": VOICE_OP_MEDIA_SINK_WANTS,
        "d": { "any": 100 },
    })
    .to_string()
}

/// Recibe el UDP del stream: descifra el transporte, reordena, arma frames
/// H.264, los descifra con DAVE y los manda al hilo de decodificación.
async fn run_stream_video_receive(
    socket: Arc<UdpSocket>,
    description: VoiceSessionDescription,
    dave_state: Arc<Mutex<VoiceDaveState>>,
    owner_user_id: Id<UserMarker>,
    sender_ssrc: u32,
    decode_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    audio_sink: StreamAudioSink,
) {
    let decryptor = match VoiceRtpDecryptor::new(&description.mode, &description.secret_key) {
        Ok(decryptor) => decryptor,
        Err(error) => {
            logging::error("stream", format!("stream RTP decrypt setup failed: {error}"));
            return;
        }
    };
    // Para pedir keyframes (PLI) por RTCP: hace falta poder cifrar.
    let encryptor = match VoiceRtpEncryptor::new(&description.mode, &description.secret_key) {
        Ok(encryptor) => Some(encryptor),
        Err(error) => {
            logging::error("stream", format!("stream RTCP encrypt setup failed: {error}"));
            None
        }
    };
    let mut rtcp_nonce = 0u32;
    let mut last_pli: Option<Instant> = None;
    let mut plis_sent = 0u64;
    let mut video_ssrc = 0u32;
    logging::debug(
        "stream",
        format!("stream UDP receive active: mode={}", description.mode),
    );

    #[cfg(feature = "voice-playback")]
    let mut audio_player = StreamAudioPlayer::new(audio_sink);
    #[cfg(not(feature = "voice-playback"))]
    let () = audio_sink;

    let mut packet = vec![0u8; 2048];
    let mut reorder = RtpReorderBuffer::default();
    let mut depacketizer = H264Depacketizer::default();
    let mut video_packets = 0u64;
    let mut transport_failures = 0u64;
    let mut dave_pending = 0u64;
    let mut dave_failures = 0u64;
    let mut frames_sent = 0u64;
    let mut frames_dropped = 0u64;

    loop {
        let len = match socket.recv(&mut packet).await {
            Ok(len) => len,
            Err(error) => {
                logging::error("stream", format!("stream UDP receive failed: {error}"));
                return;
            }
        };
        let datagram = &packet[..len];
        if parse_udp_ping_response(datagram).is_some() || looks_like_rtcp_packet(datagram) {
            continue;
        }
        let Ok(header) = parse_rtp_header(datagram) else {
            continue;
        };
        // Audio del stream (Opus): se descifra, se decodifica y suena por la
        // salida del stream, con su propio volumen.
        #[cfg(feature = "voice-playback")]
        {
            if header.payload_type == STREAM_OPUS_PAYLOAD_TYPE {
                audio_player
                    .push_packet(&decryptor, datagram, &header, &dave_state, owner_user_id)
                    .await;
                continue;
            }
        }
        // Del resto solo interesa el video H.264: las retransmisiones (RTX) se
        // ignoran por ahora.
        if header.payload_type != STREAM_H264_PAYLOAD_TYPE {
            continue;
        }
        video_packets = video_packets.saturating_add(1);
        video_ssrc = header.ssrc;
        if video_packets == 1 {
            logging::debug(
                "stream",
                format!(
                    "first stream video packet: ssrc={} seq={} timestamp={}",
                    header.ssrc, header.sequence, header.timestamp
                ),
            );
        }

        // `decrypt_packet` solo acepta el payload type de audio; para video hay
        // que usar la variante que no filtra por tipo.
        let payload = match decryptor.decrypt_packet_any(datagram, &header) {
            Ok(payload) => payload,
            Err(error) => {
                transport_failures = transport_failures.saturating_add(1);
                if transport_failures == 1 || transport_failures.is_multiple_of(200) {
                    logging::debug(
                        "stream",
                        format!(
                            "stream RTP decrypt failed: count={transport_failures} seq={} error={error}",
                            header.sequence
                        ),
                    );
                }
                continue;
            }
        };

        let ready = reorder.push(RtpVideoPacket {
            sequence: header.sequence,
            timestamp: header.timestamp,
            marker: header.marker,
            payload: payload.media_payload,
        });
        for video_packet in ready {
            let Some(access_unit) = depacketizer.push(&video_packet) else {
                continue;
            };
            let decrypted = dave_state
                .lock()
                .await
                .decrypt_video_frame(owner_user_id, &access_unit);
            let frame = match decrypted {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    dave_pending = dave_pending.saturating_add(1);
                    if dave_pending == 1 || dave_pending.is_multiple_of(200) {
                        logging::debug(
                            "stream",
                            format!("stream frame waiting for DAVE: count={dave_pending}"),
                        );
                    }
                    continue;
                }
                Err(error) => {
                    dave_failures = dave_failures.saturating_add(1);
                    if dave_failures == 1 || dave_failures.is_multiple_of(200) {
                        logging::debug(
                            "stream",
                            format!("stream frame DAVE decrypt failed: count={dave_failures} error={error}"),
                        );
                    }
                    continue;
                }
            };
            match decode_tx.try_send(frame) {
                Ok(()) => {
                    frames_sent = frames_sent.saturating_add(1);
                    if frames_sent == 1 {
                        logging::debug("stream", "first stream frame queued for decoding");
                    } else if frames_sent.is_multiple_of(300) {
                        logging::debug(
                            "stream",
                            format!("stream frames queued for decoding: {frames_sent}"),
                        );
                    }
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    frames_dropped = frames_dropped.saturating_add(1);
                    if frames_dropped == 1 || frames_dropped.is_multiple_of(100) {
                        logging::debug(
                            "stream",
                            format!("stream decoder is behind, dropped frames={frames_dropped}"),
                        );
                    }
                }
                // El hilo de decodificación terminó (falló al abrir ffmpeg).
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
            }
        }

        // Sin keyframe no hay nada que mostrar, y los streams de Discord no
        // mandan keyframes periódicas: se piden con un PLI (RTCP). Al arrancar
        // y cada vez que se pierde algo; como mucho una por segundo.
        if depacketizer.needs_keyframe()
            && let Some(encryptor) = encryptor.as_ref()
            && last_pli.is_none_or(|at| at.elapsed() >= PLI_MIN_INTERVAL)
        {
            last_pli = Some(Instant::now());
            match send_keyframe_request(&socket, encryptor, &mut rtcp_nonce, sender_ssrc, video_ssrc)
                .await
            {
                Ok(()) => {
                    plis_sent = plis_sent.saturating_add(1);
                    if plis_sent <= 5 || plis_sent.is_multiple_of(20) {
                        logging::debug(
                            "stream",
                            format!(
                                "keyframe requested (PLI): count={plis_sent} frames_sent={frames_sent}"
                            ),
                        );
                    }
                }
                Err(error) => {
                    logging::debug("stream", format!("keyframe request failed: {error}"));
                }
            }
        }
    }
}

/// Abre la salida de audio del stream y devuelve, junto a ella, el extremo con
/// el que la tarea de recepción le manda PCM. Sin dispositivo de salida el
/// stream se ve igual, solo que sin sonido.
#[cfg(feature = "voice-playback")]
fn open_stream_audio(
    control: &StreamAudioControl,
    output_source: Option<&str>,
) -> (Option<VoiceAudioOutput>, VoiceDecodedAudioOutput) {
    let sink = VoiceDecodedAudioOutput::default();
    let output = match VoiceAudioOutput::start(
        Arc::clone(&control.enabled),
        Arc::clone(&control.volume),
        output_source,
    ) {
        Ok(output) => Some(output),
        Err(error) => {
            logging::error("stream", format!("stream audio output unavailable: {error}"));
            None
        }
    };
    sink.replace(output.as_ref());
    (output, sink)
}

/// Audio del stream: descifra (transporte + DAVE), decodifica Opus y se lo pasa
/// a la salida. Es un solo emisor (quien transmite), así que no hace falta
/// mezclar ni jitter buffer propio: la salida ya tiene su colchón.
#[cfg(feature = "voice-playback")]
struct StreamAudioPlayer {
    sink: VoiceDecodedAudioOutput,
    decoder: Option<::opus::Decoder>,
    /// SSRC de audio ya asociado a quien transmite en el estado DAVE.
    recorded_ssrc: Option<u32>,
    packets: u64,
    pending: u64,
    failures: u64,
}

#[cfg(feature = "voice-playback")]
impl StreamAudioPlayer {
    fn new(sink: VoiceDecodedAudioOutput) -> Self {
        Self {
            sink,
            decoder: None,
            recorded_ssrc: None,
            packets: 0,
            pending: 0,
            failures: 0,
        }
    }

    async fn push_packet(
        &mut self,
        decryptor: &VoiceRtpDecryptor,
        datagram: &[u8],
        header: &RtpHeader,
        dave_state: &Arc<Mutex<VoiceDaveState>>,
        owner_user_id: Id<UserMarker>,
    ) {
        self.packets = self.packets.saturating_add(1);
        if self.packets == 1 {
            logging::debug(
                "stream",
                format!("first stream audio packet: ssrc={} seq={}", header.ssrc, header.sequence),
            );
        }
        let payload = match decryptor.decrypt_packet_any(datagram, header) {
            Ok(payload) => payload,
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                if self.failures == 1 || self.failures.is_multiple_of(200) {
                    logging::debug(
                        "stream",
                        format!("stream audio RTP decrypt failed: count={} error={error}", self.failures),
                    );
                }
                return;
            }
        };
        let media = {
            let mut dave = dave_state.lock().await;
            // El único que transmite audio en este stream es su dueño: se
            // asocia su SSRC para que DAVE sepa con qué clave descifrar.
            if self.recorded_ssrc != Some(header.ssrc) {
                dave.record_ssrc_user(header.ssrc, owner_user_id);
                self.recorded_ssrc = Some(header.ssrc);
            }
            dave.unwrap_media_payload_for_ssrc(header.ssrc, &payload.media_payload)
        };
        let opus = match media {
            VoiceMediaPayload::Plain(opus) | VoiceMediaPayload::DaveDecrypted { opus, .. } => opus,
            VoiceMediaPayload::DaveDecryptFailed { message, .. } => {
                self.failures = self.failures.saturating_add(1);
                if self.failures == 1 || self.failures.is_multiple_of(200) {
                    logging::debug(
                        "stream",
                        format!("stream audio DAVE decrypt failed: count={} error={message}", self.failures),
                    );
                }
                return;
            }
            pending => {
                self.pending = self.pending.saturating_add(1);
                if self.pending == 1 || self.pending.is_multiple_of(200) {
                    logging::debug(
                        "stream",
                        format!(
                            "stream audio waiting for DAVE: count={} reason={}",
                            self.pending,
                            pending.pending_reason()
                        ),
                    );
                }
                return;
            }
        };
        self.decode(&opus);
    }

    fn decode(&mut self, opus: &[u8]) {
        if self.decoder.is_none() {
            match ::opus::Decoder::new(::opus::Channels::Stereo, ::opus::SampleRate::Hz48000) {
                Ok(decoder) => self.decoder = Some(decoder),
                Err(error) => {
                    logging::error(
                        "stream",
                        format!("stream Opus decoder init failed: {}", error.message()),
                    );
                    return;
                }
            }
        }
        let Some(decoder) = self.decoder.as_mut() else { return };
        let channels = usize::from(DISCORD_VOICE_CHANNELS);
        let mut decoded = vec![0.0f32; OPUS_MAX_FRAME_SAMPLES_PER_CHANNEL * channels];
        match decoder.decode_float_to_slice(opus, &mut decoded, false) {
            Ok(samples_per_channel) => {
                decoded.truncate(samples_per_channel * channels);
                self.sink.try_send(decoded);
            }
            Err(error) => {
                logging::debug("stream", format!("stream Opus decode failed: {}", error.message()));
            }
        }
    }
}

/// PLI (Picture Loss Indication, RFC 4585): "perdí el video, mandame una
/// keyframe". 12 bytes: cabecera RTCP, SSRC propio y SSRC del video.
fn build_pli_packet(sender_ssrc: u32, media_ssrc: u32) -> [u8; 12] {
    let mut packet = [0u8; 12];
    packet[0] = 0x81; // versión 2, sin padding, FMT = 1 (PLI)
    packet[1] = 206; // PT = PSFB (feedback específico del payload)
    packet[2..4].copy_from_slice(&2u16.to_be_bytes()); // largo en palabras - 1
    packet[4..8].copy_from_slice(&sender_ssrc.to_be_bytes());
    packet[8..12].copy_from_slice(&media_ssrc.to_be_bytes());
    packet
}

async fn send_keyframe_request(
    socket: &UdpSocket,
    encryptor: &VoiceRtpEncryptor,
    nonce: &mut u32,
    sender_ssrc: u32,
    media_ssrc: u32,
) -> Result<(), String> {
    if media_ssrc == 0 {
        return Err("no video SSRC known yet".to_owned());
    }
    let datagram = encryptor.encrypt_rtcp_feedback(
        &build_pli_packet(sender_ssrc, media_ssrc),
        nonce.to_be_bytes(),
    )?;
    *nonce = nonce.wrapping_add(1);
    socket
        .send(&datagram)
        .await
        .map_err(|error| format!("PLI send failed: {error}"))?;
    Ok(())
}

/// Hilo de decodificación: access units H.264 -> frames RGBA en el slot.
fn run_decode_thread(
    access_units: std::sync::mpsc::Receiver<Vec<u8>>,
    frames: StreamFrameSlot,
    repaint: Arc<dyn Fn() + Send + Sync>,
    events: std::sync::mpsc::Sender<AppEvent>,
    stream_key: String,
) {
    let mut decoder = match H264Decoder::new() {
        Ok(decoder) => decoder,
        Err(error) => {
            logging::error("stream", &error);
            publish_status(
                &events,
                &stream_key,
                StreamWatchStatus::Failed,
                Some(error),
            );
            return;
        }
    };
    let announced = AtomicBool::new(false);
    let mut decode_errors = 0u64;
    let mut frames_decoded = 0u64;
    let mut access_units_received = 0u64;

    while let Ok(access_unit) = access_units.recv() {
        access_units_received = access_units_received.saturating_add(1);
        match decoder.decode(&access_unit) {
            Ok(mut decoded) => {
                // Solo el último frame importa: la UI pinta el más reciente.
                let Some(latest) = decoded.pop() else {
                    continue;
                };
                frames_decoded = frames_decoded.saturating_add(1);
                if frames_decoded.is_multiple_of(120) {
                    logging::debug(
                        "stream",
                        format!(
                            "stream decoder progress: access_units={access_units_received} frames_decoded={frames_decoded} size={}x{}",
                            latest.width, latest.height
                        ),
                    );
                }
                frames.store(StreamFrame {
                    width: latest.width,
                    height: latest.height,
                    rgba: latest.rgba,
                });
                if !announced.swap(true, AtomicOrdering::Relaxed) {
                    logging::debug(
                        "stream",
                        format!("first stream frame decoded: {}x{}", latest.width, latest.height),
                    );
                    publish_status(&events, &stream_key, StreamWatchStatus::Receiving, None);
                }
                (*repaint)();
            }
            Err(error) => {
                decode_errors = decode_errors.saturating_add(1);
                if decode_errors == 1 || decode_errors.is_multiple_of(100) {
                    logging::debug(
                        "stream",
                        format!("stream decode error: count={decode_errors} error={error}"),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StreamWatchParams {
        StreamWatchParams {
            stream_key: "guild:1:2:3".to_owned(),
            rtc_server_id: "99".to_owned(),
            endpoint: "example.discord.media:443".to_owned(),
            token: "secret-token".to_owned(),
            user_id: Id::new(10),
            session_id: "session".to_owned(),
            owner_user_id: Id::new(3),
            output_source: None,
        }
    }

    #[test]
    fn identify_uses_the_rtc_server_id_and_asks_for_video() {
        let payload: Value = serde_json::from_str(&stream_identify_payload(&params())).unwrap();
        assert_eq!(payload["op"], 0);
        assert_eq!(payload["d"]["server_id"], "99");
        assert_eq!(payload["d"]["user_id"], "10");
        assert_eq!(payload["d"]["video"], true);
    }

    #[test]
    fn select_protocol_announces_an_h264_decoder() {
        let discovered = DiscoveredVoiceAddress {
            address: "1.2.3.4".to_owned(),
            port: 5000,
        };
        let payload: Value = serde_json::from_str(&stream_select_protocol_payload(
            &discovered,
            AEAD_AES256_GCM_RTPSIZE,
        ))
        .unwrap();
        let codecs = payload["d"]["codecs"].as_array().unwrap();
        let h264 = codecs.iter().find(|c| c["name"] == "H264").unwrap();
        assert_eq!(h264["payload_type"], STREAM_H264_PAYLOAD_TYPE);
        assert_eq!(h264["decode"], true);
        assert_eq!(h264["encode"], false);
        assert_eq!(payload["d"]["data"]["mode"], AEAD_AES256_GCM_RTPSIZE);
    }

    #[test]
    fn debug_output_redacts_the_token_and_session() {
        let debug = format!("{:?}", params());
        assert!(!debug.contains("secret-token"));
        assert!(!debug.contains("session\""));
    }

    #[test]
    fn pli_packet_has_the_rtcp_feedback_layout() {
        let packet = build_pli_packet(0x0102_0304, 0x0a0b_0c0d);
        assert_eq!(packet[0], 0x81);
        assert_eq!(packet[1], 206);
        assert_eq!(&packet[2..4], &[0, 2]);
        assert_eq!(&packet[4..8], &[1, 2, 3, 4]);
        assert_eq!(&packet[8..12], &[0x0a, 0x0b, 0x0c, 0x0d]);
        assert!(looks_like_rtcp_packet(&packet));
    }

    #[test]
    fn frame_slot_hands_out_each_frame_once() {
        let slot = StreamFrameSlot::default();
        assert!(slot.take_new().is_none());
        slot.store(StreamFrame {
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        });
        assert!(slot.take_new().is_some());
        assert!(slot.take_new().is_none());
    }
}
