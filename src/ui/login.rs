use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Stroke, Vec2};

use crate::lib::state::{App, AuthStatus};
use crate::ui::extra;
use crate::ui::theme;

/// Contador para que cada QR generado tenga una URI "bytes://" distinta;
/// si reusáramos siempre la misma, el loader de imágenes de egui podría
/// quedarse con el SVG viejo cacheado al generar un QR nuevo (reintentos).
static QR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;

    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();

            // El degradado propio del login solo con fondo sólido: si el tema
            // trae degradado/imagen, se deja ver ese.
            if app.backdrop.kind == crate::theme::BackdropKind::Solid {
                let top = extra::blend(palette.window, palette.accent, 0.10);
                extra::paint_vertical_gradient(ui, rect, top, palette.window);
            }

            let card_width = 440.0;
            let card_height = 460.0;
            let card = Rect::from_center_size(
                rect.center() - Vec2::new(0.0, 20.0),
                Vec2::new(card_width, card_height),
            );

            let mut card_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(card)
                    .layout(Layout::top_down(Align::Center)),
            );

            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 12))
                .inner_margin(Margin::same(36))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 16],
                    blur: 48,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(&mut card_ui, |ui| {
                    ui.set_width(card_width - 72.0);
                    ui.spacing_mut().item_spacing.y = 8.0;

                    let (logo_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
                    theme::logo(ui, logo_rect.center(), 56.0, palette.accent, palette.on_accent);

                    ui.add_space(6.0);
                    theme::text(ui, "eCord", theme::bold(28.0), palette.text);
                    let subtitle = match &app.auth {
                        AuthStatus::PasswordForm | AuthStatus::PasswordSubmitting => {
                            "Iniciá sesión con tu usuario y contraseña"
                        }
                        AuthStatus::PasswordMfaRequired | AuthStatus::PasswordMfaSubmitting => {
                            "Verificación en dos pasos"
                        }
                        _ => "Iniciá sesión escaneando el código QR",
                    };
                    theme::text(ui, subtitle, theme::regular(13.5), palette.secondary);
                    ui.add_space(18.0);

                    match &app.auth {
                        AuthStatus::SignedOut => {
                            if big_button(ui, &palette, "Iniciar sesión") {
                                app.start_sign_in();
                            }
                            ui.add_space(10.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(
                                        "En Discord (celular): tocá tu avatar → Escanear código QR, \
                                         o el ícono de QR en la pantalla de login.",
                                    )
                                    .font(theme::regular(12.0))
                                    .color(palette.secondary),
                                )
                                .wrap(),
                            );
                            ui.add_space(14.0);
                            if text_link(ui, &palette, "Usar usuario y contraseña") {
                                app.switch_to_password_form();
                            }
                        }
                        AuthStatus::PasswordForm => {
                            password_form(app, ui, &palette);
                        }
                        AuthStatus::PasswordSubmitting => {
                            spinner_row(ui, &palette, "Iniciando sesión…");
                        }
                        AuthStatus::PasswordMfaRequired => {
                            mfa_form(app, ui, &palette);
                        }
                        AuthStatus::PasswordMfaSubmitting => {
                            spinner_row(ui, &palette, "Verificando código…");
                        }
                        AuthStatus::Starting => {
                            spinner_row(ui, &palette, "Generando código QR…");
                        }
                        AuthStatus::AwaitingScan { url } => {
                            qr_code(ui, &palette, url);
                            ui.add_space(10.0);
                            theme::text(
                                ui,
                                "Escaneá este código con la app de Discord en tu celular",
                                theme::medium(13.0),
                                palette.text,
                            );
                            ui.add_space(10.0);
                            if theme::pill_button(ui, &palette, "Cancelar", false).clicked() {
                                app.cancel_sign_in();
                            }
                        }
                        AuthStatus::Confirming { username } => {
                            spinner_row(ui, &palette, "Confirmá el inicio de sesión en tu celular…");
                            ui.add_space(6.0);
                            theme::text(ui, username, theme::semibold(13.5), palette.text);
                            ui.add_space(10.0);
                            if theme::pill_button(ui, &palette, "Cancelar", false).clicked() {
                                app.cancel_sign_in();
                            }
                        }
                        AuthStatus::Connecting => {
                            spinner_row(ui, &palette, "Conectando con Discord…");
                        }
                        AuthStatus::Failed(message) => {
                            theme::text(ui, message, theme::regular(13.0), palette.danger);
                            ui.add_space(12.0);
                            if big_button(ui, &palette, "Probar de nuevo") {
                                app.retry_sign_in();
                            }
                        }
                        AuthStatus::Connected => {
                            if big_button(ui, &palette, "Cerrar sesión") {
                                app.cancel_sign_in();
                            }
                        }
                    }
                });

            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 24.0),
                egui::Align2::CENTER_BOTTOM,
                format!("eCord {} • cliente no oficial de Discord", env!("CARGO_PKG_VERSION")),
                theme::regular(11.5),
                palette.dim,
            );
        });
}

fn spinner_row(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) {
    ui.horizontal(|ui| {
        let width = ui.painter().layout_no_wrap(label.to_string(), theme::medium(14.0), palette.text).size().x;
        ui.add_space((ui.available_width() - width - 26.0).max(0.0) / 2.0);
        theme::spinner(ui, 18.0, palette.accent);
        theme::text(ui, label, theme::medium(14.0), palette.text);
    });
}

/// Renderiza `url` como un código QR (SVG) y lo muestra centrado. Si por
/// lo que sea no se puede generar (no debería pasar con una URL válida),
/// muestra el link en texto como respaldo para que igual se pueda seguir
/// tocándolo/copiándolo a mano en el celular.
fn qr_code(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, url: &str) {
    let svg = match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(220, 220)
            .dark_color(qrcode::render::svg::Color("#000000"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build(),
        Err(_) => {
            theme::text(ui, url, theme::regular(11.5), palette.secondary);
            return;
        }
    };

    let id = QR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let uri = format!("bytes://qr_{id}.svg");

    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 220.0).max(0.0) / 2.0);
        Frame::new()
            .fill(egui::Color32::WHITE)
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.add(
                    egui::Image::from_bytes(uri, svg.into_bytes())
                        .fit_to_exact_size(Vec2::splat(220.0)),
                );
            });
    });
}

/// Formulario de usuario/contraseña (`AuthStatus::PasswordForm`).
fn password_form(app: &mut App, ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) {
    if let Some(error) = &app.login_error {
        theme::text(ui, error, theme::regular(12.5), palette.danger);
        ui.add_space(8.0);
    }

    let email_response = ui.add_sized(
        [ui.available_width(), 38.0],
        egui::TextEdit::singleline(&mut app.login_email).hint_text("Correo, teléfono o usuario"),
    );
    ui.add_space(8.0);
    let password_response = ui.add_sized(
        [ui.available_width(), 38.0],
        egui::TextEdit::singleline(&mut app.login_password).hint_text("Contraseña").password(true),
    );

    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
    // El botón tiene que dibujarse siempre (por eso `big_button` va
    // primero, no del lado derecho de un `||` que lo salte por
    // cortocircuito cuando ya se apretó Enter).
    let clicked = big_button(ui, palette, "Iniciar sesión");
    let submit = clicked || (enter_pressed && (email_response.lost_focus() || password_response.lost_focus()));
    if submit {
        app.submit_password_login();
    }

    ui.add_space(14.0);
    if text_link(ui, palette, "Usar código QR") {
        app.switch_to_qr_form();
    }
}

/// Pantalla de 2FA (`AuthStatus::PasswordMfaRequired`): código TOTP si
/// Discord lo ofrece, o pedir/escribir un código por SMS si no.
fn mfa_form(app: &mut App, ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) {
    let Some(challenge) = app.pending_mfa.clone() else {
        // No debería pasar (esta pantalla solo se muestra habiendo
        // recibido un desafío), pero por si acaso no dejamos la UI
        // colgada sin ninguna acción posible.
        theme::text(ui, "Se perdió el desafío de verificación.", theme::regular(13.0), palette.danger);
        ui.add_space(10.0);
        if theme::pill_button(ui, palette, "Volver", false).clicked() {
            app.switch_to_qr_form();
        }
        return;
    };

    use crate::discord::password_auth::MfaMethod;
    let has_totp = challenge.methods.contains(&MfaMethod::Totp);
    let has_sms = challenge.methods.contains(&MfaMethod::Sms);
    // Si hay TOTP, se lo prioriza (no depende de que llegue un SMS); si
    // no, el único camino es el código que mande el SMS.
    let verify_method = if has_totp { MfaMethod::Totp } else { MfaMethod::Sms };

    theme::text(
        ui,
        if has_totp {
            "Escribí el código de tu app de autenticación"
        } else {
            "Te mandamos un código por SMS"
        },
        theme::medium(13.5),
        palette.text,
    );
    ui.add_space(10.0);

    if let Some(error) = &app.login_error {
        theme::text(ui, error, theme::regular(12.5), palette.danger);
        ui.add_space(8.0);
    }

    if has_sms && !has_totp {
        // Método SMS-only: hay que pedirlo antes de poder escribir nada.
        let label = if app.login_sms_sent { "Reenviar código por SMS" } else { "Enviar código por SMS" };
        if theme::pill_button(ui, palette, label, false).clicked() {
            app.request_mfa_sms();
        }
        ui.add_space(10.0);
    }

    let code_response = ui.add_sized(
        [ui.available_width(), 38.0],
        egui::TextEdit::singleline(&mut app.login_mfa_code).hint_text("Código de verificación"),
    );
    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let clicked = big_button(ui, palette, "Verificar");
    let submit = clicked || (enter_pressed && code_response.lost_focus());
    if submit {
        app.submit_mfa_code(verify_method);
    }

    if has_sms && has_totp {
        ui.add_space(8.0);
        let label = if app.login_sms_sent { "Reenviar por SMS en su lugar" } else { "Mandar por SMS en su lugar" };
        if text_link(ui, palette, label) {
            app.request_mfa_sms();
        }
    }

    ui.add_space(14.0);
    if text_link(ui, palette, "Cancelar") {
        app.switch_to_qr_form();
    }
}

/// Texto clickeable, chico y con el color de acento — para acciones
/// secundarias ("Usar QR en su lugar", "Cancelar") que no ameritan un
/// botón grande como `big_button`.
fn text_link(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) -> bool {
    let galley = ui.painter().layout_no_wrap(label.to_string(), theme::medium(13.0), palette.accent);
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    let color = if response.hovered() { palette.accent_hover } else { palette.accent };
    ui.painter().galley(rect.min, galley, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn big_button(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) -> bool {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::semibold(15.0), palette.on_accent);
    let size = Vec2::new(ui.available_width().min(300.0), 46.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    ui.painter().rect_filled(rect, 23.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, palette.on_accent);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
