use anyhow::Result;
use egui::{Align2, Color32, Pos2};

use self::ass::parse_ass_subtitle;

mod ass;

/// Márgenes del subtítulo en píxeles del video. Tipo propio (en vez de
/// `egui::Margin`) porque el tipo de los campos de `egui::Margin` cambió
/// entre versiones de egui (i8 -> f32) y aquí solo se usa `bottom`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Margin {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Margin {
    pub const fn same(v: f32) -> Self {
        Self { left: v, right: v, top: v, bottom: v }
    }
}

#[derive(Debug)]
pub struct Subtitle {
    pub text: String,
    pub fade: FadeEffect,
    pub alignment: Align2,
    pub primary_fill: Color32,
    pub position: Option<Pos2>,
    pub font_size: f32,
    pub margin: Margin,
    pub remaining_duration_ms: i64,
}

// todo, among others
// struct Transition<'a> {
//     offset_start_ms: i64,
//     offset_end_ms: i64,
//     accel: f64,
//     field: SubtitleField<'a>,
// }

enum SubtitleField<'a> {
    Fade(FadeEffect),
    Alignment(Align2),
    PrimaryFill(Color32),
    Position(Pos2),
    #[allow(unused)]
    Undefined(&'a str),
}

#[derive(Debug, Default)]
pub struct FadeEffect {
    _fade_in_ms: i64,
    _fade_out_ms: i64,
}

impl Default for Subtitle {
    fn default() -> Self {
        Self {
            text: String::new(),
            fade: FadeEffect {
                _fade_in_ms: 0,
                _fade_out_ms: 0,
            },
            remaining_duration_ms: 0,
            font_size: 30.,
            margin: Margin::same(85.0),
            alignment: Align2::CENTER_CENTER,
            primary_fill: Color32::WHITE,
            position: None,
        }
    }
}

impl Subtitle {
    fn from_text(text: &str) -> Self {
        Subtitle::default().with_text(text)
    }
    pub(crate) fn with_text(mut self, text: &str) -> Self {
        self.text = String::from(text);
        self
    }
    pub(crate) fn with_duration_ms(mut self, duration_ms: i64) -> Self {
        self.remaining_duration_ms = duration_ms;
        self
    }
    pub(crate) fn from_ffmpeg_rect(rect: ffmpeg::subtitle::RectRef) -> Result<Self> {
        match rect.kind() {
            ffmpeg::subtitle::Type::Ass => parse_ass_subtitle(rect.ass().unwrap_or("[ No Subtitle available ]")),
            ffmpeg::subtitle::Type::Bitmap => {
                Ok(Subtitle::from_text("[ unsupported bitmap subtitle ]").with_duration_ms(500))
            }
            ffmpeg::subtitle::Type::None => anyhow::bail!("no subtitle"),
            ffmpeg::subtitle::Type::Text => Ok(Subtitle::from_text(rect.text().unwrap_or("[ No Text Subtitle available ]"))),
        }
    }
}

impl FadeEffect {
    fn _is_zero(&self) -> bool {
        self._fade_in_ms == 0 && self._fade_out_ms == 0
    }
}
