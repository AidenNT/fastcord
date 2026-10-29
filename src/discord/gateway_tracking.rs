//! Seguimiento del Gateway para las devtools (Cliente → Seguimiento).
//!
//! El hilo del gateway anota acá lo que hace (latidos, ACKs, opcode 41) y la UI
//! lee una foto con [`snapshot`]. Solo guarda tiempos y contadores: nada de
//! tokens ni de contenido de los mensajes.
//!
//! Todo se mide con [`Instant`] (monótono), así que la UI puede calcular
//! "cuánto falta" restando contra `Instant::now()` en cada cuadro.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Por qué un tick del opcode 41 no mandó nada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Todavía no hubo READY/RESUMED en esta conexión.
    NotEstablished,
    /// Ventana sin foco y sin llamada de voz (el cliente oficial tampoco lo manda).
    Inactive,
    /// `ECORD_QOS_HEARTBEAT=0`: el opcode 41 solo va con el QoS heartbeat.
    QosDisabled,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::NotEstablished => "la sesión todavía no está establecida",
            SkipReason::Inactive => "sin foco en la ventana y sin llamada de voz",
            SkipReason::QosDisabled => "QoS heartbeat desactivado (ECORD_QOS_HEARTBEAT=0)",
        }
    }
}

/// Qué disparó el último opcode 41.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendReason {
    /// Al llegar el READY (sesión nueva).
    Ready,
    /// Al llegar el RESUMED (sesión reanudada).
    Resumed,
    /// El temporizador de 30 minutos.
    Timer,
}

impl SendReason {
    pub fn label(self) -> &'static str {
        match self {
            SendReason::Ready => "al conectar (READY)",
            SendReason::Resumed => "al reanudar (RESUMED)",
            SendReason::Timer => "temporizador de 30 min",
        }
    }
}

/// Foto del estado del gateway.
#[derive(Clone, Debug, Default)]
pub struct GatewayTracking {
    /// Hay un WebSocket abierto con el gateway.
    pub connected: bool,
    /// Hubo READY/RESUMED en esta conexión.
    pub established: bool,
    /// Instante en que se abrió la conexión actual.
    pub connected_at: Option<Instant>,
    /// Conexiones abiertas desde que arrancó la app (la primera cuenta como 1).
    pub connections: u64,
    /// Último número de secuencia recibido.
    pub sequence: Option<u64>,

    /// `heartbeat_interval` que pidió Discord en el HELLO.
    pub heartbeat_interval: Option<Duration>,
    /// Cuándo toca el próximo latido (opcode 40, o 1 como respaldo).
    pub next_heartbeat_at: Option<Instant>,
    pub last_heartbeat_at: Option<Instant>,
    pub last_ack_at: Option<Instant>,
    /// Ida y vuelta del último latido contestado.
    pub last_rtt: Option<Duration>,
    /// Se mandó un latido y todavía no llegó el ACK.
    pub awaiting_ack: bool,
    pub heartbeats_sent: u64,
    pub acks_received: u64,

    /// Cuándo toca el próximo tick del opcode 41.
    pub time_spent_next_at: Option<Instant>,
    pub time_spent_last_sent_at: Option<Instant>,
    pub time_spent_last_reason: Option<SendReason>,
    /// Opcodes 41 mandados desde que arrancó la app.
    pub time_spent_sent: u64,
    /// Ticks que no mandaron nada (sin foco, sin sesión, etc.).
    pub time_spent_skipped: u64,
    pub time_spent_last_skip: Option<(Instant, SkipReason)>,
}

static TRACKING: Mutex<GatewayTracking> = Mutex::new(GatewayTracking {
    connected: false,
    established: false,
    connected_at: None,
    connections: 0,
    sequence: None,
    heartbeat_interval: None,
    next_heartbeat_at: None,
    last_heartbeat_at: None,
    last_ack_at: None,
    last_rtt: None,
    awaiting_ack: false,
    heartbeats_sent: 0,
    acks_received: 0,
    time_spent_next_at: None,
    time_spent_last_sent_at: None,
    time_spent_last_reason: None,
    time_spent_sent: 0,
    time_spent_skipped: 0,
    time_spent_last_skip: None,
});

fn with<R>(f: impl FnOnce(&mut GatewayTracking) -> R) -> R {
    // Si otro hilo entró en pánico con el candado tomado, el contenido sigue
    // siendo válido (son números sueltos): se usa igual.
    let mut guard = TRACKING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

/// Foto actual del estado, para dibujar en la UI.
pub fn snapshot() -> GatewayTracking {
    with(|t| t.clone())
}

// -- Lo que anota el hilo del gateway ----------------------------------------------------

pub(crate) fn connection_opened(time_spent_interval: Duration) {
    let now = Instant::now();
    with(|t| {
        t.connected = true;
        t.established = false;
        t.connected_at = Some(now);
        t.connections += 1;
        t.heartbeat_interval = None;
        t.next_heartbeat_at = None;
        t.awaiting_ack = false;
        // El interval de tokio descarta su primer tick: el próximo es a los 30 min.
        t.time_spent_next_at = Some(now + time_spent_interval);
    });
}

pub(crate) fn connection_closed() {
    with(|t| {
        t.connected = false;
        t.established = false;
        t.awaiting_ack = false;
        t.next_heartbeat_at = None;
        t.time_spent_next_at = None;
    });
}

pub(crate) fn established() {
    with(|t| t.established = true);
}

pub(crate) fn sequence(seq: Option<u64>) {
    with(|t| t.sequence = seq);
}

/// HELLO: Discord fijó el intervalo y el reloj de latidos arranca de cero.
pub(crate) fn hello(interval: Duration) {
    with(|t| {
        t.heartbeat_interval = Some(interval);
        t.next_heartbeat_at = Some(Instant::now() + interval);
    });
}

pub(crate) fn heartbeat_sent() {
    let now = Instant::now();
    with(|t| {
        t.heartbeats_sent += 1;
        t.last_heartbeat_at = Some(now);
        t.awaiting_ack = true;
        // Un latido fuera de turno (pedido por Discord, o el que acompaña al
        // opcode 41) no mueve `next_heartbeat_at`: el temporizador sigue su
        // ritmo y solo su propio tick lo cambia (ver `heartbeat_tick`).
    });
}

/// El tick regular del temporizador de latidos.
pub(crate) fn heartbeat_tick() {
    with(|t| {
        if let Some(interval) = t.heartbeat_interval {
            t.next_heartbeat_at = Some(Instant::now() + interval);
        }
    });
}

pub(crate) fn heartbeat_ack() {
    let now = Instant::now();
    with(|t| {
        t.acks_received += 1;
        t.last_ack_at = Some(now);
        t.awaiting_ack = false;
        t.last_rtt = t.last_heartbeat_at.map(|sent| now.saturating_duration_since(sent));
    });
}

/// Se mandó un opcode 41. `interval` reinicia la cuenta regresiva.
pub(crate) fn time_spent_sent(reason: SendReason, interval: Duration) {
    let now = Instant::now();
    with(|t| {
        t.time_spent_sent += 1;
        t.time_spent_last_sent_at = Some(now);
        t.time_spent_last_reason = Some(reason);
        t.time_spent_next_at = Some(now + interval);
    });
}

/// Venció el temporizador pero no correspondía mandar el opcode 41.
pub(crate) fn time_spent_skipped(reason: SkipReason, interval: Duration) {
    let now = Instant::now();
    with(|t| {
        t.time_spent_skipped += 1;
        t.time_spent_last_skip = Some((now, reason));
        t.time_spent_next_at = Some(now + interval);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // Los tests comparten el estado global: se hacen en uno solo.
    #[test]
    fn tracks_time_spent_countdown_and_heartbeat_rtt() {
        let interval = Duration::from_secs(1800);
        connection_opened(interval);
        let snap = snapshot();
        assert!(snap.connected && !snap.established);
        let due = snap.time_spent_next_at.expect("cuenta regresiva armada al conectar");
        assert!(due > Instant::now() + Duration::from_secs(1790));

        time_spent_skipped(SkipReason::NotEstablished, interval);
        assert_eq!(snapshot().time_spent_skipped, 1);

        established();
        time_spent_sent(SendReason::Ready, interval);
        let snap = snapshot();
        assert_eq!(snap.time_spent_sent, 1);
        assert_eq!(snap.time_spent_last_reason, Some(SendReason::Ready));

        hello(Duration::from_millis(41250));
        heartbeat_sent();
        assert!(snapshot().awaiting_ack);
        heartbeat_ack();
        let snap = snapshot();
        assert!(!snap.awaiting_ack);
        assert!(snap.last_rtt.is_some());

        connection_closed();
        let snap = snapshot();
        assert!(!snap.connected);
        assert!(snap.time_spent_next_at.is_none());
        // Los contadores sobreviven al cierre.
        assert_eq!(snap.time_spent_sent, 1);
    }
}
