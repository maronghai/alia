#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::*;
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState, RopeExt},
};
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
        RefreshTree,
        CloseTab,
        NextTab,
        PreviousTab
    ]
);

// Native modal dialogs pump Windows messages. Running them inside a GPUI
// entity update can re-enter the UI while it is mutably borrowed. Keep their
// COM apartment and message loop on a separate thread instead.
fn native_dialog<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("native-file-dialog".into())
        .spawn(operation)
        .map_err(|err| format!("无法启动文件对话框：{err}"))?
        .join()
        .map_err(|_| "文件对话框意外退出，请重试。".to_string())
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

struct Document {
    input: Entity<InputState>,
    path: Option<PathBuf>,
    saved_text: String,
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

struct Editor {
    tabs: Vec<Document>,
    active_tab: usize,
    next_untitled_id: usize,
    tree_root: Option<PathBuf>,
    tree_cache: HashMap<PathBuf, Vec<TreeEntry>>,
    expanded: HashSet<PathBuf>,
    tree_visible: bool,
    minimap_visible: bool,
    minimap_dragging: bool,
    tree_scroll_handle: UniformListScrollHandle,
    flattened_tree: Vec<FlattenedTreeEntry>,
    scroll_tree_to_active: Cell<bool>,
    message: String,
    last_window_title: String,
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
                .placeholder("开始输入，或按 Ctrl+O 打开文本文件…")
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
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
            minimap_visible: true,
            minimap_dragging: false,
            tree_scroll_handle: UniformListScrollHandle::new(),
            flattened_tree: Vec::new(),
            scroll_tree_to_active: Cell::new(true),
            message: "就绪".into(),
            last_window_title: String::new(),
        };
        editor.rebuild_flattened_tree();
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
                .set_title("操作失败")
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
                    .set_title("保存文本文件")
                    .add_filter("文本文件", &["txt"])
                    .add_filter("所有文件", &["*"]);
                if let Some(path) = &current_path {
                    if let Some(parent) = path.parent() {
                        dialog = dialog.set_directory(parent);
                    }
                    if let Some(name) = path.file_name() {
                        dialog = dialog.set_file_name(name.to_string_lossy());
                    }
                } else {
                    dialog = dialog.set_file_name("未命名.txt");
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
                "该文件已在其他标签中打开，请切换到对应标签保存。".into(),
                cx,
            );
            return false;
        }
        let text = self.text(cx);
        let output = encode_text(&text, self.crlf, self.bom);
        if let Err(err) = fs::write(&path, output) {
            self.error(format!("无法保存文件 {}：{err}", path.display()), cx);
            return false;
        }
        self.path = Some(path);
        self.update_language(cx);
        self.saved_text = text;
        self.dirty_cached = false;
        self.reveal_active_in_tree();
        self.message = "已保存".into();
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
            .set_title("保存更改")
            .set_description(format!("文档“{name}”有未保存的更改。是否先保存？\n选择“否”将放弃更改，选择“取消”返回编辑。"))
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
            .unwrap_or_else(|| format!("未命名-{}.txt", doc.untitled_id))
    }

    fn switch_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active_tab = index;
        self.input.update(cx, |state, cx| state.focus(window, cx));
        self.reveal_active_in_tree();
        cx.notify();
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let doc = Self::create_document(self.next_untitled_id, window, cx);
        self.next_untitled_id += 1;
        self.tabs.push(doc);
        self.switch_tab(self.tabs.len() - 1, window, cx);
        self.message = "新建文档".into();
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.active_tab;
        self.switch_tab(index, window, cx);
        if !self.confirm_discard(window, cx) {
            self.switch_tab(previous, window, cx);
            return;
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
        self.message = "标签已关闭".into();
        cx.notify();
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
            .bg(rgb(0x192233))
            .border_b_1()
            .border_color(rgb(0x303b50))
            .children(self.tabs.iter().enumerate().map(|(index, doc)| {
                let dirty = doc.dirty_cached;
                let name = self.document_name(index);
                div()
                    .id(("document-tab", index))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if index == self.active_tab {
                        0x30496b
                    } else {
                        0x192233
                    }))
                    .border_r_1()
                    .border_color(rgb(0x303b50))
                    .on_click(cx.listener(move |this, _, w, cx| this.switch_tab(index, w, cx)))
                    .child(format!("{}{}", if dirty { "● " } else { "" }, name))
                    .child(
                        div()
                            .id(("close-tab", index))
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgb(0x52617a)))
                            .child("×")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(index, w, cx);
                            })),
                    )
            }))
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let selected = native_dialog(|| {
            FileDialog::new()
                .set_title("打开 UTF-8 文本文件")
                .add_filter(
                    "文本和代码",
                    &[
                        "txt", "md", "rs", "toml", "json", "csv", "log", "html", "css", "js", "py",
                    ],
                )
                .add_filter("所有文件", &["*"])
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
                    state.focus(window, cx);
                });
                self.reveal_active_in_tree();
                self.message = "文件已打开".into();
                cx.notify();
            }
            Err(err) => self.error(format!("无法打开文件 {}：{err}", path.display()), cx),
        }
    }

    fn open_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        let initial = self.tree_root.clone();
        match native_dialog(move || {
            let mut dialog = FileDialog::new().set_title("打开文件夹");
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
                    self.message = "文件夹已打开".into();
                }
                Err(err) => self.message = format!("无法读取文件夹：{err}"),
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
            "文件树已刷新".into()
        } else {
            format!("刷新失败：{}", errors.join("；"))
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
            Err(err) => self.message = format!("无法读取目录：{err}"),
        }
        cx.notify();
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
            .unwrap_or("未打开文件夹".into());

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
            .w(px(250.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x192233))
            .border_r_1()
            .border_color(rgb(0x303b50))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("资源管理器"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .pb_2()
                    .child(tool("tree-open", "打开文件夹").on_click(
                        cx.listener(|this, _, w, cx| this.open_folder(&OpenFolder, w, cx)),
                    ))
                    .child(tool("tree-refresh", "刷新").on_click(
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
                                    let label = format!(
                                        "{} {}",
                                        if is_dir {
                                            if expanded { "▾" } else { "▸" }
                                        } else {
                                            "·"
                                        },
                                        entry.name
                                    );

                                    items.push(
                                        div()
                                            .id(("file-tree-row", index))
                                            .h(px(28.))
                                            .line_height(px(28.))
                                            .pl(px(12. + entry.depth as f32 * 16.))
                                            .pr_2()
                                            .text_sm()
                                            .cursor_pointer()
                                            .overflow_hidden()
                                            .bg(rgb(if active { 0x30496b } else { 0x192233 }))
                                            .hover(|style| style.bg(rgb(0x293a53)))
                                            .child(label)
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
            "已开启代码缩略图 (Minimap)".into()
        } else {
            "已隐藏代码缩略图 (Minimap)".into()
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
            .bg(rgb(0x101724)).border_l_1().border_color(rgb(0x35445b))
            .child(div().px_2().py_1().text_xs().text_color(rgb(0xa8bbd5))
                .child("MINIMAP · 滚动/拖拽/定位"))
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
        return ("text", "纯文本");
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
        _ => ("text", "纯文本"),
    }
}

fn decode_text(bytes: &[u8]) -> Result<(String, bool, bool), String> {
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let bytes = if bom { &bytes[3..] } else { bytes };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "文件不是有效的 UTF-8 编码，请先转换为 UTF-8。".to_string())?;
    if text.contains('\0') {
        return Err("文件包含二进制数据，无法作为文本打开。".into());
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

fn tool(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(0x253047))
        .hover(|style| style.bg(rgb(0x354767)))
        .text_sm()
        .child(label)
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty_cached;
        let name = self.document_name(self.active_tab);
        let title = format!(
            "{}{} — alia",
            if dirty { "● " } else { "" },
            name
        );
        if self.last_window_title != title {
            window.set_window_title(&title);
            self.last_window_title = title;
        }
        let lines = self.lines;
        let chars = self.chars;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x141b29))
            .text_color(rgb(0xdce5f4))
            .font_family("Microsoft YaHei UI")
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
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .mr_4()
                            .child("alia"),
                    )
                    .child(tool("toggle-tree", "文件树").on_click(
                        cx.listener(|this, _, w, cx| this.toggle_tree(&ToggleTree, w, cx)),
                    ))
                    .child(tool("toggle-minimap", if self.minimap_visible { "缩略图 ●" } else { "缩略图 ○" }).on_click(
                        cx.listener(|this, _, w, cx| this.toggle_minimap(&ToggleMinimap, w, cx)),
                    ))
                    .child(
                        tool("new", "新建  Ctrl+N")
                            .on_click(cx.listener(|this, _, w, cx| this.new_file(&NewFile, w, cx))),
                    )
                    .child(
                        tool("open", "打开  Ctrl+O").on_click(
                            cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx)),
                        ),
                    )
                    .child(
                        tool("save", "保存  Ctrl+S").on_click(
                            cx.listener(|this, _, w, cx| this.save_file(&SaveFile, w, cx)),
                        ),
                    )
                    .child(
                        tool("save-as", "另存为")
                            .on_click(cx.listener(|this, _, w, cx| this.save_as(&SaveAs, w, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(self.tree_visible.then(|| self.tree_panel(cx)))
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
                                        div().flex_1().min_w_0().min_h_0().overflow_hidden().px_3().pb_2().child(
                                            Input::new(&self.input)
                                                .h_full()
                                                .appearance(false)
                                                .font_family("Cascadia Code")
                                                .text_size(px(16.)),
                                        ),
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
                    .px_4()
                    .py_2()
                    .text_xs()
                    .bg(rgb(0x202c42))
                    .child(format!(
                        "{}   ·   {} 行   {} 字符",
                        self.message, lines, chars
                    ))
                    .child(format!(
                        "{}   |   UTF-8{}   |   {}   |   Ctrl+F 查找 / 替换",
                        detect_language(self.path.as_deref()).1,
                        if self.bom { " BOM" } else { "" },
                        if self.crlf { "CRLF" } else { "LF" }
                    )),
            )
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
                    title: Some("alia".into()),
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
                            editor.message = "文件夹已打开".into();
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
        .expect("无法创建编辑器窗口");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::{calculate_minimap_scroll, calculate_minimap_viewport, calculate_tree_scroll_offset, decode_text, detect_language, encode_text, minimap_click_to_line, minimap_coord_to_line, minimap_segments, MINIMAP_LINE_HEIGHT};
    #[test]
    fn minimap_merges_runs_and_bounds_scene_size() {
        let merged = minimap_segments("    abcdefghijklmnopqrstuvwxyz");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, 5.0);
        assert!(merged[0].1 > 30.0);
        assert!(minimap_segments(" \t ").is_empty());
        let adversarial = "a1{} ".repeat(40);
        let spans = minimap_segments(&adversarial);
        assert!(spans.len() <= 16);
        assert!(spans.iter().all(|(x, w, _)| *x >= 0.0 && *w > 0.0 && x + w <= 125.0));
        assert!(!minimap_segments("    中文🙂").is_empty());
    }

    #[test]
    fn startup_accepts_multiple_files() {
        let paths = super::startup_paths(["editor.exe".into(), "一.md".into(), "two.rs".into()]);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("一.md"));
        assert!(paths[1].ends_with("two.rs"));
    }

    #[test]
    fn tab_cycle_wraps_in_both_directions() {
        assert_eq!(super::next_tab_index(2, 3, true), 0);
        assert_eq!(super::next_tab_index(0, 3, false), 2);
        assert_eq!(super::next_tab_index(0, 1, false), 0);
        assert_eq!(super::next_tab_index(0, 1, true), 0);
    }

    #[test]
    fn startup_without_file_keeps_blank_document() {
        assert_eq!(super::startup_path(["editor.exe".into()]), None);
    }

    #[test]
    fn startup_preserves_chinese_and_spaces() {
        let path = std::path::PathBuf::from(r"C:\中文目录\我的 文档.md");
        let actual = super::startup_path(["editor.exe".into(), path.clone().into_os_string()]);
        assert_eq!(actual, Some(path));
    }

    #[test]
    fn startup_resolves_relative_markdown_path() {
        let path = super::startup_path(["editor.exe".into(), "文档 示例.md".into()]).unwrap();
        assert_eq!(path, std::env::current_dir().unwrap().join("文档 示例.md"));
        assert_eq!(detect_language(Some(&path)).0, "markdown");
    }

    #[test]
    fn detects_common_languages_and_special_filenames() {
        for (file, expected) in [
            ("main.RS", "rust"),
            ("hello.py", "python"),
            ("app.jsx", "javascript"),
            ("app.ts", "typescript"),
            ("app.tsx", "tsx"),
            ("main.c", "c"),
            ("main.hpp", "cpp"),
            ("Program.cs", "csharp"),
            ("Main.java", "java"),
            ("main.go", "go"),
            ("index.html", "html"),
            ("style.css", "css"),
            ("a.json", "json"),
            ("Cargo.toml", "toml"),
            ("a.yml", "yaml"),
            ("README.md", "markdown"),
            ("a.sql", "sql"),
            ("run.sh", "bash"),
            ("Makefile", "make"),
            ("CMakeLists.txt", "cmake"),
            (".bashrc", "bash"),
            ("a.txt", "text"),
            ("a.unknown", "text"),
        ] {
            assert_eq!(
                detect_language(Some(std::path::Path::new(file))).0,
                expected,
                "{file}"
            );
        }
        assert_eq!(detect_language(None).0, "text");
    }

    #[test]
    fn utf8_bom_and_windows_line_endings_round_trip() {
        let input = b"\xef\xbb\xbfhello\r\nworld\r\n";
        let (text, crlf, bom) = decode_text(input).unwrap();
        assert_eq!(text, "hello\nworld\n");
        assert_eq!(encode_text(&text, crlf, bom), input);
    }
    #[test]
    fn chinese_text_and_lf_round_trip() {
        let input = "中文输入\n第二行🦀";
        let (text, crlf, bom) = decode_text(input.as_bytes()).unwrap();
        assert_eq!(encode_text(&text, crlf, bom), input.as_bytes());
    }
    #[test]
    fn native_dialog_runs_on_a_separate_thread() {
        let caller = std::thread::current().id();
        let worker = super::native_dialog(|| std::thread::current().id()).unwrap();
        assert_ne!(caller, worker);
    }

    #[test]
    fn ancestor_dirs_under_root_returns_correct_hierarchy() {
        let root = std::path::Path::new(r"C:\project");
        let child = std::path::Path::new(r"C:\project\src\nested\mod.rs");
        let ancestors = super::ancestor_dirs_under_root(child, root);
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors[0].ends_with("src"));
        assert!(ancestors[1].ends_with("nested"));
    }

    #[test]
    fn ancestor_dirs_outside_root_returns_empty() {
        let root = std::path::Path::new(r"C:\project");
        let child = std::path::Path::new(r"D:\other\file.txt");
        let ancestors = super::ancestor_dirs_under_root(child, root);
        assert!(ancestors.is_empty());
    }

    #[test]
    fn same_path_ignores_unc_prefix() {
        let p1 = std::env::current_dir().unwrap();
        let p2 = std::fs::canonicalize(&p1).unwrap();
        assert!(super::same_path(&p1, &p2));
    }

    #[test]
    fn tree_ancestors_expansion_populates_intermediate_caches() {
        let temp_dir = std::env::var_os("FEIBO_TMP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("editor-tree-test-{}", std::process::id()));
        let sub = temp_dir.join("level1").join("level2");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("doc.txt");
        std::fs::write(&file, "test").unwrap();

        let ancestors = super::ancestor_dirs_under_root(&file, &temp_dir);
        assert_eq!(ancestors.len(), 2);
        let mut expanded = std::collections::HashSet::new();
        let mut tree_cache = std::collections::HashMap::new();

        for dir in &ancestors {
            expanded.insert(dir.clone());
            if !tree_cache.contains_key(dir) {
                if let Ok(entries) = super::read_directory(dir) {
                    tree_cache.insert(dir.clone(), entries);
                }
            }
        }

        assert!(expanded.iter().any(|p| super::same_path(p, &temp_dir.join("level1"))));
        assert!(expanded.iter().any(|p| super::same_path(p, &temp_dir.join("level1").join("level2"))));
        assert!(tree_cache.contains_key(&ancestors[0]));
        assert!(tree_cache.contains_key(&ancestors[1]));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn file_round_trip_with_chinese_path() {
        let root = std::env::var_os("FEIBO_TMP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let folder = root.join(format!("editor-io-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("中文 文档.txt");
        let text = "保存测试🦀\n第二行\n";
        for (crlf, bom) in [(true, true), (true, false), (false, false)] {
            let bytes = encode_text(text, crlf, bom);
            std::fs::write(&path, &bytes).unwrap();
            let read = std::fs::read(&path).unwrap();
            let (decoded, actual_crlf, actual_bom) = decode_text(&read).unwrap();
            assert_eq!(decoded, text);
            assert_eq!((actual_crlf, actual_bom), (crlf, bom));
            assert_eq!(encode_text(&decoded, actual_crlf, actual_bom), bytes);
        }
    }

    #[test]
    fn parses_all_supported_languages_with_tree_sitter() {
        for lang in [
            "rust",
            "python",
            "javascript",
            "typescript",
            "tsx",
            "c",
            "cpp",
            "csharp",
            "java",
            "go",
            "ruby",
            "swift",
            "scala",
            "zig",
            "elixir",
            "bash",
            "html",
            "css",
            "json",
            "toml",
            "yaml",
            "markdown",
            "sql",
            "graphql",
            "proto",
            "cmake",
            "make",
            "diff",
            "ejs",
            "erb",
            "text",
        ] {
            if lang == "text" {
                continue;
            }
            let language = gpui_component::highlighter::Language::from_str(lang);
            assert_ne!(language.name(), "text", "语言 {lang} 应该被识别为有效语法");
            let parser = gpui_component::highlighter::LanguageRegistry::singleton().language(lang);
            assert!(
                parser.is_some(),
                "语言 {lang} 应该能够成功载入 Tree-sitter 语法解析器"
            );
        }
    }

        #[test]
    fn minimap_segments_and_click_mapping_behave_properly() {
        let segs = minimap_segments("    let x = 42; // note");
        assert!(!segs.is_empty(), "minimap strokes should not be empty");
        let first = segs[0];
        assert!(first.0 >= 4.0 * 1.25, "indentation should be preserved in coordinates");

        let line = minimap_click_to_line(50.0, 100.0, 100);
        assert_eq!(line, 50, "middle click maps to middle line");

        let (start, end) = calculate_minimap_viewport(10, 100, 20);
        assert!(start <= 10 && end >= 10, "viewport window includes cursor");
    }

    #[test]
    fn tree_scroll_offset_centers_out_of_view_row_and_keeps_in_view_stable() {
        const ROW_HEIGHT: f32 = 28.0;
        const VIEWPORT_H: f32 = 600.0;
        const TOTAL_ROWS: usize = 60;

        // 位于视口中央偏上的第 5 行，当前已在可视区内且上下均有余量，无需重置滚动条
        assert_eq!(
            calculate_tree_scroll_offset(5, TOTAL_ROWS, ROW_HEIGHT, VIEWPORT_H, 0.0),
            None
        );

        // 位于深处的第 45 行，超出视口下方，应返回居中偏移量
        let offset = calculate_tree_scroll_offset(45, TOTAL_ROWS, ROW_HEIGHT, VIEWPORT_H, 0.0);
        assert!(offset.is_some());
        let offset_y = offset.unwrap();
        assert!(offset_y < 0.0);
        // 目标行中心在经过偏移后应恰好对齐视口中央
        let row_center_in_viewport = (45.0 * ROW_HEIGHT + ROW_HEIGHT / 2.0) + offset_y;
        assert!((row_center_in_viewport - VIEWPORT_H / 2.0).abs() < 1.0);

        // 位于第 0 行时从下方跳回，理想居中偏上，但经过上边界 clamp 限制为 0.0（顶格不留白）
        assert_eq!(
            calculate_tree_scroll_offset(0, TOTAL_ROWS, ROW_HEIGHT, VIEWPORT_H, -800.0),
            Some(0.0)
        );

        // 位于最后一行时，应受限于最大滚动范围（底格不留白）
        let max_scroll = (TOTAL_ROWS as f32 * ROW_HEIGHT) - VIEWPORT_H;
        assert_eq!(
            calculate_tree_scroll_offset(59, TOTAL_ROWS, ROW_HEIGHT, VIEWPORT_H, 0.0),
            Some(-max_scroll)
        );
    }


    #[test]
    fn flattened_tree_cache_and_virtualization() {
        let temp_dir = std::env::var_os("FEIBO_TMP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("flattened-tree-test-{}", std::process::id()));
        let dir_a = temp_dir.join("folder_a");
        let dir_b = temp_dir.join("folder_b");
        let sub_dir = dir_a.join("sub_folder");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("file_1.txt"), "1").unwrap();
        std::fs::write(sub_dir.join("sub_file.txt"), "sub").unwrap();
        std::fs::write(dir_b.join("file_2.txt"), "2").unwrap();

        let norm_root = super::normalized_path(&temp_dir);
        let entries = super::read_directory(&norm_root).unwrap();
        let mut tree_cache = std::collections::HashMap::new();
        tree_cache.insert(norm_root.clone(), entries);

        let mut expanded = std::collections::HashSet::new();
        // 初始折叠
        let norm_dir_a = super::normalized_path(&dir_a);
        let _norm_dir_b = super::normalized_path(&dir_b);
        let _norm_sub_dir = super::normalized_path(&sub_dir);

        // 模拟 collect
        let collect = |cache: &std::collections::HashMap<std::path::PathBuf, Vec<super::TreeEntry>>,
                       exp: &std::collections::HashSet<std::path::PathBuf>,
                       root: &std::path::Path| {
            let mut result = Vec::new();
            fn rec(
                path: &std::path::Path,
                depth: usize,
                cache: &std::collections::HashMap<std::path::PathBuf, Vec<super::TreeEntry>>,
                exp: &std::collections::HashSet<std::path::PathBuf>,
                out: &mut Vec<super::FlattenedTreeEntry>,
            ) {
                if let Some(entries) = cache.get(path) {
                    for e in entries {
                        let is_exp = e.directory && exp.contains(&e.path);
                        out.push(super::FlattenedTreeEntry {
                            path: e.path.clone(),
                            name: e.name.clone(),
                            directory: e.directory,
                            depth,
                            expanded: is_exp,
                        });
                        if is_exp {
                            rec(&e.path, depth + 1, cache, exp, out);
                        }
                    }
                }
            }
            rec(root, 0, cache, exp, &mut result);
            result
        };

        let initial_rows = collect(&tree_cache, &expanded, &norm_root);
        assert_eq!(initial_rows.len(), 2); // 两个文件夹 folder_a 和 folder_b

        // 展开 folder_a
        expanded.insert(norm_dir_a.clone());
        tree_cache.insert(norm_dir_a.clone(), super::read_directory(&norm_dir_a).unwrap());
        let expanded_rows = collect(&tree_cache, &expanded, &norm_root);
        assert_eq!(expanded_rows.len(), 4); // folder_a, file_1.txt, sub_folder, folder_b

        let _ = std::fs::remove_dir_all(&temp_dir);
    }


    #[test]
    fn minimap_scroll_calculation_handles_short_and_long_documents() {
        const VIEWPORT_H: f32 = 600.0;

        // 1. 短文档 (50 行，内容高度 50 * 2.4 = 120px < 600px)
        let short_info = calculate_minimap_scroll(10, 50, VIEWPORT_H, MINIMAP_LINE_HEIGHT);
        assert_eq!(short_info.scroll_y, 0.0, "短文档 Minimap 无需滚动");
        assert_eq!(short_info.visible_range, 0..50, "短文档全部行可见");
        assert!(short_info.slider_y >= 0.0);
        assert!(short_info.slider_y + short_info.slider_height <= VIEWPORT_H);

        // 2. 超长文档 (2000 行，内容高度 2000 * 2.4 = 4800px > 600px)
        // 顶部光标
        let top_info = calculate_minimap_scroll(0, 2000, VIEWPORT_H, MINIMAP_LINE_HEIGHT);
        assert_eq!(top_info.scroll_y, 0.0, "顶部光标时 Minimap 处于最顶端");
        assert_eq!(top_info.visible_range.start, 0);
        assert!(top_info.visible_range.end <= 260);

        // 底部光标
        let bottom_info = calculate_minimap_scroll(1999, 2000, VIEWPORT_H, MINIMAP_LINE_HEIGHT);
        let max_scroll = 4800.0 - VIEWPORT_H; // 4200.0
        assert_eq!(bottom_info.scroll_y, max_scroll, "底部光标时 Minimap 滚至最底端");
        assert_eq!(bottom_info.visible_range.end, 2000);
        assert_eq!(bottom_info.slider_y + bottom_info.slider_height, VIEWPORT_H);

        // 中间光标 (第 1000 行)
        let mid_info = calculate_minimap_scroll(1000, 2000, VIEWPORT_H, MINIMAP_LINE_HEIGHT);
        assert!((mid_info.scroll_y - max_scroll * 0.5).abs() < 5.0, "中间光标时平滑居中");
        assert!(mid_info.visible_range.contains(&1000));
    }

    #[test]
    fn minimap_coord_to_line_and_drag_bounds_clamping() {
        const TOTAL_LINES: usize = 1000;
        let line_height = MINIMAP_LINE_HEIGHT; // 2.4

        // 无滚动时直接点击
        assert_eq!(minimap_coord_to_line(0.0, 0.0, line_height, TOTAL_LINES), 0);
        assert_eq!(minimap_coord_to_line(24.0, 0.0, line_height, TOTAL_LINES), 10);

        // 包含滚动偏移时点击
        let scroll_y = 1200.0; // 相当于滚动了 500 行
        let line = minimap_coord_to_line(24.0, scroll_y, line_height, TOTAL_LINES);
        assert_eq!(line, 510);

        // 上下越界安全保护
        assert_eq!(minimap_coord_to_line(-100.0, 0.0, line_height, TOTAL_LINES), 0);
        assert_eq!(minimap_coord_to_line(10000.0, scroll_y, line_height, TOTAL_LINES), 999);
    }

    #[test]
    fn rejects_binary_and_invalid_utf8() {
        assert!(decode_text(&[0xff, 0xfe]).is_err());
        assert!(decode_text(b"abc\0def").is_err());
    }
}
