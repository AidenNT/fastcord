//! Pestaña "Discord": manda pedidos REST a la API de Discord desde las devtools.
//!
//! A diferencia de la pestaña HTTP de Red (que sale sin credenciales y con una
//! conexión emulada cualquiera), esta usa `RestClient::raw`, o sea el MISMO
//! cliente de la app: cookies de la sesión, huella TLS y, a elección, los
//! headers web del fingerprint (`X-Super-Properties`...) y el token de la
//! cuenta. Sirve para ver exactamente qué contesta Discord a lo que manda eCord.
//!
//! Seguridad:
//! - las rutas son relativas a la API (`/users/@me`): no se puede mandar el
//!   token a otro dominio;
//! - los pedidos que modifican datos (POST/PUT/PATCH/DELETE) con el token
//!   piden una segunda pulsación para confirmar;
//! - el token nunca se guarda en el historial: `Authorization` queda enmascarado
//!   y el curl copiado usa `$DISCORD_TOKEN`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};
use egui_extras::{Size, StripBuilder};

use super::console::fmt_clock;
use super::{HeaderRow, SessionInfo, badge, bar, inset_panel, lock, primary_button, runtime, style};
use crate::discord::rest::{RawExchange, RawOptions, RestClient};
use crate::logging::capture::{self, Level};
use crate::theme::{self, Palette};

const API_PREFIX: &str = "https://discord.com/api/v10";
/// Tope de cuerpo que se conserva de cada respuesta.
const MAX_BODY: usize = 200 * 1024;
/// Pedidos que se recuerdan.
const HISTORY_MAX: usize = 100;
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

struct Preset {
    category: &'static str,
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: &'static str,
    /// Necesita el token de la cuenta para ser útil.
    auth: bool,
}

const fn preset(
    category: &'static str,
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: &'static str,
    auth: bool,
) -> Preset {
    Preset { category, label, method, path, body, auth }
}

/// `{user_id}` se completa solo con el id de la cuenta; los demás `{...}` los
/// escribe el usuario en la ruta.
const PRESETS: &[Preset] = &[
    preset("Usuario", "Mi usuario", "GET", "/users/@me", "", true),
    preset("Usuario", "Mis servidores", "GET", "/users/@me/guilds?with_counts=true", "", true),
    preset("Usuario", "Mis mensajes directos", "GET", "/users/@me/channels", "", true),
    preset("Usuario", "Mis amistades", "GET", "/users/@me/relationships", "", true),
    preset("Usuario", "Mis conexiones", "GET", "/users/@me/connections", "", true),
    preset("Usuario", "Perfil de un usuario", "GET", "/users/{user_id}/profile?with_mutual_guilds=true", "", true),
    preset("Servidores", "Servidor", "GET", "/guilds/{guild_id}?with_counts=true", "", true),
    preset("Servidores", "Canales del servidor", "GET", "/guilds/{guild_id}/channels", "", true),
    preset("Servidores", "Roles del servidor", "GET", "/guilds/{guild_id}/roles", "", true),
    preset("Servidores", "Mi miembro", "GET", "/guilds/{guild_id}/members/@me", "", true),
    preset("Canales", "Canal", "GET", "/channels/{channel_id}", "", true),
    preset("Canales", "Últimos mensajes", "GET", "/channels/{channel_id}/messages?limit=5", "", true),
    preset("Canales", "Mensajes fijados", "GET", "/channels/{channel_id}/pins", "", true),
    preset(
        "Canales",
        "Enviar mensaje",
        "POST",
        "/channels/{channel_id}/messages",
        "{\n  \"content\": \"Mensaje de prueba desde las devtools\"\n}",
        true,
    ),
    preset("Sin sesión", "Gateway", "GET", "/gateway", "", false),
    preset("Sin sesión", "Experimentos", "GET", "/experiments", "", false),
    preset("Sin sesión", "Ubicación (IP)", "GET", "/auth/location-metadata", "", false),
    preset("Sin sesión", "Invitación", "GET", "/invites/{code}?with_counts=true", "", false),
    preset("Sin sesión", "Aplicaciones detectables", "GET", "/applications/detectable", "", false),
];

const CATEGORIES: [&str; 4] = ["Usuario", "Servidores", "Canales", "Sin sesión"];

// ---------------------------------------------------------------------------
// Datos compartidos con la tarea que hace el pedido
// ---------------------------------------------------------------------------

/// Un pedido ya terminado (con éxito o con error de red).
pub(super) struct Exchange {
    id: u64,
    unix_ms: i64,
    method: String,
    path: String,
    with_token: bool,
    status: Option<u16>,
    reason: String,
    version: String,
    elapsed_ms: u128,
    request_headers: Vec<(String, String)>,
    request_body: String,
    response_headers: Vec<(String, String)>,
    /// Cuerpo listo para mostrar (JSON indentado si se pudo).
    body: String,
    body_len: usize,
    truncated: bool,
    error: Option<String>,
}

#[derive(Default)]
struct Shared {
    running: bool,
    next_id: u64,
    history: Vec<Arc<Exchange>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Body,
    ResponseHeaders,
    Request,
}

pub(super) struct DiscordState {
    method: String,
    path: String,
    body: String,
    body_open: bool,
    use_token: bool,
    client_headers: bool,
    extra: Vec<HeaderRow>,
    extra_open: bool,
    /// Huella del pedido que ya pidió confirmación (ver `request_key`).
    confirm_key: Option<u64>,
    shared: Arc<Mutex<Shared>>,
    /// `None` = el último.
    selected: Option<u64>,
    view: View,
    filter: String,
}

impl Default for DiscordState {
    fn default() -> Self {
        Self {
            method: "GET".to_owned(),
            path: "/users/@me".to_owned(),
            body: String::new(),
            body_open: false,
            use_token: true,
            client_headers: true,
            extra: Vec::new(),
            extra_open: false,
            confirm_key: None,
            shared: Arc::new(Mutex::new(Shared::default())),
            selected: None,
            view: View::Body,
            filter: String::new(),
        }
    }
}

/// Reemplaza `{user_id}` y rechaza cualquier otro `{marcador}` sin completar.
fn resolve_path(path: &str, session: &SessionInfo) -> Result<String, String> {
    let mut out = path.trim().to_owned();
    if let Some(id) = &session.user_id {
        out = out.replace("{user_id}", id);
    }
    if let Some(start) = out.find('{') {
        let end = out[start..].find('}').map_or(out.len(), |offset| start + offset + 1);
        return Err(format!("Completa {} en la ruta antes de enviar.", &out[start..end]));
    }
    Ok(out)
}

fn pretty_or_raw(bytes: &[u8], truncated: bool) -> String {
    if !truncated {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty;
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[allow(clippy::too_many_arguments)]
fn build_exchange(
    id: u64,
    method: String,
    path: String,
    with_token: bool,
    request_body: String,
    result: Result<RawExchange, String>,
) -> Exchange {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    match result {
        Ok(raw) => {
            let truncated = raw.body.len() > MAX_BODY;
            let shown = &raw.body[..raw.body.len().min(MAX_BODY)];
            Exchange {
                id,
                unix_ms,
                method,
                path,
                with_token,
                status: Some(raw.status),
                reason: raw.reason,
                version: raw.version,
                elapsed_ms: raw.elapsed_ms,
                request_headers: raw.request_headers,
                request_body,
                response_headers: raw.response_headers,
                body: pretty_or_raw(shown, truncated),
                body_len: raw.body.len(),
                truncated,
                error: None,
            }
        }
        Err(error) => Exchange {
            id,
            unix_ms,
            method,
            path,
            with_token,
            status: None,
            reason: String::new(),
            version: String::new(),
            elapsed_ms: 0,
            request_headers: Vec::new(),
            request_body,
            response_headers: Vec::new(),
            body: String::new(),
            body_len: 0,
            truncated: false,
            error: Some(error),
        },
    }
}

/// `curl` equivalente; el token se reemplaza por `$DISCORD_TOKEN`.
fn curl_for(exchange: &Exchange) -> String {
    let mut out = format!("curl -X {} '{API_PREFIX}{}'", exchange.method, exchange.path);
    for (name, value) in &exchange.request_headers {
        let value = if name.eq_ignore_ascii_case("authorization") {
            "$DISCORD_TOKEN".to_owned()
        } else {
            value.replace('\'', "'\\''")
        };
        out.push_str(&format!(" \\\n  -H '{name}: {value}'"));
    }
    if !exchange.request_body.is_empty() {
        out.push_str(&format!(" \\\n  --data-raw '{}'", exchange.request_body.replace('\'', "'\\''")));
    }
    out
}

fn headers_grid(ui: &mut egui::Ui, id: &str, headers: &[(String, String)]) {
    egui::Grid::new(id).striped(true).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
        for (name, value) in headers {
            ui.label(egui::RichText::new(name).monospace().strong());
            ui.add(egui::Label::new(egui::RichText::new(value).monospace()).selectable(true));
            ui.end_row();
        }
    });
}

impl DiscordState {
    fn request_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.method, &self.path, &self.body, self.use_token, self.client_headers).hash(&mut hasher);
        hasher.finish()
    }

    fn latest(&self) -> Option<Arc<Exchange>> {
        let guard = lock(&self.shared);
        match self.selected {
            Some(id) => guard.history.iter().find(|exchange| exchange.id == id).cloned(),
            None => guard.history.last().cloned(),
        }
    }

    fn apply_preset(&mut self, preset: &Preset, has_token: bool) {
        self.method = preset.method.to_owned();
        self.path = preset.path.to_owned();
        self.body = preset.body.to_owned();
        self.body_open = !preset.body.is_empty();
        self.use_token = preset.auth && has_token;
        self.confirm_key = None;
    }

    fn record_failure(&mut self, message: String) {
        let exchange = build_exchange(
            self.next_id(),
            self.method.clone(),
            self.path.clone(),
            false,
            self.body.clone(),
            Err(message),
        );
        lock(&self.shared).history.push(Arc::new(exchange));
        self.selected = None;
    }

    fn next_id(&self) -> u64 {
        let mut guard = lock(&self.shared);
        guard.next_id += 1;
        guard.next_id
    }

    /// Manda el pedido armado en la barra.
    pub(super) fn send(&mut self, ctx: &egui::Context, session: &SessionInfo) {
        let path = match resolve_path(&self.path, session) {
            Ok(path) => path,
            Err(message) => {
                self.record_failure(message);
                return;
            }
        };
        let method_name = self.method.clone();
        let Ok(method) = reqwest::Method::from_bytes(method_name.as_bytes()) else {
            self.record_failure(format!("Método inválido: {method_name}"));
            return;
        };
        let with_token = self.use_token && session.token.is_some();
        let token = if with_token { session.token.clone().unwrap_or_default() } else { String::new() };
        let sends_body = !matches!(method_name.as_str(), "GET" | "HEAD") && !self.body.trim().is_empty();
        let body = sends_body.then(|| self.body.clone());
        let options = RawOptions {
            client_headers: self.client_headers,
            auth: with_token,
            extra: self
                .extra
                .iter()
                .filter(|row| row.on && !row.name.trim().is_empty())
                .map(|row| (row.name.trim().to_owned(), row.value.clone()))
                .collect(),
        };

        let id = self.next_id();
        lock(&self.shared).running = true;
        self.selected = None;
        capture::push(Level::Info, "devtools::discord", format!("→ {method_name} {path}"));

        let shared = Arc::clone(&self.shared);
        let ctx = ctx.clone();
        runtime().spawn(async move {
            let client = RestClient::for_token(token);
            let request_body = body.clone().unwrap_or_default();
            let result = client.raw(method, &path, body, options).await;
            let exchange = build_exchange(id, method_name.clone(), path.clone(), with_token, request_body, result);
            match (&exchange.status, &exchange.error) {
                (Some(status), _) => capture::push(
                    if *status < 400 { Level::Info } else { Level::Warn },
                    "devtools::discord",
                    format!("← {status} {method_name} {path} ({} ms, {} bytes)", exchange.elapsed_ms, exchange.body_len),
                ),
                (None, Some(error)) => capture::push(
                    Level::Error,
                    "devtools::discord",
                    format!("✗ {method_name} {path}: {}", error.lines().next().unwrap_or("error")),
                ),
                (None, None) => {}
            }
            let mut guard = lock(&shared);
            guard.running = false;
            guard.history.push(Arc::new(exchange));
            if guard.history.len() > HISTORY_MAX {
                let excess = guard.history.len() - HISTORY_MAX;
                guard.history.drain(..excess);
            }
            drop(guard);
            ctx.request_repaint();
        });
    }

    /// Atajo de la consola (`get /users/@me`): arma el pedido y lo manda con
    /// el token de la sesión, si hay.
    pub(super) fn quick(&mut self, ctx: &egui::Context, session: &SessionInfo, method: &str, path: &str, body: &str) {
        self.method = method.to_ascii_uppercase();
        self.path = path.to_owned();
        self.body = body.to_owned();
        self.body_open = !body.is_empty();
        self.use_token = session.token.is_some();
        self.confirm_key = None;
        self.send(ctx, session);
    }

    pub(super) fn busy(&self) -> bool {
        lock(&self.shared).running
    }

    // -- Lista lateral ---------------------------------------------------------------

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette, session: &SessionInfo) {
        let has_token = session.token.is_some();
        StripBuilder::new(ui).size(Size::remainder()).size(Size::exact(56.0)).vertical(|mut strip| {
            strip.cell(|ui| {
                style::panel_title(ui, palette, "Endpoints");
                egui::ScrollArea::vertical().id_salt("dev_discord_side").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    let needle = self.filter.trim().to_lowercase();
                    for category in CATEGORIES {
                        let matching: Vec<&Preset> = PRESETS
                            .iter()
                            .filter(|preset| {
                                preset.category == category
                                    && (needle.is_empty()
                                        || preset.label.to_lowercase().contains(&needle)
                                        || preset.path.to_lowercase().contains(&needle))
                            })
                            .collect();
                        if matching.is_empty() {
                            continue;
                        }
                        style::side_header(ui, palette, category);
                        for preset in matching {
                            let selected = self.method == preset.method && self.path == preset.path;
                            let item = style::side_item(
                                ui,
                                palette,
                                preset.label,
                                style::method_color(preset.method),
                                selected,
                                None,
                            )
                            .on_hover_text(format!("{} {}", preset.method, preset.path));
                            if item.clicked() {
                                self.apply_preset(preset, has_token);
                            }
                        }
                    }
                    ui.add_space(10.0);
                });
            });
            strip.cell(|ui| {
                let rect = ui.max_rect();
                ui.painter().hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, palette.outline));
                egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .desired_width(f32::INFINITY)
                            .hint_text("Filtrar endpoints…"),
                    );
                });
            });
        });
    }

    // -- Centro ------------------------------------------------------------------------

    pub(super) fn page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette, session: &SessionInfo) {
        let has_token = session.token.is_some();
        if !has_token {
            self.use_token = false;
        }
        let running = self.busy();

        theme::section_title(ui, palette, "API de Discord");
        theme::text(
            ui,
            "Pedidos REST con la huella del cliente y, si quieres, el token de la sesión actual.",
            theme::regular(12.0),
            palette.dim,
        );
        ui.add_space(4.0);

        let mutating = !matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS");
        let key = self.request_key();
        let armed = self.confirm_key == Some(key);
        let needs_confirm = mutating && self.use_token && has_token;

        bar(ui, palette, |ui| {
            egui::ComboBox::from_id_salt("dev_discord_method").selected_text(self.method.clone()).width(80.0).show_ui(
                ui,
                |ui| {
                    for method in METHODS {
                        ui.selectable_value(&mut self.method, method.to_owned(), method);
                    }
                },
            );
            ui.label(egui::RichText::new("…/api/v10").monospace().color(palette.dim));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .frame(egui::Frame::NONE)
                    .font(egui::TextStyle::Monospace)
                    .desired_width((ui.available_width() - 120.0).max(100.0))
                    .hint_text("/users/@me"),
            );
            let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let label = if needs_confirm && armed { "¿Confirmar?" } else { "Enviar" };
            let clicked = primary_button(ui, palette, label, !running);
            if (clicked || (enter && !running)) && !running {
                if needs_confirm && !armed {
                    self.confirm_key = Some(key);
                } else {
                    self.confirm_key = None;
                    self.send(ctx, session);
                }
            }
            if running {
                theme::spinner(ui, 16.0, palette.accent);
            }
        });
        if needs_confirm && armed {
            ui.label(
                egui::RichText::new("Este pedido modifica datos de tu cuenta. Pulsa de nuevo para confirmar.")
                    .small()
                    .color(style::ORANGE),
            );
        }

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(has_token, |ui| {
                ui.checkbox(&mut self.use_token, "Token de la cuenta").on_hover_text(
                    "Manda el token de la sesión actual en Authorization. Solo viaja a discord.com.",
                );
            });
            ui.checkbox(&mut self.client_headers, "Headers del cliente")
                .on_hover_text("X-Super-Properties, User-Agent, Sec-CH-UA, Referer… (los mismos que usa la app)");
            ui.toggle_value(&mut self.body_open, "Cuerpo");
            let extra_label = format!("Headers extra ({})", self.extra.iter().filter(|row| row.on).count());
            ui.toggle_value(&mut self.extra_open, extra_label);
            if !has_token {
                ui.label(egui::RichText::new("Sin sesión: los pedidos salen sin token.").small().color(palette.dim));
            }
        });

        if self.body_open {
            ui.add(
                egui::TextEdit::multiline(&mut self.body)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .hint_text("{ \"clave\": \"valor\" } — se ignora en GET"),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Formatear JSON").clicked() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.body) {
                        if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                            self.body = pretty;
                        }
                    }
                }
                if serde_json::from_str::<serde_json::Value>(&self.body).is_err() && !self.body.trim().is_empty() {
                    ui.label(egui::RichText::new("JSON no válido").small().color(style::ORANGE));
                }
            });
        }

        if self.extra_open {
            let mut remove: Option<usize> = None;
            egui::Grid::new("dev_discord_extra").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                for (index, row) in self.extra.iter_mut().enumerate() {
                    ui.checkbox(&mut row.on, "");
                    ui.add(
                        egui::TextEdit::singleline(&mut row.name)
                            .desired_width(150.0)
                            .hint_text("Nombre")
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut row.value)
                            .desired_width(240.0)
                            .hint_text("Valor")
                            .font(egui::TextStyle::Monospace),
                    );
                    if ui.small_button("✕").clicked() {
                        remove = Some(index);
                    }
                    ui.end_row();
                }
            });
            if let Some(index) = remove {
                self.extra.remove(index);
            }
            if ui.small_button("+ Header").clicked() {
                self.extra.push(HeaderRow::new("", ""));
            }
        }

        ui.add_space(4.0);
        self.response_ui(ui, palette, running);
    }

    fn response_ui(&mut self, ui: &mut egui::Ui, palette: &Palette, running: bool) {
        let Some(exchange) = self.latest() else {
            inset_panel(ui, palette, |ui| {
                ui.add_space(18.0);
                ui.vertical_centered(|ui| {
                    if running {
                        theme::spinner(ui, 20.0, palette.accent);
                        ui.label(egui::RichText::new("Enviando…").color(palette.secondary));
                    } else {
                        ui.label(egui::RichText::new("Sin respuesta todavía.").color(palette.secondary));
                        ui.label(
                            egui::RichText::new("Elige un endpoint de la lista o escribe la ruta y pulsa Enviar.")
                                .small()
                                .color(palette.dim),
                        );
                    }
                });
            });
            return;
        };

        ui.horizontal_wrapped(|ui| {
            match (exchange.status, &exchange.error) {
                (Some(status), _) => {
                    badge(ui, &format!("{status} {}", exchange.reason), style::status_color(status));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} ms · {} bytes",
                            exchange.version, exchange.elapsed_ms, exchange.body_len
                        ))
                        .color(palette.secondary),
                    );
                    if exchange.truncated {
                        ui.weak(format!("(se muestran {} KB)", MAX_BODY / 1024));
                    }
                }
                (None, _) => badge(ui, "Falló", style::RED),
            }
            badge(ui, if exchange.with_token { "con token" } else { "sin token" }, style::PURPLE);
            if running {
                theme::spinner(ui, 14.0, palette.accent);
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, View::Body, "Respuesta");
            ui.selectable_value(&mut self.view, View::ResponseHeaders, "Headers recibidos");
            ui.selectable_value(&mut self.view, View::Request, "Petición enviada");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("copiar respuesta").clicked() {
                    ui.ctx().copy_text(exchange.body.clone());
                }
            });
        });

        inset_panel(ui, palette, |ui| {
            egui::ScrollArea::both().id_salt("dev_discord_response").auto_shrink([false, false]).show(ui, |ui| {
                if let Some(error) = &exchange.error {
                    ui.add(egui::Label::new(egui::RichText::new(error).monospace().color(style::RED)).selectable(true));
                    return;
                }
                match self.view {
                    View::Body => {
                        if exchange.body.is_empty() {
                            ui.weak("(cuerpo vacío)");
                        } else {
                            ui.add(egui::Label::new(egui::RichText::new(&exchange.body).monospace()).selectable(true));
                        }
                    }
                    View::ResponseHeaders => headers_grid(ui, "dev_discord_resp_headers", &exchange.response_headers),
                    View::Request => {
                        ui.label(
                            egui::RichText::new(format!("{} {API_PREFIX}{}", exchange.method, exchange.path))
                                .monospace()
                                .strong(),
                        );
                        ui.add_space(4.0);
                        headers_grid(ui, "dev_discord_req_headers", &exchange.request_headers);
                        if !exchange.request_body.is_empty() {
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(egui::RichText::new(&exchange.request_body).monospace())
                                    .selectable(true),
                            );
                        }
                    }
                }
            });
        });
    }

    // -- Historial (inspector) ---------------------------------------------------------

    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) -> Option<&'static str> {
        let mut toast = None;
        style::panel_title(ui, palette, "Historial");

        let current = self.latest();
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 0)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(current.is_some(), egui::Button::new("Copiar curl").small()).clicked() {
                    if let Some(exchange) = &current {
                        ctx.copy_text(curl_for(exchange));
                        toast = Some("curl copiado (con $DISCORD_TOKEN)");
                    }
                }
                if ui.add_enabled(current.is_some(), egui::Button::new("Repetir").small()).clicked() {
                    if let Some(exchange) = &current {
                        self.method = exchange.method.clone();
                        self.path = exchange.path.clone();
                        self.body = exchange.request_body.clone();
                        self.body_open = !self.body.is_empty();
                        self.confirm_key = None;
                    }
                }
                if ui.small_button("Limpiar").clicked() {
                    lock(&self.shared).history.clear();
                    self.selected = None;
                }
            });
        });
        ui.add_space(8.0);

        let history: Vec<Arc<Exchange>> = lock(&self.shared).history.iter().rev().cloned().collect();
        if history.is_empty() {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
                ui.label(egui::RichText::new("Aquí aparecen los pedidos que envíes.").color(palette.dim));
            });
            return toast;
        }
        let selected_id = current.as_ref().map(|exchange| exchange.id);
        egui::ScrollArea::vertical().id_salt("dev_discord_history").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for exchange in &history {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                let is_selected = selected_id == Some(exchange.id);
                {
                    let painter = ui.painter();
                    let inner = rect.shrink2(vec2(6.0, 1.0));
                    if is_selected {
                        painter.rect_filled(inner, CornerRadius::same(8), palette.surface_active);
                    } else if response.hovered() {
                        painter.rect_filled(inner, CornerRadius::same(8), palette.surface_hover);
                    }
                    let color = style::method_color(&exchange.method);
                    painter.text(
                        pos2(rect.left() + 18.0, rect.top() + 15.0),
                        Align2::LEFT_CENTER,
                        &exchange.method,
                        theme::bold(11.5),
                        color,
                    );
                    let clip = Rect::from_min_max(pos2(rect.left() + 66.0, rect.top()), pos2(rect.right() - 14.0, rect.bottom()));
                    painter.with_clip_rect(clip).text(
                        pos2(rect.left() + 66.0, rect.top() + 15.0),
                        Align2::LEFT_CENTER,
                        &exchange.path,
                        theme::regular(12.5),
                        palette.text,
                    );
                    let (summary, summary_color) = match (exchange.status, &exchange.error) {
                        (Some(status), _) => (
                            format!("{status} · {} ms · {}", exchange.elapsed_ms, fmt_clock(exchange.unix_ms)),
                            style::status_color(status),
                        ),
                        _ => (format!("error · {}", fmt_clock(exchange.unix_ms)), style::RED),
                    };
                    painter.text(
                        pos2(rect.left() + 18.0, rect.top() + 33.0),
                        Align2::LEFT_CENTER,
                        summary,
                        theme::regular(11.5),
                        summary_color,
                    );
                    painter.hline(
                        rect.left() + 14.0..=rect.right() - 14.0,
                        rect.bottom() - 0.5,
                        Stroke::new(1.0, palette.outline.gamma_multiply(0.7)),
                    );
                }
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.selected = Some(exchange.id);
                }
            }
        });
        toast
    }
}
