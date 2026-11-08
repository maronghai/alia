# alia

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](Cargo.toml)
[![UI: GPUI](https://img.shields.io/badge/GUI-GPUI%200.2.2-green.svg)](Cargo.toml)

**alia** 是一款使用 **Rust** 和 **GPUI** 开发的高性能 Windows 原生桌面文本编辑器。结合硬件加速 GPU 渲染引擎与树形状态管理，提供现代、流畅、低延迟的代码与文档编辑体验。

---

## 核心特性

- **GPU 硬件加速渲染**：基于 GPUI 渲染流水线，界面重绘与字符排版交由 GPU 驱动，启动迅速，内存占用小。
- **多标签页（Tabs）管理**：
  - 支持多文档同时打开与快速切换。
  - 单实例规范化路径排重，避免重复打开同一文件。
  - 标签关闭与退出前未保存脏状态安全拦截。
- **左侧资源管理器与深度联动**：
  - 层次化目录树浏览，支持目录展开与折叠。
  - 虚拟化缓存（避免深层递归引起的渲染卡顿）。
  - **Tab 切换视口居中联动**：切换文档标签时，资源管理器自动定位并平滑居中显示当前文档，辅以安全区防抖算法。
- **代码缩略图（Minimap）**：
  - 真实代码缩进、空行和轮廓映射。
  - 自适应视口平滑滚动（长短文档智能切片渲染）。
  - 缩略图区域鼠标滚轮联动与按住左键平滑拖拽定位（Scrubbing）。
  - 快捷键 `Ctrl+M` 一键隐藏/显示。
- **多语言语法高亮**：
  - 基于 Tree-sitter 高性能增量语法分析（支持 30+ 种主流语言：Rust、C/C++、Go、Python、TypeScript/JavaScript、HTML/CSS、JSON、YAML、Markdown、SQL 等）。
  - 全局单例语法解析器池，避免多标签打开时的冗余内存分配。
- **Windows 原生集成与健壮 I/O**：
  - 采用无损 Unicode 参数解析（`args_os`），完美支持中文路径、带空格路径及 Windows 默认打开程序关联启动。
  - 独立工作线程执行原生文件对话框，彻底杜绝 Win32 COM 单线程套间（STA）与 GPUI UI 线程的借用重入崩溃（BorrowMutError）。
  - 保留原始换行风格（CRLF / LF）与 UTF-8 BOM 标记。

---

## 常用快捷键

| 快捷键 | 功能说明 |
| :--- | :--- |
| `Ctrl + N` | 新建标签页 / 新建文档 |
| `Ctrl + O` | 打开文件（支持多选） |
| `Ctrl + Shift + O` | 打开文件夹（设置资源管理器根目录） |
| `Ctrl + S` | 保存当前文档 |
| `Ctrl + Shift + S` | 另存为... |
| `Ctrl + W` | 关闭当前标签页 |
| `Ctrl + Tab` | 切换到下一个标签页 |
| `Ctrl + Shift + Tab` | 切换到上一个标签页 |
| `Ctrl + B` | 显示 / 隐藏左侧资源管理器 |
| `Ctrl + M` | 显示 / 隐藏右侧代码缩略图（Minimap） |
| `Ctrl + F` | 激活查找与替换面板 |

---

## 构建与运行

### 前置环境要求
- **Rust**：1.85+（推荐使用 stable 工具链）
- **C++ 构建工具**：Windows 下需要 MSVC 或对应 C++ 工具链（用于编译 Tree-sitter C 代码）

### 运行开发构建
```bash
cargo run --bin alia
```

### 运行单元测试
```bash
cargo test --bin alia
```

### 构建正式发布版（推荐）
```bash
cargo build --release --bin alia
```
编译产物位于 `target/release/alia.exe`。

---

## 架构与技术文档

本项目包含完整的架构演进与实践总结文档，可参阅：
- [ADR.md](ADR.md)：架构决策记录（UI 框架选型、Win32 COM 解耦、虚拟化文件树、Minimap 联动设计等）。
- [rust-gpui-skill.md](rust-gpui-skill.md)：基于 Rust + GPUI 的桌面端高阶开发经验、避坑指南与面试考点。

---

## 开源协议

本项目采用 [MIT 协议](LICENSE) 开源。
