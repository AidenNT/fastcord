//! Panel de ajustes de apariencia: elegir entre oscuro/claro, activar o
//! desactivar el fondo dinámico según el wallpaper del sistema, elegir uno
//! de los temas que ya creó el usuario, o crear/editar uno nuevo con un
//! color por rol.
//!
//! Mismo patrón que `ui::overlay::show_modal` (scrim + tarjeta centrada,
//! por encima de todo lo demás): todo lo que se necesita de `App` para
//! dibujar se clona/copia ANTES de entrar al `Area::show`, así que el
//! closure de adentro no toca `app` para nada — solo junta lo que
//! clickeó el usuario en variables locales (`action`, `editor`) y recién
//! después de que el `Area` termina de dibujarse se lo aplicamos a
//! `app` de una. Evita cualquier lío de préstamos entre el `&mut App`
//! de afuera y los closures anidados de adentro.

use egui::{
    Area, Color32, CornerRadius, Frame, Margin, Order, ScrollArea, Sense, Stroke, UiBuilder, Vec2,
};

use crate::discord::voice::{VoiceAudioSourceOptions, VoiceAudioSources};
use crate::discord::{VoiceAudioSettings, VoiceVolumePercent};
use crate::lib::state::{App, NotificationSide};
use crate::ui::account_settings;
use crate::ui::theme::{self, Icon, Palette, ThemeDef, ThemeMode};

const CARD_SIZE: Vec2 = Vec2::new(760.0, 640.0);

/// Pestaña activa del panel. `Voice` refresca la lista de dispositivos
/// cada vez que se entra (ver el `Action::SetTab` de más abajo), así que
/// no hace falta que `App` la pida por separado al arrancar.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsTab {
    #[default]
    Appearance,
    Voice,
    /// Ajustes de la cuenta (`PreloadedUserSettings`) en SOLO LECTURA: no
    /// tiene ninguna `Action`, ver `ui::account_settings`.
    Account,
}

/// Todo lo que puede pasar en un frame del panel: un solo clic a la vez,
/// aplicado a `app` después de que el `Area` termina de dibujarse. Para
/// los sliders de la pestaña de voz esto alcanza igual: cada frame que se
/// arrastra uno dispara su propia `Action` con el valor de ESE frame, que
/// se aplica antes del que viene.
enum Action {
    Close,
    OpenChangelog,
    SetMode(ThemeMode),
    SetImageMode(crate::ui::anim::ImageMode),
    SetNotificationSide(NotificationSide),
    ImportThemes(String),
    OpenNew,
    OpenEdit(String),
    Delete(String),
    SaveEditor,
    CancelEditor,
    SetTab(SettingsTab),
    SetVoiceInputSource(Option<String>),
    SetVoiceOutputSource(Option<String>),
    SetAllowMicrophoneTransmit(bool),
    SetNoiseSuppression(bool),
    SetMicrophoneSensitivity(i8),
    SetMicrophoneVolume(u8),
    SetOutputVolume(u8),
    /// Cambiar de cuenta, agregar otra o cerrar sesión (Ajustes → Cuenta).
    Account(crate::ui::accounts::AccountAction),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if !app.settings_open {
        return;
    }

    // Todo lo que hace falta para dibujar, ya sacado de `app`.
    let palette = app.palette;
    let mode = app.theme_mode.clone();
    let custom_themes = app.custom_themes.clone();
    let wallpaper_seed = app.wallpaper_seed();
    let mut editor = app.theme_editor.take();
    let tab = app.settings_tab;
    let notification_side = app.notification_side;
    let voice_audio = app.voice.audio.clone();
    let voice_audio_sources = app.voice.audio_sources.clone();
    let voice_audio_source_options = app.voice_audio_source_options.clone();
    // Solo se arma cuando se está viendo la pestaña: son unas pocas filas
    // de texto, pero no vale la pena recalcularlas cada frame si no se ven
    // (y así evitamos clonar `discord_settings`, que puede ser grande).
    let account_loaded = app.discord_settings.is_some();
    let account_sections = (tab == SettingsTab::Account)
        .then(|| account_settings::build(app.me.as_ref(), app.discord_settings.as_ref()));

    // Cuentas guardadas (solo se copian viendo la pestaña Cuenta).
    let saved_accounts = (tab == SettingsTab::Account).then(|| app.accounts.accounts().to_vec());
    let current_account_id = app.me.as_ref().map(|me| me.id.clone());

    let screen_rect = ui.ctx().viewport_rect();
    let mut action: Option<Action> = None;

    // Arrastrar un `.json` de temas a la ventana lo importa (solo en la
    // pestaña de apariencia y sin el editor abierto: con el editor abierto
    // un archivo soltado es la imagen de fondo).
    if editor.is_none() && tab == SettingsTab::Appearance {
    let json_file = ui.ctx().input(|i| {
        i.raw.dropped_files.iter().find_map(|f| {
            let p = f.path();
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                .then(|| p.to_path_buf())
        })
    });
        if let Some(path) = json_file {
            match std::fs::read_to_string(&path) {
                Ok(text) => action = Some(Action::ImportThemes(text)),
                Err(e) => app.push_toast(
                    crate::lib::state::ToastKind::Warning,
                    "No se pudo leer el archivo",
                    e.to_string(),
                ),
            }
        }
    }

    Area::new(egui::Id::new("settings_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_width(screen_rect.width());
            ui.set_height(screen_rect.height());
            ui.painter()
                .rect_filled(screen_rect, 0.0, Color32::from_black_alpha(150));
            // Que no se le escapen los clicks a lo que está atrás.
            ui.interact(screen_rect, egui::Id::new("settings_scrim_block"), Sense::click());

            let size = Vec2::new(
                CARD_SIZE.x.min(screen_rect.width() - 40.0),
                CARD_SIZE.y.min(screen_rect.height() - 40.0),
            );
            let card_rect = egui::Rect::from_center_size(screen_rect.center(), size);
            let mut card_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(card_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 6))
                .inner_margin(Margin::same(22))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 16],
                    blur: 40,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(&mut card_ui, |ui| {
                    ui.set_width(size.x - 44.0);
                    ui.set_height(size.y - 44.0);

                    ui.horizontal(|ui| {
                        theme::text(ui, "Ajustes", theme::bold(19.0), palette.text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, Icon::X, 15.0, palette.dim, palette.text, "Cerrar")
                                .clicked()
                            {
                                action = Some(Action::Close);
                            }
                        });
                    });
                    ui.add_space(12.0);

                    // Mientras el editor de temas está abierto ocupa toda
                    // la tarjeta (tiene su propio "← Volver"); las
                    // pestañas solo tienen sentido para elegir QUÉ ver
                    // cuando no hay ningún sub-panel tapando todo.
                    if editor.is_none() {
                        ui.horizontal(|ui| {
                            if theme::soft_button(ui, &palette, None, "Cuenta", tab == SettingsTab::Account)
                                .clicked()
                            {
                                action = Some(Action::SetTab(SettingsTab::Account));
                            }
                            if theme::soft_button(ui, &palette, None, "Apariencia", tab == SettingsTab::Appearance)
                                .clicked()
                            {
                                action = Some(Action::SetTab(SettingsTab::Appearance));
                            }
                            if theme::soft_button(ui, &palette, None, "Voz y audio", tab == SettingsTab::Voice)
                                .clicked()
                            {
                                action = Some(Action::SetTab(SettingsTab::Voice));
                            }
                        });
                        ui.add_space(14.0);
                    }

                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(ed) = editor.as_mut() {
                                if let Some(taken) = theme_editor_view(ui, ed, &palette) {
                                    action = Some(taken);
                                }
                            } else {
                                match tab {
                                    SettingsTab::Appearance => {
                                        if let Some(taken) = theme_picker_view(
                                            ui,
                                            &palette,
                                            &mode,
                                            &custom_themes,
                                            wallpaper_seed,
                                        ) {
                                            action = Some(taken);
                                        }
                                        if let Some(taken) = notification_side_section(ui, &palette, notification_side) {
                                            action = Some(taken);
                                        }
                                        if let Some(taken) = image_mode_section(ui, &palette) {
                                            action = Some(taken);
                                        }
                                    }
                                    SettingsTab::Account => {
                                        // Cuentas: cambiar, agregar, cerrar sesión.
                                        if let Some(taken) = crate::ui::accounts::settings_section(
                                            ui,
                                            &palette,
                                            saved_accounts.as_deref().unwrap_or(&[]),
                                            current_account_id.as_deref(),
                                        ) {
                                            action = Some(Action::Account(taken));
                                        }
                                        ui.add_space(18.0);
                                        // Ajustes de la cuenta: solo lectura.
                                        account_settings::view(
                                            ui,
                                            &palette,
                                            account_sections.as_deref().unwrap_or(&[]),
                                            account_loaded,
                                        );
                                        ui.add_space(18.0);
                                        theme::section_title(ui, &palette, "Acerca de");
                                        theme::text(
                                            ui,
                                            format!("eCord v{}", env!("CARGO_PKG_VERSION")),
                                            theme::regular(12.5),
                                            palette.dim,
                                        );
                                        ui.add_space(8.0);
                                        if theme::soft_button(
                                            ui,
                                            &palette,
                                            Some(Icon::Sparkles),
                                            "Ver novedades",
                                            false,
                                        )
                                        .clicked()
                                        {
                                            action = Some(Action::OpenChangelog);
                                        }
                                    }
                                    SettingsTab::Voice => {
                                        if let Some(taken) = voice_settings_view(
                                            ui,
                                            &palette,
                                            &voice_audio,
                                            &voice_audio_sources,
                                            &voice_audio_source_options,
                                        ) {
                                            action = Some(taken);
                                        }
                                    }
                                }
                            }
                        });
                });
        });

    // El editor (tocado o no este frame) vuelve a `app` antes de aplicar
    // la acción: `save_theme_editor`/`open_theme_editor_*` esperan
    // encontrarlo ahí, y si no hubo ninguna acción de todos modos hay
    // que guardar los colores que se hayan tocado en los pickers.
    app.theme_editor = editor;

    if let Some(action) = action {
        match action {
            Action::Close => app.settings_open = false,
            Action::OpenChangelog => app.open_changelog(),
            Action::SetMode(mode) => app.set_theme_mode(mode),
            Action::SetImageMode(mode) => app.set_image_mode(mode),
            Action::SetNotificationSide(side) => app.set_notification_side(side),
            Action::ImportThemes(json) => match app.import_themes(&json) {
                Ok(n) => {
                    // Vacía el cuadro de pegado y avisa.
                    ui.ctx().data_mut(|d| d.remove::<String>(egui::Id::new("theme_import_text")));
                    app.push_toast(
                        crate::lib::state::ToastKind::Success,
                        "Tema importado",
                        if n == 1 { "Se importó 1 tema y quedó activo.".to_owned() } else { format!("Se importaron {n} temas; el último quedó activo.") },
                    );
                }
                Err(msg) => app.push_toast(crate::lib::state::ToastKind::Warning, "No se pudo importar", msg),
            },
            Action::OpenNew => app.open_theme_editor_new(),
            Action::OpenEdit(name) => app.open_theme_editor_existing(&name),
            Action::Delete(name) => app.delete_custom_theme(&name),
            Action::SaveEditor => app.save_theme_editor(),
            Action::CancelEditor => app.cancel_theme_editor(),
            Action::SetTab(tab) => {
                app.settings_tab = tab;
                if tab == SettingsTab::Voice {
                    // Recién ahora, al entrar a la pestaña, vale la pena
                    // pagar el costo de enumerar dispositivos — ver el
                    // doc de `App::refresh_voice_audio_sources`.
                    app.refresh_voice_audio_sources();
                }
            }
            Action::SetVoiceInputSource(id) => app.set_voice_input_source(id),
            Action::SetVoiceOutputSource(id) => app.set_voice_output_source(id),
            Action::SetAllowMicrophoneTransmit(allow) => {
                app.set_voice_allow_microphone_transmit(allow)
            }
            Action::SetNoiseSuppression(enabled) => app.set_voice_noise_suppression(enabled),
            Action::SetMicrophoneSensitivity(db) => app.set_voice_microphone_sensitivity(db),
            Action::SetMicrophoneVolume(percent) => app.set_voice_microphone_volume(percent),
            Action::SetOutputVolume(percent) => app.set_voice_output_volume(percent),
            Action::Account(account_action) => {
                use crate::ui::accounts::AccountAction;
                match account_action {
                    AccountAction::Switch(user_id) => app.switch_account(&user_id),
                    AccountAction::Add => app.show_account_picker(),
                    AccountAction::LogOut => app.ask_log_out(),
                    AccountAction::Remove(user_id) => app.ask_remove_account(&user_id),
                }
            }
        }
    }
}

/// De qué lado de la ventana aparecen las notificaciones (menciones, DMs y
/// avisos) mientras la app está en foco.
fn notification_side_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    current: NotificationSide,
) -> Option<Action> {
    let mut action = None;
    ui.add_space(22.0);
    theme::text(ui, "Notificaciones", theme::semibold(14.0), palette.text);
    ui.add_space(4.0);
    theme::subtle(
        ui,
        palette,
        "Elegí de qué lado de la ventana aparecen las notificaciones (con la foto de quien escribe).",
    );
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if theme::soft_button(ui, palette, None, "Izquierda", current == NotificationSide::Left).clicked() {
            action = Some(Action::SetNotificationSide(NotificationSide::Left));
        }
        if theme::soft_button(ui, palette, None, "Derecha", current == NotificationSide::Right).clicked() {
            action = Some(Action::SetNotificationSide(NotificationSide::Right));
        }
    });
    action
}

/// Modo de imágenes: sirve para comprobar cuánta RAM se va en GIFs y
/// perfiles (avatares, banners, decoraciones, efectos).
fn image_mode_section(ui: &mut egui::Ui, palette: &Palette) -> Option<Action> {
    use crate::ui::anim::{image_mode, ImageMode};
    let current = image_mode();
    let mut action = None;
    ui.add_space(22.0);
    theme::text(ui, "Imágenes y uso de memoria", theme::semibold(14.0), palette.text);
    ui.add_space(4.0);
    theme::subtle(
        ui,
        palette,
        "Si ecord usa mucha RAM, probá \"Solo estáticas\" o \"Sin imágenes de perfil\" y mirá si baja.",
    );
    ui.add_space(8.0);
    let mb = |b: usize| b as f32 / (1024.0 * 1024.0);
    let stats = crate::ui::anim::mem_stats(ui.ctx());
    theme::subtle(
        ui,
        palette,
        &format!(
            "Imágenes en RAM: {:.0} MB decodificadas · {:.0} MB descargadas · {:.0} MB animadas",
            mb(stats.decoded),
            mb(stats.downloaded),
            mb(stats.animations)
        ),
    );
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if theme::soft_button(ui, palette, None, "Normal", current == ImageMode::Normal).clicked() {
            action = Some(Action::SetImageMode(ImageMode::Normal));
        }
        if theme::soft_button(ui, palette, None, "Solo estáticas", current == ImageMode::StaticOnly).clicked() {
            action = Some(Action::SetImageMode(ImageMode::StaticOnly));
        }
        if theme::soft_button(ui, palette, None, "Sin imágenes de perfil", current == ImageMode::NoProfileImages).clicked() {
            action = Some(Action::SetImageMode(ImageMode::NoProfileImages));
        }
    });
    action
}

/// Selector de modo (oscuro/claro/wallpaper) + lista de temas propios +
/// botón para crear uno nuevo. Es la vista de entrada del panel.
fn theme_picker_view(
    ui: &mut egui::Ui,
    palette: &Palette,
    mode: &ThemeMode,
    custom_themes: &[ThemeDef],
    wallpaper_seed: Option<[u8; 3]>,
) -> Option<Action> {
    let mut action = None;

    theme::subtle(
        ui,
        palette,
        "Elegí un tema, activá el fondo dinámico según tu wallpaper, o creá el tuyo.",
    );
    ui.add_space(16.0);

    ui.horizontal_wrapped(|ui| {
        if theme::soft_button(ui, palette, Some(Icon::Moon), "Oscuro", *mode == ThemeMode::Dark)
            .clicked()
        {
            action = Some(Action::SetMode(ThemeMode::Dark));
        }
        if theme::soft_button(ui, palette, Some(Icon::Sun), "Claro", *mode == ThemeMode::Light)
            .clicked()
        {
            action = Some(Action::SetMode(ThemeMode::Light));
        }
        if theme::soft_button(
            ui,
            palette,
            Some(Icon::Monitor),
            "Fondo de escritorio",
            *mode == ThemeMode::Wallpaper,
        )
        .clicked()
        {
            action = Some(Action::SetMode(ThemeMode::Wallpaper));
        }
    });

    if *mode == ThemeMode::Wallpaper {
        ui.add_space(10.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    let swatch = wallpaper_seed
                        .map(|s| Color32::from_rgb(s[0], s[1], s[2]))
                        .unwrap_or(palette.dim);
                    ui.painter().circle_filled(rect.center(), 8.0, swatch);
                    let msg = if wallpaper_seed.is_some() {
                        "Usando el color dominante de tu fondo de escritorio."
                    } else {
                        "Buscando el color de tu fondo de escritorio…"
                    };
                    theme::text(ui, msg, theme::regular(12.5), palette.secondary);
                });
                theme::text(
                    ui,
                    "Se actualiza solo si cambiás el fondo de escritorio.",
                    theme::regular(11.5),
                    palette.dim,
                );
            });
    }

    ui.add_space(20.0);

    if !custom_themes.is_empty() {
        theme::section_title(ui, palette, "Tus temas");
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for def in custom_themes {
                let active = matches!(mode, ThemeMode::Custom(n) if n == &def.name);
                let accent = crate::theme::from_hex(&def.accent);
                Frame::new()
                    .fill(if active { palette.surface_active } else { palette.surface })
                    .stroke(Stroke::new(
                        1.0,
                        if active { palette.accent } else { palette.outline },
                    ))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                            ui.painter().circle_filled(rect.center(), 6.0, accent);
                            if theme::link(ui, def.name.clone(), theme::medium(13.0), palette.text)
                                .clicked()
                            {
                                action = Some(Action::SetMode(ThemeMode::Custom(def.name.clone())));
                            }
                            if theme::icon_button(ui, Icon::Pencil, 12.0, palette.dim, palette.text, "Editar")
                                .clicked()
                            {
                                action = Some(Action::OpenEdit(def.name.clone()));
                            }
                            if theme::icon_button(
                                ui,
                                Icon::Trash,
                                12.0,
                                palette.dim,
                                palette.danger,
                                "Eliminar",
                            )
                            .clicked()
                            {
                                action = Some(Action::Delete(def.name.clone()));
                            }
                        });
                    });
            }
        });
        ui.add_space(14.0);
    }

    if theme::pill_button(ui, palette, "+ Crear tema nuevo", false).clicked() {
        action = Some(Action::OpenNew);
    }

    ui.add_space(20.0);
    theme::section_title(ui, palette, "Importar tema");
    theme::text(
        ui,
        "Pegá el JSON que exporta el editor de temas (o arrastrá el archivo .json a esta ventana).",
        theme::regular(12.5),
        palette.secondary,
    );
    ui.add_space(6.0);
    // El texto vive en la memoria temporal de egui: no hace falta un campo
    // nuevo en `App` para algo que solo existe mientras se está pegando.
    let id = egui::Id::new("theme_import_text");
    let mut text: String = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or_default());
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text("[ { \"name\": \"Mi tema\", ... } ]")
            .font(egui::TextStyle::Monospace)
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );
    ui.ctx().data_mut(|d| d.insert_temp(id, text.clone()));
    ui.add_space(6.0);
    if theme::pill_button(ui, palette, "Importar", true).clicked() {
        action = Some(Action::ImportThemes(text));
    }

    action
}

/// Editor de un tema (nuevo o existente): nombre, vista previa en vivo, y
/// un color picker por cada rol de [`Palette`].
fn theme_editor_view(
    ui: &mut egui::Ui,
    editor: &mut crate::theme::ThemeEditor,
    palette: &Palette,
) -> Option<Action> {
    let mut action = None;

    if theme::link(ui, "← Volver", theme::medium(13.0), palette.secondary).clicked() {
        action = Some(Action::CancelEditor);
    }
    ui.add_space(10.0);

    let title = if editor.editing_name.is_some() {
        "Editar tema"
    } else {
        "Crear tema"
    };
    theme::section_title(ui, palette, title);
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        theme::subtle(ui, palette, "Nombre");
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut editor.name)
                .hint_text("Mi tema")
                .desired_width(220.0),
        );
    });
    ui.add_space(12.0);

    // Vista previa en vivo: se arma con los colores y el fondo tal como
    // están en este mismo frame, así que se ve el cambio apenas se toca
    // cualquier control.
    backdrop_preview(ui, editor);
    ui.add_space(16.0);

    theme::subtle(ui, palette, "Colores");
    ui.add_space(6.0);

    color_row(ui, palette, "Fondo de ventana", &mut editor.window);
    color_row(ui, palette, "Panel", &mut editor.panel);
    color_row(ui, palette, "Superficie", &mut editor.surface);
    color_row(ui, palette, "Superficie (hover)", &mut editor.surface_hover);
    color_row(ui, palette, "Superficie (activa)", &mut editor.surface_active);
    color_row(ui, palette, "Contorno", &mut editor.outline);
    color_row(ui, palette, "Texto", &mut editor.text);
    color_row(ui, palette, "Texto secundario", &mut editor.secondary);
    color_row(ui, palette, "Texto tenue", &mut editor.dim);
    color_row(ui, palette, "Acento", &mut editor.accent);
    color_row(ui, palette, "Acento (hover)", &mut editor.accent_hover);
    color_row(ui, palette, "Texto sobre acento", &mut editor.on_accent);
    color_row(ui, palette, "Peligro", &mut editor.danger);
    color_row(ui, palette, "Advertencia", &mut editor.warning);

    ui.add_space(16.0);
    backdrop_section(ui, editor, palette);

    ui.add_space(18.0);
    ui.horizontal(|ui| {
        if theme::pill_button(ui, palette, "Guardar tema", true).clicked() {
            action = Some(Action::SaveEditor);
        }
        if theme::pill_button(ui, palette, "Cancelar", false).clicked() {
            action = Some(Action::CancelEditor);
        }
    });

    action
}

/// Maqueta de la app (barra de servidores + canales + chat) pintada con el
/// tema que se está editando: el fondo, y encima los paneles con su
/// translucidez. Sobre un tablero gris cuando la ventana es transparente,
/// para que se note qué parte deja ver el escritorio.
fn backdrop_preview(ui: &mut egui::Ui, editor: &mut crate::theme::ThemeEditor) {
    let preview = editor.to_palette();
    let width = ui.available_width().min(680.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 170.0), Sense::hover());
    let painter = ui.painter_at(rect);

    if editor.backdrop.background_opacity < 0.999 {
        theme::paint_checkerboard(&painter, rect);
    }
    editor
        .preview_rt
        .paint(ui.ctx(), &painter, rect, &editor.backdrop, &preview);

    let rail = egui::Rect::from_min_size(rect.min, Vec2::new(46.0, rect.height()));
    let side = egui::Rect::from_min_size(
        egui::pos2(rail.right(), rect.top()),
        Vec2::new(140.0, rect.height()),
    );
    let main = egui::Rect::from_min_max(egui::pos2(side.right(), rect.top()), rect.max);
    painter.rect_filled(rail, 0.0, preview.window);
    painter.rect_filled(side, 0.0, preview.panel);
    painter.rect_filled(main, 0.0, preview.window);

    for k in 0..3 {
        let color = if k == 0 { preview.accent } else { preview.surface };
        painter.circle_filled(
            egui::pos2(rail.center().x, rail.top() + 26.0 + k as f32 * 38.0),
            14.0,
            color,
        );
    }
    for (k, name) in ["# general", "# ayuda", "# música"].iter().enumerate() {
        let row = egui::Rect::from_min_size(
            egui::pos2(side.left() + 8.0, side.top() + 12.0 + k as f32 * 28.0),
            Vec2::new(side.width() - 16.0, 24.0),
        );
        if k == 0 {
            painter.rect_filled(row, CornerRadius::same(theme::RADIUS_SMALL), preview.surface_active);
        }
        painter.text(
            row.left_center() + Vec2::new(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            name,
            theme::regular(12.0),
            if k == 0 { preview.text } else { preview.secondary },
        );
    }
    for (k, line) in ["Así se ve un mensaje", "y otro, sobre una superficie"].iter().enumerate() {
        let bubble = egui::Rect::from_min_size(
            egui::pos2(main.left() + 14.0, main.top() + 14.0 + k as f32 * 44.0),
            Vec2::new(main.width() - 28.0, 36.0),
        );
        painter.rect_filled(bubble, CornerRadius::same(theme::RADIUS_SMALL), preview.surface);
        painter.text(
            bubble.left_center() + Vec2::new(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            line,
            theme::regular(12.0),
            preview.text,
        );
    }
    let pill = egui::Rect::from_min_size(
        egui::pos2(main.right() - 92.0, rect.bottom() - 38.0),
        Vec2::new(78.0, 26.0),
    );
    painter.rect_filled(pill, CornerRadius::same(13), preview.accent);
    painter.text(
        pill.center(),
        egui::Align2::CENTER_CENTER,
        "Acento",
        theme::medium(12.0),
        preview.on_accent,
    );
}

/// Slider de porcentaje (0–100 %) sobre un valor 0..1.
fn pct_slider(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32) {
    ui.spacing_mut().slider_width = 200.0;
    ui.add(
        egui::Slider::new(value, min..=max)
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
            .custom_parser(|s| s.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)),
    );
}

fn slider_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(200.0, 20.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .font(theme::regular(13.0))
                    .color(palette.text),
            ),
        );
        pct_slider(ui, value, min, max);
    });
    ui.add_space(4.0);
}

/// "Fondo" y "Transparencia" del editor de temas: tipo de fondo, degradado
/// (colores, ángulo/centro), imagen (archivo, ajuste, desenfoque, velo) y las
/// tres opacidades. Solo edita `editor.backdrop`; nada se aplica a la app
/// hasta "Guardar tema".
fn backdrop_section(ui: &mut egui::Ui, editor: &mut crate::theme::ThemeEditor, palette: &Palette) {
    use crate::theme::{Backdrop, BackdropKind, GradientStop, ImageFit};

    // Arrastrar una imagen a la ventana la elige como fondo (sin diálogo de
    // archivos: no hace falta ninguna dependencia nueva para esto).
    let dropped = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .first()
            .map(|f| f.path().to_path_buf())
    });
    if let Some(path) = dropped {
        editor.backdrop.image_path = path.to_string_lossy().into_owned();
        editor.backdrop.kind = BackdropKind::Image;
    }

    theme::subtle(ui, palette, "Fondo");
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (kind, label) in [
            (BackdropKind::Solid, "Sólido"),
            (BackdropKind::Linear, "Degradado"),
            (BackdropKind::Radial, "Radial"),
            (BackdropKind::Image, "Imagen"),
            (BackdropKind::DesktopWallpaper, "Wallpaper del sistema"),
        ] {
            if theme::soft_button(ui, palette, None, label, editor.backdrop.kind == kind).clicked() {
                editor.backdrop.kind = kind;
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        theme::subtle(ui, palette, "Ajustes rápidos");
        for (name, preset) in Backdrop::presets() {
            if theme::soft_button(ui, palette, None, name, false).clicked() {
                // Conserva la imagen ya elegida: el preset solo toca degradado y opacidades.
                let image_path = std::mem::take(&mut editor.backdrop.image_path);
                editor.backdrop = Backdrop { image_path, ..preset };
            }
        }
    });
    ui.add_space(10.0);

    match editor.backdrop.kind {
        BackdropKind::Solid => {
            theme::text(
                ui,
                "Usa el color «Fondo de ventana» de arriba.",
                theme::regular(12.5),
                palette.secondary,
            );
        }
        BackdropKind::Linear | BackdropKind::Radial => {
            let mut remove = None;
            let count = editor.backdrop.stops.len();
            for (i, stop) in editor.backdrop.stops.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::color_picker::color_edit_button_srgba(
                        ui,
                        &mut stop.color,
                        egui::color_picker::Alpha::Opaque,
                    );
                    ui.add_space(6.0);
                    pct_slider(ui, &mut stop.pos, 0.0, 1.0);
                    if count > 2
                        && theme::icon_button(ui, Icon::X, 12.0, palette.dim, palette.text, "Quitar color")
                            .clicked()
                    {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                editor.backdrop.stops.remove(i);
            }
            if editor.backdrop.stops.len() < 5
                && theme::soft_button(ui, palette, Some(Icon::Plus), "Añadir color", false).clicked()
            {
                editor.backdrop.stops.push(GradientStop { pos: 0.5, color: palette.accent });
            }
            ui.add_space(6.0);
            if editor.backdrop.kind == BackdropKind::Linear {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        Vec2::new(200.0, 20.0),
                        egui::Label::new(
                            egui::RichText::new("Ángulo")
                                .font(theme::regular(13.0))
                                .color(palette.text),
                        ),
                    );
                    ui.spacing_mut().slider_width = 200.0;
                    ui.add(egui::Slider::new(&mut editor.backdrop.angle, 0.0..=360.0).suffix("°"));
                });
            } else {
                slider_row(ui, palette, "Centro (horizontal)", &mut editor.backdrop.center[0], 0.0, 1.0);
                slider_row(ui, palette, "Centro (vertical)", &mut editor.backdrop.center[1], 0.0, 1.0);
                slider_row(ui, palette, "Radio", &mut editor.backdrop.radius, 0.2, 2.0);
            }
        }
        BackdropKind::Image | BackdropKind::DesktopWallpaper => {
            if editor.backdrop.kind == BackdropKind::Image {
                ui.horizontal(|ui| {
                    theme::subtle(ui, palette, "Archivo");
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.backdrop.image_path)
                            .hint_text("Ruta de la imagen, o arrastrá un archivo a esta ventana")
                            .desired_width(380.0),
                    );
                });
                let path = editor.backdrop.image_path.trim();
                if !path.is_empty() && !std::path::Path::new(path).is_file() {
                    theme::text(ui, "No se encuentra ese archivo.", theme::regular(12.0), palette.danger);
                }
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                theme::subtle(ui, palette, "Ajuste");
                ui.add_space(8.0);
                for (fit, label) in [
                    (ImageFit::Cover, "Llenar"),
                    (ImageFit::Contain, "Ajustar"),
                    (ImageFit::Stretch, "Estirar"),
                ] {
                    if theme::soft_button(ui, palette, None, label, editor.backdrop.image_fit == fit)
                        .clicked()
                    {
                        editor.backdrop.image_fit = fit;
                    }
                }
            });
            ui.add_space(6.0);
            slider_row(ui, palette, "Desenfoque", &mut editor.backdrop.image_blur, 0.0, 1.0);
            slider_row(ui, palette, "Velo (para leer mejor)", &mut editor.backdrop.image_dim, 0.0, 0.9);
        }
    }

    ui.add_space(12.0);
    theme::subtle(ui, palette, "Transparencia");
    ui.add_space(6.0);
    // Mínimos: con la ventana casi invisible en algunos sistemas los clics
    // "atraviesan" los píxeles transparentes y no se podría volver a editar.
    slider_row(ui, palette, "Ventana (ver el escritorio)", &mut editor.backdrop.background_opacity, 0.2, 1.0);
    slider_row(ui, palette, "Paneles", &mut editor.backdrop.panel_opacity, 0.0, 1.0);
    slider_row(ui, palette, "Superficies", &mut editor.backdrop.surface_opacity, 0.1, 1.0);
    if editor.backdrop.background_opacity < 0.999 {
        theme::text(
            ui,
            "La transparencia de la ventana necesita un compositor de escritorio (Windows y macOS lo traen; en Linux, según tu entorno). Sin él se ve negro.",
            theme::regular(11.5),
            palette.dim,
        );
    }
}

/// Pestaña "Voz y audio": dispositivo de entrada/salida, si el micrófono
/// puede transmitir, supresión de ruido, y los sliders de sensibilidad/
/// volumen. Mismo patrón que `theme_picker_view`: solo lee `app` (ya
/// clonado en `show`), nunca lo toca directo — junta lo que se clickeó o
/// arrastró en `action` y listo.
///
/// `microphone_buffer_ms` queda afuera a propósito: es un ajuste de
/// config avanzado (como en concord, que tampoco lo expone en su UI de
/// opciones), no algo que un usuario común necesite tocar desde acá.
fn voice_settings_view(
    ui: &mut egui::Ui,
    palette: &Palette,
    audio: &VoiceAudioSettings,
    sources: &VoiceAudioSources,
    source_options: &VoiceAudioSourceOptions,
) -> Option<Action> {
    let mut action = None;

    theme::subtle(
        ui,
        palette,
        "Elegí tu micrófono y tu salida de audio, y ajustá cómo se captura y se escucha tu voz en las llamadas.",
    );
    ui.add_space(16.0);

    theme::section_title(ui, palette, "Dispositivos");
    ui.add_space(8.0);

    voice_device_row(
        ui,
        palette,
        "Entrada (micrófono)",
        "Predeterminada del sistema",
        sources.input.as_deref(),
        source_options.input_label(sources.input.as_deref()),
        source_options.inputs(),
        "voice_settings_input_device",
        &mut action,
        Action::SetVoiceInputSource,
    );
    ui.add_space(6.0);
    voice_device_row(
        ui,
        palette,
        "Salida",
        "Predeterminada del sistema",
        sources.output.as_deref(),
        source_options.output_label(sources.output.as_deref()),
        source_options.outputs(),
        "voice_settings_output_device",
        &mut action,
        Action::SetVoiceOutputSource,
    );

    ui.add_space(20.0);
    theme::section_title(ui, palette, "Micrófono");
    ui.add_space(8.0);

    if theme::soft_button(
        ui,
        palette,
        None,
        "Permitir que el micrófono transmita",
        audio.allow_microphone_transmit,
    )
    .clicked()
    {
        action = Some(Action::SetAllowMicrophoneTransmit(!audio.allow_microphone_transmit));
    }
    theme::text(
        ui,
        "Apagado por default: hasta que lo prendas acá, unirte a un canal de voz no manda \
         audio tuyo, aunque no estés silenciado.",
        theme::regular(11.5),
        palette.dim,
    );
    ui.add_space(12.0);

    ui.add_enabled_ui(audio.allow_microphone_transmit, |ui| {
        if theme::soft_button(ui, palette, None, "Supresión de ruido", audio.noise_suppression)
            .clicked()
        {
            action = Some(Action::SetNoiseSuppression(!audio.noise_suppression));
        }
        ui.add_space(12.0);

        let mut sensitivity = i32::from(audio.microphone_sensitivity.value());
        let sensitivity_resp = ui.add(
            egui::Slider::new(&mut sensitivity, -100..=0)
                .suffix(" dB")
                .text("Sensibilidad (más bajo detecta un audio más suave)"),
        );
        if sensitivity_resp.changed() {
            action = Some(Action::SetMicrophoneSensitivity(sensitivity as i8));
        }
        ui.add_space(6.0);

        let mut mic_volume = i32::from(audio.microphone_volume.value());
        let mic_volume_resp = ui.add(
            egui::Slider::new(&mut mic_volume, 0..=i32::from(VoiceVolumePercent::maximum()))
                .suffix("%")
                .text("Volumen del micrófono"),
        );
        if mic_volume_resp.changed() {
            action = Some(Action::SetMicrophoneVolume(mic_volume as u8));
        }
    });

    ui.add_space(20.0);
    theme::section_title(ui, palette, "Salida");
    ui.add_space(8.0);

    let mut output_volume = i32::from(audio.voice_output_volume.value());
    let output_volume_resp = ui.add(
        egui::Slider::new(&mut output_volume, 0..=i32::from(VoiceVolumePercent::maximum()))
            .suffix("%")
            .text("Volumen de salida"),
    );
    if output_volume_resp.changed() {
        action = Some(Action::SetOutputVolume(output_volume as u8));
    }

    action
}

/// Una fila "etiqueta + combo de dispositivos", compartida entre entrada
/// y salida. `on_pick` arma la `Action` correspondiente (uno de los dos
/// constructores de variante, `Action::SetVoiceInputSource` o
/// `Action::SetVoiceOutputSource`) a partir del id elegido (`None` para
/// "predeterminado del sistema").
fn voice_device_row<'a>(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    default_label: &str,
    selected_id: Option<&str>,
    selected_text: String,
    options: impl Iterator<Item = (&'a str, &'a str)>,
    id_salt: &str,
    action: &mut Option<Action>,
    on_pick: impl Fn(Option<String>) -> Action,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(160.0, 20.0),
            egui::Label::new(
                egui::RichText::new(label).font(theme::regular(13.0)).color(palette.text),
            ),
        );
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected_text)
            .width(300.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(selected_id.is_none(), default_label).clicked() {
                    *action = Some(on_pick(None));
                }
                for (id, device_label) in options {
                    let selected = selected_id == Some(id);
                    if ui.selectable_label(selected, device_label).clicked() {
                        *action = Some(on_pick(Some(id.to_owned())));
                    }
                }
            });
    });
}

/// Una fila "etiqueta + color picker", con las etiquetas alineadas entre
/// sí para que la columna de colores quede pareja.
fn color_row(ui: &mut egui::Ui, palette: &Palette, label: &str, color: &mut Color32) {
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(160.0, 20.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .font(theme::regular(13.0))
                    .color(palette.text),
            ),
        );
        egui::color_picker::color_edit_button_srgba(ui, color, egui::color_picker::Alpha::Opaque);
    });
    ui.add_space(4.0);
}
