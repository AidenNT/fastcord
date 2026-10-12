//! Parte visual de las actualizaciones automáticas (la lógica vive en
//! `crate::updater`).
//!
//! * `poll` se llama una vez por frame desde `App::ui`: arranca el updater en
//!   el primer frame, confirma al ayudante que una versión recién instalada
//!   arrancó bien, avisa si hubo que restaurar la anterior, y abre un
//!   diálogo cuando hay una versión lista (o una que hay que bajar a mano).
//! * `section` dibuja el estado en Ajustes → Cuenta → Acerca de.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::lib::state::{App, ToastKind};
use crate::theme::{self, Icon, Palette};
use crate::ui::shell::dialog::{ButtonStyle, Dialog, DialogKind};
use crate::updater::{self, View};

static STARTED: AtomicBool = AtomicBool::new(false);
/// Última versión para la que ya se abrió un diálogo (no insistir en cada frame).
static PROMPTED: Mutex<Option<String>> = Mutex::new(None);

/// Acción pedida desde Ajustes.
#[derive(Clone, Debug)]
pub enum UiAction {
    CheckNow,
    Restart,
    OpenPage(String),
}

/// Llamar una vez por frame, al final de `App::ui` (el primer frame ya se
/// dibujó, que es cuando hay que confirmar la actualización).
pub fn poll(app: &mut App, ctx: &egui::Context) {
    if !STARTED.swap(true, Ordering::SeqCst) {
        let repaint_ctx = ctx.clone();
        updater::start(move || repaint_ctx.request_repaint());
        if updater::take_restore_error().is_some() {
            app.push_toast(
                ToastKind::Warning,
                "No se pudo actualizar",
                "La versión nueva no arrancó y se restauró la anterior.",
            );
        }
        if updater::acknowledge_receipt() {
            app.push_toast(
                ToastKind::Success,
                "eCord actualizado",
                format!("Ahora estás en la versión {}.", env!("CARGO_PKG_VERSION")),
            );
        }
    }

    let (key, dialog) = match updater::view() {
        View::Ready { version } => {
            let dialog = ready_dialog(&version);
            (format!("ready:{version}"), dialog)
        }
        View::Manual { version, url, reason } => {
            let dialog = manual_dialog(&version, &url, &reason);
            (format!("manual:{version}"), dialog)
        }
        _ => return,
    };
    let mut prompted = PROMPTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if prompted.as_deref() != Some(key.as_str()) {
        *prompted = Some(key);
        drop(prompted);
        app.open_dialog(dialog);
    }
}

fn ready_dialog(version: &str) -> Dialog {
    Dialog::confirm(
        "update_ready",
        DialogKind::Sparkle,
        "Actualización lista",
        format!(
            "eCord {version} ya se descargó y se verificó. Al reiniciar se instala y eCord \
             vuelve a abrirse solo. Si estás en una llamada, se corta."
        ),
        "Reiniciar ahora",
    )
    .on_result(|app, ctx, result| {
        if result.is("confirm") {
            apply(app, ctx, UiAction::Restart);
        }
    })
}

fn manual_dialog(version: &str, url: &str, reason: &str) -> Dialog {
    let url = url.to_owned();
    Dialog::new("update_manual", DialogKind::Info, "Hay una versión nueva")
        .subtitle(format!("eCord {version}"))
        .text(reason.to_owned())
        .button("later", "Más tarde", ButtonStyle::Secondary)
        .button("open", "Ver descarga", ButtonStyle::Primary)
        .dismissable()
        .on_result(move |app, ctx, result| {
            if result.is("open") {
                apply(app, ctx, UiAction::OpenPage(url));
            }
        })
}

/// Ejecuta una acción de la UI.
pub fn apply(_app: &mut App, ctx: &egui::Context, action: UiAction) {
    match action {
        UiAction::CheckNow => {
            updater::check_now();
            ctx.request_repaint();
        }
        UiAction::Restart => {
            let repaint = ctx.clone();
            let quit = ctx.clone();
            updater::install_and_quit(
                move || repaint.request_repaint(),
                move || quit.send_viewport_cmd(egui::ViewportCommand::Close),
            );
        }
        UiAction::OpenPage(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
    }
}

/// Estado y botones para Ajustes → Cuenta → Acerca de.
pub fn section(ui: &mut egui::Ui, palette: &Palette) -> Option<UiAction> {
    let mut action = None;
    let (message, button): (String, Option<(Icon, &str, UiAction)>) = match updater::view() {
        View::Idle | View::UpToDate => (
            "Tenés la última versión.".into(),
            Some((Icon::Refresh, "Buscar actualizaciones", UiAction::CheckNow)),
        ),
        View::Checking => ("Buscando actualizaciones...".into(), None),
        View::CheckFailed(error) => (
            format!("No se pudo buscar actualizaciones: {error}"),
            Some((Icon::Refresh, "Reintentar", UiAction::CheckNow)),
        ),
        View::Manual { version, url, reason } => (
            format!("Hay una versión nueva ({version}). {reason}"),
            Some((Icon::Download, "Ver descarga", UiAction::OpenPage(url))),
        ),
        View::Downloading { version, received, total } => {
            let progress = if total > 0 {
                format!(" ({} %)", received.saturating_mul(100) / total)
            } else {
                String::new()
            };
            (format!("Descargando la versión {version}{progress}..."), None)
        }
        View::Ready { version } => (
            format!("La versión {version} está lista para instalar."),
            Some((Icon::Download, "Reiniciar para actualizar", UiAction::Restart)),
        ),
        View::Installing => ("Instalando la actualización...".into(), None),
        View::Failed(error) => (
            format!("No se pudo actualizar: {error}"),
            Some((Icon::Refresh, "Reintentar", UiAction::CheckNow)),
        ),
    };
    theme::text(ui, message, theme::regular(12.5), palette.dim);
    if let Some((icon, label, taken)) = button {
        ui.add_space(8.0);
        if theme::soft_button(ui, palette, Some(icon), label, false).clicked() {
            action = Some(taken);
        }
    }
    action
}
