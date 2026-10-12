use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, Vec2};

use crate::lib::state::{App, AuthStatus};
use crate::ui::shell::extra;
use crate::ui::theme::{self, Palette};

/// Contador para que cada QR generado tenga una URI "bytes://" distinta;
/// si reusáramos siempre la misma, el loader de imágenes de egui podría
/// quedarse con el SVG viejo cacheado al generar un QR nuevo (reintentos).
static QR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tamaño de la tarjeta de login en pantallas anchas (formulario a la
/// izquierda, QR a la derecha).
const WIDE_CARD_W: f32 = 820.0;
const WIDE_CARD_H: f32 = 470.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;

    // Usuario/contraseña y QR conviven en la misma pantalla (como en el
    // cliente oficial), así que el QR se genera solo apenas se llega al
    // login, sin pasar por ningún botón ni opción. Sin cuentas guardadas se
    // arranca directo; con cuentas se muestra primero el selector y el QR
    // arranca con "Agregar otra cuenta". `PasswordForm` es lo que queda tras un
    // intento fallido de contraseña (que cortó el QR anterior): se genera uno
    // nuevo conservando el error y lo ya tipeado.
    let start_qr = match app.auth {
        AuthStatus::SignedOut => app.accounts.is_empty() && app.pending_resume_token.is_none(),
        AuthStatus::PasswordForm => true,
        _ => false,
    };
    if start_qr {
        app.start_qr_keep_error();
    }

    // Pantalla combinada (formulario + QR) o tarjeta de un solo paso (selector
    // de cuentas, 2FA, conectando…). Una falla sin sesión de por medio también
    // es combinada, para poder seguir con usuario y contraseña aunque el QR no
    // haya podido generarse.
    let combined = match &app.auth {
        AuthStatus::Starting
        | AuthStatus::AwaitingScan { .. }
        | AuthStatus::Confirming { .. }
        | AuthStatus::PasswordForm => true,
        AuthStatus::Failed(_) => !app.has_saved_session(),
        _ => false,
    };

    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();

            // El degradado propio del login solo con fondo sólido: si el tema
            // trae degradado/imagen, se deja ver ese.
            if app.backdrop.kind == crate::theme::BackdropKind::Solid {
                let top = extra::blend(palette.window, palette.accent, 0.14);
                extra::paint_vertical_gradient(ui, rect, top, palette.window);
            }

            if combined {
                combined_card(app, ui, rect, &palette);
            } else {
                single_card(app, ui, rect, &palette);
            }

            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 24.0),
                egui::Align2::CENTER_BOTTOM,
                format!("eCord {} • cliente no oficial de Discord", env!("CARGO_PKG_VERSION")),
                theme::regular(11.5),
                palette.dim,
            );
        });
}

/// Pantalla principal: usuario/contraseña y código QR juntos. En ventanas
/// anchas van lado a lado dentro de una sola tarjeta flotante (el QR sobre una
/// tarjeta interior más clara); en ventanas angostas o bajas se apilan y la
/// pantalla hace scroll.
fn combined_card(app: &mut App, ui: &mut egui::Ui, rect: Rect, palette: &Palette) {
    let wide = rect.width() >= 800.0 && rect.height() >= 520.0;
    let outer_radius = CornerRadius::same(theme::CARD_RADIUS + 6);
    let shadow = egui::epaint::Shadow {
        offset: [0, 16],
        blur: 48,
        spread: 0,
        color: palette.shadow,
    };

    if wide {
        let size = Vec2::new(WIDE_CARD_W.min(rect.width() - 48.0), WIDE_CARD_H);
        let area = Rect::from_center_size(rect.center() - Vec2::new(0.0, 14.0), size + Vec2::splat(8.0));
        let mut card_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(Layout::top_down(Align::Center)),
        );
        Frame::new()
            .fill(palette.panel)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(outer_radius)
            .shadow(shadow)
            .inner_margin(Margin::same(0))
            .show(&mut card_ui, |ui| {
                let (inner, _) = ui.allocate_exact_size(size, Sense::hover());
                let left_w = (inner.width() * 0.56).round();

                // Izquierda: formulario.
                let left = Rect::from_min_size(inner.min, Vec2::new(left_w, inner.height()))
                    .shrink2(Vec2::new(40.0, 36.0));
                // Derecha: tarjeta interior con el QR.
                let right_card = Rect::from_min_max(egui::pos2(inner.min.x + left_w, inner.min.y), inner.max).shrink(14.0);
                ui.painter().rect_filled(
                    right_card,
                    CornerRadius::same(theme::CARD_RADIUS + 2),
                    palette.surface,
                );

                let mut left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(left)
                        .layout(Layout::top_down(Align::Min)),
                );
                form_pane(app, &mut left_ui, palette);

                let right = right_card.shrink2(Vec2::new(18.0, 26.0));
                let mut right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right)
                        .layout(Layout::top_down(Align::Center)),
                );
                qr_pane(app, &mut right_ui, palette);
            });
    } else {
        let w = 440.0f32.min(rect.width() - 32.0).max(280.0);
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
                ui.add_space(((area.height() - 820.0) / 2.0).max(0.0));
                Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(outer_radius)
                    .shadow(shadow)
                    .inner_margin(Margin::same(28))
                    .show(ui, |ui| {
                        ui.set_width(w - 58.0);
                        form_pane(app, ui, palette);
                        ui.add_space(18.0);
                        ui.separator();
                        ui.add_space(14.0);
                        Frame::new()
                            .fill(palette.surface)
                            .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
                            .inner_margin(Margin::same(18))
                            .show(ui, |ui| {
                                ui.set_width(w - 58.0 - 36.0);
                                qr_pane(app, ui, palette);
                            });
                    });
                ui.add_space(12.0);
            });
    }
}

/// Lado del formulario: usuario y contraseña (el mismo flujo de siempre,
/// `App::submit_password_login`).
fn form_pane(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;

        ui.horizontal(|ui| {
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
            theme::logo(ui, logo_rect.center(), 34.0, palette.accent, palette.on_accent);
            ui.add_space(4.0);
            theme::text(ui, "eCord", theme::bold(20.0), palette.text);
        });
        ui.add_space(16.0);
        theme::text(ui, "Te damos la bienvenida", theme::bold(26.0), palette.text);
        theme::text(ui, "Entrá con tu cuenta de Discord", theme::regular(13.5), palette.secondary);
        ui.add_space(14.0);

        if let Some(error) = app.login_error.clone() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(error)
                        .font(theme::regular(12.5))
                        .color(palette.danger),
                )
                .wrap(),
            );
            ui.add_space(6.0);
        }

        field_label(ui, palette, "Correo, teléfono o usuario");
        let email_response = ui.add_sized(
            [ui.available_width(), 40.0],
            egui::TextEdit::singleline(&mut app.login_email),
        );
        ui.add_space(8.0);
        field_label(ui, palette, "Contraseña");
        let password_response = ui.add_sized(
            [ui.available_width(), 40.0],
            egui::TextEdit::singleline(&mut app.login_password).password(true),
        );
        ui.add_space(10.0);

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        // El botón tiene que dibujarse siempre (por eso va primero, no del
        // lado derecho de un `||` que lo salte por cortocircuito cuando ya se
        // apretó Enter).
        let clicked = wide_button(ui, palette, "Iniciar sesión");
        let submit = clicked || (enter_pressed && (email_response.lost_focus() || password_response.lost_focus()));
        if submit {
            app.submit_password_login();
        }

        // Con cuentas guardadas se llegó acá desde el selector: dejar volver.
        if !app.accounts.is_empty() {
            ui.add_space(12.0);
            if text_link(ui, palette, "← Volver a las cuentas") {
                app.cancel_sign_in();
            }
        }
    });
}

/// Qué muestra el lado del QR.
enum QrState {
    /// Esperando que el servidor de "remote auth" mande el fingerprint.
    Loading,
    /// QR listo para escanear.
    Ready(String),
    /// Escaneado; falta confirmar en el celular.
    Confirming(String),
    /// No se pudo generar/completar el QR.
    Error(String),
}

/// Lado del QR: código para escanear con la app de Discord del celular.
fn qr_pane(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    let state = match &app.auth {
        AuthStatus::AwaitingScan { url } => QrState::Ready(url.clone()),
        AuthStatus::Confirming { username } => QrState::Confirming(username.clone()),
        AuthStatus::Failed(message) => QrState::Error(message.clone()),
        _ => QrState::Loading,
    };

    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        theme::text(ui, "Entrá con código QR", theme::bold(19.0), palette.text);
        ui.add_space(2.0);
        for line in [
            "Escaneá este código con la app",
            "de Discord en tu celular para",
            "entrar al instante.",
        ] {
            theme::text(ui, line, theme::regular(13.0), palette.secondary);
        }
        ui.add_space(14.0);

        match state {
            QrState::Loading => {
                qr_placeholder(ui, palette, 244.0, true);
                ui.add_space(6.0);
                theme::text(ui, "Generando código QR…", theme::medium(12.5), palette.dim);
            }
            QrState::Ready(url) => {
                qr_code(ui, palette, &url);
            }
            QrState::Confirming(username) => {
                qr_placeholder(ui, palette, 244.0, true);
                ui.add_space(8.0);
                theme::text(
                    ui,
                    "Confirmá el inicio de sesión en tu celular",
                    theme::medium(13.0),
                    palette.text,
                );
                theme::text(ui, username, theme::semibold(13.5), palette.text);
                ui.add_space(4.0);
                if text_link(ui, palette, "Cancelar") {
                    app.start_sign_in();
                }
            }
            QrState::Error(message) => {
                qr_placeholder(ui, palette, 244.0, false);
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(message)
                            .font(theme::regular(12.5))
                            .color(palette.danger),
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                if theme::pill_button(ui, palette, "Generar código nuevo", true).clicked() {
                    app.start_sign_in();
                }
            }
        }
    });
}

/// Cuadro vacío del tamaño del QR (con spinner mientras se genera) para que la
/// pantalla no salte cuando llega el código.
fn qr_placeholder(ui: &mut egui::Ui, palette: &Palette, size: f32, busy: bool) {
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - size).max(0.0) / 2.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        ui.painter().rect_filled(rect, 12.0, palette.surface_hover);
        if busy {
            let mut spinner_ui = ui.new_child(
                egui::UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), Vec2::splat(24.0))),
            );
            theme::spinner(&mut spinner_ui, 24.0, palette.accent);
        }
    });
}

fn field_label(ui: &mut egui::Ui, palette: &Palette, label: &str) {
    theme::text(ui, label, theme::semibold(12.0), palette.secondary);
}

/// Tarjeta de un solo paso: selector de cuentas, 2FA, conectando, errores de
/// conexión con sesión guardada…
fn single_card(app: &mut App, ui: &mut egui::Ui, rect: Rect, palette: &Palette) {
    let card_width = 440.0;
    // Con cuentas guardadas, la tarjeta crece para mostrar el selector
    // (hasta donde entre en la ventana; después la lista hace scroll).
    let base_card_height = 420.0;
    let saved_count = if matches!(app.auth, AuthStatus::SignedOut) {
        app.accounts.accounts().len()
    } else {
        0
    };
    let picker_wanted = if saved_count > 0 {
        44.0 + saved_count as f32 * crate::ui::auth::accounts::ROW_STRIDE
    } else {
        0.0
    };
    let card_height = (base_card_height + picker_wanted).min((rect.height() - 40.0).max(base_card_height));
    let picker_list_height = (card_height - base_card_height - 44.0).max(0.0);
    let card = Rect::from_center_size(rect.center() - Vec2::new(0.0, 20.0), Vec2::new(card_width, card_height));

    let mut card_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card)
            .layout(Layout::top_down(Align::Center)),
    );

    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS + 6))
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

            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
            theme::logo(ui, logo_rect.center(), 56.0, palette.accent, palette.on_accent);

            ui.add_space(6.0);
            theme::text(ui, "eCord", theme::bold(28.0), palette.text);
            let subtitle = match &app.auth {
                AuthStatus::SignedOut if !app.accounts.is_empty() => "Elegí una cuenta o agregá otra",
                AuthStatus::PasswordMfaRequired | AuthStatus::PasswordMfaSubmitting => "Verificación en dos pasos",
                _ => "",
            };
            if !subtitle.is_empty() {
                theme::text(ui, subtitle, theme::regular(13.5), palette.secondary);
            }
            ui.add_space(18.0);

            match &app.auth {
                AuthStatus::SignedOut => {
                    // Motivo por el que se volvió acá (p. ej. la
                    // sesión guardada ya no era válida).
                    if let Some(error) = &app.login_error {
                        theme::text(ui, error, theme::regular(13.0), palette.danger);
                        ui.add_space(12.0);
                    }
                    // Cuentas guardadas: elegir con cuál entrar (las que
                    // tienen la sesión cerrada se marcan como tales).
                    if !app.accounts.is_empty() {
                        crate::ui::auth::accounts::picker(app, ui, palette, picker_list_height);
                        // Lleva a la pantalla combinada (usuario/contraseña + QR).
                        if big_button(ui, palette, "Agregar otra cuenta") {
                            app.start_sign_in();
                        }
                    } else {
                        // Sin cuentas el QR arranca solo (ver `show`); esto solo
                        // se ve mientras espera el aviso de seguridad.
                        spinner_row(ui, palette, "Esperando confirmación…");
                    }
                }
                AuthStatus::PasswordSubmitting => {
                    submitting_row(app, ui, palette, "Iniciando sesión…");
                }
                AuthStatus::PasswordMfaRequired => {
                    mfa_form(app, ui, palette);
                }
                AuthStatus::PasswordMfaSubmitting => {
                    submitting_row(app, ui, palette, "Verificando código…");
                }
                AuthStatus::Connecting => {
                    spinner_row(ui, palette, "Conectando con Discord…");
                    connection_steps(app, ui, palette);
                }
                AuthStatus::Failed(message) => {
                    theme::text(ui, message, theme::regular(13.0), palette.danger);
                    ui.add_space(12.0);
                    if big_button(ui, palette, "Probar de nuevo") {
                        app.retry_sign_in();
                    }
                    // Con sesión guardada, "Probar de nuevo" reconecta
                    // con ese token; esto es la salida para entrar con
                    // otra cuenta (o ver cuáles hay). No borra nada.
                    if app.has_saved_session() || !app.accounts.is_empty() {
                        ui.add_space(14.0);
                        if text_link(ui, palette, "Elegir otra cuenta") {
                            app.show_account_picker();
                        }
                    }
                }
                AuthStatus::Connected => {
                    if big_button(ui, palette, "Cerrar sesión") {
                        app.ask_log_out();
                    }
                }
                // Los pasos del QR y el formulario son la pantalla combinada
                // (`combined_card`); no llegan acá.
                AuthStatus::Starting
                | AuthStatus::AwaitingScan { .. }
                | AuthStatus::Confirming { .. }
                | AuthStatus::PasswordForm => {}
            }
        });
}

/// Lista corta de lo que está haciendo la conexión, debajo del spinner:
/// los pasos ya hechos atenuados y el actual (el último) resaltado.
fn connection_steps(app: &App, ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) {
    let steps = &app.connection_steps;
    if steps.is_empty() {
        return;
    }
    ui.add_space(8.0);
    let last = steps.len() - 1;
    for (index, step) in steps.iter().enumerate() {
        if index == last {
            theme::text(ui, step, theme::medium(12.5), palette.secondary);
        } else {
            theme::text(ui, step, theme::regular(12.0), palette.dim);
        }
    }
}

fn spinner_row(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) {
    ui.horizontal(|ui| {
        let width = ui
            .painter()
            .layout_no_wrap(label.to_string(), theme::medium(14.0), palette.text)
            .size()
            .x;
        ui.add_space((ui.available_width() - width - 26.0).max(0.0) / 2.0);
        theme::spinner(ui, 18.0, palette.accent);
        theme::text(ui, label, theme::medium(14.0), palette.text);
    });
}

/// Como `spinner_row`, pero si Discord pidió un captcha y hay una ventana
/// esperando a la persona, lo dice (y deja cancelar) en vez de parecer que
/// el login se colgó.
fn submitting_row(app: &mut App, ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) {
    if !app.captcha.is_waiting() {
        spinner_row(ui, palette, label);
        return;
    }
    spinner_row(ui, palette, "Esperando el captcha…");
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Se abrió una ventana con la verificación de Discord. \
                 Resolvela y el inicio de sesión sigue solo.",
            )
            .font(theme::regular(12.0))
            .color(palette.secondary),
        )
        .wrap(),
    );
    ui.add_space(14.0);
    if text_link(ui, palette, "Cancelar") {
        app.captcha.cancel_all();
    }
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
                ui.add(egui::Image::from_bytes(uri, svg.into_bytes()).fit_to_exact_size(Vec2::splat(220.0)));
            });
    });
}

/// Pantalla de 2FA (`AuthStatus::PasswordMfaRequired`): código TOTP si
/// Discord lo ofrece, o pedir/escribir un código por SMS si no.
fn mfa_form(app: &mut App, ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) {
    let Some(challenge) = app.pending_mfa.clone() else {
        // No debería pasar (esta pantalla solo se muestra habiendo
        // recibido un desafío), pero por si acaso no dejamos la UI
        // colgada sin ninguna acción posible.
        theme::text(
            ui,
            "Se perdió el desafío de verificación.",
            theme::regular(13.0),
            palette.danger,
        );
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
        let label = if app.login_sms_sent {
            "Reenviar código por SMS"
        } else {
            "Enviar código por SMS"
        };
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
        let label = if app.login_sms_sent {
            "Reenviar por SMS en su lugar"
        } else {
            "Mandar por SMS en su lugar"
        };
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
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::medium(13.0), palette.accent);
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    let color = if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    ui.painter().galley(rect.min, galley, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn big_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> bool {
    let width = ui.available_width().min(300.0);
    sized_button(ui, palette, label, width)
}

/// Botón principal a todo el ancho disponible (el del formulario).
fn wide_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> bool {
    let width = ui.available_width();
    sized_button(ui, palette, label, width)
}

fn sized_button(ui: &mut egui::Ui, palette: &Palette, label: &str, width: f32) -> bool {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::semibold(15.0), palette.on_accent);
    let size = Vec2::new(width, 44.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, palette.on_accent);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
