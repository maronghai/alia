#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![recursion_limit = "256"]

use gpui::*;
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Backspace, Delete, IndentInline, Input, InputEvent, InputState, OutdentInline, Position, RopeExt},
};

const EXPLORER_DEFAULT_WIDTH: f32 = 240.0;

fn explorer_width(requested: f32, viewport: f32) -> f32 {
    let maximum = (viewport - 320.0).clamp(180.0, 600.0);
    requested.clamp(180.0, maximum)
}

pub const APP_NAME: &str = "alia";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

actions!(
    editor,
    [
        NewFile,
        OpenFile,
        SaveFile,
        SaveAs,
        OpenFolder,
        ToggleTree,
        ToggleMinimap,
        RefreshTree
    ]
);

actions!(
    tabs,
    [
        CloseTab,
        NextTab,
        PreviousTab
    ]
);

actions!(
    column,
    [
        ToggleColumnMode,
        ColumnExtendDown,
        ColumnExtendUp,
        ColumnExtendRight,
        ColumnExtendLeft,
        ColumnShiftLeft,
        ColumnShiftRight,
        ColumnShiftUp,
        ColumnShiftDown,
        ColumnEscape
    ]
);

actions!(
    navigation,
    [
        GoToLine,
        GoToLineEscape
    ]
);


/// Parse 1-based target line number for Go-To-Line functionality
pub fn parse_target_line(input: &str, max_lines: usize) -> Option<usize> {
    let clean = input.trim();
    if clean.is_empty() {
        return None;
    }
    let clean = clean.trim_start_matches(':').trim_start_matches('L').trim_start_matches('l').trim();
    clean.parse::<usize>().ok().map(|line| {
        if line == 0 {
            0
        } else {
            line.saturating_sub(1).min(max_lines.saturating_sub(1))
        }
    })
}

/// Determine icon and color for tree explorer items (VSCode / Sublime style)
pub fn file_badge_and_color(name: &str, is_dir: bool, expanded: bool) -> (&'static str, &'static str, u32) {
    if is_dir {
        if expanded {
            ("▾", "📂", 0xf59e0b) // amber
        } else {
            ("▸", "📁", 0xf59e0b) // amber
        }
    } else {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "rs" => ("", "🦀", 0xf97316),       // Rust orange
            "md" | "markdown" => ("", "📝", 0x818cf8), // Markdown indigo
            "toml" | "json" | "yaml" | "yml" => ("", "⚙", 0xfacc15), // Config yellow
            "js" | "ts" | "jsx" | "tsx" | "html" | "css" => ("", "🌐", 0x38bdf8), // Web sky-blue
            "py" => ("", "🐍", 0x3b82f6),       // Python blue
            "c" | "cpp" | "h" | "hpp" => ("", "⚡", 0x60a5fa), // C/C++ blue
            "txt" | "log" => ("", "📄", 0x94a3b8), // Plain slate
            _ => ("", "📄", 0x94a3b8),          // Generic slate
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSelection {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub head_line: usize,
    pub head_col: usize,
}

impl ColumnSelection {
    pub fn new(anchor_line: usize, anchor_col: usize, head_line: usize, head_col: usize) -> Self {
        Self { anchor_line, anchor_col, head_line, head_col }
    }
    pub fn line_range(self) -> (usize, usize) {
        (self.anchor_line.min(self.head_line), self.anchor_line.max(self.head_line))
    }
    pub fn column_range(self) -> (usize, usize) {
        (self.anchor_col.min(self.head_col), self.anchor_col.max(self.head_col))
    }
    pub fn is_empty(self) -> bool {
        self.anchor_line == self.head_line && self.anchor_col == self.head_col
    }
}

pub fn column_selection_rows(selection: ColumnSelection, total_lines: usize) -> std::ops::RangeInclusive<usize> {
    let (start, end) = selection.line_range();
    start.min(total_lines.saturating_sub(1))..=end.min(total_lines.saturating_sub(1))
}

/// Advance the selection head by one row, preserving its anchor and document bounds.
fn extended_column_head(head: usize, total_lines: usize, down: bool) -> usize {
    let last = total_lines.saturating_sub(1);
    if down { head.saturating_add(1).min(last) } else { head.saturating_sub(1).min(last) }
}

/// Character-based navigation for a rectangular selection's active row.
#[derive(Clone, Copy)]
enum ColumnNavigation { Home, End, PreviousWord, NextWord }
fn column_navigation_target(text: &str, column: usize, navigation: ColumnNavigation) -> usize {
    let chars: Vec<char> = text.trim_end_matches(['\r', '\n']).chars().collect();
    let mut at = column.min(chars.len());
    let class = |c: char| if c.is_whitespace() { 0 } else if c.is_alphanumeric() || c == '_' { 1 } else { 2 };
    match navigation {
        ColumnNavigation::Home => 0,
        ColumnNavigation::End => chars.len(),
        ColumnNavigation::PreviousWord => {
            while at > 0 && chars[at-1].is_whitespace() { at -= 1; }
            if at > 0 { let kind = class(chars[at-1]); while at > 0 && class(chars[at-1]) == kind { at -= 1; } }
            at
        }
        ColumnNavigation::NextWord => {
            if at < chars.len() { let kind = class(chars[at]); while at < chars.len() && class(chars[at]) == kind { at += 1; } }
            while at < chars.len() && chars[at].is_whitespace() { at += 1; }
            at
        }
    }
}
fn column_range_after_navigation(left: usize, width: usize, reversed: bool, target: usize, select: bool) -> (usize, usize, bool) {
    if !select { return (target, 0, false); }
    let anchor = if reversed { left + width } else { left };
    (anchor.min(target), anchor.abs_diff(target), target < anchor)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnCursorRowMetrics {
    pub row: usize,
    pub caret_x: f32,
    pub box_x: f32,
    pub box_width: f32,
    pub y: f32,
    pub height: f32,
    pub is_primary: bool,
}

/// Calculate multi-cursor visual positions for each edited row in column mode
pub fn calculate_column_cursors(
    start_line: usize,
    end_line: usize,
    col: usize,
    box_width: usize,
    input_len: usize,
    char_width: f32,
    line_number_width: f32,
    line_height: f32,
    primary_line: usize,
    viewport_y_offset: f32,
) -> Vec<ColumnCursorRowMetrics> {
    let min_line = start_line.min(end_line);
    let max_line = start_line.max(end_line);
    let effective_caret_col = col + box_width + input_len;
    let box_x = line_number_width + (col as f32) * char_width;
    let caret_x = line_number_width + (effective_caret_col as f32) * char_width;
    let box_pixel_width = (box_width as f32) * char_width;
    let mut rows = Vec::with_capacity(max_line - min_line + 1);

    for row in min_line..=max_line {
        let y = viewport_y_offset + (row as f32) * line_height;
        rows.push(ColumnCursorRowMetrics {
            row,
            caret_x,
            box_x,
            box_width: box_pixel_width,
            y,
            height: line_height,
            is_primary: row == primary_line,
        });
    }
    rows
}

// Column editing pure algorithms
pub fn apply_column_insert(
    original: &str,
    start_line: usize,
    end_line: usize,
    col: usize,
    width: usize,
    insert_text: &str,
) -> String {
    let crlf = original.contains("\r\n");
    let normalized = original.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let min_line = start_line.min(end_line);
    let max_line = start_line.max(end_line).min(lines.len().saturating_sub(1));

    let mut out_lines = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i >= min_line && i <= max_line {
            let mut chars: Vec<char> = line.chars().collect();
            if chars.len() < col {
                chars.extend(std::iter::repeat(' ').take(col - chars.len()));
            }
            let left: String = chars[..col].iter().collect();
            let right_start = (col + width).min(chars.len());
            let right: String = chars[right_start..].iter().collect();
            out_lines.push(format!("{}{}{}", left, insert_text, right));
        } else {
            out_lines.push(line.to_string());
        }
    }
    let sep = if crlf { "\r\n" } else { "\n" };
    out_lines.join(sep)
}

pub fn apply_column_numbers(
    original: &str,
    start_line: usize,
    end_line: usize,
    col: usize,
    width: usize,
    start_num: i64,
    step: i64,
    zero_pad: usize,
) -> String {
    let crlf = original.contains("\r\n");
    let normalized = original.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let min_line = start_line.min(end_line);
    let max_line = start_line.max(end_line).min(lines.len().saturating_sub(1));

    let mut out_lines = Vec::with_capacity(lines.len());
    let mut step_idx = 0;
    for (i, line) in lines.iter().enumerate() {
        if i >= min_line && i <= max_line {
            let num = start_num + step_idx * step;
            step_idx += 1;
            let num_str = if zero_pad > 0 {
                format!("{:0width$}", num, width = zero_pad)
            } else {
                format!("{}", num)
            };
            let mut chars: Vec<char> = line.chars().collect();
            if chars.len() < col {
                chars.extend(std::iter::repeat(' ').take(col - chars.len()));
            }
            let left: String = chars[..col].iter().collect();
            let right_start = (col + width).min(chars.len());
            let right: String = chars[right_start..].iter().collect();
            out_lines.push(format!("{}{}{}", left, num_str, right));
        } else {
            out_lines.push(line.to_string());
        }
    }
    let sep = if crlf { "\r\n" } else { "\n" };
    out_lines.join(sep)
}

pub fn apply_column_delete(
    original: &str,
    start_line: usize,
    end_line: usize,
    col: usize,
    width: usize,
) -> String {
    let actual_width = if width == 0 { 1 } else { width };
    apply_column_insert(original, start_line, end_line, col, actual_width, "")
}


pub fn apply_column_outdent(
    original: &str,
    start_line: usize,
    end_line: usize,
    col: usize,
    tab_size: usize,
) -> (String, usize) {
    if col == 0 || tab_size == 0 {
        return (original.to_string(), 0);
    }
    let crlf = original.contains("\r\n");
    let normalized = original.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let min_line = start_line.min(end_line);
    let max_line = end_line.max(start_line).min(lines.len().saturating_sub(1));
    let remove_limit = tab_size.min(col);
    let mut removed = remove_limit;
    for (i, line) in lines.iter().enumerate() {
        if i < min_line || i > max_line {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut count = 0;
        while count < remove_limit && col > count && col - count - 1 < chars.len() && chars[col - count - 1] == ' ' {
            count += 1;
        }
        removed = removed.min(count);
    }
    if removed == 0 {
        return (original.to_string(), 0);
    }
    let mut out_lines = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i < min_line || i > max_line {
            out_lines.push((*line).to_string());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let cut = removed.min(col);
        let left: String = chars[..col.saturating_sub(cut).min(chars.len())].iter().collect();
        let right: String = if col < chars.len() {
            chars[col..].iter().collect()
        } else {
            String::new()
        };
        out_lines.push(format!("{}{}", left, right));
    }
    let sep = if crlf { "\r\n" } else { "\n" };
    (out_lines.join(sep), removed)
}


// Native modal dialogs pump Windows messages. Running them inside a GPUI
// entity update can re-enter the UI while it is mutably borrowed. Keep their
// COM apartment and message loop on a separate thread instead.
fn native_dialog<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("native-file-dialog".into())
        .spawn(operation)
        .map_err(|err| format!("Failed to launch file dialog: {err}"))?
        .join()
        .map_err(|_| "File dialog exited unexpectedly, please try again.".to_string())
}


#[derive(Clone, Debug, PartialEq, Eq)]
struct FlattenedTreeEntry {
    path: PathBuf,
    name: String,
    directory: bool,
    depth: usize,
    expanded: bool,
}

#[derive(Clone)]
struct TreeEntry {
    path: PathBuf,
    name: String,
    directory: bool,
}

fn read_directory(path: &Path) -> Result<Vec<TreeEntry>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        entries.push(TreeEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            directory: kind.is_dir() && !kind.is_symlink(),
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiskStamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(DiskStamp { modified: metadata.modified().ok(), len: metadata.len() })
}

#[derive(Debug, PartialEq, Eq)]
enum ExternalChange { Unchanged, Reload, Conflict }

fn external_change(saved: &str, crlf: bool, bom: bool, dirty: bool, bytes: &[u8]) -> ExternalChange {
    if encode_text(saved, crlf, bom) == bytes { ExternalChange::Unchanged }
    else if dirty { ExternalChange::Conflict }
    else { ExternalChange::Reload }
}

struct Document {
    input: Entity<InputState>,
    path: Option<PathBuf>,
    saved_text: String,
    disk_stamp: Option<DiskStamp>,
    dirty_cached: bool,
    lines: usize,
    chars: usize,
    crlf: bool,
    bom: bool,
    untitled_id: usize,
    minimap_cache: RefCell<Option<Rc<Vec<Vec<(f32, f32, u32)>>>>>,
    _subscription: Subscription,
}


/// Minimap scroll metrics and viewport synchronization
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapScrollInfo {
    pub content_height: f32,
    pub scroll_y: f32,
    pub slider_y: f32,
    pub slider_height: f32,
    pub visible_range: std::ops::Range<usize>,
}

pub const MINIMAP_LINE_HEIGHT: f32 = 2.4;

pub fn calculate_minimap_scroll(
    cursor_line: usize,
    total_lines: usize,
    container_height: f32,
    line_height: f32,
) -> MinimapScrollInfo {
    let total_lines = total_lines.max(1);
    let line_height = line_height.max(1.0);
    let content_height = total_lines as f32 * line_height;
    let container_height = container_height.max(1.0);

    let (scroll_y, _max_scroll) = if content_height <= container_height {
        (0.0, 0.0)
    } else {
        let max_s = content_height - container_height;
        let progress = (cursor_line as f32) / (total_lines.saturating_sub(1).max(1) as f32);
        ((progress * max_s).clamp(0.0, max_s), max_s)
    };

    let approx_editor_lines = (container_height / 20.0).clamp(12.0, 80.0);
    let slider_height = (approx_editor_lines * line_height).clamp(24.0, container_height);

    let cursor_y = (cursor_line as f32 * line_height) - scroll_y;
    let slider_y = (cursor_y - slider_height * 0.5).clamp(0.0, (container_height - slider_height).max(0.0));

    let start_row = (scroll_y / line_height).floor().max(0.0) as usize;
    let end_row = (((scroll_y + container_height) / line_height).ceil() as usize).min(total_lines);

    MinimapScrollInfo {
        content_height,
        scroll_y,
        slider_y,
        slider_height,
        visible_range: start_row..end_row,
    }
}

pub fn minimap_coord_to_line(
    y_in_canvas: f32,
    scroll_y: f32,
    line_height: f32,
    total_lines: usize,
) -> usize {
    if total_lines == 0 || line_height <= 0.0 {
        return 0;
    }
    let absolute_y = (y_in_canvas + scroll_y).max(0.0);
    let line = (absolute_y / line_height).round() as usize;
    line.min(total_lines.saturating_sub(1))
}

/// Minimap display metrics and pure calculation helpers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapLineMetrics {
    pub indent_px: f32,
    pub width_px: f32,
    pub is_empty: bool,
}

pub fn calculate_minimap_line_metrics(line: &str, max_width_px: f32) -> MinimapLineMetrics {
    let trimmed_start = line.trim_start();
    if trimmed_start.is_empty() {
        return MinimapLineMetrics {
            indent_px: 0.0,
            width_px: 0.0,
            is_empty: true,
        };
    }
    let leading_spaces = line.len() - trimmed_start.len();
    // 1 space ~= 1.5px indent, capped at 40px
    let indent_px = ((leading_spaces as f32) * 1.5).min(40.0);
    // Non-space length scale: roughly 0.8px per char, capped so indent + width <= max_width_px
    let raw_width = (trimmed_start.len() as f32) * 0.9;
    let width_px = raw_width.min((max_width_px - indent_px).max(8.0));

    MinimapLineMetrics {
        indent_px,
        width_px,
        is_empty: false,
    }
}

pub fn calculate_minimap_viewport(
    cursor_line: usize,
    total_lines: usize,
    viewport_row_span: usize,
) -> (usize, usize) {
    if total_lines == 0 {
        return (0, 0);
    }
    let half_span = viewport_row_span / 2;
    let start = cursor_line.saturating_sub(half_span);
    let end = (start + viewport_row_span).min(total_lines);
    let start = if end == total_lines && total_lines > viewport_row_span {
        total_lines - viewport_row_span
    } else {
        start
    };
    (start, end)
}

pub fn minimap_click_to_line(
    click_y: f32,
    minimap_height: f32,
    total_lines: usize,
) -> usize {
    if minimap_height <= 0.0 || total_lines == 0 {
        return 0;
    }
    let ratio = (click_y / minimap_height).clamp(0.0, 1.0);
    let target = (ratio * (total_lines.saturating_sub(1) as f32)).round() as usize;
    target.min(total_lines.saturating_sub(1))
}

// Merge adjacent same-color glyphs; bound scene submissions per sampled line.
pub fn minimap_segments(text: &str) -> Vec<(f32, f32, u32)> {
    let mut result: Vec<(f32, f32, u32)> = Vec::new();
    let mut column = 0usize;
    let mut quoted = false;
    let trimmed = text.trim_start();
    let comment = trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("<!--");
    for ch in text.chars().take(160) {
        if column >= 100 { break; }
        if ch == '\t' { column += 4; continue; }
        if ch == '"' || ch == '\'' { quoted = !quoted; }
        if !ch.is_whitespace() {
            let color = if comment { 0x6e9277 } else if quoted { 0xc9b482 }
                else if ch.is_ascii_digit() { 0xbfa0d5 }
                else if "{}[]();:=<>".contains(ch) { 0x75b9dc }
                else { 0x9caec9 };
            let x = column as f32 * 1.25;
            let at_limit = result.len() >= 16;
            if let Some(last) = result.last_mut() {
                if (last.2 == color && x - (last.0 + last.1) < 0.4) || at_limit {
                    last.1 = x + 0.9 - last.0;
                    column += 1;
                    continue;
                }
            }
            result.push((x, 0.9, color));
        }
        column += 1;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTabsKind {
    All,
    Others,
    Right,
    Left,
}

pub fn calculate_tabs_to_close_indices(
    kind: CloseTabsKind,
    target_index: usize,
    total_tabs: usize,
) -> Vec<usize> {
    match kind {
        CloseTabsKind::All => (0..total_tabs).rev().collect(),
        CloseTabsKind::Others => {
            let mut indices = Vec::new();
            if target_index < total_tabs {
                for i in (target_index + 1..total_tabs).rev() {
                    indices.push(i);
                }
                for i in (0..target_index).rev() {
                    indices.push(i);
                }
            }
            indices
        }
        CloseTabsKind::Right => {
            if target_index + 1 < total_tabs {
                (target_index + 1..total_tabs).rev().collect()
            } else {
                Vec::new()
            }
        }
        CloseTabsKind::Left => {
            if target_index < total_tabs {
                (0..target_index).rev().collect()
            } else {
                Vec::new()
            }
        }
    }
}

#[derive(Clone, Debug)]
struct TabContextMenu {
    tab_index: usize,
    position: Point<Pixels>,
}

struct Editor {
    tabs: Vec<Document>,
    active_tab: usize,
    next_untitled_id: usize,
    tree_root: Option<PathBuf>,
    tree_cache: HashMap<PathBuf, Vec<TreeEntry>>,
    expanded: HashSet<PathBuf>,
    tree_visible: bool,
    tree_width: f32,
    tree_resize: Option<(f32, f32)>,
    minimap_visible: bool,
    minimap_dragging: bool,
    tree_scroll_handle: UniformListScrollHandle,
    tab_scroll_handle: ScrollHandle,
    flattened_tree: Vec<FlattenedTreeEntry>,
    scroll_tree_to_active: Cell<bool>,
    message: String,
    last_window_title: String,
    tab_context_menu: Option<TabContextMenu>,
    column_mode: bool,
    column_start_line: usize,
    column_end_line: usize,
    column_col: usize,
    column_width: usize,
    column_reversed: bool,
    /// Vertical content offset mirrored from the editor viewport while column mode is active.
    goto_line_open: bool,
    goto_line_input: Entity<InputState>,
}

impl std::ops::Deref for Editor {
    type Target = Document;
    fn deref(&self) -> &Document {
        &self.tabs[self.active_tab]
    }
}
impl std::ops::DerefMut for Editor {
    fn deref_mut(&mut self) -> &mut Document {
        &mut self.tabs[self.active_tab]
    }
}

// Compare Rope chunks without materializing the full document.
fn text_matches_saved(input: &InputState, saved: &str) -> bool {
    let rope = input.text();
    if rope.len() != saved.len() {
        return false;
    }
    let mut offset = 0;
    for chunk in rope.chunks() {
        if saved.as_bytes().get(offset..offset + chunk.len()) != Some(chunk.as_bytes()) {
            return false;
        }
        offset += chunk.len();
    }
    true
}

fn normalized_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
fn same_path(a: &Path, b: &Path) -> bool {
    normalized_path(a) == normalized_path(b)
}

fn ancestor_dirs_under_root(child: &Path, root: &Path) -> Vec<PathBuf> {
    let norm_child = normalized_path(child);
    let norm_root = normalized_path(root);

    let Ok(rel) = norm_child.strip_prefix(&norm_root) else {
        return Vec::new();
    };

    let mut ancestors = Vec::new();
    let mut current = norm_root;
    let components: Vec<_> = rel.components().collect();
    if components.len() > 1 {
        for component in &components[..components.len() - 1] {
            current = current.join(component);
            ancestors.push(current.clone());
        }
    }
    ancestors
}

fn next_tab_index(active: usize, count: usize, forward: bool) -> usize {
    if forward {
        (active + 1) % count
    } else {
        (active + count - 1) % count
    }
}


#[cfg_attr(not(test), allow(dead_code))]
fn calculate_tree_scroll_offset(
    row_index: usize,
    total_rows: usize,
    row_height: f32,
    viewport_height: f32,
    current_offset_y: f32,
) -> Option<f32> {
    if viewport_height <= 0.0 || total_rows == 0 {
        return None;
    }
    let content_height = total_rows as f32 * row_height;
    let max_scroll = (content_height - viewport_height).max(0.0);
    let item_top_in_content = row_index as f32 * row_height;
    let item_bottom_in_content = item_top_in_content + row_height;

    let item_top_in_viewport = item_top_in_content + current_offset_y;
    let item_bottom_in_viewport = item_bottom_in_content + current_offset_y;

    let margin = row_height * 1.5;
    if item_top_in_viewport >= margin && item_bottom_in_viewport <= (viewport_height - margin) {
        return None;
    }

    let ideal_offset_y = (viewport_height - row_height) / 2.0 - item_top_in_content;
    let target_offset_y = ideal_offset_y.clamp(-max_scroll, 0.0);
    Some(target_offset_y)
}

impl Editor {
    fn create_document(
        untitled_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Document {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .line_number(true)
                .placeholder("Start typing, or press Ctrl+O to open a file...")
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                input.update(cx, |state, cx| state.refresh_highlighter(cx));
                if let Some(doc) = this.tabs.iter_mut().find(|doc| doc.input == *input) {
                    *doc.minimap_cache.borrow_mut() = None;
                    let state = input.read(cx);
                    doc.dirty_cached = !text_matches_saved(state, &doc.saved_text);
                    doc.lines = state.text().chunks().map(|chunk| chunk.bytes().filter(|b| *b == b'\n').count()).sum::<usize>() + 1;
                    doc.chars = state.text().chunks().map(|chunk| chunk.chars().count()).sum();
                }
                cx.notify();
            }
        });
        Document {
            input,
            path: None,
            saved_text: String::new(),
            disk_stamp: None,
            dirty_cached: false,
            lines: 1,
            chars: 0,
            crlf: true,
            bom: false,
            untitled_id,
            minimap_cache: RefCell::new(None),
            _subscription: subscription,
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let document = Self::create_document(1, window, cx);
        document
            .input
            .update(cx, |state, cx| state.focus(window, cx));
        let tree_root = std::env::current_dir().ok().map(|p| normalized_path(&p));
        let mut tree_cache = HashMap::new();
        if let Some(root) = &tree_root {
            if let Ok(entries) = read_directory(root) {
                tree_cache.insert(root.clone(), entries);
            }
        }
        let mut editor = Self {
            tabs: vec![document],
            active_tab: 0,
            next_untitled_id: 2,
            tree_root,
            tree_cache,
            expanded: HashSet::new(),
            tree_visible: true,
            tree_width: EXPLORER_DEFAULT_WIDTH,
            tree_resize: None,
            minimap_visible: true,
            minimap_dragging: false,
            tree_scroll_handle: UniformListScrollHandle::new(),
            tab_scroll_handle: ScrollHandle::new(),
            flattened_tree: Vec::new(),
            scroll_tree_to_active: Cell::new(true),
            message: "Ready".into(),
            last_window_title: String::new(),
            tab_context_menu: None,
            column_mode: false,
            column_start_line: 0,
            column_end_line: 0,
            column_col: 0,
            column_width: 0,
            column_reversed: false,
            goto_line_open: false,
            goto_line_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Go to line (e.g. 42, press Enter)...")
            }),
        };
        editor.rebuild_flattened_tree();
        cx.spawn_in(window, async move |view, cx| {
            loop {
                Timer::after(std::time::Duration::from_secs(1)).await;
                let Ok(requests) = view.update(cx, |editor, _| {
                    editor.tabs.iter().filter_map(|doc| doc.path.clone().map(|path|
                        (doc.untitled_id, path, doc.disk_stamp.clone()))).collect::<Vec<_>>()
                }) else { break; };
                let changes = cx.background_executor().spawn(async move {
                    requests.into_iter().filter_map(|(id, path, previous)| {
                        let stamp = disk_stamp(&path);
                        if stamp == previous { return None; }
                        let bytes = fs::read(&path);
                        Some((id, path, previous, stamp, bytes))
                    }).collect::<Vec<_>>()
                }).await;
                if cx.update(|window, cx| {
                    let _ = view.update(cx, |editor, cx| {
                        for (id, path, previous, stamp, bytes) in changes {
                            let Some(index) = editor.tabs.iter().position(|doc|
                                doc.untitled_id == id && doc.path.as_ref() == Some(&path)) else { continue; };
                            let doc = &mut editor.tabs[index];
                            // A save/open may have completed while the background read was in flight.
                            if doc.disk_stamp != previous { continue; }
                            doc.disk_stamp = stamp;
                            let bytes = match bytes {
                                Ok(bytes) => bytes,
                                Err(_) => {
                                    editor.message = format!("File removed or unavailable: {} — local content kept", path.display());
                                    cx.notify();
                                    continue;
                                }
                            };
                            match external_change(&doc.saved_text, doc.crlf, doc.bom,
                                !text_matches_saved(doc.input.read(cx), &doc.saved_text), &bytes) {
                                ExternalChange::Unchanged => {},
                                ExternalChange::Conflict => {
                                    editor.message = format!("External changes: {} — local edits kept; save to resolve", path.display());
                                    cx.notify();
                                }
                                ExternalChange::Reload => match decode_text(&bytes) {
                                    Ok((text, crlf, bom)) => {
                                        let cursor = doc.input.read(cx).cursor_position();
                                        doc.lines = text.bytes().filter(|b| *b == b'\n').count() + 1;
                                        doc.chars = text.chars().count();
                                        doc.saved_text = text.clone();
                                        doc.crlf = crlf;
                                        doc.bom = bom;
                                        doc.dirty_cached = false;
                                        *doc.minimap_cache.borrow_mut() = None;
                                        doc.input.update(cx, |state, cx| {
                                            state.set_value(text, window, cx);
                                            state.set_cursor_position(cursor, window, cx);
                                            state.refresh_highlighter(cx);
                                        });
                                        editor.message = format!("Reloaded external changes: {}", path.display());
                                        cx.notify();
                                    }
                                    Err(err) => {
                                        editor.message = format!("Cannot reload {}: {err} — local content kept", path.display());
                                        cx.notify();
                                    }
                                }
                            }
                        }
                    });
                }).is_err() { break; }
            }
        }).detach();
        editor
    }

    fn reveal_active_in_tree(&mut self) {
        let Some(current_path) = self.path.clone() else {
            return;
        };
        let Some(root) = self.tree_root.clone() else {
            return;
        };
        let ancestors = ancestor_dirs_under_root(&current_path, &root);
        let mut changed = false;
        for dir in ancestors {
            if self.expanded.insert(dir.clone()) {
                changed = true;
            }
            if !self.tree_cache.contains_key(&dir) {
                if let Ok(entries) = read_directory(&dir) {
                    self.tree_cache.insert(dir, entries);
                    changed = true;
                }
            }
        }
        if changed || self.flattened_tree.is_empty() {
            self.rebuild_flattened_tree();
        }
        self.scroll_tree_to_active.set(true);
    }

    fn update_language(&mut self, cx: &mut Context<Self>) {
        let language = detect_language(self.path.as_deref()).0;
        self.input
            .update(cx, |state, cx| state.set_highlighter(language, cx));
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
    fn dirty(&self, cx: &App) -> bool {
        !text_matches_saved(self.input.read(cx), &self.saved_text)
    }

    fn error(&mut self, message: String, cx: &mut Context<Self>) {
        self.message = message.clone();
        let _ = native_dialog(move || {
            MessageDialog::new()
                .set_title("Operation Failed")
                .set_level(MessageLevel::Error)
                .set_description(&message)
                .show()
        });
        cx.notify();
    }

    fn save(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let path = if save_as || self.path.is_none() {
            let current_path = self.path.clone();
            let selected = native_dialog(move || {
                let mut dialog = FileDialog::new()
                    .set_title("Save Text File")
                    .add_filter("Text Files", &["txt"])
                    .add_filter("All Files", &["*"]);
                if let Some(path) = &current_path {
                    if let Some(parent) = path.parent() {
                        dialog = dialog.set_directory(parent);
                    }
                    if let Some(name) = path.file_name() {
                        dialog = dialog.set_file_name(name.to_string_lossy());
                    }
                } else {
                    dialog = dialog.set_file_name("Untitled.txt");
                }
                dialog.save_file()
            });
            match selected {
                Ok(Some(path)) => path,
                Ok(None) => return false,
                Err(err) => {
                    self.error(err, cx);
                    return false;
                }
            }
        } else {
            self.path.clone().unwrap()
        };
        let path = normalized_path(&path);
        if self
            .tabs
            .iter()
            .enumerate()
            .any(|(i, tab)| i != self.active_tab && tab.path.as_ref() == Some(&path))
        {
            self.error(
                "This file is already open in another tab. Please switch to that tab to save.".into(),
                cx,
            );
            return false;
        }
        if self.path.as_ref() == Some(&path) {
            let disk = fs::read(&path);
            let conflict = match &disk {
                Ok(bytes) => external_change(&self.saved_text, self.crlf, self.bom, true, bytes)
                    == ExternalChange::Conflict,
                Err(_) => true,
            };
            if conflict {
                let name = path.display().to_string();
                let decision = native_dialog(move || MessageDialog::new()
                    .set_title("File Changed on Disk")
                    .set_description(format!("{name} was changed or removed outside alia. Overwrite with your local content? Choose No to keep both versions unchanged."))
                    .set_level(MessageLevel::Warning)
                    .set_buttons(MessageButtons::YesNo).show());
                if !matches!(decision, Ok(MessageDialogResult::Yes)) { return false; }
            }
        }
        let text = self.text(cx);
        let output = encode_text(&text, self.crlf, self.bom);
        if let Err(err) = fs::write(&path, output) {
            self.error(format!("Cannot save file {}: {err}", path.display()), cx);
            return false;
        }
        self.disk_stamp = disk_stamp(&path);
        self.path = Some(path);
        self.update_language(cx);
        self.saved_text = text;
        self.dirty_cached = false;
        self.reveal_active_in_tree();
        self.message = "Saved".into();
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
        true
    }

    fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.dirty(cx) {
            return true;
        }
        let name = self.document_name(self.active_tab);
        let decision = native_dialog(move || {
            MessageDialog::new()
            .set_title("Save Changes")
            .set_description(format!("Document \"{name}\" has unsaved changes. Do you want to save first?\nChoose 'No' to discard, or 'Cancel' to return."))
            .set_level(MessageLevel::Warning)
            .set_buttons(MessageButtons::YesNoCancel)
            .show()
        });
        match decision {
            Ok(MessageDialogResult::Yes) => self.save(false, window, cx),
            Ok(MessageDialogResult::No) => true,
            Err(err) => {
                self.error(err, cx);
                false
            }
            _ => false,
        }
    }

    fn document_name(&self, index: usize) -> String {
        let doc = &self.tabs[index];
        doc.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("Untitled-{}.txt", doc.untitled_id))
    }

    fn switch_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active_tab = index;
        self.tab_scroll_handle.scroll_to_item(index);
        self.input.update(cx, |state, cx| state.focus(window, cx));
        self.reveal_active_in_tree();
        cx.notify();
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let doc = Self::create_document(self.next_untitled_id, window, cx);
        self.next_untitled_id += 1;
        self.tabs.push(doc);
        self.switch_tab(self.tabs.len() - 1, window, cx);
        self.message = "New document".into();
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        let previous = self.active_tab;
        self.switch_tab(index, window, cx);
        if !self.confirm_discard(window, cx) {
            self.switch_tab(previous, window, cx);
            return false;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_file(&NewFile, window, cx);
        } else {
            let next = if previous == index {
                index.min(self.tabs.len() - 1)
            } else if previous > index {
                previous - 1
            } else {
                previous
            };
            self.switch_tab(next, window, cx);
        }
        self.message = "Tab closed".into();
        cx.notify();
        true
    }

    fn open_tab_context_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index < self.tabs.len() {
            self.switch_tab(index, window, cx);
            self.tab_context_menu = Some(TabContextMenu {
                tab_index: index,
                position,
            });
            cx.notify();
        }
    }

    fn batch_close_tabs(
        &mut self,
        kind: CloseTabsKind,
        target_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_context_menu = None;
        let indices = calculate_tabs_to_close_indices(kind, target_index, self.tabs.len());
        for index in indices {
            if index < self.tabs.len() {
                if !self.close_tab_at(index, window, cx) {
                    break;
                }
            }
        }
        if !self.tabs.is_empty() {
            let active = self.active_tab.min(self.tabs.len() - 1);
            self.switch_tab(active, window, cx);
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }
    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_tab(
            next_tab_index(self.active_tab, self.tabs.len(), true),
            window,
            cx,
        );
    }
    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_tab(
            next_tab_index(self.active_tab, self.tabs.len(), false),
            window,
            cx,
        );
    }
    fn confirm_close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let previous = self.active_tab;
        for index in 0..self.tabs.len() {
            self.switch_tab(index, window, cx);
            if !self.confirm_discard(window, cx) {
                return false;
            }
        }
        self.switch_tab(previous, window, cx);
        true
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tab-bar")
            .flex()
            .w_full()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll_handle)
            .bg(rgb(0x13161f))
            .border_b_1()
            .border_color(rgb(0x232838))
            .children(self.tabs.iter().enumerate().map(|(index, doc)| {
                let dirty = doc.dirty_cached;
                let name = self.document_name(index);
                div()
                    .id(("document-tab", index))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .min_w(px(120.))
                    .max_w(px(220.))
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if index == self.active_tab {
                        0x1a1e29
                    } else {
                        0x13161f
                    }))
                    .hover(|style| style.bg(rgb(0x1b202b)))
                    .border_t_2()
                    .border_color(rgb(if index == self.active_tab { 0x3b82f6 } else { 0x13161f }))
                    .border_r_1()
                    .border_color(rgb(0x232838))
                    .on_click(cx.listener(move |this, _, w, cx| this.switch_tab(index, w, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, w, cx| {
                            this.open_tab_context_menu(index, event.position, w, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.close_tab_at(index, w, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!("{}{}", if dirty { "● " } else { "" }, name)),
                    )
                    .child(
                        div()
                            .id(("close-tab", index))
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgb(0x303746)).text_color(rgb(0xf8fafc)))
                            .child("×")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(index, w, cx);
                            })),
                    )
            }))
            .child(
                div()
                    .id("tab-bar-empty-area")
                    .flex_1()
                    .h_full()
                    .min_w(px(80.))
                    .cursor_default()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, w, cx| {
                            if event.click_count >= 2 {
                                this.new_file(&NewFile, w, cx);
                            }
                        }),
                    ),
            )
    }

    fn context_menu_item(
        &self,
        id_name: &'static str,
        label: &'static str,
        enabled: bool,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if enabled {
            let action = std::rc::Rc::new(action);
            let action_down = action.clone();
            let action_click = action;
            div()
                .id(id_name)
                .px_3()
                .py_1p5()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(rgb(0xe2e8f0))
                .hover(|s| s.bg(rgb(0x2a3244)).text_color(rgb(0xffffff)))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        action_down(this, w, cx);
                    }),
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    action_click(this, w, cx);
                }))
        } else {
            div()
                .id(id_name)
                .px_3()
                .py_1p5()
                .rounded(px(4.))
                .text_color(rgb(0x56617a))
                .cursor_not_allowed()
                .child(label)
        }
    }


    fn toggle_bom_active(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.bom = !self.bom;
        self.dirty_cached = true;
        self.message = if self.bom {
            "Encoding changed: UTF-8 with BOM (Bompad)".into()
        } else {
            "Encoding changed: UTF-8 without BOM".into()
        };
        cx.notify();
    }

    fn toggle_crlf_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new_crlf = !self.crlf;
        let current_text = self.text(cx);
        let converted = if new_crlf {
            current_text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            current_text.replace("\r\n", "\n")
        };
        self.crlf = new_crlf;
        self.input.update(cx, |state, cx| {
            state.set_value(converted, window, cx);
        });
        self.dirty_cached = true;
        self.message = if new_crlf {
            "Line endings: CRLF (Windows)".into()
        } else {
            "Line endings: LF (Unix)".into()
        };
        cx.notify();
    }

    fn open_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goto_line_open = true;
        let total = self.lines;
        let current_line = self.input.read(cx).cursor_position().line + 1;
        self.goto_line_input.update(cx, |state, cx| {
            state.set_value(format!("{}", current_line), window, cx);
            state.focus(window, cx);
        });
        self.message = format!("Go to line (1..{})", total);
        cx.notify();
    }

    fn close_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto_line_open {
            self.goto_line_open = false;
            self.input.update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    fn apply_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let val = self.goto_line_input.read(cx).value().to_string();
        if let Some(target_line) = parse_target_line(&val, self.lines) {
            self.input.update(cx, |state, cx| {
                state.set_cursor_position(Position { line: target_line as u32, character: 0 }, window, cx);
                state.focus(window, cx);
            });
            self.message = format!("Jumped to line {}", target_line + 1);
            self.goto_line_open = false;
            cx.notify();
        } else {
            self.message = "Invalid line number".into();
            cx.notify();
        }
    }

    fn goto_line_panel(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_lines = self.lines;
        deferred(
            anchored()
                .position(Point::new(px(0.), px(0.)))
                .snap_to_window()
                .child(
                    div()
                        .size_full()
                        .bg(rgba(0x00000066))
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(40.))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, w, cx| {
                                this.close_goto_line(w, cx);
                            }),
                        )
                        .child(
                            div()
                                .w(px(460.))
                                .bg(rgb(0x1a1e28))
                                .border_1()
                                .border_color(rgb(0x3b82f6))
                                .rounded(px(6.))
                                .shadow_lg()
                                .p_3()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .items_center()
                                        .mb_2()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(0x60a5fa))
                                                .child(format!("GO TO LINE : (1 .. {})", total_lines)),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x94a3b8))
                                                .child("Press Enter to jump, Esc to cancel"),
                                        ),
                                )
                                .child(
                                    div()
                                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, w, cx| {
                                            if event.keystroke.key == "enter" {
                                                this.apply_goto_line(w, cx);
                                            } else if event.keystroke.key == "escape" {
                                                this.close_goto_line(w, cx);
                                            }
                                        }))
                                        .child(
                                            Input::new(&self.goto_line_input)
                                                .appearance(true)
                                                .cleanable(false)
                                                .font_family("Cascadia Code")
                                                .text_size(px(14.)),
                                        ),
                                ),
                        ),
                ),
        )
    }

    fn tab_context_menu_panel(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = match &self.tab_context_menu {
            Some(m) => m.clone(),
            None => return div().into_any_element(),
        };
        let target_index = menu.tab_index;
        let total_tabs = self.tabs.len();
        let has_left = target_index > 0;
        let has_right = target_index + 1 < total_tabs;
        let has_others = total_tabs > 1;

        let menu_width = px(200.);
        let menu_height = px(180.);
        let window_size = window.bounds().size;
        let pos_x = if menu.position.x + menu_width > window_size.width {
            (window_size.width - menu_width - px(8.)).max(px(0.))
        } else {
            menu.position.x
        };
        let pos_y = if menu.position.y + menu_height > window_size.height {
            (window_size.height - menu_height - px(8.)).max(px(0.))
        } else {
            menu.position.y
        };

        deferred(
            anchored().child(
                div()
                    .id("context-menu-layer")
                    .absolute()
                    .size_full()
                    .top_0()
                    .left_0()
                    .child(
                        div()
                            .id("context-menu-backdrop")
                            .size_full()
                            .absolute()
                            .top_0()
                            .left_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.tab_context_menu = None;
                                cx.notify();
                            }))
                            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                                this.tab_context_menu = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("tab-context-menu-popup")
                            .occlude()
                            .absolute()
                            .left(pos_x)
                            .top(pos_y)
                            .w(menu_width)
                            .bg(rgb(0x1a1e28))
                            .border_1()
                            .border_color(rgb(0x323a4e))
                            .rounded(px(6.))
                            .shadow_lg()
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_sm()
                            .child(self.context_menu_item(
                                "menu-close-tab",
                                "Close Tab",
                                true,
                                move |this, w, cx| {
                                    this.tab_context_menu = None;
                                    this.close_tab_at(target_index, w, cx);
                                },
                                cx,
                            ))
                            .child(self.context_menu_item(
                                "menu-close-others",
                                "Close Other Tabs",
                                has_others,
                                move |this, w, cx| {
                                    this.batch_close_tabs(CloseTabsKind::Others, target_index, w, cx);
                                },
                                cx,
                            ))
                            .child(self.context_menu_item(
                                "menu-close-right",
                                "Close Tabs to the Right",
                                has_right,
                                move |this, w, cx| {
                                    this.batch_close_tabs(CloseTabsKind::Right, target_index, w, cx);
                                },
                                cx,
                            ))
                            .child(self.context_menu_item(
                                "menu-close-left",
                                "Close Tabs to the Left",
                                has_left,
                                move |this, w, cx| {
                                    this.batch_close_tabs(CloseTabsKind::Left, target_index, w, cx);
                                },
                                cx,
                            ))
                            .child(
                                div()
                                    .h(px(1.))
                                    .bg(rgb(0x283042))
                                    .my_1()
                                    .mx_1(),
                            )
                            .child(self.context_menu_item(
                                "menu-close-all",
                                "Close All Tabs",
                                true,
                                move |this, w, cx| {
                                    this.batch_close_tabs(CloseTabsKind::All, target_index, w, cx);
                                },
                                cx,
                            )),
                    ),
            ),
        )
        .into_any_element()
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let selected = native_dialog(|| {
            FileDialog::new()
                .set_title("Open Text File")
                .add_filter(
                    "Text & Code",
                    &[
                        "txt", "md", "rs", "toml", "json", "csv", "log", "html", "css", "js", "py",
                    ],
                )
                .add_filter("All Files", &["*"])
                .pick_files()
        });
        let paths = match selected {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(err) => {
                self.error(err, cx);
                return;
            }
        };
        for path in paths {
            self.open_path(path, window, cx);
        }
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalized_path(&path);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path.as_ref() == Some(&path))
        {
            self.switch_tab(index, window, cx);
            return;
        }
        let decoded = fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode_text(&bytes));
        match decoded {
            Ok((text, crlf, bom)) => {
                let reuse_blank =
                    self.path.is_none() && self.saved_text.is_empty() && self.text(cx).is_empty();
                if !reuse_blank {
                    self.new_file(&NewFile, window, cx);
                }
                self.disk_stamp = disk_stamp(&path);
                self.path = Some(path);
                self.update_language(cx);
                self.lines = text.bytes().filter(|b| *b == b'\n').count() + 1;
                self.chars = text.chars().count();
                self.dirty_cached = false;
                self.saved_text = text.clone();
                self.crlf = crlf;
                self.bom = bom;
                self.input.update(cx, |state, cx| {
                    state.set_value(text, window, cx);
                    state.refresh_highlighter(cx);
                    state.focus(window, cx);
                });
                self.reveal_active_in_tree();
                self.message = "File opened".into();
                cx.notify();
            }
            Err(err) => self.error(format!("Cannot open file {}: {err}", path.display()), cx),
        }
    }

    fn open_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        let initial = self.tree_root.clone();
        match native_dialog(move || {
            let mut dialog = FileDialog::new().set_title("Open Folder");
            if let Some(root) = initial {
                dialog = dialog.set_directory(root);
            }
            dialog.pick_folder()
        }) {
            Ok(Some(root)) => match read_directory(&root) {
                Ok(entries) => {
                    let root = normalized_path(&root);
                    self.tree_cache.clear();
                    self.expanded.clear();
                    if let Ok(entries) = read_directory(&root) {
                        self.tree_cache.insert(root.clone(), entries);
                    } else {
                        self.tree_cache.insert(root.clone(), entries);
                    }
                    self.tree_root = Some(root);
                    self.reveal_active_in_tree();
                    self.rebuild_flattened_tree();
                    self.tree_visible = true;
                    self.message = "Folder opened".into();
                }
                Err(err) => self.message = format!("Cannot read folder: {err}"),
            },
            Ok(None) => return,
            Err(err) => self.message = err,
        }
        cx.notify();
    }

    fn toggle_tree(&mut self, _: &ToggleTree, _: &mut Window, cx: &mut Context<Self>) {
        self.tree_visible = !self.tree_visible;
        cx.notify();
    }

    fn refresh_tree(&mut self, _: &RefreshTree, _: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.tree_root.clone() else {
            return;
        };
        let mut paths: Vec<_> = self.expanded.iter().cloned().collect();
        paths.push(root);
        let mut errors = Vec::new();
        for path in paths {
            match read_directory(&path) {
                Ok(entries) => {
                    self.tree_cache.insert(path, entries);
                }
                Err(err) => {
                    self.tree_cache.remove(&path);
                    errors.push(format!("{}：{err}", path.display()));
                }
            }
        }
        self.rebuild_flattened_tree();
        self.message = if errors.is_empty() {
            "Explorer refreshed".into()
        } else {
            format!("Refresh failed: {}", errors.join("; "))
        };
        cx.notify();
    }

    fn rebuild_flattened_tree(&mut self) {
        let mut rows = Vec::new();
        if let Some(root) = &self.tree_root {
            self.collect_flattened_tree(root, 0, &mut rows);
        }
        self.flattened_tree = rows;
    }

    fn collect_flattened_tree(&self, path: &Path, depth: usize, rows: &mut Vec<FlattenedTreeEntry>) {
        if let Some(entries) = self.tree_cache.get(path) {
            for entry in entries {
                let expanded = entry.directory && self.expanded.contains(&entry.path);
                rows.push(FlattenedTreeEntry {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    directory: entry.directory,
                    depth,
                    expanded,
                });
                if expanded {
                    self.collect_flattened_tree(&entry.path, depth + 1, rows);
                }
            }
        }
    }

    fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path_buf = path.to_path_buf();
        if self.expanded.remove(&path_buf) {
            self.rebuild_flattened_tree();
            cx.notify();
            return;
        }
        match read_directory(&path_buf) {
            Ok(entries) => {
                self.tree_cache.insert(path_buf.clone(), entries);
                self.expanded.insert(path_buf);
                self.rebuild_flattened_tree();
            }
            Err(err) => self.message = format!("Cannot read directory: {err}"),
        }
        cx.notify();
    }

    fn tree_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("explorer-resize-handle")
            .w(px(6.)).h_full().flex_shrink_0()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(if self.tree_resize.is_some() { rgb(0x3b82f6) } else { rgb(0x232838) })
            .hover(|style| style.bg(rgb(0x3b82f6)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if event.click_count >= 2 {
                    this.tree_width = EXPLORER_DEFAULT_WIDTH;
                    this.tree_resize = None;
                } else {
                    this.tree_resize = Some((f32::from(event.position.x), this.tree_width));
                }
                cx.notify();
            }))
    }

    fn tree_panel(&self, cx: &mut Context<Self>) -> Div {
        let root_name = self
            .tree_root
            .as_ref()
            .map(|p| {
                p.file_name()
                    .unwrap_or(p.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or("NO FOLDER OPENED".into());

        let active_path = self.path.clone();
        if self.scroll_tree_to_active.take() {
            if let Some(target_index) = self.flattened_tree.iter().position(|entry| {
                active_path.as_ref() == Some(&entry.path)
            }) {
                self.tree_scroll_handle.scroll_to_item(target_index, ScrollStrategy::Center);
            }
        }

        let count = self.flattened_tree.len();

        div()
            .w(px(self.tree_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x151821))
            .border_r_1()
            .border_color(rgb(0x232838))
            .child(
                div()
                    .px_3()
                    .py_3()
                    .text_xs()
                    .text_color(rgb(0x94a3b8))
                    .font_weight(FontWeight::BOLD)
                    .child("EXPLORER"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .pb_2()
                    .child(tool("tree-open", "Open Folder").on_click(
                        cx.listener(|this, _, w, cx| this.open_folder(&OpenFolder, w, cx)),
                    ))
                    .child(tool("tree-refresh", "Refresh").on_click(
                        cx.listener(|this, _, w, cx| this.refresh_tree(&RefreshTree, w, cx)),
                    )),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .overflow_hidden()
                    .child(root_name),
            )
            .child(
                div()
                    .id("file-tree-container")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "file-tree-list",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                                let mut items = Vec::with_capacity(range.len());
                                for index in range {
                                    let Some(entry) = this.flattened_tree.get(index) else {
                                        continue;
                                    };
                                    let active = this.path.as_ref() == Some(&entry.path);
                                    let is_dir = entry.directory;
                                    let expanded = entry.expanded;
                                    let entry_path = entry.path.clone();
                                    let (prefix, icon, icon_color) = file_badge_and_color(&entry.name, is_dir, expanded);

                                    items.push(
                                        div()
                                            .id(("file-tree-row", index))
                                            .h(px(28.))
                                            .flex()
                                            .items_center()
                                            .pl(px(12. + entry.depth as f32 * 16.))
                                            .pr_2()
                                            .text_sm()
                                            .cursor_pointer()
                                            .overflow_hidden()
                                            .bg(rgb(if active { 0x232a3b } else { 0x151821 }))
                                            .text_color(rgb(if active { 0xf8fafc } else { 0xcbd5e1 }))
                                            .hover(|style| style.bg(rgb(0x1c202c)))
                                            .border_l_2()
                                            .border_color(rgb(if active { 0x3b82f6 } else { 0x151821 }))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .overflow_hidden()
                                                    .child(div().text_xs().text_color(rgb(0x94a3b8)).child(prefix))
                                                    .child(div().text_sm().text_color(rgb(icon_color)).child(icon))
                                                    .child(div().overflow_hidden().text_ellipsis().child(entry.name.clone())),
                                            )
                                            .on_click(cx.listener(move |this, _, w, cx| {
                                                if is_dir {
                                                    this.toggle_folder(&entry_path, cx);
                                                } else {
                                                    this.open_path(entry_path.clone(), w, cx);
                                                }
                                            })),
                                    );
                                }
                                items
                            }),
                        )
                        .track_scroll(self.tree_scroll_handle.clone())
                        .size_full(),
                    ),
            )
    }


    fn toggle_minimap(&mut self, _: &ToggleMinimap, _: &mut Window, cx: &mut Context<Self>) {
        self.minimap_visible = !self.minimap_visible;
        self.message = if self.minimap_visible {
            "Minimap enabled".into()
        } else {
            "Minimap hidden".into()
        };
        cx.notify();
    }

    fn minimap_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let input = self.input.read(cx);
        let rope = input.text();
        let total = rope.lines_len().max(1);
        let cursor = (input.cursor_position().line as usize).min(total.saturating_sub(1));
        let samples = {
            let mut cache = self.minimap_cache.borrow_mut();
            cache.get_or_insert_with(|| {
                let lines_to_cache = total.min(10000);
                Rc::new((0..lines_to_cache).map(|row| {
                    let text = rope.slice_line(row).chars().take(160).collect::<String>();
                    minimap_segments(&text)
                }).collect())
            }).clone()
        };
        let bounds_cell = Rc::new(Cell::new(Bounds::default()));
        let paint_bounds = bounds_cell.clone();

        div().id("editor-minimap").w(px(140.)).h_full().min_h_0()
            .flex_shrink_0().flex().flex_col().overflow_hidden()
            .bg(rgb(0x111319)).border_l_1().border_color(rgb(0x202430))
            .child(div().px_2().py_2().text_xs().text_color(rgb(0x64748b))
                .child("MINIMAP"))
            .child(div().id("minimap-canvas").relative().flex_1().min_h_0()
                .cursor_pointer().overflow_hidden()
                .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                    let delta_lines = match event.delta {
                        ScrollDelta::Lines(p) => -p.y * 4.0,
                        ScrollDelta::Pixels(p) => -f32::from(p.y) / 16.0,
                    };
                    let input = this.input.read(cx);
                    let current_line = input.cursor_position().line as f32;
                    let total_lines = input.text().lines_len().max(1);
                    let new_line = (current_line + delta_lines).round().clamp(0.0, (total_lines - 1) as f32) as usize;
                    this.input.update(cx, |state, cx| {
                        state.set_cursor_position(gpui_component::input::Position {
                            line: new_line as u32,
                            character: 0,
                        }, window, cx);
                        state.focus(window, cx);
                    });
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, cx.listener({
                    let bounds_cell = bounds_cell.clone();
                    move |this, event: &MouseDownEvent, window, cx| {
                        this.minimap_dragging = true;
                        let bounds = bounds_cell.get();
                        let height = f32::from(bounds.size.height);
                        let total_lines = this.input.read(cx).text().lines_len().max(1);
                        let cursor_line = this.input.read(cx).cursor_position().line as usize;
                        let scroll_info = calculate_minimap_scroll(cursor_line, total_lines, height, MINIMAP_LINE_HEIGHT);
                        let y = f32::from(event.position.y - bounds.origin.y);
                        let target_line = minimap_coord_to_line(y, scroll_info.scroll_y, MINIMAP_LINE_HEIGHT, total_lines);
                        this.input.update(cx, |state, cx| {
                            state.set_cursor_position(gpui_component::input::Position {
                                line: target_line as u32,
                                character: 0,
                            }, window, cx);
                            state.focus(window, cx);
                        });
                        cx.notify();
                    }
                }))
                .on_mouse_move(cx.listener({
                    let bounds_cell = bounds_cell.clone();
                    move |this, event: &MouseMoveEvent, window, cx| {
                        if !this.minimap_dragging {
                            return;
                        }
                        let bounds = bounds_cell.get();
                        let height = f32::from(bounds.size.height);
                        let total_lines = this.input.read(cx).text().lines_len().max(1);
                        let cursor_line = this.input.read(cx).cursor_position().line as usize;
                        let scroll_info = calculate_minimap_scroll(cursor_line, total_lines, height, MINIMAP_LINE_HEIGHT);
                        let y = f32::from(event.position.y - bounds.origin.y);
                        let target_line = minimap_coord_to_line(y, scroll_info.scroll_y, MINIMAP_LINE_HEIGHT, total_lines);
                        this.input.update(cx, |state, cx| {
                            state.set_cursor_position(gpui_component::input::Position {
                                line: target_line as u32,
                                character: 0,
                            }, window, cx);
                            state.focus(window, cx);
                        });
                        cx.notify();
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.minimap_dragging {
                        this.minimap_dragging = false;
                        cx.notify();
                    }
                }))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.minimap_dragging {
                        this.minimap_dragging = false;
                        cx.notify();
                    }
                }))
                .child(canvas(move |bounds, _, _| {
                    paint_bounds.set(bounds);
                }, move |bounds, _, window, _| {
                    let height = f32::from(bounds.size.height);
                    let scroll_info = calculate_minimap_scroll(cursor, total, height, MINIMAP_LINE_HEIGHT);
                    let mut paint = |x: f32, y: f32, w: f32, h: f32, color: Hsla| {
                        if y + h >= 0.0 && y <= height {
                            window.paint_quad(fill(Bounds::new(
                                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                size(px(w), px(h))), color));
                        }
                    };

                    paint(0., scroll_info.slider_y, 140., scroll_info.slider_height, rgba(0x7898c12a).into());
                    paint(0., scroll_info.slider_y, 140., 1.0, rgba(0xa0c0e844).into());
                    paint(0., scroll_info.slider_y + scroll_info.slider_height - 1.0, 140., 1.0, rgba(0xa0c0e844).into());

                    for row in scroll_info.visible_range {
                        if let Some(segments) = samples.get(row) {
                            let y = (row as f32 * MINIMAP_LINE_HEIGHT) - scroll_info.scroll_y;
                            for &(x, width, color) in segments {
                                paint(6. + x, y, width, 1.6, rgb(color).into());
                            }
                        }
                    }

                    let cursor_render_y = (cursor as f32 * MINIMAP_LINE_HEIGHT) - scroll_info.scroll_y;
                    if cursor_render_y >= 0.0 && cursor_render_y <= height {
                        paint(0., cursor_render_y, 4., 2.4, rgb(0x83c9ff).into());
                    }
                }).size_full()))
    }

    fn save_file(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        self.save(false, window, cx);
    }
    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save(true, window, cx);
    }
}

/// Use the exact language IDs registered by gpui-component's Tree-sitter backend.
fn detect_language(path: Option<&Path>) -> (&'static str, &'static str) {
    let Some(path) = path else {
        return ("text", "Plain Text");
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        "makefile" | "gnumakefile" => return ("make", "Makefile"),
        "cmakelists.txt" => return ("cmake", "CMake"),
        ".bashrc" | ".bash_profile" | ".profile" => return ("bash", "Shell"),
        _ => {}
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => ("rust", "Rust"),
        "py" | "pyw" | "pyi" => ("python", "Python"),
        "js" | "jsx" | "mjs" | "cjs" => ("javascript", "JavaScript / JSX"),
        "ts" | "mts" | "cts" => ("typescript", "TypeScript"),
        "tsx" => ("tsx", "TSX"),
        "c" | "h" => ("c", "C"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => ("cpp", "C++"),
        "cs" => ("csharp", "C#"),
        "java" => ("java", "Java"),
        "go" => ("go", "Go"),
        "rb" | "rake" | "gemspec" => ("ruby", "Ruby"),
        "swift" => ("swift", "Swift"),
        "scala" | "sc" => ("scala", "Scala"),
        "zig" => ("zig", "Zig"),
        "ex" | "exs" => ("elixir", "Elixir"),
        "sh" | "bash" => ("bash", "Shell"),
        "html" | "htm" => ("html", "HTML"),
        "css" => ("css", "CSS"),
        "json" => ("json", "JSON"),
        "toml" => ("toml", "TOML"),
        "yaml" | "yml" => ("yaml", "YAML"),
        "md" | "markdown" => ("markdown", "Markdown"),
        "sql" => ("sql", "SQL"),
        "graphql" | "gql" => ("graphql", "GraphQL"),
        "proto" => ("proto", "Protocol Buffers"),
        "cmake" => ("cmake", "CMake"),
        "mk" => ("make", "Makefile"),
        "diff" | "patch" => ("diff", "Diff"),
        "ejs" => ("ejs", "EJS"),
        "erb" => ("erb", "ERB"),
        _ => ("text", "Plain Text"),
    }
}

fn decode_text(bytes: &[u8]) -> Result<(String, bool, bool), String> {
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let bytes = if bom { &bytes[3..] } else { bytes };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "File is not valid UTF-8. Please convert it to UTF-8 first.".to_string())?;
    if text.contains('\0') {
        return Err("File contains binary data and cannot be opened as text.".into());
    }
    let crlf = text.contains("\r\n");
    Ok((text.replace("\r\n", "\n").replace('\r', "\n"), crlf, bom))
}

fn encode_text(text: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = if crlf {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    };
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}


fn tool_active(id: &'static str, label: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(if active { rgb(0x1d4ed8) } else { rgb(0x1f232e) })
        .border_1()
        .border_color(if active { rgb(0x3b82f6) } else { rgb(0x2d3445) })
        .hover(move |style| style.bg(if active { rgb(0x2563eb) } else { rgb(0x2a3040) }))
        .text_color(if active { rgb(0xffffff) } else { rgb(0xd1d5db) })
        .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
        .text_sm()
        .child(label)
}

fn tool(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(0x1f232e))
        .border_1()
        .border_color(rgb(0x2d3445))
        .hover(|style| style.bg(rgb(0x2a3040)))
        .text_color(rgb(0xd1d5db))
        .text_sm()
        .child(label)
}



impl Editor {
fn toggle_column_mode(&mut self, _: &ToggleColumnMode, window: &mut Window, cx: &mut Context<Self>) {
        self.column_mode = !self.column_mode;
        if self.column_mode {
            let cursor = self.input.read(cx).cursor_position();
            let line = cursor.line as usize;
            let col = cursor.character as usize;
            self.column_start_line = line;
            self.column_end_line = line.saturating_add(1).min(self.lines.saturating_sub(1));
            self.column_col = col;
            self.column_width = 0;
            self.column_reversed = false;
            Theme::global_mut(cx).caret = rgba(0x00000000).into();
            self.message = format!("Column Mode ON: Lines {}..{}, Col {}", self.column_start_line + 1, self.column_end_line + 1, self.column_col + 1);
        } else {
            Theme::global_mut(cx).caret = rgb(0xffffff).into();
            let target_line = self.column_start_line.min(self.column_end_line);
            let target_col = self.column_col;
            self.input.update(cx, |state, cx| {
                state.set_cursor_position(
                    gpui_component::input::Position {
                        line: target_line as u32,
                        character: target_col as u32,
                    },
                    window,
                    cx,
                );
                state.focus(window, cx);
            });
            self.message = "Column Mode OFF".into();
        }
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn sync_column_input_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.column_end_line;
        let col = if self.column_reversed { self.column_col } else { self.column_col + self.column_width };
        self.input.update(cx, |state, cx| {
            state.set_cursor_position(
                gpui_component::input::Position { line: line as u32, character: col as u32 },
                window,
                cx,
            );
        });
    }

    fn column_extend_down(&mut self, _: &ColumnExtendDown, window: &mut Window, cx: &mut Context<Self>) {
        if !self.column_mode {
            self.toggle_column_mode(&ToggleColumnMode, window, cx);
            return;
        }
        self.column_end_line = extended_column_head(self.column_end_line, self.lines, true);
        self.message = format!("Column Selection: Lines {}..{}, Col {} (w:{})", self.column_start_line.min(self.column_end_line) + 1, self.column_start_line.max(self.column_end_line) + 1, self.column_col + 1, self.column_width);
        self.sync_column_input_cursor(window, cx);
        cx.notify();
    }

    fn column_extend_up(&mut self, _: &ColumnExtendUp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.column_mode {
            self.toggle_column_mode(&ToggleColumnMode, window, cx);
            self.column_end_line = self.column_start_line;
        }
        self.column_end_line = extended_column_head(self.column_end_line, self.lines, false);
        self.message = format!("Column Selection: Lines {}..{}, Col {} (w:{})", self.column_start_line.min(self.column_end_line) + 1, self.column_start_line.max(self.column_end_line) + 1, self.column_col + 1, self.column_width);
        self.sync_column_input_cursor(window, cx);
        cx.notify();
    }

    fn navigate_column(&mut self, navigation: ColumnNavigation, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        let head = if self.column_reversed { self.column_col } else { self.column_col + self.column_width };
        let text = self.input.read(cx).text().slice_lines(self.column_end_line..self.column_end_line + 1).to_string();
        let target = column_navigation_target(&text, head, navigation);
        self.set_column_head(target, select, window, cx);
    }

    fn set_column_head(&mut self, target: usize, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        (self.column_col, self.column_width, self.column_reversed) = column_range_after_navigation(
            self.column_col, self.column_width, self.column_reversed, target, select);
        self.sync_column_input_cursor(window, cx);
        cx.notify();
    }

    fn column_extend_right(&mut self, _: &ColumnExtendRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode {
            let head = if self.column_reversed { self.column_col } else { self.column_col + self.column_width };
            self.set_column_head(head.saturating_add(1), true, window, cx);
        }
    }
    fn column_extend_left(&mut self, _: &ColumnExtendLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode {
            let head = if self.column_reversed { self.column_col } else { self.column_col + self.column_width };
            self.set_column_head(head.saturating_sub(1), true, window, cx);
        }
    }
    fn column_shift_left(&mut self, _: &ColumnShiftLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode {
            let target = if self.column_width > 0 { self.column_col } else { self.column_col.saturating_sub(1) };
            self.set_column_head(target, false, window, cx);
        }
    }
    fn column_shift_right(&mut self, _: &ColumnShiftRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode {
            let target = if self.column_width > 0 { self.column_col + self.column_width } else { self.column_col.saturating_add(1) };
            self.set_column_head(target, false, window, cx);
        }
    }

    fn column_shift_up(&mut self, _: &ColumnShiftUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode && self.column_start_line > 0 && self.column_end_line > 0 {
            self.column_start_line -= 1;
            self.column_end_line -= 1;
            self.message = format!("Column Carets: Lines {}..{}, Col {} (w:{})", self.column_start_line.min(self.column_end_line) + 1, self.column_start_line.max(self.column_end_line) + 1, self.column_col + 1, self.column_width);
            self.sync_column_input_cursor(window, cx);
            cx.notify();
        }
    }

    fn column_shift_down(&mut self, _: &ColumnShiftDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_mode && self.column_start_line.max(self.column_end_line) + 1 < self.lines {
            self.column_start_line += 1;
            self.column_end_line += 1;
            self.message = format!("Column Carets: Lines {}..{}, Col {} (w:{})", self.column_start_line.min(self.column_end_line) + 1, self.column_start_line.max(self.column_end_line) + 1, self.column_col + 1, self.column_width);
            self.sync_column_input_cursor(window, cx);
            cx.notify();
        }
    }

    fn column_escape(&mut self, _: &ColumnEscape, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_context_menu.is_some() {
            self.tab_context_menu = None;
            cx.notify();
        } else if self.column_mode {
            self.column_mode = false;
            Theme::global_mut(cx).caret = rgb(0xffffff).into();
            let target_line = self.column_start_line.min(self.column_end_line);
            let target_col = self.column_col;
            self.input.update(cx, |state, cx| {
                state.set_cursor_position(
                    gpui_component::input::Position {
                        line: target_line as u32,
                        character: target_col as u32,
                    },
                    window,
                    cx,
                );
                state.focus(window, cx);
            });
            self.message = "Column Mode OFF".into();
            cx.notify();
        }
    }

    #[allow(dead_code)]
    fn apply_column_insert_text(&mut self, insert_str: &str, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.text(cx);
        let new_text = apply_column_insert(
            &original,
            self.column_start_line,
            self.column_end_line,
            self.column_col,
            self.column_width,
            &insert_str,
        );
        let target_line = self.column_start_line.min(self.column_end_line);
        let target_col = self.column_col + insert_str.chars().count();
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(
                gpui_component::input::Position {
                    line: target_line as u32,
                    character: target_col as u32,
                },
                window,
                cx,
            );
        });
        let total_lines = self.column_start_line.abs_diff(self.column_end_line) + 1;
        self.message = format!("Column edit applied ({} lines)", total_lines);
        cx.notify();
    }

    #[allow(dead_code)]
    fn apply_column_insert_numbers(&mut self, start: i64, step: i64, zero_pad: usize, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.text(cx);
        let new_text = apply_column_numbers(
            &original,
            self.column_start_line,
            self.column_end_line,
            self.column_col,
            self.column_width,
            start,
            step,
            zero_pad,
        );
        let target_line = self.column_start_line.min(self.column_end_line);
        let target_col = self.column_col + 2;
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(
                gpui_component::input::Position {
                    line: target_line as u32,
                    character: target_col as u32,
                },
                window,
                cx,
            );
        });
        let total_lines = self.column_start_line.abs_diff(self.column_end_line) + 1;
        self.message = format!("Column numbers inserted ({} lines)", total_lines);
        cx.notify();
    }

    fn apply_column_delete_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.text(cx);
        let new_text = apply_column_delete(
            &original,
            self.column_start_line,
            self.column_end_line,
            self.column_col,
            self.column_width,
        );
        let target_line = self.column_start_line.min(self.column_end_line);
        let target_col = self.column_col;
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(
                gpui_component::input::Position {
                    line: target_line as u32,
                    character: target_col as u32,
                },
                window,
                cx,
            );
        });
        self.column_width = 0;
        self.column_reversed = false;
        let total_lines = self.column_start_line.abs_diff(self.column_end_line) + 1;
        self.message = format!("Column deleted ({} lines)", total_lines);
        cx.notify();
    }


    fn apply_column_char_insert(&mut self, insert_str: &str, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.text(cx);
        let new_text = apply_column_insert(
            &original,
            self.column_start_line,
            self.column_end_line,
            self.column_col,
            self.column_width,
            insert_str,
        );
        self.column_col += insert_str.chars().count();
        self.column_width = 0;
            self.column_reversed = false;
        let target_line = self.column_start_line.min(self.column_end_line);
        let target_col = self.column_col;
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(
                gpui_component::input::Position {
                    line: target_line as u32,
                    character: target_col as u32,
                },
                window,
                cx,
            );
        });
        cx.notify();
    }


    fn apply_column_outdent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.column_mode { return; }
        let original = self.text(cx);
        let (new_text, removed) = apply_column_outdent(
            &original, self.column_start_line, self.column_end_line, self.column_col, 4,
        );
        if removed == 0 { return; }
        self.column_col = self.column_col.saturating_sub(removed);
        let target_line = self.column_start_line.min(self.column_end_line);
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(Position { line: target_line as u32, character: self.column_col as u32 }, window, cx);
        });
        self.message = format!("Column outdent applied ({} lines)", self.column_start_line.abs_diff(self.column_end_line) + 1);
        cx.notify();
    }

    fn apply_column_backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_width > 0 {
            self.apply_column_delete_box(window, cx);
            return;
        }
        if self.column_col == 0 {
            return;
        }
        let original = self.text(cx);
        let new_text = apply_column_delete(
            &original,
            self.column_start_line,
            self.column_end_line,
            self.column_col - 1,
            1,
        );
        self.column_col -= 1;
        let target_line = self.column_start_line.min(self.column_end_line);
        let target_col = self.column_col;
        self.input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(
                gpui_component::input::Position {
                    line: target_line as u32,
                    character: target_col as u32,
                },
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn handle_editor_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.column_mode {
            return false;
        }
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;

        let navigation = match key {
            "home" => Some(ColumnNavigation::Home),
            "end" => Some(ColumnNavigation::End),
            "left" if mods.control => Some(ColumnNavigation::PreviousWord),
            "right" if mods.control => Some(ColumnNavigation::NextWord),
            _ => None,
        };
        if let Some(navigation) = navigation {
            self.navigate_column(navigation, mods.shift, window, cx);
            return true;
        }
        if key == "escape" {
            self.column_escape(&ColumnEscape, window, cx);
            return true;
        }

        if key == "backspace" && !mods.control && !mods.alt {
            self.apply_column_backspace(window, cx);
            return true;
        }

        if key == "delete" && !mods.control && !mods.alt {
            self.apply_column_delete_box(window, cx);
            return true;
        }

        if key == "left" {
            if mods.shift {
                self.column_extend_left(&ColumnExtendLeft, window, cx);
            } else {
                self.column_shift_left(&ColumnShiftLeft, window, cx);
            }
            return true;
        }

        if key == "right" {
            if mods.shift {
                self.column_extend_right(&ColumnExtendRight, window, cx);
            } else {
                self.column_shift_right(&ColumnShiftRight, window, cx);
            }
            return true;
        }

        if key == "up" {
            if mods.shift {
                self.column_extend_up(&ColumnExtendUp, window, cx);
            } else {
                self.column_shift_up(&ColumnShiftUp, window, cx);
            }
            return true;
        }

        if key == "down" {
            if mods.shift {
                self.column_extend_down(&ColumnExtendDown, window, cx);
            } else {
                self.column_shift_down(&ColumnShiftDown, window, cx);
            }
            return true;
        }

        // Printable char insertion
        if !mods.control && !mods.alt && !mods.platform && !mods.function {
            let insert_char: Option<&str> = match key {
                "space" => Some(" "),
                "tab" => Some("    "),
                s if s.chars().count() == 1 => Some(s),
                _ => None,
            };
            if let Some(ch) = insert_char {
                self.apply_column_char_insert(ch, window, cx);
                return true;
            }
        }

        false
    }

    fn column_cursors_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        let first = self.column_start_line.min(self.column_end_line);
        let last = self.column_start_line.max(self.column_end_line);
        let column = self.column_col;
        let width = self.column_width;
        let reversed = self.column_reversed;
        div().absolute().inset_0().child(canvas(
            move |_, _, _| {},
            move |_, _, window, cx| {
                let carets = input.read(cx).column_caret_bounds(first, last, column, width);
                let left_carets = if reversed { input.read(cx).column_caret_bounds(first, last, column, 0) } else { Vec::new() };
                for (index, (_, caret, selection)) in carets.into_iter().enumerate() {
                    let caret = left_carets.get(index).map(|(_, bounds, _)| *bounds).unwrap_or(caret);
                    if width > 0 { window.paint_quad(fill(selection, rgba(0x38bdf826))); }
                    window.paint_quad(fill(caret, rgb(0x38bdf8)));
                }
            },
        ).size_full())
    }

}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.column_mode {
            Theme::global_mut(cx).caret = rgba(0x00000000).into();
        } else {
            Theme::global_mut(cx).caret = rgb(0xffffff).into();
        }
        let dirty = self.dirty_cached;
        let name = self.document_name(self.active_tab);
        let title = format!(
            "{}{} — {} v{}",
            if dirty { "● " } else { "" },
            name,
            APP_NAME,
            APP_VERSION
        );
        if self.last_window_title != title {
            window.set_window_title(&title);
            self.last_window_title = title;
        }
        let lines = self.lines;
        let chars = self.chars;
        let cursor_pos = self.input.read(cx).cursor_position();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0x13151b))
            .text_color(rgb(0xe2e8f0))
            .font_family("Segoe UI")
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if let Some((start_x, start_width)) = this.tree_resize {
                    if !event.dragging() {
                        this.tree_resize = None;
                    } else {
                        this.tree_width = explorer_width(
                            start_width + f32::from(event.position.x) - start_x,
                            f32::from(window.viewport_size().width),
                        );
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.tree_resize.take().is_some() { cx.notify(); }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.tree_resize.take().is_some() { cx.notify(); }
            }))
            //
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::save_file))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::toggle_tree))
            .on_action(cx.listener(Self::toggle_minimap))
            .on_action(cx.listener(Self::refresh_tree))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::toggle_column_mode))
            .on_action(cx.listener(Self::column_extend_down))
            .on_action(cx.listener(Self::column_extend_up))
            .on_action(cx.listener(Self::column_extend_right))
            .on_action(cx.listener(Self::column_extend_left))
            .on_action(cx.listener(Self::column_shift_left))
            .on_action(cx.listener(Self::column_shift_right))
            .on_action(cx.listener(Self::column_shift_up))
            .on_action(cx.listener(Self::column_shift_down))
            .on_action(cx.listener(Self::column_escape))
            .on_action(cx.listener(|this, _: &GoToLine, w, cx| this.open_goto_line(w, cx)))
            .on_action(cx.listener(|this, _: &GoToLineEscape, w, cx| this.close_goto_line(w, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(0x303b50))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .mr_4()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0x60a5fa))
                                    .child(APP_NAME),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(rgb(0x64748b))
                                    .child(format!("v{}", APP_VERSION)),
                            ),
                    )
                    .child(tool("toggle-tree", "Explorer").on_click(
                        cx.listener(|this, _, w, cx| this.toggle_tree(&ToggleTree, w, cx)),
                    ))
                    .child(tool("toggle-minimap", if self.minimap_visible { "Minimap ON" } else { "Minimap OFF" }).on_click(
                        cx.listener(|this, _, w, cx| this.toggle_minimap(&ToggleMinimap, w, cx)),
                    ))
                    .child(
                        tool_active("toggle-column", if self.column_mode { "Column: ON (Alt+C)" } else { "Column (Alt+C)" }, self.column_mode)
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_column_mode(&ToggleColumnMode, w, cx))),
                    )
                    .child(
                        tool("new", "New   Ctrl+N")
                            .on_click(cx.listener(|this, _, w, cx| this.new_file(&NewFile, w, cx))),
                    )
                    .child(
                        tool("open", "Open   Ctrl+O").on_click(
                            cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx)),
                        ),
                    )
                    .child(
                        tool("save", "Save   Ctrl+S").on_click(
                            cx.listener(|this, _, w, cx| this.save_file(&SaveFile, w, cx)),
                        ),
                    )
                    .child(
                        tool("save-as", "Save As")
                            .on_click(cx.listener(|this, _, w, cx| this.save_as(&SaveAs, w, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(self.tree_visible.then(|| self.tree_panel(cx)))
                    .children(self.tree_visible.then(|| self.tree_resize_handle(cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.tab_bar(cx))

                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div().relative().flex_1().min_w_0().min_h_0().overflow_hidden().px_3().pb_2()
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveHome, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::Home, false, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveEnd, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::End, false, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectToStartOfLine, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::Home, true, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectToEndOfLine, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::End, true, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveToPreviousWord, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::PreviousWord, false, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveToNextWord, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::NextWord, false, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectToPreviousWordStart, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::PreviousWord, true, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectToNextWordEnd, window, cx| {
                                                if this.column_mode {
                                                    this.navigate_column(ColumnNavigation::NextWord, true, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::Paste, window, cx| {
                                                if this.column_mode {
                                                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                                                        if !text.contains(['\r', '\n']) {
                                                            this.apply_column_char_insert(&text, window, cx);
                                                            cx.stop_propagation();
                                                        }
                                                    }
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectUp, window, cx| {
                                                if this.column_mode {
                                                    this.column_extend_up(&ColumnExtendUp, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectDown, window, cx| {
                                                if this.column_mode {
                                                    this.column_extend_down(&ColumnExtendDown, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectLeft, window, cx| {
                                                if this.column_mode {
                                                    this.column_extend_left(&ColumnExtendLeft, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::SelectRight, window, cx| {
                                                if this.column_mode {
                                                    this.column_extend_right(&ColumnExtendRight, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveLeft, window, cx| {
                                                if this.column_mode {
                                                    this.column_shift_left(&ColumnShiftLeft, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveRight, window, cx| {
                                                if this.column_mode {
                                                    this.column_shift_right(&ColumnShiftRight, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveUp, window, cx| {
                                                if this.column_mode {
                                                    this.column_shift_up(&ColumnShiftUp, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveDown, window, cx| {
                                                if this.column_mode {
                                                    this.column_shift_down(&ColumnShiftDown, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                                                if this.column_mode {
                                                    this.apply_column_char_insert("    ", window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                                                if this.column_mode {
                                                    this.apply_column_outdent(window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &Backspace, window, cx| {
                                                if this.column_mode {
                                                    this.apply_column_backspace(window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_action(cx.listener(|this, _: &Delete, window, cx| {
                                                if this.column_mode {
                                                    this.apply_column_delete_box(window, cx);
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                                if this.handle_editor_key_down(event, window, cx) {
                                                    cx.stop_propagation();
                                                }
                                            }))
                                            .child(
                                                Input::new(&self.input)
                                                    .h_full()
                                                    .appearance(false)
                                                    .font_family("Cascadia Code")
                                                    .text_size(px(16.)),
                                            )
                                            .children(self.column_mode.then(|| self.column_cursors_overlay(cx))),
                                    )
                                    .children(self.minimap_visible.then(|| self.minimap_panel(cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_3()
                    .py_1p5()
                    .text_xs()
                    .bg(rgb(0x101217))
                    .border_t_1()
                    .border_color(rgb(0x1e222d))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_color(rgb(0x94a3b8))
                                    .child(self.message.clone()),
                            )
                            .child(
                                div()
                                    .id("status-cursor-pos")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .cursor_pointer()
                                    .bg(rgb(0x1c212c))
                                    .text_color(rgb(0x60a5fa))
                                    .hover(|s| s.bg(rgb(0x2a3244)).text_color(rgb(0x93c5fd)))
                                    .child(format!("Ln {}, Col {}", cursor_pos.line + 1, cursor_pos.character + 1))
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.open_goto_line(w, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x64748b))
                                    .child(format!("{} lines · {} chars", lines, chars)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .text_color(rgb(0xa5b4fc))
                                    .child(detect_language(self.path.as_deref()).1),
                            )
                            .child(
                                div()
                                    .id("status-encoding-btn")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .cursor_pointer()
                                    .bg(rgb(0x1a2130))
                                    .text_color(rgb(if self.bom { 0xf59e0b } else { 0xe2e8f0 }))
                                    .hover(|s| s.bg(rgb(0x28334a)))
                                    .child(if self.bom { "UTF-8 BOM" } else { "UTF-8" })
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.toggle_bom_active(w, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("status-crlf-btn")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .cursor_pointer()
                                    .bg(rgb(0x1a2130))
                                    .text_color(rgb(0xe2e8f0))
                                    .hover(|s| s.bg(rgb(0x28334a)))
                                    .child(if self.crlf { "CRLF" } else { "LF" })
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.toggle_crlf_active(w, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x475569))
                                    .child("Ctrl+G Jump | Ctrl+F Find"),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .text_color(rgb(0x64748b))
                                    .child(format!("v{}", APP_VERSION)),
                            ),
                    ),
            )
            .children(self.goto_line_open.then(|| self.goto_line_panel(window, cx)))
            .child(self.tab_context_menu_panel(window, cx))
    }
}

// Windows file associations pass the selected path as an OS-string argument.
// Keep it lossless, including Unicode and spaces; quotes are handled by the OS.
fn startup_paths(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<PathBuf> {
    args.into_iter()
        .skip(1)
        .filter(|arg| !arg.is_empty())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&path))
                    .unwrap_or(path)
            }
        })
        .collect()
}

#[cfg(test)]
fn startup_path(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<PathBuf> {
    startup_paths(args).into_iter().next()
}

fn main() {
    let initial_paths = startup_paths(std::env::args_os());
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-n", NewFile, None),
            KeyBinding::new("ctrl-o", OpenFile, None),
            KeyBinding::new("ctrl-s", SaveFile, None),
            KeyBinding::new("ctrl-shift-s", SaveAs, None),
            KeyBinding::new("ctrl-shift-o", OpenFolder, None),
            KeyBinding::new("ctrl-b", ToggleTree, None),
            KeyBinding::new("ctrl-m", ToggleMinimap, None),
            KeyBinding::new("ctrl-w", CloseTab, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
            KeyBinding::new("alt-c", ToggleColumnMode, None),
            KeyBinding::new("ctrl-alt-down", ColumnExtendDown, None),
            KeyBinding::new("ctrl-alt-up", ColumnExtendUp, None),
            KeyBinding::new("alt-shift-down", ColumnExtendDown, None),
            KeyBinding::new("alt-shift-up", ColumnExtendUp, None),
            KeyBinding::new("alt-shift-right", ColumnExtendRight, None),
            KeyBinding::new("alt-shift-left", ColumnExtendLeft, None),
            KeyBinding::new("escape", ColumnEscape, None),
            KeyBinding::new("ctrl-g", GoToLine, None),
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.), px(760.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} v{}", APP_NAME, APP_VERSION).into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(420.))),
                ..Default::default()
            },
            move |window, cx| {
                let editor = cx.new(|cx| {
                    let mut editor = Editor::new(window, cx);
                    for path in initial_paths {
                        let root = if path.is_dir() {
                            Some(path.clone())
                        } else {
                            path.parent().map(Path::to_path_buf)
                        };
                        if let Some(root) = root {
                            let root = normalized_path(&root);
                            editor.tree_cache.clear();
                            editor.expanded.clear();
                            if let Ok(entries) = read_directory(&root) {
                                editor.tree_cache.insert(root.clone(), entries);
                            }
                            editor.tree_root = Some(root);
                        }
                        if path.is_dir() {
                            editor.message = "Folder opened".into();
                        } else {
                            editor.open_path(path, window, cx);
                        }
                    }
                    editor.reveal_active_in_tree();
                    editor
                });
                let weak = editor.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    weak.update(cx, |editor, cx| editor.confirm_close_all(window, cx))
                        .unwrap_or(true)
                });
                cx.new(|cx| Root::new(editor, window, cx))
            },
        )
        .expect("Failed to create editor window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests;
