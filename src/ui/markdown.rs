//! Subconjunto del markdown de Discord para el contenido de un mensaje:
//! `**negrita**`, `*cursiva*`/`_cursiva_`, `***negrita cursiva***`,
//! `__subrayado__`, `~~tachado~~`, `` `código en línea` ``, bloques
//! ` ```código``` `, citas `> ` y listas `- `/`* `, encabezados
//! `#`/`##`/`###`, código en línea con 1, 2 o 3 backticks (también en una
//! sola línea, como en las bios), timestamps `<t:unix:estilo>`, emojis custom `<:nombre:id>`/`<a:nombre:id>`,
//! menciones `<@id>`/`<@!id>`/`<@&id>`/`<#id>`/`@everyone`/`@here`, y
//! emoji unicode "jumbo" (mensajes que son solo emojis, como en Discord).
//!
//! No hay ninguna librería de markdown ya en `Cargo.toml` y el "sabor" de
//! Discord (spoilers `||...||`, menciones con `<...>`, sin markdown de
//! imágenes/links) no coincide con CommonMark de todos modos, así que
//! esto es un parser chiquito hecho a mano en vez de agregar una
//! dependencia nueva.

use std::collections::HashMap;

use egui::{Color32, CornerRadius, CursorIcon, Frame, Margin, Pos2, RichText, Sense, Ui, Vec2};

use crate::ui::theme::{self, Palette};

/// Lo que hace falta para resolver menciones dentro de un mensaje. Los
/// usuarios mencionados vienen embebidos en el propio mensaje (Discord
/// los manda así, ver `ChatMessage::mentions`); los canales solo se
/// pueden resolver si estamos mostrando el mensaje dentro de un server
/// (en un DM no hay lista de canales — queda `None`).
pub struct MentionCtx<'a> {
    pub mentions: &'a [(String, String)],
    pub channels: Option<&'a HashMap<String, String>>,
}

impl<'a> MentionCtx<'a> {
    pub fn none() -> Self {
        Self { mentions: &[], channels: None }
    }

    fn user_name(&self, id: &str) -> Option<&str> {
        self.mentions.iter().find(|(uid, _)| uid == id).map(|(_, name)| name.as_str())
    }

    fn channel_name(&self, id: &str) -> Option<&str> {
        self.channels.and_then(|m| m.get(id)).map(|s| s.as_str())
    }
}


// ---------------------------------------------------------------------
// Roles y clics en menciones
// ---------------------------------------------------------------------
//
// `ui::chat` está muy virtualizado y no recibe la lista de roles ni un
// `&mut App`, así que (igual que `profile_popup::request_open`) los roles
// del server abierto se publican en la memoria de egui una vez por frame
// (`publish_roles`, desde `App::ui`) y los clics se dejan como "pedidos"
// que `App::ui` levanta al frame siguiente.

/// id de rol -> (nombre, color `0xRRGGBB`; 0 = sin color).
type RoleMap = std::sync::Arc<HashMap<String, (String, u32)>>;

fn roles_key() -> egui::Id {
    egui::Id::new("ecord_mention_roles")
}

fn channel_click_key() -> egui::Id {
    egui::Id::new("ecord_mention_channel_click")
}

fn role_click_key() -> egui::Id {
    egui::Id::new("ecord_mention_role_click")
}

/// Publica los roles del server actual (vacío fuera de un server) para que
/// las menciones `<@&id>` muestren el nombre y el color reales.
pub fn publish_roles(ctx: &egui::Context, roles: &[crate::discord::models::Role]) {
    let map: HashMap<String, (String, u32)> =
        roles.iter().map(|r| (r.id.clone(), (r.name.clone(), r.color))).collect();
    ctx.memory_mut(|m| m.data.insert_temp::<RoleMap>(roles_key(), std::sync::Arc::new(map)));
}

fn role_info(ctx: &egui::Context, id: &str) -> Option<(String, u32)> {
    ctx.memory(|m| m.data.get_temp::<RoleMap>(roles_key())).and_then(|map| map.get(id).cloned())
}

/// Clic en una mención de rol: qué rol y dónde estaba el puntero.
#[derive(Clone)]
pub struct RoleClick {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub anchor: Option<Pos2>,
}

/// Saca (si había) el canal cuya mención `#canal` se clickeó este frame.
pub fn take_channel_request(ctx: &egui::Context) -> Option<String> {
    ctx.memory_mut(|m| {
        let req: Option<String> = m.data.get_temp(channel_click_key());
        if req.is_some() {
            m.data.remove::<String>(channel_click_key());
        }
        req
    })
}

/// Saca (si había) la mención `@rol` clickeada este frame.
pub fn take_role_request(ctx: &egui::Context) -> Option<RoleClick> {
    ctx.memory_mut(|m| {
        let req: Option<RoleClick> = m.data.get_temp(role_click_key());
        if req.is_some() {
            m.data.remove::<RoleClick>(role_click_key());
        }
        req
    })
}

fn role_color(color: u32) -> Option<Color32> {
    (color != 0).then(|| Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8))
}

/// Dibuja el contenido de un mensaje con el subset de markdown de Discord
/// ya aplicado. `size` es el tamaño de fuente "base" (negrita/cursiva/etc.
/// escalan relativo a eso); `color` es el color de texto normal.
pub fn show(ui: &mut Ui, palette: &Palette, text: &str, size: f32, color: Color32, ctx: &MentionCtx) {
    // Mensaje "solo emojis" (como máximo unos pocos, nada más de texto):
    // Discord los muestra bastante más grandes que el resto de los
    // mensajes. Lo hacemos acá, antes de partir en bloques, porque
    // aplica al mensaje entero.
    if let Some(runs) = jumbo_emoji_runs(text) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            for run in runs {
                render_run(ui, palette, &run, InlineStyle::default(), size * 2.6, color, false, ctx);
            }
        });
        return;
    }

    for block in split_blocks(text) {
        match block {
            Block::CodeBlock(code) => code_block(ui, palette, &code),
            Block::Heading(level, text) => {
                let bump = match level {
                    1 => 8.0,
                    2 => 5.0,
                    _ => 2.0,
                };
                render_paragraph(ui, palette, &text, size + bump, color, true, ctx);
            }
            Block::Quote(text) => quote_block(ui, palette, &text, size, color, ctx),
            // `-# texto`: subtexto, más chico y apagado.
            Block::Subtext(text) => {
                render_paragraph(ui, palette, &text, (size - 2.5).max(10.0), palette.dim, false, ctx)
            }
            Block::List(items) => {
                for item in items {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        theme::text(ui, "•", theme::regular(size), palette.dim);
                        ui.add_space(4.0);
                        ui.vertical(|ui| render_paragraph(ui, palette, &item, size, color, false, ctx));
                    });
                }
            }
            Block::Paragraph(text) => render_paragraph(ui, palette, &text, size, color, false, ctx),
        }
    }
}

// ---------------------------------------------------------------------
// División en bloques (nivel de línea)
// ---------------------------------------------------------------------

enum Block {
    CodeBlock(String),
    Heading(u8, String),
    Quote(String),
    Subtext(String),
    List(Vec<String>),
    Paragraph(String),
}

/// `-# texto` (subtexto de Discord). Devuelve el texto sin el marcador.
fn subtext_line(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("-# ")
}

fn heading_level(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    for level in [3u8, 2, 1] {
        let marker = "#".repeat(level as usize);
        if let Some(rest) = trimmed.strip_prefix(&marker) {
            if rest.starts_with(' ') {
                return Some((level, rest.trim_start()));
            }
        }
    }
    None
}

/// ¿La línea abre un bloque de código de varias líneas? Una línea como
/// ` ```🎮 Roblox``` ` (abre y cierra en la misma línea) es código EN LÍNEA,
/// no un bloque, y se deja pasar como párrafo.
fn is_fence_start(line: &str) -> bool {
    match line.trim_start().strip_prefix("```") {
        Some(after) => !after.contains("```"),
        None => false,
    }
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ") || trimmed.starts_with("* ")
}

fn list_item_text(line: &str) -> String {
    line.trim_start().trim_start_matches("- ").trim_start_matches("* ").to_string()
}

fn split_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }

        if is_fence_start(line) {
            let mut code_lines = Vec::new();
            let mut closed = false;
            for code_line in lines.by_ref() {
                if code_line.trim() == "```" {
                    closed = true;
                    break;
                }
                code_lines.push(code_line.to_string());
            }
            // Si nunca se cerró (usuario tipeando, o texto raro), mostramos
            // igual lo que había en vez de perderlo.
            let _ = closed;
            blocks.push(Block::CodeBlock(code_lines.join("\n")));
            continue;
        }

        if let Some((level, rest)) = heading_level(line) {
            blocks.push(Block::Heading(level, rest.to_string()));
            continue;
        }

        if let Some(rest) = subtext_line(line) {
            blocks.push(Block::Subtext(rest.to_string()));
            continue;
        }

        // `>>> ` cita TODO lo que sigue, incluidas las líneas siguientes.
        if let Some(rest) = line.trim_start().strip_prefix(">>> ") {
            let mut quote_lines = vec![rest.to_string()];
            quote_lines.extend(lines.by_ref().map(|l| l.to_string()));
            blocks.push(Block::Quote(quote_lines.join("\n")));
            continue;
        }

        if let Some(rest) = line.trim_start().strip_prefix("> ") {
            let mut quote_lines = vec![rest.to_string()];
            while let Some(next) = lines.peek() {
                if let Some(r2) = next.trim_start().strip_prefix("> ") {
                    quote_lines.push(r2.to_string());
                    lines.next();
                } else {
                    break;
                }
            }
            blocks.push(Block::Quote(quote_lines.join("\n")));
            continue;
        }

        if is_list_item(line) {
            let mut items = vec![list_item_text(line)];
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() || !is_list_item(next) {
                    break;
                }
                items.push(list_item_text(next));
                lines.next();
            }
            blocks.push(Block::List(items));
            continue;
        }

        let mut para_lines = vec![line.to_string()];
        while let Some(next) = lines.peek() {
            let stops = next.trim().is_empty()
                || is_fence_start(next)
                || heading_level(next).is_some()
                || subtext_line(next).is_some()
                || next.trim_start().starts_with("> ")
                || next.trim_start().starts_with(">>> ")
                || is_list_item(next);
            if stops {
                break;
            }
            para_lines.push(next.to_string());
            lines.next();
        }
        blocks.push(Block::Paragraph(para_lines.join("\n")));
    }

    blocks
}

// ---------------------------------------------------------------------
// Estilo y contenido en línea (negrita/cursiva/.../emoji/menciones)
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    spoiler: bool,
}

/// Un fragmento de contenido ya identificado: texto plano (para seguir
/// aplicándole negrita/cursiva/etc.) o uno de los tokens especiales de
/// Discord, que se renderizan aparte (imagen o "pill").
#[derive(Clone)]
enum RunContent {
    Text(String),
    CustomEmoji { name: String, id: String, animated: bool },
    UserMention { id: String },
    RoleMention { id: String },
    ChannelMention { id: String },
    /// `<t:unix>` / `<t:unix:estilo>`: se muestra en la hora local.
    Timestamp { unix: i64, style: char },
    /// `@everyone` o `@here`, tal cual (para mostrar el texto correcto).
    Broadcast(&'static str),
}

type Run = (RunContent, InlineStyle);

/// Delimitadores reconocidos, del más largo al más corto: hay que probar
/// `***` antes que `**`/`*` en la misma posición o nunca se arma la
/// combinación negrita+cursiva.
const DELIMS: &[(&str, fn(InlineStyle) -> InlineStyle)] = &[
    ("***", |s| InlineStyle { bold: true, italic: true, ..s }),
    ("**", |s| InlineStyle { bold: true, ..s }),
    ("__", |s| InlineStyle { underline: true, ..s }),
    ("~~", |s| InlineStyle { strike: true, ..s }),
    ("||", |s| InlineStyle { spoiler: true, ..s }),
    ("*", |s| InlineStyle { italic: true, ..s }),
    ("_", |s| InlineStyle { italic: true, ..s }),
];

/// Si `s` empieza con un emoji custom o una mención al estilo Discord
/// (`<:nombre:id>`, `<a:nombre:id>`, `<@id>`, `<@!id>`, `<@&id>`,
/// `<#id>`), devuelve el contenido ya armado y cuántos bytes ocupó el
/// token en `s`. `None` si no matchea nada (el `<` es literal).
fn try_parse_angle_token(s: &str) -> Option<(RunContent, usize)> {
    let rest = s.strip_prefix('<')?;

    // Timestamp: `<t:1234567890>` o `<t:1234567890:t>` (t T d D f F R).
    if let Some(after) = rest.strip_prefix("t:") {
        let gt = after.find('>')?;
        let body = &after[..gt];
        let (num, style) = body.split_once(':').unwrap_or((body, "f"));
        let unix: i64 = num.parse().ok()?;
        let mut chars = style.chars();
        let style = match (chars.next(), chars.next()) {
            (Some(c), None) if "tTdDfFR".contains(c) => c,
            _ => return None,
        };
        return Some((RunContent::Timestamp { unix, style }, 1 + 2 + gt + 1));
    }

    // Emoji custom: <:nombre:id> o animado <a:nombre:id>. El nombre solo
    // tiene letras/números/`_` en Discord de verdad, pero no perdemos
    // nada siendo permisivos acá (si no cierra bien, simplemente no
    // matchea y queda como texto literal).
    for prefix in [":", "a:"] {
        if let Some(after) = rest.strip_prefix(prefix) {
            if let Some(colon) = after.find(':') {
                let name = &after[..colon];
                let after_colon = &after[colon + 1..];
                if let Some(gt) = after_colon.find('>') {
                    let id = &after_colon[..gt];
                    if !name.is_empty() && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                        let consumed = 1 + prefix.len() + colon + 1 + gt + 1;
                        return Some((
                            RunContent::CustomEmoji {
                                name: name.to_string(),
                                id: id.to_string(),
                                animated: prefix == "a:",
                            },
                            consumed,
                        ));
                    }
                }
            }
            // Un `<:`/`<a:` que no cierra bien no puede ser ninguna otra
            // cosa tampoco (los otros prefijos no empiezan igual) — no
            // seguimos probando, para no confundir `<a:` con nada más.
            return None;
        }
    }

    // Menciones: `<@&id>` (rol), `<@!id>`/`<@id>` (usuario), `<#id>` (canal).
    let (marker_len, make): (usize, fn(String) -> RunContent) = if rest.starts_with("@&") {
        (2, |id| RunContent::RoleMention { id })
    } else if rest.starts_with("@!") {
        (2, |id| RunContent::UserMention { id })
    } else if rest.starts_with('@') {
        (1, |id| RunContent::UserMention { id })
    } else if rest.starts_with('#') {
        (1, |id| RunContent::ChannelMention { id })
    } else {
        return None;
    };

    let after_marker = &rest[marker_len..];
    let gt = after_marker.find('>')?;
    let id = &after_marker[..gt];
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let consumed = 1 + marker_len + gt + 1;
    Some((make(id.to_string()), consumed))
}

/// Posición (en bytes) de la primera tanda de EXACTAMENTE `n` backticks
/// seguidos dentro de `s`.
fn find_backtick_run(s: &str, n: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' {
            let start = i;
            while i < b.len() && b[i] == b'`' {
                i += 1;
            }
            if i - start == n {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Formatea un `<t:unix:estilo>` en la zona horaria local, como Discord.
fn format_timestamp(unix: i64, style: char) -> String {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    const MESES: [&str; 12] = [
        "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre",
        "octubre", "noviembre", "diciembre",
    ];
    const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];

    let Some(dt) = Local.timestamp_opt(unix, 0).single() else {
        return format!("<t:{unix}>");
    };
    let hm = format!("{:02}:{:02}", dt.hour(), dt.minute());
    let hms = format!("{hm}:{:02}", dt.second());
    let short_date = format!("{:02}/{:02}/{}", dt.day(), dt.month(), dt.year());
    let long_date = format!("{} de {} de {}", dt.day(), MESES[dt.month0() as usize], dt.year());
    match style {
        't' => hm,
        'T' => hms,
        'd' => short_date,
        'D' => long_date,
        'F' => format!(
            "{}, {long_date} {hm}",
            DIAS[dt.weekday().num_days_from_monday() as usize]
        ),
        'R' => {
            let diff = unix - Local::now().timestamp();
            let abs = diff.unsigned_abs();
            let (n, unit) = match abs {
                0..=59 => (abs, "segundo"),
                60..=3599 => (abs / 60, "minuto"),
                3600..=86399 => (abs / 3600, "hora"),
                86400..=2_591_999 => (abs / 86400, "día"),
                2_592_000..=31_535_999 => (abs / 2_592_000, "mes"),
                _ => (abs / 31_536_000, "año"),
            };
            let plural = match (n, unit) {
                (1, _) => unit.to_string(),
                (_, "mes") => "meses".to_string(),
                _ => format!("{unit}s"),
            };
            if diff >= 0 { format!("en {n} {plural}") } else { format!("hace {n} {plural}") }
        }
        // 'f' (y cualquier otro): fecha larga + hora.
        _ => format!("{long_date} {hm}"),
    }
}

/// Convierte texto plano con marcadores de Discord en una lista de
/// fragmentos con estilo. Recursivo: el contenido entre un par de
/// delimitadores se vuelve a parsear con el estilo acumulado, así
/// `**_esto_**` sale negrita Y cursiva.
fn parse_inline(text: &str, style: InlineStyle, prev_char: Option<char>, out: &mut Vec<Run>) {
    let mut rest = text;
    let mut literal = String::new();
    let mut last_char = prev_char;

    'outer: while !rest.is_empty() {
        // Emoji custom / menciones: van primero porque no se anidan (son
        // un token opaco) y porque `<` no es un delimitador de énfasis,
        // así que no compite con nada del loop de `DELIMS` de abajo.
        if rest.starts_with('<') {
            if let Some((content, consumed)) = try_parse_angle_token(rest) {
                if !literal.is_empty() {
                    out.push((RunContent::Text(std::mem::take(&mut literal)), style));
                }
                out.push((content, style));
                rest = &rest[consumed..];
                last_char = Some('>');
                continue 'outer;
            }
        }
        if rest.starts_with("@everyone") || rest.starts_with("@here") {
            let (tag, len) = if rest.starts_with("@everyone") { ("everyone", 9) } else { ("here", 5) };
            if !literal.is_empty() {
                out.push((RunContent::Text(std::mem::take(&mut literal)), style));
            }
            out.push((RunContent::Broadcast(tag), style));
            rest = &rest[len..];
            last_char = Some('e');
            continue 'outer;
        }

        // Los backticks son especiales: no se anida nada adentro (es código
        // literal) y no aplican las reglas de "no partir palabras" de `_`/`*`.
        // Se cierra con una tanda de backticks de la MISMA cantidad que la que
        // abrió, así `` `x` ``, ` ``x`` ` y ` ```x``` ` (como en las bios de
        // Discord, en una sola línea) salen como código en línea.
        if rest.starts_with('`') {
            let n = rest.bytes().take_while(|&b| b == b'`').count();
            let after = &rest[n..];
            if let Some(close_rel) = find_backtick_run(after, n) {
                if !literal.is_empty() {
                    out.push((RunContent::Text(std::mem::take(&mut literal)), style));
                }
                let code = &after[..close_rel];
                if !code.is_empty() {
                    out.push((RunContent::Text(code.to_string()), InlineStyle { code: true, ..style }));
                }
                rest = &after[close_rel + n..];
                last_char = Some('`');
                continue 'outer;
            }
            // Sin cierre: esos backticks son texto literal.
            literal.push_str(&rest[..n]);
            rest = &rest[n..];
            last_char = Some('`');
            continue 'outer;
        }

        for &(delim, apply) in DELIMS {
            let Some(after) = rest.strip_prefix(delim) else { continue };
            // `_`/`__` no cuentan como énfasis en medio de una palabra
            // (`archivo_final_v2` no debería salir en cursiva): mismo
            // criterio que CommonMark para subrayados sueltos.
            if delim.starts_with('_') {
                let prev_alnum = last_char.is_some_and(|c| c.is_alphanumeric());
                if prev_alnum {
                    continue;
                }
            }
            let Some(close_rel) = after.find(delim) else { continue };
            let inner = &after[..close_rel];
            if delim.starts_with('_') {
                let next_char = after[close_rel + delim.len()..].chars().next();
                if next_char.is_some_and(|c| c.is_alphanumeric()) {
                    continue;
                }
            }
            if !literal.is_empty() {
                out.push((RunContent::Text(std::mem::take(&mut literal)), style));
            }
            parse_inline(inner, apply(style), None, out);
            rest = &after[close_rel + delim.len()..];
            last_char = delim.chars().last();
            continue 'outer;
        }

        let mut chars = rest.chars();
        let c = chars.next().expect("rest no está vacío acá");
        literal.push(c);
        last_char = Some(c);
        rest = chars.as_str();
    }

    if !literal.is_empty() {
        out.push((RunContent::Text(literal), style));
    }
}

/// Corta un fragmento en tokens de "palabra" y "espacio" preservando el
/// texto exacto de cada uno, para poder ir metiéndolos como widgets
/// separados en un `horizontal_wrapped` y que el wrap ocurra en los
/// espacios en vez de cortar cualquier fragmento con estilo entero.
fn split_ws_tokens(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut in_space: Option<bool> = None;
    for (i, c) in s.char_indices() {
        let is_space = c == ' ';
        match in_space {
            None => in_space = Some(is_space),
            Some(prev) if prev != is_space => {
                tokens.push(&s[start..i]);
                start = i;
                in_space = Some(is_space);
            }
            _ => {}
        }
    }
    if start < s.len() {
        tokens.push(&s[start..]);
    }
    tokens
}

fn render_paragraph(
    ui: &mut Ui,
    palette: &Palette,
    text: &str,
    size: f32,
    color: Color32,
    heading: bool,
    ctx: &MentionCtx,
) {
    for line in text.split('\n') {
        let mut runs = Vec::new();
        parse_inline(line, InlineStyle::default(), None, &mut runs);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
            for (content, style) in &runs {
                render_run(ui, palette, content, *style, size, color, heading, ctx);
            }
        });
    }
}

fn render_run(
    ui: &mut Ui,
    palette: &Palette,
    content: &RunContent,
    style: InlineStyle,
    size: f32,
    color: Color32,
    heading: bool,
    ctx: &MentionCtx,
) {
    match content {
        RunContent::CustomEmoji { name, id, animated } => {
            custom_emoji(ui, name, id, *animated, size);
        }
        RunContent::UserMention { id } => {
            let known = ctx.user_name(id);
            let label = match known {
                Some(name) => format!("@{name}"),
                // Sin el objeto del usuario embebido (no debería pasar
                // para menciones reales, Discord siempre las manda en
                // `message.mentions`) no hay de dónde sacar el nombre.
                None => "@usuario".to_string(),
            };
            if let Some(resp) = mention_pill(ui, palette, &label, size, None, true) {
                if resp.clicked() {
                    // Abre la tarjeta de perfil de la persona mencionada.
                    crate::ui::profile_popup::request_open(
                        ui.ctx(),
                        id.clone(),
                        known.unwrap_or("usuario").to_string(),
                        None,
                        palette.accent,
                    );
                }
            }
        }
        RunContent::RoleMention { id } => {
            match role_info(ui.ctx(), id) {
                Some((name, color)) => {
                    let tint = role_color(color);
                    if let Some(resp) = mention_pill(ui, palette, &format!("@{name}"), size, tint, true) {
                        if resp.clicked() {
                            let anchor = ui.ctx().input(|i| i.pointer.latest_pos());
                            let click = RoleClick { id: id.clone(), name, color, anchor };
                            ui.ctx().memory_mut(|m| m.data.insert_temp(role_click_key(), click));
                            ui.ctx().request_repaint();
                        }
                    }
                }
                // Sin la lista de roles del guild (DM, server sin cargar)
                // no hay nombre ni miembros que mostrar.
                None => {
                    mention_pill(ui, palette, "@rol", size, None, false);
                }
            }
        }
        RunContent::ChannelMention { id } => {
            let known = ctx.channel_name(id);
            let label = match known {
                Some(name) => format!("#{name}"),
                None => "#canal".to_string(),
            };
            if let Some(resp) = mention_pill(ui, palette, &label, size, None, known.is_some()) {
                if resp.clicked() {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(channel_click_key(), id.clone()));
                    ui.ctx().request_repaint();
                }
            }
        }
        RunContent::Broadcast(tag) => {
            // `@everyone`/`@here` no llevan a ningún lado.
            mention_pill(ui, palette, &format!("@{tag}"), size, None, false);
        }
        RunContent::Timestamp { unix, style: ts_style } => {
            let text = RunContent::Text(format_timestamp(*unix, *ts_style));
            render_run(ui, palette, &text, style, size, color, heading, ctx);
        }
        RunContent::Text(text) => {
            if text.is_empty() {
                return;
            }
            if style.code {
                code_chip(ui, palette, text, size);
                return;
            }
            if style.spoiler {
                spoiler_chip(ui, palette, text, style, size, color);
                return;
            }
            for tok in split_ws_tokens(text) {
                if tok.is_empty() {
                    continue;
                }
                let font = if style.bold || heading { theme::bold(size) } else { theme::regular(size) };
                let styled = |ui: &mut Ui, s: &str| {
                    let mut rt = RichText::new(s).font(font.clone()).color(color);
                    if style.italic {
                        rt = rt.italics();
                    }
                    if style.underline {
                        rt = rt.underline();
                    }
                    if style.strike {
                        rt = rt.strikethrough();
                    }
                    ui.add(egui::Label::new(rt).selectable(true));
                };
                // Emojis Unicode: imagen a color (Twemoji) en vez del glifo
                // monocromático de la fuente. El resto del token sigue
                // siendo texto con su estilo.
                if crate::ui::emoji::contains_emoji(tok) {
                    for seg in crate::ui::emoji::split(tok) {
                        match seg {
                            crate::ui::emoji::Seg::Text(t) => styled(&mut *ui, t),
                            crate::ui::emoji::Seg::Emoji(e) => crate::ui::emoji::inline(ui, e, &font, color),
                        }
                    }
                } else {
                    styled(ui, tok);
                }
            }
        }
    }
}

/// Emoji custom de un server (`<:nombre:id>` o animado `<a:nombre:id>`):
/// se pide como imagen al CDN de Discord, en `.gif` si es animado y
/// `.png` si no — mismo criterio que el picker de reacciones
/// (`lib::data::CustomEmoji::url`) y las reacciones ya puestas
/// (`ui::chat::reaction_pill_custom`), con el mismo empujoncito
/// `gif_safe_url` para que el loader animado de `egui_extras` lo detecte
/// a pesar del `?size=48` después de la extensión. Si no carga (falló la
/// red, se borró el emoji), se cae a `:nombre:` como texto.
fn custom_emoji(ui: &mut Ui, name: &str, id: &str, animated: bool, size: f32) {
    let px = (size * 1.35).round().max(16.0);
    let ext = if animated { "gif" } else { "png" };
    let url = format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48");
    let url = if animated { crate::ui::media::gif_safe_url(&url) } else { url };
    let image = egui::Image::new(crate::ui::anim::source(ui.ctx(), &url))
        .fit_to_exact_size(Vec2::splat(px))
        .show_loading_spinner(false);
    match image.load_for_size(ui.ctx(), Vec2::splat(px)) {
        Ok(egui::load::TexturePoll::Ready { .. }) => {
            ui.add(image);
        }
        _ => {
            // Mientras carga (o si falla), mostramos el nombre como
            // texto — mejor que un hueco en blanco o un ícono de error.
            ui.label(RichText::new(format!(":{name}:")).font(theme::regular(size)));
        }
    }
}

/// Fondo tipo "pill" resaltado usado para `@menciones`/`#canales`, igual
/// que en Discord: un rectángulo redondeado en el color de acento, con
/// texto también en ese color.
fn mention_pill(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    size: f32,
    tint: Option<Color32>,
    clickable: bool,
) -> Option<egui::Response> {
    let tint = tint.unwrap_or(palette.accent);
    let inner = Frame::new()
        .fill(tint.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(label).font(theme::medium(size)).color(tint))
                    // Seleccionable le robaría el clic al pill.
                    .selectable(!clickable),
            );
        });
    if !clickable {
        return None;
    }
    let rect = inner.response.rect;
    let resp = ui
        .interact(rect, inner.response.id.with("mention_click"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint.gamma_multiply(0.14));
    }
    Some(resp)
}

fn code_chip(ui: &mut Ui, palette: &Palette, text: &str, size: f32) {
    let font = egui::FontId::new(size - 1.0, egui::FontFamily::Monospace);
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            let code_label = |ui: &mut Ui, t: &str| {
                ui.add(
                    egui::Label::new(RichText::new(t).font(font.clone()).color(palette.accent))
                        .selectable(true),
                );
            };
            // Emojis a color (Twemoji) también dentro del chip, igual que en
            // el texto normal; el resto queda en monoespaciada.
            if crate::ui::emoji::contains_emoji(text) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for seg in crate::ui::emoji::split(text) {
                        match seg {
                            crate::ui::emoji::Seg::Text(t) => code_label(ui, t),
                            crate::ui::emoji::Seg::Emoji(e) => {
                                crate::ui::emoji::inline(ui, e, &font, palette.accent)
                            }
                        }
                    }
                });
            } else {
                code_label(ui, text);
            }
        });
}

/// Spoiler `||texto||`: tapado con una barra hasta que se le pasa el
/// mouse por encima (versión simplificada del click-to-reveal real de
/// Discord, para no tener que guardar un estado de "revelado" persistente
/// por mensaje).
fn spoiler_chip(ui: &mut Ui, palette: &Palette, text: &str, style: InlineStyle, size: f32, color: Color32) {
    let font = if style.bold { theme::bold(size) } else { theme::regular(size) };
    let response = ui.add(
        egui::Label::new(RichText::new(text).font(font).color(color))
            .sense(egui::Sense::hover()),
    );
    if !response.hovered() {
        ui.painter().rect_filled(response.rect, CornerRadius::same(3), palette.dim);
    }
}

fn code_block(ui: &mut Ui, palette: &Palette, code: &str) {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS_SMALL + 2))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for line in code.lines() {
                ui.add(
                    egui::Label::new(
                        RichText::new(if line.is_empty() { " " } else { line })
                            .font(egui::FontId::new(12.5, egui::FontFamily::Monospace))
                            .color(palette.text),
                    ),
                );
            }
        });
    ui.add_space(2.0);
}

fn quote_block(ui: &mut Ui, palette: &Palette, text: &str, size: f32, color: Color32, ctx: &MentionCtx) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let inner = ui.vertical(|ui| render_paragraph(ui, palette, text, size, color, false, ctx));
        let content_rect = inner.response.rect;
        let bar = egui::Rect::from_min_size(
            egui::pos2(content_rect.min.x - 8.0, content_rect.min.y),
            egui::vec2(3.0, content_rect.height().max(16.0)),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), palette.outline);
    });
}

// ---------------------------------------------------------------------
// Emoji "jumbo": mensajes que son solo (pocos) emojis
// ---------------------------------------------------------------------

/// Si el mensaje entero, sacando espacios, son nada más que unos pocos
/// emojis (unicode y/o custom `<:nombre:id>`, mezclados), devuelve esos
/// fragmentos para pintarlos grandes — igual que hace Discord. `None` si
/// hay cualquier otra cosa de por medio (texto, markdown, muchos emojis).
fn jumbo_emoji_runs(text: &str) -> Option<Vec<RunContent>> {
    const MAX_JUMBO: usize = 8;
    let mut runs = Vec::new();
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }

    while !rest.is_empty() {
        if rest.starts_with(char::is_whitespace) {
            rest = rest.trim_start();
            continue;
        }
        if rest.starts_with('<') {
            if let Some((content @ RunContent::CustomEmoji { .. }, consumed)) = try_parse_angle_token(rest) {
                runs.push(content);
                rest = &rest[consumed..];
                if runs.len() > MAX_JUMBO {
                    return None;
                }
                continue;
            }
            return None;
        }
        let mut chars = rest.chars();
        let c = chars.next()?;
        if !is_emoji_char(c) {
            return None;
        }
        let len = c.len_utf8();
        // Selectores de variación / modificadores de tono de piel / ZWJ
        // pegados al emoji anterior (👍🏽, 👨‍👩‍👧, etc.) cuentan como parte
        // del mismo emoji visual, no como uno nuevo — si no, algo como
        // "👨‍👩‍👧" (3 personas unidas con ZWJ) se rechazaría por "muchos
        // emojis" en vez de tratarse como uno solo.
        let mut end = len;
        for c2 in rest[len..].chars() {
            if c2 == '\u{200D}' || (c2 as u32) == 0xFE0F || is_skin_tone_modifier(c2) {
                end += c2.len_utf8();
            } else if end > len && is_emoji_char(c2) && rest[..end].ends_with('\u{200D}') {
                end += c2.len_utf8();
            } else {
                break;
            }
        }
        runs.push(RunContent::Text(rest[..end].to_string()));
        rest = &rest[end..];
        if runs.len() > MAX_JUMBO {
            return None;
        }
    }

    if runs.is_empty() {
        None
    } else {
        Some(runs)
    }
}

fn is_skin_tone_modifier(c: char) -> bool {
    matches!(c as u32, 0x1F3FB..=0x1F3FF)
}

/// Heurística de "esto es un emoji unicode": no es exacta al 100% (nadie
/// sin la base completa de Unicode lo es), pero cubre los rangos donde
/// vive la gran mayoría de los emojis que la gente realmente manda.
fn is_emoji_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x1F300..=0x1FAFF // símbolos misc, pictografs, emoticons, transporte, suplementarios
        | 0x2600..=0x27BF  // misc symbols + dingbats (☀ ✉ ✂ ❤ etc.)
        | 0x2190..=0x21FF  // flechas (usadas como emoji sueltas a veces)
        | 0x2300..=0x23FF  // misc technical (⌚ ⏰ etc.)
        | 0x2B00..=0x2BFF  // misc symbols and arrows (⭐ ➡ etc.)
        | 0x1F1E6..=0x1F1FF // banderas (pares de "regional indicator")
        | 0x200D           // ZWJ (unión de emoji compuestos)
        | 0xFE0F            // selector de variación (forzar presentación emoji)
    )
}
