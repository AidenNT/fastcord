use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::lib::guild_order::{SidebarEntry, SidebarFolder, build_sidebar_order};
use crate::lib::state::{App, Screen};
use crate::ui::shell::extra;
use crate::ui::theme::{self, Icon};

/// Ancho fijo que ocupa este contenido dentro de `ui::shell::nav`.
pub const WIDTH: f32 = 72.0;

/// Ancho del rail de la interfaz nueva (`content_modern`).
pub const MODERN_WIDTH: f32 = 60.0;

/// Mismo tamaño de "casillero" para un server suelto o una carpeta
/// colapsada — así la barra no salta de ancho/alineación entre entradas.
const SLOT: f32 = 48.0;

/// Contenido del rail de servidores (logo + servidores/carpetas + "añadir
/// servidor"). Antes era un `egui::Panel::left` propio con su propia barra
/// de usuario abajo; ahora se dibuja como una columna dentro del panel de
/// navegación combinado (`ui::shell::nav`), que es quien pone la barra de usuario
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
        app.discord_settings.as_ref().and_then(|s| s.guild_folders.as_ref()),
    );

    Frame::new().inner_margin(Margin::symmetric(0, 12)).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::click());
            theme::logo(ui, rect.center(), 48.0, palette.accent, palette.on_accent);
            if is_dm_area {
                selected_pill(ui, &palette, rect.center(), 48.0);
            }
            // Menciones sin leer en los DMs.
            crate::ui::shell::notifications::paint_badge(
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
                    let mut want_members: Option<usize> = None;
                    let mut toggled_folder = None;
                    join_requests_button(&*app, ui, SLOT, 24, rail_bg(&palette));

                    for entry in &order {
                        match entry {
                            SidebarEntry::Guild(i) => {
                                let i = *i;
                                let server = &app.servers[i];
                                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
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
                                crate::ui::shell::notifications::paint_badge(
                                    ui.painter(),
                                    rect.right_bottom() - Vec2::new(6.0, 6.0),
                                    app.server_mentions(i),
                                    &palette,
                                    rail_bg(&palette),
                                );
                                if server_hover(&*app, i, resp.on_hover_cursor(egui::CursorIcon::PointingHand), &palette, &mut want_members)
                                    .clicked()
                                {
                                    clicked_server = Some(i);
                                }
                                ui.add_space(8.0);
                            }
                            SidebarEntry::Folder(folder) => {
                                let is_open = app.open_guild_folders.contains(&folder.id);
                                let folder_selected =
                                    selected_server.is_some_and(|i| folder.guild_indices.contains(&i));

                                if !is_open {
                                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
                                    draw_collapsed_folder(ui, rect, folder, &app.servers, &palette);
                                    if folder_selected {
                                        selected_pill(ui, &palette, rect.center(), SLOT);
                                    }
                                    let folder_mentions: u32 =
                                        folder.guild_indices.iter().map(|&gi| app.server_mentions(gi)).sum();
                                    crate::ui::shell::notifications::paint_badge(
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
                                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
                                        // Fondo agrupador detrás de cada ícono
                                        // de la carpeta abierta (el mismo
                                        // color se ve "atravesar" los huecos
                                        // entre íconos, como en el cliente
                                        // real).
                                        ui.painter().rect_filled(
                                            Rect::from_center_size(rect.center(), Vec2::new(SLOT, SLOT + 8.0)),
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
                                        crate::ui::shell::notifications::paint_badge(
                                            ui.painter(),
                                            rect.right_bottom() - Vec2::new(6.0, 6.0),
                                            app.server_mentions(i),
                                            &palette,
                                            rail_bg(&palette),
                                        );
                                        if server_hover(&*app, i, resp.on_hover_cursor(egui::CursorIcon::PointingHand), &palette, &mut want_members)
                                            .clicked()
                                        {
                                            clicked_server = Some(i);
                                        }
                                        ui.add_space(8.0);
                                    }

                                    // Barrita angosta al pie del grupo: click
                                    // para volver a plegar la carpeta.
                                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(SLOT, 10.0), Sense::click());
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

                    if let Some(i) = want_members {
                        app.on_server_hover(i);
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
const M_ICON: f32 = 40.0;
/// Separación vertical entre íconos.
const M_STEP: f32 = 6.0;
/// Margen interno (arriba y abajo) de cada píldora.
const M_PAD: f32 = 10.0;
/// Radio de las esquinas de los íconos.
const M_CORNER: f32 = 12.0;

/// Alto de una píldora con `n` íconos.
fn pill_height(n: usize) -> f32 {
    M_PAD * 2.0 + M_ICON * n as f32 + M_STEP * n.saturating_sub(1) as f32
}

/// Fondo de una píldora del rail.
fn paint_pill(ui: &egui::Ui, rect: Rect, palette: &theme::Palette) {
    let radius = CornerRadius::same(theme::CARD_RADIUS + 2);
    ui.painter().rect_filled(rect, radius, palette.panel);
    ui.painter()
        .rect_stroke(rect, radius, theme::card_stroke(palette), StrokeKind::Inside);
}

/// Barra blanca pegada al borde izquierdo de la píldora: marca el servidor
/// (o el inicio) elegido.
fn side_marker(ui: &egui::Ui, pill: Rect, center_y: f32, palette: &theme::Palette) {
    let rect = Rect::from_center_size(egui::pos2(pill.left() + 3.0, center_y), Vec2::new(4.0, M_ICON * 0.55));
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
    theme::paint_icon(ui, icon, rect, 20.0, icon_color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Tutorial de una sola vez: explica adónde fue el botón de colgar al ocultar la
/// barra de llamada. Se dibuja pegado a la derecha de la píldora de abajo.
fn call_hidden_tip(app: &mut App, ctx: &egui::Context, pill: Rect, t: f32) {
    let palette = app.palette;
    let mut dismiss = false;
    egui::Area::new(egui::Id::new("call_bar_hidden_tip"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(pill.right() + 12.0 - 10.0 * (1.0 - t), pill.bottom()))
        .show(ctx, |ui| {
            ui.set_opacity(t);
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.accent))
                .corner_radius(CornerRadius::same(theme::radius() + 4))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(250.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    theme::text(ui, "Barra de llamada oculta", theme::semibold(13.0), palette.text);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "El botón de colgar ahora está junto a Ajustes. Pasa el cursor por esta \
                                 píldora para desplegarla; con la flecha de arriba vuelves a mostrar la barra.",
                            )
                            .font(theme::regular(12.0))
                            .color(palette.dim),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    let button = egui::Button::new(
                        egui::RichText::new("Entendido").font(theme::semibold(12.0)).color(palette.on_accent),
                    )
                    .fill(palette.accent);
                    if ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        dismiss = true;
                    }
                });
        });
    if dismiss {
        app.call_bar_tip_open = false;
        app.call_bar_tip_seen = true;
        let _ = web_local_storage_api::set_item("call_bar_tip_seen", "1");
    }
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
    // Píldora de abajo: silenciar, ensordecer y ajustes. Con la barra de llamada
    // oculta a mano suma "mostrar barra" y "colgar" y se pliega igual que la de
    // arriba: en reposo solo se ve Ajustes y al pasar el cursor se despliega hacia
    // arriba. `bottom_expand` va de 0.0 (plegada) a 1.0 (desplegada).
    let call_hidden = app.call_bar_hidden && app.voice_target.is_some() && app.new_call_ui;
    // 0.0 = píldora normal (3 botones), 1.0 = barra oculta (plegable, 5 botones). Se anima
    // para que al ocultar / mostrar la barra la píldora no salte de golpe.
    let hid = egui::emath::easing::cubic_out(ui.ctx().animate_bool_with_time(
        egui::Id::new("rail_bottom_hidden_t"),
        call_hidden,
        0.28,
    ));
    let hidden_layout = hid > 0.002;
    let bottom_n: usize = 5;
    let bottom_expand = if hidden_layout {
        let ctx = ui.ctx().clone();
        let id = egui::Id::new("rail_bottom_expand");
        let was: bool = ctx.data(|d| d.get_temp(id)).unwrap_or(false);
        let test_h = if was { pill_height(bottom_n) } else { pill_height(1) };
        let test_rect = Rect::from_min_max(
            egui::pos2(full.min.x, full.max.y - test_h),
            egui::pos2(full.min.x + width, full.max.y),
        );
        // Mientras el aviso está abierto se mantiene desplegada para mostrar dónde quedó "Colgar".
        let hover = (call_hidden && app.call_bar_tip_open)
            || ctx.pointer_hover_pos().is_some_and(|p| test_rect.contains(p));
        ctx.data_mut(|d| d.insert_temp(id, hover));
        egui::emath::easing::cubic_out(ctx.animate_bool_with_time(id.with("t"), hover, 0.22))
    } else {
        1.0
    };
    let hidden_h = pill_height(1) + (pill_height(bottom_n) - pill_height(1)) * bottom_expand;
    let bottom_h = pill_height(3) + (hidden_h - pill_height(3)) * hid;
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
        crate::ui::shell::notifications::paint_badge(
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

    // --- Píldora de abajo: silenciar, ensordecer, ajustes (y colgar) -----
    {
        let self_mute = app.voice_target.as_ref().map_or(app.self_mute, |t| t.self_mute);
        let self_deaf = app.voice_target.as_ref().map_or(app.self_deaf, |t| t.self_deaf);
        let cx = bottom_rect.center().x;
        let step = M_ICON + M_STEP;
        // Ajustes no se mueve: los demás salen de detrás suyo, hacia arriba.
        let settings_y = bottom_rect.max.y - M_PAD - M_ICON / 2.0;
        let lerp = |a: f32, b: f32| a + (b - a) * hid;
        // Posiciones: normal (silenciar, ensordecer, ajustes) y con la barra oculta.
        let mute_y = lerp(settings_y - 2.0 * step, settings_y - 3.0 * step * bottom_expand);
        let deaf_y = lerp(settings_y - step, settings_y - 2.0 * step * bottom_expand);
        let hang_y = settings_y - step * bottom_expand;
        let show_y = settings_y - 4.0 * step * bottom_expand;
        // Silenciar y ensordecer siempre están; solo se atenúan al plegarse.
        let fade = 1.0 + (bottom_expand - 1.0) * hid;
        let ready = fade > 0.6;
        // Colgar y mostrar la barra aparecen con la ocultación.
        let extra_fade = bottom_expand * hid;
        let extra_ready = extra_fade > 0.6;
        let danger_fill = extra::blend(palette.panel, palette.danger, 0.35);
        let danger_hover = extra::blend(palette.panel, palette.danger, 0.5);

        if hidden_layout && extra_fade > 0.02 {
            if square_button(
                ui,
                "show_call_bar",
                egui::pos2(cx, show_y),
                palette.surface.gamma_multiply(extra_fade),
                palette.surface_hover.gamma_multiply(extra_fade),
                Icon::ChevronUp,
                palette.secondary.gamma_multiply(extra_fade),
                "Mostrar la barra de llamada",
            )
            .clicked()
                && extra_ready
            {
                crate::ui::call::call_bar::show_again(app, ui.ctx());
            }
        }

        if fade > 0.02 {
            let (mute_icon, mute_fill, mute_hover, mute_color) = if self_mute {
                (Icon::MicOff, danger_fill, danger_hover, palette.danger)
            } else {
                (Icon::Mic, palette.surface, palette.surface_hover, palette.secondary)
            };
            if square_button(
                ui,
                "mute",
                egui::pos2(cx, mute_y),
                mute_fill.gamma_multiply(fade),
                mute_hover.gamma_multiply(fade),
                mute_icon,
                mute_color.gamma_multiply(fade),
                if self_mute { "Dejar de silenciar" } else { "Silenciar" },
            )
            .clicked()
                && ready
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
                egui::pos2(cx, deaf_y),
                deaf_fill.gamma_multiply(fade),
                deaf_hover.gamma_multiply(fade),
                deaf_icon,
                deaf_color.gamma_multiply(fade),
                if self_deaf { "Dejar de ensordecer" } else { "Ensordecer" },
            )
            .clicked()
                && ready
            {
                app.toggle_self_deafen();
            }
        }

        if hidden_layout
            && extra_fade > 0.02
            && square_button(
                ui,
                "hang_up",
                egui::pos2(cx, hang_y),
                palette.danger.gamma_multiply(extra_fade),
                extra::blend(palette.danger, Color32::WHITE, 0.12).gamma_multiply(extra_fade),
                Icon::PhoneOff,
                Color32::WHITE.gamma_multiply(extra_fade),
                "Colgar la llamada",
            )
            .clicked()
            && extra_ready
        {
            app.leave_voice();
        }

        if square_button(
            ui,
            "settings",
            egui::pos2(cx, settings_y),
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

    // Aviso de la primera vez que se oculta la barra de llamada.
    let tip_t = egui::emath::easing::cubic_out(ui.ctx().animate_bool_with_time(
        egui::Id::new("call_bar_hidden_tip_t"),
        call_hidden && app.call_bar_tip_open,
        0.25,
    ));
    if tip_t > 0.02 {
        call_hidden_tip(app, ui.ctx(), bottom_rect, tip_t);
    }

    // --- Píldora del medio: servidores y carpetas ------------------------
    let order = build_sidebar_order(
        &app.servers,
        app.discord_settings.as_ref().and_then(|s| s.guild_folders.as_ref()),
    );
    let inner = Rect::from_min_max(mid_rect.min + Vec2::new(0.0, 8.0), mid_rect.max - Vec2::new(0.0, 8.0));
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
                    let mut want_members: Option<usize> = None;
                        let mut toggled_folder = None;
                        join_requests_button(&*app, ui, M_ICON, 16, palette.panel);

                        for entry in &order {
                            match entry {
                                SidebarEntry::Guild(i) => {
                                    let i = *i;
                                    let server = &app.servers[i];
                                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
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
                                    crate::ui::shell::notifications::paint_badge(
                                        ui.painter(),
                                        rect.right_bottom() - Vec2::new(4.0, 4.0),
                                        app.server_mentions(i),
                                        &palette,
                                        palette.panel,
                                    );
                                    if server_hover(&*app, i, resp.on_hover_cursor(egui::CursorIcon::PointingHand), &palette, &mut want_members)
                                        .clicked()
                                    {
                                        clicked_server = Some(i);
                                    }
                                    ui.add_space(M_STEP);
                                }
                                SidebarEntry::Folder(folder) => {
                                    let is_open = app.open_guild_folders.contains(&folder.id);
                                    let folder_selected =
                                        selected_server.is_some_and(|i| folder.guild_indices.contains(&i));
                                    if !is_open {
                                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
                                        if folder_selected {
                                            side_marker(ui, mid_rect, rect.center().y, &palette);
                                        }
                                        draw_collapsed_folder_modern(ui, rect, folder, &app.servers, &palette);
                                        let folder_mentions: u32 =
                                            folder.guild_indices.iter().map(|&gi| app.server_mentions(gi)).sum();
                                        crate::ui::shell::notifications::paint_badge(
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
                                            let (rect, resp) =
                                                ui.allocate_exact_size(Vec2::splat(M_ICON), Sense::click());
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
                                            crate::ui::shell::notifications::paint_badge(
                                                ui.painter(),
                                                rect.right_bottom() - Vec2::new(4.0, 4.0),
                                                app.server_mentions(i),
                                                &palette,
                                                palette.panel,
                                            );
                                            if server_hover(&*app, i, resp.on_hover_cursor(egui::CursorIcon::PointingHand), &palette, &mut want_members)
                                                .clicked()
                                            {
                                                clicked_server = Some(i);
                                            }
                                            ui.add_space(M_STEP);
                                        }
                                        // Barrita al pie del grupo: click para plegarlo.
                                        let (rect, resp) =
                                            ui.allocate_exact_size(Vec2::new(M_ICON, 10.0), Sense::click());
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

                        if let Some(i) = want_members {
                        app.on_server_hover(i);
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
    if theme::is_modern() {
        palette.window
    } else {
        palette.panel
    }
}

fn selected_pill(ui: &mut egui::Ui, palette: &theme::Palette, icon_center: egui::Pos2, icon_size: f32) {
    let x = icon_center.x - icon_size / 2.0 - 10.0;
    let rect = egui::Rect::from_center_size(egui::pos2(x, icon_center.y), Vec2::new(4.0, icon_size * 0.5));
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
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), egui::Sense::hover());
    let center = rect.center();
    ui.painter().rect_filled(
        egui::Rect::from_center_size(center, Vec2::new(w, 2.0)),
        1.0,
        palette.outline,
    );
}

/// Reloj de arena de las solicitudes de ingreso pendientes, como en el cliente
/// oficial: solo aparece mientras haya alguna solicitud esperando revisión y,
/// al tocarlo, abre una lista flotante con los servidores. El estado
/// abierto/cerrado vive en la memoria de egui.
fn join_requests_button(app: &App, ui: &mut egui::Ui, size: f32, corner: u8, ring: Color32) {
    if app.pending_join_requests.is_empty() {
        return;
    }
    let palette = app.palette;
    let ctx = ui.ctx().clone();
    let open_id = egui::Id::new("join_requests_popup_open");
    let mut open: bool = ctx.data(|d| d.get_temp(open_id)).unwrap_or(false);

    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let lit = resp.hovered() || open;
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(corner),
        if lit { palette.surface_hover } else { palette.surface },
    );
    theme::paint_icon(
        ui,
        Icon::Hourglass,
        rect,
        size * 0.5,
        if lit { palette.text } else { palette.secondary },
    );
    crate::ui::shell::notifications::paint_badge(
        ui.painter(),
        rect.right_bottom() - Vec2::new(4.0, 4.0),
        app.pending_join_requests.len() as u32,
        &palette,
        ring,
    );
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Solicitudes de ingreso pendientes");
    let toggled = resp.clicked();
    if toggled {
        open = !open;
    }
    ui.add_space(8.0);

    if open {
        const W: f32 = 300.0;
        const EST_H: f32 = 200.0;
        let screen = ctx.viewport_rect();
        let mut pos = rect.right_top() + Vec2::new(12.0, 0.0);
        pos.x = pos.x.min(screen.max.x - W - 12.0).max(screen.min.x + 12.0);
        pos.y = pos.y.min(screen.max.y - EST_H - 12.0).max(screen.min.y + 12.0);
        let inner = egui::Area::new(egui::Id::new("join_requests_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::radius() + 2))
                    .inner_margin(Margin::same(12))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.set_width(W - 24.0);
                        theme::text(
                            ui,
                            "Solicitudes de ingreso pendientes",
                            theme::semibold(14.0),
                            palette.text,
                        );
                        ui.add_space(2.0);
                        theme::text(
                            ui,
                            "Los moderadores todavía están revisando tus respuestas.",
                            theme::regular(12.0),
                            palette.dim,
                        );
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for request in &app.pending_join_requests {
                                    let name = app.join_request_guild_name(request);
                                    let server = app.servers.iter().find(|s| s.guild_id == request.guild_id);
                                    let icon_url = server.and_then(|s| s.icon_url.clone()).or_else(|| {
                                        request.guild_icon.as_ref().map(|hash| {
                                            format!(
                                                "https://cdn.discordapp.com/icons/{}/{hash}.png?size=128",
                                                request.guild_id
                                            )
                                        })
                                    });
                                    let color = server.map(|s| s.icon_color).unwrap_or(palette.accent);
                                    let initial = name
                                        .chars()
                                        .next()
                                        .map(|c| c.to_uppercase().to_string())
                                        .unwrap_or_else(|| "?".to_owned());
                                    let (row, _) =
                                        ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
                                    extra::avatar(
                                        ui,
                                        row.left_center() + Vec2::new(20.0, 0.0),
                                        16.0,
                                        icon_url.as_deref(),
                                        color,
                                        &initial,
                                        &palette,
                                    );
                                    crate::ui::chat::emoji::paint_line_top(
                                        ui,
                                        row.left_top() + Vec2::new(48.0, 7.0),
                                        &name,
                                        theme::medium(13.5),
                                        palette.text,
                                        row.width() - 56.0,
                                    );
                                    ui.painter().text(
                                        row.left_top() + Vec2::new(48.0, 27.0),
                                        egui::Align2::LEFT_TOP,
                                        "En revisión",
                                        theme::regular(12.0),
                                        palette.dim,
                                    );
                                }
                            });
                    });
            });
        let popup_rect = inner.response.rect;
        let pressed_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx
                .pointer_interact_pos()
                .is_some_and(|p| !popup_rect.contains(p) && !rect.contains(p));
        if pressed_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ctx.data_mut(|d| d.insert_temp(open_id, open));
}


/// Número con separador de miles (`33389` → `33,389`), como lo muestra Discord.
fn fmt_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Tarjeta que aparece al pasar el mouse sobre un servidor del rail, a su
/// derecha y con una colita que apunta al ícono: ícono + nombre, quiénes están
/// en voz (de todos los canales de voz juntos, avatares apilados con un "+N" al
/// final), quiénes transmiten, "Silenciado" si corresponde y, abajo, en línea /
/// total con separador de miles.
///
/// Apenas el mouse está encima (sin esperar a que se dibuje la tarjeta)
/// deja `want_members = Some(i)`: el llamador corre `App::on_server_hover`,
/// que le pide al Gateway el conteo en línea / total y los datos de quienes
/// están en voz y todavía no conocemos.
fn server_hover(
    app: &App,
    index: usize,
    resp: egui::Response,
    palette: &theme::Palette,
    want_members: &mut Option<usize>,
) -> egui::Response {
    /// Avatares visibles en una pila antes de resumir con la píldora "+N".
    const MAX_STACK: usize = 5;
    /// Largo de la colita que apunta al ícono del server.
    const TAIL: f32 = 7.0;

    let Some(server) = app.servers.get(index) else {
        return resp;
    };
    if resp.hovered() {
        *want_members = Some(index);
    }
    // Instantáneo, como Discord. No se muestra mientras hay un botón apretado
    // (p. ej. arrastrando).
    let pressed = resp.ctx.input(|i| i.pointer.any_down());
    if !resp.hovered() || pressed {
        return resp;
    }

    let ctx = resp.ctx.clone();
    let icon_rect = resp.rect;
    // A la derecha del ícono, centrada en él (egui la corre si se sale de la pantalla).
    let pos = egui::pos2(icon_rect.right() + 14.0, icon_rect.center().y);
    egui::Area::new(egui::Id::new(("server_hover_card", index)))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos)
        .pivot(egui::Align2::LEFT_CENTER)
        .interactable(false)
        .show(&ctx, |ui| {
            let card = Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(14, 12))
                .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: palette.shadow })
                .show(ui, |ui| {
                    ui.set_min_width(176.0);
                    ui.set_max_width(300.0);

                    // Cabecera: ícono del server + nombre.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        extra::avatar_rounded(
                            ui,
                            icon_rect.center(),
                            14.0,
                            8.0,
                            server.icon_url.as_deref(),
                            server.icon_color,
                            &server.icon_initial,
                            palette,
                        );
                        theme::text(ui, extra::shorten(&server.name, 32), theme::semibold(15.0), palette.text);
                    });

                    // Todos los canales de voz del server juntos (una persona está en uno
                    // solo, pero se deduplica igual por las dudas).
                    let mut seen = std::collections::HashSet::new();
                    let voice: Vec<&crate::lib::data::VoiceOccupant> = server
                        .categories
                        .iter()
                        .flat_map(|category| category.channels.iter())
                        .flat_map(|channel| channel.voice_members.iter())
                        .filter(|occupant| seen.insert(occupant.user_id.as_str()))
                        .collect();

                    // Fila: ícono gris a la izquierda + pila de avatares (cada uno tapa un
                    // poco al anterior, con un aro del color de la tarjeta). Si hay más de
                    // `MAX_STACK`, una píldora dice cuántos faltan (tope "+99").
                    let row = |ui: &mut egui::Ui, icon: Icon, people: &[&crate::lib::data::VoiceOccupant]| {
                        const RADIUS: f32 = 14.0;
                        const RING: f32 = 2.0;
                        // Distancia entre centros: menos que el diámetro => se solapan.
                        const STEP: f32 = 21.0;
                        const ICON_COL: f32 = 36.0;

                        let shown = people.len().min(MAX_STACK);
                        let extra_count = people.len() - shown;
                        let pill_text = format!("+{}", extra_count.min(99));
                        let pill_w = if extra_count > 0 {
                            let galley = ui.painter().layout_no_wrap(
                                pill_text.clone(),
                                theme::semibold(12.5),
                                palette.secondary,
                            );
                            (galley.size().x + 22.0).max(RADIUS * 2.0)
                        } else {
                            0.0
                        };
                        let stack_end = if extra_count > 0 {
                            STEP * shown as f32 + pill_w
                        } else {
                            STEP * shown.saturating_sub(1) as f32 + RADIUS * 2.0
                        };
                        let width = ICON_COL + RING + stack_end + RING;
                        let height = (RADIUS + RING) * 2.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
                        let y = rect.center().y;

                        let icon_rect = Rect::from_center_size(egui::pos2(rect.left() + 11.0, y), Vec2::splat(22.0));
                        ui.put(icon_rect, |ui: &mut egui::Ui| theme::icon(ui, icon, 22.0, palette.dim));

                        let x0 = rect.left() + ICON_COL + RING + RADIUS;
                        for (i, occupant) in people.iter().take(shown).enumerate() {
                            let center = egui::pos2(x0 + STEP * i as f32, y);
                            ui.painter().circle_filled(center, RADIUS + RING, palette.overlay);
                            extra::avatar(
                                ui,
                                center,
                                RADIUS,
                                occupant.avatar_url.as_deref(),
                                occupant.avatar_color,
                                &occupant.initial(),
                                palette,
                            );
                        }
                        if extra_count > 0 {
                            let pill = Rect::from_min_size(
                                egui::pos2(x0 - RADIUS + STEP * shown as f32, y - RADIUS),
                                Vec2::new(pill_w, RADIUS * 2.0),
                            );
                            ui.painter().rect_filled(pill.expand(RING), CornerRadius::same(255), palette.overlay);
                            ui.painter().rect_filled(pill, CornerRadius::same(255), palette.surface_active);
                            ui.painter().text(
                                pill.center(),
                                egui::Align2::CENTER_CENTER,
                                pill_text,
                                theme::semibold(12.5),
                                palette.secondary,
                            );
                        }
                    };

                    if !voice.is_empty() {
                        ui.add_space(10.0);
                        row(ui, Icon::Volume2, &voice);
                    }

                    // Transmisiones (Go Live): otra fila, con el ícono de pantalla.
                    let streamers: Vec<&crate::lib::data::VoiceOccupant> =
                        voice.iter().copied().filter(|occupant| occupant.streaming).collect();
                    if !streamers.is_empty() {
                        ui.add_space(6.0);
                        row(ui, Icon::ScreenShare, &streamers);
                    }

                    if app.server_muted(index) {
                        ui.add_space(12.0);
                        theme::text(ui, "Silenciado", theme::regular(13.0), palette.dim);
                    }

                    // En línea / total, al final.
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let online = Color32::from_rgb(0x37, 0xa6, 0x62);
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 6.0, online);
                        theme::text(ui, fmt_count(server.online_count), theme::semibold(16.0), online);
                        if server.total_count > 0 {
                            ui.add_space(12.0);
                            let (ring, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            ui.painter().circle_stroke(ring.center(), 5.0, Stroke::new(2.5, palette.secondary));
                            theme::text(ui, fmt_count(server.total_count), theme::semibold(16.0), palette.secondary);
                        }
                    });
                });

            // Colita a la izquierda, apuntando al ícono del server. Se pinta con el
            // painter de la capa (no el del `ui`, que recortaría fuera de la tarjeta).
            let rect = card.response.rect;
            let tip_y = icon_rect.center().y.max(rect.top() + TAIL + 8.0).min(rect.bottom() - TAIL - 8.0);
            let painter = ui.ctx().layer_painter(ui.layer_id());
            let left = rect.left();
            let tip = egui::pos2(left - TAIL, tip_y);
            let upper = egui::pos2(left + 1.0, tip_y - TAIL);
            let lower = egui::pos2(left + 1.0, tip_y + TAIL);
            painter.add(egui::Shape::convex_polygon(vec![tip, upper, lower], palette.overlay, Stroke::NONE));
            painter.line_segment([tip, upper], Stroke::new(1.0, palette.outline));
            painter.line_segment([tip, lower], Stroke::new(1.0, palette.outline));
        });
    resp
}
