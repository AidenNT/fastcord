//! Sistema de diálogos unificado de ecord: un solo diseño (fondo oscuro
//! teñido + tarjeta centrada con franja de color, icono con halo, botones en
//! píldora) para avisos, confirmaciones, novedades y formularios.
//!
//! # Uso rápido
//!
//! ```ignore
//! // Aviso informativo con un botón:
//! app.open_dialog(Dialog::info("sync_ok", "Todo listo", "Se sincronizó tu cuenta."));
//!
//! // Confirmación; el closure recibe qué botón se tocó:
//! app.open_dialog(
//!     Dialog::confirm("borrar_tema", DialogKind::Danger, "¿Borrar tema?",
//!                     "No se puede deshacer.", "Borrar")
//!         .on_result(|app, _ctx, result| {
//!             if result.is("confirm") { /* ... */ }
//!         }),
//! );
//!
//! // Formulario con validación de campos obligatorios:
//! app.open_dialog(
//!     Dialog::new("renombrar", DialogKind::Question, "Renombrar")
//!         .field(FormField::new("name", "Nombre").required().max_len(32))
//!         .button("cancel", "Cancelar", ButtonStyle::Secondary)
//!         .button("ok", "Guardar", ButtonStyle::Primary)
//!         .dismissable()
//!         .on_result(|app, _ctx, result| {
//!             if result.is("ok") {
//!                 let name = result.value("name").unwrap_or_default();
//!             }
//!         }),
//! );
//! ```
//!
//! Los diálogos van en una cola (`App::dialogs`): se muestra de a uno, y
//! `App::open_dialog` ignora uno con un `id` que ya está en la cola.
//!
//! # Entrada
//!
//! Mientras haya un diálogo abierto, el teclado no le llega a la app de atrás
//! (`begin_frame` separa los eventos de teclado antes de dibujar las
//! pantallas y `render` se los devuelve solo al diálogo). El ratón lo frena
//! el propio fondo oscuro.

use std::collections::HashMap;
use std::time::Duration;

use egui::{
    Align, Align2, Area, Color32, CornerRadius, Frame, Id, Layout, Margin, Order, Rect, Response,
    RichText, Sense, Stroke, Vec2,
};

use crate::lib::state::App;
use crate::ui::theme::{self, Icon, Palette};

const CARD_MARGIN: i8 = 24;
fn card_radius() -> u8 {
    theme::radius() + 8
}
const DEFAULT_WIDTH: f32 = 480.0;
/// Azul de los avisos informativos (la paleta no trae uno).
const INFO_BLUE: Color32 = Color32::from_rgb(0x5b, 0x9b, 0xf5);

const WAS_OPEN_KEY: &str = "dialog_was_open";
const STASH_KEY: &str = "dialog_stashed_events";

// ---------------------------------------------------------------------------
// Tipos públicos
// ---------------------------------------------------------------------------

/// Tono del diálogo: decide el color, el icono y el tinte del fondo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Info,
    Success,
    Warning,
    Danger,
    Question,
    /// Novedades / cosas nuevas.
    Sparkle,
}

impl DialogKind {
    pub fn color(self, palette: &Palette) -> Color32 {
        match self {
            DialogKind::Info => INFO_BLUE,
            DialogKind::Success | DialogKind::Question | DialogKind::Sparkle => palette.accent,
            DialogKind::Warning => palette.warning,
            DialogKind::Danger => palette.danger,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}

#[derive(Clone, Debug)]
pub struct DialogButton {
    pub id: String,
    pub label: String,
    pub style: ButtonStyle,
    /// Si es `true`, tocarlo exige que los campos obligatorios estén llenos.
    pub validate: bool,
}

/// Tipo de cambio en una entrada del changelog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeTag {
    New,
    Improved,
    Fixed,
    Removed,
}

#[derive(Clone, Debug)]
pub struct ChangeEntry {
    pub tag: ChangeTag,
    pub text: String,
}

/// Una versión del changelog.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub title: Option<String>,
    pub entries: Vec<ChangeEntry>,
}

impl Release {
    pub fn new(version: impl Into<String>) -> Self {
        Self { version: version.into(), date: None, title: None, entries: Vec::new() }
    }
    pub fn date(mut self, date: impl Into<String>) -> Self {
        self.date = Some(date.into());
        self
    }
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
    pub fn entry(mut self, tag: ChangeTag, text: impl Into<String>) -> Self {
        self.entries.push(ChangeEntry { tag, text: text.into() });
        self
    }
    pub fn added(self, text: impl Into<String>) -> Self {
        self.entry(ChangeTag::New, text)
    }
    pub fn improved(self, text: impl Into<String>) -> Self {
        self.entry(ChangeTag::Improved, text)
    }
    pub fn fixed(self, text: impl Into<String>) -> Self {
        self.entry(ChangeTag::Fixed, text)
    }
    pub fn removed(self, text: impl Into<String>) -> Self {
        self.entry(ChangeTag::Removed, text)
    }
}

/// Campo de texto de un formulario.
#[derive(Clone, Debug)]
pub struct FormField {
    pub id: String,
    pub label: String,
    pub placeholder: String,
    pub value: String,
    pub multiline: bool,
    pub rows: usize,
    pub password: bool,
    pub required: bool,
    pub max_len: usize,
}

impl FormField {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            placeholder: String::new(),
            value: String::new(),
            multiline: false,
            rows: 5,
            password: false,
            required: false,
            max_len: 4000,
        }
    }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }
    pub fn value(mut self, text: impl Into<String>) -> Self {
        self.value = text.into();
        self
    }
    pub fn multiline(mut self, yes: bool) -> Self {
        self.multiline = yes;
        self
    }
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }
    /// Marca el campo con `*` y exige que no esté vacío al validar.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn max_len(mut self, max: usize) -> Self {
        self.max_len = max;
        self
    }
}

/// Bloques de contenido del cuerpo, en el orden en que se dibujan.
#[derive(Clone, Debug)]
pub enum Block {
    Text(String),
    /// Línea chica con un icono (consejos, notas al pie).
    Note { icon: Icon, text: String },
    /// Tarjeta con un nombre en píldora monoespaciada y su explicación.
    Flag { name: String, text: String },
    /// Recuadro de color con título opcional.
    Callout { kind: DialogKind, title: String, text: String },
    Bullets(Vec<String>),
    Changelog(Vec<Release>),
    Field(FormField),
    /// Encabezado "La aplicación (foto + nombre) solicitó ..." para los
    /// diálogos que pide un bot.
    AppBanner { name: String, avatar_url: Option<String>, request: String },
}

/// Qué pasó al cerrarse el diálogo.
#[derive(Clone, Debug)]
pub struct DialogResult {
    /// `id` del botón tocado; `None` si se descartó (Escape, fondo o ✕).
    pub button: Option<String>,
    /// Valor de cada campo del formulario por su `id`.
    pub values: HashMap<String, String>,
}

impl DialogResult {
    pub fn is(&self, button_id: &str) -> bool {
        self.button.as_deref() == Some(button_id)
    }
    pub fn dismissed(&self) -> bool {
        self.button.is_none()
    }
    pub fn value(&self, field_id: &str) -> Option<&str> {
        self.values.get(field_id).map(String::as_str)
    }
}

pub type ResultFn = Box<dyn FnOnce(&mut App, &egui::Context, DialogResult)>;

pub struct Dialog {
    pub id: String,
    pub kind: DialogKind,
    pub title: String,
    pub subtitle: Option<String>,
    pub blocks: Vec<Block>,
    pub buttons: Vec<DialogButton>,
    /// Mensaje de error bajo el cuerpo (validación).
    pub error: Option<String>,
    pub width: f32,
    pub dismiss_on_escape: bool,
    pub dismiss_on_scrim: bool,
    /// Halo del icono con latido (para avisos importantes).
    pub pulse: bool,
    pub on_result: Option<ResultFn>,
}

impl Dialog {
    pub fn new(id: impl Into<String>, kind: DialogKind, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            subtitle: None,
            blocks: Vec::new(),
            buttons: Vec::new(),
            error: None,
            width: DEFAULT_WIDTH,
            dismiss_on_escape: false,
            dismiss_on_scrim: false,
            pulse: false,
            on_result: None,
        }
    }

    /// Aviso informativo con un solo botón "Entendido".
    pub fn info(id: impl Into<String>, title: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(id, DialogKind::Info, title)
            .text(text)
            .button("ok", "Entendido", ButtonStyle::Primary)
            .dismissable()
    }

    /// Confirmación con "Cancelar" y un botón `confirm`.
    pub fn confirm(
        id: impl Into<String>,
        kind: DialogKind,
        title: impl Into<String>,
        text: impl Into<String>,
        confirm_label: impl Into<String>,
    ) -> Self {
        let style = if kind == DialogKind::Danger { ButtonStyle::Danger } else { ButtonStyle::Primary };
        Self::new(id, kind, title)
            .text(text)
            .button("cancel", "Cancelar", ButtonStyle::Secondary)
            .button("confirm", confirm_label, style)
            .dismissable()
    }

    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Text(text.into()));
        self
    }
    pub fn note(self, text: impl Into<String>) -> Self {
        self.note_icon(Icon::Info, text)
    }
    pub fn note_icon(mut self, icon: Icon, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Note { icon, text: text.into() });
        self
    }
    pub fn flag(mut self, name: impl Into<String>, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Flag { name: name.into(), text: text.into() });
        self
    }
    pub fn callout(
        mut self,
        kind: DialogKind,
        title: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.blocks.push(Block::Callout { kind, title: title.into(), text: text.into() });
        self
    }
    pub fn bullets<S: Into<String>>(mut self, items: impl IntoIterator<Item = S>) -> Self {
        self.blocks.push(Block::Bullets(items.into_iter().map(Into::into).collect()));
        self
    }
    pub fn changelog(mut self, releases: Vec<Release>) -> Self {
        self.blocks.push(Block::Changelog(releases));
        self
    }
    pub fn field(mut self, field: FormField) -> Self {
        self.blocks.push(Block::Field(field));
        self
    }
    /// Línea "La aplicación <foto> <nombre> <request>", por ejemplo con
    /// `request = "solicitó que llenes un formulario"`.
    pub fn app_banner(
        mut self,
        name: impl Into<String>,
        avatar_url: Option<String>,
        request: impl Into<String>,
    ) -> Self {
        self.blocks.push(Block::AppBanner {
            name: name.into(),
            avatar_url,
            request: request.into(),
        });
        self
    }
    pub fn error(mut self, text: impl Into<String>) -> Self {
        self.error = Some(text.into());
        self
    }
    /// Botones en orden visual de izquierda a derecha; el último queda más a
    /// la derecha (el principal, normalmente). Los `Secondary` no validan.
    pub fn button(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        style: ButtonStyle,
    ) -> Self {
        self.buttons.push(DialogButton {
            id: id.into(),
            label: label.into(),
            style,
            validate: style != ButtonStyle::Secondary,
        });
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    pub fn pulse(mut self) -> Self {
        self.pulse = true;
        self
    }
    pub fn dismiss_on_escape(mut self, yes: bool) -> Self {
        self.dismiss_on_escape = yes;
        self
    }
    pub fn dismiss_on_scrim(mut self, yes: bool) -> Self {
        self.dismiss_on_scrim = yes;
        self
    }
    /// Escape, clic afuera y ✕ lo cierran (`DialogResult::button == None`).
    pub fn dismissable(self) -> Self {
        self.dismiss_on_escape(true).dismiss_on_scrim(true)
    }
    pub fn on_result(
        mut self,
        f: impl FnOnce(&mut App, &egui::Context, DialogResult) + 'static,
    ) -> Self {
        self.on_result = Some(Box::new(f));
        self
    }
}

// ---------------------------------------------------------------------------
// Integración con App::ui
// ---------------------------------------------------------------------------

fn is_keyboard_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}

/// Llamar al INICIO de `App::ui`, antes de dibujar las pantallas. Si hay algún
/// diálogo abierto (cola, `Modal` o formulario de bot) quita el foco una vez
/// al abrirse y aparta los eventos de teclado para que la app de atrás no los
/// vea; `render` se los devuelve al diálogo.
pub fn begin_frame(ctx: &egui::Context, app: &App) {
    let open = !app.dialogs.is_empty() || app.modal.is_some() || app.component_modal.is_some();
    let was_open = ctx
        .data_mut(|d| d.get_temp::<bool>(Id::new(WAS_OPEN_KEY)))
        .unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(Id::new(WAS_OPEN_KEY), open));

    if !open {
        ctx.data_mut(|d| {
            d.remove_temp::<Vec<egui::Event>>(Id::new(STASH_KEY));
        });
        return;
    }
    if !was_open {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    let stashed: Vec<egui::Event> = ctx.input_mut(|i| {
        let mut kept = Vec::with_capacity(i.events.len());
        let mut taken = Vec::new();
        for event in i.events.drain(..) {
            if is_keyboard_event(&event) {
                taken.push(event);
            } else {
                kept.push(event);
            }
        }
        i.events = kept;
        taken
    });
    ctx.data_mut(|d| d.insert_temp(Id::new(STASH_KEY), stashed));
}

/// Le devuelve al diálogo los eventos de teclado que `begin_frame` apartó.
fn reinject_events(ctx: &egui::Context) {
    let stashed = ctx.data_mut(|d| d.remove_temp::<Vec<egui::Event>>(Id::new(STASH_KEY)));
    if let Some(events) = stashed {
        ctx.input_mut(|i| i.events.extend(events));
    }
}

/// Reinicia la animación de entrada para que el próximo diálogo con ese `id`
/// vuelva a aparecer con fundido.
pub fn reset_anim(ctx: &egui::Context, dialog_id: &str) {
    ctx.animate_bool_with_time(Id::new(("dialog_in", dialog_id)), false, 0.0);
}

/// Dibuja el diálogo al frente de la cola (si hay). Llamar al final de
/// `App::ui`, por encima de todo lo demás.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(mut dialog) = app.dialogs.pop_front() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let palette = app.palette;
    match render(&ctx, &palette, &mut dialog) {
        None => app.dialogs.push_front(dialog),
        Some(result) => {
            reset_anim(&ctx, &dialog.id);
            if let Some(callback) = dialog.on_result.take() {
                callback(app, &ctx, result);
            }
            // Por si quedó otro diálogo esperando en la cola.
            ctx.request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// Dibujado
// ---------------------------------------------------------------------------

/// Dibuja un diálogo y devuelve `Some(resultado)` el frame en que se cierra.
/// No toca la cola: sirve tanto para `show` como para los diálogos
/// "transitorios" que se arman cada frame (ver `ui::overlay`).
pub fn render(ctx: &egui::Context, palette: &Palette, d: &mut Dialog) -> Option<DialogResult> {
    reinject_events(ctx);
    if d.buttons.is_empty() {
        d.buttons.push(DialogButton {
            id: "ok".to_owned(),
            label: "Aceptar".to_owned(),
            style: ButtonStyle::Primary,
            validate: true,
        });
    }

    let screen = ctx.viewport_rect();
    let t = ctx.animate_bool_with_time(Id::new(("dialog_in", d.id.as_str())), true, 0.22);
    let eased = 1.0 - (1.0 - t).powi(3);
    let time = ctx.input(|i| i.time) as f32;

    let kind = d.kind;
    let accent = kind.color(palette);
    let dialog_id = d.id.clone();
    let title = d.title.clone();
    let subtitle = d.subtitle.clone();
    let error = d.error.clone();
    let buttons = d.buttons.clone();
    let width = d.width;
    let pulse = d.pulse;
    let closable = d.dismiss_on_escape || d.dismiss_on_scrim;
    let max_body_h = (screen.height() - 330.0).max(160.0);

    let mut clicked: Option<String> = None;
    let mut dismiss = d.dismiss_on_escape && ctx.input(|i| i.key_pressed(egui::Key::Escape));

    // Fondo oscuro teñido con el color del tono; se come los clics.
    let scrim = Area::new(Id::new(("dialog_scrim", dialog_id.as_str())))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_width(screen.width());
            ui.set_height(screen.height());
            ui.painter().rect_filled(
                screen,
                0.0,
                Color32::from_rgba_unmultiplied(
                    accent.r() / 10,
                    accent.g() / 10,
                    accent.b() / 10,
                    (200.0 * t) as u8,
                ),
            );
            ui.interact(screen, Id::new(("dialog_block", dialog_id.as_str())), Sense::click())
        });
    if d.dismiss_on_scrim && scrim.inner.clicked() {
        dismiss = true;
    }

    Area::new(Id::new(("dialog_card", dialog_id.as_str())))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, (1.0 - eased) * 18.0])
        .show(ctx, |ui| {
            ui.set_opacity(t);

            let card = Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, accent.gamma_multiply(0.55)))
                .corner_radius(CornerRadius::same(card_radius()))
                .inner_margin(Margin::same(CARD_MARGIN))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 16],
                    blur: 40,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    let inner_w = width - CARD_MARGIN as f32 * 2.0;
                    ui.set_width(inner_w);

                    if header(
                        ui,
                        palette,
                        kind,
                        accent,
                        &title,
                        subtitle.as_deref(),
                        pulse,
                        time,
                        closable,
                    ) {
                        dismiss = true;
                    }

                    if !d.blocks.is_empty() {
                        ui.add_space(16.0);
                        egui::ScrollArea::vertical()
                            .max_height(max_body_h)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                render_blocks(ui, palette, &mut d.blocks, &dialog_id, accent);
                            });
                    }

                    if let Some(message) = &error {
                        ui.add_space(12.0);
                        theme::text(ui, message, theme::regular(12.5), palette.danger);
                    }

                    ui.add_space(20.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        // `buttons` va de izquierda a derecha; este layout
                        // dibuja de derecha a izquierda.
                        for button in buttons.iter().rev() {
                            if styled_button(ui, palette, &button.label, button.style).clicked() {
                                clicked = Some(button.id.clone());
                            }
                        }
                    });
                });

            // Franja superior de color.
            let rect = card.response.rect;
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(rect.width(), 4.0)),
                CornerRadius { nw: card_radius(), ne: card_radius(), sw: 0, se: 0 },
                accent,
            );
        });

    // Resolver qué pasó.
    let mut closed_with: Option<Option<String>> = None;
    if dismiss {
        closed_with = Some(None);
    } else if let Some(button_id) = clicked {
        let validate = buttons
            .iter()
            .find(|b| b.id == button_id)
            .is_some_and(|b| b.validate);
        match validate.then(|| missing_required(&d.blocks)).flatten() {
            Some(label) => d.error = Some(format!("Falta completar: {label}")),
            None => closed_with = Some(Some(button_id)),
        }
    }

    match closed_with {
        Some(button) => Some(DialogResult { button, values: collect_values(&d.blocks) }),
        None => {
            if pulse {
                // Para animar el latido del icono.
                ctx.request_repaint_after(Duration::from_millis(40));
            }
            None
        }
    }
}

fn missing_required(blocks: &[Block]) -> Option<String> {
    blocks.iter().find_map(|block| match block {
        Block::Field(f) if f.required && f.value.trim().is_empty() => {
            Some(if f.label.is_empty() { f.id.clone() } else { f.label.clone() })
        }
        _ => None,
    })
}

fn collect_values(blocks: &[Block]) -> HashMap<String, String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Field(f) => Some((f.id.clone(), f.value.clone())),
            _ => None,
        })
        .collect()
}

/// Icono con halo (pulsante si `pulse`) + título, subtítulo y ✕ opcional.
/// Devuelve `true` si se tocó la ✕.
#[allow(clippy::too_many_arguments)]
fn header(
    ui: &mut egui::Ui,
    palette: &Palette,
    kind: DialogKind,
    accent: Color32,
    title: &str,
    subtitle: Option<&str>,
    pulse: bool,
    time: f32,
    closable: bool,
) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;

        let (badge, _) = ui.allocate_exact_size(Vec2::splat(60.0), Sense::hover());
        let wave = if pulse { 0.5 + 0.5 * (time * 2.4).sin() } else { 0.35 };
        let center = badge.center();
        ui.painter().circle_filled(
            center,
            25.0 + 4.0 * wave,
            accent.gamma_multiply(0.08 + 0.07 * wave),
        );
        ui.painter()
            .circle_filled(center, 21.0, accent.gamma_multiply(0.22));
        ui.painter()
            .circle_stroke(center, 21.0, Stroke::new(1.0, accent.gamma_multiply(0.6)));
        paint_kind_icon(ui, kind, badge, accent);

        ui.vertical(|ui| {
            theme::text(ui, title, theme::bold(19.0), palette.text);
            if let Some(subtitle) = subtitle {
                ui.add_space(2.0);
                theme::text(ui, subtitle, theme::regular(12.5), palette.dim);
            }
        });

        if closable {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if theme::icon_button(ui, Icon::X, 14.0, palette.dim, palette.text, "Cerrar")
                    .clicked()
                {
                    close = true;
                }
            });
        }
    });
    close
}

fn paint_kind_icon(ui: &mut egui::Ui, kind: DialogKind, rect: Rect, color: Color32) {
    let icon = match kind {
        DialogKind::Info => Icon::Info,
        DialogKind::Success => Icon::CircleCheck,
        DialogKind::Warning | DialogKind::Danger => Icon::CircleAlert,
        DialogKind::Sparkle => Icon::Sparkles,
        DialogKind::Question => {
            ui.painter()
                .text(rect.center(), Align2::CENTER_CENTER, "?", theme::bold(26.0), color);
            return;
        }
    };
    theme::paint_icon(ui, icon, rect, 24.0, color);
}

fn render_blocks(
    ui: &mut egui::Ui,
    palette: &Palette,
    blocks: &mut [Block],
    dialog_id: &str,
    accent: Color32,
) {
    let mut first_field = true;
    for (index, block) in blocks.iter_mut().enumerate() {
        if index > 0 {
            ui.add_space(12.0);
        }
        match block {
            Block::Text(text) => paragraph(ui, text, 13.5, palette.secondary),
            Block::Note { icon, text } => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    theme::icon(ui, *icon, 14.0, palette.dim);
                    paragraph(ui, text, 12.5, palette.dim);
                });
            }
            Block::Flag { name, text } => flag_row(ui, palette, accent, name, text),
            Block::Callout { kind, title, text } => callout(ui, palette, *kind, title, text),
            Block::Bullets(items) => bullets(ui, palette, accent, items),
            Block::Changelog(releases) => changelog(ui, palette, accent, releases),
            Block::Field(field) => form_field(ui, palette, dialog_id, field, &mut first_field),
            Block::AppBanner { name, avatar_url, request } => {
                app_banner(ui, palette, name, avatar_url.as_deref(), request)
            }
        }
    }
}

/// "La aplicación [foto] Nombre solicitó que llenes un formulario".
fn app_banner(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    avatar_url: Option<&str>,
    request: &str,
) {
    const RADIUS: f32 = 20.0;
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::radius() + 2))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(RADIUS * 2.0), Sense::hover());
                let initial = name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_owned());
                crate::ui::extra::avatar(
                    ui,
                    rect.center(),
                    RADIUS,
                    avatar_url,
                    palette.accent.gamma_multiply(0.55),
                    &initial,
                    palette,
                );
                ui.vertical(|ui| {
                    paragraph(ui, "La aplicación", 12.0, palette.dim);
                    theme::text(ui, name, theme::bold(15.0), palette.text);
                    paragraph(ui, request, 12.5, palette.secondary);
                });
            });
        });
}

fn paragraph(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    ui.add(
        egui::Label::new(RichText::new(text).font(theme::regular(size)).color(color)).wrap(),
    );
}

fn flag_row(ui: &mut egui::Ui, palette: &Palette, accent: Color32, name: &str, text: &str) {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::radius() + 2))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            chip(ui, name, accent, egui::FontId::monospace(12.0));
            ui.add_space(8.0);
            paragraph(ui, text, 12.5, palette.secondary);
        });
}

fn callout(ui: &mut egui::Ui, palette: &Palette, kind: DialogKind, title: &str, text: &str) {
    let color = kind.color(palette);
    Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(theme::radius() + 2))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                theme::text(ui, title, theme::semibold(13.0), color);
                ui.add_space(4.0);
            }
            paragraph(ui, text, 12.5, palette.secondary);
        });
}

fn bullets(ui: &mut egui::Ui, palette: &Palette, accent: Color32, items: &[String]) {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            ui.add_space(6.0);
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (dot, _) = ui.allocate_exact_size(Vec2::new(6.0, 18.0), Sense::hover());
            ui.painter()
                .circle_filled(egui::pos2(dot.center().x, dot.min.y + 9.0), 2.5, accent);
            paragraph(ui, item, 13.0, palette.secondary);
        });
    }
}

fn changelog(ui: &mut egui::Ui, palette: &Palette, accent: Color32, releases: &[Release]) {
    for (index, release) in releases.iter().enumerate() {
        if index > 0 {
            ui.add_space(14.0);
            let (line, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, palette.outline);
            ui.add_space(14.0);
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            chip(ui, &format!("v{}", release.version), accent, theme::semibold(12.0));
            if let Some(date) = &release.date {
                theme::text(ui, date, theme::regular(12.0), palette.dim);
            }
        });
        if let Some(title) = &release.title {
            ui.add_space(8.0);
            theme::text(ui, title, theme::bold(14.5), palette.text);
        }
        ui.add_space(8.0);

        for (i, entry) in release.entries.iter().enumerate() {
            if i > 0 {
                ui.add_space(6.0);
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                tag_pill(ui, palette, entry.tag);
                paragraph(ui, &entry.text, 13.0, palette.secondary);
            });
        }
    }
}

fn tag_pill(ui: &mut egui::Ui, palette: &Palette, tag: ChangeTag) {
    let (label, color) = match tag {
        ChangeTag::New => ("NUEVO", palette.accent),
        ChangeTag::Improved => ("MEJORA", INFO_BLUE),
        ChangeTag::Fixed => ("ARREGLO", palette.warning),
        ChangeTag::Removed => ("ELIMINADO", palette.danger),
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(74.0, 18.0), Sense::hover());
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, color.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::semibold(10.5),
        color,
    );
}

fn form_field(
    ui: &mut egui::Ui,
    palette: &Palette,
    dialog_id: &str,
    field: &mut FormField,
    first_field: &mut bool,
) {
    if !field.label.is_empty() {
        let label = if field.required {
            format!("{} *", field.label)
        } else {
            field.label.clone()
        };
        theme::text(ui, label, theme::semibold(12.0), palette.secondary);
        ui.add_space(4.0);
    }
    let edit = if field.multiline {
        egui::TextEdit::multiline(&mut field.value).desired_rows(field.rows)
    } else {
        egui::TextEdit::singleline(&mut field.value)
    };
    let response = ui.add(
        edit.password(field.password)
            .id(Id::new(("dialog_field", dialog_id, field.id.as_str())))
            .hint_text(field.placeholder.as_str())
            .char_limit(field.max_len)
            .font(theme::regular(13.5))
            .desired_width(f32::INFINITY),
    );
    // El primer campo arranca con el foco (si nada más lo tiene).
    if *first_field {
        *first_field = false;
        if !response.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
            response.request_focus();
        }
    }
}

/// Píldora con texto (nombre de variable, versión...).
fn chip(ui: &mut egui::Ui, label: &str, color: Color32, font: egui::FontId) {
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let size = galley.size() + Vec2::new(18.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, color.gamma_multiply(0.16));
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, color.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
}

fn styled_button(ui: &mut egui::Ui, palette: &Palette, label: &str, style: ButtonStyle) -> Response {
    match style {
        ButtonStyle::Primary => theme::pill_button(ui, palette, label, true),
        ButtonStyle::Secondary => theme::pill_button(ui, palette, label, false),
        ButtonStyle::Danger => {
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), theme::semibold(13.0), palette.on_accent);
            let size = galley.size() + Vec2::new(36.0, 16.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
            });
            if ui.is_rect_visible(rect) {
                let fill = if response.hovered() {
                    crate::ui::extra::blend(palette.danger, Color32::WHITE, 0.15)
                } else {
                    palette.danger
                };
                ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
                let pos = rect.center() - galley.size() / 2.0;
                ui.painter().galley(pos, galley, palette.on_accent);
            }
            theme::focus_ring(ui, &response);
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        }
    }
}
