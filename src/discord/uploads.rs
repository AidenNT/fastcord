//! Envío de mensajes con archivos adjuntos.
//!
//! Sigue el mismo camino que el cliente oficial (no un `multipart` directo a
//! `/messages`, que es lo que hacen los bots):
//!
//! 1. `POST /channels/{id}/attachments` con nombre y tamaño de cada archivo:
//!    Discord devuelve, por archivo, una `upload_url` (Google Cloud Storage) y
//!    un `upload_filename`.
//! 2. `PUT` de los bytes a esa `upload_url`.
//! 3. `POST /channels/{id}/messages` con `attachments: [{ id, filename,
//!    uploaded_filename }]`, el texto y la respuesta (si hay).
//!
//! La UI (`ui::attachments`) arma la lista de [`UploadFile`], guarda un
//! [`UploadJob`] para mostrar el avance y el error, y llama a
//! [`spawn_send_with_files`]. El mensaje definitivo llega por el Gateway
//! (`MESSAGE_CREATE`), así que acá no hay "eco local".

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::uwu_rest::UwuRest;

/// Máximo de adjuntos por mensaje (límite de Discord).
pub const MAX_FILES_PER_MESSAGE: usize = 10;

const MIB: u64 = 1024 * 1024;

/// Tamaño máximo por archivo que se le deja elegir a la cuenta.
///
/// Es una barrera de cortesía, no la regla: la que manda es la de Discord
/// (cambia con los años y con los boosts del server), así que ante la duda se
/// deja pasar y se traduce el error del servidor (`explain_error`).
///
/// - Sin Nitro: 10 MiB en un DM. En un server puede ser más (boosts nivel 2 y
///   3 = 50 / 100 MiB), y acá no se lee el nivel de boost, así que se deja
///   hasta 100 MiB y el servidor decide.
/// - Nitro Básico: 50 MiB. Nitro: 500 MiB (también Classic, por simplicidad).
/// - Se desconoce el nivel (`None`): 500 MiB.
pub fn max_upload_bytes(premium_type: Option<u8>, in_guild: bool) -> u64 {
    match premium_type {
        Some(0) if in_guild => 100 * MIB,
        Some(0) => 10 * MIB,
        Some(3) => 50 * MIB,
        _ => 500 * MIB,
    }
}

/// Un archivo listo para subir.
#[derive(Clone, Debug)]
pub struct UploadFile {
    pub path: PathBuf,
    /// Nombre que se publica (ya con el prefijo `SPOILER_` si corresponde).
    pub filename: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Uploading { done: usize, total: usize },
    Done,
    Failed(String),
}

/// Estado compartido entre el hilo que sube y la UI que lo dibuja.
#[derive(Debug)]
pub struct UploadJob {
    state: Mutex<JobState>,
}

impl UploadJob {
    pub fn new(total: usize) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(JobState::Uploading { done: 0, total }) })
    }

    pub fn snapshot(&self) -> JobState {
        self.state.lock().map(|s| s.clone()).unwrap_or(JobState::Done)
    }

    fn set(&self, next: JobState) {
        if let Ok(mut s) = self.state.lock() {
            *s = next;
        }
    }
}

/// Sube los archivos y manda el mensaje en un hilo aparte (igual que
/// `spawn_send_message`). Los errores quedan en `job` para que la UI los
/// muestre; `ctx` se usa para pedir un repaint cuando algo cambia.
#[allow(clippy::too_many_arguments)]
pub fn spawn_send_with_files(
    ctx: egui::Context,
    token: String,
    channel_id: String,
    guild_id: Option<String>,
    content: String,
    reply_to: Option<String>,
    files: Vec<UploadFile>,
    job: Arc<UploadJob>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            job.set(JobState::Failed("No se pudo iniciar la subida.".to_string()));
            ctx.request_repaint();
            return;
        };
        let total = files.len();
        let result = rt.block_on(async {
            let rest = UwuRest::for_token(token).await?;
            let progress_job = Arc::clone(&job);
            let progress_ctx = ctx.clone();
            rest.send_message_with_files(
                &channel_id,
                guild_id.as_deref(),
                &content,
                reply_to.as_deref(),
                &files,
                move |done| {
                    progress_job.set(JobState::Uploading { done, total });
                    progress_ctx.request_repaint();
                },
            )
            .await
        });
        match result {
            Ok(()) => job.set(JobState::Done),
            Err(e) => {
                log::warn!("No se pudo mandar el mensaje con archivos: {e}");
                job.set(JobState::Failed(explain_error(&e.to_string())));
            }
        }
        ctx.request_repaint();
    });
}

/// Traduce el error crudo de Discord/CDN a algo que se le pueda mostrar a
/// la persona.
pub fn explain_error(raw: &str) -> String {
    if raw.contains("50013") || raw.contains("code 403") {
        "No tienes permiso para adjuntar archivos en este canal.".to_string()
    } else if raw.contains("40005") || raw.contains("code 413") || raw.contains("Request entity too large") {
        "Algún archivo supera el tamaño máximo permitido aquí.".to_string()
    } else if raw.contains("20028") || raw.contains("code 429") {
        "Discord te pidió esperar un momento. Intenta de nuevo.".to_string()
    } else if raw.contains("No se pudo leer") {
        raw.to_string()
    } else {
        "No se pudo enviar el mensaje con los archivos.".to_string()
    }
}

/// Tipo de contenido por extensión (lo que Discord y el CDN esperan en el
/// `Content-Type` del PUT).
pub fn mime_for(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
    .to_string()
}
