use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, Vec2};

use crate::lib::state::{App, AuthStatus};
use crate::ui::shell::extra;
use crate::ui::auth::ocean;
use crate::ui::theme::{self, Palette};

/// Contador para que cada QR generado tenga una URI "bytes://" distinta;
/// si reusáramos siempre la misma, el loader de imágenes de egui podría
/// quedarse con el SVG viejo cacheado al generar un QR nuevo (reintentos).
static QR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tamaño de la tarjeta de login en pantallas anchas (formulario a la
/// izquierda, QR a la derecha).
const WIDE_CARD_W: f32 = 800.0;
const WIDE_CARD_H: f32 = 480.0;
/// Ancho de la columna del QR y separación con el formulario (como la página).
const QR_COL_W: f32 = 240.0;
const COL_GAP: f32 = 40.0;
const CARD_PAD: f32 = 36.0;
/// Cuánto dura el splash de arranque antes de mostrar el login (s).
const STARTUP_SPLASH_SECS: f64 = 2.5;

#[derive(Clone, Copy)]
struct StartupSplash {
    begin: f64,
    enabled: bool,
}

/// Qué muestra el splash a pantalla completa en este cuadro, si corresponde:
/// (segundos desde que empezó, etiqueta, progreso 0..1).
fn splash_now(app: &App, ui: &egui::Ui) -> Option<(f32, String, f32)> {
    let ctx = ui.ctx();
    let now = ui.input(|i| i.time);

    // Carga real: conectando al gateway o enviando usuario/contraseña.
    let loading_label = match &app.auth {
        AuthStatus::Connecting => Some(
            app.connection_steps
                .last()
                .cloned()
                .unwrap_or_else(|| "Conectando con el gateway...".to_string()),
        ),
        AuthStatus::PasswordSubmitting => Some(if app.captcha.is_waiting() {
            "Esperando el captcha…".to_string()
        } else {
            "Comprobando sesión...".to_string()
        }),
        AuthStatus::PasswordMfaSubmitting => Some("Verificando código...".to_string()),
        AuthStatus::RegisterSubmitting => Some(if app.captcha.is_waiting() {
            "Esperando el captcha…".to_string()
        } else {
            "Creando tu cuenta...".to_string()
        }),
        _ => None,
    };
    let load_id = egui::Id::new("ecord_login_load_start");
    if let Some(label) = loading_label {
        let begin: f64 = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(load_id, || now));
        let e = (now - begin).max(0.0) as f32;
        // Sin porcentaje real: se acerca a 94 % sin llegar (el pase a la
        // pantalla principal lo cierra solo).
        let progress = 0.94 * (1.0 - (-e / 2.2).exp());
        return Some((e, label, progress));
    }
    ctx.data_mut(|d| d.remove_temp::<f64>(load_id));

    // Splash de arranque (solo si se llegó directo al login).
    let id = egui::Id::new("ecord_login_startup");
    let st: StartupSplash = ctx.data_mut(|d| {
        *d.get_temp_mut_or_insert_with(id, || StartupSplash {
            begin: now,
            enabled: true,
        })
    });
    if st.enabled {
        let e = now - st.begin;
        if e < STARTUP_SPLASH_SECS {
            let labels = ["Iniciando eCord...", "Preparando el cliente...", "Comprobando sesión..."];
            let i = ((e / STARTUP_SPLASH_SECS) * labels.len() as f64) as usize;
            let progress = (e / STARTUP_SPLASH_SECS) as f32;
            return Some((e as f32, labels[i.min(labels.len() - 1)].to_string(), progress));
        }
    }
    None
}

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
    // de cuentas, 2FA…). Una falla sin sesión de por medio también es
    // combinada, para poder seguir con usuario y contraseña aunque el QR no
    // haya podido generarse.
    let combined = match &app.auth {
        AuthStatus::Starting
        | AuthStatus::AwaitingScan { .. }
        | AuthStatus::Confirming { .. }
        | AuthStatus::PasswordForm => true,
        AuthStatus::Failed(_) => !app.has_saved_session(),
        _ => false,
    };

    // Si se llegó directo al login (no a "Conectando…"), el splash de arranque
    // corre; si la app arrancó ya conectando, no hace falta uno falso encima.
    {
        let id = egui::Id::new("ecord_login_startup");
        let now = ui.input(|i| i.time);
        let enabled = !matches!(
            app.auth,
            AuthStatus::Connecting
                | AuthStatus::PasswordSubmitting
                | AuthStatus::PasswordMfaSubmitting
                | AuthStatus::RegisterSubmitting
        );
        ui.ctx().data_mut(|d| {
            if d.get_temp::<StartupSplash>(id).is_none() {
                d.insert_temp(id, StartupSplash { begin: now, enabled });
            }
        });
    }

    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();

            // El océano propio del login solo con fondo sólido: si el tema
            // trae degradado/imagen, se deja ver ese.
            if app.backdrop.kind == crate::theme::BackdropKind::Solid {
                ocean::paint(ui, rect, &palette);
            }

            // Cargando (conectando, comprobando contraseña…): splash a pantalla
            // completa con datos curiosos, en lugar de la tarjeta.
            if let Some((elapsed, label, progress)) = splash_now(app, ui) {
                let anchor = ocean::paint_splash(ui, rect, &palette, elapsed, &label, progress);
                if matches!(app.auth, AuthStatus::PasswordSubmitting | AuthStatus::RegisterSubmitting)
                    && app.captcha.is_waiting()
                {
                    let continues = if matches!(app.auth, AuthStatus::RegisterSubmitting) {
                        "el registro sigue solo"
                    } else {
                        "el inicio de sesión sigue solo"
                    };
                    let area = Rect::from_min_size(egui::pos2(anchor.x - 200.0, anchor.y), Vec2::new(400.0, 90.0));
                    let mut hint_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(area)
                            .layout(Layout::top_down(Align::Center)),
                    );
                    hint_ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "Se abrió una ventana con la verificación de Discord. \
                                 Resolvela y {continues}."
                            ))
                            .font(theme::regular(12.0))
                            .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    hint_ui.add_space(10.0);
                    if text_link(&mut hint_ui, &palette, "Cancelar") {
                        app.captcha.cancel_all();
                    }
                }
                return;
            }

            if matches!(app.auth, AuthStatus::RegisterForm) {
                crate::ui::auth::register::show(app, ui, rect, &palette);
            } else if combined {
                combined_card(app, ui, rect, &palette);
            } else {
                single_card(app, ui, rect, &palette);
            }

            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 14.0),
                egui::Align2::CENTER_BOTTOM,
                format!("eCord {} • cliente no oficial de Discord", env!("CARGO_PKG_VERSION")),
                theme::regular(13.0),
                palette.dim,
            );

            // Distintivo "DEV" abajo a la derecha, solo en compilaciones de depuración.
            if cfg!(debug_assertions) {
                let galley = ui
                    .painter()
                    .layout_no_wrap("DEV".to_string(), theme::bold(12.0), palette.text);
                let size = galley.size() + Vec2::new(24.0, 6.0);
                let pill = Rect::from_min_size(
                    egui::pos2(rect.right() - 12.0 - size.x, rect.bottom() - 10.0 - size.y),
                    size,
                );
                let r = pill.height() / 2.0;
                ui.painter()
                    .rect_filled(pill, r, extra::blend(palette.window_solid, Color32::BLACK, 0.4));
                ui.painter()
                    .rect_stroke(pill, r, Stroke::new(1.0, palette.outline), egui::StrokeKind::Inside);
                ui.painter()
                    .galley(pill.center() - galley.size() / 2.0, galley, palette.text);
            }
        });
}

/// Pantalla principal: usuario/contraseña y código QR juntos. En ventanas
/// anchas van lado a lado dentro de una sola tarjeta (formulario a la izquierda,
/// columna del QR a la derecha); en ventanas angostas o bajas se apilan y la
/// pantalla hace scroll.
fn combined_card(app: &mut App, ui: &mut egui::Ui, rect: Rect, palette: &Palette) {
    let wide = rect.width() > 700.0 && rect.height() >= 540.0;
    let outer_radius = CornerRadius::same(18);
    let shadow = egui::epaint::Shadow {
        offset: [0, 24],
        blur: 60,
        spread: 0,
        color: palette.shadow,
    };

    if wide {
        let size = Vec2::new(WIDE_CARD_W.min(rect.width() - 32.0), WIDE_CARD_H);
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
                let content = inner.shrink(CARD_PAD);
                let left_w = (content.width() - QR_COL_W - COL_GAP).max(240.0);

                // Izquierda: formulario.
                let left = Rect::from_min_size(content.min, Vec2::new(left_w, content.height()));
                let mut left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(left)
                        .layout(Layout::top_down(Align::Min)),
                );
                form_pane(app, &mut left_ui, palette);

                // Derecha: columna del QR, centrada en vertical.
                let right = Rect::from_min_size(
                    egui::pos2(content.max.x - QR_COL_W, content.min.y),
                    Vec2::new(QR_COL_W, content.height()),
                );
                let mut right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right)
                        .layout(Layout::top_down(Align::Center)),
                );
                right_ui.add_space(((content.height() - QR_CONTENT_H) / 2.0).max(0.0));
                qr_pane(app, &mut right_ui, palette);
            });
    } else {
        let w = 480.0f32.min(rect.width() - 32.0).max(280.0);
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
                ui.add_space(((area.height() - 860.0) / 2.0).max(0.0));
                Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(outer_radius)
                    .shadow(shadow)
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(w - 8.0 - 48.0);
                        form_pane(app, ui, palette);
                        ui.add_space(26.0);
                        let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                        ui.painter().rect_filled(line, 0.0, palette.outline);
                        ui.add_space(26.0);
                        qr_pane(app, ui, palette);
                    });
                ui.add_space(12.0);
            });
    }
}

/// Alto aproximado del contenido de la columna del QR (para centrarlo).
const QR_CONTENT_H: f32 = 330.0;

/// Etiqueta de un campo: texto en mayúsculas, asterisco rojo y, si falta,
/// el aviso "– Este campo es obligatorio" en cursiva al lado.
pub(super) fn field_label(ui: &mut egui::Ui, palette: &Palette, label: &str, missing: bool) {
    let fmt = |font: egui::FontId, color: Color32, italics: bool| TextFormat {
        font_id: font,
        color,
        italics,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    job.append(label, 0.0, fmt(theme::semibold(12.0), palette.secondary, false));
    job.append(" *", 0.0, fmt(theme::semibold(12.0), palette.danger, false));
    if missing {
        job.append(
            " – Este campo es obligatorio",
            0.0,
            fmt(theme::medium(12.0), palette.danger, true),
        );
    }
    ui.add(egui::Label::new(job).selectable(false));
}

/// Campo de texto con el aspecto de la página: fondo más oscuro que la
/// tarjeta, borde fino, radio 8 y 42 px de alto; borde rojo si falta.
pub(super) fn styled_input(
    ui: &mut egui::Ui,
    palette: &Palette,
    text: &mut String,
    password: bool,
    missing: bool,
) -> egui::Response {
    ui.scope(|ui| {
        {
            let v = ui.visuals_mut();
            v.extreme_bg_color = palette.window_solid;
            let radius = CornerRadius::same(8);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
                w.corner_radius = radius;
                w.bg_stroke = Stroke::new(1.0, palette.outline);
            }
            v.selection.stroke = Stroke::new(1.5, palette.accent);
        }
        let edit = egui::TextEdit::singleline(text)
            .password(password)
            .font(theme::regular(15.0))
            .margin(Margin::symmetric(12, 0))
            .vertical_align(Align::Center);
        let response = ui.add_sized([ui.available_width(), 42.0], edit);
        if missing {
            ui.painter()
                .rect_stroke(response.rect, 8.0, Stroke::new(1.0, palette.danger), egui::StrokeKind::Inside);
        }
        response
    })
    .inner
}

/// Lado del formulario: usuario y contraseña (el mismo flujo de siempre,
/// `App::submit_password_login`).
fn form_pane(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    // Campos vacíos marcados tras apretar "Iniciar sesión" (correo, contraseña).
    let missing_id = egui::Id::new("ecord_login_missing");
    let (mut miss_user, mut miss_pass) = ui
        .ctx()
        .data(|d| d.get_temp::<(bool, bool)>(missing_id))
        .unwrap_or((false, false));

    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
            theme::logo(ui, logo_rect.center(), 32.0, palette.accent, palette.on_accent);
            theme::text(ui, "eCord", theme::bold(18.0), palette.text);
        });
        ui.add_space(22.0);

        ui.vertical_centered(|ui| {
            theme::text(ui, "¡Hola de nuevo!", theme::bold(24.0), palette.text);
            ui.add_space(6.0);
            theme::text(
                ui,
                "¡Nos alegramos mucho de volver a verte!",
                theme::regular(15.0),
                palette.secondary,
            );
        });
        ui.add_space(22.0);

        if let Some(error) = app.login_error.clone() {
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

        field_label(ui, palette, "CORREO ELECTRÓNICO O NÚMERO DE TELÉFONO", miss_user);
        ui.add_space(8.0);
        let email_response = styled_input(ui, palette, &mut app.login_email, false, miss_user);
        if email_response.changed() {
            miss_user = false;
        }
        ui.add_space(18.0);

        field_label(ui, palette, "CONTRASEÑA", miss_pass);
        ui.add_space(8.0);
        let password_response = styled_input(ui, palette, &mut app.login_password, true, miss_pass);
        if password_response.changed() {
            miss_pass = false;
        }
        ui.add_space(10.0);

        if text_link(ui, palette, "¿Olvidaste tu contraseña?") {
            // Discord resuelve el "olvidé mi contraseña" desde su pantalla de login.
            ui.ctx().open_url(egui::OpenUrl::new_tab("https://discord.com/login"));
        }
        ui.add_space(20.0);

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        // El botón tiene que dibujarse siempre (por eso va primero, no del
        // lado derecho de un `||` que lo salte por cortocircuito cuando ya se
        // apretó Enter).
        let clicked = wide_button(ui, palette, "Iniciar sesión");
        let submit = clicked || (enter_pressed && (email_response.lost_focus() || password_response.lost_focus()));
        if submit {
            miss_user = app.login_email.trim().is_empty();
            miss_pass = app.login_password.trim().is_empty();
            if !miss_user && !miss_pass {
                app.submit_password_login();
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            theme::text(ui, "¿Necesitas una cuenta?", theme::regular(13.0), palette.dim);
            if text_link(ui, palette, "Regístrate") {
                app.start_register();
            }
        });

        // Con cuentas guardadas se llegó acá desde el selector: dejar volver.
        if !app.accounts.is_empty() {
            ui.add_space(12.0);
            if text_link(ui, palette, "← Volver a las cuentas") {
                app.cancel_sign_in();
            }
        }
    });

    ui.ctx().data_mut(|d| d.insert_temp(missing_id, (miss_user, miss_pass)));
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
        ui.spacing_mut().item_spacing.y = 0.0;

        match state {
            QrState::Loading => {
                qr_placeholder(ui, palette, QR_BOX, true);
            }
            QrState::Ready(url) => {
                qr_code(ui, palette, &url);
            }
            QrState::Confirming(username) => {
                qr_placeholder(ui, palette, QR_BOX, true);
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
                qr_placeholder(ui, palette, QR_BOX, false);
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

        ui.add_space(22.0);
        theme::text(ui, "Inicia sesión con código QR", theme::bold(20.0), palette.text);
        ui.add_space(8.0);

        let fmt = |font: egui::FontId| TextFormat {
            font_id: font,
            color: palette.secondary,
            ..Default::default()
        };
        let mut job = LayoutJob::default();
        job.halign = Align::Center;
        job.wrap.max_width = 210.0;
        job.append("Escanea esto con la ", 0.0, fmt(theme::regular(14.0)));
        job.append("aplicación móvil de Discord", 0.0, fmt(theme::bold(14.0)));
        job.append(" para iniciar sesión al instante.", 0.0, fmt(theme::regular(14.0)));
        ui.add(egui::Label::new(job).selectable(false));
    });
}

/// Lado de la caja blanca del QR (con su borde de 10 px).
const QR_BOX: f32 = 176.0;

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
                // El formulario de registro y su "creando cuenta…" los
                // dibuja `ui::auth::register` / el splash (ver `show`).
                AuthStatus::RegisterForm | AuthStatus::RegisterSubmitting => {}
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

/// Renderiza `url` como un código QR (SVG) en una caja blanca de 176 px con el
/// logo de eCord en el centro, y lo muestra centrado. Como el logo tapa una
/// parte del código, se usa corrección de errores alta (H). Si por lo que sea
/// no se puede generar (no debería pasar con una URL válida), muestra el link
/// en texto como respaldo para que igual se pueda seguir tocándolo/copiándolo
/// a mano en el celular.
fn qr_code(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, url: &str) {
    let svg = match qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::H) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(220, 220)
            .quiet_zone(false)
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
        ui.add_space((ui.available_width() - QR_BOX).max(0.0) / 2.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(QR_BOX), Sense::hover());
        ui.painter().rect_filled(rect, 12.0, Color32::WHITE);
        let img_rect = rect.shrink(10.0);
        egui::Image::from_bytes(uri, svg.into_bytes())
            .fit_to_exact_size(img_rect.size())
            .paint_at(ui, img_rect);
        // Logo en el centro, sobre un cuadrito blanco.
        let badge = Rect::from_center_size(rect.center(), Vec2::splat(38.0));
        ui.painter().rect_filled(badge, 8.0, Color32::WHITE);
        theme::logo(ui, rect.center(), 28.0, palette.accent, Color32::WHITE);
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
pub(super) fn text_link(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette, label: &str) -> bool {
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
pub(super) fn wide_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> bool {
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
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, palette.on_accent);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
