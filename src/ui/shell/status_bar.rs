//! Barra de estado: una franja fina, a todo el ancho, justo debajo de la barra
//! superior. Solo aparece cuando hay algo que contar y desaparece sola:
//!
//! * modelo de voz (DeepFilterNet): descargando (con progreso), cargando, listo,
//!   falló / no incluido en este build, o "elegido pero todavía no activo"
//!   (en ese caso la cadena usa RNNoise de respaldo);
//! * conexión con Discord caída (reconectando);
//! * actualización de la app descargándose o lista para instalar.
//!
//! El modelo de voz solo se avisa cuando hay algo fuera de lo normal (descarga,
//! carga lenta, fallo). Todo se lee de estados globales ya existentes
//! (`clean_mic`, `updater`, `App::gateway_disconnected`); este módulo no guarda nada salvo el último
//! estado del modelo, para poder mostrar "listo" unos segundos.

use egui::{Align, Color32, Frame, Layout, Margin, RichText, Sense, Vec2};

use crate::discord::clean_mic::{self, DeepFilterStatus, NoiseEngine};
use crate::lib::state::{App, Screen};
use crate::ui::theme::{self, Icon, Palette};
use crate::updater::{self, View};

/// Alto de la barra.
pub const HEIGHT: f32 = 28.0;

/// Cuánto se queda el aviso de "modelo listo" (segundos).
const READY_LINGER: f64 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Ok,
    Info,
    Warning,
    Error,
}

enum Lead {
    Spinner,
    Icon(Icon),
}

struct Item {
    level: Level,
    lead: Lead,
    text: String,
    /// 0..1, si hay un progreso que mostrar.
    progress: Option<f32>,
    tooltip: Option<&'static str>,
}

impl Item {
    fn new(level: Level, lead: Lead, text: impl Into<String>) -> Self {
        Self {
            level,
            lead,
            text: text.into(),
            progress: None,
            tooltip: None,
        }
    }

    fn progress(mut self, value: f32) -> Self {
        self.progress = Some(value.clamp(0.0, 1.0));
        self
    }

    fn tooltip(mut self, tip: &'static str) -> Self {
        self.tooltip = Some(tip);
        self
    }
}

/// Dibuja la barra si hay algo que mostrar. Se llama justo después de la barra superior.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if matches!(app.screen, Screen::Login) {
        return;
    }
    let now = ui.input(|i| i.time);
    let items = collect(app, ui.ctx(), now);
    if items.is_empty() {
        return;
    }
    // Los estados que cambian desde otros hilos (descarga, carga del modelo)
    // no piden repintado por su cuenta: se consulta unas veces por segundo.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));

    let palette = app.palette;
    let worst = items.iter().map(|item| item.level).max().unwrap_or(Level::Info);
    let tint = level_color(&palette, worst);

    egui::Panel::top("app_status_bar")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(palette.window_solid)
                .stroke(egui::Stroke::new(1.0, palette.outline))
                .inner_margin(Margin::symmetric(14, 0)),
        )
        .show(ui, |ui| {
            // Velo del color del estado más grave, encima del fondo sólido.
            ui.painter().rect_filled(ui.max_rect().expand2(Vec2::new(14.0, 0.0)), 0.0, tint.gamma_multiply(0.14));

            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(14.0);
                    }
                    draw_item(ui, &palette, item);
                }
            });
        });
}

fn draw_item(ui: &mut egui::Ui, palette: &Palette, item: &Item) {
    let color = level_color(palette, item.level);
    let response = ui
        .scope(|ui| {
            match item.lead {
                Lead::Spinner => {
                    theme::spinner(ui, 14.0, color);
                }
                Lead::Icon(icon) => {
                    theme::icon(ui, icon, 14.0, color);
                }
            }
            ui.add(egui::Label::new(RichText::new(&item.text).font(theme::medium(12.0)).color(palette.text)).wrap_mode(egui::TextWrapMode::Extend));
            if let Some(progress) = item.progress {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(90.0, 5.0), Sense::hover());
                ui.painter().rect_filled(rect, 3.0, palette.outline);
                let mut fill = rect;
                fill.set_width(rect.width() * progress);
                ui.painter().rect_filled(fill, 3.0, color);
            }
        })
        .response;
    if let Some(tip) = item.tooltip {
        let _ = response.on_hover_text(tip);
    }
}

fn level_color(palette: &Palette, level: Level) -> Color32 {
    match level {
        Level::Ok => Color32::from_rgb(0x3b, 0xc9, 0x7a),
        Level::Info => palette.accent,
        Level::Warning => palette.warning,
        Level::Error => palette.danger,
    }
}

/// Junta todos los estados que hay que mostrar ahora, del más importante al menos.
fn collect(app: &App, ctx: &egui::Context, now: f64) -> Vec<Item> {
    let mut items = Vec::new();

    if app.gateway_disconnected {
        items.push(
            Item::new(Level::Warning, Lead::Spinner, "Sin conexión con Discord. Reconectando…")
                .tooltip("Se reintenta solo; los mensajes nuevos llegarán al volver."),
        );
    }

    if let Some(item) = voice_model_item(app, ctx, now) {
        items.push(item);
    }

    match updater::view() {
        View::Downloading { version, received, total } => {
            let mut item = Item::new(Level::Info, Lead::Icon(Icon::Download), format!("Descargando actualización v{version}"));
            if total > 0 {
                item = item.progress(received as f32 / total as f32);
            }
            items.push(item);
        }
        View::Ready { version } => items.push(
            Item::new(
                Level::Ok,
                Lead::Icon(Icon::CircleCheck),
                format!("Actualización v{version} lista: reiniciá eCord para instalarla"),
            )
            .tooltip("Ajustes → Actualizaciones"),
        ),
        View::Installing => items.push(Item::new(Level::Info, Lead::Spinner, "Instalando actualización…")),
        _ => {}
    }

    items.sort_by(|a, b| b.level.cmp(&a.level));
    items
}

/// Estado del modelo de voz elegido en Clean Mic.
fn voice_model_item(app: &App, ctx: &egui::Context, now: f64) -> Option<Item> {
    let settings = clean_mic::current();
    if settings.engine != NoiseEngine::DeepFilterNet {
        return None;
    }
    let in_call = app.voice_target.is_some();

    if !clean_mic::DEEPFILTER_COMPILED {
        // Solo molesta cuando de verdad se está usando el micrófono.
        return in_call.then(|| {
            Item::new(
                Level::Warning,
                Lead::Icon(Icon::CircleAlert),
                "Modelo de voz DeepFilterNet no incluido en este build: se usa RNNoise",
            )
            .tooltip("Compilá con --features deepfilternet-ort, o elegí RNNoise en Clean Mic.")
        });
    }

    let status = clean_mic::deepfilter_status();

    // Recuerda cuándo cambió el estado, para mostrar "listo" unos segundos.
    let id = egui::Id::new("status_bar_deepfilter_seen");
    let (previous, changed_at) = ctx
        .data(|d| d.get_temp::<(DeepFilterStatus, f64)>(id))
        .unwrap_or((status, f64::NEG_INFINITY));
    let changed_at = if previous != status {
        ctx.data_mut(|d| d.insert_temp(id, (status, now)));
        now
    } else {
        if changed_at == f64::NEG_INFINITY {
            ctx.data_mut(|d| d.insert_temp(id, (status, f64::NEG_INFINITY)));
        }
        changed_at
    };

    // "Listo" solo tiene sentido si ESTA sesión bajó el modelo: al entrar a cada
    // llamada se vuelve a cargar desde el disco y eso no hace falta avisarlo.
    let dl_id = egui::Id::new("status_bar_deepfilter_downloaded");
    if status == DeepFilterStatus::DownloadingModel {
        ctx.data_mut(|d| d.insert_temp(dl_id, true));
    }
    let downloaded = ctx.data(|d| d.get_temp::<bool>(dl_id)).unwrap_or(false);

    match status {
        DeepFilterStatus::DownloadingModel => {
            let (done, total) = clean_mic::deepfilter_download_progress();
            let mut text = "Descargando modelo de voz (DeepFilterNet)".to_string();
            let mut item_progress = None;
            if total > 0 {
                text.push_str(&format!(" {done}/{total}"));
                item_progress = Some(f32::from(done) / f32::from(total));
            }
            text.push_str(": mientras tanto se usa RNNoise");
            let mut item = Item::new(Level::Info, Lead::Icon(Icon::Download), text);
            if let Some(progress) = item_progress {
                item = item.progress(progress);
            }
            Some(item)
        }
        // Cargar desde disco es rápido y normal; solo se avisa si se demora.
        DeepFilterStatus::Loading if now - changed_at > 3.0 => Some(Item::new(
            Level::Info,
            Lead::Spinner,
            "Cargando modelo de voz (DeepFilterNet): mientras tanto se usa RNNoise",
        )),
        DeepFilterStatus::Loading => None,
        DeepFilterStatus::Failed => Some(
            Item::new(
                Level::Error,
                Lead::Icon(Icon::CircleX),
                "No se pudo cargar el modelo de voz seleccionado: se usa RNNoise",
            )
            .tooltip("¿Sin conexión para bajarlo o falta onnxruntime? Mirá el detalle en Clean Mic."),
        ),
        DeepFilterStatus::Ready => {
            if downloaded && now - changed_at < READY_LINGER {
                Some(Item::new(Level::Ok, Lead::Icon(Icon::CircleCheck), "Modelo de voz DeepFilterNet listo y en uso"))
            } else {
                // Ya se avisó: las próximas cargas desde disco no vuelven a mostrarlo.
                if downloaded {
                    ctx.data_mut(|d| d.insert_temp(dl_id, false));
                }
                None
            }
        }
        DeepFilterStatus::Idle => {
            // Todavía no se pidió: solo vale la pena avisar si ya pasa audio por la cadena.
            (in_call && clean_mic::meter().active).then(|| {
                Item::new(
                    Level::Warning,
                    Lead::Icon(Icon::CircleAlert),
                    "El modelo de voz DeepFilterNet todavía no está activo: se usa RNNoise",
                )
            })
        }
    }
}
