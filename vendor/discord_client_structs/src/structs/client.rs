#[derive(Debug, Clone)]
pub struct ClientSession {
    pub client_launch_id: uuid::Uuid,
    pub client_heartbeat_session_id: uuid::Uuid,
    pub launch_signature: uuid::Uuid,
    pub heartbeat_expiry: chrono::DateTime<chrono::Utc>,
}

/// Máscara de bits válidos para `launch_signature` (idéntica a la de
/// `fingerprint::generate_launch_signature` del stack principal).
const LAUNCH_SIGNATURE_MASK: [u8; 16] = [
    0b1111_1111,
    0b0111_1111,
    0b1110_1111,
    0b1110_1111,
    0b1111_0111,
    0b1110_1111,
    0b1111_0111,
    0b1111_1111,
    0b1101_1111,
    0b0111_1110,
    0b1111_1111,
    0b1011_1111,
    0b1111_1110,
    0b1111_1111,
    0b1111_0111,
    0b1111_1111,
];

fn generate_launch_signature() -> uuid::Uuid {
    let mut bytes = *uuid::Uuid::new_v4().as_bytes();
    for (byte, mask) in bytes.iter_mut().zip(LAUNCH_SIGNATURE_MASK) {
        *byte &= mask;
    }
    uuid::Uuid::from_bytes(bytes)
}

impl ClientSession {
    pub fn new() -> Self {
        let now = chrono::Utc::now();
        Self {
            client_launch_id: uuid::Uuid::new_v4(),
            client_heartbeat_session_id: uuid::Uuid::new_v4(),
            launch_signature: generate_launch_signature(),
            heartbeat_expiry: now + chrono::Duration::minutes(30),
        }
    }

    pub fn refresh_heartbeat(&mut self) {
        self.client_heartbeat_session_id = uuid::Uuid::new_v4();
        self.heartbeat_expiry = chrono::Utc::now() + chrono::Duration::minutes(30);
    }
}

#[derive(Debug, Clone)]
pub struct BuildNumbers {
    pub client_build_number: u32,
    pub native_build_number: Option<u32>,
}

impl BuildNumbers {
    pub fn new(client_build_number: u32, native_build_number: Option<u32>) -> Self {
        Self {
            client_build_number,
            native_build_number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_signature_respects_mask() {
        for _ in 0..256 {
            let session = ClientSession::new();
            let bytes = session.launch_signature.as_bytes();
            for (byte, mask) in bytes.iter().zip(LAUNCH_SIGNATURE_MASK) {
                assert_eq!(byte & !mask, 0);
            }
        }
    }
}
