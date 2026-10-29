//! Fondo de océano animado (olas, brillo y burbujas) y splash de carga del
//! login. Réplica de la página de referencia "eCord – Iniciar sesión".

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::time::Duration;

use egui::epaint::{Mesh, Shape};
use egui::{pos2, vec2, Align, Color32, Pos2, Rect, Stroke};

use crate::theme::Palette;
use crate::ui::shell::extra::blend;

/// Burbujas: (posición horizontal 0..1, diámetro, duración s, retraso s).
const BUBBLES: [(f32, f32, f32, f32); 6] = [
    (0.12, 10.0, 11.0, 0.0),
    (0.28, 6.0, 8.0, 2.0),
    (0.47, 14.0, 13.0, 4.0),
    (0.66, 8.0, 9.0, 1.0),
    (0.82, 12.0, 12.0, 5.0),
    (0.92, 6.0, 7.0, 3.0),
];

/// Datos curiosos del splash.
const TIPS: [&str; 8] = [
    "Una nube pesa lo mismo que quinientos elefantes, pero ninguno se ha quejado todavía.",
    "Los lunes huelen a calcetín mojado en algunos universos paralelos.",
    "Nuestro hámster de pruebas dejó el cargo para dedicarse a la poesía abstracta.",
    "Si dices “pepino” tres veces seguidas, el servidor carga un 0 % más rápido.",
    "La luna estudió un año de contabilidad, pero lo dejó por falta de calculadora.",
    "Un pingüino con sombrero no es un pingüino: es una reunión de directivos.",
    "El wifi de los dinosaurios se caía mucho por culpa de los meteoritos.",
    "Alguien, en algún lugar, está comiendo una tostada con forma de Bolivia.",
];

/// Cada cuánto cambia el dato curioso (s) y lo que dura el fundido (s).
const TIP_EVERY: f32 = 3.4;
const TIP_FADE: f32 = 0.35;

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Colores del fondo. En oscuro son los de la página; en claro se derivan de
/// la paleta para no pintar un océano negro sobre un tema claro.
struct Colors {
    stops: [(f32, Color32); 4],
    glow: Color32,
    waves: [Color32; 3],
}

fn colors(p: &Palette) -> Colors {
    if p.dark {
        Colors {
            stops: [
                (0.0, Color32::from_rgb(0x1b, 0x20, 0x38)),
                (0.45, Color32::from_rgb(0x12, 0x18, 0x2c)),
                (0.75, Color32::from_rgb(0x0c, 0x1a, 0x33)),
                (1.0, Color32::from_rgb(0x0a, 0x25, 0x40)),
            ],
            glow: rgba(107, 124, 255, 0.22),
            waves: [rgba(0x2e, 0x3b, 0x9c, 0.32), rgba(0x23, 0x3a, 0x7a, 0.5), rgba(0x0e, 0x2a, 0x4d, 0.85)],
        }
    } else {
        let base = p.window_solid;
        let a = p.accent;
        Colors {
            stops: [
                (0.0, blend(base, a, 0.10)),
                (0.45, blend(base, a, 0.14)),
                (0.75, blend(base, a, 0.20)),
                (1.0, blend(base, a, 0.28)),
            ],
            glow: rgba(a.r(), a.g(), a.b(), 0.18),
            waves: [
                rgba(a.r(), a.g(), a.b(), 0.12),
                rgba(a.r(), a.g(), a.b(), 0.18),
                rgba(a.r(), a.g(), a.b(), 0.28),
            ],
        }
    }
}

fn gradient(painter: &egui::Painter, rect: Rect, stops: &[(f32, Color32)]) {
    let mut mesh = Mesh::default();
    for &(t, c) in stops {
        let y = rect.top() + rect.height() * t;
        mesh.colored_vertex(pos2(rect.left(), y), c);
        mesh.colored_vertex(pos2(rect.right(), y), c);
    }
    for i in 0..stops.len() - 1 {
        let a = (i * 2) as u32;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// Elipse con degradado radial: `color` en el centro y transparente en el borde.
fn radial_glow(painter: &egui::Painter, center: Pos2, rx: f32, ry: f32, color: Color32) {
    const N: u32 = 48;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    for i in 0..N {
        let a = TAU * i as f32 / N as f32;
        mesh.colored_vertex(center + vec2(a.cos() * rx, a.sin() * ry), Color32::TRANSPARENT);
    }
    for i in 0..N {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % N);
    }
    painter.add(Shape::mesh(mesh));
}

/// Una ola: relleno desde la curva hasta el borde inferior de su caja.
fn wave(
    painter: &egui::Painter,
    rect: Rect,
    bottom_offset: f32,
    y_units: f32,
    seconds: f32,
    reverse: bool,
    color: Color32,
    t: f32,
) {
    let w = rect.width().max(1.0);
    let h = (rect.height() * 0.34).max(170.0);
    let bottom = rect.bottom() - bottom_offset;
    let top = bottom - h;
    let y0 = top + y_units / 140.0 * h;
    let amp = 30.0 / 140.0 * h;
    let frac = (t / seconds).fract();
    let shift = if reverse { (1.0 - frac) * w } else { frac * w };

    let mut mesh = Mesh::default();
    let step = 6.0;
    let mut x = rect.left();
    loop {
        let u = (x - rect.left()) + shift;
        let y = y0 - amp * (TAU * u / w).sin();
        mesh.colored_vertex(pos2(x, y), color);
        mesh.colored_vertex(pos2(x, bottom), color);
        if x >= rect.right() {
            break;
        }
        x = (x + step).min(rect.right());
    }
    let columns = mesh.vertices.len() / 2;
    for i in 0..columns.saturating_sub(1) {
        let a = (i * 2) as u32;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// Pinta el océano ocupando `rect`. Pide repintado continuo (~30 fps).
pub fn paint(ui: &egui::Ui, rect: Rect, p: &Palette) {
    let t = ui.input(|i| i.time) as f32;
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    let painter = ui.painter().with_clip_rect(rect);
    let c = colors(p);

    gradient(&painter, rect, &c.stops);

    // Brillo superior que respira (9 s, de ida y vuelta).
    let phase = (t / 9.0) % 2.0;
    let tri = if phase < 1.0 { phase } else { 2.0 - phase };
    let eased = 0.5 - 0.5 * (PI * tri).cos();
    let k = 0.55 + 0.45 * eased;
    let glow = c.glow.gamma_multiply(k);
    radial_glow(
        &painter,
        pos2(rect.center().x, rect.top() - rect.height() * 0.2),
        rect.width() * 0.7 * 0.7,
        rect.height() * 0.7 * 0.6 * 0.7,
        glow,
    );

    // Burbujas.
    let bubble_fill = if p.dark { rgba(107, 124, 255, 0.08) } else { rgba(100, 110, 220, 0.08) };
    let bubble_line = if p.dark { rgba(154, 166, 255, 0.35) } else { rgba(100, 110, 220, 0.35) };
    for &(left, size, dur, delay) in &BUBBLES {
        if t < delay {
            continue;
        }
        let f = ((t - delay) / dur).fract();
        let opacity = if f < 0.15 { f / 0.15 } else { 1.0 - (f - 0.15) / 0.85 };
        let y = rect.bottom() + 20.0 - size / 2.0 - f * rect.height() * 0.7;
        let center = pos2(rect.left() + rect.width() * left + size / 2.0, y);
        painter.circle_filled(center, size / 2.0, bubble_fill.gamma_multiply(opacity));
        painter.circle_stroke(center, size / 2.0, Stroke::new(1.0, bubble_line.gamma_multiply(opacity)));
    }

    // Tres olas a distinta velocidad y profundidad.
    wave(&painter, rect, rect.height() * 0.06, 60.0, 34.0, false, c.waves[0], t);
    wave(&painter, rect, rect.height() * 0.02, 70.0, 24.0, true, c.waves[1], t);
    wave(&painter, rect, -rect.height() * 0.03, 80.0, 16.0, false, c.waves[2], t);
}

fn tip_seed(ctx: &egui::Context) -> usize {
    let id = egui::Id::new("ecord_login_tip_seed");
    ctx.data_mut(|d| {
        *d.get_temp_mut_or_insert_with(id, || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.subsec_nanos() as usize)
                .unwrap_or(0)
        })
    })
}

/// Pinta `text` centrado en `center_x`, con el borde superior en `top`, ajustado
/// a `wrap` de ancho. Devuelve el alto usado.
fn centered_text(
    ui: &egui::Ui,
    center_x: f32,
    top: f32,
    wrap: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
) -> f32 {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, wrap);
    job.halign = Align::Center;
    let galley = ui.painter().layout_job(job);
    let pos = pos2(center_x - galley.rect.center().x, top);
    let height = galley.size().y;
    ui.painter().galley(pos, galley, color);
    height
}

/// Splash de carga a pantalla completa. `elapsed` son los segundos desde que
/// empezó (para rotar los datos curiosos), `progress` va de 0 a 1. Devuelve el
/// punto justo debajo de la etiqueta por si hay que poner algo más (captcha).
pub fn paint_splash(ui: &mut egui::Ui, rect: Rect, p: &Palette, elapsed: f32, label: &str, progress: f32) -> Pos2 {
    let t = ui.input(|i| i.time) as f32;
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    let c = colors(p);
    let painter = ui.painter().with_clip_rect(rect);

    // Fondo: degradado + brillo radial a 38 % de la altura.
    let bottom = c.stops[3].1;
    let top_col = c.stops[0].1;
    gradient(&painter, rect, &[(0.0, top_col), (0.7, bottom), (1.0, bottom)]);
    radial_glow(
        &painter,
        pos2(rect.center().x, rect.top() + rect.height() * 0.38),
        rect.width() * 0.6 * 0.7,
        rect.height() * 0.5 * 0.7,
        blend(bottom, p.accent, 0.22),
    );

    let cx = rect.center().x;
    let block_top = rect.center().y - 168.0;

    // Marca: aro tenue + arco que gira + logo que respira.
    let mark_c = pos2(cx, block_top + 52.0);
    painter.circle_stroke(mark_c, 51.0, Stroke::new(2.0, p.accent.gamma_multiply(0.18)));
    let rot = t * TAU / 1.1;
    let start = -3.0 * PI / 4.0 + rot;
    let points: Vec<Pos2> = (0..=16)
        .map(|i| {
            let a = start + FRAC_PI_2 * i as f32 / 16.0;
            mark_c + vec2(a.cos(), a.sin()) * 51.0
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(2.0, p.accent)));
    let breathe = 1.0 + 0.07 * (0.5 - 0.5 * (TAU * t / 2.4).cos());
    crate::theme::logo(ui, mark_c, 68.0 * breathe, p.accent, p.on_accent);

    // "¿Sabías que...?"
    let did_top = block_top + 104.0 + 56.0;
    centered_text(ui, cx, did_top, 420.0, "¿Sabías que...?", crate::theme::bold(12.0), blend(p.text, p.secondary, 0.2));

    // Dato curioso con fundido al cambiar.
    let seed = tip_seed(ui.ctx());
    let cycle = (elapsed / TIP_EVERY).floor().max(0.0);
    let into = elapsed - cycle * TIP_EVERY;
    // Ciclo k >= 1: el dato anterior se desvanece, y luego entra el nuevo.
    let (shown, alpha) = if elapsed < TIP_EVERY {
        (0usize, 1.0)
    } else if into < TIP_FADE {
        (cycle as usize - 1, 1.0 - into / TIP_FADE)
    } else if into < TIP_FADE * 2.0 {
        (cycle as usize, (into - TIP_FADE) / TIP_FADE)
    } else {
        (cycle as usize, 1.0)
    };
    let tip = TIPS[(seed + shown) % TIPS.len()];
    let tip_color = blend(p.secondary, p.text, 0.5).gamma_multiply(alpha);
    centered_text(ui, cx, did_top + 26.0, 420.0, tip, crate::theme::regular(19.0), tip_color);

    // Barra de progreso.
    let bar_top = did_top + 26.0 + 86.0 + 30.0;
    let bar = Rect::from_min_size(pos2(cx - 120.0, bar_top), vec2(240.0, 4.0));
    painter.rect_filled(bar, 2.0, blend(p.window_solid, p.accent, 0.14));
    let filled = Rect::from_min_size(bar.min, vec2(bar.width() * progress.clamp(0.0, 1.0), 4.0));
    if filled.width() > 0.5 {
        let mut mesh = Mesh::default();
        let left = p.accent;
        let right = blend(p.accent, Color32::WHITE, 0.3);
        mesh.colored_vertex(filled.left_top(), left);
        mesh.colored_vertex(filled.left_bottom(), left);
        mesh.colored_vertex(filled.right_top(), right);
        mesh.colored_vertex(filled.right_bottom(), right);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(Shape::mesh(mesh));
    }

    // Etiqueta de lo que está haciendo.
    let label_top = bar_top + 4.0 + 12.0;
    let h = centered_text(ui, cx, label_top, 420.0, label, crate::theme::regular(13.0), p.secondary);
    pos2(cx, label_top + h + 14.0)
}
