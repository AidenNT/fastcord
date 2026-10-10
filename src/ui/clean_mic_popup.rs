//! Popup de "Micrófono y Clean Mic": todo lo del micrófono en un solo lugar.
//!
//! Arriba el dispositivo de entrada, si puede transmitir, la supresión de
//! ruido, la sensibilidad y el volumen (los ajustes de siempre, que viven en
//! `App::voice.audio`); abajo cada etapa de la cadena de Clean Mic con su
//! interruptor y sus parámetros (`discord::clean_mic`). Los parámetros de la
//! cadena se aplican en vivo: `voice/noise.rs` los relee en el frame siguiente.
//!
//! Mismo patrón que `ui::settings`: lo que hace falta de `App` se clona antes
//! de dibujar, el closure solo junta lo que tocó el usuario y recién después
//! se aplica sobre `app`.

use std::time::Duration;

use egui::{Area, Color32, CornerRadius, Frame, Id, Margin, Order, RichText, ScrollArea, Sense, Stroke, UiBuilder, Vec2};

use crate::discord::VoiceVolumePercent;
use crate::discord::clean_mic::{self, CleanMicSettings, EQ_BAND_NAMES, MeterSnapshot, PRESETS};
use crate::lib::state::App;
use crate::ui::theme::{self, Icon, Palette};

const CARD_SIZE: Vec2 = Vec2::new(640.0, 700.0);

enum Action {
    Close,
    SetInput(Option<String>),
    SetAllowTransmit(bool),
    SetNoiseSuppression(bool),
    SetSensitivity(i8),
    SetVolume(u8),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if !app.clean_mic_open {
        return;
    }

    let palette = app.palette;
    let audio = app.voice.audio.clone();
    let sources = app.voice.audio_sources.clone();
    let options = app.voice_audio_source_options.clone();
    let ctx = ui.ctx().clone();
    let screen_rect = ctx.viewport_rect();

    let mut s = clean_mic::current();
    let original = s.clone();
    let meter = clean_mic::meter();
    let mut action: Option<Action> = None;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(Action::Close);
    }
    // El medidor se mueve solo: pedir repintado mientras el popup está abierto.
    ctx.request_repaint_after(Duration::from_millis(50));

    Area::new(Id::new("clean_mic_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(&ctx, |ui| {
            ui.set_width(screen_rect.width());
            ui.set_height(screen_rect.height());
            ui.painter().rect_filled(screen_rect, 0.0, Color32::from_black_alpha(150));
            ui.interact(screen_rect, Id::new("clean_mic_scrim_block"), Sense::click());

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
                .corner_radius(CornerRadius::same(theme::radius() + 6))
                .inner_margin(Margin::same(22))
                .shadow(egui::epaint::Shadow { offset: [0, 16], blur: 40, spread: 0, color: palette.shadow })
                .show(&mut card_ui, |ui| {
                    ui.set_width(size.x - 44.0);
                    ui.set_height(size.y - 44.0);

                    ui.horizontal(|ui| {
                        theme::text(ui, "Micrófono y Clean Mic", theme::bold(19.0), palette.text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, Icon::X, 15.0, palette.dim, palette.text, "Cerrar").clicked() {
                                action = Some(Action::Close);
                            }
                        });
                    });
                    ui.add_space(10.0);

                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        meter_bar(ui, &palette, meter, audio.noise_suppression);
                        ui.add_space(14.0);

                        // ------------------------------------------------ micrófono
                        theme::section_title(ui, &palette, "Micrófono");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                Vec2::new(150.0, 20.0),
                                egui::Label::new(RichText::new("Entrada").font(theme::regular(13.0)).color(palette.text)),
                            );
                            egui::ComboBox::from_id_salt("clean_mic_input_device")
                                .selected_text(options.input_label(sources.input.as_deref()))
                                .width(320.0)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(sources.input.is_none(), "Predeterminada del sistema")
                                        .clicked()
                                    {
                                        action = Some(Action::SetInput(None));
                                    }
                                    for (id, label) in options.inputs() {
                                        if ui.selectable_label(sources.input.as_deref() == Some(id), label).clicked() {
                                            action = Some(Action::SetInput(Some(id.to_owned())));
                                        }
                                    }
                                });
                        });
                        ui.add_space(8.0);
                        if theme::soft_button(
                            ui,
                            &palette,
                            None,
                            "Permitir que el micrófono transmita",
                            audio.allow_microphone_transmit,
                        )
                        .clicked()
                        {
                            action = Some(Action::SetAllowTransmit(!audio.allow_microphone_transmit));
                        }
                        ui.add_space(6.0);
                        ui.add_enabled_ui(audio.allow_microphone_transmit, |ui| {
                            if theme::soft_button(
                                ui,
                                &palette,
                                None,
                                "Limpieza de voz (Clean Mic)",
                                audio.noise_suppression,
                            )
                            .clicked()
                            {
                                action = Some(Action::SetNoiseSuppression(!audio.noise_suppression));
                            }
                            hint(ui, &palette, "Interruptor general: apagado, la voz sale sin procesar y las etapas de abajo no se aplican.");
                            ui.add_space(8.0);

                            let mut sensitivity = i32::from(audio.microphone_sensitivity.value());
                            if ui
                                .add(
                                    egui::Slider::new(&mut sensitivity, -100..=0)
                                        .suffix(" dB")
                                        .text("Sensibilidad (más bajo detecta un audio más suave)"),
                                )
                                .changed()
                            {
                                action = Some(Action::SetSensitivity(sensitivity as i8));
                            }
                            ui.add_space(4.0);
                            let mut volume = i32::from(audio.microphone_volume.value());
                            if ui
                                .add(
                                    egui::Slider::new(&mut volume, 0..=i32::from(VoiceVolumePercent::maximum()))
                                        .suffix("%")
                                        .text("Volumen del micrófono"),
                                )
                                .changed()
                            {
                                action = Some(Action::SetVolume(volume as u8));
                            }
                        });

                        // ------------------------------------------- limpieza
                        ui.add_space(18.0);
                        theme::section_title(ui, &palette, "Ruido y eco");
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Cancelación de eco (AEC3)", &mut s.echo_cancel);
                        hint(ui, &palette, "Resta de tu micrófono lo que suena en tus parlantes. Casi no hace falta con auriculares.");
                        ui.add_space(8.0);
                        pct_slider(ui, &mut s.strength, "Fuerza de la limpieza (cuánto baja el fondo entre palabras)");

                        ui.add_space(18.0);
                        theme::section_title(ui, &palette, "Compuerta de voz");
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Compuerta de voz", &mut s.gate);
                        ui.add_enabled_ui(s.gate, |ui| {
                            ui.add(
                                egui::Slider::new(&mut s.gate_margin_db, 0.0..=20.0)
                                    .suffix(" dB")
                                    .text("Cuánto más fuerte que el fondo para abrir"),
                            );
                            pct_slider(ui, &mut s.gate_vad, "Confianza de voz para abrir");
                            ui.add(
                                egui::Slider::new(&mut s.gate_hold_ms, 0.0..=800.0)
                                    .suffix(" ms")
                                    .text("Seguir abierta después de hablar"),
                            );
                        });

                        ui.add_space(18.0);
                        theme::section_title(ui, &palette, "Graves y volumen");
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Filtro de retumbos (pasa-altos)", &mut s.high_pass);
                        ui.add_enabled_ui(s.high_pass, |ui| {
                            ui.add(
                                egui::Slider::new(&mut s.high_pass_hz, 40.0..=300.0)
                                    .suffix(" Hz")
                                    .text("Cortar por debajo de"),
                            );
                        });
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Volumen automático", &mut s.agc);
                        ui.add_enabled_ui(s.agc, |ui| {
                            pct_slider(ui, &mut s.loudness, "Nivel de la voz (50% = normal)");
                        });

                        ui.add_space(18.0);
                        theme::section_title(ui, &palette, "Tono (ecualizador)");
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Ecualizador", &mut s.eq);
                        ui.add_enabled_ui(s.eq, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for (key, name, _, _) in PRESETS {
                                    if theme::soft_button(ui, &palette, None, name, s.eq_preset == key).clicked() {
                                        s.eq_preset = key.to_owned();
                                        s.eq_bands = clean_mic::bands_for(key);
                                    }
                                }
                            });
                            let description = PRESETS
                                .iter()
                                .find(|(key, ..)| *key == s.eq_preset)
                                .map_or("Tu propio sonido. Elegí un estilo para empezar de nuevo.", |(_, _, d, _)| *d);
                            hint(ui, &palette, description);
                            egui::CollapsingHeader::new("Ajuste fino por banda").default_open(false).show(ui, |ui| {
                                let mut touched = false;
                                for (i, band) in s.eq_bands.iter_mut().enumerate() {
                                    let name = EQ_BAND_NAMES[i];
                                    touched |= ui
                                        .add(
                                            egui::Slider::new(&mut band.gain, -18.0..=18.0)
                                                .suffix(" dB")
                                                .text(format!("{name}: ganancia")),
                                        )
                                        .changed();
                                    touched |= ui
                                        .add(
                                            egui::Slider::new(&mut band.freq, 20.0..=20_000.0)
                                                .logarithmic(true)
                                                .suffix(" Hz")
                                                .text(format!("{name}: frecuencia")),
                                        )
                                        .changed();
                                    if i != 0 && i != clean_mic::EQ_BANDS - 1 {
                                        touched |= ui
                                            .add(
                                                egui::Slider::new(&mut band.q, 0.2..=8.0).text(format!("{name}: ancho (Q)")),
                                            )
                                            .changed();
                                    }
                                    ui.add_space(6.0);
                                }
                                if touched {
                                    s.eq_preset = "custom".to_owned();
                                }
                            });
                        });

                        ui.add_space(18.0);
                        theme::section_title(ui, &palette, "Picos");
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Compresor", &mut s.compressor);
                        ui.add_enabled_ui(s.compressor, |ui| {
                            ui.add(
                                egui::Slider::new(&mut s.compressor_threshold_db, -40.0..=0.0)
                                    .suffix(" dB")
                                    .text("Umbral"),
                            );
                            ui.add(egui::Slider::new(&mut s.compressor_ratio, 1.0..=10.0).suffix(":1").text("Razón"));
                        });
                        ui.add_space(8.0);
                        toggle(ui, &palette, "Limitador (nunca satura)", &mut s.limiter);
                        ui.add_enabled_ui(s.limiter, |ui| {
                            ui.add(
                                egui::Slider::new(&mut s.limiter_ceiling_db, -12.0..=0.0)
                                    .suffix(" dB")
                                    .text("Techo"),
                            );
                        });

                        ui.add_space(20.0);
                        if theme::soft_button(ui, &palette, None, "Restablecer Clean Mic", false).clicked() {
                            s = CleanMicSettings::default();
                        }
                        ui.add_space(8.0);
                    });
                });
        });

    // Guardar en disco recién al soltar el mouse: un slider dispara cambios en
    // cada frame mientras se arrastra.
    let dirty_id = Id::new("clean_mic_dirty");
    if s != original {
        clean_mic::set(s);
        ctx.data_mut(|d| d.insert_temp(dirty_id, true));
    }
    let dirty = ctx.data(|d| d.get_temp::<bool>(dirty_id)).unwrap_or(false);
    if dirty && !ctx.input(|i| i.pointer.any_down()) {
        clean_mic::save_to_storage();
        ctx.data_mut(|d| d.insert_temp(dirty_id, false));
    }

    if let Some(action) = action {
        match action {
            Action::Close => app.clean_mic_open = false,
            Action::SetInput(id) => app.set_voice_input_source(id),
            Action::SetAllowTransmit(allow) => app.set_voice_allow_microphone_transmit(allow),
            Action::SetNoiseSuppression(enabled) => app.set_voice_noise_suppression(enabled),
            Action::SetSensitivity(db) => app.set_voice_microphone_sensitivity(db),
            Action::SetVolume(percent) => app.set_voice_microphone_volume(percent),
        }
    }
}

/// Interruptor de una etapa: el mismo botón "suave" que usan los Ajustes.
fn toggle(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &mut bool) {
    if theme::soft_button(ui, palette, None, label, *value).clicked() {
        *value = !*value;
    }
}

/// Slider 0–100 % sobre un valor 0..1 (solo escribe si el usuario lo movió).
fn pct_slider(ui: &mut egui::Ui, value: &mut f32, label: &str) {
    let mut pct = *value * 100.0;
    if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").text(label)).changed() {
        *value = pct / 100.0;
    }
}

fn hint(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).font(theme::regular(11.5)).color(palette.dim));
}

/// Barra con el nivel de entrada y el estado de la compuerta.
fn meter_bar(ui: &mut egui::Ui, palette: &Palette, meter: MeterSnapshot, cleaning_on: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(7), palette.surface);
    let live = meter.active && cleaning_on;
    if live {
        let fraction = ((meter.level_db + 80.0) / 80.0).clamp(0.0, 1.0);
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        let color = if meter.gate_open { palette.accent } else { palette.dim };
        ui.painter().rect_filled(filled, CornerRadius::same(7), color);
    }
    ui.add_space(4.0);
    let status = if !cleaning_on {
        "Limpieza apagada: se manda tu voz sin procesar."
    } else if !live {
        "Sin audio todavía: entrá a una llamada con el micrófono permitido y hablá para ver el medidor."
    } else if meter.gate_open {
        "Voz detectada: la compuerta está abierta."
    } else {
        "Solo ruido de fondo: la compuerta está cerrada."
    };
    hint(ui, palette, status);
}
