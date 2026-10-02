//! Overlays flotantes: notificaciones apiladas en una esquina y los popups
//! modales. Los modales se dibujan con el sistema de diálogos unificado
//! (`ui::dialog`), así que comparten diseño con los avisos, las novedades y
//! el resto de diálogos del cliente.
//!
//! Se dibujan al final de `App::ui`, después de la pantalla activa, para
//! quedar siempre por encima de todo lo demás.

use crate::lib::state::App;
use crate::ui::dialog::{self, Block, ButtonStyle, Dialog, DialogKind, FormField};

/// Los avisos flotantes (`App::toasts`) ahora se dibujan junto con las
/// tarjetas de mención, en una sola pila con el diseño del overlay de
/// Nullscape: ver `ui::notifications::show_in_app`. Se deja esta función
/// (vacía) para no tocar el punto de llamada en `App::ui`.
pub fn show_toasts(_app: &mut App, _ui: &mut egui::Ui) {}

/// Popup modal informativo (`App::modal`): título, mensaje y un botón de
/// confirmación. Bloquea el resto de la ventana mientras está abierto; se
/// cierra con el botón o con Escape.
pub fn show_modal(app: &mut App, ui: &mut egui::Ui) {
    // Si hay un diálogo de la cola, ese tiene prioridad (se muestra de a uno).
    if !app.dialogs.is_empty() {
        return;
    }
    let Some(modal) = &app.modal else {
        return;
    };
    let mut dialog = Dialog::new("app_modal", DialogKind::Info, modal.title.clone())
        .text(modal.message.clone())
        .button("ok", modal.confirm_label.clone(), ButtonStyle::Primary)
        .dismiss_on_escape(true);

    let ctx = ui.ctx().clone();
    let palette = app.palette;
    if dialog::render(&ctx, &palette, &mut dialog).is_some() {
        app.close_modal();
        dialog::reset_anim(&ctx, "app_modal");
    }
}

/// Formulario que pidió un bot tras apretar uno de sus botones (por ejemplo
/// "Responder anónimo" pide el texto de la respuesta). Se arma un diálogo
/// transitorio con los campos de `App::component_modal` en cada frame; lo
/// escrito vuelve a esos campos y el envío (con su validación) lo sigue
/// haciendo `App::submit_component_modal`.
pub fn show_component_modal(app: &mut App, ui: &mut egui::Ui) {
    if !app.dialogs.is_empty() {
        return;
    }
    let Some(modal) = app.component_modal.as_ref() else {
        return;
    };

    let mut dialog = Dialog::new("component_modal", DialogKind::Question, modal.request.title.clone())
        .width(460.0)
        .dismiss_on_escape(true)
        // "La aplicación <foto> <nombre> solicitó que llenes un formulario".
        .app_banner(
            modal.request.app_name.clone().unwrap_or_else(|| "Aplicación desconocida".to_owned()),
            modal.request.app_icon_url.clone(),
            "solicitó que llenes un formulario",
        )
        // Leyenda de seguridad: el contenido lo recibe el bot, no Discord.
        .callout(
            DialogKind::Warning,
            "No compartas datos sensibles",
            "Este formulario lo envía la aplicación, no Discord. Nunca ingreses contraseñas, \
             tokens, códigos de verificación (2FA), datos de tarjetas ni información personal \
             o financiera.",
        );
    for (index, field) in modal.fields.iter().enumerate() {
        // El asterisco es solo visual: la validación real (obligatorio,
        // largo mínimo) la hace `ComponentModal::validate` al enviar.
        let label = if field.required && !field.label.is_empty() {
            format!("{} *", field.label)
        } else {
            field.label.clone()
        };
        dialog = dialog.field(
            FormField::new(index.to_string(), label)
                .placeholder(field.placeholder.clone())
                .value(field.value.clone())
                .multiline(field.paragraph)
                .max_len(field.max_length),
        );
    }
    if modal.fields.is_empty() {
        dialog = dialog.text("Este formulario no tiene campos que ecord pueda mostrar.");
    }
    if let Some(error) = &modal.error {
        dialog = dialog.error(error.clone());
    }
    let mut dialog = dialog
        .button("cancel", "Cancelar", ButtonStyle::Secondary)
        .button("submit", "Enviar", ButtonStyle::Primary);
    // La validación la hace `submit_component_modal`, no el diálogo.
    for button in &mut dialog.buttons {
        button.validate = false;
    }

    let ctx = ui.ctx().clone();
    let palette = app.palette;
    let result = dialog::render(&ctx, &palette, &mut dialog);

    // Lo que se escribió este frame vuelve a los campos reales.
    if let Some(modal) = app.component_modal.as_mut() {
        let edited = dialog.blocks.iter().filter_map(|block| match block {
            Block::Field(field) => Some(field),
            _ => None,
        });
        for (field, edited) in modal.fields.iter_mut().zip(edited) {
            field.value = edited.value.clone();
        }
    }

    if let Some(result) = result {
        if result.is("submit") {
            app.submit_component_modal();
        } else {
            app.component_modal = None;
        }
    }
    if app.component_modal.is_none() {
        dialog::reset_anim(&ctx, "component_modal");
    }
}
