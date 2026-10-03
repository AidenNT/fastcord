//! Aviso de seguridad al abrir ecord con alguna de las variables de
//! depuración peligrosas activas:
//!
//! - `ECORD_USE_MOCK_URL`: el Gateway se conecta a un servidor "mock" en vez
//!   de a Discord, y ese servidor recibe el token en el `IDENTIFY`.
//! - `ECORD_INSECURE_TLS=1`: se apaga la verificación de certificados TLS, así
//!   que cualquiera que esté en medio de la conexión (MITM) puede leer el
//!   token.
//!
//! Es un diálogo más del sistema `ui::dialog` (sin cierre por Escape ni por
//! clic afuera, para que la decisión sea deliberada). `App::default` lo mete
//! en la cola y, mientras no se acepte, la sesión guardada NO se conecta
//! (queda en `App::pending_resume_token`).

use crate::ui::dialog::{ButtonStyle, Dialog, DialogKind};
use crate::ui::theme::Icon;

/// Máximo de caracteres de la URL del mock que se muestran en el aviso.
const MAX_URL_CHARS: usize = 46;

/// Lee las variables de entorno con las MISMAS funciones que usa el cliente
/// para activar cada modo, para que el aviso nunca discrepe de lo que
/// realmente está pasando. `None` si no hay nada que avisar.
pub fn detect() -> Option<Dialog> {
    let mock_url = crate::discord::gateway::use_mock_url().then(crate::discord::gateway::mock_url);
    let insecure_tls = discord_client_rest::insecure_tls_enabled();

    if mock_url.is_none() && !insecure_tls {
        return None;
    }
    log::warn!(
        "Modo inseguro activo (mock_url={mock_url:?}, insecure_tls={insecure_tls}): \
         el token podría quedar expuesto"
    );

    let mut dialog = Dialog::new("security_warning", DialogKind::Danger, "Conexión no segura")
        .subtitle("ecord detectó variables de depuración activas")
        .pulse()
        .text(
            "Usar el cliente con estas variables activas puede provocar que tu token de \
             Discord sea robado. Con tu token, cualquiera puede entrar a tu cuenta sin \
             necesitar tu contraseña ni el 2FA.",
        );

    if let Some(url) = &mock_url {
        dialog = dialog.flag(
            "ECORD_USE_MOCK_URL",
            format!(
                "El Gateway se conecta a {} en lugar de a Discord. Ese servidor recibe tu \
                 token al iniciar sesión y puede guardarlo o reutilizarlo.",
                shorten(url, MAX_URL_CHARS)
            ),
        );
    }
    if insecure_tls {
        dialog = dialog.flag(
            "ECORD_INSECURE_TLS",
            "La verificación de certificados TLS está desactivada. Cualquiera en tu red \
             (Wi-Fi público, proxy, malware) puede hacerse pasar por Discord y leer tu \
             token en tránsito.",
        );
    }

    Some(
        dialog
            .note_icon(
                Icon::Lock,
                "Para usar ecord de forma segura, elimina estas variables de entorno y \
                 vuelve a abrirlo.",
            )
            .button("continue", "Entiendo el riesgo, continuar", ButtonStyle::Secondary)
            .button("close", "Cerrar ecord", ButtonStyle::Primary)
            .on_result(|app, ctx, result| {
                if result.is("continue") {
                    // Recién ahora, con el riesgo aceptado, se conecta la
                    // sesión guardada.
                    if let Some(token) = app.pending_resume_token.take() {
                        app.resume_saved_session(token);
                    }
                } else {
                    // "Cerrar ecord" (o cualquier otra salida): el token no
                    // llega a salir nunca.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }),
    )
}

/// Suena la alerta (`assets/sounds/alert.mp3`) al mostrarse el aviso. No
/// bloquea la UI y, si el audio falla, el diálogo se ve igual.
pub fn play_alert() {
    crate::support::sound_alerts::play(crate::support::sound_alerts::SoundAlert::SecurityWarning);
}

/// Recorta `text` a `max` caracteres añadiendo `…` (sin partir un carácter
/// multibyte).
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
