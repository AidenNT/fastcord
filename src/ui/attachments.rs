//! Adjuntos del compositor: selector de archivos, arrastrar y soltar, pegar
//! una imagen del portapapeles, la fila de "chips" con lo pendiente y el
//! aviso de subida. El envío en sí vive en `discord::uploads`.
//!
//! Los archivos pendientes se guardan por compositor y por canal en la memoria
//! temporal de egui (solo la ruta, nunca los bytes: se leen al enviar).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Color32, CornerRadius, Frame, Margin, Stroke};

use crate::discord::uploads::{self, JobState, MAX_FILES_PER_MESSAGE, UploadFile, UploadJob};
use crate::ui::media::human_size;
use crate::ui::theme::{self, Icon, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Other,
}

#[derive(Clone, Debug)]
pub struct PendingFile {
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
    pub mime: String,
    pub kind: FileKind,
    pub spoiler: bool,
    /// Archivo temporal que creamos nosotros (captura pegada): se borra al
    /// quitarlo o al enviarlo.
    pub temp: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub files: Vec<PendingFile>,
}

/// Resultado del selector nativo, que corre en otro hilo.
#[derive(Clone, Default)]
struct PickerHandle(Arc<Mutex<PickerState>>);

#[derive(Default)]
enum PickerState {
    #[default]
    Idle,
    Open,
    Picked(Vec<PathBuf>),
}

pub fn load(ctx: &egui::Context, id: egui::Id) -> Pending {
    ctx.data(|d| d.get_temp::<Pending>(id)).unwrap_or_default()
}

pub fn store(ctx: &egui::Context, id: egui::Id, pending: Pending) {
    ctx.data_mut(|d| d.insert_temp(id, pending));
}

/// Abre el selector de archivos del sistema (sin congelar la UI).
pub fn open_picker(ctx: &egui::Context, picker_id: egui::Id) {
    let handle: PickerHandle = ctx.data(|d| d.get_temp(picker_id)).unwrap_or_default();
    {
        let Ok(mut state) = handle.0.lock() else { return };
        if matches!(*state, PickerState::Open) {
            return; // ya hay uno abierto
        }
        *state = PickerState::Open;
    }
    ctx.data_mut(|d| d.insert_temp(picker_id, handle.clone()));
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let picked = rfd::FileDialog::new().set_title("Adjuntar archivos").pick_files().unwrap_or_default();
        if let Ok(mut state) = handle.0.lock() {
            *state = PickerState::Picked(picked);
        }
        ctx.request_repaint();
    });
}

/// Los archivos que eligió el selector desde el último frame (si hay).
pub fn take_picked(ctx: &egui::Context, picker_id: egui::Id) -> Vec<PathBuf> {
    let Some(handle) = ctx.data(|d| d.get_temp::<PickerHandle>(picker_id)) else {
        return Vec::new();
    };
    let Ok(mut state) = handle.0.lock() else { return Vec::new() };
    match std::mem::take(&mut *state) {
        PickerState::Picked(paths) => paths,
        other => {
            *state = other;
            Vec::new()
        }
    }
}

/// Archivos soltados sobre la ventana en este frame.
pub fn dropped_paths(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect())
}

pub fn hovering_files(ctx: &egui::Context) -> bool {
    ctx.input(|i| !i.raw.hovered_files.is_empty())
}

fn kind_of(mime: &str) -> FileKind {
    if mime.starts_with("image/") {
        FileKind::Image
    } else if mime.starts_with("video/") {
        FileKind::Video
    } else if mime.starts_with("audio/") {
        FileKind::Audio
    } else {
        FileKind::Other
    }
}

/// Suma `paths` a lo pendiente validando cantidad, tamaño y que sean archivos.
/// Devuelve los avisos de lo que se descartó (para mostrar en rojo).
pub fn add_paths(pending: &mut Pending, paths: Vec<PathBuf>, max_bytes: u64) -> Vec<String> {
    let mut problems = Vec::new();
    for path in paths {
        if pending.files.len() >= MAX_FILES_PER_MESSAGE {
            problems.push(format!("Solo se pueden adjuntar {MAX_FILES_PER_MESSAGE} archivos por mensaje."));
            break;
        }
        if pending.files.iter().any(|f| f.path == path) {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("archivo").to_string();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                problems.push(format!("No se pudo leer «{name}»."));
                continue;
            }
        };
        if !meta.is_file() {
            problems.push(format!("«{name}» es una carpeta: solo se pueden adjuntar archivos."));
            continue;
        }
        if meta.len() == 0 {
            problems.push(format!("«{name}» está vacío."));
            continue;
        }
        if meta.len() > max_bytes {
            problems.push(format!(
                "«{name}» pesa {} y el máximo es {}.",
                human_size(meta.len()),
                human_size(max_bytes)
            ));
            continue;
        }
        let mime = uploads::mime_for(&path);
        pending.files.push(PendingFile {
            kind: kind_of(&mime),
            path,
            filename: name,
            size: meta.len(),
            mime,
            spoiler: false,
            temp: false,
        });
    }
    problems.dedup();
    problems
}

/// Si el portapapeles trae una imagen (captura de pantalla, "copiar imagen"),
/// la guarda como PNG temporal y la suma a lo pendiente. `false` = no había.
pub fn paste_image(pending: &mut Pending, max_bytes: u64) -> Result<bool, String> {
    let Ok(mut clipboard) = arboard::Clipboard::new() else { return Ok(false) };
    let Ok(img) = clipboard.get_image() else { return Ok(false) };
    let (w, h) = (img.width as u32, img.height as u32);
    let Some(rgba) = image::RgbaImage::from_raw(w, h, img.bytes.into_owned()) else {
        return Err("No se pudo leer la imagen del portapapeles.".to_string());
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let filename = format!("imagen-{stamp}.png");
    let path = std::env::temp_dir().join(format!("ecord-{filename}"));
    rgba.save(&path).map_err(|_| "No se pudo guardar la imagen del portapapeles.".to_string())?;
    let before = pending.files.len();
    let problems = add_paths(pending, vec![path.clone()], max_bytes);
    if let Some(file) = pending.files.get_mut(before) {
        file.filename = filename;
        file.temp = true;
        return Ok(true);
    }
    let _ = std::fs::remove_file(&path);
    Err(problems.into_iter().next().unwrap_or_else(|| "No se pudo adjuntar la imagen.".to_string()))
}

/// Quita el archivo `index` (y borra el temporal si lo creamos nosotros).
pub fn remove(pending: &mut Pending, index: usize) {
    if index < pending.files.len() {
        let file = pending.files.remove(index);
        if file.temp {
            let _ = std::fs::remove_file(&file.path);
        }
    }
}

/// Convierte lo pendiente en la lista que sube `discord::uploads` (aplica el
/// prefijo `SPOILER_`). Las capturas pegadas quedan en el directorio temporal
/// del sistema hasta que el hilo de subida las lea.
pub fn to_upload_files(pending: &Pending) -> Vec<UploadFile> {
    pending
        .files
        .iter()
        .map(|f| UploadFile {
            path: f.path.clone(),
            filename: if f.spoiler && !f.filename.starts_with("SPOILER_") {
                format!("SPOILER_{}", f.filename)
            } else {
                f.filename.clone()
            },
            size: f.size,
            mime: f.mime.clone(),
        })
        .collect()
}

/// Cuadro de "suelta para adjuntar" mientras se arrastran archivos encima.
pub fn drop_overlay(ctx: &egui::Context, palette: &Palette, allowed: bool) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("ecord_drop_overlay")));
    painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(140));
    let text = if allowed {
        "Suelta los archivos para adjuntarlos"
    } else {
        "No tienes permiso para adjuntar archivos aquí"
    };
    let color = if allowed { palette.text } else { palette.danger };
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        text,
        theme::semibold(18.0),
        color,
    );
}

/// Fila de chips con los archivos pendientes. Devuelve el alto que ocupó.
pub fn show_chips(ui: &mut egui::Ui, palette: &Palette, pending: &mut Pending) -> f32 {
    if pending.files.is_empty() {
        return 0.0;
    }
    let top = ui.cursor().top();
    let mut remove_index: Option<usize> = None;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(theme::radius() + 6))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 16.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    for (i, file) in pending.files.iter_mut().enumerate() {
                        chip(ui, palette, file, i, &mut remove_index);
                    }
                });
            });
    });
    if let Some(i) = remove_index {
        remove(pending, i);
    }
    ui.cursor().top() - top
}

fn chip(ui: &mut egui::Ui, palette: &Palette, file: &mut PendingFile, index: usize, remove_index: &mut Option<usize>) {
    let icon = match file.kind {
        FileKind::Video => Icon::Video,
        FileKind::Audio => Icon::Music,
        FileKind::Image => Icon::LayoutGrid,
        FileKind::Other => Icon::Download,
    };
    Frame::new()
        .fill(palette.surface_active)
        .corner_radius(CornerRadius::same(theme::radius_small()))
        .inner_margin(Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, icon, 14.0, palette.dim);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    theme::text(ui, short_name(&file.filename, 24), theme::semibold(12.0), palette.text);
                    theme::text(ui, human_size(file.size), theme::regular(10.5), palette.dim);
                });
                let tone = if file.spoiler { palette.accent } else { palette.dim };
                let resp = ui
                    .add(egui::Label::new(egui::RichText::new("Spoiler").font(theme::medium(10.5)).color(tone)).sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Marcar como spoiler");
                if resp.clicked() {
                    file.spoiler = !file.spoiler;
                }
                if theme::icon_button(ui, Icon::X, 10.0, palette.dim, palette.text, "Quitar archivo").clicked() {
                    *remove_index = Some(index);
                }
            });
        });
}

fn short_name(name: &str, max_chars: usize) -> String {
    let count = name.chars().count();
    if count <= max_chars {
        return name.to_string();
    }
    // Se conserva la extensión: "reporte-largo….pdf".
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    let keep = max_chars.saturating_sub(ext.chars().count() + 2);
    let head: String = name.chars().take(keep).collect();
    if ext.is_empty() { format!("{head}…") } else { format!("{head}….{ext}") }
}

/// Registra una subida en curso para este compositor.
pub fn push_job(ctx: &egui::Context, jobs_id: egui::Id, job: Arc<UploadJob>) {
    let mut jobs: Vec<Arc<UploadJob>> = ctx.data(|d| d.get_temp(jobs_id)).unwrap_or_default();
    jobs.push(job);
    ctx.data_mut(|d| d.insert_temp(jobs_id, jobs));
}

/// Revisa las subidas: saca las terminadas y devuelve los errores nuevos.
pub fn poll_jobs(ctx: &egui::Context, jobs_id: egui::Id) -> Vec<String> {
    let Some(jobs) = ctx.data(|d| d.get_temp::<Vec<Arc<UploadJob>>>(jobs_id)) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    let mut alive = Vec::new();
    for job in jobs {
        match job.snapshot() {
            JobState::Uploading { .. } => alive.push(job),
            JobState::Done => {}
            JobState::Failed(msg) => errors.push(msg),
        }
    }
    if !alive.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    ctx.data_mut(|d| d.insert_temp(jobs_id, alive));
    errors
}

/// "Subiendo 2 de 3 archivos…" mientras haya subidas en curso.
pub fn status_line(ctx: &egui::Context, jobs_id: egui::Id) -> Option<String> {
    let jobs = ctx.data(|d| d.get_temp::<Vec<Arc<UploadJob>>>(jobs_id))?;
    let (mut done_total, mut total) = (0usize, 0usize);
    for job in &jobs {
        if let JobState::Uploading { done, total: t } = job.snapshot() {
            done_total += done;
            total += t;
        }
    }
    (total > 0).then(|| format!("Subiendo archivos… {done_total} de {total}"))
}
