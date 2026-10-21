//! Transporte WebRTC (DTLS-SRTP) para transmitir nuestra pantalla. Es el transporte
//! predeterminado. Para usar el camino UDP clásico: devtools (Transmisión → Transporte) o
//! `ECORD_STREAM_TRANSPORT=udp`.
//!
//! Replica lo que hace el módulo de Node `Discord-video-stream` (libdatachannel):
//!
//! * `SELECT_PROTOCOL` con `protocol: "webrtc"`, un `sdp` local y un
//!   `rtc_connection_id`. No hay descubrimiento de IP por UDP ni modo de cifrado
//!   AEAD: el cifrado lo pone DTLS-SRTP.
//! * De la respuesta de Discord (`sdp` en SESSION_DESCRIPTION) solo se usan las
//!   credenciales ICE, el fingerprint DTLS y la dirección del servidor.
//! * Los SSRC de audio, video y RTX son los que asigna Discord en READY.
//! * Playout delay (id 5) en los paquetes de video con el valor elegido en las devtools o
//!   en `ECORD_PLAYOUT_DELAY` (el mismo que el camino UDP; sin elegir, no se manda).
//! * Timestamps RTP regulares: salen del número de frame del codificador
//!   (`EncodedFrame::pts`), no de la hora a la que el codificador entregó el paquete.
//! * Sender Reports y reenvíos por NACK (con RTX) los hace la biblioteca.
//!
//! La biblioteca es `str0m` 0.24 (sans-IO) en modo RTP: este módulo le pasa los
//! datagramas UDP y los paquetes RTP ya armados. Se usa su API directa (`direct_api`)
//! porque deja fijar los SSRC y las credenciales ICE/DTLS sin negociar un SDP completo.
//!
//! Escrito contra la documentación de str0m 0.24.1, pero sin poder compilarlo.

use super::*;
use std::net::{IpAddr, SocketAddr};

/// Playout delay en unidades de 10 ms `(mínimo, máximo)` para los paquetes de video.
/// Es el mismo valor que usa el camino UDP: devtools, luego `ECORD_PLAYOUT_DELAY`, luego
/// `PlayoutDelay::DEFAULT`. (Antes, sin nada elegido, aquí se mandaba siempre 0-100 ms, el
/// valor fijo del módulo de Node, y no el que se había configurado.) `Off` y `Legacy` no
/// tienen representación en str0m: sin extensión.
fn playout_units() -> Option<(u32, u32)> {
    match playout_delay() {
        PlayoutDelay::Range { min_ms, max_ms } => {
            let min = (min_ms / 10).min(0xFFF);
            Some((min, (max_ms / 10).min(0xFFF).max(min)))
        }
        PlayoutDelay::Off | PlayoutDelay::Legacy => None,
    }
}

// ---------------------------------------------------------------------------
// SDP
// ---------------------------------------------------------------------------

/// Credenciales locales (ICE + DTLS) que van en el SDP que se le manda a Discord.
#[derive(Clone, Debug)]
pub(super) struct LocalCreds {
    pub(super) ufrag: String,
    pub(super) pwd: String,
    /// `sha-256 AA:BB:...` (el `Display` del fingerprint local de str0m).
    pub(super) fingerprint: String,
}

/// Lo que se usa del SDP de Discord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteSdp {
    pub(super) ufrag: String,
    pub(super) pwd: String,
    /// `sha-256 AA:BB:...`, tal cual lo parsea `str0m::config::Fingerprint`.
    pub(super) fingerprint: String,
    pub(super) address: SocketAddr,
}

fn is_valid_fingerprint_hex(text: &str) -> bool {
    !text.is_empty() && text.split(':').all(|pair| u8::from_str_radix(pair.trim(), 16).is_ok())
}

/// El SDP de oferta que se le manda a Discord. Se arma a mano (en vez de pedírselo
/// a la biblioteca) porque Discord solo mira las credenciales, los codecs y los SSRC.
/// Orden de m-lines: audio (mid 0) y video (mid 1), igual que el módulo de Node.
pub(super) fn build_offer_sdp(creds: &LocalCreds, audio_ssrc: u32, video_ssrc: u32, rtx_ssrc: u32) -> String {
    let session_id = u64::from(rand::random::<u32>()) + 1;
    let transport = |lines: &mut Vec<String>, mid: &str| {
        lines.push("c=IN IP4 0.0.0.0".to_owned());
        lines.push("a=rtcp:9 IN IP4 0.0.0.0".to_owned());
        lines.push(format!("a=ice-ufrag:{}", creds.ufrag));
        lines.push(format!("a=ice-pwd:{}", creds.pwd));
        lines.push(format!("a=fingerprint:{}", creds.fingerprint));
        lines.push("a=setup:actpass".to_owned());
        lines.push(format!("a=mid:{mid}"));
        lines.push("a=sendonly".to_owned());
        lines.push("a=rtcp-mux".to_owned());
    };
    let mut lines: Vec<String> = vec![
        "v=0".to_owned(),
        format!("o=- {session_id} 2 IN IP4 127.0.0.1"),
        "s=-".to_owned(),
        "t=0 0".to_owned(),
        "a=group:BUNDLE 0 1".to_owned(),
        "a=msid-semantic: WMS *".to_owned(),
        format!("m=audio 9 UDP/TLS/RTP/SAVPF {PUBLISH_OPUS_PAYLOAD_TYPE}"),
    ];
    transport(&mut lines, "0");
    lines.push(format!("a=rtpmap:{PUBLISH_OPUS_PAYLOAD_TYPE} opus/48000/2"));
    lines.push(format!("a=fmtp:{PUBLISH_OPUS_PAYLOAD_TYPE} minptime=10;useinbandfec=1"));
    lines.push(format!("a=ssrc:{audio_ssrc} cname:ecord"));
    lines.push(format!(
        "m=video 9 UDP/TLS/RTP/SAVPF {PUBLISH_H264_PAYLOAD_TYPE} {PUBLISH_H264_RTX_PAYLOAD_TYPE}"
    ));
    transport(&mut lines, "1");
    // Se declara la extensión de playout delay (id 5), como en el SDP de Discord: sin esto
    // el servidor puede no tratarla como negociada y no reenviarla a los visores.
    lines.push("a=extmap:5 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay".to_owned());
    lines.push(format!("a=rtpmap:{PUBLISH_H264_PAYLOAD_TYPE} H264/90000"));
    lines.push(format!("a=rtcp-fb:{PUBLISH_H264_PAYLOAD_TYPE} nack"));
    lines.push(format!("a=rtcp-fb:{PUBLISH_H264_PAYLOAD_TYPE} nack pli"));
    lines.push(format!("a=rtcp-fb:{PUBLISH_H264_PAYLOAD_TYPE} ccm fir"));
    lines.push(format!(
        "a=fmtp:{PUBLISH_H264_PAYLOAD_TYPE} level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
    ));
    lines.push(format!("a=rtpmap:{PUBLISH_H264_RTX_PAYLOAD_TYPE} rtx/90000"));
    lines.push(format!("a=fmtp:{PUBLISH_H264_RTX_PAYLOAD_TYPE} apt={PUBLISH_H264_PAYLOAD_TYPE}"));
    lines.push(format!("a=ssrc-group:FID {video_ssrc} {rtx_ssrc}"));
    lines.push(format!("a=ssrc:{video_ssrc} cname:ecord"));
    lines.push(format!("a=ssrc:{rtx_ssrc} cname:ecord"));
    let mut sdp = lines.join("\r\n");
    sdp.push_str("\r\n");
    sdp
}

/// Saca del SDP de Discord lo único que hace falta. Igual que el módulo de Node
/// (que lee `c=`, `a=rtcp`, `a=ice-ufrag`, `a=ice-pwd`, `a=fingerprint` y
/// `a=candidate`): se prefiere la dirección del candidato, y si no hay, `c=` + `a=rtcp`.
pub(super) fn parse_remote_sdp(sdp: &str) -> Result<RemoteSdp, String> {
    let (mut ufrag, mut pwd, mut fingerprint) = (None, None, None);
    let (mut candidate, mut connection_ip, mut rtcp_port) = (None, None, None);
    for line in sdp.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("a=ice-ufrag:") {
            ufrag.get_or_insert_with(|| value.to_owned());
        } else if let Some(value) = line.strip_prefix("a=ice-pwd:") {
            pwd.get_or_insert_with(|| value.to_owned());
        } else if let Some(value) = line.strip_prefix("a=fingerprint:") {
            if let Some((hash, hex)) = value.split_once(' ') {
                fingerprint.get_or_insert_with(|| (hash.to_ascii_lowercase(), hex.trim().to_owned()));
            }
        } else if let Some(value) = line.strip_prefix("a=candidate:") {
            // foundation component protocol priority ip port typ ...
            let fields: Vec<&str> = value.split_whitespace().collect();
            if candidate.is_none()
                && let (Some(ip), Some(port)) = (fields.get(4), fields.get(5))
                && let (Ok(ip), Ok(port)) = (ip.parse::<IpAddr>(), port.parse::<u16>())
            {
                candidate = Some(SocketAddr::new(ip, port));
            }
        } else if let Some(value) = line.strip_prefix("c=IN IP4 ") {
            connection_ip.get_or_insert_with(|| value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("a=rtcp:") {
            if let Some(port) = value.split_whitespace().next().and_then(|port| port.parse::<u16>().ok()) {
                rtcp_port.get_or_insert(port);
            }
        }
    }
    let address = candidate
        .or_else(|| {
            let ip = connection_ip?.parse::<IpAddr>().ok()?;
            Some(SocketAddr::new(ip, rtcp_port?))
        })
        .ok_or_else(|| "webrtc: el SDP de Discord no trae dirección del servidor".to_owned())?;
    let (fingerprint_hash, fingerprint_hex) =
        fingerprint.ok_or_else(|| "webrtc: el SDP de Discord no trae fingerprint".to_owned())?;
    if !is_valid_fingerprint_hex(&fingerprint_hex) {
        return Err("webrtc: fingerprint ilegible en el SDP de Discord".to_owned());
    }
    Ok(RemoteSdp {
        ufrag: ufrag.ok_or_else(|| "webrtc: el SDP de Discord no trae ice-ufrag".to_owned())?,
        pwd: pwd.ok_or_else(|| "webrtc: el SDP de Discord no trae ice-pwd".to_owned())?,
        fingerprint: format!("{fingerprint_hash} {fingerprint_hex}"),
        address,
    })
}

/// SELECT_PROTOCOL (opcode 1) en modo WebRTC. Mismos campos que el módulo de Node.
pub(super) fn select_protocol_payload(sdp: &str, rtc_connection_id: &str) -> String {
    json!({
        "op": 1,
        "d": {
            "protocol": "webrtc",
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
            "data": sdp,
            "sdp": sdp,
            "rtc_connection_id": rtc_connection_id,
        },
    })
    .to_string()
}

/// SESSION_DESCRIPTION en modo WebRTC: trae `sdp` en vez de `mode` y `secret_key`.
pub(super) struct WebRtcSessionDescription {
    pub(super) sdp: String,
    pub(super) dave_protocol_version: Option<u64>,
}

pub(super) fn parse_session_description(value: &Value) -> Result<WebRtcSessionDescription, String> {
    let data = value
        .get("d")
        .ok_or_else(|| "webrtc: la session description no trae datos".to_owned())?;
    let sdp = data
        .get("sdp")
        .and_then(Value::as_str)
        .filter(|sdp| !sdp.is_empty())
        .ok_or_else(|| "webrtc: la session description no trae sdp".to_owned())?
        .to_owned();
    Ok(WebRtcSessionDescription {
        sdp,
        dave_protocol_version: data.get("dave_protocol_version").and_then(Value::as_u64),
    })
}

// ---------------------------------------------------------------------------
// Reloj de frames
// ---------------------------------------------------------------------------

/// Convierte el número de frame del codificador en timestamp RTP (90 kHz),
/// como hace el módulo de Node (suma `frametime` por frame). Si el codificador se
/// reinicia, su pts vuelve a 0: se reengancha sin que el timestamp retroceda.
struct FrameClock {
    fps: u64,
    first_pts: Option<i64>,
    last_pts: i64,
    offset: u64,
    last_ticks: u64,
    fallback_frames: u64,
}

impl FrameClock {
    fn new(fps: u32) -> Self {
        Self {
            fps: u64::from(fps.max(1)),
            first_pts: None,
            last_pts: 0,
            offset: 0,
            last_ticks: 0,
            fallback_frames: 0,
        }
    }

    /// Timestamp (sin el offset aleatorio inicial) del frame con ese pts.
    fn ticks(&mut self, pts: Option<i64>) -> u64 {
        let frame_ticks = RTP_VIDEO_CLOCK_HZ / self.fps;
        let ticks = match pts {
            Some(pts) => {
                if self.first_pts.is_none_or(|_| pts < self.last_pts) {
                    self.offset = if self.first_pts.is_some() { self.last_ticks + frame_ticks } else { 0 };
                    self.first_pts = Some(pts);
                }
                self.last_pts = pts;
                let first = self.first_pts.unwrap_or(pts);
                self.offset + (pts - first).max(0) as u64 * RTP_VIDEO_CLOCK_HZ / self.fps
            }
            None => {
                let ticks = self.last_ticks.max(self.fallback_frames * frame_ticks);
                self.fallback_frames += 1;
                ticks
            }
        };
        self.last_ticks = ticks;
        ticks
    }
}

// ---------------------------------------------------------------------------
// str0m (0.24)
// ---------------------------------------------------------------------------

// Modo RTP de str0m (`set_rtp_mode(true)`): no empaqueta ni desempaqueta, solo cifra
// (SRTP), reenvía por NACK (RTX), manda Sender Reports y maneja ICE/DTLS. Ecord ya
// trae su propio paquetizador H.264, así que le pasa paquetes RTP hechos.
use str0m::config::Fingerprint;
use str0m::format::{Codec, FormatParams};
use str0m::ice::IceCreds;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{Extension, ExtensionValues, RtpWrite, Ssrc};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

// ---------------------------------------------------------------------------
// Pacer (como el `PacingHandler` de libdatachannel que usa el módulo de Node)
// ---------------------------------------------------------------------------
//
// El emisor deja todos los paquetes de un frame en una cola y la tarea del transporte los
// suelta cada `PACER_INTERVAL` con un presupuesto de bytes (`velocidad × tiempo`). La
// velocidad la fija el emisor antes de cada frame: `1,25 ×` el bitrate medido en 250 ms,
// con un piso. Así una IDR se reparte a esa velocidad en vez de salir de golpe, y el
// emisor no se queda dormido esperando entre paquetes.

/// Cada cuánto se vacía la cola (en Node: `new PacingHandler(10 Mbps, 2)`).
const PACER_INTERVAL: Duration = Duration::from_millis(2);
/// Velocidad inicial del pacer: 10 Mbps, como en Node.
const PACER_INITIAL_BYTES_PER_SEC: u64 = 10 * 1000 * 1000 / 8;
/// Lo máximo de presupuesto que se acumula. libdatachannel lo limita a un intervalo
/// (2 ms), pero en Windows un sleep de 2 ms puede durar 15 ms y con ese tope se perdería
/// casi todo el presupuesto de cada tick: se deja margen para recuperar el atraso.
const PACER_BUDGET_CAP: Duration = Duration::from_millis(20);
/// Si la cola llega a esto (~1,2 MB) la red no da abasto: se vacía y se pide una IDR en
/// vez de acumular segundos de retraso.
const PACER_MAX_QUEUE: usize = 1000;
/// Margen sobre el bitrate medido (en %), como el `* 1.25` de Node.
const PACER_HEADROOM_PERCENT: u64 = 125;
/// Piso de la velocidad como porcentaje del bitrate configurado. Node usa solo 1 Mbps;
/// con el bitrate configurado como piso, una IDR de ~33 KB sale en ~100 ms (a 2500 kbps).
pub(super) const PACER_FLOOR_PERCENT: u64 = 100;

/// Un paquete RTP de video para la tarea del transporte.
struct RtpOut {
    /// Número de secuencia extendido (str0m se queda con los 16 bits bajos).
    sequence: u64,
    timestamp: u32,
    marker: bool,
    payload: Vec<u8>,
    /// Playout delay en unidades de 10 ms.
    playout: Option<(u32, u32)>,
    wallclock: Instant,
}

/// Un paquete RTP de audio (Opus, 20 ms) para la tarea del transporte. El audio no pasa
/// por el pacer: son ~160 bytes cada 20 ms.
struct AudioOut {
    sequence: u64,
    timestamp: u32,
    payload: Vec<u8>,
}

/// Lo que ya está listo antes de saber a dónde conectar: credenciales locales
/// para armar el SDP de `SELECT_PROTOCOL`.
pub(super) struct PreparedTransport {
    rtc: Rtc,
    pt: Pt,
    audio_pt: Pt,
    audio_ssrc: u32,
    video_ssrc: u32,
    rtx_ssrc: u32,
    creds: LocalCreds,
    keyframe: KeyframeRequest,
}

impl PreparedTransport {
    pub(super) fn new(audio_ssrc: u32, video_ssrc: u32, rtx_ssrc: u32, keyframe: KeyframeRequest) -> Result<Self, String> {
        let ice_creds = IceCreds::new();
        // Si `build` se queja de que no hay proveedor criptográfico, instalar el de los
        // features (`str0m::crypto::from_feature_flags()`) antes de construir.
        let mut config = RtcConfig::new()
            .set_rtp_mode(true)
            // Un IDR a 60 fps son cientos de paquetes de golpe; el buffer por defecto (1000)
            // también sirve de historial para los reenvíos, así que se agranda.
            .set_send_buffer_video(4096)
            .set_local_ice_credentials(ice_creds.clone())
            .clear_codecs()
            // Mismo id que usa Discord para el playout delay.
            .set_extension(5, Extension::PlayoutDelay);
        config.codec_config().add_h264(
            Pt::new_with_value(PUBLISH_H264_PAYLOAD_TYPE),
            Some(Pt::new_with_value(PUBLISH_H264_RTX_PAYLOAD_TYPE)),
            true,
            0x0042_E01F,
        );
        // Opus estéreo a 48 kHz con el mismo payload type que anuncia el SDP.
        config.codec_config().add_config(
            Pt::new_with_value(PUBLISH_OPUS_PAYLOAD_TYPE),
            None,
            Codec::Opus,
            Frequency::FORTY_EIGHT_KHZ,
            Some(2),
            FormatParams { min_p_time: Some(10), use_inband_fec: Some(true), ..Default::default() },
        );
        let mut rtc = config.build(Instant::now());

        let fingerprint = rtc.direct_api().local_dtls_fingerprint().to_string();
        let pt = rtc
            .codec_config()
            .find(|params| params.spec().codec == Codec::H264)
            .map(|params| params.pt())
            .ok_or_else(|| "webrtc: str0m no tiene H264 configurado".to_owned())?;
        let audio_pt = rtc
            .codec_config()
            .find(|params| params.spec().codec == Codec::Opus)
            .map(|params| params.pt())
            .ok_or_else(|| "webrtc: str0m no tiene Opus configurado".to_owned())?;

        // Mid 1 = video (el 0 es el audio, que no se transmite todavía).
        let mid: Mid = "1".into();
        let audio_mid: Mid = "0".into();
        {
            let mut api = rtc.direct_api();
            // Audio (mid 0): sin RTX, el audio no se reenvía.
            api.declare_media(audio_mid, MediaKind::Audio);
            api.declare_stream_tx(Ssrc::from(audio_ssrc), None, audio_mid, None);
            api.declare_media(mid, MediaKind::Video);
            api.declare_stream_tx(Ssrc::from(video_ssrc), Some(Ssrc::from(rtx_ssrc)), mid, None);
            // Por defecto str0m usa 1024 paquetes / 3 s y un tope de RTX del 15 % (si los
            // reenvíos pasan de esa proporción respecto al tráfico normal, VACÍA la cola de
            // reenvíos). A 30/60 fps con ráfagas grandes de NACK se pasa del tope y los
            // reenvíos se descartan; a 15 fps no. Sin tope: Discord ya limita con sus NACK.
            if let Some(stream) = api.stream_tx(&Ssrc::from(video_ssrc)) {
                stream.set_rtx_cache(4096, Duration::from_secs(3), None);
            }
        }
        Ok(Self {
            rtc,
            pt,
            audio_pt,
            audio_ssrc,
            video_ssrc,
            rtx_ssrc,
            creds: LocalCreds { ufrag: ice_creds.ufrag, pwd: ice_creds.pass, fingerprint },
            keyframe,
        })
    }

    pub(super) fn creds(&self) -> &LocalCreds {
        &self.creds
    }

    /// Con la respuesta de Discord: abre el socket, configura ICE/DTLS y arranca la
    /// tarea que mueve los datagramas.
    pub(super) async fn start(mut self, remote: RemoteSdp, tasks: &mut PublishTasks) -> Result<WebRtcHandle, String> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|error| format!("webrtc: no se pudo abrir el socket UDP: {error}"))?;
        // Se fija el destino: así `local_addr` es la IP con la que de verdad se llega al
        // servidor (la usa el candidato local) y `send`/`recv` solo hablan con él.
        socket
            .connect(remote.address)
            .await
            .map_err(|error| format!("webrtc: no se pudo fijar el destino UDP: {error}"))?;
        let local_addr = socket
            .local_addr()
            .map_err(|error| format!("webrtc: sin dirección local: {error}"))?;
        let _ = socket2::SockRef::from(&socket).set_send_buffer_size(2 * 1024 * 1024);

        let local_candidate = Candidate::host(local_addr, "udp")
            .map_err(|error| format!("webrtc: candidato local inválido: {error:?}"))?;
        let remote_candidate = Candidate::host(remote.address, "udp")
            .map_err(|error| format!("webrtc: candidato remoto inválido: {error:?}"))?;
        self.rtc.add_local_candidate(local_candidate);
        self.rtc.add_remote_candidate(remote_candidate);

        {
            let fingerprint: Fingerprint = remote
                .fingerprint
                .parse()
                .map_err(|error| format!("webrtc: fingerprint de Discord inválido: {error:?}"))?;
            let mut api = self.rtc.direct_api();
            // El servidor de Discord es ICE-lite: controlamos nosotros. Él responde con
            // `a=setup:passive`, así que el cliente DTLS somos nosotros.
            api.set_ice_lite(false);
            api.set_ice_controlling(true);
            api.set_remote_ice_credentials(IceCreds { ufrag: remote.ufrag.clone(), pass: remote.pwd.clone() });
            api.set_remote_fingerprint(fingerprint);
            api.start_dtls(true)
                .map_err(|error| format!("webrtc: no arrancó DTLS: {error:?}"))?;
        }
        self.rtc
            .handle_input(Input::Timeout(Instant::now()))
            .map_err(|error| format!("webrtc: error al arrancar: {error:?}"))?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<RtpOut>();
        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<AudioOut>();
        let pacer_rate = Arc::new(std::sync::atomic::AtomicU64::new(PACER_INITIAL_BYTES_PER_SEC));
        let (connected_tx, connected_rx) = watch::channel(false);
        logging::debug(
            "stream",
            format!(
                "webrtc: conectando a {} (video_ssrc={} rtx_ssrc={})",
                remote.address, self.video_ssrc, self.rtx_ssrc
            ),
        );
        tasks.push(tokio::spawn(run_transport(
            self.rtc,
            socket,
            local_addr,
            remote.address,
            cmd_rx,
            audio_rx,
            connected_tx,
            self.keyframe,
            Ssrc::from(self.video_ssrc),
            self.pt,
            Ssrc::from(self.audio_ssrc),
            self.audio_pt,
            Arc::clone(&pacer_rate),
        )));
        Ok(WebRtcHandle { cmd_tx, audio_tx, connected_rx, pacer_rate })
    }
}

/// Lo que usa el emisor de video para hablar con la tarea del transporte.
#[derive(Clone)]
pub(super) struct WebRtcHandle {
    cmd_tx: mpsc::UnboundedSender<RtpOut>,
    audio_tx: mpsc::UnboundedSender<AudioOut>,
    connected_rx: watch::Receiver<bool>,
    /// Velocidad del pacer en bytes/s (la fija el emisor antes de cada frame).
    pacer_rate: Arc<std::sync::atomic::AtomicU64>,
}

impl WebRtcHandle {
    /// Espera a que ICE y DTLS terminen (como el `ready` del módulo de Node).
    async fn wait_connected(&mut self) -> Result<(), String> {
        timeout(Duration::from_secs(20), async {
            while !*self.connected_rx.borrow() {
                self.connected_rx
                    .changed()
                    .await
                    .map_err(|_| "webrtc: el transporte terminó antes de conectar".to_owned())?;
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|_| "webrtc: no se conectó en 20 s (ICE/DTLS)".to_owned())?
    }

    fn send_audio(&self, packet: AudioOut) -> Result<(), String> {
        self.audio_tx
            .send(packet)
            .map_err(|_| "webrtc: el transporte terminó".to_owned())
    }

    fn set_pacer_rate(&self, bytes_per_sec: u64) {
        self.pacer_rate.store(bytes_per_sec, std::sync::atomic::Ordering::Relaxed);
    }

    fn send(&self, packet: RtpOut) -> Result<(), String> {
        self.cmd_tx
            .send(packet)
            .map_err(|_| "webrtc: el transporte se cerró".to_owned())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_transport(
    mut rtc: Rtc,
    socket: UdpSocket,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    mut cmd_rx: mpsc::UnboundedReceiver<RtpOut>,
    mut audio_rx: mpsc::UnboundedReceiver<AudioOut>,
    connected_tx: watch::Sender<bool>,
    keyframe: KeyframeRequest,
    video_ssrc: Ssrc,
    pt: Pt,
    audio_ssrc: Ssrc,
    audio_pt: Pt,
    pacer_rate: Arc<std::sync::atomic::AtomicU64>,
) {
    let mut buffer = vec![0u8; 2048];
    let mut queue: std::collections::VecDeque<RtpOut> = std::collections::VecDeque::new();
    let mut budget: f64 = 0.0;
    let mut last_run = Instant::now();
    let mut queue_max: usize = 0;
    let mut overflows: u64 = 0;
    let mut last_log = Instant::now();
    let mut wire = WireStats::default();
    let mut audio_open = true;
    loop {
        // 1) Vaciar todo lo que la biblioteca quiere hacer ahora.
        let deadline = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(at)) => break at,
                Ok(Output::Transmit(transmit)) => {
                    let data = transmit.contents.to_vec();
                    wire.count_out(&data);
                    if let Err(error) = socket.send(&data).await {
                        logging::debug("stream", format!("webrtc: UDP send falló: {error}"));
                    }
                }
                Ok(Output::Event(event)) => match event {
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        logging::debug("stream", "webrtc: ICE desconectado");
                        let _ = connected_tx.send(false);
                        return;
                    }
                    Event::IceConnectionStateChange(state) => {
                        logging::debug("stream", format!("webrtc: ICE {state:?}"));
                    }
                    // Un espectador (o el servidor) pide una IDR (PLI / FIR).
                    Event::KeyframeRequest(_) => keyframe.request(),
                    // Los NACK no llegan como evento: str0m los atiende solo (RTX) porque el
                    // stream se declaró con `rtx_ssrc` y el media es `nackable(true)`.
                    _ => {}
                },
                Err(error) => {
                    logging::debug("stream", format!("webrtc: error del transporte: {error:?}"));
                    return;
                }
            }
        };
        let connected = rtc.is_connected();
        connected_tx.send_if_modified(|value| {
            if *value == connected {
                false
            } else {
                logging::debug("stream", format!("webrtc: conectado={connected}"));
                *value = connected;
                true
            }
        });

        let wait = deadline.saturating_duration_since(Instant::now());
        // Absoluta (no un sleep nuevo en cada vuelta): los datagramas que llegan no deben
        // retrasar el siguiente tick del pacer.
        let pacer_wait = (last_run + PACER_INTERVAL).saturating_duration_since(Instant::now());
        tokio::select! {
            () = tokio::time::sleep(pacer_wait), if !queue.is_empty() => {
                run_pacer(&mut rtc, &mut queue, &mut budget, &mut last_run, &pacer_rate, video_ssrc, pt);
            }
            () = tokio::time::sleep(wait) => {
                if let Err(error) = rtc.handle_input(Input::Timeout(Instant::now())) {
                    logging::debug("stream", format!("webrtc: error en el timeout: {error:?}"));
                    return;
                }
            }
            received = socket.recv(&mut buffer) => {
                let Ok(length) = received else { continue };
                wire.count_in(&buffer[..length]);
                let Ok(contents) = buffer[..length].try_into() else { continue };
                let receive = Receive {
                    proto: Protocol::Udp,
                    source: remote_addr,
                    destination: local_addr,
                    contents,
                };
                if let Err(error) = rtc.handle_input(Input::Receive(Instant::now(), receive)) {
                    logging::debug("stream", format!("webrtc: error al procesar un datagrama: {error:?}"));
                    return;
                }
            }
            audio = audio_rx.recv(), if audio_open => {
                match audio {
                    Some(packet) => write_audio(&mut rtc, audio_ssrc, audio_pt, packet),
                    None => audio_open = false,
                }
            }
            command = cmd_rx.recv() => {
                let Some(packet) = command else { return };
                queue.push_back(packet);
                if queue.len() > PACER_MAX_QUEUE {
                    overflows = overflows.saturating_add(1);
                    logging::debug(
                        "stream",
                        format!("webrtc: la cola del pacer llegó a {} paquetes: se vacía y se pide una IDR", queue.len()),
                    );
                    queue.clear();
                    budget = 0.0;
                    keyframe.request();
                } else {
                    queue_max = queue_max.max(queue.len());
                    // Como `schedule(0)` de libdatachannel: se intenta enviar ya.
                    run_pacer(&mut rtc, &mut queue, &mut budget, &mut last_run, &pacer_rate, video_ssrc, pt);
                }
            }
        }
        if last_log.elapsed() >= Duration::from_secs(5) {
            last_log = Instant::now();
            logging::debug(
                "stream",
                format!(
                    "webrtc pacer: cola máx={queue_max} paquetes, vaciados={overflows}, velocidad={:.2} Mbps",
                    pacer_rate.load(std::sync::atomic::Ordering::Relaxed) as f64 * 8.0 / 1_000_000.0
                ),
            );
            queue_max = queue.len();
            logging::debug(
                "stream",
                format!(
                    "webrtc wire (5 s): video={} ({} KB) rtx={} ({} KB) rtcp_out={} | rtcp_in={} (sr={} rr={} nack={} twcc={} pli={} fir={} otros={})",
                    wire.video_out,
                    wire.video_out_bytes / 1024,
                    wire.rtx_out,
                    wire.rtx_out_bytes / 1024,
                    wire.rtcp_out,
                    wire.rtcp_in,
                    wire.sr_in,
                    wire.rr_in,
                    wire.nack_in,
                    wire.twcc_in,
                    wire.pli_in,
                    wire.fir_in,
                    wire.other_in,
                ),
            );
            wire = WireStats::default();
        }
    }
}

/// Cuenta lo que sale y entra por el socket mirando solo las cabeceras que van en claro
/// (no depende de la API de estadísticas de str0m). Sirve para saber si se reenvía por RTX
/// cuando el visor pide NACK.
#[derive(Default)]
struct WireStats {
    video_out: u64,
    video_out_bytes: u64,
    rtx_out: u64,
    rtx_out_bytes: u64,
    rtcp_out: u64,
    rtcp_in: u64,
    rr_in: u64,
    sr_in: u64,
    nack_in: u64,
    twcc_in: u64,
    pli_in: u64,
    fir_in: u64,
    other_in: u64,
}

impl WireStats {
    fn count_out(&mut self, data: &[u8]) {
        // Solo RTP/RTCP (versión 2); se ignoran STUN y DTLS.
        if data.len() < 12 || data[0] >> 6 != 2 {
            return;
        }
        if (192..=223).contains(&data[1]) {
            self.rtcp_out += 1;
            return;
        }
        match data[1] & 0x7f {
            PUBLISH_H264_RTX_PAYLOAD_TYPE => {
                self.rtx_out += 1;
                self.rtx_out_bytes += data.len() as u64;
            }
            PUBLISH_H264_PAYLOAD_TYPE => {
                self.video_out += 1;
                self.video_out_bytes += data.len() as u64;
            }
            _ => {}
        }
    }

    /// En SRTCP solo los 8 primeros bytes van en claro: se ve el tipo del primer paquete
    /// de cada datagrama (un NACK detrás de un RR no se vería).
    fn count_in(&mut self, data: &[u8]) {
        // Discord manda RTCP compuesto: `[RR, SDES, NACK, NACK, …]`. Antes
        // se miraba sólo el primer paquete; los NACKs van detrás del RR,
        // así que `nack_in` siempre quedaba en 0 aunque el receptor emitiera
        // miles. Recorremos todo el compuesto.
        let mut rest = data;
        while rest.len() >= 4 {
            if rest[0] >> 6 != 2 || !(192..=223).contains(&rest[1]) {
                break;
            }
            let length = (usize::from(u16::from_be_bytes([rest[2], rest[3]])) + 1) * 4;
            if length > rest.len() {
                break;
            }
            self.rtcp_in += 1;
            let fmt = rest[0] & 0x1f;
            match (rest[1], fmt) {
                (200, _) => self.sr_in += 1,
                (201, _) => self.rr_in += 1,
                (205, 1) => self.nack_in += 1,
                (205, 15) => self.twcc_in += 1,
                (206, 1) => self.pli_in += 1,
                (206, 4) => self.fir_in += 1,
                _ => self.other_in += 1,
            }
            rest = &rest[length..];
        }
    }
}

/// Un tick del pacer: suma presupuesto según el tiempo pasado y manda paquetes mientras
/// quede (se permite pasarse con el último, como libdatachannel).
fn run_pacer(
    rtc: &mut Rtc,
    queue: &mut std::collections::VecDeque<RtpOut>,
    budget: &mut f64,
    last_run: &mut Instant,
    pacer_rate: &std::sync::atomic::AtomicU64,
    ssrc: Ssrc,
    pt: Pt,
) {
    let now = Instant::now();
    let rate = pacer_rate.load(std::sync::atomic::Ordering::Relaxed) as f64;
    let elapsed = now.saturating_duration_since(*last_run).as_secs_f64();
    *budget = (*budget + elapsed * rate).min(PACER_BUDGET_CAP.as_secs_f64() * rate);
    *last_run = now;
    while *budget > 0.0 {
        let Some(packet) = queue.pop_front() else { break };
        *budget -= (packet.payload.len() + 12) as f64;
        write_rtp(rtc, ssrc, pt, packet);
    }
}

fn write_audio(rtc: &mut Rtc, ssrc: Ssrc, pt: Pt, packet: AudioOut) {
    let mut api = rtc.direct_api();
    let Some(stream) = api.stream_tx(&ssrc) else {
        return;
    };
    let write = RtpWrite::new(pt, packet.sequence.into(), packet.timestamp, Instant::now(), packet.payload);
    let _ = stream.write_rtp(write);
}

fn write_rtp(rtc: &mut Rtc, ssrc: Ssrc, pt: Pt, packet: RtpOut) {
    let mut extensions = ExtensionValues::default();
    if let Some((min, max)) = packet.playout {
        // El playout delay va en bloques de 10 ms; MediaTime es tiempo real.
        extensions.play_delay_min = Some(MediaTime::from_millis(u64::from(min) * 10));
        extensions.play_delay_max = Some(MediaTime::from_millis(u64::from(max) * 10));
    }
    let mut api = rtc.direct_api();
    let Some(stream) = api.stream_tx(&ssrc) else {
        return;
    };
    // Reenviable: str0m responde a los NACK (por RTX) con su historial.
    let write = RtpWrite::new(pt, packet.sequence.into(), packet.timestamp, packet.wallclock, packet.payload)
        .marker(packet.marker)
        .ext_vals(extensions)
        .nackable(true);
    let _ = stream.write_rtp(write);
}

// ---------------------------------------------------------------------------
// Emisor de video
// ---------------------------------------------------------------------------

pub(super) struct WebRtcVideoSender {
    pub(super) transport: WebRtcHandle,
    pub(super) dave_state: Arc<Mutex<VoiceDaveState>>,
    pub(super) frames_rx: mpsc::Receiver<EncodedFrame>,
    pub(super) fatal_tx: mpsc::UnboundedSender<String>,
    pub(super) events: std::sync::mpsc::Sender<AppEvent>,
    pub(super) stream_key: String,
    pub(super) pacing_floor_bytes_per_sec: u64,
    #[allow(dead_code)]
    pub(super) fps: u32,
    pub(super) keyframe: KeyframeRequest,
}

pub(super) async fn run_video_send(mut sender: WebRtcVideoSender) {
    let message = match send_frames(&mut sender).await {
        Ok(()) => "el codificador de video terminó".to_owned(),
        Err(error) => error,
    };
    let _ = sender.fatal_tx.send(message);
}

/// Lo que necesita el emisor de audio de la transmisión.
pub(super) struct WebRtcAudioSender {
    pub(super) transport: WebRtcHandle,
    pub(super) dave_state: Arc<Mutex<VoiceDaveState>>,
    pub(super) target: super::super::screen_capture::CaptureTarget,
}

/// El audio es opcional: si no se puede capturar o codificar, la transmisión sigue solo
/// con video (no se avisa como error fatal).
pub(super) async fn run_audio_send(mut sender: WebRtcAudioSender) {
    if let Err(error) = send_audio(&mut sender).await {
        logging::debug("stream", format!("webrtc: transmisión sin audio: {error}"));
    }
}

async fn send_audio(sender: &mut WebRtcAudioSender) -> Result<(), String> {
    sender.transport.wait_connected().await?;
    let mut capture = super::super::stream_audio::start(&sender.target)?;
    let mut encoder = super::super::opus::VoiceOpusEncode::new_system_audio()?;
    logging::debug("stream", "webrtc: capturando el audio de la transmisión");

    // 20 ms a 48 kHz.
    const TICKS_PER_FRAME: u32 = 960;
    let mut sequence: u64 = u64::from(rand::random::<u16>());
    let base_timestamp: u32 = rand::random();
    let mut ticks: u32 = 0;
    let mut last_frame: Option<Instant> = None;
    let (mut frames_sent, mut frames_blocked) = (0u64, 0u64);

    while let Some(pcm) = capture.frames_rx.recv().await {
        let now = Instant::now();
        // Sin sonido, WASAPI no entrega nada: el reloj RTP sigue la hora real para que el
        // espectador no acumule desfase.
        if let Some(previous) = last_frame {
            let gap_frames = (now.saturating_duration_since(previous).as_millis() / 20) as u32;
            if gap_frames > 3 {
                ticks = ticks.wrapping_add((gap_frames - 1).saturating_mul(TICKS_PER_FRAME));
            }
        }
        last_frame = Some(now);
        let timestamp = base_timestamp.wrapping_add(ticks);
        ticks = ticks.wrapping_add(TICKS_PER_FRAME);

        let opus = match encoder.encode_20ms_i16(&pcm) {
            Ok(opus) => opus,
            Err(error) => {
                logging::debug("stream", format!("webrtc: Opus del audio falló: {error}"));
                continue;
            }
        };
        let payload = match sender.dave_state.lock().await.prepare_outbound_opus(&opus) {
            VoiceDaveOutboundPayload::Plain(data) | VoiceDaveOutboundPayload::Encrypted(data) => data,
            VoiceDaveOutboundPayload::Blocked(reason) => {
                frames_blocked = frames_blocked.saturating_add(1);
                if frames_blocked == 1 || frames_blocked % 500 == 0 {
                    logging::debug("stream", format!("webrtc: audio bloqueado ({frames_blocked}): {reason:?}"));
                }
                continue;
            }
        };
        sender.transport.send_audio(AudioOut { sequence, timestamp, payload })?;
        sequence = sequence.wrapping_add(1);
        frames_sent = frames_sent.saturating_add(1);
        if frames_sent == 1 {
            logging::debug("stream", "webrtc: primer paquete de audio enviado");
        }
    }
    Ok(())
}

/// Igual que `send_frames` del camino UDP (DAVE antes de paquetizar, pacing por
/// frame, espera de IDR), salvo: timestamps regulares, playout delay por la
/// biblioteca y nada de cifrado/historial propios (DTLS-SRTP y los NACK los maneja str0m).
async fn send_frames(sender: &mut WebRtcVideoSender) -> Result<(), String> {
    sender.transport.wait_connected().await?;
    // Lo que se acumuló mientras conectaba es viejo: se descarta y se pide una IDR nueva.
    while sender.frames_rx.try_recv().is_ok() {}
    sender.keyframe.request();

    let mut sequence: u64 = u64::from(rand::random::<u16>());
    let base_timestamp: u32 = rand::random();
    let mut clock = FrameClock::new(sender.fps);
    let mut bitrate_window = BitrateWindow::default();
    let mut waiting_for_keyframe = true;
    let (mut frames_sent, mut frames_blocked, mut frames_skipped, mut keyframes_sent) = (0u64, 0u64, 0u64, 0u64);
    let mut bytes_sent = 0u64;
    let mut last_stats = Instant::now();
    let mut logged_playout: Option<Option<(u32, u32)>> = None;

    while let Some(frame) = sender.frames_rx.recv().await {
        let playout = playout_units();
        if logged_playout != Some(playout) {
            logged_playout = Some(playout);
            logging::debug(
                "stream",
                format!("webrtc: playout delay (unidades de 10 ms) de los paquetes de video: {playout:?}"),
            );
        }
        if last_stats.elapsed() >= Duration::from_secs(5) {
            last_stats = Instant::now();
            logging::debug(
                "stream",
                format!(
                    "webrtc stats: sent={frames_sent} keyframes={keyframes_sent} bytes={bytes_sent} \
                     blocked_by_dave={frames_blocked} skipped_waiting_keyframe={frames_skipped}"
                ),
            );
        }
        if waiting_for_keyframe && !frame.keyframe {
            frames_skipped = frames_skipped.saturating_add(1);
            continue;
        }

        let outbound = sender.dave_state.lock().await.prepare_outbound_h264(&frame.data);
        let (access_unit, encrypted_by_dave) = match outbound {
            VoiceDaveOutboundPayload::Plain(data) => (data, false),
            VoiceDaveOutboundPayload::Encrypted(data) => (data, true),
            VoiceDaveOutboundPayload::Blocked(reason) => {
                waiting_for_keyframe = true;
                sender.keyframe.request();
                frames_blocked = frames_blocked.saturating_add(1);
                if frames_blocked == 1 || frames_blocked % 100 == 0 {
                    logging::debug("stream", format!("webrtc: frame bloqueado ({frames_blocked}): {reason:?}"));
                }
                continue;
            }
        };

        let timestamp = base_timestamp.wrapping_add(clock.ticks(frame.pts) as u32);
        let payloads = packetize_h264_access_unit(&access_unit, RTP_VIDEO_MAX_PAYLOAD);
        let count = payloads.len();
        let frame_started = Instant::now();
        // Igual que Node: `setBitrate(max(1 Mbps, bitrate medido × 1,25))` antes de cada frame.
        let measured_bps = bitrate_window.add_sample(frame_started, access_unit.len() as u64);
        let pacing_rate = (measured_bps * PACER_HEADROOM_PERCENT / 100 / 8).max(sender.pacing_floor_bytes_per_sec);
        sender.transport.set_pacer_rate(pacing_rate);
        for (index, payload) in payloads.into_iter().enumerate() {
            sender.transport.send(RtpOut {
                sequence,
                timestamp,
                marker: index + 1 == count,
                payload,
                playout,
                // Todos los paquetes de un frame comparten timestamp RTP y hora.
                wallclock: frame_started,
            })?;
            sequence = sequence.wrapping_add(1);
        }

        if frame.keyframe {
            logging::debug(
                "stream",
                format!(
                    "webrtc: IDR encolada: bytes={} packets={count} velocidad del pacer={:.2} Mbps",
                    access_unit.len(),
                    pacing_rate as f64 * 8.0 / 1_000_000.0
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
            logging::debug(
                "stream",
                format!(
                    "webrtc: primer frame enviado: keyframe={} bytes={} packets={count} dave_encrypted={encrypted_by_dave}",
                    frame.keyframe,
                    access_unit.len()
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

    const DISCORD_SDP: &str = "v=0\r\no=- 1420070400000 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
        m=audio 50001 ICE/SDP\r\nc=IN IP4 66.22.1.2\r\na=rtcp:50001\r\n\
        a=ice-ufrag:AbCd\r\na=ice-pwd:0123456789abcdef0123456789\r\n\
        a=fingerprint:sha-256 00:11:22:AA:BB:FF\r\n\
        a=candidate:1 1 UDP 4261412863 66.22.1.2 50001 typ host\r\n";

    #[test]
    fn remote_sdp_is_parsed() {
        let remote = parse_remote_sdp(DISCORD_SDP).unwrap();
        assert_eq!(remote.ufrag, "AbCd");
        assert_eq!(remote.pwd, "0123456789abcdef0123456789");
        assert_eq!(remote.fingerprint, "sha-256 00:11:22:AA:BB:FF");
        assert_eq!(remote.address, "66.22.1.2:50001".parse().unwrap());
    }

    #[test]
    fn remote_sdp_falls_back_to_connection_line_and_rtcp_port() {
        let sdp = DISCORD_SDP.replace("a=candidate:1 1 UDP 4261412863 66.22.1.2 50001 typ host\r\n", "");
        let remote = parse_remote_sdp(&sdp).unwrap();
        assert_eq!(remote.address, "66.22.1.2:50001".parse().unwrap());
        assert!(parse_remote_sdp("v=0\r\n").is_err());
    }

    #[test]
    fn offer_lists_audio_then_video_with_the_assigned_ssrcs() {
        let creds = LocalCreds { ufrag: "u".into(), pwd: "p".into(), fingerprint: "sha-256 AA:BB".into() };
        let sdp = build_offer_sdp(&creds, 10, 11, 12);
        let audio = sdp.find("m=audio").unwrap();
        let video = sdp.find("m=video").unwrap();
        assert!(audio < video);
        assert!(sdp.contains("a=group:BUNDLE 0 1\r\n"));
        assert!(sdp.contains("a=ssrc-group:FID 11 12\r\n"));
        assert!(sdp.contains("a=rtpmap:101 H264/90000\r\n"));
        assert!(sdp.contains("a=fmtp:102 apt=101\r\n"));
        assert!(sdp.contains("a=fingerprint:sha-256 AA:BB\r\n"));
        assert!(sdp.ends_with("\r\n"));
    }

    #[test]
    fn select_protocol_carries_the_sdp_and_the_connection_id() {
        let payload: Value = serde_json::from_str(&select_protocol_payload("v=0", "abc")).unwrap();
        assert_eq!(payload["op"], 1);
        assert_eq!(payload["d"]["protocol"], "webrtc");
        assert_eq!(payload["d"]["sdp"], "v=0");
        assert_eq!(payload["d"]["data"], "v=0");
        assert_eq!(payload["d"]["rtc_connection_id"], "abc");
    }

    #[test]
    fn session_description_needs_an_sdp() {
        let ok = json!({ "op": 4, "d": { "sdp": "v=0", "dave_protocol_version": 1 } });
        let parsed = parse_session_description(&ok).unwrap();
        assert_eq!(parsed.sdp, "v=0");
        assert_eq!(parsed.dave_protocol_version, Some(1));
        assert!(parse_session_description(&json!({ "op": 4, "d": { "mode": "x" } })).is_err());
    }

    #[test]
    fn frame_clock_is_regular_and_survives_an_encoder_restart() {
        let mut clock = FrameClock::new(60);
        assert_eq!(clock.ticks(Some(100)), 0);
        assert_eq!(clock.ticks(Some(101)), 1500);
        assert_eq!(clock.ticks(Some(104)), 4500); // se perdieron 2 frames: el reloj sigue la hora
        // El codificador se reinicia y su pts vuelve a 0: el timestamp no retrocede.
        assert_eq!(clock.ticks(Some(0)), 6000);
        assert_eq!(clock.ticks(Some(1)), 7500);
        // Sin pts (no debería pasar) avanza un frame por llamada.
        let mut fallback = FrameClock::new(30);
        assert_eq!(fallback.ticks(None), 0);
        assert_eq!(fallback.ticks(None), 3000);
    }
}