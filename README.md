# alia

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](Cargo.toml)
[![UI: GPUI](https://img.shields.io/badge/GUI-GPUI%200.2.2-green.svg)](Cargo.toml)

**alia** is a high-performance native Windows text editor built with **Rust** and **GPUI**. It combines GPU-accelerated rendering with a state-driven editor architecture to provide a modern, responsive experience for editing code and documents.

[中文说明](README.zh-CN.md)

---

## Features

- **GPU-accelerated rendering**: Built on the GPUI rendering pipeline for responsive layout, text rendering, and interface updates.
- **Multi-tab document management**:
  - Open and switch between multiple documents.
  - Avoid duplicate tabs through normalized path comparison.
  - Protect unsaved changes when closing tabs or exiting.
  - Right-click context menu: Close Tab, Close Other Tabs, Close Tabs to the Right, Close Tabs to the Left, Close All Tabs with unsaved confirmation.
- **Project Explorer**:
  - Browse hierarchical directories with expand and collapse support.
  - Use a virtualized file tree to keep large directories responsive.
  - Keep the active file visible and centered when switching tabs.
- **Code minimap**:
  - Displays indentation, blank lines, and code shape in a compact overview.
  - Synchronizes with the editor viewport for long and short documents.
  - Supports mouse-wheel navigation and click-drag scrubbing.
  - Toggle visibility with `Ctrl + M`.
- **Multi-language syntax highlighting**:
  - Powered by Tree-sitter incremental parsing.
  - Supports 30+ common languages and formats, including Rust, C/C++, Go, Python, TypeScript/JavaScript, HTML/CSS, JSON, YAML, Markdown, and SQL.
  - Shares parser instances across documents to reduce unnecessary allocations.
- **Windows integration and reliable I/O**:
  - Uses lossless `args_os` parsing for Unicode paths, spaces, and file-association launches.
  - Runs native file dialogs on a dedicated worker thread to avoid Win32 COM STA and GPUI UI-thread reentrancy issues.
  - Preserves UTF-8 BOM markers and CRLF/LF line-ending styles.

---

## Keyboard shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl + N` | Create a new tab and document |
| `Ctrl + O` | Open files, including multiple selections |
| `Ctrl + Shift + O` | Open a folder as the Explorer root |
| `Ctrl + S` | Save the active document |
| `Ctrl + Shift + S` | Save the active document as a new file |
| `Ctrl + W` | Close the active tab |
| `Ctrl + Tab` | Switch to the next tab |
| `Ctrl + Shift + Tab` | Switch to the previous tab |
| `Ctrl + B` | Show or hide the Explorer |
| `Ctrl + M` | Show or hide the minimap |
| `Ctrl + F` | Open the find and replace panel |
| `Alt + C` | Toggle column editing (starts with the current and next row) |
| `Shift + Up / Down` | Extend the row range while in column mode |
| `Alt + Shift + Up / Down` or `Ctrl + Alt + Up / Down` | Add or extend column carets |
| `Shift + Left / Right` | Adjust the rectangular selection width in column mode |

---

## Build and run

### Requirements

- **Rust**: 1.85 or newer, preferably the stable toolchain.
- **C++ build tools**: Windows requires MSVC or a compatible C++ toolchain for compiling Tree-sitter C grammars.

### Run the development build

```bash
cargo run --bin alia
```

### Run tests

```bash
cargo test --bin alia
```

### Build a release version

```bash
cargo build --release --bin alia
```

The release executable is generated at `target/release/alia.exe`.

---

## Architecture and engineering notes

- [ADR.md](ADR.md): Architecture Decision Records covering framework selection, Win32 COM isolation, the virtualized Explorer, tab synchronization, and minimap interaction.
- [rust-gpui-skill.md](rust-gpui-skill.md): Practical Rust and GPUI development notes, troubleshooting guidance, pitfalls, and interview topics.

---

## License

This project is released under the [MIT License](LICENSE).

### Column navigation

In Column Mode, Home moves to column zero; End and Ctrl+Left/Right use the active row as the boundary reference for the entire rectangle. Hold Shift to extend the selection (including to the left). Typing replaces the rectangle on all selected rows. Mouse double-click word selection is not integrated with column mode.

### Resizable Explorer

Drag the right edge of Explorer to resize it (180–600 logical pixels, subject to available window space). Double-click the divider to restore the default 240-pixel width. The width is retained while toggling Explorer within the current window; it is not persisted across restarts.

### External file changes

Opened files are checked in the background approximately once per second using modification time and size. Clean documents reload automatically, including encoding and syntax highlighting. Unsaved local edits are preserved and a status message reports conflicts. Saving checks disk content again and asks before overwriting external changes or recreating an unavailable file. Reload restores the cursor position where possible; full selection and scroll restoration are not guaranteed.

### Automatic Explorer refresh

Explorer checks the root and expanded directories in the background approximately once per second. Created, renamed and deleted entries appear automatically. Unchanged snapshots do not trigger redraws; expansion state and the scroll handle are retained. Collapsed directories are read when expanded. Explorer has no manual Refresh or Open Folder buttons. Use the toolbar Open menu for Open File (Ctrl+O) or Open Folder (Ctrl+Shift+O).

### Git decorations in Explorer

Explorer checks Git status in the background with its automatic refresh. File names and right-aligned badges show M (modified), A (added), D (deleted), R (renamed), U (conflict), and ? (untracked), including staged changes. Folders summarize descendant changes, prioritizing conflicts. Deleted files appear through their remaining parent folder, not as synthetic file entries. Ignored files are not decorated. Non-repository folders have no Git badges. Git must be available on PATH; status collection failures clear the decorations.
### Git Change Navigation

The Explorer and editor show Git changes together. Added, modified, and deleted lines receive colored markers in the editor gutter, and clicking a marker moves the caret to that line. Markers refresh automatically with the repository status.
