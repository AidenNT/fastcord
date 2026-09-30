//! Depaquetizador H.264 sobre RTP (RFC 6184) para el visor de streams (Go Live).
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
