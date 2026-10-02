//! Selector de cuentas: lista de las cuentas guardadas (ver
//! `lib::accounts`) para elegir con cuál entrar, avisando de las que tienen la
//! sesión cerrada o expirada.
//!
//! Se usa en dos lados:
//! * `picker`: en la pantalla de login (`ui::login`), cuando no hay ninguna
//!   cuenta abierta (arranque sin sesión válida, sesión caída, "Elegir otra
//!   cuenta", "Agregar cuenta").
//! * `settings_section`: en Ajustes → Cuenta, para cambiar de cuenta, agregar
//!   otra o cerrar la sesión de la actual.

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::lib::accounts::SavedAccount;
use crate::lib::state::App;
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

const ROW_HEIGHT: f32 = 56.0;
/// Alto que ocupa una fila más el espacio entre filas (para dimensionar el
/// selector del login).
pub const ROW_STRIDE: f32 = ROW_HEIGHT + 8.0;
const AVATAR_RADIUS: f32 = 18.0;

/// Qué pidió la persona en una fila.
pub enum RowAction {
    Select(String),
    Remove(String),
}

/// Lo que se puede hacer desde Ajustes → Cuenta.
pub enum AccountAction {
    /// Cambiar a otra cuenta guardada (sin cerrar la sesión de la actual).
    Switch(String),
    /// Volver al selector para agregar otra cuenta.
    Add,
    /// Cerrar la sesión de la cuenta abierta.
    LogOut,
    /// Quitar una cuenta de la lista.
    Remove(String),
}

/// Una fila de cuenta: avatar, nombre, estado y (opcional) "Quitar".
pub fn account_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    index: usize,
    account: &SavedAccount,
    is_current: bool,
    show_remove: bool,
) -> Option<RowAction> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
    let mut action = None;

    let fill = if is_current {
        extra::blend(palette.surface, palette.accent, 0.14)
    } else if response.hovered() {
        palette.surface_hover
    } else {
        palette.surface
    };
    let stroke_color = if is_current { palette.accent } else { palette.outline };
    let radius = CornerRadius::same(12);
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, stroke_color), StrokeKind::Inside);

    // Avatar (o inicial sobre un círculo de color si no tiene).
    let center = egui::pos2(rect.min.x + 14.0 + AVATAR_RADIUS, rect.center().y);
    let initial: String = account
        .label()
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned());
    extra::avatar(
        ui,
        center,
        AVATAR_RADIUS,
        account.avatar_url.as_deref(),
        extra::blend(palette.surface, palette.accent, 0.45),
        &initial,
        palette,
    );

    // Nombre y estado. El texto se recorta antes de la zona de "Quitar".
    let text_x = center.x + AVATAR_RADIUS + 12.0;
    let text_clip = Rect::from_min_max(rect.min, egui::pos2(rect.right() - 80.0, rect.bottom()));
    let painter = ui.painter().with_clip_rect(text_clip);
    painter.text(
        egui::pos2(text_x, rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        account.label(),
        theme::semibold(14.0),
        palette.text,
    );
    let (status, status_color) = if is_current {
        ("Sesión activa", palette.accent)
    } else if account.needs_login() {
        ("Sesión cerrada o expirada · iniciá sesión de nuevo", palette.warning)
    } else {
        ("Sesión guardada", palette.secondary)
    };
    painter.text(
        egui::pos2(text_x, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        status,
        theme::regular(12.0),
        status_color,
    );

    // "Quitar": se dibuja y se registra DESPUÉS de la fila, así que un clic
    // ahí lo recibe este botón y no la fila.
    if show_remove {
        let galley = ui
            .painter()
            .layout_no_wrap("Quitar".to_owned(), theme::medium(12.5), palette.dim);
        let size = galley.size();
        let remove_rect = Rect::from_min_size(
            egui::pos2(rect.right() - size.x - 24.0, rect.center().y - 14.0),
            Vec2::new(size.x + 16.0, 28.0),
        );
        let remove = ui.interact(
            remove_rect,
            ui.id().with(("account_remove", index)),
            Sense::click(),
        );
        let color = if remove.hovered() { palette.danger } else { palette.dim };
        ui.painter().galley(
            egui::pos2(remove_rect.min.x + 8.0, remove_rect.center().y - size.y / 2.0),
            galley,
            color,
        );
        if remove.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Some(RowAction::Remove(account.user_id.clone()));
        }
    }

    if action.is_none()
        && !is_current
        && response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    {
        action = Some(RowAction::Select(account.user_id.clone()));
    }
    action
}

/// Lista de cuentas de la pantalla de login. No dibuja nada si no hay cuentas.
/// `max_height` limita el alto de la lista (después hace scroll).
pub fn picker(app: &mut App, ui: &mut egui::Ui, palette: &Palette, max_height: f32) {
    if app.accounts.is_empty() {
        return;
    }
    theme::text(ui, "Elegí una cuenta", theme::semibold(13.5), palette.text);
    ui.add_space(8.0);

    let accounts = app.accounts.accounts().to_vec();
    let mut action = None;
    egui::ScrollArea::vertical()
        .max_height(max_height.max(ROW_HEIGHT))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for (index, account) in accounts.iter().enumerate() {
                if let Some(taken) = account_row(ui, palette, index, account, false, true) {
                    action = Some(taken);
                }
            }
        });

    match action {
        Some(RowAction::Select(id)) => app.sign_in_saved_account(&id),
        Some(RowAction::Remove(id)) => app.ask_remove_account(&id),
        None => {}
    }
    ui.add_space(12.0);
}

/// Sección "Cuentas" de Ajustes → Cuenta. `current_id` es la cuenta abierta
/// (si hay una).
pub fn settings_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    accounts: &[SavedAccount],
    current_id: Option<&str>,
) -> Option<AccountAction> {
    let mut action = None;
    theme::section_title(ui, palette, "Cuentas");
    ui.add_space(4.0);
    theme::subtle(
        ui,
        palette,
        "Cambiá de cuenta sin cerrar sesión, agregá otra o cerrá la sesión de la actual.",
    );
    ui.add_space(10.0);

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        for (index, account) in accounts.iter().enumerate() {
            let is_current = current_id.is_some_and(|id| id == account.user_id);
            match account_row(ui, palette, index, account, is_current, !is_current) {
                Some(RowAction::Select(id)) => action = Some(AccountAction::Switch(id)),
                Some(RowAction::Remove(id)) => action = Some(AccountAction::Remove(id)),
                None => {}
            }
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if theme::soft_button(ui, palette, Some(Icon::CirclePlus), "Agregar cuenta", false)
                .clicked()
            {
                action = Some(AccountAction::Add);
            }
            if current_id.is_some()
                && theme::soft_button(ui, palette, Some(Icon::LogOut), "Cerrar sesión", false)
                    .clicked()
            {
                action = Some(AccountAction::LogOut);
            }
        });
    });
    action
}
