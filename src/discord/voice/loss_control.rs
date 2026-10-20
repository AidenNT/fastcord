//! Control del bitrate de video según la pérdida que reporta el servidor.
//!
//! El servidor manda cada ~0,5 s un receiver report de nuestro SSRC de video con
//! la fracción perdida. El REMB solo sirve de tope (suele venir fijo), así que sin
//! esto el bitrate no se mueve aunque se pierda el 3-5 % de los paquetes, y con el
//! buffer de 0 ms del visor cada paquete perdido es un congelamiento.
//!
//! Regla: pérdida suavizada alta y sostenida → bajar el bitrate un escalón. Si el
//! escalón no mejoró la pérdida (es del enlace, no del bitrate) se deshace y se deja
//! de insistir un rato: bajar la calidad sin ganar nada es peor. Con la pérdida
//! baja y sostenida se vuelve a subir de a poco hasta el techo.
//!
//! No depende de nada del resto del programa para poder probarse sola.

use std::time::{Duration, Instant};

/// Pérdida suavizada (%) a partir de la cual se baja el bitrate.
const LOSS_HIGH_PERCENT: f64 = 2.0;
/// Pérdida suavizada (%) por debajo de la cual se considera el enlace limpio.
const LOSS_CLEAN_PERCENT: f64 = 0.5;
/// Reportes seguidos con pérdida alta antes de actuar (~2 s).
const HIGH_STREAK_REPORTS: u32 = 4;
/// Peso del reporte nuevo en la media móvil.
const EWMA_ALPHA: f64 = 0.15;
/// Cada bajada deja el bitrate en este % del actual.
const STEP_DOWN_PERCENT: u64 = 80;
/// Cada subida lo deja en este % del actual (sin pasar del techo).
const STEP_UP_PERCENT: u64 = 115;
/// Piso, en % del bitrate inicial: por debajo de esto no se baja.
const FLOOR_PERCENT_OF_INITIAL: u64 = 35;
/// Tiempo mínimo entre cambios. Cada cambio reinicia el codificador.
const MIN_CHANGE_INTERVAL: Duration = Duration::from_secs(8);
/// Cuánto debe haber pasado tras una bajada para juzgar si sirvió.
const JUDGE_AFTER: Duration = Duration::from_secs(8);
/// La bajada "sirvió" si la pérdida quedó por debajo de este % de la anterior.
const HELPED_BELOW_PERCENT: f64 = 75.0;
/// Pérdida limpia sostenida antes de subir un escalón.
const CLEAN_BEFORE_UP: Duration = Duration::from_secs(45);
/// Bajadas seguidas sin mejora antes de concluir que la pérdida no depende del
/// bitrate (con un solo escalón del 20 % a veces no alcanza).
const MAX_UNHELPFUL_STEPS: u32 = 3;
/// Pausa tras concluir eso: no se vuelve a tocar el bitrate.
const FUTILE_HOLD: Duration = Duration::from_secs(300);

/// Qué decidió el controlador y por qué (para el log).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Decision {
    pub kbps: u64,
    pub reason: String,
}

/// Estado de una racha de bajadas: bitrate y pérdida de antes de la primera, y
/// cuántas bajadas seguidas no mejoraron la pérdida.
#[derive(Clone, Copy)]
struct Probe {
    base_kbps: u64,
    base_loss: f64,
    unhelpful: u32,
}

#[derive(Default)]
pub(super) struct LossController {
    ewma: Option<f64>,
    high_streak: u32,
    last_change: Option<Instant>,
    clean_since: Option<Instant>,
    /// Bajada en curso: se juzga si sirvió.
    probe: Option<Probe>,
    hold_until: Option<Instant>,
}

impl LossController {
    /// Pérdida suavizada actual (%), para el log.
    pub fn smoothed_percent(&self) -> Option<f64> {
        self.ewma
    }

    /// Otro mecanismo (el REMB) cambió el bitrate: se respeta el tiempo mínimo.
    pub fn note_external_change(&mut self, now: Instant) {
        self.last_change = Some(now);
        self.probe = None;
        self.high_streak = 0;
    }

    /// Procesa un reporte del servidor. `fraction_lost` va en 1/256. Devuelve el
    /// bitrate nuevo si hay que cambiarlo. `ceiling_kbps` es el techo (el bitrate
    /// configurado, o menos si el REMB lo limita).
    pub fn on_report(
        &mut self,
        now: Instant,
        fraction_lost: u8,
        current_kbps: u64,
        initial_kbps: u64,
        ceiling_kbps: u64,
    ) -> Option<Decision> {
        let loss = f64::from(fraction_lost) * 100.0 / 256.0;
        let smoothed = match self.ewma {
            Some(prev) => prev + EWMA_ALPHA * (loss - prev),
            // El primer reporte no puede disparar nada por sí solo.
            None => loss.min(LOSS_CLEAN_PERCENT),
        };
        self.ewma = Some(smoothed);

        self.high_streak = if smoothed >= LOSS_HIGH_PERCENT { self.high_streak + 1 } else { 0 };
        if smoothed <= LOSS_CLEAN_PERCENT {
            self.clean_since.get_or_insert(now);
        } else {
            self.clean_since = None;
        }

        let since_change = self.last_change.map(|at| now.duration_since(at));
        let can_change = since_change.is_none_or(|elapsed| elapsed >= MIN_CHANGE_INTERVAL);
        if !can_change {
            return None;
        }

        // ¿Sirvió la última bajada? Si varias seguidas no mejoraron nada, la pérdida
        // es del enlace: se deshace todo y se deja de insistir.
        if let Some(mut probe) = self.probe {
            if since_change.is_none_or(|elapsed| elapsed < JUDGE_AFTER) {
                return None;
            }
            if smoothed < LOSS_HIGH_PERCENT || smoothed <= probe.base_loss * HELPED_BELOW_PERCENT / 100.0 {
                self.probe = None;
            } else {
                probe.unhelpful += 1;
                if probe.unhelpful >= MAX_UNHELPFUL_STEPS {
                    self.probe = None;
                    self.hold_until = Some(now + FUTILE_HOLD);
                    self.last_change = Some(now);
                    self.high_streak = 0;
                    let target = probe.base_kbps.min(ceiling_kbps);
                    if target > current_kbps {
                        return Some(Decision {
                            kbps: target,
                            reason: format!(
                                "la pérdida ({smoothed:.1}%) no mejoró tras {} bajadas (antes {:.1}%): \
                                 es del enlace, no del bitrate; se vuelve a {target} kbps y se deja de ajustar {} s",
                                probe.unhelpful,
                                probe.base_loss,
                                FUTILE_HOLD.as_secs()
                            ),
                        });
                    }
                    return None;
                }
                self.probe = Some(probe);
            }
        }

        if self.hold_until.is_some_and(|until| now < until) {
            return None;
        }

        let floor = (initial_kbps * FLOOR_PERCENT_OF_INITIAL / 100).max(1);
        if self.high_streak >= HIGH_STREAK_REPORTS && current_kbps > floor {
            let target = (current_kbps * STEP_DOWN_PERCENT / 100).max(floor);
            if target < current_kbps {
                self.probe = Some(self.probe.unwrap_or(Probe {
                    base_kbps: current_kbps,
                    base_loss: smoothed,
                    unhelpful: 0,
                }));
                self.last_change = Some(now);
                self.high_streak = 0;
                self.clean_since = None;
                return Some(Decision {
                    kbps: target,
                    reason: format!(
                        "pérdida {smoothed:.1}% sostenida: baja de {current_kbps} a {target} kbps"
                    ),
                });
            }
        }

        let clean_long_enough = self.clean_since.is_some_and(|since| now.duration_since(since) >= CLEAN_BEFORE_UP);
        if clean_long_enough && current_kbps < ceiling_kbps {
            let target = (current_kbps * STEP_UP_PERCENT / 100).max(current_kbps + 1).min(ceiling_kbps);
            self.last_change = Some(now);
            self.clean_since = Some(now);
            return Some(Decision {
                kbps: target,
                reason: format!(
                    "pérdida {smoothed:.1}% baja durante {} s: sube de {current_kbps} a {target} kbps",
                    CLEAN_BEFORE_UP.as_secs()
                ),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fracción en 1/256 para un porcentaje.
    fn frac(percent: f64) -> u8 {
        (percent * 256.0 / 100.0).round() as u8
    }

    /// Alimenta reportes cada 0,5 s con `loss_of(t)` hasta `seconds`. Devuelve el
    /// bitrate final aplicando las decisiones.
    fn run(
        controller: &mut LossController,
        start: Instant,
        from_s: u64,
        to_s: u64,
        mut kbps: u64,
        ceiling: u64,
        loss_of: impl Fn(u64, u64) -> f64,
    ) -> (u64, Vec<Decision>) {
        let mut decisions = Vec::new();
        let mut t = from_s * 2;
        while t < to_s * 2 {
            let now = start + Duration::from_millis(t * 500);
            let loss = loss_of(t / 2, kbps);
            if let Some(decision) = controller.on_report(now, frac(loss), kbps, 6000, ceiling) {
                kbps = decision.kbps;
                decisions.push(decision);
            }
            t += 1;
        }
        (kbps, decisions)
    }

    #[test]
    fn ignores_isolated_loss_spikes() {
        let mut c = LossController::default();
        let start = Instant::now();
        let (kbps, decisions) = run(&mut c, start, 0, 60, 6000, 6000, |t, _| if t % 10 == 0 { 8.0 } else { 0.2 });
        assert_eq!(kbps, 6000);
        assert!(decisions.is_empty());
    }

    #[test]
    fn steps_down_when_loss_depends_on_bitrate() {
        let mut c = LossController::default();
        let start = Instant::now();
        // Con más de 3500 kbps se pierde el 4 %; por debajo, casi nada.
        let (kbps, decisions) = run(&mut c, start, 0, 60, 6000, 6000, |_, k| if k > 3500 { 4.0 } else { 0.2 });
        assert!(kbps <= 3500, "kbps={kbps}");
        assert!(kbps >= 2100, "no debe bajar del piso: {kbps}");
        assert!(decisions.iter().all(|d| d.kbps < 6000));
    }

    #[test]
    fn reverts_and_holds_when_loss_is_independent_of_bitrate() {
        let mut c = LossController::default();
        let start = Instant::now();
        let (kbps, decisions) = run(&mut c, start, 0, 100, 6000, 6000, |_, _| 3.5);
        // Tres bajadas sin mejora, luego se deshace y no vuelve a tocarse durante la pausa.
        assert_eq!(kbps, 6000, "{decisions:?}");
        assert_eq!(decisions.len(), 4, "{decisions:?}");
        assert!(decisions[3].reason.contains("no mejoró"));
    }

    #[test]
    fn recovers_when_link_gets_clean() {
        let mut c = LossController::default();
        let start = Instant::now();
        let (low, _) = run(&mut c, start, 0, 30, 6000, 6000, |_, k| if k > 3500 { 4.0 } else { 0.1 });
        assert!(low < 6000);
        let (high, _) = run(&mut c, start, 30, 400, low, 6000, |_, _| 0.1);
        assert_eq!(high, 6000);
    }

    #[test]
    fn never_exceeds_ceiling() {
        let mut c = LossController::default();
        let start = Instant::now();
        let (kbps, _) = run(&mut c, start, 0, 300, 2000, 2500, |_, _| 0.0);
        assert_eq!(kbps, 2500);
    }

    #[test]
    fn external_change_respects_min_interval() {
        let mut c = LossController::default();
        let start = Instant::now();
        c.note_external_change(start);
        // 4 s de pérdida alta justo tras el cambio externo: no actúa.
        let (kbps, decisions) = run(&mut c, start, 0, 4, 4000, 6000, |_, _| 5.0);
        assert_eq!(kbps, 4000);
        assert!(decisions.is_empty());
    }
}