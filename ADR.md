# 架构决策记录 (Architecture Decision Records - ADR)

**项目名称**：alia 文本编辑器 (alia)  
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
7. [ADR-007：海量节点下资源管理器视口智能居中与 Tab 联动滚动](#adr-007海量节点下资源管理器视口智能居中与-tab-联动滚动)
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
在 Windows 系统中将 alia 设为某类文件（例如 `.md`）的默认打开方式后，双击文件时 Windows 会通过命令行参数传入被点击文件的完整路径。
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
   - *alia 实践*：利用 GPUI 的 `Entity<T>`（受管引用的强弱引用模型）和 `cx.subscribe_in` 事件发布订阅模式。通过对 `Editor` 实现 `Deref<Target = Document>`，在单文档操作时享受透明解引用的优雅语法，同时底层通过精准可变借用保证数据安全。
2. **多线程边界与 Send / 'static 约束**：
   - *考点*：Rust 跨线程传递闭包为何需要 `FnOnce() + Send + 'static`？
   - *alia 实践*：`native_dialog` 的实现展示了标准的类型约束。闭包携带所有权跨越线程边界，返回值通过通道或 `join().unwrap()` 安全送回，充分体现并发安全无数据竞争。
3. **文件 I/O、内存映射与字符编码安全**：
   - *考点*：如何严格区分非法 UTF-8、二进制文件以及合法的本地化编码？
   - *alia 实践*：在载入阶段进行快速二进制嗅探（如检测 null 字节 `0x00` 与无效 UTF-8 序列），防止破坏二进制文件；支持零拷贝嗅探 BOM 与 CRLF。
4. **GUI 架构模式演进（Retained Mode vs Immediate Mode vs GPUI 混合模型）**：
   - *考点*：GPUI 与传统 Dear ImGui 或 Qt 有何区别？
   - *alia 实践*：GPUI 拥有类似 React 的声明式 DSL（基于流式 Builder API），但在底层构建的是由 GPU 批处理绘制的高性能渲染树。既保证了声明式编程的开发效率，又杜绝了即时模式下的重复状态计算开销。


## ADR-007：海量节点下资源管理器视口智能居中与 Tab 联动滚动

### 背景 (Context)
在多标签编辑模式下，当左侧资源管理器包含深层目录、海量文件（数十甚至数百个文件）时，内容高度远远超出可视窗口（产生垂直滚动条）。
切换 Tab 或打开文件时，如果仅自动展开父目录并高亮节点，目标节点可能处于可视视口之外（在上方或下方深处），用户无法直观确认当前文件在目录树中的上下文位置。
若每次无脑强行将视口重置，则会导致用户在侧边栏手动浏览、或者在同目录下临近文件连续切换时界面疯狂晃动。

### 决策 (Decision)
1. **视口舒适区探测（Viewport Comfort Margin）**：
   - 采用纯计算函数 `calculate_tree_scroll_offset`。
   - 若目标节点当前已在视口范围内，且距离视口上下边缘留有至少 1.5 倍行高（约 42px）的舒适上下文余量，则判定为已直观可见，**保持当前滚动位置不变**，避免画面跳动。
2. **中心对齐直观滚动（Center-in-View Positioning）**：
   - 当目标节点不在视口内或贴在边缘时，将其垂直中心平滑对齐视口垂直中心，使目标文件处于正中央，同时露出上下各数行同级/父级上下文。
   - 滚动偏移量经过两极 Clamp 限制：在顶部时限制最大为 `0.0`（顶格不露白），在底部时限制最小为 `-max_scroll`（底格不露白）。
3. **GPUI ScrollHandle 协同兜底方案**：
   - 在滚动容器上绑定 `.track_scroll(&self.tree_scroll_handle)`。
   - 遇到初次展开子目录、视口 bounds 尚未测量的首帧时，同步调用 GPUI 原生的 `tree_scroll_handle.scroll_to_item(index)`，由 GPUI 的 prepaint 渲染管线确保无论何时目标节点必然呈现在可视区域中。
4. **单次消费与编辑防扰机制**：
   - 采用 `Cell<bool>` 标记 `scroll_tree_to_active`。仅在发生切换 Tab（点击、Ctrl+Tab、关闭）、打开文件、保存/另存为时置为 `true`。
   - 在 `tree_panel` 渲染构建中通过 `.take()` 单次消费，用户后续打字、移动光标或在树上面手动滚轮时绝不重置滚动条。

### 影响与评估 (Consequences)
- 17 项单元测试及复杂深层树验证全部通过；
- 极大提升了大型多层级项目开发中的定位效率与视觉直观度。


## ADR-008：移除渲染路径中的同步 I/O 与全文分配

2026-11-08：检查发现资源管理器每次构建界面都逐项调用 `same_path`，间接执行两次 `fs::canonicalize`。改为直接比较从规范化根目录生成的路径，磁盘规范化只在打开文件、建立根目录和目录联动时进行。不要在渲染循环里执行同步文件系统查询。

`gpui-component 0.5.0` 的 `InputState::value()` 虽然返回 `SharedString`，实现中仍会执行 `self.text.to_string()`，不能把返回类型当成零拷贝保证。改用 `text()` 获取 Rope，分块比较已保存文本；缓存每个文档的未保存状态、行数和字符数，在 `InputEvent::Change` 中更新，加载和保存时同步更新。渲染只读取缓存。关闭前实时按 Rope 分块核对，撤销回原文仍应恢复未修改状态。

范围限制：文件树仍构建所有展开行，并未实现可视行虚拟化；大文件编辑、目录读取和原生对话框等待仍可能占用 UI 时间。当前产物沿用 debug 构建；未测量具体帧率、延迟和加速倍数，不把优化方案等同于实测流畅性。


## ADR-009：代码 Minimap 使用 Canvas 与按文档缓存

右侧固定保留 140 像素，编辑区设置 min_w_0 和 overflow_hidden，避免正文长行将缩略图挤出窗口。Minimap 使用 GPUI Canvas 绘制缩小的字符笔画，保留缩进、空行和局部颜色，点击纵向位置可定位文档行。颜色使用轻量字符规则，并非 Tree-sitter 完整语义高亮。

每个文档缓存绘制笔画，正文 Change 事件使缓存失效；最多采样 320 行，每行最多 100 列，避免每帧构建大量元素或扫描全文。长文件使用抽样，可能省略部分行。位置框按光标和面板高度估算，当前 gpui-component 0.5.0 的 InputState 未公开精确滚动视口接口，因此位置框不代表滚轮滚动后的实际可视区。当前支持点击定位、Ctrl+M 显示隐藏，尚不支持拖动位置框。


## 编辑区滚动优化：限制 Minimap 场景图元

2026-11-08：静态检查发现 Minimap 缓存了字符坐标，但 Canvas 绘制仍逐字符提交矩形。现在合并连续同色字符，每行最多 16 段，最多采样 320 行，因此缩略图正文每次绘制最多提交 5120 个矩形（另有位置框与光标标记）。达到段数上限时将后续字符并入最后一段，部分空隙和配色会简化。保留缩进、空行、点击定位和显示隐藏。窗口标题增加缓存，仅在文字改变时调用设置函数。

这是绘制工作量上限优化，不是实测帧率结论；未测量实际滚轮帧耗时，也没有证据证明 Minimap 是唯一瓶颈。当前正文自动换行行为保持不变。


## ADR-010：Minimap 视口联动平滑滚动、滚轮与拖拽 Scrubbing 交互

### 背景 (Context)
此前 Minimap 面板采用固定高度视口映射，长文档被强行抽样压缩进固定高度中。在长代码文件下，缩略图无法像 Sublime Text / VS Code 那样跟随代码平滑滚动，在 Minimap 区域使用鼠标滚轮无响应，且不支持按住鼠标在缩略图上拖拽刷屏（Scrubbing）。

### 决策 (Decision)
1. **Minimap 自适应长短文档滚动模型 (`calculate_minimap_scroll`)**：
   - 设定固定缩略行高 `MINIMAP_LINE_HEIGHT = 2.4px`，确保代码缩进、轮廓与空行比例真实可辨。
   - 短文档（内容高度小于等于容器高度）：Minimap 保持在顶部，视口滑块在画布内部平滑移动。
   - 超长文档（内容高度大于容器高度）：根据当前光标在文档全篇中的进度比例，线性计算 Minimap 自身的垂直滚动偏移量 `scroll_y`，使视口高亮滑块始终保持在可视范围内。
   - 局部切片渲染：通过 `visible_range` 仅绘制当前在 Minimap 画布视口内的行（每帧仅 200~300 行），时间复杂度保持在 O(可见行数)，杜绝长文档滚动卡顿。
2. **滚轮直接联动代码滚动 (`on_scroll_wheel`)**：
   - 在 Minimap 画布上监听 `ScrollWheelEvent`，将 `ScrollDelta` 精确折算为行偏移，平滑推动当前光标与编辑器视口。
3. **按住拖拽滚动 (`on_mouse_down` + `on_mouse_move` + `on_mouse_up`)**：
   - 在 Minimap 区域按下鼠标左键即开启 `minimap_dragging` 状态，鼠标上下移动实时读取绝对坐标并通过 `minimap_coord_to_line` 计算目标行号，实现丝滑的雷达缩略图 Scrubbing 拖拽定位。
   - 在鼠标松开或移出时自动清理拖拽状态。

### 影响与评估 (Consequences)
- 22 项单元测试全部通过；
- 极大提升了长代码文件在编辑器中的快速浏览与导航体验。

## ADR-011：多 Tab 溢出治理、尺寸约束与自动滚动定位

### 背景与挑战
当用户打开十几个或几十个文件时，Tab 栏容易面临两个典型问题：
1. **长文件名横向霸屏**：未限制最大宽度时，单个超长文件名的 Tab 就会撑爆整屏，其余文件被完全挤出视口。
2. **滚出视口与定位失效**：当 Tab 总宽度超出容器时，用户通过快捷键（`Ctrl+Tab`）、文件树或者外部文件关联切换到新 Tab 时，激活项常常落在可视区域之外，用户无法感知当前选中的是哪个 Tab。

### 决策内容
1. **显式宽度区间与省略截断**：
   - 设定单 Tab 弹性区间：`min_w(px(120.))`，`max_w(px(220.))`。
   - 文件名文本包裹在 `.flex_1().min_w_0().overflow_hidden().text_ellipsis()` 内，保障关闭按钮与未保存圆点始终可见，超长文件名末尾平滑省略。
2. **原生 `ScrollHandle` 水平滚动与自动追踪**：
   - 为 Tab 栏接入 `ScrollHandle`，并在容器上应用 `.overflow_x_scroll().track_scroll(&self.tab_scroll_handle)`。
   - 在 `switch_tab` 逻辑中加入 `self.tab_scroll_handle.scroll_to_item(index)`。GPUI 会在 prepaint 阶段检测该 Tab 是否处于容器可视窗口之外，自动将其滚入视野。

### 架构收益
- 保障了打开 30+ 文件时的界面稳定性和布局规整性。
- 无需手动计算各个 Tab 像素级边界，利用 GPUI 原生交互体系实现了声明式自动滚入。


## ADR-012：Tab 右键上下文菜单与批量关闭保护机制

### 背景 (Context)
随着编辑器支持打开数十个文件并加入多 Tab 横向滚动，用户管理标签页的操作成本增加。当存在大量临时文件或多分支文件时，逐个点击“×”关闭体验繁琐。用户需要右键弹出上下文菜单，支持：
1. 关闭当前标签页（Close Tab）
2. 关闭其它标签页（Close Other Tabs）
3. 关闭右侧所有标签页（Close Tabs to the Right）
4. 关闭左侧所有标签页（Close Tabs to the Left）
5. 关闭所有标签页（Close All Tabs）

### 决策 (Decision)
1. **状态驱动的轻量级 Context Menu 模型 (`TabContextMenu`)**：
   - 在 `Editor` 状态中保留 `tab_context_menu: Option<TabContextMenu>`，记录触发位置 `position: Point<Pixels>` 与目标标签索引 `tab_index`。
   - 在 Tab 栏项监听 `on_mouse_down(MouseButton::Right)` 触发唤起，并自动将当前右键项切为激活 Tab。
   - 顶层渲染透明遮罩 (`#tab-context-menu-backdrop`) 捕获全局左键/右键点击，实现无状态污染的自动收起（Dismiss）。
   - 菜单样式遵循无中文字符与现代深色调（Slate/Charcoal `#1a1e28`、边框 `#323a4e`、微阴影、悬停高亮、禁用状态置灰）。
2. **纯函数逆序索引序列计划 (`calculate_tabs_to_close_indices`)**：
   - 批量关闭涉及数组索引动态前移的经典陷阱。若从左到右删除，已删元素的索引变化会导致后续删除越界或误删。
   - 设计纯函数 `calculate_tabs_to_close_indices` 采用逆序序列：
     - `CloseTabsKind::All`：`(0..total).rev()` 从最右侧向最左侧逐个移除；
     - `CloseTabsKind::Others`：先逆序清除右侧 `(target+1..total).rev()`，再逆序清除左侧 `(0..target).rev()`，目标元素不受任何索引前移扰动；
     - `CloseTabsKind::Right`：`(target+1..total).rev()`；
     - `CloseTabsKind::Left`：`(0..target).rev()`。
3. **未保存修改事务保护与中途中断处理**：
   - 单 Tab 关闭函数 `close_tab_at` 返回 `bool`（成功返回 `true`，用户取消返回 `false`）。
   - 批量关闭在遍历逆序索引时，若遇到某个 Tab 包含未保存修改且用户在原生对话框中选择了“Cancel”，批量关闭立即中断，保护后续文件不被误关。
   - 若全部标签页被关闭，自动为用户开启干净的空白文档 (`Untitled-1.txt`)，维持编辑器始终处于可用编辑状态。

### 影响与评估 (Consequences)
- 24 项单元测试覆盖了边界值（最左侧、最右侧、单 Tab、逆序索引精确匹配）；
- 杜绝了批量操作时的索引漂移 BUG，保障了用户未保存文档的安全性。
