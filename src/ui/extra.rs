//! Este módulo NO es parte de tu `theme.rs` original. Son los agregados
//! puntuales que necesita la vista estilo Discord (avatar placeholder con
//! inicial, punto de estado, mezcla de colores, degradado) y que tu theme
//! real no cubre porque está pensado para un cliente de Spotify. Si ya
//! tenés algo equivalente en tu proyecto (por ej. en `widgets.rs`), usá eso
//! y borrá este archivo.

use egui::{Color32, CornerRadius, Pos2, Rect, Vec2};

use crate::lib::data::Status;
use crate::ui::theme::{self, Palette};

/// A qué color de la paleta mapeamos cada estado. Tu `Palette` no tiene
/// colores de presencia (online/ausente/no molestar/desconectado), así que
/// reusamos los que más se parecen.
pub fn status_color(status: Status, palette: &Palette) -> Color32 {
    match status {
        Status::Online => palette.accent,
        Status::Idle => palette.warning,
        Status::Dnd => palette.danger,
        Status::Offline => palette.dim,
    }
}

/// Avatar placeholder circular con la inicial del nombre (no hay imágenes
/// reales de usuarios acá).
pub fn avatar_circle(
    ui: &mut egui::Ui,
    center: Pos2,
    radius: f32,
    bg: Color32,
    initial: &str,
    palette: &Palette,
) {
    ui.painter().circle_filled(center, radius, bg);
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        initial,
        theme::semibold(radius * 0.9),
        palette.text,
    );
}

/// Avatar real (si `avatar_url` trae algo) o círculo placeholder con
/// inicial como fallback — mismo fallback mientras la imagen todavía está
/// cargando (el loader de `egui_extras` es async; el primer frame o dos se
/// ve el círculo y después aparece la imagen).
///
/// El "clip a círculo" es un truco de egui: como el rect que le pasamos a
/// la imagen es exactamente `radius*2` de lado, ponerle `corner_radius`
/// igual a `radius` lo redondea hasta volverse un círculo perfecto.
pub fn avatar(
    ui: &mut egui::Ui,
    center: Pos2,
    radius: f32,
    avatar_url: Option<&str>,
    bg: Color32,
    initial: &str,
    palette: &Palette,
) {
    let Some(url) = avatar_url.filter(|u| !u.is_empty() && !crate::ui::anim::hide_profile_images()) else {
        // Sin URL: el círculo de color + inicial es el resultado final.
        avatar_circle(ui, center, radius, bg, initial, palette);
        return;
    };

    let size = Vec2::splat(radius * 2.0);
    // Avatares e íconos de server animados (`a_hash.gif?size=…`) los anima
    // `ui::anim` — ver ese módulo.
    // `anim::source`: si es un GIF (avatar/ícono animado) ya decodificado,
    // el cuadro que toca ahora; si no, la URL tal cual. Solo si está a la
    // vista: las listas de amigos/miembros dibujan filas fuera de pantalla
    // y no tiene sentido decodificar ni animar esos avatares.
    // Como en Discord, los avatares chicos (listas, mensajes, llamada) solo
    // se animan con el mouse encima; el resto se ve como imagen fija (el
    // `.png` del mismo avatar) y no gasta RAM en cuadros. Los grandes (la
    // tarjeta de perfil) se animan siempre.
    let rect = Rect::from_center_size(center, size);
    let source = if !ui.is_rect_visible(rect) {
        crate::ui::anim::plain(url)
    } else if radius >= 32.0 || ui.rect_contains_pointer(rect) {
        // Cuadros al doble de resolución del avatar (pantallas HiDPI).
        let px = (size.x * ui.ctx().pixels_per_point() * 1.5).ceil() as u32;
        crate::ui::anim::source_sized(ui.ctx(), url, px)
    } else {
        crate::ui::anim::plain(&crate::ui::anim::static_url(url))
    };
    let image = egui::Image::new(source)
        .corner_radius(CornerRadius::same(radius.min(255.0) as u8))
        .fit_to_exact_size(size)
        // Sin spinner: el círculo de abajo ya hace de placeholder mientras
        // carga.
        .show_loading_spinner(false);

    // `load_for_size` dispara la carga y nos dice en qué estado está, sin
    // dibujar nada. Muchos íconos de servidor/avatares son PNG con fondo
    // transparente (el logo solo, sin relleno): si pintáramos el círculo
    // de color SIEMPRE debajo, ese relleno se vería a través de las partes
    // transparentes del PNG en vez de dejar pasar el fondo real del panel
    // (rail/lista de DMs), y el ícono nunca se vería "transparente" de
    // verdad. Por eso el círculo de placeholder solo se pinta mientras la
    // imagen está cargando o si la carga falló — una vez que cargó bien,
    // se dibuja SOLO la imagen, dejando que su propio canal alfa se
    // mezcle con lo que sea que haya detrás.
    match image.load_for_size(ui.ctx(), size) {
        Ok(egui::load::TexturePoll::Ready { .. }) => {
            ui.put(Rect::from_center_size(center, size), image);
        }
        _ => {
            avatar_circle(ui, center, radius, bg, initial, palette);
        }
    }
}

/// Punto de presencia en la esquina de un avatar, con un anillo del color
/// del fondo detrás para que se recorte contra el avatar.
pub fn status_dot(ui: &mut egui::Ui, avatar_center: Pos2, avatar_radius: f32, color: Color32, ring: Color32) {
    let dot_radius = avatar_radius * 0.34;
    let dot_center = avatar_center + Vec2::new(avatar_radius * 0.75, avatar_radius * 0.75);
    ui.painter().circle_filled(dot_center, dot_radius + 2.5, ring);
    ui.painter().circle_filled(dot_center, dot_radius, color);
}

pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    // Interpola los 4 canales (premultiplicados): con colores opacos da lo
    // mismo que antes, y con los translúcidos de un tema con fondo no
    // convierte el resultado en opaco.
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_premultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

pub fn paint_vertical_gradient(ui: &mut egui::Ui, rect: Rect, top: Color32, bottom: Color32) {
    let steps = 32;
    let step_h = rect.height() / steps as f32;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        let color = blend(top, bottom, t);
        let y0 = rect.top() + step_h * i as f32;
        let y1 = y0 + step_h + 1.0;
        ui.painter().rect_filled(
            Rect::from_min_max(Pos2::new(rect.left(), y0), Pos2::new(rect.right(), y1)),
            0.0,
            color,
        );
    }
}
