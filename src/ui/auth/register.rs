//! Formulario para crear una cuenta nueva sin salir del cliente
//! (`AuthStatus::RegisterForm`). La lógica (validación, pedido a
//! `/auth/register`, captcha) vive en `App::submit_register` y
//! `discord::register`; esto solo dibuja los campos con el mismo aspecto que
//! el login (`login::styled_input`, `login::field_label`...).
//!
//! Mientras se crea la cuenta (`AuthStatus::RegisterSubmitting`) lo que se ve
//! es el splash de `login::show`, que también avisa si hay un captcha
//! esperando a la persona.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, Vec2};

use crate::lib::state::App;
use crate::ui::auth::login::{field_label, styled_input, text_link, wide_button};
use crate::ui::theme::{self, Palette};

const TERMS_URL: &str = "https://discord.com/terms";
const PRIVACY_URL: &str = "https://discord.com/privacy";

/// Tarjeta centrada con scroll (el formulario es más alto que el login).
pub fn show(app: &mut App, ui: &mut egui::Ui, rect: Rect, palette: &Palette) {
    let w = 460.0f32.min(rect.width() - 32.0).max(280.0);
    let area = Rect::from_min_max(
        egui::pos2(rect.center().x - w / 2.0 - 4.0, rect.min.y + 12.0),
        egui::pos2(rect.center().x + w / 2.0 + 4.0, rect.max.y - 44.0),
    );
    let mut scroll_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(Layout::top_down(Align::Center)),
    );
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut scroll_ui, |ui| {
            // Centrado vertical aproximado cuando sobra lugar.
            ui.add_space(((area.height() - 800.0) / 2.0).max(0.0));
            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(18))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 24],
                    blur: 60,
                    spread: 0,
                    color: palette.shadow,
                })
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.set_width(w - 8.0 - 48.0);
                    form(app, ui, palette);
                });
            ui.add_space(12.0);
        });
}

fn form(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    // Después del primer intento fallido de mandar, los campos vacíos se
    // marcan en rojo (igual que en el login).
    let tried_id = egui::Id::new("ecord_register_tried");
    let tried = ui.ctx().data(|d| d.get_temp::<bool>(tried_id)).unwrap_or(false);
    // Calculados antes de prestar los campos como `&mut` a los inputs.
    let miss_email = tried && app.register_email.trim().is_empty();
    let miss_username = tried && app.register_username.trim().is_empty();
    let miss_password = tried && app.register_password.is_empty();

    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
            theme::logo(ui, logo_rect.center(), 32.0, palette.accent, palette.on_accent);
            theme::text(ui, "eCord", theme::bold(18.0), palette.text);
        });
        ui.add_space(20.0);

        ui.vertical_centered(|ui| {
            theme::text(ui, "Crea una cuenta", theme::bold(24.0), palette.text);
            ui.add_space(6.0);
            theme::text(
                ui,
                "Se registra directamente en Discord",
                theme::regular(14.0),
                palette.secondary,
            );
        });
        ui.add_space(20.0);

        if let Some(error) = app.register_error.clone() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(error)
                        .font(theme::regular(12.5))
                        .color(palette.danger),
                )
                .wrap(),
            );
            ui.add_space(10.0);
        }

        field_label(ui, palette, "CORREO ELECTRÓNICO", miss_email);
        ui.add_space(8.0);
        let email = styled_input(ui, palette, &mut app.register_email, false, miss_email);
        ui.add_space(16.0);

        theme::text(ui, "NOMBRE PARA MOSTRAR (OPCIONAL)", theme::semibold(12.0), palette.secondary);
        ui.add_space(8.0);
        let display = styled_input(ui, palette, &mut app.register_display_name, false, false);
        ui.add_space(16.0);

        field_label(ui, palette, "NOMBRE DE USUARIO", miss_username);
        ui.add_space(8.0);
        let username = styled_input(ui, palette, &mut app.register_username, false, miss_username);
        ui.add_space(16.0);

        field_label(ui, palette, "CONTRASEÑA", miss_password);
        ui.add_space(8.0);
        let password = styled_input(ui, palette, &mut app.register_password, true, miss_password);
        ui.add_space(16.0);

        field_label(ui, palette, "FECHA DE NACIMIENTO", false);
        ui.add_space(8.0);
        let birth = birth_row(app, ui, palette, tried);
        ui.add_space(4.0);
        theme::text(ui, "Día  /  Mes  /  Año", theme::regular(11.5), palette.dim);
        ui.add_space(16.0);

        ui.add(egui::Checkbox::new(
            &mut app.register_consent,
            egui::RichText::new("He leído y acepto los Términos de servicio y la Política de privacidad de Discord")
                .font(theme::regular(13.0))
                .color(palette.secondary),
        ));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.add_space(26.0);
            if text_link(ui, palette, "Términos de servicio") {
                ui.ctx().open_url(egui::OpenUrl::new_tab(TERMS_URL));
            }
            if text_link(ui, palette, "Política de privacidad") {
                ui.ctx().open_url(egui::OpenUrl::new_tab(PRIVACY_URL));
            }
        });
        ui.add_space(8.0);
        ui.add(egui::Checkbox::new(
            &mut app.register_promo_emails,
            egui::RichText::new("(Opcional) Quiero recibir novedades y promociones de Discord por correo")
                .font(theme::regular(13.0))
                .color(palette.secondary),
        ));
        ui.add_space(20.0);

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let clicked = wide_button(ui, palette, "Crear cuenta");
        let focus_left = email.lost_focus()
            || display.lost_focus()
            || username.lost_focus()
            || password.lost_focus()
            || birth.iter().any(|r| r.lost_focus());
        if clicked || (enter_pressed && focus_left) {
            ui.ctx().data_mut(|d| d.insert_temp(tried_id, true));
            app.submit_register();
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            theme::text(ui, "¿Ya tienes una cuenta?", theme::regular(13.0), palette.dim);
            if text_link(ui, palette, "Inicia sesión") {
                ui.ctx().data_mut(|d| d.remove_temp::<bool>(tried_id));
                app.cancel_register();
            }
        });
    });
}

/// Los tres campos de la fecha de nacimiento (día, mes, año): solo dígitos y
/// con largo máximo 2/2/4.
fn birth_row(app: &mut App, ui: &mut egui::Ui, palette: &Palette, tried: bool) -> [egui::Response; 3] {
    let spacing = 10.0;
    let total = ui.available_width() - spacing * 2.0;
    let small = 84.0f32.min(total / 3.0);
    let year_w = (total - small * 2.0).max(small);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = spacing;
        let day = digits_input(ui, palette, small, &mut app.register_birth_day, 2, tried);
        let month = digits_input(ui, palette, small, &mut app.register_birth_month, 2, tried);
        let year = digits_input(ui, palette, year_w, &mut app.register_birth_year, 4, tried);
        [day, month, year]
    })
    .inner
}

fn digits_input(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    text: &mut String,
    max_len: usize,
    tried: bool,
) -> egui::Response {
    let missing = tried && text.trim().is_empty();
    let response = ui
        .allocate_ui(Vec2::new(width, 42.0), |ui| styled_input(ui, palette, text, false, missing))
        .inner;
    if response.changed() {
        text.retain(|c| c.is_ascii_digit());
        text.truncate(max_len);
    }
    response
}
