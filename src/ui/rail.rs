use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::lib::guild_order::{SidebarEntry, SidebarFolder, build_sidebar_order};
use crate::lib::state::{App, Screen};
use crate::ui::extra;
use crate::ui::theme::{self, Icon};

/// Ancho fijo que ocupa este contenido dentro de `ui::nav`.
pub const WIDTH: f32 = 72.0;

/// Ancho del rail de la interfaz nueva (`content_modern`).
pub const MODERN_WIDTH: f32 = 76.0;

/// Mismo tamaño de "casillero" para un server suelto o una carpeta
/// colapsada — así la barra no salta de ancho/alineación entre entradas.
const SLOT: f32 = 48.0;

/// Contenido del rail de servidores (logo + servidores/carpetas + "añadir
/// servidor"). Antes era un `egui::Panel::left` propio con su propia barra
/// de usuario abajo; ahora se dibuja como una columna dentro del panel de
/// navegación combinado (`ui::nav`), que es quien pone la barra de usuario
/// abajo, ocupando el ancho de esta columna MÁS el panel de al lado (pedido
/// explícito: que la barra de usuario no se quede solo con el ancho del
/// panel de DMs/canales).
///
/// El orden de los íconos (y las carpetas que agrupan varios) no es el
/// orden en que `READY` mandó los guilds: sale de
/// `App::discord_settings.guild_folders`, el mismo blob que ordena la
/// barra en el cliente oficial (ver `lib::guild_order`). Sin settings
/// cargados todavía (cuenta recién logueada, o falló el pedido) se cae
/// de vuelta al orden de `READY`, sin carpetas.
pub fn content(app: &mut App, ui: &mut egui::Ui) {
    if theme::is_modern() {
        content_modern(app, ui);
        return;
    }
    let palette = app.palette;
    let is_dm_area = matches!(app.screen, Screen::Home | Screen::Dm(_));
    let selected_server = match app.screen {
        Screen::Server(i) => Some(i),
        _ => None,
    };

    let order = build_sidebar_order(
        &app.servers,
        app.discord_settings
            .as_ref()
            .and_then(|s| s.guild_folders.as_ref()),
    );

    Frame::new()
        .inner_margin(Margin::symmetric(0, 12))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::click());
                theme::logo(ui, rect.center(), 48.0, palette.accent, palette.on_accent);
                if is_dm_area {
                    selected_pill(ui, &palette, rect.center(), 48.0);
                }
                // Menciones sin leer en los DMs.
                crate::ui::notifications::paint_badge(
                    ui.painter(),
                    rect.right_bottom() - Vec2::new(6.0, 6.0),
                    app.dm_mentions_total(),
                    &palette,
                    rail_bg(&palette),
                );
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    app.go_home();
                }

                ui.add_space(8.0);
                divider(ui, &palette);
                ui.add_space(8.0);
            });

            // Lista de servidores con scroll. Antes todo esto se dibujaba
            // directo en la columna, sin límite de alto: con muchos
            // servidores el contenido se salía de `top_height`, estiraba
            // el panel y empujaba la barra de usuario fuera de pantalla.
            // El `ScrollArea` se limita al alto que queda bajo el logo
            // (`auto_shrink` en false = ocupa todo ese alto) y la barra
            // de scroll va oculta, como en el cliente oficial; la rueda
            // del mouse y el trackpad siguen funcionando.
            egui::ScrollArea::vertical()
                .id_salt("rail_scroll")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let mut clicked_server = None;
                        let mut toggled_folder = None;

                        for entry in &order {
                            match entry {
                                SidebarEntry::Guild(i) => {
                                    let i = *i;
                                    let server = &app.servers[i];
                                    let (rect, resp) =
                                        ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
                                    extra::avatar(
                                        ui,
                                        rect.center(),
                                        24.0,
                                        server.icon_url.as_deref(),
                                        server.icon_color,
                                        &server.icon_initial,
                                        &palette,
                                    );
                                    if selected_server == Some(i) {
                                        selected_pill(ui, &palette, rect.center(), SLOT);
                                    }
                                    crate::ui::notifications::paint_badge(
                                        ui.painter(),
                                        rect.right_bottom() - Vec2::new(6.0, 6.0),
                                        app.server_mentions(i),
                                        &palette,
                                        rail_bg(&palette),
                                    );
                                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        clicked_server = Some(i);
                                    }
                                    ui.add_space(8.0);
                                }
                                SidebarEntry::Folder(folder) => {
                                    let is_open = app.open_guild_folders.contains(&folder.id);
                                    let folder_selected = selected_server
                                        .is_some_and(|i| folder.guild_indices.contains(&i));

                                    if !is_open {
                                        let (rect, resp) =
                                            ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
                                        draw_collapsed_folder(ui, rect, folder, &app.servers, &palette);
                                        if folder_selected {
                                            selected_pill(ui, &palette, rect.center(), SLOT);
                                        }
                                        let folder_mentions: u32 = folder
                                            .guild_indices
                                            .iter()
                                            .map(|&gi| app.server_mentions(gi))
                                            .sum();
                                        crate::ui::notifications::paint_badge(
                                            ui.painter(),
                                            rect.right_bottom() - Vec2::new(6.0, 6.0),
                                            folder_mentions,
                                            &palette,
                                            rail_bg(&palette),
                                        );
                                        if resp
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text(folder_label(folder))
                                            .clicked()
                                        {
                                            toggled_folder = Some(folder.id);
                                        }
                                        ui.add_space(8.0);
                                    } else {
                                        let base_color = folder.color.unwrap_or(palette.accent);
                                        let folder_color = Color32::from_rgba_unmultiplied(
                                            base_color.r(),
                                            base_color.g(),
                                            base_color.b(),
                                            50,
                                        );
                                        for &i in &folder.guild_indices {
                                            let server = &app.servers[i];
                                            let (rect, resp) =
                                                ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
                                            // Fondo agrupador detrás de cada ícono
                                            // de la carpeta abierta (el mismo
                                            // color se ve "atravesar" los huecos
                                            // entre íconos, como en el cliente
                                            // real).
                                            ui.painter().rect_filled(
                                                Rect::from_center_size(
                                                    rect.center(),
                                                    Vec2::new(SLOT, SLOT + 8.0),
                                                ),
                                                CornerRadius::same(18),
                                                folder_color,
                                            );
                                            extra::avatar(
                                                ui,
                                                rect.center(),
                                                24.0,
                                                server.icon_url.as_deref(),
                                                server.icon_color,
                                                &server.icon_initial,
                                                &palette,
                                            );
                                            if selected_server == Some(i) {
                                                selected_pill(ui, &palette, rect.center(), SLOT);
                                            }
                                            crate::ui::notifications::paint_badge(
                                                ui.painter(),
                                                rect.right_bottom() - Vec2::new(6.0, 6.0),
                                                app.server_mentions(i),
                                                &palette,
                                                rail_bg(&palette),
                                            );
                                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                                            {
                                                clicked_server = Some(i);
                                            }
                                            ui.add_space(8.0);
                                        }

                                        // Barrita angosta al pie del grupo: click
                                        // para volver a plegar la carpeta.
                                        let (rect, resp) = ui.allocate_exact_size(
                                            Vec2::new(SLOT, 10.0),
                                            Sense::click(),
                                        );
                                        ui.painter().rect_filled(
                                            Rect::from_center_size(rect.center(), Vec2::new(24.0, 6.0)),
                                            CornerRadius::same(3),
                                            folder.color.unwrap_or(palette.dim),
                                        );
                                        if resp
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text(folder_label(folder))
                                            .clicked()
                                        {
                                            toggled_folder = Some(folder.id);
                                        }
                                        ui.add_space(8.0);
                                    }
                                }
                            }
                        }

                        if let Some(i) = clicked_server {
                            app.open_server(i);
                        }
                        if let Some(id) = toggled_folder {
                            if !app.open_guild_folders.insert(id) {
                                app.open_guild_folders.remove(&id);
                            }
                        }

                        // Todavía no hay flujo real de "crear/unirse a un
                        // servidor"; se muestra el popup modal de la demo (pedido
                        // de "popups" al estilo Fastpotify).
                        if theme::circle_button(
                            ui,
                            Icon::Plus,
                            48.0,
                            palette.surface,
                            palette.accent,
                            palette.text,
                            "Añadir servidor",
                        )
                        .clicked()
                        {
                            app.open_modal(
                                "Crear servidor",
                                "Crear o unirse a un servidor todavía no está disponible en esta demo.",
                            );
                        }

                        ui.add_space(8.0);
                    });
                });
        });
}

// ---------------------------------------------------------------------
// Interfaz nueva: tres píldoras (inicio, servidores, controles)
// ---------------------------------------------------------------------

/// Lado de los íconos cuadrados redondeados de las píldoras.
const M_ICON: f32 = 50.0;
/// Separación vertical entre íconos.
const M_STEP: f32 = 10.0;
/// Margen interno (arriba y abajo) de cada píldora.
const M_PAD: f32 = 14.0;
/// Radio de las esquinas de los íconos.
const M_CORNER: f32 = 16.0;

/// Alto de una píldora con `n` íconos.
fn pill_height(n: usize) -> f32 {
    M_PAD * 2.0 + M_ICON * n as f32 + M_STEP * n.saturating_sub(1) as f32
}

/// Fondo de una píldora del rail.
fn paint_pill(ui: &egui::Ui, rect: Rect, palette: &theme::Palette) {
    let radius = CornerRadius::same(theme::CARD_RADIUS + 4);
    ui.painter().rect_filled(rect, radius, palette.panel);
    ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, palette.outline), StrokeKind::Inside);
}

/// Barra blanca pegada al borde izquierdo de la píldora: marca el servidor
/// (o el inicio) elegido.
fn side_marker(ui: &egui::Ui, pill: Rect, center_y: f32, palette: &theme::Palette) {
    let rect = Rect::from_center_size(
        egui::pos2(pill.left() + 3.0, center_y),
        Vec2::new(4.0, M_ICON * 0.55),
    );
    ui.painter().rect_filled(rect, 2.0, palette.text);
}

/// Botón cuadrado redondeado con un ícono de `theme::Icon` en el medio.
fn square_button(
    ui: &mut egui::Ui,
    id: &str,
    center: egui::Pos2,
    fill: Color32,
    fill_hover: Color32,
    icon: Icon,
    icon_color: Color32,
    tooltip: &str,
) -> egui::Response {
    let rect = Rect::from_center_size(center, Vec2::splat(M_ICON));
    let resp = ui.interact(rect, ui.id().with(("rail_square", id)), Sense::click());
    let bg = if resp.hovered() { fill_hover } else { fill };
    ui.painter().rect_filled(rect, CornerRadius::same(M_CORNER as u8), bg);
    theme::paint_icon(ui, icon, rect, 22.0, icon_color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
}

/// Contenido del rail en la interfaz nueva: tres píldoras flotantes. Arriba
/// el inicio (mensajes directos), explorar y añadir; en el medio los
/// servidores y carpetas (con scroll); abajo silenciar, ensordecer y ajustes.
pub fn content_modern(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let gap = theme::GAP as f32;
    let full = ui.available_rect_before_wrap();
    let width = MODERN_WIDTH;

    // Píldora de arriba plegable: en reposo solo se ve el botón de mensajes
    // directos; al pasar el cursor por encima se despliegan explorar y añadir
    // (animado). `expand` va de 0.0 (plegada) a 1.0 (desplegada).
    let collapsed_h = pill_height(1);
    let full_h = pill_height(3);
    let expand = {
        let ctx = ui.ctx().clone();
        let id = egui::Id::new("rail_top_expand");
        let was: bool = ctx.data(|d| d.get_temp(id)).unwrap_or(false);
        let test_h = if was { full_h } else { collapsed_h };
        let test_rect = Rect::from_min_size(full.min, Vec2::new(width, test_h));
        let hover = ctx.pointer_hover_pos().is_some_and(|p| test_rect.contains(p));
        ctx.data_mut(|d| d.insert_temp(id, hover));
        egui::emath::easing::cubic_out(ctx.animate_bool_with_time(id.with("t"), hover, 0.22))
    };
    let top_h = collapsed_h + (full_h - collapsed_h) * expand;
    let bottom_h = pill_height(3);
    let top_rect = Rect::from_min_size(full.min, Vec2::new(width, top_h));
    let bottom_rect = Rect::from_min_size(
        egui::pos2(full.min.x, full.max.y - bottom_h),
        Vec2::new(width, bottom_h),
    );
    let mid_top = top_rect.max.y + gap;
    let mid_bottom = (bottom_rect.min.y - gap).max(mid_top + 80.0);
    let mid_rect = Rect::from_min_max(
        egui::pos2(full.min.x, mid_top),
        egui::pos2(full.min.x + width, mid_bottom),
    );

    paint_pill(ui, top_rect, &palette);
    paint_pill(ui, mid_rect, &palette);
    paint_pill(ui, bottom_rect, &palette);

    let is_dm_area = matches!(app.screen, Screen::Home | Screen::Dm(_));
    let selected_server = match app.screen {
        Screen::Server(i) => Some(i),
        _ => None,
    };

    // --- Píldora de arriba: inicio, explorar, añadir ---------------------
    {
        let cx = top_rect.center().x;
        let y0 = top_rect.min.y + M_PAD + M_ICON / 2.0;
        let y1 = y0 + M_ICON + M_STEP;
        let y2 = y1 + M_ICON + M_STEP;

        let (home_fill, home_hover, home_icon) = if is_dm_area {
            (palette.accent, palette.accent_hover, palette.on_accent)
        } else {
            (palette.surface, palette.surface_hover, palette.text)
        };
        // Explorar y añadir salen desde detrás del botón de inicio (se dibujan
        // antes para que quede por encima) y aparecen con el desplegado.
        if expand > 0.02 {
            let fade = expand;
            let ready = expand > 0.6;
            if square_button(
                ui,
                "explore",
                egui::pos2(cx, y0 + (y1 - y0) * expand),
                palette.surface.gamma_multiply(fade),
                palette.surface_hover.gamma_multiply(fade),
                Icon::Compass,
                palette.secondary.gamma_multiply(fade),
                "Explorar servidores",
            )
            .clicked()
                && ready
            {
                app.open_modal(
                    "Explorar servidores",
                    "Explorar servidores todavía no está disponible en esta demo.",
                );
            }

            if square_button(
                ui,
                "add",
                egui::pos2(cx, y0 + (y2 - y0) * expand),
                palette.surface.gamma_multiply(fade),
                palette.surface_hover.gamma_multiply(fade),
                Icon::Plus,
                palette.accent.gamma_multiply(fade),
                "Añadir servidor",
            )
            .clicked()
                && ready
            {
                app.open_modal(
                    "Crear servidor",
                    "Crear o unirse a un servidor todavía no está disponible en esta demo.",
                );
            }
        }

        let home_center = egui::pos2(cx, y0);
        if is_dm_area {
            side_marker(ui, top_rect, y0, &palette);
        }
        let home = square_button(
            ui,
            "home",
            home_center,
            home_fill,
            home_hover,
            Icon::MessageCircle,
            home_icon,
            "Mensajes directos",
        );
        crate::ui::notifications::paint_badge(
            ui.painter(),
            home_center + Vec2::new(M_ICON / 2.0 - 4.0, M_ICON / 2.0 - 4.0),
            app.dm_mentions_total(),
            &palette,
            palette.panel,
        );
        if home.clicked() {
            app.go_home();
        }
    }

    // --- Píldora de abajo: silenciar, ensordecer, ajustes ----------------
    {
        let self_mute = app.voice_target.as_ref().is_some_and(|t| t.self_mute);
        let self_deaf = app.voice_target.as_ref().is_some_and(|t| t.self_deaf);
        let cx = bottom_rect.center().x;
        let y0 = bottom_rect.min.y + M_PAD + M_ICON / 2.0;
        let y1 = y0 + M_ICON + M_STEP;
        let y2 = y1 + M_ICON + M_STEP;
        let danger_fill = extra::blend(palette.panel, palette.danger, 0.35);
        let danger_hover = extra::blend(palette.panel, palette.danger, 0.5);

        let (mute_icon, mute_fill, mute_hover, mute_color) = if self_mute {
            (Icon::MicOff, danger_fill, danger_hover, palette.danger)
        } else {
            (Icon::Mic, palette.surface, palette.surface_hover, palette.secondary)
        };
        if square_button(
            ui,
            "mute",
            egui::pos2(cx, y0),
            mute_fill,
            mute_hover,
            mute_icon,
            mute_color,
            if self_mute { "Dejar de silenciar" } else { "Silenciar" },
        )
        .clicked()
        {
            app.toggle_self_mute();
        }

        let (deaf_icon, deaf_fill, deaf_hover, deaf_color) = if self_deaf {
            (Icon::VolumeX, danger_fill, danger_hover, palette.danger)
        } else {
            (Icon::Headphones, palette.surface, palette.surface_hover, palette.secondary)
        };
        if square_button(
            ui,
            "deafen",
            egui::pos2(cx, y1),
            deaf_fill,
            deaf_hover,
            deaf_icon,
            deaf_color,
            if self_deaf { "Dejar de ensordecer" } else { "Ensordecer" },
        )
        .clicked()
        {
            app.toggle_self_deafen();
        }

        if square_button(
            ui,
            "settings",
            egui::pos2(cx, y2),
            palette.surface,
            palette.surface_hover,
            Icon::Settings,
            palette.secondary,
            "Ajustes",
        )
        .clicked()
        {
            app.settings_open = true;
        }
    }

    // --- Píldora del medio: servidores y carpetas ------------------------
    let order = build_sidebar_order(
        &app.servers,
        app.discord_settings
            .as_ref()
            .and_then(|s| s.guild_folders.as_ref()),
    );
    let inner = Rect::from_min_max(
        mid_rect.min + Vec2::new(0.0, 8.0),
        mid_rect.max - Vec2::new(0.0, 8.0),
    );
    let mid_clip = mid_rect.intersect(ui.clip_rect());
    ui.scope_builder(
        UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Center)),
        |ui| {
            ui.set_clip_rect(mid_clip);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::ScrollArea::vertical()
                .id_salt("rail_scroll_modern")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        let mut clicked_server = None;
                        let mut toggled_folder = None;

                        for entry in &order {
                            match entry {
                                SidebarEntry::Guild(i) => {
                                    let i = *i;
                                    let server = &app.servers[i];
                                    let (rect, resp) =
                                        ui.allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
                                    if selected_server == Some(i) {
                                        side_marker(ui, mid_rect, rect.center().y, &palette);
                                    }
                                    extra::avatar_rounded(
                                        ui,
                                        rect.center(),
                                        M_ICON / 2.0,
                                        M_CORNER,
                                        server.icon_url.as_deref(),
                                        server.icon_color,
                                        &server.icon_initial,
                                        &palette,
                                    );
                                    crate::ui::notifications::paint_badge(
                                        ui.painter(),
                                        rect.right_bottom() - Vec2::new(4.0, 4.0),
                                        app.server_mentions(i),
                                        &palette,
                                        palette.panel,
                                    );
                                    if resp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(&server.name)
                                        .clicked()
                                    {
                                        clicked_server = Some(i);
                                    }
                                    ui.add_space(M_STEP);
                                }
                                SidebarEntry::Folder(folder) => {
                                    let is_open = app.open_guild_folders.contains(&folder.id);
                                    let folder_selected = selected_server
                                        .is_some_and(|i| folder.guild_indices.contains(&i));
                                    if !is_open {
                                        let (rect, resp) =
                                            ui.allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
                                        if folder_selected {
                                            side_marker(ui, mid_rect, rect.center().y, &palette);
                                        }
                                        draw_collapsed_folder_modern(
                                            ui,
                                            rect,
                                            folder,
                                            &app.servers,
                                            &palette,
                                        );
                                        let folder_mentions: u32 = folder
                                            .guild_indices
                                            .iter()
                                            .map(|&gi| app.server_mentions(gi))
                                            .sum();
                                        crate::ui::notifications::paint_badge(
                                            ui.painter(),
                                            rect.right_bottom() - Vec2::new(4.0, 4.0),
                                            folder_mentions,
                                            &palette,
                                            palette.panel,
                                        );
                                        if resp
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text(folder_label(folder))
                                            .clicked()
                                        {
                                            toggled_folder = Some(folder.id);
                                        }
                                        ui.add_space(M_STEP);
                                    } else {
                                        let base_color = folder.color.unwrap_or(palette.accent);
                                        let tint = extra::blend(palette.panel, base_color, 0.35);
                                        for &i in &folder.guild_indices {
                                            let server = &app.servers[i];
                                            let (rect, resp) = ui
                                                .allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
                                            ui.painter().rect_filled(
                                                Rect::from_center_size(
                                                    rect.center(),
                                                    Vec2::new(M_ICON + 6.0, M_ICON + M_STEP),
                                                ),
                                                CornerRadius::same(M_CORNER as u8 + 2),
                                                tint,
                                            );
                                            if selected_server == Some(i) {
                                                side_marker(ui, mid_rect, rect.center().y, &palette);
                                            }
                                            extra::avatar_rounded(
                                                ui,
                                                rect.center(),
                                                M_ICON / 2.0,
                                                M_CORNER,
                                                server.icon_url.as_deref(),
                                                server.icon_color,
                                                &server.icon_initial,
                                                &palette,
                                            );
                                            crate::ui::notifications::paint_badge(
                                                ui.painter(),
                                                rect.right_bottom() - Vec2::new(4.0, 4.0),
                                                app.server_mentions(i),
                                                &palette,
                                                palette.panel,
                                            );
                                            if resp
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .on_hover_text(&server.name)
                                                .clicked()
                                            {
                                                clicked_server = Some(i);
                                            }
                                            ui.add_space(M_STEP);
                                        }
                                        // Barrita al pie del grupo: click para plegarlo.
                                        let (rect, resp) = ui.allocate_exact_size(
                                            Vec2::new(M_ICON, 10.0),
                                            Sense::click(),
                                        );
                                        ui.painter().rect_filled(
                                            Rect::from_center_size(rect.center(), Vec2::new(24.0, 6.0)),
                                            CornerRadius::same(3),
                                            folder.color.unwrap_or(palette.dim),
                                        );
                                        if resp
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text(folder_label(folder))
                                            .clicked()
                                        {
                                            toggled_folder = Some(folder.id);
                                        }
                                        ui.add_space(M_STEP);
                                    }
                                }
                            }
                        }

                        if let Some(i) = clicked_server {
                            app.open_server(i);
                        }
                        if let Some(id) = toggled_folder {
                            if !app.open_guild_folders.insert(id) {
                                app.open_guild_folders.remove(&id);
                            }
                        }
                        ui.add_space(6.0);
                    });
                });
        },
    );
}

/// Carpeta colapsada de la interfaz nueva: cuadrado redondeado teñido con el
/// color de la carpeta y una grilla 2x2 de círculos (inicial/color de hasta 4
/// servidores). Como en la versión clásica, no carga las imágenes reales.
fn draw_collapsed_folder_modern(
    ui: &mut egui::Ui,
    rect: Rect,
    folder: &SidebarFolder,
    servers: &[crate::lib::data::Server],
    palette: &theme::Palette,
) {
    let base = folder.color.unwrap_or(palette.surface_active);
    let bg = extra::blend(palette.panel, base, 0.55);
    ui.painter().rect_filled(rect, CornerRadius::same(M_CORNER as u8), bg);

    let cell = 18.0_f32;
    let gap = 4.0_f32;
    let origin = rect.center() - Vec2::new(cell + gap / 2.0, cell + gap / 2.0);
    let offsets = [
        Vec2::new(0.0, 0.0),
        Vec2::new(cell + gap, 0.0),
        Vec2::new(0.0, cell + gap),
        Vec2::new(cell + gap, cell + gap),
    ];
    for (slot, &guild_index) in folder.guild_indices.iter().take(4).enumerate() {
        let server = &servers[guild_index];
        let center = origin + offsets[slot] + Vec2::splat(cell / 2.0);
        ui.painter().circle_filled(center, cell / 2.0, server.icon_color);
        let initial = server.icon_initial.chars().next().unwrap_or('#').to_string();
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            initial,
            theme::semibold(cell * 0.55),
            contrast_text(server.icon_color),
        );
    }
}

/// Nombre para el tooltip de una carpeta. Las carpetas reales pueden no
/// tener nombre puesto (Discord las deja sin nombrar) — en ese caso el
/// cliente oficial muestra la cantidad de servers adentro, así que
/// hacemos lo mismo.
fn folder_label(folder: &SidebarFolder) -> String {
    match &folder.name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => format!("Carpeta ({} servers)", folder.guild_indices.len()),
    }
}

/// Ícono colapsado de una carpeta: una grilla 2x2 de mini-cuadraditos con
/// el color/inicial de hasta 4 de los servers que contiene, sobre un
/// fondo redondeado con el color de la carpeta (o `palette.surface_active`
/// si no tiene uno puesto). Deliberadamente NO carga las imágenes reales
/// de los íconos acá (sería cargar 4 texturas por carpeta colapsada nada
/// más que para verlas en miniatura); usa el mismo círculo-con-inicial
/// que `extra::avatar_circle` dibuja como placeholder en todos lados.
fn draw_collapsed_folder(
    ui: &mut egui::Ui,
    rect: Rect,
    folder: &SidebarFolder,
    servers: &[crate::lib::data::Server],
    palette: &theme::Palette,
) {
    let bg = folder.color.unwrap_or(palette.surface_active);
    ui.painter().rect_filled(rect, CornerRadius::same(18), bg);

    let cell = 16.0_f32;
    let gap = 3.0_f32;
    let origin = rect.center() - Vec2::new(cell + gap / 2.0, cell + gap / 2.0);
    let offsets = [
        Vec2::new(0.0, 0.0),
        Vec2::new(cell + gap, 0.0),
        Vec2::new(0.0, cell + gap),
        Vec2::new(cell + gap, cell + gap),
    ];

    for (slot, &guild_index) in folder.guild_indices.iter().take(4).enumerate() {
        let server = &servers[guild_index];
        let center = origin + offsets[slot] + Vec2::splat(cell / 2.0);
        ui.painter().rect_filled(
            Rect::from_center_size(center, Vec2::splat(cell)),
            CornerRadius::same(5),
            server.icon_color,
        );
        let initial = server.icon_initial.chars().next().unwrap_or('#').to_string();
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            initial,
            theme::semibold(cell * 0.55),
            contrast_text(server.icon_color),
        );
    }
}

/// Texto blanco o casi negro según qué tan clara sea `bg`, para que la
/// inicial de la mini-celda de la carpeta se vea sin importar el color
/// del ícono del server (mismo criterio simple de luminancia que se usa
/// para elegir texto sobre un color de acento).
fn contrast_text(bg: Color32) -> Color32 {
    let luminance = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if luminance > 150.0 {
        Color32::from_black_alpha(230)
    } else {
        Color32::WHITE
    }
}

/// Barrita a la izquierda del ícono, como el indicador de
/// "servidor/DM seleccionado" de Discord.
/// Color de fondo sobre el que flota el rail: el de la ventana en la
/// interfaz nueva (sin tarjeta propia) y el del panel en la clásica.
fn rail_bg(palette: &theme::Palette) -> Color32 {
    if theme::is_modern() { palette.window } else { palette.panel }
}

fn selected_pill(ui: &mut egui::Ui, palette: &theme::Palette, icon_center: egui::Pos2, icon_size: f32) {
    let x = icon_center.x - icon_size / 2.0 - 10.0;
    let rect = egui::Rect::from_center_size(
        egui::pos2(x, icon_center.y),
        Vec2::new(4.0, icon_size * 0.5),
    );
    ui.painter().rect_filled(rect, 2.0, palette.accent);
    // Interfaz nueva: además un aro suave alrededor del ícono seleccionado.
    if theme::is_modern() {
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(icon_center, Vec2::splat(icon_size + 6.0)),
            CornerRadius::same(18),
            egui::Stroke::new(2.0, palette.accent),
            egui::StrokeKind::Outside,
        );
    }
}

fn divider(ui: &mut egui::Ui, palette: &theme::Palette) {
    let w = ui.available_width().min(32.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), egui::Sense::hover());
    let center = rect.center();
    ui.painter().rect_filled(
        egui::Rect::from_center_size(center, Vec2::new(w, 2.0)),
        1.0,
        palette.outline,
    );
}
