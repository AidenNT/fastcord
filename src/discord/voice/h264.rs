//! H.264 sobre RTP (RFC 6184) para Go Live: depaquetizador del visor y, al final
//! del archivo, el lado emisor (divisor de access units, paquetizador FU-A y
//! armado del paquete RTP de video).
//!
//! Parte receptora:
//!
//!
//! Recibe los payloads RTP ya descifrados (capa de transporte) y devuelve
//! *access units* completos en formato Annex B (cada NAL precedida por
//! `00 00 00 01`), que es lo que espera tanto el descifrado DAVE como el
//! decoder de ffmpeg. El orden de las operaciones en el receptor es:
//!
//! 1. descifrar el paquete RTP (AEAD del transporte),
//! 2. reordenar y depaquetizar hasta tener el frame entero (este módulo),
//! 3. descifrar el frame con DAVE (`VoiceDaveState::decrypt_video_frame`),
//! 4. decodificar.
//!
//! DAVE cifra el frame ANTES de paquetizarlo, por eso el paso 3 va después
//! de reensamblar y no por paquete.
//!
//! No se manda ningún PLI/FIR: si se pierde un paquete se descartan los
//! frames hasta la próxima keyframe (IDR/SPS) y listo.

use std::collections::BTreeMap;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_STAP_A: u8 = 24;
const NAL_FU_A: u8 = 28;

/// Cuántos paquetes fuera de orden se toleran antes de dar por perdido el
/// que falta y seguir con lo que hay.
const MAX_HELD_PACKETS: usize = 48;
/// Tope de un frame armado: un frame más grande es basura o un ataque.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Un paquete RTP de video, ya descifrado a nivel transporte.
#[derive(Clone, Debug)]
pub(super) struct RtpVideoPacket {
    pub(super) sequence: u16,
    pub(super) timestamp: u32,
    pub(super) marker: bool,
    pub(super) payload: Vec<u8>,
}

/// Reordena paquetes por número de secuencia (con vuelta de u16).
#[derive(Debug, Default)]
pub(super) struct RtpReorderBuffer {
    last_extended: Option<u64>,
    next_extended: Option<u64>,
    held: BTreeMap<u64, RtpVideoPacket>,
}

impl RtpReorderBuffer {
    /// Extiende el número de secuencia de 16 bits a 64 para poder ordenar
    /// aunque dé la vuelta.
    fn extend(&mut self, sequence: u16) -> u64 {
        let extended = match self.last_extended {
            None => u64::from(sequence) + 65_536,
            Some(last) => {
                let diff = i64::from(sequence.wrapping_sub(last as u16) as i16);
                (last as i64 + diff).max(0) as u64
            }
        };
        if self.last_extended.is_none_or(|last| extended > last) {
            self.last_extended = Some(extended);
        }
        extended
    }

    /// Mete un paquete y devuelve, en orden, todos los que ya se pueden
    /// entregar. Los duplicados y los que llegan demasiado tarde se
    /// descartan.
    pub(super) fn push(&mut self, packet: RtpVideoPacket) -> Vec<RtpVideoPacket> {
        let extended = self.extend(packet.sequence);
        match self.next_extended {
            None => self.next_extended = Some(extended),
            Some(next) if extended < next => return Vec::new(),
            Some(_) => {}
        }
        self.held.insert(extended, packet);

        let mut ready = Vec::new();
        self.drain_in_order(&mut ready);
        if self.held.len() > MAX_HELD_PACKETS
            && let Some(&lowest) = self.held.keys().next()
        {
            // El paquete que falta no va a llegar: se salta el hueco (el
            // depaquetizador lo detecta por la secuencia y espera keyframe).
            self.next_extended = Some(lowest);
            self.drain_in_order(&mut ready);
        }
        ready
    }

    fn drain_in_order(&mut self, ready: &mut Vec<RtpVideoPacket>) {
        while let Some(next) = self.next_extended {
            match self.held.remove(&next) {
                Some(packet) => {
                    ready.push(packet);
                    self.next_extended = Some(next + 1);
                }
                None => break,
            }
        }
    }
}

/// Arma access units H.264 en Annex B a partir de paquetes RTP en orden.
#[derive(Debug)]
pub(super) struct H264Depacketizer {
    current_timestamp: Option<u32>,
    last_sequence: Option<u16>,
    /// Frame en construcción (NALs completas con su start code).
    frame: Vec<u8>,
    /// NAL fragmentada (FU-A) en construcción: header + datos.
    fragment: Option<Vec<u8>>,
    frame_broken: bool,
    frame_has_keyframe_nal: bool,
    /// Arranca en `true`: hasta ver una keyframe no hay nada decodificable.
    need_keyframe: bool,
}

impl Default for H264Depacketizer {
    fn default() -> Self {
        Self {
            current_timestamp: None,
            last_sequence: None,
            frame: Vec::new(),
            fragment: None,
            frame_broken: false,
            frame_has_keyframe_nal: false,
            need_keyframe: true,
        }
    }
}

impl H264Depacketizer {
    /// Procesa un paquete. Devuelve un access unit completo cuando el
    /// paquete lleva el marker de fin de frame y el frame es utilizable.
    pub(super) fn push(&mut self, packet: &RtpVideoPacket) -> Option<Vec<u8>> {
        // Timestamp nuevo = frame nuevo. Si el anterior quedó a medias
        // (nunca llegó su marker) se tira.
        if self.current_timestamp != Some(packet.timestamp) {
            if !self.frame.is_empty() || self.fragment.is_some() {
                self.need_keyframe = true;
            }
            self.reset_frame();
            self.current_timestamp = Some(packet.timestamp);
        }

        // Hueco en la secuencia: se perdió algo. Lo que se venía armando
        // ya no sirve y las referencias de los frames siguientes tampoco.
        if let Some(last) = self.last_sequence
            && packet.sequence != last.wrapping_add(1)
        {
            self.need_keyframe = true;
            self.frame_broken = true;
            self.frame.clear();
            self.fragment = None;
            self.frame_has_keyframe_nal = false;
        }
        self.last_sequence = Some(packet.sequence);

        if !self.frame_broken {
            self.consume_payload(&packet.payload);
        }

        if !packet.marker {
            return None;
        }

        // Fin del frame.
        let complete = !self.frame_broken && self.fragment.is_none() && !self.frame.is_empty();
        let has_keyframe = self.frame_has_keyframe_nal;
        let frame = std::mem::take(&mut self.frame);
        self.reset_frame();
        self.current_timestamp = None;

        if !complete {
            self.need_keyframe = true;
            return None;
        }
        if has_keyframe {
            self.need_keyframe = false;
        }
        if self.need_keyframe {
            return None;
        }
        Some(frame)
    }

    /// `true` mientras no se pueda mostrar nada hasta recibir una keyframe:
    /// al arrancar, o después de perder paquetes. Es la señal para pedirla
    /// con un PLI (los streams de Discord no mandan keyframes periódicas).
    pub(super) fn needs_keyframe(&self) -> bool {
        self.need_keyframe
    }

    fn reset_frame(&mut self) {
        self.frame.clear();
        self.fragment = None;
        self.frame_broken = false;
        self.frame_has_keyframe_nal = false;
    }

    fn consume_payload(&mut self, payload: &[u8]) {
        let Some(&first) = payload.first() else {
            return;
        };
        match first & 0x1f {
            1..=23 => self.push_nal(payload),
            NAL_STAP_A => {
                let mut offset = 1usize;
                while offset + 2 <= payload.len() {
                    let size = usize::from(u16::from_be_bytes([payload[offset], payload[offset + 1]]));
                    offset += 2;
                    if size == 0 || offset + size > payload.len() {
                        self.frame_broken = true;
                        return;
                    }
                    self.push_nal(&payload[offset..offset + size]);
                    offset += size;
                }
            }
            NAL_FU_A => {
                if payload.len() < 2 {
                    self.frame_broken = true;
                    return;
                }
                let indicator = payload[0];
                let header = payload[1];
                let start = header & 0x80 != 0;
                let end = header & 0x40 != 0;
                let nal_type = header & 0x1f;
                if start {
                    // Header de la NAL reconstruida: F+NRI del indicador y
                    // el tipo real del header FU.
                    self.fragment = Some(vec![(indicator & 0xe0) | nal_type]);
                }
                match self.fragment.as_mut() {
                    Some(buffer) => buffer.extend_from_slice(&payload[2..]),
                    None => {
                        // Fragmento del medio sin haber visto el inicio.
                        self.frame_broken = true;
                        return;
                    }
                }
                if end && let Some(nal) = self.fragment.take() {
                    self.push_nal(&nal);
                }
            }
            // STAP-B, MTAP16/24, FU-B: no los usa ningún cliente de Discord.
            _ => self.frame_broken = true,
        }
    }

    fn push_nal(&mut self, nal: &[u8]) {
        if nal.is_empty() {
            return;
        }
        if self.frame.len() + nal.len() + START_CODE.len() > MAX_FRAME_BYTES {
            self.frame_broken = true;
            return;
        }
        let nal_type = nal[0] & 0x1f;
        if nal_type == NAL_IDR || nal_type == NAL_SPS {
            self.frame_has_keyframe_nal = true;
        }
        self.frame.extend_from_slice(&START_CODE);
        self.frame.extend_from_slice(nal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(sequence: u16, timestamp: u32, marker: bool, payload: &[u8]) -> RtpVideoPacket {
        RtpVideoPacket {
            sequence,
            timestamp,
            marker,
            payload: payload.to_vec(),
        }
    }

    // SPS (7), PPS (8), IDR (5) y un P-slice (1) de mentira.
    const SPS: [u8; 3] = [0x67, 0x42, 0x00];
    const PPS: [u8; 2] = [0x68, 0xce];
    const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x00];
    const P_SLICE: [u8; 3] = [0x41, 0x9a, 0x01];

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn single_nal_packets_form_a_keyframe_access_unit() {
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.push(&packet(1, 100, false, &SPS)).is_none());
        assert!(depacketizer.push(&packet(2, 100, false, &PPS)).is_none());
        let frame = depacketizer.push(&packet(3, 100, true, &IDR)).expect("frame");
        assert_eq!(frame, annexb(&[&SPS, &PPS, &IDR]));
    }

    #[test]
    fn stap_a_is_split_into_nals() {
        let mut stap = vec![NAL_STAP_A];
        for nal in [&SPS[..], &PPS[..]] {
            stap.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap.extend_from_slice(nal);
        }
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.push(&packet(1, 100, false, &stap)).is_none());
        let frame = depacketizer.push(&packet(2, 100, true, &IDR)).expect("frame");
        assert_eq!(frame, annexb(&[&SPS, &PPS, &IDR]));
    }

    #[test]
    fn fu_a_fragments_are_reassembled_with_the_original_nal_header() {
        // IDR = 0x65 -> F/NRI = 0x60, tipo 5. Datos: 88 84 00 11 22 33.
        let indicator = 0x60 | NAL_FU_A;
        let start = [indicator, 0x80 | 5, 0x88, 0x84];
        let middle = [indicator, 5, 0x00, 0x11];
        let end = [indicator, 0x40 | 5, 0x22, 0x33];
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.push(&packet(1, 100, false, &SPS)).is_none());
        assert!(depacketizer.push(&packet(2, 100, false, &start)).is_none());
        assert!(depacketizer.push(&packet(3, 100, false, &middle)).is_none());
        let frame = depacketizer.push(&packet(4, 100, true, &end)).expect("frame");
        let idr = [0x65, 0x88, 0x84, 0x00, 0x11, 0x22, 0x33];
        assert_eq!(frame, annexb(&[&SPS, &idr]));
    }

    #[test]
    fn delta_frames_are_dropped_until_the_first_keyframe() {
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.push(&packet(1, 100, true, &P_SLICE)).is_none());
        let key = depacketizer.push(&packet(2, 200, true, &IDR)).expect("keyframe");
        assert_eq!(key, annexb(&[&IDR]));
        let delta = depacketizer.push(&packet(3, 300, true, &P_SLICE)).expect("delta");
        assert_eq!(delta, annexb(&[&P_SLICE]));
    }

    #[test]
    fn needs_keyframe_until_one_arrives_and_again_after_a_loss() {
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.needs_keyframe());
        depacketizer.push(&packet(1, 100, true, &IDR)).expect("keyframe");
        assert!(!depacketizer.needs_keyframe());
        // Se pierde la secuencia 2.
        depacketizer.push(&packet(3, 300, true, &P_SLICE));
        assert!(depacketizer.needs_keyframe());
    }

    #[test]
    fn a_lost_packet_drops_frames_until_the_next_keyframe() {
        let mut depacketizer = H264Depacketizer::default();
        depacketizer.push(&packet(1, 100, true, &IDR)).expect("keyframe");
        // Se pierde la secuencia 2 (un frame entero).
        assert!(depacketizer.push(&packet(3, 300, true, &P_SLICE)).is_none());
        assert!(depacketizer.push(&packet(4, 400, true, &P_SLICE)).is_none());
        let key = depacketizer.push(&packet(5, 500, true, &IDR)).expect("keyframe");
        assert_eq!(key, annexb(&[&IDR]));
    }

    #[test]
    fn lost_fragment_in_the_middle_of_a_frame_discards_it() {
        let indicator = 0x60 | NAL_FU_A;
        let start = [indicator, 0x80 | 5, 0x88];
        let end = [indicator, 0x40 | 5, 0x22];
        let mut depacketizer = H264Depacketizer::default();
        assert!(depacketizer.push(&packet(1, 100, false, &start)).is_none());
        // Falta la secuencia 2.
        assert!(depacketizer.push(&packet(3, 100, true, &end)).is_none());
    }

    #[test]
    fn reorder_buffer_delivers_in_sequence_order() {
        let mut buffer = RtpReorderBuffer::default();
        assert_eq!(buffer.push(packet(10, 1, false, &[1])).len(), 1);
        // 12 llega antes que 11: se retiene.
        assert!(buffer.push(packet(12, 1, false, &[3])).is_empty());
        let ready = buffer.push(packet(11, 1, false, &[2]));
        let sequences: Vec<u16> = ready.iter().map(|p| p.sequence).collect();
        assert_eq!(sequences, vec![11, 12]);
    }

    #[test]
    fn reorder_buffer_handles_sequence_wraparound_and_duplicates() {
        let mut buffer = RtpReorderBuffer::default();
        assert_eq!(buffer.push(packet(65_534, 1, false, &[1])).len(), 1);
        assert_eq!(buffer.push(packet(65_535, 1, false, &[2])).len(), 1);
        assert_eq!(buffer.push(packet(0, 1, false, &[3])).len(), 1);
        // Duplicado de uno ya entregado.
        assert!(buffer.push(packet(65_535, 1, false, &[2])).is_empty());
        assert_eq!(buffer.push(packet(1, 1, false, &[4])).len(), 1);
    }

    #[test]
    fn reorder_buffer_skips_a_hole_when_too_many_packets_pile_up() {
        let mut buffer = RtpReorderBuffer::default();
        assert_eq!(buffer.push(packet(1, 1, false, &[0])).len(), 1);
        // La 2 nunca llega.
        let mut delivered = 0;
        for sequence in 3..(3 + MAX_HELD_PACKETS as u16 + 2) {
            delivered += buffer.push(packet(sequence, 1, false, &[0])).len();
        }
        assert!(delivered > 0, "debería haber saltado el hueco");
    }
}

// ===========================================================================
// Lado emisor: transmitir nuestra pantalla (Go Live propio)
// ===========================================================================
//
// El orden en el emisor es el inverso del receptor:
//
// 1. el codificador entrega un access unit completo en Annex B,
// 2. se cifra con DAVE (`VoiceDaveState::prepare_outbound_h264`) ANTES de
//    paquetizar,
// 3. se paquetiza en RTP (este módulo: NAL sueltas o FU-A),
// 4. se cifra cada paquete con el AEAD del transporte y se manda por UDP.

/// Tamaño máximo del payload H.264 de un paquete RTP. Con la cabecera RTP, la
/// extensión, el tag AEAD y el sufijo del nonce queda por debajo de los ~1280
/// bytes que se pueden mandar por UDP sin fragmentar.
pub(super) const RTP_VIDEO_MAX_PAYLOAD: usize = 1200;

/// Reloj RTP del video: 90 kHz (RFC 6184 §8.2.1).
pub(super) const RTP_VIDEO_CLOCK_HZ: u64 = 90_000;

const NAL_SLICE: u8 = 1;
const NAL_SEI: u8 = 6;
const NAL_PPS: u8 = 8;
const NAL_AUD: u8 = 9;

/// Un frame ya codificado (un access unit en Annex B).
#[derive(Clone, Debug)]
pub(crate) struct EncodedFrame {
    pub(crate) data: Vec<u8>,
    /// Contiene una IDR: un espectador nuevo puede empezar a decodificar acá.
    pub(crate) keyframe: bool,
    /// Cuándo salió del codificador. El timestamp RTP se saca de acá y no de la
    /// hora de envío: si una IDR grande demora el envío, los frames que esperan
    /// detrás no deben salir con timestamps amontonados (microtirones).
    pub(crate) captured: std::time::Instant,
    /// Número de frame del codificador (unidades de 1/fps). Con él el camino WebRTC
    /// arma timestamps RTP regulares, sin la variación de latencia del codificador.
    pub(crate) pts: Option<i64>,
}

/// Posiciones `(inicio_del_start_code, inicio_de_la_NAL)` de cada start code
/// (`00 00 01` o `00 00 00 01`) de un buffer Annex B.
fn find_start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        if data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 1 {
            let code_start = if index > 0 && data[index - 1] == 0 {
                index - 1
            } else {
                index
            };
            found.push((code_start, index + 3));
            index += 3;
        } else {
            index += 1;
        }
    }
    found
}

/// Parte un buffer Annex B en NALs, sin los start codes. Una NAL nunca termina
/// en `0x00` (el RBSP siempre cierra con un bit en 1), así que los ceros del
/// final pertenecen al start code siguiente o son relleno y se descartan.
pub(super) fn split_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let codes = find_start_codes(data);
    let mut nals = Vec::with_capacity(codes.len());
    for (position, &(_, nal_start)) in codes.iter().enumerate() {
        let mut end = codes
            .get(position + 1)
            .map(|&(next_code_start, _)| next_code_start)
            .unwrap_or(data.len());
        while end > nal_start && data[end - 1] == 0 {
            end -= 1;
        }
        if end > nal_start {
            nals.push(&data[nal_start..end]);
        }
    }
    nals
}

/// Convierte un access unit Annex B en la lista de payloads RTP (RFC 6184):
/// las NAL que entran van como "single NAL unit packet"; las más grandes, como
/// fragmentos FU-A. El llamador pone el bit de marcador en el último payload.
pub(super) fn packetize_h264_access_unit(access_unit: &[u8], max_payload: usize) -> Vec<Vec<u8>> {
    let max_payload = max_payload.max(3);
    let mut payloads = Vec::new();
    for nal in split_annexb_nals(access_unit) {
        if nal.len() <= max_payload {
            payloads.push(nal.to_vec());
            continue;
        }
        let indicator = (nal[0] & 0xE0) | NAL_FU_A;
        let nal_type = nal[0] & 0x1F;
        let body = &nal[1..];
        let chunk = max_payload - 2;
        let count = body.len().div_ceil(chunk);
        for (index, part) in body.chunks(chunk).enumerate() {
            let mut fu_header = nal_type;
            if index == 0 {
                fu_header |= 0x80; // S: primer fragmento
            }
            if index + 1 == count {
                fu_header |= 0x40; // E: último fragmento
            }
            let mut payload = Vec::with_capacity(2 + part.len());
            payload.push(indicator);
            payload.push(fu_header);
            payload.extend_from_slice(part);
            payloads.push(payload);
        }
    }
    payloads
}

/// Playout delay que se le pide al visor con la extensión RTP de los paquetes de
/// video (id 5, `http://www.webrtc.org/experiments/rtp-hdrext/playout-delay`).
///
/// Lo elige, en este orden: las devtools (`set_playout_delay_override`), la variable
/// `ECORD_PLAYOUT_DELAY` y `PlayoutDelay::DEFAULT` (sin extensión). Vale desde el próximo
/// paquete que salga, sin reiniciar la transmisión. La transmisión escribe en el log qué
/// valor está en uso al arrancar y cada vez que cambia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayoutDelay {
    /// Sin extensión: el visor usa su jitter buffer adaptativo (~0-6 ms con este stream).
    Off,
    /// Los bytes que mandaba ecord antes (id 5, 2 bytes de datos en cero, que no es el
    /// formato estándar). El visor mostraba Jitter Buffer y Target Delay en 0 ms.
    Legacy,
    /// Mínimo y máximo de retardo de reproducción, en ms. El visor elige su buffer dentro
    /// de ese rango; un mínimo mayor que 0 lo obliga a esperar ese tiempo antes de mostrar
    /// cada frame, lo que da margen a que llegue un paquete reenviado.
    Range { min_ms: u32, max_ms: u32 },
}

impl PlayoutDelay {
    /// Sin extensión: es lo único que se ha comprobado que renderiza siempre en el visor.
    /// Con `0-100` (lo que manda la librería de Node `Discord-video-stream`, en unidades de
    /// 10 ms: `playoutDelayMin = 0`, `playoutDelayMax = 10`) el visor llegó a no mostrar
    /// la imagen en alguna prueba; es opcional, por las devtools o `ECORD_PLAYOUT_DELAY`.
    pub(crate) const DEFAULT: Self = Self::Off;

    /// Texto corto para las devtools y el log.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Off => "sin extensión".to_owned(),
            Self::Legacy => "anterior (2 bytes en cero)".to_owned(),
            Self::Range { min_ms, max_ms } => format!("{min_ms}-{max_ms} ms"),
        }
    }

    /// La extensión de un byte (id 5) lista para meter tras la cabecera RTP: el
    /// formato estándar de 3 bytes lleva 12 bits de mínimo y 12 de máximo en unidades
    /// de 10 ms.
    fn extension_bytes(self) -> Option<[u8; 4]> {
        match self {
            Self::Off => None,
            Self::Legacy => Some([0x51, 0x00, 0x00, 0x00]),
            Self::Range { min_ms, max_ms } => {
                let units = |ms: u32| (ms / 10).min(0xFFF);
                let min = units(min_ms);
                let max = units(max_ms).max(min);
                Some([
                    0x52, // id 5, 3 bytes de datos
                    (min >> 4) as u8,
                    (((min & 0xF) << 4) | (max >> 8)) as u8,
                    (max & 0xFF) as u8,
                ])
            }
        }
    }
}

/// Lo que eligen las devtools en esta sesión: gana sobre `ECORD_PLAYOUT_DELAY`.
static PLAYOUT_DELAY_OVERRIDE: std::sync::Mutex<Option<PlayoutDelay>> = std::sync::Mutex::new(None);

pub(crate) fn playout_delay_override() -> Option<PlayoutDelay> {
    *PLAYOUT_DELAY_OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `None` vuelve a `ECORD_PLAYOUT_DELAY` (o al valor por defecto).
pub(crate) fn set_playout_delay_override(value: Option<PlayoutDelay>) {
    *PLAYOUT_DELAY_OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// El playout delay en uso: devtools, luego `ECORD_PLAYOUT_DELAY`, luego el valor por defecto.
pub(crate) fn playout_delay() -> PlayoutDelay {
    playout_delay_override().unwrap_or_else(playout_delay_from_env)
}

/// `ECORD_PLAYOUT_DELAY`: sin definir (o `default`) = `PlayoutDelay::DEFAULT`; `off`; `legacy`;
/// o `MIN-MAX` en ms (p. ej. `0-100` o `60-150`). Se lee una sola vez, al primer paquete.
fn playout_delay_from_env() -> PlayoutDelay {
    static FROM_ENV: std::sync::OnceLock<PlayoutDelay> = std::sync::OnceLock::new();
    *FROM_ENV.get_or_init(|| parse_playout_delay_mode(std::env::var("ECORD_PLAYOUT_DELAY").ok().as_deref()))
}

fn parse_playout_delay_mode(value: Option<&str>) -> PlayoutDelay {
    let Some(value) = value.map(|text| text.trim().to_ascii_lowercase()) else {
        return PlayoutDelay::DEFAULT;
    };
    match value.as_str() {
        "off" => PlayoutDelay::Off,
        "legacy" => PlayoutDelay::Legacy,
        range => range
            .split_once('-')
            .and_then(|(min, max)| Some((min.trim().parse::<u32>().ok()?, max.trim().parse::<u32>().ok()?)))
            .map_or(PlayoutDelay::DEFAULT, |(min_ms, max_ms)| PlayoutDelay::Range { min_ms, max_ms }),
    }
}

fn playout_delay_extension() -> Option<[u8; 4]> {
    playout_delay().extension_bytes()
}

/// Arma un paquete RTP de video (ver `playout_delay_extension` para la extensión).
///
/// Con los modos `*_rtpsize` la cabecera fija y la cabecera de la extensión
/// quedan como AAD y el cuerpo de la extensión se cifra junto con el payload;
/// `parse_rtp_header` + `VoiceRtpEncryptor::encrypt_media_packet` ya lo hacen
/// así para los paquetes que se reciben.
pub(super) fn build_video_rtp_packet(
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload_type: u8,
    marker: bool,
    payload: &[u8],
) -> Vec<u8> {
    build_video_rtp_packet_with(playout_delay_extension(), sequence, timestamp, ssrc, payload_type, marker, payload)
}

fn build_video_rtp_packet_with(
    extension: Option<[u8; 4]>,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload_type: u8,
    marker: bool,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(12 + 8 + payload.len());
    // V=2, P=0, X según haya extensión, CC=0.
    packet.push(if extension.is_some() { 0x90 } else { 0x80 });
    packet.push((u8::from(marker) << 7) | (payload_type & 0x7F));
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    if let Some(extension) = extension {
        packet.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
        packet.extend_from_slice(&extension);
    }
    packet.extend_from_slice(payload);
    packet
}

/// ¿La NAL es el primer slice de una imagen nueva? `first_mb_in_slice` es un
/// Exp-Golomb: vale 0 cuando el primer bit tras el header de la NAL es 1.
fn starts_new_picture(nal: &[u8]) -> bool {
    nal.get(1).is_some_and(|byte| byte & 0x80 != 0)
}

/// Corta el stream Annex B que escribe el codificador en access units. No
/// depende de que el codificador emita delimitadores (AUD): una NAL abre un
/// access unit nuevo si llega después de un slice y es un AUD/SEI/SPS/PPS o el
/// primer slice de otra imagen. Los frames con varios slices (x264 con
/// `zerolatency` los usa) quedan juntos.
///
/// Un access unit se entrega recién cuando llega el principio del siguiente, así
/// que suma como mucho un frame de latencia.
#[derive(Default)]
pub(crate) struct AccessUnitSplitter {
    /// Bytes que todavía no forman una NAL completa (falta el start code siguiente).
    tail: Vec<u8>,
    pending: Vec<u8>,
    pending_has_slice: bool,
    pending_keyframe: bool,
}

impl AccessUnitSplitter {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<EncodedFrame> {
        self.tail.extend_from_slice(bytes);
        let codes = find_start_codes(&self.tail);
        // La última NAL puede estar a medias: solo valen las que tienen un
        // start code después.
        let Some(&(last_code_start, _)) = codes.last() else {
            // Sin ningún start code todavía: se descarta basura vieja para que
            // no crezca sin límite.
            if self.tail.len() > MAX_FRAME_BYTES {
                self.tail.clear();
            }
            return Vec::new();
        };
        let complete = self.tail[..last_code_start].to_vec();
        self.tail.drain(..last_code_start);

        let mut frames = Vec::new();
        for nal in split_annexb_nals(&complete) {
            let nal_type = nal[0] & 0x1F;
            let is_slice = matches!(nal_type, NAL_SLICE | NAL_IDR);
            let opens_new_unit = self.pending_has_slice
                && match nal_type {
                    NAL_AUD | NAL_SEI | NAL_SPS | NAL_PPS => true,
                    NAL_SLICE | NAL_IDR => starts_new_picture(nal),
                    _ => false,
                };
            if opens_new_unit {
                frames.push(EncodedFrame {
                    data: std::mem::take(&mut self.pending),
                    keyframe: self.pending_keyframe,
                    captured: std::time::Instant::now(),
                    pts: None,
                });
                self.pending_has_slice = false;
                self.pending_keyframe = false;
            }
            self.pending.extend_from_slice(&START_CODE);
            self.pending.extend_from_slice(nal);
            if is_slice {
                self.pending_has_slice = true;
            }
            if nal_type == NAL_IDR {
                self.pending_keyframe = true;
            }
        }
        frames
    }
}

/// Arma un frame a partir de un access unit que ya llega entero (un paquete de
/// `avcodec_receive_packet`). A diferencia de `AccessUnitSplitter`, no espera
/// a ver el principio del siguiente: no suma latencia. Normaliza los start
/// codes a 4 bytes y marca `keyframe` si trae una IDR. Devuelve `None` si no
/// hay ningún slice (paquete vacío o solo cabeceras).
pub(crate) fn encoded_frame_from_access_unit(data: &[u8]) -> Option<EncodedFrame> {
    let mut out = Vec::with_capacity(data.len() + 8);
    let mut has_slice = false;
    let mut keyframe = false;
    for nal in split_annexb_nals(data) {
        let nal_type = nal[0] & 0x1F;
        match nal_type {
            NAL_SLICE => has_slice = true,
            NAL_IDR => {
                has_slice = true;
                keyframe = true;
            }
            _ => {}
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
    }
    has_slice.then_some(EncodedFrame {
        data: out,
        keyframe,
        captured: std::time::Instant::now(),
        pts: None,
    })
}

/// Limpia un access unit en Annex B (lo devuelve con start codes de 4 bytes):
///
/// * `strip_sei`: quita los NAL de SEI (6) y de relleno (12). Ninguno hace
///   falta para decodificar; NVENC en CBR mete SEI de temporización en CADA
///   frame (dos en la IDR), algo que x264 casi no hace.
/// * `strip_vui`: reescribe el VUI del SPS (ver [`with_low_delay_vui`]). El SPS de
///   NVENC en CBR trae `timing_info` y parámetros HRD que ningún decodificador
///   necesita y NO declara `bitstream_restriction`, que x264 sí declara
///   (`max_num_reorder_frames = 0`). Sin ese bloque el estándar obliga a suponer
///   el reordenamiento máximo del nivel (hasta 9 frames a 4.1 y 720p).
pub(crate) fn cleaned(data: &[u8], strip_sei: bool, strip_vui: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for nal in split_annexb_nals(data) {
        let kind = nal[0] & 0x1F;
        if strip_sei && matches!(kind, NAL_SEI | 12) {
            continue;
        }
        out.extend_from_slice(&START_CODE);
        match (strip_vui && kind == NAL_SPS).then(|| with_low_delay_vui(nal)).flatten() {
            Some(sps) => out.extend_from_slice(&sps),
            None => out.extend_from_slice(nal),
        }
    }
    out
}

/// Lector de bits (MSB primero) para el SPS, ya sin emulation prevention.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.position / 8)?;
        let bit = (byte >> (7 - self.position % 8)) & 1;
        self.position += 1;
        Some(bit)
    }

    fn bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.bit()?);
        }
        Some(value)
    }

    /// Exp-Golomb sin signo. Con signo ocupa los mismos bits, así que sirve
    /// también para saltarse un `se(v)`.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
}

/// Quita los bytes de emulation prevention (`00 00 03` pasa a `00 00`).
fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &byte in data {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Inversa de [`remove_emulation_prevention`].
fn add_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 2);
    let mut zeros = 0;
    for &byte in data {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Un SPS leído hasta el `vui_parameters_present_flag`.
struct SpsUpToVui {
    /// RBSP completo (sin el byte de la NAL ni los bytes de emulation prevention).
    rbsp: Vec<u8>,
    /// Posición, en bits, del `vui_parameters_present_flag`.
    vui_flag_at: usize,
    /// Valor de ese flag: ¿el SPS ya trae VUI?
    has_vui: bool,
    /// `max_num_ref_frames` del SPS.
    max_num_ref_frames: u32,
}

/// Recorre el SPS hasta el flag del VUI. `None` si no es un SPS, si está cortado
/// antes de ese punto o si usa listas de escala (perfiles altos con matrices
/// propias), casos en los que no se toca.
fn parse_sps_up_to_vui(nal: &[u8]) -> Option<SpsUpToVui> {
    if nal.first()? & 0x1F != NAL_SPS {
        return None;
    }
    let rbsp = remove_emulation_prevention(&nal[1..]);
    let (vui_flag_at, has_vui, max_num_ref_frames) = {
        let mut reader = BitReader { data: &rbsp, position: 0 };
        let profile = reader.bits(8)?;
        reader.bits(16)?; // constraint flags y level_idc
        reader.ue()?; // seq_parameter_set_id
        if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
            if reader.ue()? == 3 {
                reader.bit()?; // separate_colour_plane_flag
            }
            reader.ue()?; // bit_depth_luma_minus8
            reader.ue()?; // bit_depth_chroma_minus8
            reader.bit()?; // qpprime_y_zero_transform_bypass_flag
            if reader.bit()? == 1 {
                return None; // seq_scaling_matrix_present_flag
            }
        }
        reader.ue()?; // log2_max_frame_num_minus4
        match reader.ue()? {
            0 => {
                reader.ue()?; // log2_max_pic_order_cnt_lsb_minus4
            }
            1 => {
                reader.bit()?; // delta_pic_order_always_zero_flag
                reader.ue()?; // offset_for_non_ref_pic
                reader.ue()?; // offset_for_top_to_bottom_field
                let cycle = reader.ue()?;
                for _ in 0..cycle.min(255) {
                    reader.ue()?; // offset_for_ref_frame
                }
            }
            _ => {}
        }
        let max_num_ref_frames = reader.ue()?;
        reader.bit()?; // gaps_in_frame_num_value_allowed_flag
        reader.ue()?; // pic_width_in_mbs_minus1
        reader.ue()?; // pic_height_in_map_units_minus1
        if reader.bit()? == 0 {
            reader.bit()?; // mb_adaptive_frame_field_flag
        }
        reader.bit()?; // direct_8x8_inference_flag
        if reader.bit()? == 1 {
            for _ in 0..4 {
                reader.ue()?; // frame_crop_*_offset
            }
        }
        let vui_flag_at = reader.position;
        let has_vui = reader.bit()? == 1;
        (vui_flag_at, has_vui, max_num_ref_frames)
    };
    Some(SpsUpToVui { rbsp, vui_flag_at, has_vui, max_num_ref_frames })
}

/// Escritor de bits (MSB primero), la inversa de [`BitReader`].
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    /// Bits ya escritos en el último byte (0 = el último byte está completo).
    used: usize,
}

impl BitWriter {
    fn bit(&mut self, value: u8) {
        if self.used == 0 {
            self.bytes.push(0);
        }
        if value != 0 {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 1 << (7 - self.used);
        }
        self.used = (self.used + 1) % 8;
    }

    /// Copia los primeros `count` bits de `data`.
    fn copy_bits(&mut self, data: &[u8], count: usize) {
        for index in 0..count {
            self.bit((data[index / 8] >> (7 - index % 8)) & 1);
        }
    }

    /// Exp-Golomb sin signo.
    fn ue(&mut self, value: u32) {
        let coded = u64::from(value) + 1;
        let length = 64 - coded.leading_zeros();
        for _ in 1..length {
            self.bit(0);
        }
        for shift in (0..length).rev() {
            self.bit(((coded >> shift) & 1) as u8);
        }
    }

    /// Cierra con `rbsp_trailing_bits` (un 1 y ceros hasta completar el byte).
    fn finish(mut self) -> Vec<u8> {
        self.bit(1);
        while self.used != 0 {
            self.bit(0);
        }
        self.bytes
    }
}

/// El mismo SPS (sin start code) con `vui_parameters_present_flag` en 0 y nada
/// después. `None` si no es un SPS, si no trae VUI o si usa listas de escala
/// (perfiles altos con matrices propias), casos en los que no se toca.
///
/// Ya no se usa al emitir: un SPS sin ningún VUI tampoco declara
/// `bitstream_restriction`. Queda para comparar.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn without_vui(nal: &[u8]) -> Option<Vec<u8>> {
    let sps = parse_sps_up_to_vui(nal)?;
    if !sps.has_vui {
        return None; // no hay VUI que quitar
    }
    let mut writer = BitWriter::default();
    writer.copy_bits(&sps.rbsp, sps.vui_flag_at);
    writer.bit(0); // vui_parameters_present_flag
    let mut result = vec![nal[0]];
    result.extend(add_emulation_prevention(&writer.finish()));
    Some(result)
}

/// El mismo SPS (sin start code) con el VUI reemplazado por uno mínimo que solo
/// lleva `bitstream_restriction`, como el de x264:
///
/// * sin `timing_info` ni HRD (lo que mete NVENC en CBR);
/// * `max_num_reorder_frames = 0`: no hay B-frames, el orden de salida es el de
///   decodificación;
/// * `max_dec_frame_buffering = max_num_ref_frames` (mínimo 1; nunca puede ser
///   menor que las referencias que declara el SPS).
///
/// Sin ese bloque, el estándar obliga al decodificador a suponer el
/// reordenamiento máximo que permite el nivel. Funciona aunque el SPS original
/// no tuviera VUI. `None` si no es un SPS, si está cortado antes del VUI o si usa
/// listas de escala, casos en los que no se toca.
pub(crate) fn with_low_delay_vui(nal: &[u8]) -> Option<Vec<u8>> {
    let sps = parse_sps_up_to_vui(nal)?;
    let mut writer = BitWriter::default();
    writer.copy_bits(&sps.rbsp, sps.vui_flag_at);
    writer.bit(1); // vui_parameters_present_flag
    for _ in 0..4 {
        writer.bit(0); // aspect_ratio_info, overscan_info, video_signal_type, chroma_loc_info
    }
    writer.bit(0); // timing_info_present_flag
    for _ in 0..3 {
        writer.bit(0); // nal_hrd, vcl_hrd, pic_struct_present_flag
    }
    writer.bit(1); // bitstream_restriction_flag
    writer.bit(1); // motion_vectors_over_pic_boundaries_flag
    writer.ue(0); // max_bytes_per_pic_denom
    writer.ue(0); // max_bits_per_mb_denom
    writer.ue(11); // log2_max_mv_length_horizontal
    writer.ue(11); // log2_max_mv_length_vertical
    writer.ue(0); // max_num_reorder_frames
    writer.ue(sps.max_num_ref_frames.max(1)); // max_dec_frame_buffering
    let mut result = vec![nal[0]];
    result.extend(add_emulation_prevention(&writer.finish()));
    Some(result)
}

/// Resumen legible de un SPS (sin el start code): perfil, restricciones y nivel
/// (bytes 1 a 3 tras el header) más los primeros bytes en hexadecimal. Para el log.
pub(crate) fn describe_sps(nal: &[u8]) -> String {
    let hex: String = nal.iter().take(24).map(|byte| format!("{byte:02x}")).collect();
    match nal {
        [_, profile, constraints, level, ..] => {
            format!("profile_idc={profile} constraints={constraints:#04x} level_idc={level} bytes={} hex={hex}", nal.len())
        }
        _ => format!("sps corto hex={hex}"),
    }
}

#[cfg(test)]
mod sender_tests {
    use super::*;

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn splits_three_and_four_byte_start_codes() {
        let data = [0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 4];
        let nals = split_annexb_nals(&data);
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4][..]]);
    }

    #[test]
    fn whole_access_unit_is_normalized_and_flags_idr() {
        // AUD + SPS + PPS + IDR con start codes mezclados de 3 y 4 bytes.
        let data = [
            0, 0, 0, 1, 0x09, 0xF0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 0x88, 9,
        ];
        let frame = encoded_frame_from_access_unit(&data).expect("tiene slice");
        assert!(frame.keyframe);
        assert_eq!(
            frame.data,
            annexb(&[&[0x09, 0xF0], &[0x67, 1], &[0x68, 2], &[0x65, 0x88, 9]])
        );
        // Un P-frame no es keyframe; sin slices no hay frame.
        let p = encoded_frame_from_access_unit(&[0, 0, 0, 1, 0x41, 0x9A, 1]).unwrap();
        assert!(!p.keyframe);
        assert!(encoded_frame_from_access_unit(&[0, 0, 0, 1, 0x67, 1]).is_none());
        assert!(encoded_frame_from_access_unit(&[]).is_none());
    }

    #[test]
    fn sei_and_filler_are_stripped_and_the_rest_is_kept() {
        // AUD, SPS, PPS, SEI, SEI, relleno, IDR.
        let data = annexb(&[&[0x09, 0xF0], &[0x67, 1], &[0x68, 2], &[0x06, 3], &[0x06, 4], &[0x0C, 5], &[0x65, 6]]);
        let stripped = cleaned(&data, true, false);
        assert_eq!(stripped, annexb(&[&[0x09, 0xF0], &[0x67, 1], &[0x68, 2], &[0x65, 6]]));
        // Sin SEI el resultado es el mismo access unit.
        assert_eq!(cleaned(&stripped, true, false), stripped);
        assert!(encoded_frame_from_access_unit(&stripped).unwrap().keyframe);
        // Sin pedir nada solo se normalizan los start codes.
        assert_eq!(cleaned(&data, false, false), data);
    }

    /// El SPS real de NVENC (1280x720, Constrained Baseline 3.1, CBR): trae VUI
    /// con timing_info y HRD. Los bytes son los primeros 24 de un SPS de 33.
    const NVENC_SPS: [u8; 24] = [
        0x67, 0x42, 0xC0, 0x1F, 0x95, 0x90, 0x05, 0x00, 0x5B, 0xB0, 0x11, 0x00, 0x00, 0x03, 0x03, 0xE8, 0x00, 0x00,
        0xEA, 0x60, 0xE0, 0x00, 0x00, 0x02,
    ];

    #[test]
    fn the_vui_of_an_sps_is_removed_and_nothing_else_changes() {
        let clean = without_vui(&NVENC_SPS).unwrap();
        assert_eq!(clean, [0x67, 0x42, 0xC0, 0x1F, 0x95, 0x90, 0x05, 0x00, 0x5B, 0x90]);
        // Ya limpio no hay nada más que quitar.
        assert!(without_vui(&clean).is_none());
        assert!(without_vui(&[0x68, 1, 2]).is_none());
        // Y `cleaned` reescribe el VUI solo en los SPS.
        let data = annexb(&[&NVENC_SPS, &[0x68, 2], &[0x65, 6]]);
        let rewritten = with_low_delay_vui(&NVENC_SPS).unwrap();
        assert_eq!(cleaned(&data, false, true), annexb(&[&rewritten, &[0x68, 2], &[0x65, 6]]));
        assert_eq!(cleaned(&data, false, false), data);
    }

    /// El SPS real de NVENC del log (1280x720, Constrained Baseline 4.1, CBR,
    /// `max_num_ref_frames = 2`). Son los primeros 24 de sus 32 bytes: alcanza
    /// porque solo se lee hasta el flag del VUI.
    const NVENC_SPS_720P_L41: [u8; 24] = [
        0x67, 0x42, 0xC0, 0x29, 0x95, 0xB0, 0x14, 0x01, 0x6E, 0xC0, 0x44, 0x00, 0x00, 0x0F, 0xA0, 0x00, 0x03, 0xA9,
        0x83, 0x80, 0x00, 0x00, 0x98, 0x96,
    ];

    #[test]
    fn the_vui_is_rewritten_with_a_bitstream_restriction_like_x264s() {
        let rewritten = with_low_delay_vui(&NVENC_SPS_720P_L41).unwrap();
        assert_eq!(
            rewritten,
            [0x67, 0x42, 0xC0, 0x29, 0x95, 0xB0, 0x14, 0x01, 0x6E, 0x80, 0x78, 0xC1, 0x97]
        );
        // Se lee de vuelta: sin timing ni HRD, sin reordenamiento y con
        // `max_dec_frame_buffering` igual a `max_num_ref_frames` (2 en NVENC).
        let sps = parse_sps_up_to_vui(&rewritten).unwrap();
        assert!(sps.has_vui);
        assert_eq!(sps.max_num_ref_frames, 2);
        let mut reader = BitReader { data: &sps.rbsp, position: sps.vui_flag_at + 1 };
        for _ in 0..8 {
            assert_eq!(reader.bit(), Some(0)); // aspect..chroma_loc, timing, nal/vcl hrd, pic_struct
        }
        assert_eq!(reader.bit(), Some(1)); // bitstream_restriction_flag
        assert_eq!(reader.bit(), Some(1)); // motion_vectors_over_pic_boundaries_flag
        let fields: Vec<u32> = (0..6).map(|_| reader.ue().unwrap()).collect();
        assert_eq!(fields, [0, 0, 11, 11, 0, 2]);
        assert_eq!(reader.bit(), Some(1)); // bit de parada
        // Es idempotente y no toca lo que no es un SPS.
        assert_eq!(with_low_delay_vui(&rewritten).unwrap(), rewritten);
        assert!(with_low_delay_vui(&[0x68, 1, 2]).is_none());
    }

    #[test]
    fn exp_golomb_writer_matches_the_reader() {
        for value in [0u32, 1, 2, 3, 6, 11, 79, 255, 78_124] {
            let mut writer = BitWriter::default();
            writer.ue(value);
            let bytes = writer.finish();
            let mut reader = BitReader { data: &bytes, position: 0 };
            assert_eq!(reader.ue(), Some(value));
            assert_eq!(reader.bit(), Some(1)); // el bit de parada de `finish`
        }
    }

    #[test]
    fn emulation_prevention_round_trips() {
        let escaped = [0u8, 0, 3, 1, 9, 0, 0, 3, 0];
        let plain = remove_emulation_prevention(&escaped);
        assert_eq!(plain, [0, 0, 1, 9, 0, 0, 0]);
        assert_eq!(add_emulation_prevention(&plain), escaped);
    }

    #[test]
    fn an_sps_is_described_by_profile_and_level() {
        let text = describe_sps(&[0x67, 66, 0xE0, 31, 0xAB]);
        assert!(text.contains("profile_idc=66"), "{text}");
        assert!(text.contains("level_idc=31"), "{text}");
        assert!(text.contains("hex=67"), "{text}");
    }

    #[test]
    fn small_nals_are_sent_as_single_packets() {
        let au = annexb(&[&[0x67, 1, 2, 3], &[0x68, 4]]);
        let payloads = packetize_h264_access_unit(&au, 100);
        assert_eq!(payloads, vec![vec![0x67, 1, 2, 3], vec![0x68, 4]]);
    }

    #[test]
    fn big_nals_round_trip_through_fu_a() {
        let mut nal = vec![0x65];
        nal.extend((0..5000u32).map(|n| (n % 251) as u8 + 1));
        let au = annexb(&[&nal]);
        let payloads = packetize_h264_access_unit(&au, 1200);
        assert!(payloads.len() > 1);
        assert!(payloads.iter().all(|payload| payload.len() <= 1200));
        assert_eq!(payloads[0][0], (0x65 & 0xE0) | NAL_FU_A);
        assert_eq!(payloads[0][1], 0x80 | 5);
        assert_eq!(payloads.last().unwrap()[1], 0x40 | 5);

        // El depaquetizador del visor tiene que rearmar lo mismo.
        let mut depacketizer = H264Depacketizer::default();
        let mut rebuilt = None;
        let last = payloads.len() - 1;
        for (index, payload) in payloads.into_iter().enumerate() {
            rebuilt = depacketizer.push(&RtpVideoPacket {
                sequence: index as u16,
                timestamp: 1000,
                marker: index == last,
                payload,
            });
        }
        assert_eq!(rebuilt.expect("access unit"), au);
    }

    #[test]
    fn splitter_groups_slices_and_marks_keyframes() {
        let sps = [0x67, 0x42, 0x00, 0x1f];
        let pps = [0x68, 0xce, 0x38];
        let idr_a = [0x65, 0x88, 1, 2]; // first_mb = 0
        let idr_b = [0x65, 0x10, 3, 4]; // otro slice de la misma imagen
        let p_frame = [0x41, 0x9a, 5, 6];
        // El último AUD solo sirve de centinela: una NAL se procesa cuando llega
        // el start code que la sigue, así que la final queda esperando.
        let stream = annexb(&[
            &[0x09, 0x10],
            &sps,
            &pps,
            &idr_a,
            &idr_b,
            &[0x09, 0x30],
            &p_frame,
            &[0x09, 0x30],
            &[0x09, 0x10],
        ]);

        let mut splitter = AccessUnitSplitter::default();
        let mut frames = Vec::new();
        // En trozos de 5 bytes, para ejercitar los cortes en medio de una NAL.
        for chunk in stream.chunks(5) {
            frames.extend(splitter.push(chunk));
        }
        assert_eq!(frames.len(), 2);
        assert!(frames[0].keyframe);
        assert!(!frames[1].keyframe);
        assert_eq!(frames[0].data, annexb(&[&[0x09, 0x10], &sps, &pps, &idr_a, &idr_b]));
        assert_eq!(frames[1].data, annexb(&[&[0x09, 0x30], &p_frame]));
    }

    #[test]
    fn video_rtp_packet_parses_with_and_without_extension() {
        let with = build_video_rtp_packet_with(Some([0x52, 0, 0, 20]), 7, 9000, 0x01020304, 101, true, &[1, 2, 3]);
        let header = crate::discord::voice::rtp::parse_rtp_header(&with).unwrap();
        assert_eq!(header.payload_type, 101);
        assert!(header.marker);
        assert_eq!(header.authenticated_header_len, 16);
        assert_eq!(header.encrypted_extension_body_len, 4);
        assert_eq!(&with[header.payload_offset..], &[1, 2, 3]);

        let without = build_video_rtp_packet_with(None, 7, 9000, 0x01020304, 101, true, &[1, 2, 3]);
        let header = crate::discord::voice::rtp::parse_rtp_header(&without).unwrap();
        assert_eq!(header.payload_type, 101);
        assert!(header.marker);
        assert_eq!(header.authenticated_header_len, 12);
        assert_eq!(header.encrypted_extension_body_len, 0);
        assert_eq!(&without[header.payload_offset..], &[1, 2, 3]);
    }

    #[test]
    fn playout_delay_modes() {
        // Por defecto no se manda extensión.
        assert_eq!(PlayoutDelay::DEFAULT, PlayoutDelay::Off);
        assert_eq!(PlayoutDelay::DEFAULT.extension_bytes(), None);
        // 0-100 ms: 0 y 10 unidades de 10 ms.
        assert_eq!(
            parse_playout_delay_mode(Some("0-100")).extension_bytes(),
            Some([0x52, 0x00, 0x00, 0x0A])
        );
        for text in [None, Some(""), Some("default"), Some("basura"), Some("5-")] {
            assert_eq!(parse_playout_delay_mode(text), PlayoutDelay::DEFAULT, "{text:?}");
        }
        assert_eq!(parse_playout_delay_mode(Some("off")), PlayoutDelay::Off);
        assert_eq!(PlayoutDelay::Off.extension_bytes(), None);
        assert_eq!(parse_playout_delay_mode(Some(" LEGACY ")), PlayoutDelay::Legacy);
        assert_eq!(PlayoutDelay::Legacy.extension_bytes(), Some([0x51, 0, 0, 0]));
        let range = parse_playout_delay_mode(Some("0-200"));
        assert_eq!(range, PlayoutDelay::Range { min_ms: 0, max_ms: 200 });
        // 0-200 ms: min 0, max 20 unidades de 10 ms.
        assert_eq!(range.extension_bytes(), Some([0x52, 0x00, 0x00, 0x14]));
        // 100-4000 ms: min 10 (0x00A), max 400 (0x190).
        assert_eq!(
            PlayoutDelay::Range { min_ms: 100, max_ms: 4000 }.extension_bytes(),
            Some([0x52, 0x00, 0xA1, 0x90])
        );
        // Un máximo menor que el mínimo se sube al mínimo.
        assert_eq!(
            PlayoutDelay::Range { min_ms: 100, max_ms: 20 }.extension_bytes(),
            Some([0x52, 0x00, 0xA0, 0x0A])
        );
    }

    #[test]
    fn playout_delay_override_wins_and_can_be_cleared() {
        let custom = PlayoutDelay::Range { min_ms: 60, max_ms: 150 };
        set_playout_delay_override(Some(custom));
        assert_eq!(playout_delay(), custom);
        assert_eq!(playout_delay_override(), Some(custom));
        set_playout_delay_override(None);
        assert_eq!(playout_delay_override(), None);
    }
}