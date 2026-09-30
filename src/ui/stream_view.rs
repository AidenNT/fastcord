//! Visor del stream (Go Live) que se está viendo. Se dibuja arriba de la
//! lista de conectados en la vista del canal de voz donde transmite la
//! persona (`ui::server::voice_channel_view`).
//!
//! El video llega ya decodificado como RGBA: `App::pump_stream_frames` lo sube
//! a `WatchedStream::texture` una vez por frame y acá solo se pinta.

use egui::{CornerRadius, Frame, Margin, Vec2};

use crate::discord::StreamWatchStatus;
use crate::lib::state::App;
use crate::ui::theme::{self, Icon, Palette};

/// Ancho máximo del video en la vista; más grande no aporta y se come la
/// lista de conectados.
const MAX_VIDEO_WIDTH: f32 = 960.0;

pub fn show(app: &mut App, ui: &mut egui::Ui, palette: &Palette, channel_id: &str) {
    let Some(watched) = app.watching_stream.as_ref() else { return };
    if watched.channel_id != channel_id {
        return;
    }
    let title = format!("Stream de {}", watched.owner_name);
    let status = watched.status;
    let message = watched.message.clone();
    let frame_size = watched.frame_size;
    let frames_shown = watched.frames_shown;
    let texture = watched.texture.clone();

    let mut close_clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                // `Frame::show` hereda el layout del contenedor (acá, el
                // `horizontal` de arriba, que da el margen izquierdo): sin este
                // `vertical` el encabezado y el video quedarían UNO AL LADO DEL
                // OTRO y el video, empujado fuera de la pantalla.
                ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Monitor, 14.0, palette.accent);
                    theme::text(ui, &title, theme::semibold(13.0), palette.text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            Icon::X,
                            14.0,
                            palette.dim,
                            palette.text,
                            "Dejar de ver el stream",
                        )
                        .clicked()
                        {
                            close_clicked = true;
                        }
                    });
                });
                ui.add_space(8.0);
                match texture.as_ref() {
                    Some(texture) if frame_size[0] > 0 && frame_size[1] > 0 => {
                        let width = (ui.available_width() - 24.0).clamp(160.0, MAX_VIDEO_WIDTH);
                        let aspect = frame_size[1] as f32 / frame_size[0] as f32;
                        ui.add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(Vec2::new(width, width * aspect)),
                        );
                        theme::text(
                            ui,
                            format!("{}x{} · {frames_shown} frames", frame_size[0], frame_size[1]),
                            theme::regular(11.0),
                            palette.dim,
                        );
                    }
                    _ if status == StreamWatchStatus::Failed => {
                        theme::text(
                            ui,
                            message.as_deref().unwrap_or("No se pudo conectar al stream"),
                            theme::regular(12.5),
                            palette.danger,
                        );
                    }
                    _ => {
                        let text = match status {
                            StreamWatchStatus::Connected => "Esperando el primer frame…",
                            StreamWatchStatus::Receiving => "Recibiendo video, mostrando el primer frame…",
                            _ => "Conectando al stream…",
                        };
                        theme::text(ui, text, theme::regular(12.5), palette.dim);
                    }
                }
                });
            });
    });
    ui.add_space(14.0);

    if close_clicked {
        app.stop_watching_stream();
    }
}
