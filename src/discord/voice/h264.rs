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
        let complete =
            !self.frame_broken && self.fragment.is_none() && !self.frame.is_empty();
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
                    let size =
                        usize::from(u16::from_be_bytes([payload[offset], payload[offset + 1]]));
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

/// Arma un paquete RTP de video con la extensión de "playout delay" que
/// llevan los paquetes del cliente oficial (one-byte header, id 5, valor 0).
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
    let mut packet = Vec::with_capacity(12 + 8 + payload.len());
    packet.push(0x90); // V=2, P=0, X=1, CC=0
    packet.push((u8::from(marker) << 7) | (payload_type & 0x7F));
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    packet.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
    packet.extend_from_slice(&[0x51, 0x00, 0x00, 0x00]);
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
    })
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
        assert_eq!(
            frames[0].data,
            annexb(&[&[0x09, 0x10], &sps, &pps, &idr_a, &idr_b])
        );
        assert_eq!(frames[1].data, annexb(&[&[0x09, 0x30], &p_frame]));
    }

    #[test]
    fn video_rtp_packet_parses_with_its_extension() {
        let packet = build_video_rtp_packet(7, 9000, 0x01020304, 101, true, &[1, 2, 3]);
        let header = crate::discord::voice::rtp::parse_rtp_header(&packet).unwrap();
        assert_eq!(header.payload_type, 101);
        assert!(header.marker);
        assert_eq!(header.authenticated_header_len, 16);
        assert_eq!(header.encrypted_extension_body_len, 4);
        assert_eq!(&packet[header.payload_offset..], &[1, 2, 3]);
    }
}
