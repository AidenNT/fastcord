//! Pestaña "Memoria" de Ajustes: contador de todo lo que usa RAM y caché.
//!
//! Solo dibuja: los números salen de `App::memory_report_cached`
//! (`lib/state/memory.rs`), que `ui::settings` calcula ANTES de abrir el
//! `Area`, igual que hace con las secciones de la cuenta.

use egui::{Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Vec2};

use crate::lib::state::memory::MemoryReport;
use crate::support::mem_report::{fmt_bytes, fmt_bytes_u64};
use crate::ui::theme::{self, Palette};

const C_BASE: Color32 = Color32::from_rgb(0x8a, 0x94, 0xa6);
const C_DISCORD: Color32 = Color32::from_rgb(0x58, 0x65, 0xf2);
const C_DECODED: Color32 = Color32::from_rgb(0x2e, 0xc4, 0xb6);
const C_DOWNLOADED: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
const C_GPU: Color32 = Color32::from_rgb(0xb3, 0x7f, 0xfe);
const C_DISK: Color32 = Color32::from_rgb(0x4d, 0x9d, 0xff);

/// Una fila: nombre a la izquierda, tamaño a la derecha y una barrita debajo.
struct Row<'a> {
    label: &'a str,
    bytes: u64,
    /// Lo que representa la barra completa (el total del que es parte).
    of: u64,
    color: Color32,
    /// Sangría: las filas con sangría son el detalle de la de arriba.
    indent: f32,
    /// Agrega el porcentaje (`bytes / of`) junto al tamaño.
    percent: bool,
    /// Texto del tooltip (vacío = sin tooltip).
    hint: &'a str,
}

pub fn view(ui: &mut egui::Ui, palette: &Palette, r: &MemoryReport) {
    let total = r.total as u64;

    // --- Cabecera: el número grande ---
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = if r.resident.is_some() {
                "RAM usada por eCord ahora"
            } else {
                "Memoria usada por eCord (heap de Rust; el sistema no informa la RAM del proceso)"
            };
            theme::text(ui, title, theme::regular(12.5), palette.secondary);
            ui.add_space(2.0);
            theme::text(ui, fmt_bytes(r.total), theme::bold(28.0), palette.text);
            if r.resident.is_some() {
                ui.add_space(2.0);
                note(
                    ui,
                    palette,
                    &format!(
                        "Heap de Rust: {}  ·  Fuera del heap (programa, librerías, drivers): {}",
                        fmt_bytes(r.heap),
                        fmt_bytes(r.total.saturating_sub(r.heap)),
                    ),
                );
            }
        });

    // --- En RAM ---
    section(ui, palette, "En RAM", "Lo que ocupa cada parte dentro del proceso.");
    row(
        ui,
        palette,
        Row {
            label: "Funcionamiento",
            bytes: r.base as u64,
            of: total,
            color: C_BASE,
            indent: 0.0,
            percent: true,
            hint: "El programa en sí: interfaz, fuentes, audio y voz, video (ffmpeg), \
                   librerías y fragmentación del allocator. Es lo que queda de la RAM \
                   total al restar todo lo demás.",
        },
    );

    let d = &r.discord;
    row(
        ui,
        palette,
        Row {
            label: "Datos de Discord (estimado)",
            bytes: d.total() as u64,
            of: total,
            color: C_DISCORD,
            indent: 0.0,
            percent: true,
            hint: "Todo lo que bajó el cliente de Discord y guarda en memoria. Es una \
                   estimación a partir del tamaño de las estructuras de datos.",
        },
    );
    let discord_total = d.total() as u64;
    for (label, bytes, hint) in [
        ("Mensajes", d.messages, "Mensajes de todos los canales, hilos y DMs que se cargaron."),
        ("Servidores y canales", d.structure, "Canales, categorías, roles, emojis, stickers y permisos."),
        ("Miembros y usuarios", d.members, "Listas de miembros, apodos, roles y usuarios conocidos."),
        ("Amigos y mensajes directos", d.friends_dms, "Lista de amigos y DMs (sin sus mensajes)."),
        ("Cuenta y estado", d.account, "Tu usuario, ajustes de la cuenta, presencias, menciones y notificaciones."),
    ] {
        row(
            ui,
            palette,
            Row { label, bytes: bytes as u64, of: discord_total, color: C_DISCORD, indent: 18.0, percent: false, hint },
        );
    }

    row(
        ui,
        palette,
        Row {
            label: "Imágenes decodificadas",
            bytes: r.images_decoded as u64,
            of: total,
            color: C_DECODED,
            indent: 0.0,
            percent: true,
            hint: "Avatares, emojis y adjuntos ya convertidos a píxeles por egui. Se sueltan \
                   solas al pasar de un tope o al cambiar de canal.",
        },
    );
    row(
        ui,
        palette,
        Row {
            label: "Imágenes descargadas (caché en RAM)",
            bytes: r.images_downloaded as u64,
            of: total,
            color: C_DOWNLOADED,
            indent: 0.0,
            percent: true,
            hint: "Archivos de imagen tal como llegaron de internet, guardados en memoria \
                   además de en el disco.",
        },
    );

    // --- GPU ---
    section(
        ui,
        palette,
        "En la GPU (texturas)",
        "No cuenta como RAM del proceso; en gráficos integrados usa memoria compartida.",
    );
    let gpu = r.gpu_total as u64;
    row(
        ui,
        palette,
        Row {
            label: "Texturas de la interfaz",
            bytes: gpu,
            of: gpu,
            color: C_GPU,
            indent: 0.0,
            percent: false,
            hint: "Todo lo que egui tiene subido a la GPU.",
        },
    );
    row(
        ui,
        palette,
        Row {
            label: "GIFs y animaciones",
            bytes: r.animations as u64,
            of: gpu,
            color: C_GPU,
            indent: 18.0,
            percent: false,
            hint: "Cuadros de GIF/APNG animados (avatares, emojis, adjuntos).",
        },
    );
    row(
        ui,
        palette,
        Row {
            label: "Fuentes, íconos e imágenes",
            bytes: gpu.saturating_sub(r.animations as u64),
            of: gpu,
            color: C_GPU,
            indent: 18.0,
            percent: false,
            hint: "",
        },
    );

    // --- Disco ---
    let disk = &r.disk;
    section(
        ui,
        palette,
        "Caché en disco",
        if disk.scanned { "Se puede borrar sin perder nada; se vuelve a bajar." } else { "Calculando…" },
    );
    let disk_total = disk.total();
    let images_label = format!("Imágenes y archivos bajados ({} archivos)", disk.image_files);
    row(
        ui,
        palette,
        Row {
            label: &images_label,
            bytes: disk.images,
            of: disk_total,
            color: C_DISK,
            indent: 0.0,
            percent: false,
            hint: "Avatares, emojis, íconos y adjuntos. Los archivos viejos y lo que pase del \
                   tope se borran solos al abrir eCord.",
        },
    );
    row(
        ui,
        palette,
        Row {
            label: "Catálogos (JSON)",
            bytes: disk.json,
            of: disk_total,
            color: C_DISK,
            indent: 0.0,
            percent: false,
            hint: "Catálogos que casi no cambian (efectos de perfil, coleccionables).",
        },
    );
    row(
        ui,
        palette,
        Row {
            label: "Sonidos de alertas",
            bytes: disk.sounds,
            of: disk_total,
            color: C_DISK,
            indent: 0.0,
            percent: false,
            hint: "Audios de las alertas, volcados del programa para que ffmpeg los pueda abrir.",
        },
    );
    ui.add_space(4.0);
    theme::text(
        ui,
        format!("Total en disco: {}", fmt_bytes_u64(disk_total)),
        theme::medium(12.5),
        palette.secondary,
    );

    ui.add_space(16.0);
    note(
        ui,
        palette,
        "Los números de «Datos de Discord» son estimaciones. La RAM total incluye las páginas \
         de librerías compartidas que se usaron, así que puede verse algo mayor que en el \
         Administrador de tareas.",
    );
}

fn section(ui: &mut egui::Ui, palette: &Palette, title: &str, subtitle: &str) {
    ui.add_space(20.0);
    theme::text(ui, title, theme::semibold(14.0), palette.text);
    ui.add_space(2.0);
    theme::subtle(ui, palette, subtitle);
    ui.add_space(8.0);
}

/// Texto chico con salto de línea.
fn note(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text).font(theme::regular(12.0)).color(palette.dim),
        )
        .wrap(),
    );
}

fn row(ui: &mut egui::Ui, palette: &Palette, spec: Row<'_>) {
    let nested = spec.indent > 0.0;
    let height = if nested { 28.0 } else { 36.0 };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());

    if ui.is_rect_visible(rect) {
        let (label_font, value_font, label_color) = if nested {
            (theme::regular(12.5), theme::regular(12.5), palette.secondary)
        } else {
            (theme::medium(13.5), theme::semibold(13.5), palette.text)
        };
        let value = if spec.percent && spec.of > 0 {
            format!(
                "{}  ·  {:.0}%",
                fmt_bytes_u64(spec.bytes),
                spec.bytes as f64 * 100.0 / spec.of as f64
            )
        } else {
            fmt_bytes_u64(spec.bytes)
        };
        let painter = ui.painter();
        painter.text(
            rect.left_top() + Vec2::new(spec.indent, 2.0),
            Align2::LEFT_TOP,
            spec.label,
            label_font,
            label_color,
        );
        painter.text(
            rect.right_top() + Vec2::new(0.0, 2.0),
            Align2::RIGHT_TOP,
            value,
            value_font,
            palette.text,
        );

        let bar = Rect::from_min_size(
            egui::pos2(rect.left() + spec.indent, rect.bottom() - 8.0),
            Vec2::new(rect.width() - spec.indent, 4.0),
        );
        painter.rect_filled(bar, 2.0, palette.surface_active);
        if spec.of > 0 && spec.bytes > 0 {
            let frac = (spec.bytes as f32 / spec.of as f32).clamp(0.0, 1.0);
            let fill = Rect::from_min_size(bar.min, Vec2::new((bar.width() * frac).max(2.0), bar.height()));
            painter.rect_filled(fill, 2.0, spec.color);
        }
    }

    if !spec.hint.is_empty() {
        let _ = response.on_hover_text(spec.hint);
    }
}
