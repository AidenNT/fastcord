//! Pestaña "Cliente": qué es esta instancia de eCord y qué declara hacia
//! afuera. Resumen del proceso y del sistema, rendimiento (tiempo de cuadro),
//! memoria, identidad ante Discord (IDs, `X-Super-Properties`, `launch_signature`)
//! y entorno (variables, argumentos, rutas).
//!
//! También arma el "informe de diagnóstico" de texto que se muestra en el
//! inspector y se copia con un clic. Ese informe NO incluye el token ni los
//! identificadores de instalación o fingerprint, para poder pegarlo en un reporte.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use egui::{CornerRadius, Sense, Stroke, pos2, vec2};

use super::{
    SessionInfo, STORAGE_FLAG, card, kv_row, parse_super_properties, session_rows, signature_section,
    super_properties_section, style, theme_error, theme_ok, theme_warn,
};
use crate::discord::{launch_signature, rest};
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
    Identifiers,
    SuperProperties,
    Signature,
    Environment,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Summary,
        Section::Performance,
        Section::Memory,
        Section::Identifiers,
        Section::SuperProperties,
        Section::Signature,
        Section::Environment,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Summary => "Resumen",
            Section::Performance => "Rendimiento",
            Section::Memory => "Memoria",
            Section::Identifiers => "Identificadores",
            Section::SuperProperties => "Super-Properties",
            Section::Signature => "launch_signature",
            Section::Environment => "Entorno",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Section::Summary => "Esta instancia de eCord, el sistema donde corre y la sesión de Discord.",
            Section::Performance => "Tiempo de cada cuadro de la interfaz mientras esta pestaña está abierta.",
            Section::Memory => "RAM del proceso, texturas de la GPU y cachés en disco.",
            Section::Identifiers => "Lo que este cliente declara de sí mismo: installation ID, fingerprint y build.",
            Section::SuperProperties => "El header X-Super-Properties tal como sale, en el orden en que se envía.",
            Section::Signature => "Mods de cliente detectados por bitfield y coherencia con has_client_mods.",
            Section::Environment => "Variables de entorno, argumentos de línea de comandos y rutas de trabajo.",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Section::Summary => style::BLUE,
            Section::Performance => style::GREEN,
            Section::Memory => style::ORANGE,
            Section::Identifiers => style::PURPLE,
            Section::SuperProperties => style::PINK,
            Section::Signature => style::RED,
            Section::Environment => style::TEAL,
        }
    }
}

pub(super) struct ClientState {
    pub section: Section,
    frame_ms: VecDeque<f32>,
    sig_input: String,
}

impl Default for ClientState {
    fn default() -> Self {
        Self { section: Section::Summary, frame_ms: VecDeque::new(), sig_input: String::new() }
    }
}

fn fmt_duration(duration: Duration) -> String {
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
                Section::Identifiers | Section::SuperProperties | Section::Signature => self.identity(ui),
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
