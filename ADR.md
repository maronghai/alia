# 架构决策记录 (Architecture Decision Records - ADR)

**项目名称**：Ferrite 文本编辑器 (gpui_editor)  
**目标平台**：Windows (x86_64)  
**核心技术栈**：Rust (2024 Edition) + GPUI 0.2 + gpui-component 0.5 + Tree-sitter + rfd 0.15  
**文档状态**：已批准 (Approved) / 活跃维护中  

---

## 目录
1. [ADR-001：UI 框架选型 —— 为何选择 Rust + GPUI](#adr-001ui-框架选型--为何选择-rust--gpui)
2. [ADR-002：Windows 原生对话框与 UI 线程解耦 —— 独立线程运行 COM 对话框](#adr-002windows-原生对话框与-ui-线程解耦--独立线程运行-com-对话框)
3. [ADR-003：轻量级层级文件树与虚拟化目录缓存](#adr-003轻量级层级文件树与虚拟化目录缓存)
4. [ADR-004：多语言语法高亮引擎 —— Tree-sitter 单例与按需高亮集成](#adr-004多语言语法高亮引擎--tree-sitter-单例与按需高亮集成)
5. [ADR-005：Windows 启动参数与文件关联深度适配 (无损 Unicode & 路径归一化)](#adr-005windows-启动参数与文件关联深度适配-无损-unicode--路径归一化)
6. [ADR-006：多标签页 (Tab) 架构与单实例防重复冲突机制](#adr-006多标签页-tab-架构与单实例防重复冲突机制)
7. [重点、难点剖析](#重点难点剖析)
8. [项目避坑指南 (Windows + GPUI 生态)](#项目避坑指南-windows--gpui-生态)
9. [Rust / 高性能客户端面试核心考点](#rust--高性能客户端面试核心考点)

---

## ADR-001：UI 框架选型 —— 为何选择 Rust + GPUI

### 背景 (Context)
构建一个现代桌面文本编辑器需要满足以下严苛要求：
1. **即时启动与低内存占用**：避免传统基于 Electron 带来的数百兆内存消耗及冷启动延迟。
2. **GPU 硬件加速与高帧率渲染**：大文档平滑滚动、60~120 FPS 响应速度。
3. **Rust 内存安全与系统级调用能力**：与 Windows 原生 API、文件系统、Tree-sitter C FFI 原生互操作。

### 决策 (Decision)
选用 Zed 团队开源的 **GPUI** 框架（配合 `gpui-component` 组件生态）：
- 采用 GPU 直接渲染机制（Windows 上底层基于 Direct3D 11/12），文本排版和形状绘制皆在 GPU 上高效流水化。
- 采用响应式实体/上下文架构（`Entity<T>`、`Context<T>`），状态变更与脏标记渲染自然分离。
- 采用 Rust 2024 Edition，启用 Windows 子系统配置（`#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]`），隐藏控制台黑框。

### 影响与评估 (Consequences)
- **正面**：冷启动在数十毫秒以内，空载内存占用通常在 30~50MB 区间；渲染流畅，字体抗锯齿表现优秀。
- **负面**：GPUI 在 Windows 平台的生态仍在快速迭代，部分 Windows 原生事件机制（如拖放、部分 IME 边角情况）需要业务层自行封装规避。

---

## ADR-002：Windows 原生对话框与 UI 线程解耦 —— 独立线程运行 COM 对话框

### 背景 (Context)
在初版实现中，直接在 GPUI 的实体更新回调（如 `cx.listener`）中调用 `rfd::FileDialog::open_file()` 或 `MessageDialog::show()`。在 Windows 操作系统中，打开文件、保存文件以及错误弹窗频繁发生闪退、死锁或以下严重错误：
```text
BorrowMutError: already borrowed: BorrowMutError
```
其本质原因：Windows 原生通用对话框（`IFileDialog`）底层依赖 COM 组件。在弹出模态对话框时，Windows 会接管当前线程并启动内部的消息泵（Message Pump），拦截并分发窗口消息。由于该消息分发发生在同一个 OS 线程内，GPUI 框架收到了嵌套的重入事件，触发对已处于可变借用状态的上下文进行二次借用，导致立即 Panic。

### 决策 (Decision)
引入统一的模态对话框运行器 `native_dialog`：
```rust
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
```
所有原生文件选择器（打开单个/多个文件、选择目录、另存为）以及原生确认/错误弹框，统一强制投递到独立的 Worker 线程中执行并等待结果。

### 影响与评估 (Consequences)
- **正面**：彻底隔离了 COM 单线程套间 (STA) 与 GPUI UI 渲染线程的消息循环，杜绝了重入借用崩溃。
- **负面**：模态等待期间阻塞发起线程（等待 Worker join），但对于用户本就必须交互的原生对话框而言完全符合交互预期。

---

## ADR-003：轻量级层级文件树与虚拟化目录缓存

### 背景 (Context)
IDE/编辑器需要提供左侧文件树，支持按需展开/折叠、状态缓存、文件快速检索与打开。若采用全量递归扫描大工程目录，容易发生 I/O 阻塞并消耗过多内存。

### 决策 (Decision)
1. **按需延迟加载与内存缓存**：
   - 维护 `tree_cache: HashMap<PathBuf, Vec<TreeEntry>>` 与 `expanded: HashSet<PathBuf>`。
   - 只有当用户点击展开某个文件夹节点时，才触发 `read_directory` 遍历该子目录并写入缓存。
2. **专业排序策略**：
   - 目录优先于文件（`b.directory.cmp(&a.directory)`）。
   - 同类型按大小写不敏感自然排序（Windows 文件名惯例），再做严格 Unicode 二级排序。
3. **视觉状态联动**：
   - 支持全局快捷键 `Ctrl+B` 显隐，`Ctrl+Shift+O` 动态切换根目录。
   - 当前激活 Tab 的文档路径在左侧树中予以视觉高亮。

---

## ADR-004：多语言语法高亮引擎 —— Tree-sitter 单例与按需高亮集成

### 背景 (Context)
现代编辑器不能局限于纯文本，必须对常见代码和标记语言（Markdown、Rust、Python、JavaScript、C/C++、Go、JSON 等 30+ 种）提供语法分析与高亮渲染。同时，解析器不能在每次切标签或键入时重复编译生成。

### 决策 (Decision)
1. **依赖与特性控制**：
   在 `Cargo.toml` 中配置：
   ```toml
   gpui-component = { version = "=0.5.0", features = ["tree-sitter-languages"] }
   ```
2. **规则化语言映射与 fallback 机制**：
   - 实现 `detect_language(path: Option<&Path>) -> (&'static str, &'static str)`。
   - 涵盖系统级语言（Rust、C、C++、C#、Go 等）、脚本语言（Python、Shell 等）、Web 栈（JS/TS/TSX/HTML/CSS）、配置/数据（JSON/TOML/YAML/SQL/Proto）及工程构建（CMakeLists.txt、Makefile 等）。
   - 默认回退到通用纯文本 `("text", "纯文本")`。
3. **单例模式**：
   通过 `gpui_component::highlighter::LanguageRegistry::singleton()` 共享 Tree-sitter 语法定义，避免多文档打开时的冗余内存占用。

---

## ADR-005：Windows 启动参数与文件关联深度适配 (无损 Unicode & 路径归一化)

### 背景 (Context)
在 Windows 系统中将 Ferrite 设为某类文件（例如 `.md`）的默认打开方式后，双击文件时 Windows 会通过命令行参数传入被点击文件的完整路径。
传统使用 `std::env::args()` 的做法会强行以 UTF-8 校验字符串，且易丢失复杂的 Windows 本地化路径；同时未处理启动参数导致程序直接打开空白默认文档，无法加载目标文件。

### 决策 (Decision)
1. **无损操作系统字符串解析**：
   - 使用 `std::env::args_os()` 接收原生的 `OsString`，经过 `startup_paths` 滤除空参数，支持包含空格、中文等宽字符及符号的路径。
   - 将相对路径统一通过当前工作目录 `current_dir` 转换为绝对路径。
2. **启动上下文自动初始化**：
   - 遍历传入的文件列表：若传入的是目录，直接将其设定为左侧文件树根目录；
   - 若传入的是文件，将其父目录设为左侧树根目录，并依次将文件加入编辑器标签栏，最终激活首选文件，完成语法高亮与状态计算。

---

## ADR-006：多标签页 (Tab) 架构与单实例防重复冲突机制

### 背景 (Context)
多文件编辑需要良好的 Tab 容器模型，同时必须防止同一个文件被重复在多个 Tab 打开，进而产生“覆盖写丢失”（Lost Update）冲突。

### 决策 (Decision)
1. **解耦的文档实体架构**：
   定义独立的 `Document` 结构体：
   ```rust
   struct Document {
       input: Entity<InputState>,
       path: Option<PathBuf>,
       saved_text: String,
       crlf: bool,
       bom: bool,
       untitled_id: usize,
       _subscription: Subscription,
   }
   ```
   每个 Tab 拥有独立的撤销重做栈、光标位置、语法高亮器、BOM 状态及换行模式（CRLF / LF）。
2. **路径规范化与单实例防重复 (Deduplication)**：
   - 借助 `canonicalize` 得到文件的规范路径（Resolving Junction / Symlink / 大小写差异）。
   - 当用户从文件树、`Ctrl+O` 快捷键或命令行再次打开同一文件时，直接比对 `tab.path`，若已存在则直接将 `active_tab` 切换至该标签页，不产生冗余副本。
3. **另存为跨 Tab 防重保护**：
   在 `save_as` 操作时，校验目标路径是否已被其他 Tab 占用，若已打开则弹出明确提示并阻止覆盖，防止文件内容被静默冲刷。
4. **状态感知销毁流程**：
   - 关闭 Tab 时优先校验当前文档 `dirty()` 状态。
   - 未保存文档触发原生安全对话框（保存 / 不保存 / 取消）。
   - 最后一个 Tab 被关闭后自动新建未命名文档，保持 UI 不坍塌。

---

## 重点、难点剖析

| 模块 | 核心难点 | 解决方案与攻关要点 |
| :--- | :--- | :--- |
| **Windows UI 线程冲突** | Win32 消息循环嵌套导致 GPUI Context 二次可变借用 Crash | 剥离独立 Worker 线程执行 `native_dialog`，严格隔离 COM 模态与 GPU 渲染流 |
| **无损文本编解码** | 跨平台文件换行（CRLF vs LF）及 UTF-8 BOM 标记丢失导致 Git Diff 混乱 | 在读取时侦测 `\r\n` 与 `0xEF, 0xBB, 0xBF`，写回时严格还原原格式 |
| **大规模语法高亮并发** | 多 Tab 同时打开多个语法差异巨大的源码文件，解析器内存爆炸 | 利用 `LanguageRegistry::singleton()` 共享静态解析器池，按需切换高亮实例 |
| **OS 命令行集成** | 双击文件关联启动时带有长路径、空格及不同代码页编码 | 使用 `args_os()` 并在应用入口进行无损规范化解析，自适应设置工作区与 Tab |

---

## 项目避坑指南 (Windows + GPUI 生态)

1. **切勿在 GPUI 上下文回调中直接调用阻塞式 Win32 API / COM 对话框**：
   - *坑*：直接在 `cx.update` 里调用 `rfd::FileDialog`，在 Windows 下极高概率引发重入崩溃。
   - *解*：必须包裹在独立的 `std::thread::spawn` 中 `join` 等待。
2. **Windows 下输入框换行符必须规范为 LF 供代码编辑器消费**：
   - *坑*：直接将包含 `\r\n` 的文本送入 Tree-sitter 或 `InputState`，行号和光标在换行处会出现多余偏移或退格不同步。
   - *解*：在 `decode_text` 中将文本行尾统一规范为 `\n` 给编辑器内核，记录 `crlf: bool` 标记；在落盘 `encode_text` 时再统一转回 `\r\n`。
3. **文件规范化（Canonicalize）的 UNC 坑**：
   - *坑*：Windows 下 `fs::canonicalize` 会给路径加上 `\\?\` 前缀，导致某些原生 API 或字符串比对失败。
   - *解*：实现 `normalized_path` 兼容回退机制，确保路径比对逻辑在相对/绝对路径下一致。
4. **编译期依赖与链接问题**：
   - *坑*：构建 Tree-sitter C 源码时若多线程并行过高或路径包含特殊符号，C 编译阶段容易发生超时或链接冲突。
   - *解*：构建命令配置合理的 `-j` 并发参数；锁定 `gpui-component` 与 Tree-sitter 依赖版本。

---

## Rust / 高性能客户端面试核心考点

1. **Rust 所有权与 GUI 状态管理 (Deref / DerefMut 与实体借用)**：
   - *考点*：GUI 框架普遍存在双向引用与事件回调，Rust 如何解决所有权与借用冲突？
   - *Ferrite 实践*：利用 GPUI 的 `Entity<T>`（受管引用的强弱引用模型）和 `cx.subscribe_in` 事件发布订阅模式。通过对 `Editor` 实现 `Deref<Target = Document>`，在单文档操作时享受透明解引用的优雅语法，同时底层通过精准可变借用保证数据安全。
2. **多线程边界与 Send / 'static 约束**：
   - *考点*：Rust 跨线程传递闭包为何需要 `FnOnce() + Send + 'static`？
   - *Ferrite 实践*：`native_dialog` 的实现展示了标准的类型约束。闭包携带所有权跨越线程边界，返回值通过通道或 `join().unwrap()` 安全送回，充分体现并发安全无数据竞争。
3. **文件 I/O、内存映射与字符编码安全**：
   - *考点*：如何严格区分非法 UTF-8、二进制文件以及合法的本地化编码？
   - *Ferrite 实践*：在载入阶段进行快速二进制嗅探（如检测 null 字节 `0x00` 与无效 UTF-8 序列），防止破坏二进制文件；支持零拷贝嗅探 BOM 与 CRLF。
4. **GUI 架构模式演进（Retained Mode vs Immediate Mode vs GPUI 混合模型）**：
   - *考点*：GPUI 与传统 Dear ImGui 或 Qt 有何区别？
   - *Ferrite 实践*：GPUI 拥有类似 React 的声明式 DSL（基于流式 Builder API），但在底层构建的是由 GPU 批处理绘制的高性能渲染树。既保证了声明式编程的开发效率，又杜绝了即时模式下的重复状态计算开销。
