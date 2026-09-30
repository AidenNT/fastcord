//! Right-to-left text support for egui's left-to-right layout.
//!
//! epaint shapes letters within Arabic and Hebrew words but does not reorder
//! the words or align the line to the right.
//!
//! [`display_text`] applies visual word order while preserving logical letter
//! order for shaping. [`layout`] handles wrapped and truncated text one row at
//! a time and places ellipses at the left reading edge.

use std::borrow::Cow;
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Align2, Color32, FontId, Galley, Painter, Pos2, Rect, pos2};
use unicode_bidi::{BidiClass, BidiInfo, bidi_class};

/// The character that marks a cut.
pub const ELLIPSIS: char = '\u{2026}';

/// Whether the text reads right to left: decided by its first strong
/// character, the way the bidi algorithm decides a paragraph's direction.
/// ASCII never does, and most text is ASCII, so that check comes first and
/// costs nothing per frame.
pub fn is_rtl(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    for character in text.chars() {
        match bidi_class(character) {
            BidiClass::L => return false,
            BidiClass::R | BidiClass::AL => return true,
            _ => {}
        }
    }
    false
}

/// The edge the text should hug.
pub fn halign_for(text: &str) -> Align {
    if is_rtl(text) {
        Align::RIGHT
    } else {
        Align::LEFT
    }
}

/// `text` as the engine should receive it: every right-to-left line with
/// its words reordered. Text that needs no change comes back borrowed.
pub fn display_text(text: &str) -> Cow<'_, str> {
    if !text.contains('\n') {
        return display_line(text);
    }
    if !text.split('\n').any(is_rtl) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.split('\n')
            .map(display_line)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The reordered text, or `None` when `text` can be used as it is. Saves a
/// copy where the caller already owns a `String`.
pub fn reorder(text: &str) -> Option<String> {
    match display_text(text) {
        Cow::Owned(owned) => Some(owned),
        Cow::Borrowed(_) => None,
    }
}

/// One line. Punctuation, digits, and an ellipsis marking a cut keep the
/// places the bidi algorithm gives them; only the letters of each
/// right-to-left run go back to logical order, for the shaper to mirror.
fn display_line(line: &str) -> Cow<'_, str> {
    if !is_rtl(line) || line.chars().all(is_rtl_letter) {
        return Cow::Borrowed(line);
    }
    let line = line.trim_end_matches('\r');
    let info = BidiInfo::new(line, None);
    let Some(paragraph) = info.paragraphs.first() else {
        return Cow::Borrowed(line);
    };
    let visual = info.reorder_line(paragraph, paragraph.range.clone());
    let mut out = String::with_capacity(line.len());
    let mut letters: Vec<char> = Vec::new();
    for (index, word) in visual.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        for character in word.chars() {
            if is_rtl_letter(character) {
                letters.push(character);
            } else {
                out.extend(letters.drain(..).rev());
                out.push(character);
            }
        }
        out.extend(letters.drain(..).rev());
    }
    Cow::Owned(out)
}

/// A letter of a right-to-left script, or a mark that rides on one.
fn is_rtl_letter(character: char) -> bool {
    matches!(
        bidi_class(character),
        BidiClass::R | BidiClass::AL | BidiClass::NSM
    )
}

/// Lays `text` out within `wrap_width`, on at most `max_rows` rows, with
/// `overflow` marking a cut, in reading order for either direction. The
/// galley's `halign` says which edge to anchor it at; [`galley_pos`] does
/// that for a painter, and `egui::Label` does it on its own.
pub fn layout(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    wrap_width: f32,
    max_rows: usize,
    overflow: Option<char>,
) -> Arc<Galley> {
    if !text.split('\n').any(is_rtl) {
        let mut job = LayoutJob::simple(text.to_owned(), font, color, wrap_width);
        job.wrap.max_rows = max_rows;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = overflow;
        return painter.layout_job(job);
    }
    // The rows are found on the logical text, whole words at a time, then
    // each row is reordered on its own, so a paragraph's first row still
    // holds its first words. epaint shapes the words itself, so a row's
    // glyphs say nothing reliable about its characters; measuring words is
    // the dependable way to know what fits.
    let width = |piece: &str| {
        painter
            .layout_no_wrap(piece.to_owned(), font.clone(), color)
            .size()
            .x
    };
    let rows = break_rows(text, wrap_width, max_rows, overflow, width);
    let display = rows
        .iter()
        .map(|row| display_line(row))
        .collect::<Vec<_>>()
        .join("\n");
    let mut job = LayoutJob::simple(display, font, color, f32::INFINITY);
    job.halign = Align::RIGHT;
    painter.layout_job(job)
}

/// Fills rows with whole words up to `wrap_width`, at most `max_rows` of
/// them, and ends a cut with `overflow`. The rows come back in logical
/// order, one string each.
fn break_rows(
    text: &str,
    wrap_width: f32,
    max_rows: usize,
    overflow: Option<char>,
    width: impl Fn(&str) -> f32,
) -> Vec<String> {
    let max_rows = max_rows.max(1);
    let space = width(" ");
    let mut rows: Vec<(String, f32)> = Vec::new();
    let mut row = (String::new(), 0.0_f32);
    let mut cut = false;
    'paragraphs: for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            if rows.len() + 1 >= max_rows {
                cut = true;
                break;
            }
            rows.push(std::mem::take(&mut row));
        }
        for word in paragraph.split_whitespace() {
            let word_width = width(word);
            if !row.0.is_empty() && row.1 + space + word_width > wrap_width {
                if rows.len() + 1 >= max_rows {
                    cut = true;
                    break 'paragraphs;
                }
                rows.push(std::mem::take(&mut row));
            }
            if !row.0.is_empty() {
                row.0.push(' ');
                row.1 += space;
            }
            row.0.push_str(word);
            row.1 += word_width;
        }
    }
    rows.push(row);
    // The last row can be wider than the column even when nothing was cut:
    // the first word of a row is taken whatever it measures, because a row
    // has to hold something, and a word longer than the column has nowhere
    // to break. `layout` hands the finished rows to epaint with no wrap
    // width of its own -- the fitting is meant to have happened here -- so a
    // row left too wide is drawn straight past the edge it was given.
    if let Some(mark) = overflow {
        let last = rows.last_mut().expect("one row at least");
        if cut || last.1 > wrap_width {
            // Each candidate is measured as `layout` will draw it: shaped
            // whole, mark included, in display order. Letters join and
            // ligate, so what is left of a word cannot be worked out from
            // the widths of the letters taken away.
            let drawn = |row: &str| width(&display_line(&format!("{row}{mark}")));
            while drawn(&last.0) > wrap_width
                && let Some(at) = last.0.rfind(' ')
            {
                last.0.truncate(at);
            }
            // A single word with no space left to give up: letters go
            // instead, which is the only way the ellipsis can mean anything.
            // A piece is kept only once it has been measured to fit, so what
            // is drawn fits even where a shorter piece joins into a wider
            // form.
            if !last.0.is_empty() && drawn(&last.0) > wrap_width {
                let cuts = cuts(&last.0);
                let (mut fits, mut over) = (0, cuts.len());
                while over - fits > 1 {
                    let middle = (fits + over) / 2;
                    if drawn(&last.0[..cuts[middle]]) <= wrap_width {
                        fits = middle;
                    } else {
                        over = middle;
                    }
                }
                last.0.truncate(cuts[fits]);
            }
            last.0.push(mark);
        }
    }
    rows.into_iter().map(|(text, _)| text).collect()
}

/// Where `word` can be cut, as byte offsets from its start up to but not
/// including its end: never before a mark that rides on the letter ahead of
/// it, and never after a joiner.
fn cuts(word: &str) -> Vec<usize> {
    let mut previous = None;
    word.char_indices()
        .filter(|&(at, character)| {
            let joined = previous == Some('\u{200d}');
            previous = Some(character);
            let rides = matches!(bidi_class(character), BidiClass::NSM | BidiClass::BN);
            at == 0 || !(rides || joined)
        })
        .map(|(at, _)| at)
        .collect()
}

/// Where to paint a galley from [`layout`] so that it sits inside `rect`:
/// its left edge, or its right edge for right-to-left text.
pub fn galley_pos(rect: Rect, galley: &Galley) -> Pos2 {
    match galley.job.halign {
        Align::RIGHT => rect.right_top(),
        Align::Center => rect.center_top(),
        _ => rect.left_top(),
    }
}

/// Paints one line of text centred on `y`, starting at `left`, or ending at
/// `right` when it reads right to left. Returns the painted rect.
pub fn paint_line(
    painter: &Painter,
    left: f32,
    right: f32,
    y: f32,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    if is_rtl(text) {
        painter.text(
            pos2(right, y),
            Align2::RIGHT_CENTER,
            display_text(text),
            font,
            color,
        )
    } else {
        painter.text(pos2(left, y), Align2::LEFT_CENTER, text, font, color)
    }
}
