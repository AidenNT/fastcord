//! Pestaña "Cliente": qué es esta instancia de eCord y qué declara hacia
//! afuera. Resumen del proceso y del sistema, rendimiento (tiempo de cuadro),
//! memoria, identidad ante Discord (IDs, `X-Super-Properties`, `launch_signature`),
//! transmisión (codificador, perfil de NVENC, paquete opcional con libx264),
//! actualizaciones (estado y reinstalación forzada) y
//! entorno (variables, argumentos, rutas).
//!
//! También arma el "informe de diagnóstico" de texto que se muestra en el
//! inspector y se copia con un clic. Ese informe NO incluye el token ni los
//! identificadores de instalación o fingerprint, para poder pegarlo en un reporte.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use egui::{CornerRadius, Sense, Stroke, pos2, vec2};

use super::{
    SessionInfo, STORAGE_FLAG, badge, card, copy_button, kv_row, opcodes, parse_super_properties, session_rows,
    signature_section, super_properties_section, style, theme_error, theme_ok, theme_warn,
};
use crate::discord::{gateway, gateway_tracking, launch_signature, rest};
use crate::logging::capture;
use crate::support::mem_report;
use crate::theme::{self, Palette};

/// Cuántos tiempos de cuadro se recuerdan para el gráfico.
const FRAME_SAMPLES: usize = 240;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Summary,
    Performance,
    Memory,
    Tracking,
    Opcodes,
    Identifiers,
    SuperProperties,
    Signature,
    Streaming,
    Updates,
    Environment,
}

impl Section {
    const ALL: [Section; 11] = [
        Section::Summary,
        Section::Performance,
        Section::Memory,
        Section::Tracking,
        Section::Opcodes,
        Section::Identifiers,
        Section::SuperProperties,
        Section::Signature,
        Section::Streaming,
        Section::Updates,
        Section::Environment,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Summary => "Resumen",
            Section::Performance => "Rendimiento",
            Section::Memory => "Memoria",
            Section::Tracking => "Seguimiento",
            Section::Opcodes => "Opcodes",
            Section::Identifiers => "Identificadores",
            Section::SuperProperties => "Super-Properties",
            Section::Signature => "launch_signature",
            Section::Streaming => "Transmisión",
            Section::Updates => "Actualizaciones",
            Section::Environment => "Entorno",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Section::Summary => "Esta instancia de eCord, el sistema donde corre y la sesión de Discord.",
            Section::Performance => "Tiempo de cada cuadro de la interfaz mientras esta pestaña está abierta.",
            Section::Memory => "RAM del proceso, texturas de la GPU y cachés en disco.",
            Section::Tracking => "Cuenta regresiva del opcode 41, latidos del Gateway y estado de la conexión.",
            Section::Opcodes => "Todos los opcodes del Gateway y de voz, cuáles usa eCord y plantillas para probarlos.",
            Section::Identifiers => "Lo que este cliente declara de sí mismo: installation ID, fingerprint y build.",
            Section::SuperProperties => "El header X-Super-Properties tal como sale, en el orden en que se envía.",
            Section::Signature => "Mods de cliente detectados por bitfield y coherencia con has_client_mods.",
            Section::Streaming => "Codificador de video, perfil de NVENC y el paquete opcional con libx264 (GPL).",
            Section::Updates => "Estado del actualizador y reinstalación forzada de la última versión.",
            Section::Environment => "Variables de entorno, argumentos de línea de comandos y rutas de trabajo.",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Section::Summary => style::BLUE,
            Section::Performance => style::GREEN,
            Section::Memory => style::ORANGE,
            Section::Tracking => style::YELLOW,
            Section::Opcodes => style::TEAL,
            Section::Identifiers => style::PURPLE,
            Section::SuperProperties => style::PINK,
            Section::Signature => style::RED,
            Section::Streaming => style::BLUE,
            Section::Updates => style::GREEN,
            Section::Environment => style::TEAL,
        }
    }
}

pub(super) struct ClientState {
    pub section: Section,
    frame_ms: VecDeque<f32>,
    sig_input: String,
    /// Búsqueda de la sección Opcodes (número o parte del nombre).
    op_filter: String,
    /// Opcodes del Gateway de voz en vez de los del Gateway principal.
    op_voice: bool,
    /// Solo los opcodes que eCord todavía no implementa.
    op_only_missing: bool,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            section: Section::Summary,
            frame_ms: VecDeque::new(),
            sig_input: String::new(),
            op_filter: String::new(),
            op_voice: false,
            op_only_missing: false,
        }
    }
}

pub(super) fn fmt_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if hours > 0 {
        format!("{hours} h {minutes:02} min {seconds:02} s")
    } else if minutes > 0 {
        format!("{minutes} min {seconds:02} s")
    } else {
        format!("{seconds} s")
    }
}

/// "hace" entre dos instantes, p. ej. `2 min 05 s`.
fn fmt_ago(now: Instant, at: Instant) -> String {
    fmt_duration(now.saturating_duration_since(at))
}

fn yes_no(value: bool) -> &'static str {
    if value { "sí" } else { "no" }
}

/// Fila de dos columnas (nombre · valor) para las tablas de seguimiento.
fn track_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.add(egui::Label::new(egui::RichText::new(label).weak()).extend());
    ui.add(egui::Label::new(egui::RichText::new(value).monospace()).wrap().selectable(true));
    ui.end_row();
}

/// Una línea con el estado del Gateway y la cuenta regresiva del opcode 41.
fn tracking_summary() -> String {
    let snap = gateway_tracking::snapshot();
    if !snap.connected {
        return "desconectado".to_owned();
    }
    let now = Instant::now();
    let op41 = match snap.time_spent_next_at {
        Some(next) => format!("opcode 41 en {}", fmt_duration(next.saturating_duration_since(now))),
        None => "opcode 41 sin programar".to_owned(),
    };
    format!(
        "{} · {op41} · {} enviados, {} omitidos",
        if snap.established { "sesión establecida" } else { "conectando" },
        snap.time_spent_sent,
        snap.time_spent_skipped,
    )
}

fn env_value(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        _ => "(sin definir)".to_owned(),
    }
}

/// Informe de diagnóstico en texto (sin token ni identificadores de instalación).
pub(super) fn report(ctx: &egui::Context, session: &SessionInfo, connection: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "eCord {} ({}) · {} {}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "debug" } else { "release" },
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    let _ = writeln!(out, "PID {} · activo {}", std::process::id(), fmt_duration(capture::started_at().elapsed()));
    let _ = writeln!(
        out,
        "Sesión: {} · usuario {} · {} servidores · {} DMs · {} amistades",
        if session.status.is_empty() { "—" } else { session.status.as_str() },
        session.username.as_deref().unwrap_or("—"),
        session.servers,
        session.dms,
        session.friends,
    );
    if let Some(identity) = rest::current_identity() {
        let _ = writeln!(out, "Build number de Discord: {}", identity.client_build_number);
        let _ = writeln!(out, "User-Agent: {}", identity.user_agent);
    }
    let _ = writeln!(
        out,
        "Pantalla: {:.0}×{:.0} pt · {:.2} px/pt",
        ctx.viewport_rect().width(),
        ctx.viewport_rect().height(),
        ctx.pixels_per_point(),
    );
    let _ = writeln!(
        out,
        "Memoria: heap {} · RAM {} · texturas {}",
        mem_report::fmt_bytes(mem_report::heap_bytes()),
        mem_report::process_memory().map_or("—".to_owned(), |memory| mem_report::fmt_bytes(memory.resident)),
        mem_report::fmt_bytes(mem_report::gpu_texture_bytes(ctx)),
    );
    let _ = writeln!(out, "Conexión emulada: {connection}");
    let _ = writeln!(out, "Gateway: {}", tracking_summary());
    let counts = capture::counts();
    let _ = writeln!(
        out,
        "Logs: {} errores · {} advertencias · {} info · {} debug",
        counts[0], counts[1], counts[2], counts[3]
    );
    out
}

impl ClientState {
    /// Guarda el tiempo del cuadro actual (se ignoran los huecos de reposo).
    pub(super) fn sample_frame(&mut self, dt_seconds: f32) {
        let ms = dt_seconds * 1000.0;
        if ms.is_finite() && ms > 0.0 && ms < 250.0 {
            self.frame_ms.push_back(ms);
            while self.frame_ms.len() > FRAME_SAMPLES {
                self.frame_ms.pop_front();
            }
        }
    }

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        style::panel_title(ui, palette, "Cliente");
        egui::ScrollArea::vertical().id_salt("dev_client_side").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for section in Section::ALL {
                if style::side_item(ui, palette, section.title(), section.color(), self.section == section, None)
                    .clicked()
                {
                    self.section = section;
                }
            }
        });
    }

    /// Dibuja la sección elegida. Devuelve un aviso para el "toast", si hace falta.
    pub(super) fn page(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &Palette,
        session: &SessionInfo,
        remember: &mut bool,
    ) -> Option<&'static str> {
        let mut toast = None;
        theme::section_title(ui, palette, self.section.title());
        theme::text(ui, self.section.subtitle(), theme::regular(12.0), palette.dim);
        ui.add_space(6.0);

        egui::ScrollArea::vertical().id_salt("dev_client_page").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            match self.section {
                Section::Summary => self.summary(ui, ctx, session),
                Section::Performance => self.performance(ui, ctx, palette),
                Section::Memory => toast = self.memory(ui, ctx),
                Section::Tracking => self.tracking(ui, ctx, palette),
                Section::Opcodes => self.opcodes(ui),
                Section::Identifiers | Section::SuperProperties | Section::Signature => self.identity(ui),
                Section::Streaming => toast = self.streaming(ui, ctx),
                Section::Updates => toast = self.updates(ui, ctx),
                Section::Environment => self.environment(ui, remember),
            }
            ui.add_space(12.0);
        });
        toast
    }

    // -- Resumen -------------------------------------------------------------------------

    fn summary(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, session: &SessionInfo) {
        card(ui, "Aplicación", |ui| {
            egui::Grid::new("dev_client_app").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "Nombre", Some("eCord"));
                kv_row(ui, "Versión", Some(env!("CARGO_PKG_VERSION")));
                kv_row(ui, "Compilación", Some(if cfg!(debug_assertions) { "debug" } else { "release" }));
                kv_row(ui, "Proceso (PID)", Some(&std::process::id().to_string()));
                kv_row(ui, "Tiempo activo", Some(&fmt_duration(capture::started_at().elapsed())));
                let exe = std::env::current_exe().ok().map(|path| path.display().to_string());
                kv_row(ui, "Ejecutable", exe.as_deref());
            });
        });

        card(ui, "Sistema", |ui| {
            egui::Grid::new("dev_client_system").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "Sistema operativo", Some(std::env::consts::OS));
                kv_row(ui, "Arquitectura", Some(std::env::consts::ARCH));
                let threads = std::thread::available_parallelism().map(|count| count.get().to_string()).ok();
                kv_row(ui, "Hilos de CPU", threads.as_deref());
            });
        });

        card(ui, "Ventana", |ui| {
            egui::Grid::new("dev_client_window").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                let size = ctx.viewport_rect().size();
                kv_row(ui, "Tamaño (puntos)", Some(&format!("{:.0} × {:.0}", size.x, size.y)));
                kv_row(ui, "Escala (px por punto)", Some(&format!("{:.2}", ctx.pixels_per_point())));
                kv_row(ui, "Renderizador", Some("OpenGL (glow)"));
            });
        });

        card(ui, "Sesión de Discord", |ui| {
            egui::Grid::new("dev_client_session").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                let status = if session.status.is_empty() { None } else { Some(session.status.as_str()) };
                kv_row(ui, "Estado", status);
                kv_row(ui, "Usuario", session.username.as_deref());
                kv_row(ui, "ID de usuario", session.user_id.as_deref());
                kv_row(ui, "Servidores", Some(&session.servers.to_string()));
                kv_row(ui, "Mensajes directos", Some(&session.dms.to_string()));
                kv_row(ui, "Amistades", Some(&session.friends.to_string()));
                let token = session.token.as_ref().map(|token| format!("presente ({} caracteres)", token.chars().count()));
                kv_row(ui, "Token", token.as_deref().or(Some("ausente")));
            });
            ui.weak("El token nunca se muestra ni se incluye en el informe.");
        });
    }

    // -- Rendimiento ------------------------------------------------------------------------

    fn performance(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        // Para que haya cuadros que medir, se repinta ~30 veces por segundo
        // mientras se mira este gráfico.
        ctx.request_repaint_after(Duration::from_millis(33));

        card(ui, "Tiempo de cuadro", |ui| {
            if self.frame_ms.is_empty() {
                ui.weak("Todavía no hay muestras.");
                return;
            }
            let count = self.frame_ms.len() as f32;
            let average = self.frame_ms.iter().sum::<f32>() / count;
            let slowest = self.frame_ms.iter().copied().fold(0.0_f32, f32::max);
            let fastest = self.frame_ms.iter().copied().fold(f32::MAX, f32::min);
            ui.horizontal_wrapped(|ui| {
                super::badge(ui, &format!("{:.0} FPS", 1000.0 / average.max(0.001)), style::GREEN);
                ui.label(
                    egui::RichText::new(format!("media {average:.1} ms · mejor {fastest:.1} ms · peor {slowest:.1} ms"))
                        .color(palette.secondary),
                );
            });
            ui.add_space(6.0);

            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 130.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(8), theme::inset(palette));
            let ceiling = slowest.max(33.4);
            let to_y = |ms: f32| rect.bottom() - 8.0 - (ms / ceiling) * (rect.height() - 16.0);
            for (reference, label) in [(16.7_f32, "16.7 ms · 60 FPS"), (33.3, "33.3 ms · 30 FPS")] {
                let y = to_y(reference);
                painter.hline(rect.left()..=rect.right(), y, Stroke::new(1.0, palette.outline));
                painter.text(
                    pos2(rect.right() - 8.0, y - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    label,
                    theme::regular(10.5),
                    palette.dim,
                );
            }
            let step = (rect.width() - 16.0) / (FRAME_SAMPLES as f32 - 1.0);
            let points: Vec<egui::Pos2> = self
                .frame_ms
                .iter()
                .enumerate()
                .map(|(index, ms)| pos2(rect.left() + 8.0 + step * index as f32, to_y(*ms)))
                .collect();
            if points.len() >= 2 {
                painter.add(egui::Shape::line(points, Stroke::new(1.6, palette.accent)));
            }
        });
    }

    // -- Memoria --------------------------------------------------------------------------------

    fn memory(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<&'static str> {
        let mut toast = None;
        card(ui, "Memoria del proceso", |ui| {
            egui::Grid::new("dev_client_memory").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "Heap de Rust (vivo)", Some(&mem_report::fmt_bytes(mem_report::heap_bytes())));
                let resident = mem_report::process_memory().map(|memory| mem_report::fmt_bytes(memory.resident));
                kv_row(ui, "RAM del proceso", resident.as_deref());
                kv_row(ui, "Texturas en la GPU", Some(&mem_report::fmt_bytes(mem_report::gpu_texture_bytes(ctx))));
            });
            ui.add_space(4.0);
            if ui.button("Devolver memoria libre al sistema").clicked() {
                mem_report::release_free_memory();
                toast = Some("Memoria liberada");
            }
        });

        card(ui, "Caché en disco", |ui| {
            let disk = mem_report::disk_stats();
            if !disk.scanned {
                ui.weak("Escaneando…");
                return;
            }
            egui::Grid::new("dev_client_disk").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(
                    ui,
                    "Imágenes",
                    Some(&format!("{} ({} archivos)", mem_report::fmt_bytes_u64(disk.images), disk.image_files)),
                );
                kv_row(ui, "JSON", Some(&mem_report::fmt_bytes_u64(disk.json)));
                kv_row(ui, "Sonidos", Some(&mem_report::fmt_bytes_u64(disk.sounds)));
                kv_row(ui, "Videos", Some(&mem_report::fmt_bytes_u64(disk.video)));
                kv_row(ui, "Total", Some(&mem_report::fmt_bytes_u64(disk.total())));
            });
        });
        toast
    }

    // -- Transmisión --------------------------------------------------------------------------------

    /// Codificador de la transmisión de pantalla y paquete opcional de libx264.
    /// Lo que se cambia acá vale hasta cerrar eCord. Con una transmisión en curso reinicia
    /// el codificador en el acto (la imagen se corta ~1 s); sin ella, vale para la próxima.
    fn streaming(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<&'static str> {
        use crate::discord::voice::{
            ENCODER_CHOICES, KEYFRAME_INTERVAL_CHOICES_MS, NVENC_PROFILES, PlayoutDelay, encoder_override, keyframe_interval_ms,
            keyframe_interval_override, last_encoder, nvenc_match_x264, nvenc_multi_slice, nvenc_profile, nvenc_profile_override, playout_delay,
            playout_delay_override, set_encoder_override, set_keyframe_interval_override, set_nvenc_match_x264, set_nvenc_multi_slice,
            set_nvenc_profile_override, set_playout_delay_override, set_strip_sei, set_strip_vui, strip_sei, strip_vui, x264_available,
            StreamTransport, set_stream_transport_override, stream_transport, stream_transport_override,
        };
        use crate::gpl_pack::{self, Pref};
        #[cfg(windows)]
        use crate::gpl_pack::Status;

        // Solo se usa en Windows (reinstalar el paquete GPL).
        let _ = ctx;
        let mut toast = None;

        card(ui, "Codificador", |ui| {
            egui::Grid::new("dev_client_stream_encoder").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "libx264 cargado", Some(if x264_available() { "sí" } else { "no" }));
                kv_row(ui, "Último codificador usado", Some(last_encoder().unwrap_or("ninguno todavía")));
                kv_row(ui, "Perfil de NVENC en uso", Some(&nvenc_profile()));
            });
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Forzar codificador");
                let current = encoder_override();
                egui::ComboBox::from_id_salt("dev_stream_encoder")
                    .selected_text(current.unwrap_or("predeterminado (entorno)"))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "predeterminado (WebRTC)").clicked() {
                            set_encoder_override(None);
                        }
                        for choice in ENCODER_CHOICES {
                            if ui.selectable_label(current == Some(choice), choice).clicked() {
                                set_encoder_override(Some(choice));
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Perfil de NVENC");
                let current = nvenc_profile_override();
                egui::ComboBox::from_id_salt("dev_stream_nvenc_profile")
                    .selected_text(current.unwrap_or("predeterminado (entorno)"))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "predeterminado (entorno)").clicked() {
                            set_nvenc_profile_override(None);
                        }
                        for profile in NVENC_PROFILES {
                            if ui.selectable_label(current == Some(profile), profile).clicked() {
                                set_nvenc_profile_override(Some(profile));
                            }
                        }
                    });
            });
            let mut matched = nvenc_match_x264();
            if ui
                .checkbox(&mut matched, "NVENC: pedir el mismo nivel H.264 y 1 referencia que x264")
                .on_hover_text(
                    "x264 declara el nivel calculado (mínimo 4.1) y usa 1 referencia; NVENC por su cuenta declara 3.1 \
                     a 720p30 y usa 3. También se apaga con ECORD_NVENC_AUTO_LEVEL=1.",
                )
                .changed()
            {
                set_nvenc_match_x264(matched);
            }
            let mut slices = nvenc_multi_slice();
            if ui
                .checkbox(&mut slices, "NVENC: partir cada frame en 6 slices (como x264)")
                .on_hover_text(
                    "x264 manda cada frame en 6 slices y el espectador lo decodifica; NVENC manda 1 solo. \
                     También se apaga con ECORD_NVENC_SINGLE_SLICE=1.",
                )
                .changed()
            {
                set_nvenc_multi_slice(slices);
            }
            let mut strip = strip_sei();
            if ui
                .checkbox(&mut strip, "Quitar los SEI de NVENC antes de enviar")
                .on_hover_text(
                    "NVENC mete SEI de temporización en cada frame. No hacen falta para decodificar y algunos \
                     espectadores se atragantan. También se apaga con ECORD_KEEP_SEI=1.",
                )
                .changed()
            {
                set_strip_sei(strip);
            }
            let mut strip = strip_vui();
            if ui
                .checkbox(&mut strip, "Reescribir el VUI del SPS de NVENC (como x264)")
                .on_hover_text(
                    "Reemplaza el VUI del SPS (NVENC en CBR mete timing_info y HRD y no declara \
                     bitstream_restriction) por uno mínimo con bitstream_restriction: sin reordenamiento y \
                     buffer igual a las referencias, como declara x264. Apagado, el SPS sale tal cual lo \
                     escribe NVENC. También se apaga con ECORD_KEEP_VUI=1.",
                )
                .changed()
            {
                set_strip_vui(strip);
            }
            ui.add_space(4.0);
            ui.weak(
                "Vale hasta cerrar eCord y gana sobre las variables de entorno. Con una transmisión en curso, cada cambio \
                 reinicia el codificador en vivo (la imagen se corta ~1 s); sin ella, vale para la próxima.",
            );
        });

        card(ui, "Entrega a los espectadores", |ui| {
            // Presets del playout delay (mínimo-máximo en ms que se le pide al visor).
            const PLAYOUT_PRESETS: [PlayoutDelay; 8] = [
                PlayoutDelay::Off,
                PlayoutDelay::Legacy,
                PlayoutDelay::Range { min_ms: 0, max_ms: 100 },
                PlayoutDelay::Range { min_ms: 0, max_ms: 200 },
                PlayoutDelay::Range { min_ms: 50, max_ms: 150 },
                PlayoutDelay::Range { min_ms: 60, max_ms: 150 },
                PlayoutDelay::Range { min_ms: 100, max_ms: 200 },
                PlayoutDelay::Range { min_ms: 100, max_ms: 300 },
            ];
            egui::Grid::new("dev_client_stream_delivery").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "Transporte (próxima transmisión)", Some(stream_transport().label()));
                kv_row(ui, "Playout delay en uso", Some(&playout_delay().label()));
                kv_row(ui, "Intervalo entre IDR en uso", Some(&format!("{} ms", keyframe_interval_ms())));
            });
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Transporte");
                let current = stream_transport_override();
                egui::ComboBox::from_id_salt("dev_stream_transport")
                    .selected_text(current.map_or_else(|| "predeterminado (WebRTC)".to_owned(), |transport| transport.label().to_owned()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "predeterminado (entorno)").clicked() {
                            set_stream_transport_override(None);
                        }
                        for transport in StreamTransport::CHOICES {
                            if ui.selectable_label(current == Some(transport), transport.label()).clicked() {
                                set_stream_transport_override(Some(transport));
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "WebRTC (DTLS-SRTP) es el predeterminado: negocia un SDP con Discord como la librería \
                         de Node Discord-video-stream: Sender Reports, reenvíos por NACK/RTX y timestamps regulares. Solo \
                         se aplica al iniciar una transmisión nueva (una en curso sigue con el que arrancó). También se \
                         elige con ECORD_STREAM_TRANSPORT=udp (camino UDP clásico).",
                    );
            });
            ui.horizontal(|ui| {
                ui.label("Playout delay (min-max)");
                let current = playout_delay_override();
                egui::ComboBox::from_id_salt("dev_stream_playout_delay")
                    .selected_text(current.map_or_else(|| "predeterminado (entorno)".to_owned(), PlayoutDelay::label))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "predeterminado (entorno)").clicked() {
                            set_playout_delay_override(None);
                        }
                        for preset in PLAYOUT_PRESETS {
                            if ui.selectable_label(current == Some(preset), preset.label()).clicked() {
                                set_playout_delay_override(Some(preset));
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "Lo que se le pide al visor con la extensión RTP de playout delay. Con un mínimo mayor que 0 el visor \
                         espera ese tiempo antes de mostrar cada frame y le da margen a un paquete reenviado, a costa de ese \
                         retraso. Por defecto 0-100 ms, como la librería de Node. También se elige con \
                         ECORD_PLAYOUT_DELAY (off, legacy o MIN-MAX en ms). Se aplica desde el próximo paquete, sin reiniciar \
                         la transmisión.",
                    );
            });
            ui.horizontal(|ui| {
                ui.label("Intervalo entre IDR");
                let current = keyframe_interval_override();
                egui::ComboBox::from_id_salt("dev_stream_keyframe_interval")
                    .selected_text(current.map_or_else(|| "predeterminado (entorno)".to_owned(), |ms| format!("{ms} ms")))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "predeterminado (entorno)").clicked() {
                            set_keyframe_interval_override(None);
                        }
                        for ms in KEYFRAME_INTERVAL_CHOICES_MS {
                            if ui.selectable_label(current == Some(ms), format!("{ms} ms")).clicked() {
                                set_keyframe_interval_override(Some(ms));
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "Cada cuánto se fuerza una IDR. Si el visor no recupera un paquete perdido, la imagen queda congelada \
                         hasta la próxima IDR: un intervalo menor acorta el congelamiento, a costa de ~28 KB extra por IDR. \
                         También se elige con ECORD_KEYFRAME_INTERVAL_MS (250 a 4000). Con una transmisión en curso reinicia \
                         el codificador en el acto.",
                    );
            });
            ui.add_space(4.0);
            ui.weak("Gana sobre las variables de entorno. El playout delay cambia sin cortar la imagen; el intervalo de IDR reinicia el codificador (~1 s).");
        });

        card(ui, "Paquete GPL (libx264)", |ui| {
            let pref = gpl_pack::pref();
            egui::Grid::new("dev_client_stream_gpl").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(
                    ui,
                    "Respuesta guardada",
                    Some(match pref {
                        Pref::Ask => "sin responder",
                        Pref::Accepted => "aceptado",
                        Pref::Declined => "rechazado",
                    }),
                );
                kv_row(ui, "Estado del paquete", Some(&format!("{:?}", gpl_pack::status())));
                kv_row(
                    ui,
                    "Se preguntaría al transmitir",
                    Some(if gpl_pack::should_ask() { "sí" } else { "no" }),
                );
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(pref != Pref::Ask, egui::Button::new("Volver a preguntar"))
                    .on_hover_text("Olvida la respuesta guardada: la pregunta de x264 sale en la próxima transmisión.")
                    .clicked()
                {
                    gpl_pack::set_pref(Pref::Ask);
                    toast = Some("Se volverá a preguntar por x264");
                }
                // Reinstalar encima de un paquete ya instalado pisaría las copias
                // *.lgpl-bak con DLL GPL; por eso primero se vuelve a LGPL.
                #[cfg(windows)]
                if ui
                    .add_enabled(
                        pref == Pref::Accepted && matches!(gpl_pack::status(), Status::Idle | Status::Failed(_)),
                        egui::Button::new("Reinstalar paquete GPL"),
                    )
                    .on_hover_text(
                        "Vuelve a las DLL LGPL y baja e instala de nuevo el paquete de esta versión. \
                         Hay que reiniciar eCord. Solo si ya aceptaste la licencia.",
                    )
                    .clicked()
                {
                    toast = Some(match gpl_pack::revert() {
                        Ok(_) => {
                            let repaint = ctx.clone();
                            gpl_pack::start_download(move || repaint.request_repaint());
                            "Reinstalando el paquete GPL"
                        }
                        Err(_) => "No se pudo preparar la reinstalación",
                    });
                }
                #[cfg(windows)]
                if ui
                    .button("Quitar x264 (volver a LGPL)")
                    .on_hover_text("Restaura las DLL *.lgpl-bak. Hay que reiniciar eCord.")
                    .clicked()
                {
                    toast = Some(match gpl_pack::revert() {
                        Ok(_) => "DLL LGPL restauradas: reiniciá eCord",
                        Err(_) => "No se pudo volver a las DLL LGPL",
                    });
                }
            });
            if !x264_available() && pref == Pref::Ask {
                ui.colored_label(theme_warn(), "x264 no está cargado y no hay respuesta: la pregunta aparece al transmitir.");
            }
        });
        toast
    }

    // -- Actualizaciones ----------------------------------------------------------------------------

    /// Estado del actualizador y reinstalación forzada. La reinstalación baja la
    /// última versión publicada AUNQUE sea la que ya está instalada, con la misma
    /// descarga verificada (SHA-256 y firma) y el mismo ayudante que una
    /// actualización: reemplaza el `.exe` y las DLL, y con el `-setup.exe` lo
    /// ejecuta en silencio.
    fn updates(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<&'static str> {
        use crate::updater::{self, View};

        let mut toast = None;
        let view = updater::view();
        let state = match &view {
            View::Idle => "sin buscar todavía (o actualizaciones apagadas)".to_owned(),
            View::Checking => "buscando…".to_owned(),
            View::UpToDate => "al día".to_owned(),
            View::CheckFailed(error) => format!("no se pudo consultar: {error}"),
            View::NeedsElevation { version } => format!("{version} lista, pero hace falta administrador"),
            View::Manual { version, reason, .. } => format!("{version} disponible, esta copia no se actualiza sola: {reason}"),
            View::Downloading { version, received, total } => format!("bajando {version}: {received}/{total} bytes"),
            View::Ready { version } => format!("{version} descargada y verificada: lista para instalar"),
            View::Installing => "instalando: eCord se está cerrando".to_owned(),
            View::Failed(error) => format!("falló: {error}"),
        };

        card(ui, "Estado", |ui| {
            egui::Grid::new("dev_client_updates").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                kv_row(ui, "Versión instalada", Some(updater::UPDATES.current_version));
                kv_row(ui, "Estado", Some(&state));
                kv_row(
                    ui,
                    "Carpeta protegida (pide administrador)",
                    Some(if updater::needs_elevation() { "sí" } else { "no" }),
                );
                kv_row(ui, updater::DISABLE_ENV, Some(&env_value(updater::DISABLE_ENV)));
            });
        });

        card(ui, "Acciones", |ui| {
            let idle = matches!(view, View::Idle | View::UpToDate | View::CheckFailed(_) | View::Failed(_));
            ui.horizontal(|ui| {
                if ui.add_enabled(idle, egui::Button::new("Buscar actualizaciones")).clicked() {
                    updater::check_now();
                    toast = Some("Buscando actualizaciones");
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Forzar reinstalación"))
                    .on_hover_text("Baja la última versión publicada aunque sea la misma que tenés.")
                    .clicked()
                {
                    updater::force_reinstall();
                    toast = Some("Bajando la última versión para reinstalar");
                }
            });
            match &view {
                View::Ready { .. } => {
                    if ui.button("Instalar y reiniciar ahora").clicked() {
                        let repaint = ctx.clone();
                        let quit = ctx.clone();
                        updater::install_and_quit(
                            move || repaint.request_repaint(),
                            move || crate::ui::settings::updates::quit_now(&quit),
                        );
                    }
                }
                View::NeedsElevation { .. } => {
                    if ui.button("Reinstalar como administrador").clicked() {
                        let repaint = ctx.clone();
                        let quit = ctx.clone();
                        updater::request_elevated_reinstall(
                            move || repaint.request_repaint(),
                            move || crate::ui::settings::updates::quit_now(&quit),
                        );
                    }
                }
                _ => {}
            }
            ui.add_space(4.0);
            ui.weak(
                "Forzar reinstalación baja la última versión publicada, la verifica (SHA-256 y firma) y deja \
                 \"Instalar y reiniciar ahora\" listo. Reemplaza el .exe y las DLL; con el instalador \
                 (-setup.exe) lo ejecuta en silencio.",
            );
        });
        toast
    }

    // -- Seguimiento ------------------------------------------------------------------------------

    fn tracking(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        // La cuenta regresiva se mueve sola: repintar ~4 veces por segundo.
        ctx.request_repaint_after(Duration::from_millis(250));
        let snap = gateway_tracking::snapshot();
        let now = Instant::now();
        let interval = gateway::time_spent_interval();

        // -- Opcode 41 --
        card(ui, "Opcode 41 · Update Time Spent Session ID", |ui| {
            match (snap.connected, snap.time_spent_next_at) {
                (true, Some(next)) => {
                    let remaining = next.saturating_duration_since(now);
                    let done = 1.0 - (remaining.as_secs_f32() / interval.as_secs_f32()).clamp(0.0, 1.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(fmt_duration(remaining)).monospace().size(22.0).color(palette.accent),
                        );
                        ui.label(egui::RichText::new("para el próximo tick").color(palette.dim));
                    });
                    ui.add(
                        egui::ProgressBar::new(done)
                            .desired_height(10.0)
                            .corner_radius(CornerRadius::same(5))
                            .fill(style::YELLOW),
                    );
                    ui.add_space(2.0);
                    match gateway::time_spent_skip_reason(snap.established) {
                        None => ui.horizontal_wrapped(|ui| {
                            badge(ui, "se mandaría ahora", style::GREEN);
                            ui.weak("Si el tick venciera en este momento, el opcode 41 saldría junto con un QoS heartbeat.");
                        }),
                        Some(reason) => ui.horizontal_wrapped(|ui| {
                            badge(ui, "se omitiría ahora", style::ORANGE);
                            ui.weak(format!("Motivo: {}.", reason.label()));
                        }),
                    };
                }
                _ => {
                    ui.horizontal_wrapped(|ui| {
                        badge(ui, "sin conexión", style::RED);
                        ui.weak("La cuenta regresiva arranca al conectar con el Gateway.");
                    });
                }
            }
            ui.add_space(6.0);
            egui::Grid::new("dev_track_op41").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                track_row(ui, "Intervalo", &fmt_duration(interval));
                track_row(
                    ui,
                    "Último envío",
                    &match (snap.time_spent_last_sent_at, snap.time_spent_last_reason) {
                        (Some(at), Some(reason)) => format!("hace {} · {}", fmt_ago(now, at), reason.label()),
                        _ => "todavía no se mandó".to_owned(),
                    },
                );
                track_row(ui, "Enviados (desde que abrió eCord)", &snap.time_spent_sent.to_string());
                track_row(ui, "Ticks omitidos", &snap.time_spent_skipped.to_string());
                track_row(
                    ui,
                    "Último tick omitido",
                    &match snap.time_spent_last_skip {
                        Some((at, reason)) => format!("hace {} · {}", fmt_ago(now, at), reason.label()),
                        None => "ninguno".to_owned(),
                    },
                );
                let (focused, in_voice) = gateway::qos_state();
                track_row(ui, "Ventana con foco", yes_no(focused));
                track_row(ui, "En llamada de voz", yes_no(in_voice));
            });
            ui.add_space(4.0);
            ui.weak(
                "Sale al conectar (READY/RESUMED) y cada 30 min si la ventana tiene foco o hay llamada. \
                 Si el tick vence sin foco ni llamada se omite y el siguiente intento es 30 min después.",
            );
        });

        // -- Latidos --
        card(ui, &format!("Latidos · opcode {} (con ACK en el 11)", gateway::heartbeat_opcode()), |ui| {
            if !snap.connected {
                ui.weak("Sin conexión con el Gateway.");
                return;
            }
            ui.horizontal_wrapped(|ui| {
                if snap.awaiting_ack {
                    badge(ui, "esperando ACK", style::ORANGE);
                } else {
                    badge(ui, "ACK al día", style::GREEN);
                }
            });
            ui.add_space(4.0);
            egui::Grid::new("dev_track_hb").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                track_row(
                    ui,
                    "Intervalo de Discord",
                    &snap.heartbeat_interval.map_or("— (falta el HELLO)".to_owned(), |d| format!("{:.2} s", d.as_secs_f32())),
                );
                track_row(
                    ui,
                    "Próximo latido en",
                    &snap
                        .next_heartbeat_at
                        .map_or("—".to_owned(), |at| fmt_duration(at.saturating_duration_since(now))),
                );
                track_row(
                    ui,
                    "Último latido",
                    &snap.last_heartbeat_at.map_or("todavía no se mandó".to_owned(), |at| format!("hace {}", fmt_ago(now, at))),
                );
                track_row(
                    ui,
                    "Último ACK",
                    &snap.last_ack_at.map_or("todavía no llegó".to_owned(), |at| format!("hace {}", fmt_ago(now, at))),
                );
                track_row(
                    ui,
                    "Ida y vuelta",
                    &snap.last_rtt.map_or("—".to_owned(), |rtt| format!("{} ms", rtt.as_millis())),
                );
                track_row(ui, "Latidos mandados / ACKs", &format!("{} / {}", snap.heartbeats_sent, snap.acks_received));
                let reasons = gateway::current_qos_reasons();
                track_row(
                    ui,
                    "Motivos QoS ahora",
                    &if reasons.is_empty() { "ninguno (active: false)".to_owned() } else { reasons.join(", ") },
                );
            });
        });

        // -- Conexión --
        card(ui, "Conexión con el Gateway", |ui| {
            ui.horizontal_wrapped(|ui| {
                match (snap.connected, snap.established) {
                    (true, true) => badge(ui, "sesión establecida", style::GREEN),
                    (true, false) => badge(ui, "conectando", style::ORANGE),
                    _ => badge(ui, "desconectado", style::RED),
                };
            });
            ui.add_space(4.0);
            egui::Grid::new("dev_track_conn").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                track_row(
                    ui,
                    "Conectado hace",
                    &match (snap.connected, snap.connected_at) {
                        (true, Some(at)) => fmt_ago(now, at),
                        _ => "—".to_owned(),
                    },
                );
                track_row(ui, "Conexiones abiertas", &snap.connections.to_string());
                track_row(ui, "Última secuencia (s)", &snap.sequence.map_or("—".to_owned(), |s| s.to_string()));
            });
        });
    }

    // -- Opcodes --------------------------------------------------------------------------------------

    fn opcodes(&mut self, ui: &mut egui::Ui) {
        let list: &[opcodes::Op] = if self.op_voice { &opcodes::VOICE[..] } else { &opcodes::GATEWAY[..] };

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.op_voice, false, "Gateway");
            ui.selectable_value(&mut self.op_voice, true, "Voz");
            ui.add(
                egui::TextEdit::singleline(&mut self.op_filter)
                    .desired_width(180.0)
                    .hint_text("Buscar por número o nombre"),
            );
            ui.checkbox(&mut self.op_only_missing, "Solo los que faltan");
        });
        let missing = opcodes::missing(list);
        ui.horizontal_wrapped(|ui| {
            badge(ui, &format!("{} implementados", list.len() - missing), style::GREEN);
            badge(ui, &format!("{missing} sin implementar"), style::ORANGE);
            ui.weak("Verde: eCord lo usa · naranja: falta en eCord · ↑ lo manda el cliente · ↓ lo recibe · ↕ ambos");
        });
        ui.add_space(4.0);

        let needle = self.op_filter.trim().to_lowercase();
        let voice = self.op_voice;
        let mut shown = 0;
        for (index, op) in list.iter().enumerate() {
            if self.op_only_missing && op.ecord.is_some() {
                continue;
            }
            if !needle.is_empty() && !op.code.to_string().starts_with(&needle) && !op.name.to_lowercase().contains(&needle)
            {
                continue;
            }
            shown += 1;
            let color = if op.ecord.is_some() { style::GREEN } else { style::ORANGE };
            let title = format!(
                "{:>2}  {}  {}{}",
                op.code,
                op.dir.arrow(),
                op.name,
                if op.deprecated { "  (obsoleto)" } else { "" }
            );
            egui::CollapsingHeader::new(egui::RichText::new(title).monospace().color(color))
                .id_salt(("dev_op", voice, index))
                .show(ui, |ui| {
                    egui::Grid::new(("dev_op_grid", voice, index)).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        track_row(ui, "Dirección", op.dir.label());
                        track_row(ui, "En eCord", op.ecord.unwrap_or("todavía no implementado"));
                        if !op.note.is_empty() {
                            track_row(ui, "Nota", op.note);
                        }
                    });
                    if let Some(template) = op.template {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.weak("Plantilla");
                            copy_button(ui, template);
                        });
                        let pretty = serde_json::from_str::<serde_json::Value>(template)
                            .ok()
                            .and_then(|value| serde_json::to_string_pretty(&value).ok())
                            .unwrap_or_else(|| template.to_owned());
                        ui.add(egui::Label::new(egui::RichText::new(pretty).monospace().size(12.0)).wrap().selectable(true));
                        ui.weak("Se puede cargar en Red → WebSocket con el selector \"Opcodes\".");
                    }
                });
        }
        if shown == 0 {
            ui.add_space(6.0);
            ui.weak("Ningún opcode coincide con el filtro.");
        }
    }

    // -- Identidad ----------------------------------------------------------------------------------

    fn identity(&mut self, ui: &mut egui::Ui) {
        let Some(identity) = rest::current_identity() else {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Todavía no hay identidad de cliente.").strong());
            ui.weak(
                "El fingerprint compartido se crea al iniciar sesión (discord::init_fingerprint). \
                 Inicia sesión y vuelve a esta pestaña.",
            );
            return;
        };
        let fields = parse_super_properties(&identity.super_properties_json);
        match self.section {
            Section::Identifiers => card(ui, "Sesión", |ui| session_rows(ui, &identity)),
            Section::SuperProperties => {
                card(ui, "X-Super-Properties", |ui| super_properties_section(ui, &identity, &fields));
            }
            _ => {
                let report = launch_signature::analyze(&identity.launch_signature);
                let declares_mods = fields
                    .as_ref()
                    .ok()
                    .and_then(|fields| fields.iter().find(|(key, _)| key.as_str() == "has_client_mods"))
                    .and_then(|(_, value)| value.as_bool());
                card(ui, "launch_signature", |ui| {
                    signature_section(ui, &identity, &report, declares_mods, &mut self.sig_input);
                });
            }
        }
    }

    // -- Entorno ----------------------------------------------------------------------------------------

    fn environment(&mut self, ui: &mut egui::Ui, remember: &mut bool) {
        card(ui, "Variables de entorno", |ui| {
            egui::Grid::new("dev_client_env").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                for name in [
                    "USE_DEVELOPER_FEATURES",
                    "ECORD_USE_DEVELOPER_FEATURES",
                    "ECORD_INSECURE_TLS",
                    "ECORD_OPAQUE",
                    "ECORD_STREAM_ENCODER",
                    "ECORD_NVENC_PROFILE",
                    "ECORD_KEEP_SEI",
                    "ECORD_KEEP_VUI",
                    "ECORD_NVENC_AUTO_LEVEL",
                    "ECORD_NVENC_SINGLE_SLICE",
                    "RUST_LOG",
                ] {
                    kv_row(ui, name, Some(&env_value(name)));
                }
            });
            if discord_client_rest::insecure_tls_enabled() {
                ui.colored_label(theme_error(), "TLS inseguro ACTIVO: no se verifican los certificados.");
            } else {
                ui.colored_label(theme_ok(), "Los certificados TLS se verifican.");
            }
        });

        card(ui, "Argumentos", |ui| {
            let args: Vec<String> = std::env::args().skip(1).collect();
            if args.is_empty() {
                ui.weak("Sin argumentos.");
            }
            for arg in args {
                ui.add(egui::Label::new(egui::RichText::new(arg).monospace()).selectable(true));
            }
        });

        card(ui, "Rutas", |ui| {
            egui::Grid::new("dev_client_paths").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                let show = |path: Option<std::path::PathBuf>| path.map(|path| path.display().to_string());
                kv_row(ui, "Registro (log)", show(crate::paths::log_file()).as_deref());
                kv_row(ui, "Cookies de Discord", show(crate::paths::discord_cookie_file()).as_deref());
                kv_row(ui, "Caché HTTP", show(crate::paths::http_cache_dir()).as_deref());
            });
        });

        card(ui, "Herramientas de desarrollo", |ui| {
            if ui
                .checkbox(remember, "Recordar activado en próximos arranques")
                .on_hover_text("Guarda la opción use_developer_features.")
                .changed()
            {
                let _ = web_local_storage_api::set_item(STORAGE_FLAG, if *remember { "true" } else { "false" });
            }
            ui.colored_label(theme_warn(), "Atajo: Ctrl+Shift+D abre y cierra este panel.");
        });
    }

    // -- Inspector ------------------------------------------------------------------------------------------

    /// Informe de diagnóstico en el panel derecho.
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, palette: &Palette, report: &str) -> Option<&'static str> {
        let mut toast = None;
        style::panel_title(ui, palette, "Informe");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Para pegar en un reporte de error. Sin token ni IDs de instalación.")
                    .small()
                    .color(palette.dim),
            );
            ui.add_space(6.0);
            if ui.button("Copiar informe").clicked() {
                ui.ctx().copy_text(report.to_owned());
                toast = Some("Informe copiado");
            }
            ui.add_space(8.0);
            super::inset_panel(ui, palette, |ui| {
                egui::ScrollArea::vertical().id_salt("dev_client_report").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(report).monospace().size(12.0)).wrap().selectable(true),
                    );
                });
            });
        });
        toast
    }
}