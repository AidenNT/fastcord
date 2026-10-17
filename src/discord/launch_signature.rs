//! Decodificador de `launch_signature` (el campo de `X-Super-Properties`).
//!
//! Basado en ingeniería inversa de `libdiscore` (WASM). Formato:
//!
//! - Es un UUID: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (16 bytes en el orden
//!   en que aparecen escritos, igual que `Uuid::as_bytes`).
//! - Cada mod de cliente conocido tiene un índice de bit fijo. Si el cliente
//!   lo trae, ese bit sale en 1; el resto de la firma es aleatorio.
//!
//! Índice de bit para un mod `id`:
//!
//! ```text
//! byte_index  = id >> 3
//! bit_in_byte = 7 - (id & 7)
//! mask        = 1 << bit_in_byte
//! ```
//!
//! # Ojo con los "primeros 8 bytes"
//!
//! La descripción original habla de un bitfield de 8 bytes, pero los ids de
//! `Legcord` en adelante (66, 72, 79, 89, 103, 116) caen en los bytes 8–14.
//! Por eso la detección se hace siempre bit a bit sobre los 16 bytes, y el
//! "bitfield" que se muestra se parte en dos mitades (`low_bitfield`:
//! bytes 0–7, `high_bitfield`: bytes 8–15).
//!
//! # Relación con `fingerprint::generate_launch_signature`
//!
//! Ese generador aplica una máscara que apaga justo estos bits (los 11 de
//! [`CLIENT_MODS`] más el bit [`RESERVED_BITS`]), así que una firma generada
//! por este cliente tiene que salir siempre limpia. Las devtools
//! (pestaña "Identidad") usan este módulo para comprobarlo en vivo.

use std::fmt;

/// Un mod de cliente conocido y el índice de bit con el que se marca.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientMod {
    pub id: u8,
    pub name: &'static str,
}

pub const CLIENT_MODS: [ClientMod; 11] = [
    ClientMod { id: 19, name: "BetterDiscord" },
    ClientMod { id: 27, name: "Rambox" },
    ClientMod { id: 36, name: "Revenge" },
    ClientMod { id: 43, name: "Vencord" },
    ClientMod { id: 52, name: "Replugged" },
    ClientMod { id: 66, name: "Legcord" },
    ClientMod { id: 72, name: "Dorion" },
    ClientMod { id: 79, name: "GCDP" },
    ClientMod { id: 89, name: "Openasar" },
    ClientMod { id: 103, name: "Shelter" },
    ClientMod { id: 116, name: "Moonlight" },
];

/// Bits que la máscara de generación también apaga pero que no corresponden a
/// ningún mod conocido (todavía). Si salen encendidos en una firma, algo
/// distinto de un cliente web limpio la produjo.
pub const RESERVED_BITS: [u8; 1] = [8];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchSignatureError {
    /// Cantidad de dígitos hexadecimales distinta de 32 (guiones aparte).
    BadLength(usize),
    /// Hay caracteres que no son hexadecimales.
    NotHex,
}

impl fmt::Display for LaunchSignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLength(found) => write!(f, "longitud de UUID inválida: {found} (esperado 32)"),
            Self::NotHex => f.write_str("el UUID contiene caracteres no hexadecimales"),
        }
    }
}

impl std::error::Error for LaunchSignatureError {}

/// Resultado de analizar una firma.
#[derive(Clone, Debug)]
pub struct LaunchSignatureReport {
    pub raw: String,
    pub bytes: [u8; 16],
    /// Mods cuyo bit está encendido.
    pub detected: Vec<ClientMod>,
    /// Mods cuyo bit está apagado.
    pub absent: Vec<ClientMod>,
    /// Ids de [`RESERVED_BITS`] que están encendidos.
    pub reserved_set: Vec<u8>,
}

impl LaunchSignatureReport {
    /// `true` si no se detectó ningún mod (los bits reservados no cuentan: se
    /// informan aparte en `reserved_set`).
    pub fn is_clean(&self) -> bool {
        self.detected.is_empty()
    }

    /// Los 16 bytes en hexadecimal separados por espacios.
    pub fn bytes_hex(&self) -> String {
        self.bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ")
    }

    /// Bytes 0–7 leídos como `u64` little-endian.
    pub fn low_bitfield(&self) -> u64 {
        le_u64(&self.bytes[..8])
    }

    /// Bytes 8–15 leídos como `u64` little-endian (acá viven los mods con id ≥ 64).
    pub fn high_bitfield(&self) -> u64 {
        le_u64(&self.bytes[8..])
    }
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(bytes);
    u64::from_le_bytes(buffer)
}

/// Convierte el UUID (con o sin guiones) a sus 16 bytes.
pub fn parse_bytes(signature: &str) -> Result<[u8; 16], LaunchSignatureError> {
    let hex: String = signature.trim().chars().filter(|c| *c != '-').collect();
    let length = hex.chars().count();
    if length != 32 {
        return Err(LaunchSignatureError::BadLength(length));
    }
    // `from_str_radix` aceptaría un "+" inicial: se valida antes a mano.
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(LaunchSignatureError::NotHex);
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        // Todo es ASCII (validado arriba), así que cortar por bytes es seguro.
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| LaunchSignatureError::NotHex)?;
    }
    Ok(bytes)
}

/// ¿Está encendido el bit del `id` dado?
pub fn has_bit(bytes: &[u8; 16], id: u8) -> bool {
    let byte_index = usize::from(id >> 3);
    let bit_in_byte = 7 - (id & 7);
    bytes.get(byte_index).is_some_and(|byte| (*byte & (1u8 << bit_in_byte)) != 0)
}

/// Analiza una `launch_signature` y devuelve los mods detectados.
pub fn analyze(signature: &str) -> Result<LaunchSignatureReport, LaunchSignatureError> {
    let bytes = parse_bytes(signature)?;
    let (detected, absent): (Vec<ClientMod>, Vec<ClientMod>) =
        CLIENT_MODS.iter().copied().partition(|client_mod| has_bit(&bytes, client_mod.id));
    let reserved_set = RESERVED_BITS.iter().copied().filter(|id| has_bit(&bytes, *id)).collect();
    Ok(LaunchSignatureReport { raw: signature.trim().to_owned(), bytes, detected, absent, reserved_set })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: &str = "00000000-0000-0000-0000-000000000000";

    #[test]
    fn all_zero_signature_is_clean() {
        let report = analyze(ZERO).unwrap();
        assert!(report.is_clean());
        assert_eq!(report.absent.len(), CLIENT_MODS.len());
        assert!(report.reserved_set.is_empty());
        assert_eq!(report.low_bitfield(), 0);
        assert_eq!(report.high_bitfield(), 0);
    }

    #[test]
    fn detects_a_mod_in_the_low_half() {
        // Vencord = bit 43 -> byte 5, bit 7 - 3 = 4 -> 0x10.
        let report = analyze("00000000-0010-0000-0000-000000000000").unwrap();
        assert_eq!(report.detected, vec![ClientMod { id: 43, name: "Vencord" }]);
        assert!(!report.is_clean());
        assert_eq!(report.low_bitfield(), 0x0000_1000_0000_0000);
    }

    #[test]
    fn detects_a_mod_in_the_high_half() {
        // Moonlight = bit 116 -> byte 14, bit 7 - 4 = 3 -> 0x08.
        let report = analyze("00000000-0000-0000-0000-000000000800").unwrap();
        assert_eq!(report.detected, vec![ClientMod { id: 116, name: "Moonlight" }]);
        // El bitfield de los "primeros 8 bytes" no lo ve: de ahí la mitad alta.
        assert_eq!(report.low_bitfield(), 0);
        assert_ne!(report.high_bitfield(), 0);
    }

    #[test]
    fn all_ones_detects_everything() {
        let report = analyze("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();
        assert_eq!(report.detected.len(), CLIENT_MODS.len());
        assert!(report.absent.is_empty());
        assert_eq!(report.reserved_set, vec![8]);
    }

    #[test]
    fn reserved_bit_is_reported_but_not_a_mod() {
        // Bit 8 -> byte 1, bit 7 -> 0x80.
        let report = analyze("00800000-0000-0000-0000-000000000000").unwrap();
        assert!(report.is_clean());
        assert_eq!(report.reserved_set, vec![8]);
    }

    #[test]
    fn accepts_hyphenless_and_uppercase() {
        let report = analyze("00000000001000000000000000000000").unwrap();
        assert_eq!(report.detected.len(), 1);
        assert!(analyze(&ZERO.to_uppercase()).is_ok());
    }

    #[test]
    fn rejects_malformed_signatures() {
        assert_eq!(analyze("1234").unwrap_err(), LaunchSignatureError::BadLength(4));
        assert_eq!(analyze("zzzzzzzz-0000-0000-0000-000000000000").unwrap_err(), LaunchSignatureError::NotHex);
        // 32 caracteres con un '+' que `from_str_radix` aceptaría.
        assert_eq!(analyze("+0000000-0000-0000-0000-000000000000").unwrap_err(), LaunchSignatureError::NotHex);
    }

    #[test]
    fn bytes_hex_formats_all_sixteen_bytes() {
        let report = analyze("00112233-4455-6677-8899-aabbccddeeff").unwrap();
        assert_eq!(report.bytes_hex(), "00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff");
        assert_eq!(report.low_bitfield(), 0x7766_5544_3322_1100);
    }
}
