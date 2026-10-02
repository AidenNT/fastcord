//! Novedades de ecord ("changelog"), mostradas con el sistema `ui::dialog`.
//!
//! - Al arrancar con una versión nueva (`CARGO_PKG_VERSION`) aparece solo
//!   una vez, con la versión más reciente (`queue_if_new`).
//! - Desde Ajustes → Cuenta → "Ver novedades" se abre el historial completo
//!   (`App::open_changelog`).
//!
//! Para agregar una versión, súbela al principio de `releases()`.

use crate::lib::state::App;
use crate::ui::dialog::{ButtonStyle, Dialog, DialogKind, Release};

/// Clave de `web_local_storage_api` con la última versión cuyas novedades ya
/// se mostraron.
const STORAGE_KEY: &str = "last_seen_changelog";

/// Historial de versiones, de la más nueva a la más vieja.
///
/// NOTA: el contenido de abajo es el que corresponde a los cambios de este
/// parche; edítalo con las novedades reales de cada versión.
pub fn releases() -> Vec<Release> {
    vec![
        Release::new(env!("CARGO_PKG_VERSION"))
            .date("2 oct 2026")
            .title("Seguridad y diálogos")
            .added(
                "Aviso de seguridad al abrir ecord con ECORD_USE_MOCK_URL o \
                 ECORD_INSECURE_TLS activos: tu token podría quedar expuesto.",
            )
            .added("Nuevo sistema de diálogos para avisos, confirmaciones, formularios y novedades.")
            .improved(
                "Con una sesión guardada en modo inseguro, ecord ya no se conecta hasta que \
                 aceptes el aviso.",
            )
            .improved("Mientras hay un diálogo abierto, el teclado ya no llega a la pantalla de atrás."),
    ]
}

/// Diálogo con las últimas `count` versiones (`usize::MAX` = todas).
pub fn dialog(count: usize) -> Dialog {
    let releases: Vec<Release> = releases().into_iter().take(count).collect();
    let latest = releases
        .first()
        .map(|r| r.version.clone())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());

    Dialog::new("changelog", DialogKind::Sparkle, "Novedades")
        .subtitle(format!("eCord v{latest}"))
        .width(520.0)
        .changelog(releases)
        .button("ok", "Genial", ButtonStyle::Primary)
        .dismissable()
}

/// Encola las novedades de la versión actual si todavía no se vieron. Al
/// cerrarse (por el botón o descartándolas) se recuerda la versión, así que
/// no vuelven a salir hasta la próxima actualización.
pub fn queue_if_new(app: &mut App) {
    let current = env!("CARGO_PKG_VERSION");
    let seen = web_local_storage_api::get_item(STORAGE_KEY).ok().flatten();
    if seen.as_deref() == Some(current) {
        return;
    }
    app.open_dialog(dialog(1).on_result(|_app, _ctx, _result| {
        let _ = web_local_storage_api::set_item(STORAGE_KEY, env!("CARGO_PKG_VERSION"));
    }));
}
