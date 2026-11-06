#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::*;
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState},
};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::{fs, path::PathBuf};

actions!(editor, [NewFile, OpenFile, SaveFile, SaveAs]);

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

struct Editor {
    input: Entity<InputState>,
    path: Option<PathBuf>,
    saved_text: String,
    crlf: bool,
    bom: bool,
    message: String,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("plain_text")
                .line_number(true)
                .placeholder("开始输入，或按 Ctrl+O 打开文本文件…")
        });
        let subscription = cx.subscribe_in(&input, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        Self {
            input,
            path: None,
            saved_text: String::new(),
            crlf: true,
            bom: false,
            message: "就绪".into(),
            _subscriptions: vec![subscription],
        }
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
    fn dirty(&self, cx: &App) -> bool {
        self.text(cx) != self.saved_text
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
        let text = self.text(cx);
        let output = encode_text(&text, self.crlf, self.bom);
        if let Err(err) = fs::write(&path, output) {
            self.error(format!("无法保存文件 {}：{err}", path.display()), cx);
            return false;
        }
        self.path = Some(path);
        self.saved_text = text;
        self.message = "已保存".into();
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
        true
    }

    fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.dirty(cx) {
            return true;
        }
        let decision = native_dialog(|| {
            MessageDialog::new()
            .set_title("保存更改")
            .set_description(
                "当前文档有未保存的更改。是否先保存？\n选择“否”将放弃更改，选择“取消”返回编辑。",
            )
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

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_discard(window, cx) {
            return;
        }
        self.path = None;
        self.saved_text.clear();
        self.bom = false;
        self.crlf = true;
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.message = "新建文档".into();
        cx.notify();
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
                .pick_file()
        });
        let path = match selected {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(err) => {
                self.error(err, cx);
                return;
            }
        };
        let decoded = fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode_text(&bytes));
        match decoded {
            Ok((text, crlf, bom)) => {
                if !self.confirm_discard(window, cx) {
                    return;
                }
                self.path = Some(path);
                self.saved_text = text.clone();
                self.crlf = crlf;
                self.bom = bom;
                self.input.update(cx, |state, cx| {
                    state.set_value(text, window, cx);
                    state.focus(window, cx);
                });
                self.message = "文件已打开".into();
                cx.notify();
            }
            Err(err) => self.error(format!("无法打开文件 {}：{err}", path.display()), cx),
        }
    }

    fn save_file(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        self.save(false, window, cx);
    }
    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save(true, window, cx);
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
        let dirty = self.dirty(cx);
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or("未命名.txt".into());
        window.set_window_title(&format!(
            "{}{} — Ferrite 文本编辑器",
            if dirty { "● " } else { "" },
            name
        ));
        let value = self.input.read(cx).value();
        let lines = value.bytes().filter(|b| *b == b'\n').count() + 1;
        let chars = value.chars().count();
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
                            .child("Ferrite"),
                    )
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
                    .items_center()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .gap_3()
                    .child(
                        div()
                            .text_color(rgb(0x8bbcff))
                            .child(if dirty { "●" } else { "○" }),
                    )
                    .child(name)
                    .child(div().text_color(rgb(0x8796ad)).child(if dirty {
                        "未保存"
                    } else {
                        "已同步"
                    })),
            )
            .child(
                div().flex_1().min_h_0().px_3().pb_2().child(
                    Input::new(&self.input)
                        .h_full()
                        .appearance(false)
                        .font_family("Cascadia Code")
                        .text_size(px(16.)),
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
                        "UTF-8{}   |   {}   |   Ctrl+F 查找 / 替换",
                        if self.bom { " BOM" } else { "" },
                        if self.crlf { "CRLF" } else { "LF" }
                    )),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-n", NewFile, None),
            KeyBinding::new("ctrl-o", OpenFile, None),
            KeyBinding::new("ctrl-s", SaveFile, None),
            KeyBinding::new("ctrl-shift-s", SaveAs, None),
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
                    title: Some("Ferrite 文本编辑器".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(420.))),
                ..Default::default()
            },
            |window, cx| {
                let editor = cx.new(|cx| Editor::new(window, cx));
                let weak = editor.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    weak.update(cx, |editor, cx| editor.confirm_discard(window, cx))
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
    use super::{decode_text, encode_text};
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
    fn rejects_binary_and_invalid_utf8() {
        assert!(decode_text(&[0xff, 0xfe]).is_err());
        assert!(decode_text(b"abc\0def").is_err());
    }
}
