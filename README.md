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
