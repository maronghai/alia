//! Display-only hard-tab layout. Source bytes and glyph indices stay unchanged.
use gpui::{Window, Pixels, SharedString, ShapedLine, TextRun, px};
use std::sync::Arc;

pub(super) const TAB_COLUMNS: usize = 4;

pub(super) fn shape_line(text: SharedString, font_size: Pixels, runs: &[TextRun], cx: &Window) -> ShapedLine {
    if !text.contains('\t') || runs.is_empty() {
        return cx.text_system().shape_line(text, font_size, runs, None);
    }
    let display: SharedString = text.replace('\t', " ").into();
    let mut line = cx.text_system().shape_line(display, font_size, runs, None);
    let mut space_run = runs[0].clone();
    space_run.len = 1;
    let space = cx.text_system().shape_line(" ".into(), font_size, &[space_run], None).width;
    let stop = f32::from(space) * TAB_COLUMNS as f32;
    let mut adjustments = Vec::new();
    let mut shift = px(0.);
    for (index, _) in text.match_indices('\t') {
        let x = line.x_for_index(index) + shift;
        let old_width = line.x_for_index(index + 1) - line.x_for_index(index);
        let advance = tab_advance(f32::from(x), stop);
        let delta = px(advance) - old_width;
        adjustments.push((index, delta));
        shift += delta;
    }
    let mut glyph_runs = line.runs.clone();
    for run in &mut glyph_runs {
        for glyph in &mut run.glyphs {
            for &(index, delta) in &adjustments {
                if glyph.index > index { glyph.position.x += delta; }
            }
        }
    }
    let layout = gpui::LineLayout {
        font_size: line.font_size, width: line.width + shift,
        ascent: line.ascent, descent: line.descent, runs: glyph_runs, len: text.len(),
    };
    *std::ops::DerefMut::deref_mut(&mut line) = Arc::new(layout);
    line
}

pub(super) fn tab_advance(x: f32, stop: f32) -> f32 {
    if stop <= 0.0 { return 0.0; }
    ((x / stop + 0.0001).floor() + 1.0) * stop - x
}

/// Expanded text and display-byte-to-source-byte map, used only for wrapping.
pub(super) fn wrap_text(text: &str) -> (String, Vec<usize>) {
    let mut display = String::new();
    let mut map = Vec::new();
    let mut column = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\t' {
            let count = TAB_COLUMNS - column % TAB_COLUMNS;
            for _ in 0..count { display.push(' '); map.push(index); }
            column += count;
        } else {
            display.push(ch);
            for byte in 0..ch.len_utf8() { map.push(index + byte); }
            column += 1;
        }
    }
    map.push(text.len());
    (display, map)
}
