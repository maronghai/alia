# alia

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](Cargo.toml)
[![UI: GPUI](https://img.shields.io/badge/GUI-GPUI%200.2.2-green.svg)](Cargo.toml)

**alia** 是一款使用 **Rust** 和 **GPUI** 开发的高性能 Windows 原生桌面文本编辑器。它结合 GPU 加速渲染与状态驱动的编辑器架构，为代码和文档编辑提供现代、流畅的使用体验。

[English README](README.md)

---

## 核心特性

- **GPU 加速渲染**：基于 GPUI 渲染流水线，提供响应迅速的布局、文本渲染和界面更新。
- **多标签页文档管理**：
  - 支持同时打开和切换多个文档。
  - 通过规范化路径比较避免同一文件重复打开。
  - 关闭标签页或退出程序时保护未保存修改。
  - Tab 右键上下文菜单：支持关闭当前、关闭其它、关闭右侧所有、关闭左侧所有及关闭全部标签页，并具备未保存确认保护。
- **项目资源管理器**：
  - 浏览层级目录，支持展开和折叠。
  - 使用虚拟化文件树，保持大量文件时的流畅性。
  - 切换标签页时自动让当前文件保持可见并居中显示。
- **代码缩略图 Minimap**：
  - 以紧凑视图显示缩进、空行和代码轮廓。
  - 与编辑区视口同步，适配长文档和短文档。
  - 支持鼠标滚轮导航和按住鼠标拖动定位。
  - 使用 `Ctrl + M` 显示或隐藏。
- **多语言语法高亮**：
  - 基于 Tree-sitter 增量语法解析。
  - 支持 30 多种常见语言和格式，包括 Rust、C/C++、Go、Python、TypeScript/JavaScript、HTML/CSS、JSON、YAML、Markdown 和 SQL。
  - 在多个文档之间共享解析器实例，减少不必要的内存分配。
- **Windows 集成与可靠 I/O**：
  - 使用无损的 `args_os` 解析，支持 Unicode 路径、包含空格的路径以及文件关联启动。
  - 在独立工作线程中运行原生文件对话框，避免 Win32 COM STA 与 GPUI UI 线程之间的重入问题。
  - 保留 UTF-8 BOM 标记以及 CRLF/LF 换行风格。

---

## 常用快捷键

| 快捷键 | 功能 |
| :--- | :--- |
| `Ctrl + N` | 新建标签页和文档 |
| `Ctrl + O` | 打开文件，支持多选 |
| `Ctrl + Shift + O` | 打开文件夹并设置为资源管理器根目录 |
| `Ctrl + S` | 保存当前文档 |
| `Ctrl + Shift + S` | 将当前文档另存为新文件 |
| `Ctrl + W` | 关闭当前标签页 |
| `Ctrl + Tab` | 切换到下一个标签页 |
| `Ctrl + Shift + Tab` | 切换到上一个标签页 |
| `Ctrl + B` | 显示或隐藏资源管理器 |
| `Ctrl + M` | 显示或隐藏 Minimap |
| `Ctrl + F` | 打开查找和替换面板 |
| `Alt + C` | 切换列编辑（从当前行和下一行开始） |
| `Shift + Up / Down` | 在列模式中连续扩展行范围 |
| `Alt + Shift + Up / Down` 或 `Ctrl + Alt + Up / Down` | 添加或扩展列光标 |
| `Shift + Left / Right` | 在列模式中调整矩形选区宽度 |

---

## 构建与运行

### 环境要求

- **Rust**：1.85 或更高版本，建议使用 stable 工具链。
- **C++ 构建工具**：Windows 下需要 MSVC 或兼容的 C++ 工具链，用于编译 Tree-sitter C 语法解析器。

### 运行开发构建

```bash
cargo run --bin alia
```

### 运行测试

```bash
cargo test --bin alia
```

### 构建正式版本

```bash
cargo build --release --bin alia
```

正式版可执行文件生成在 `target/release/alia.exe`。

---

## 架构与工程实践文档

- [ADR.md](ADR.md)：架构决策记录，涵盖框架选型、Win32 COM 解耦、虚拟化资源管理器、标签页联动和 Minimap 交互设计。
- [rust-gpui-skill.md](rust-gpui-skill.md)：Rust 与 GPUI 桌面端开发经验、排错方法、避坑指南和面试考点。

---

## 开源协议

本项目采用 [MIT License](LICENSE) 开源。

### 列编辑导航

列模式下 Home 移至第 0 列；End、Ctrl+左右方向键以主操作行的行尾或词边界为基准移动整组光标。按住 Shift 可向两侧扩展矩形选区，输入会替换所有选中行的矩形范围。鼠标双击选词尚未与列模式联动。

### 调整 Explorer 宽度

拖动 Explorer 右边缘可调整宽度（180–600 个逻辑像素，受窗口可用空间限制），双击分隔线恢复默认 240 像素。隐藏再显示侧栏会保留当前宽度，重启后恢复默认值。

### 外部文件修改同步

后台约每秒检查已打开文件的修改时间及大小，变化后读取内容。无本地修改时自动重载并更新编码、语法高亮；有未保存修改时保留本地正文并显示冲突提示。保存前再次核对磁盘内容，覆盖外部修改或重新创建不可用文件前询问。重载尽量恢复光标位置，不保证完整选区及滚动位置恢复。

### Explorer 自动刷新

后台约每秒检查根目录及已展开目录，自动反映新增、重命名和删除。目录内容未变化时不触发重绘；保留展开状态和滚动句柄。折叠目录在展开时读取，不再显示 Refresh 和 Open Folder 按钮。统一通过顶部 Open 菜单选择打开文件（Ctrl+O）或文件夹（Ctrl+Shift+O）。

### Explorer Git 变更标记

后台随自动刷新检测 Git 状态。文件名颜色和右侧标记分别表示 M 修改、A 新增、D 删除、R 重命名、U 冲突、? 未跟踪，包含暂存区变更。文件夹汇总后代状态，优先显示冲突。删除项通过仍存在的父目录标记体现，不额外生成虚拟文件行；忽略项不标记。非仓库不显示标记。Git 需在 PATH 中可用，采集失败时清除标记。
