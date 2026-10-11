//! Overlays flotantes: notificaciones apiladas en una esquina y los popups
//! modales. Los modales se dibujan con el sistema de diálogos unificado
//! (`ui::shell::dialog`), así que comparten diseño con los avisos, las novedades y
//! el resto de diálogos del cliente.
//!
//! Se dibujan al final de `App::ui`, después de la pantalla activa, para
//! quedar siempre por encima de todo lo demás.

use crate::discord::join_form::{self, FieldKind};
use crate::lib::state::{App, JoinFormModal};
use crate::ui::shell::dialog::{self, Block, ButtonStyle, CheckField, ChoiceField, Dialog, DialogKind, FormField};

/// Los avisos flotantes (`App::toasts`) ahora se dibujan junto con las
/// tarjetas de mención, en una sola pila con el diseño del overlay de
/// Nullscape: ver `ui::shell::notifications::show_in_app`. Se deja esta función
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
            modal
                .request
                .app_name
                .clone()
                .unwrap_or_else(|| "Aplicación desconocida".to_owned()),
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
    let mut dialog =
        dialog
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

/// Formulario de ingreso que pide un server tras unirte con una invitación
/// (reglas para aceptar, preguntas abiertas, opción múltiple). Es el mismo
/// diálogo que el formulario de una aplicación, pero el encabezado dice "El
/// servidor <foto> <nombre> solicitó que llenes un formulario para unirte".
///
/// Igual que `show_component_modal`: se arma un diálogo transitorio con las
/// respuestas guardadas en `App::join_form` en cada frame, y lo editado vuelve
/// a esos campos. La validación y el envío los hace `App::submit_join_form`.
pub fn show_join_form(app: &mut App, ui: &mut egui::Ui) {
    if !app.dialogs.is_empty() {
        return;
    }
    let Some(modal) = app.join_form.as_ref() else {
        return;
    };
    let form = &modal.form;

    let mut dialog = Dialog::new("join_form", DialogKind::Question, "Formulario de ingreso")
        .width(480.0)
        .dismiss_on_escape(true)
        // "El servidor <foto> <nombre> solicitó que llenes un formulario para unirte".
        .banner(
            "El servidor",
            form.guild_name.clone(),
            form.guild_icon_url.clone(),
            "solicitó que llenes un formulario para unirte",
        )
        .callout(
            DialogKind::Info,
            "Tus respuestas las recibe el servidor",
            "Los moderadores del servidor pueden ver lo que respondas. Nunca ingreses \
             contraseñas, tokens, códigos de verificación (2FA) ni datos de tarjetas.",
        );
    if let Some(description) = &form.description {
        dialog = dialog.text(description.clone());
    }
    for (index, field) in form.fields.iter().enumerate() {
        // El id de cada bloque es su posición: así lo editado vuelve a su campo.
        let id = index.to_string();
        // El asterisco es solo visual; la validación real la hace
        // `JoinForm::validate` al enviar.
        let label = if field.required && !field.label.is_empty() {
            format!("{} *", field.label)
        } else {
            field.label.clone()
        };
        match &field.kind {
            FieldKind::Terms => {
                // El título de este campo es siempre el texto estándar de
                // Discord ("Read and agree to the server rules"), en inglés:
                // se reemplaza por uno propio.
                if let Some(description) = &field.description {
                    dialog = dialog.text(description.clone());
                }
                if !field.values.is_empty() {
                    let rules = field
                        .values
                        .iter()
                        .enumerate()
                        .map(|(i, rule)| format!("{}. {rule}", i + 1))
                        .collect::<Vec<_>>()
                        .join("\n");
                    dialog = dialog.callout(DialogKind::Question, "Reglas del servidor", rules);
                }
                dialog = dialog.check(
                    CheckField::new(id, "Leí y acepto las reglas del servidor")
                        .checked(field.accepted)
                        .required(field.required),
                );
            }
            FieldKind::ShortText | FieldKind::Paragraph => {
                if let Some(description) = &field.description {
                    dialog = dialog.note(description.clone());
                }
                dialog = dialog.field(
                    FormField::new(id, label)
                        .value(field.text.clone())
                        .multiline(field.kind == FieldKind::Paragraph)
                        .max_len(join_form::MAX_ANSWER_LEN),
                );
            }
            FieldKind::Choice => {
                if let Some(description) = &field.description {
                    dialog = dialog.note(description.clone());
                }
                dialog = dialog.choice(
                    ChoiceField::new(id, field.label.clone())
                        .options(field.values.clone())
                        .selected(field.choice)
                        .required(field.required),
                );
            }
            FieldKind::Unsupported(_) => {
                let name = if field.label.is_empty() {
                    "una pregunta"
                } else {
                    field.label.as_str()
                };
                dialog = dialog.note(format!(
                    "Este formulario incluye {name}, un tipo de pregunta que ecord todavía no puede mostrar."
                ));
            }
        }
    }
    if let Some(error) = &modal.error {
        dialog = dialog.error(error.clone());
    }
    let submit_label = if modal.submitting { "Enviando…" } else { "Enviar" };
    let mut dialog = dialog.button("cancel", "Cancelar", ButtonStyle::Secondary).button(
        "submit",
        submit_label,
        ButtonStyle::Primary,
    );
    // La validación la hace `submit_join_form`, no el diálogo.
    for button in &mut dialog.buttons {
        button.validate = false;
    }

    let ctx = ui.ctx().clone();
    let palette = app.palette;
    let result = dialog::render(&ctx, &palette, &mut dialog);

    // Lo que se escribió o marcó este frame vuelve a los campos reales.
    if let Some(modal) = app.join_form.as_mut() {
        for block in &dialog.blocks {
            match block {
                Block::Field(edited) => {
                    if let Some(field) = join_field_at(modal, &edited.id) {
                        field.text = edited.value.clone();
                    }
                }
                Block::Check(edited) => {
                    if let Some(field) = join_field_at(modal, &edited.id) {
                        field.accepted = edited.checked;
                    }
                }
                Block::Choice(edited) => {
                    if let Some(field) = join_field_at(modal, &edited.id) {
                        field.choice = edited.selected;
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(result) = result {
        if result.is("submit") {
            app.submit_join_form();
        } else {
            // Cancelar, Escape o la ✕.
            app.join_form = None;
        }
    }
    if app.join_form.is_none() {
        dialog::reset_anim(&ctx, "join_form");
    }
}

/// La pregunta del formulario de ingreso a la que corresponde el bloque con
/// `id` (en `show_join_form` el id de cada bloque es su posición).
fn join_field_at<'a>(modal: &'a mut JoinFormModal, id: &str) -> Option<&'a mut join_form::JoinField> {
    let index: usize = id.parse().ok()?;
    modal.form.fields.get_mut(index)
}
