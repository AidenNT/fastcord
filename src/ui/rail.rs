use egui::{Color32, CornerRadius, Frame, Margin, Rect, Sense, Vec2};

use crate::lib::guild_order::{SidebarEntry, SidebarFolder, build_sidebar_order};
use crate::lib::state::{App, Screen};
use crate::ui::extra;
use crate::ui::theme::{self, Icon};

/// Ancho fijo que ocupa este contenido dentro de `ui::nav`.
pub const WIDTH: f32 = 72.0;

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
                    palette.panel,
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
                                        palette.panel,
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
                                            palette.panel,
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
                                                CornerRadius::same(16),
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
                                                palette.panel,
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
    ui.painter().rect_filled(rect, CornerRadius::same(16), bg);

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
fn selected_pill(ui: &mut egui::Ui, palette: &theme::Palette, icon_center: egui::Pos2, icon_size: f32) {
    let x = icon_center.x - icon_size / 2.0 - 10.0;
    let rect = egui::Rect::from_center_size(
        egui::pos2(x, icon_center.y),
        Vec2::new(4.0, icon_size * 0.6),
    );
    ui.painter().rect_filled(rect, 2.0, palette.text);
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
