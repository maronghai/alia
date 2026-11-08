# Rust & GPUI 桌面客户端实战经验与技能总结

> **项目沉淀**：基于 Ferrite 文本编辑器 (Windows 平台) 的 Rust + GPUI 生产级开发实践  
> **核心技术栈**：Rust (2024 Edition) + GPUI 0.2 + gpui-component 0.5 + Tree-sitter + rfd 0.15  
> **文档定位**：架构心智模型、核心设计模式、Windows 原生集成避坑指南、重难点深度剖析及高频面试考点。

---

## 目录
1. [GPUI 核心心智模型与架构认知](#1-gpui-核心心智模型与架构认知)
2. [GPUI 核心设计模式与 Rust 结合技巧](#2-gpui-核心设计模式与-rust-结合技巧)
3. [Windows 平台级深水区踩坑与解决方案](#3-windows-平台级深水区踩坑与解决方案)
4. [高性能文本与 Tree-sitter 语法高亮集成经验](#4-高性能文本与-tree-sitter-语法高亮集成经验)
5. [项目重难点深度剖析](#5-项目重难点深度剖析)
6. [Windows + GPUI 避坑速查手册](#6-windows--gpui-避坑速查手册)
7. [Rust 与高性能客户端开发面试核心考点](#7-rust-与高性能客户端开发面试核心考点)

---

## 1. GPUI 核心心智模型与架构认知

### 1.1 区别于传统 GUI 框架的渲染管线
- **DirectX/Metal GPU 硬件加速**：
  传统跨平台客户端（如 Electron / Qt 默认部件）常依赖昂贵的多进程 Chromium 引擎或 CPU 软件绘制缓冲；GPUI 从底层起就针对现代 GPU 设计。在 Windows 平台通过 Direct3D 11/12 进行文本排版塑形与矩形/边框图元批处理渲染，冷启动数十毫秒，空载内存通常仅在数十兆区间。
- **混合架构模型 (Hybrid Model)**：
  - **声明式构建流**：采用类似 React 的链式声明式 DSL（`div().flex().child(...)`），所见即所得；
  - **持久化实体树 (Retained Entity Graph)**：底层状态常驻内存，仅当数据变更触发通知时，才脏标记关联子树参与重排和 GPU Draw Call 提交，避免了即时模式（如 Dear ImGui）每帧全量重跑业务逻辑与内存分配的 CPU 开销。

### 1.2 Entity 与 Context 核心概念
GPUI 的运行时围绕 `Entity<T>`（受管实体）与 `Context`（生命周期与事件调度上下文）运转：
```rust
// 实体创建
let editor = cx.new(|cx| Editor::new(window, cx));
```
- **`Entity<T>`**：带引用计数与脏状态标记的轻量句柄（类似于具备事件总线感知的 `SlotMap` 句柄），组件间避免直接持有裸引用或复杂自引用结构；
- **`Context<T>`**：负责调度事件循环、通知重绘（`cx.notify()`）、挂载动作快捷键（`cx.bind_keys()`）、订阅子实体事件（`cx.subscribe_in()`）及管理异步任务（`cx.spawn()`）。

---

## 2. GPUI 核心设计模式与 Rust 结合技巧

### 2.1 利用 Rust `Deref` / `DerefMut` 模式实现多文档透明代理
在多标签页（Tab）架构中，外层容器是 `Editor`，内层维护 `tabs: Vec<Document>` 和 `active_tab: usize`。如果所有业务逻辑都显式书写 `self.tabs[self.active_tab].xxx`，代码将极其冗余且容易在越界时引发 panic。

**工程实践：**
```rust
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
```
- **收益**：在 `Editor` 内部方法中，可直接调用 `self.input`、`self.path`、`self.crlf`、`self.dirty(cx)`，在语法层面获得与单文档时期完全一致的人机工程学体验；当切换标签页（改变 `self.active_tab`）时，所有基于解引用的状态自然指向新的活跃文档。

### 2.2 响应式事件订阅与 RAII 生命周期绑定
子组件（如 `InputState`）的内容变动需要实时通知父级刷新窗口标题（显示未保存圆点 `●`）和状态栏：
```rust
let subscription = cx.subscribe_in(&input, window, |_, _, event, _, cx| {
    if matches!(event, InputEvent::Change) {
        cx.notify(); // 标记当前 Entity 脏，触发下一帧 GPU 重绘
    }
});
```
- **关键规则**：必须将返回的 `Subscription` 存入结构体字段（如 `_subscription: Subscription`）。一旦它离开作用域被 drop，事件监听即刻自动注销，符合 Rust 的 RAII 资源回收原则。

### 2.3 状态驱动的流式排版 DSL
GPUI 通过流式方法链表达弹性布局（Flexbox）：
```rust
div()
    .size_full()
    .flex()
    .flex_col()
    .bg(rgb(0x141b29))
    .on_action(cx.listener(Self::new_file))
    .on_action(cx.listener(Self::open_file))
    .child(self.tab_bar(cx))
    .child(
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .children(self.tree_visible.then(|| self.tree_panel(cx)))
            .child(self.editor_pane(cx))
    )
    .child(self.status_bar(cx))
```
- **技巧**：善用 `self.tree_visible.then(|| ...)` 或 `.when(bool, |el| ...)` 进行条件挂载，避免多层嵌套的 `if-else` 切割流式调用。

---

## 3. Windows 平台级深水区踩坑与解决方案

### 3.1 致命的 Win32 COM 消息循环重入崩溃
#### 现象
在 GPUI 上下文回调中直接调用 `rfd::FileDialog` 打开或保存文件时，Windows 经常发生闪退或抛出：
```text
BorrowMutError: already borrowed: BorrowMutError
```

#### 原因剖析
1. Windows 原生通用文件对话框（`IFileDialog`）底层基于 COM。
2. 弹出原生模态对话框时，Windows 会接管当前 OS 线程并就地启动局部的 Windows 消息泵（Message Pump）。
3. 如果调用发生在 GPUI 的更新事务内，COM 消息循环会在同一线程内分发新的窗口消息给 GPUI 运行时。
4. GPUI 尝试再次对已处于可变借用状态的上下文进行二次借用，直接触发 Rust 运行时的动态借用检查（RefCell 语义）Panic。

#### 解决方案：剥离至独立 OS Worker 线程
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
- 在全新线程中启动模态对话框，该线程持有独立的 Windows 消息队列；
- UI 主线程通过 `join()` 等待其返回，物理阻断了 COM 消息循环对 UI 上下文的重入途径。

---

### 3.2 Windows 启动参数无损 Unicode 解析与工作区挂载
#### 现象
将编辑器设为 `.md` 文件的默认打开方式后，双击文件时程序能启动但无法加载文件内容，甚至在遇到包含中文、空格或圆括号路径时发生乱码或 Panic。

#### 根因与解法
1. **切勿使用 `std::env::args()`**：它会强制以 UTF-8 校验传入的命令行，遇到非法 Unicode 序列会产生丢失或错误。
2. **改用 `std::env::args_os()` 接收无损 `OsString`**：
```rust
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
```
3. **工作区智能挂载策略**：
   - 若传入的是目录，直接设定为左侧文件树根目录；
   - 若传入的是文件，将其父目录设定为文件树根目录，并自动开启 Tab 加载该文件、激活对应的语法高亮。

---

### 3.3 跨平台换行符 (CRLF/LF) 与光标/高亮精准同步
#### 陷阱
Windows 文件通常使用 `\r\n`。若将包含 `\r` 的文本直接喂给 Tree-sitter 语法分析器或 `InputState`：
- Tree-sitter 会把 `\r` 计算为多余字节偏移，导致语法高亮区间错位；
- 换行与退格时容易出现光标漂移、退格不同步等问题。

#### 解法：规范化内存模型与保真持久化
1. **载入解码时统一归一化为 `\n`**，并记录 `crlf: bool` 与 `bom: bool` 标识：
```rust
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
```
2. **落盘保存时原样还原本地方格**：
```rust
fn encode_text(text: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = if crlf { normalized.replace('\n', "\r\n") } else { normalized };
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}
```
- 编辑器内核与 Tree-sitter 纯粹以 `\n` 为基准计算行号和光标；落盘时不改变原文件的换行符与 BOM，杜绝污染 Git 提交记录。

---

## 4. 高性能文本与 Tree-sitter 语法高亮集成经验

### 4.1 共享单例与内存控制
多标签页编辑器若为每个打开的 Tab 编译构建独立的语法解析树表，内存会严重膨胀。
```toml
# Cargo.toml
gpui-component = { version = "=0.5.0", features = ["tree-sitter-languages"] }
```
- 利用 `LanguageRegistry::singleton()` 共享全生命周期的解析器元数据；
- 文档加载时按文件名和后缀快速识别映射（如 `rs -> rust`, `md -> markdown`），然后调用 `state.set_highlighter(language, cx)` 完成按需高亮。

### 4.2 鲁棒的语言自动识别矩阵
除常见后缀名外，必须覆盖系统级工程的无后缀及特殊配置文件：
- `Makefile` / `makefile` / `*.mk` -> `make`
- `CMakeLists.txt` / `*.cmake` -> `cmake`
- `.bashrc` / `.bash_profile` / `.profile` -> `bash`
- `.dockerignore` / `.gitignore` / `*.diff` -> 差异与配置规则

---

## 5. 项目重难点深度剖析

| 攻关模块 | 核心难点 | 架构攻关要点 |
| :--- | :--- | :--- |
| **Tab 冲突与覆盖写丢失** | 多个标签页或另存为指向同一物理文件 | 结合 `canonicalize` 路径规范化比对；打开已存在文件强制切换 Tab，另存为冲突时前置安全拦截。 |
| **编码与二进制防破坏** | 误把非文本或含有空字节的文件当作文本打开 | 检测 UTF-8 BOM 签名的同时，嗅探是否存在 Null 字节 `0x00`，对非法格式或二进制文件及时拒绝。 |
| **层级文件树与虚拟化缓存** | 目录项全量递归扫描引发 I/O 阻塞 | 采用 `tree_cache: HashMap<PathBuf, Vec<TreeEntry>>` 懒加载，点击文件夹时按需载入并缓存，采用“目录优先 + 自然排序”。 |
| **窗口生命周期守卫** | 关闭窗口时需要保证所有未保存 Tab 安全提示 | 注册 `window.on_window_should_close` 钩子，遍历所有 Tab 依次确认未保存状态，用户取消时安全拦截窗口关闭。 |

---

## 6. Windows + GPUI 避坑速查手册

1. **禁用 Windows 控制台黑框**：
   在 `src/main.rs` 顶部声明 `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]`，构建发布版时不会伴随弹出 CMD 黑色终端。
2. **警惕 `fs::canonicalize` 的 Windows UNC 前缀**：
   Windows 下 `canonicalize` 会给绝对路径加上 `\\?\` 前缀（如 `\\?\C:\project`）。字符串显示或部分外部工具调用时需做容错处理。
3. **关闭最后一个 Tab 的安全降级**：
   在多标签页编辑器中，关闭最后一个 Tab 时不要直接退出应用或留下空白画布，而是应自动新建一个未命名空文档（`untitled_id` 递增），维持界面完整性。
4. **编译期并发控制**：
   Tree-sitter 语言特性依赖大量 C/C++ 语法源文件编译，若在 Windows 上并发过高易因 MSVC 文件锁或内存压力导致编译卡死，构建命令推荐显式添加 `-j 2` 控制并发。

---

## 7. Rust 与高性能客户端开发面试核心考点

### 考点 1：Rust 中 GUI 框架所有权难题如何破解？
- **考点提问**：传统面向对象 GUI（如 Qt、WPF）依赖广泛的双向指针与父子互相引用，在 Rust 严格的所有权与借用规则下，GPUI 是如何解决的？
- **答题核心**：
  1. GPUI 采用了**句柄/实体机制 (`Entity<T>`)**，类似基于 SlotMap 的弱类型受管索引；组件间不直接持有对方的 raw reference，而是持有轻量级句柄。
  2. 状态读写只能在 `cx.update(|this, cx| { ... })` 提供的受限生命周期内完成，确保在任何给定的时刻，同一实体的可变借用始终唯一。
  3. 视图树与状态树分离：View 仅负责从 Context 读取数据并构建 Render Tree，不持久化子组件的复杂引用链。

### 考点 2：跨线程闭包约束 `FnOnce() + Send + 'static` 深度理解
- **考点提问**：为什么 `native_dialog` 的实现中，参数约束必须包含 `Send + 'static`？
- **答题核心**：
  1. `Send` 保证闭包所捕获的环境变量（Upvars）能安全转移至新生成的 OS 线程所有权空间。
  2. `'static` 约束要求被捕获的数据不得包含非全局的生命周期引用（借用），因为新线程的生命周期独立于调用栈，可能在当前函数栈帧退出后才执行，必须拥有捕获变量的完整所有权。

### 考点 3：GUI 渲染架构：即时模式 (Immediate Mode) vs 保留模式 (Retained Mode) vs GPUI 混合模型
- **考点提问**：对比 Dear ImGui（即时）、WPF/DOM（保留）与 GPUI，各自权衡是什么？
- **答题核心**：
  - **Dear ImGui**：极简、无状态同步困扰，但每帧全量重算造成高 CPU 消耗，复杂排版支持较弱。
  - **DOM / WPF**：构建庞大对象树并做 Diff，表达力强但框架臃肿、内存与启动性能开销大。
  - **GPUI**：保留模式的底座（Entity 图谱）+ 声明式的 Builder DSL。仅在 Entity 触发 `cx.notify()` 时，才将该局部子树提交至 GPU 流水线进行重排与批处理渲染（Instanced Rendering），兼顾极简心智模型与极致渲染性能。

### 考点 4：文本编辑器底层数据结构演进与抉择
- **考点提问**：大文档编辑为什么不能只用简单的 `String`？Piece Table、Rope 与 Gap Buffer 各有何优劣？
- **答题核心**：
  - `String`（连续数组）：插入/删除复杂度为 $O(N)$，大文件（数百兆）每次敲击都发生大量内存拷贝。
  - **Gap Buffer**：光标周围留空，局部打字为 $O(1)$，但在文档跨处跳转时移动光标需移动间隙。
  - **Rope（绳索数据结构，如 Zed 底层）**：基于不可变平衡二叉树（如 B-Tree）分割文本切片，插入、删除、切分均为 $O(\log N)$，极度适合并发、长文本以及撤销重做（持久化历史树）。
  - **Piece Table（如 VS Code）**：原始只读缓冲 + 追加缓冲，由双向链表或红黑树维护片段索引，内存紧凑且天然支持无限撤销。


## 8. 资源管理器海量项与 Tab 联动滚动实战经验

### 8.1 核心痛点与问题本质
当项目文件规模扩大（深层目录、数十乃至数百个文件）时，资源管理器出现纵向滚动条。
如果仅展开目录和高亮当前文件，被高亮的文件很可能位于视口之外（在最上方或最下方），用户切换 Tab 后在资源管理器视窗内看不到当前文件。
如果在每帧重绘时无脑将当前文件拉回中央，又会导致用户在代码区打字输入或者手动滚轮查看侧边栏时，滚动条被强制跳回，极大伤害交互体验。

### 8.2 GPUI `ScrollHandle` 核心技术要点
1. **`ScrollHandle` 的数据通道**：
   - GPUI 的 `ScrollHandle` 底层为 `Rc<RefCell<ScrollHandleState>>`，内部包含 `offset: Point<Pixels>`、`bounds: Bounds<Pixels>` 与 `child_bounds: Vec<Bounds<Pixels>>`。
   - 必须通过 `.track_scroll(&self.tree_scroll_handle)` 绑定在带有 `.overflow_y_scroll()` 的容器上，GPUI 才能在 prepaint 阶段捕获布局并响应滚动。
2. **`Pixels` 类型安全与转换**：
   - GPUI 的 `Pixels` 是 tuple struct `Pixels(pub(crate) f32)`，其内部字段是私有的。直接访问 `.0` 会触发 `E0616` 编译错误。
   - 正确转换方式是使用 `f32::from(pixels)` 或 `f32::from(&pixels)`。
3. **视口舒适区防抖与垂直居中算法**：
   ```rust
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

       // 上下预留 1.5 行高（约 42px）舒适余量，若已在视口舒适区内，则保持原样不跳动
       let margin = row_height * 1.5;
       if item_top_in_viewport >= margin && item_bottom_in_viewport <= (viewport_height - margin) {
           return None;
       }

       // 否则将目标行垂直中心对齐视口中心，并限制上下边界防止露白
       let ideal_offset_y = (viewport_height - row_height) / 2.0 - item_top_in_content;
       let target_offset_y = ideal_offset_y.clamp(-max_scroll, 0.0);
       Some(target_offset_y)
   }
   ```
4. **`Cell<bool>` 延迟单次消费模式**：
   - 在 `Editor` 中维护 `scroll_tree_to_active: Cell<bool>`。
   - 仅在切换 Tab（点击、快捷键、关闭）、打开文件、保存/另存为时置为 `true`。
   - 在 `tree_panel` 视图渲染时通过 `self.scroll_tree_to_active.take()` 单次消费并执行定位，彻底避免日常编辑与滚轮浏览被打扰。
5. **双保险兜底**：
   - 计算偏移后通过 `set_offset(Point::new(px(0.), px(target_y)))` 立即更新视口。
   - 同步调用 GPUI 原生的 `self.tree_scroll_handle.scroll_to_item(target_index)`，确保即使初次展开视口 bounds 尚未测量的首帧，GPUI 渲染管线也能将目标项目精准呈现于可视区中。


## 9. 卡顿排查：渲染热路径与 Rope 接口

2026-11-08：检查发现资源管理器每次构建界面都逐项调用 `same_path`，间接执行两次 `fs::canonicalize`。改为直接比较从规范化根目录生成的路径，磁盘规范化只在打开文件、建立根目录和目录联动时进行。不要在渲染循环里执行同步文件系统查询。

`gpui-component 0.5.0` 的 `InputState::value()` 虽然返回 `SharedString`，实现中仍会执行 `self.text.to_string()`，不能把返回类型当成零拷贝保证。改用 `text()` 获取 Rope，分块比较已保存文本；缓存每个文档的未保存状态、行数和字符数，在 `InputEvent::Change` 中更新，加载和保存时同步更新。渲染只读取缓存。关闭前实时按 Rope 分块核对，撤销回原文仍应恢复未修改状态。

范围限制：文件树仍构建所有展开行，并未实现可视行虚拟化；大文件编辑、目录读取和原生对话框等待仍可能占用 UI 时间。当前产物沿用 debug 构建；未测量具体帧率、延迟和加速倍数，不把优化方案等同于实测流畅性。


## Minimap：Canvas、缓存与布局约束

- 右侧固定宽度面板与正文并列时，应让正文 flex_1、min_w_0、overflow_hidden，侧栏 flex_shrink_0，防止长行撑开布局。
- 使用 Canvas 与 paint_quad(fill(...)) 绘制微小字符笔画，避免每行创建 Div。按文档缓存笔画，InputEvent::Change 时失效；为采样行数和列数设置上限。
- Canvas 的 prepaint 阶段记录 Bounds，点击用 event.position.y 减 bounds.origin.y，再按实际绘制高度映射行号。短文档未占满面板时也应使用实际绘制高度。
- gpui-component 0.5.0 未公开 InputState 的滚动句柄；当前框线按光标和面板高度估算，不是实际滚动视口。轻量字符配色也不等同完整语法高亮。不要把上述近似实现描述为与 Sublime Text 完全一致。


## 编辑区滚动优化：限制 Minimap 场景图元

2026-11-08：静态检查发现 Minimap 缓存了字符坐标，但 Canvas 绘制仍逐字符提交矩形。现在合并连续同色字符，每行最多 16 段，最多采样 320 行，因此缩略图正文每次绘制最多提交 5120 个矩形（另有位置框与光标标记）。达到段数上限时将后续字符并入最后一段，部分空隙和配色会简化。保留缩进、空行、点击定位和显示隐藏。窗口标题增加缓存，仅在文字改变时调用设置函数。

这是绘制工作量上限优化，不是实测帧率结论；未测量实际滚轮帧耗时，也没有证据证明 Minimap 是唯一瓶颈。当前正文自动换行行为保持不变。


---

## 10. Minimap 视口联动平滑滚动、滚轮拦截与拖拽 Scrubbing 实现经验

### 10.1 长短文档自适应滚动模型
在设计代码缩略图（Minimap）时，常见误区是将所有行等比例压缩进固定高度面板。对于上千行的源码，每行被压缩到不足 0.2px，不仅画面糊成一片，而且失去缩进可读性。
Sublime Text / VS Code 的成熟做法是：
- 赋予 Minimap 固定的微型行高（如 `2.4px`），真实还原代码空行和缩进结构；
- 当 `content_height > container_height` 时，Minimap 自身随主视口滚动而平滑滚动：
  $$scroll\_y = \frac{cursor\_line}{total\_lines - 1} \times (content\_height - container\_height)$$
- 视口滑块（Viewport Slider）指示当前可见代码范围，并始终保持在 Minimap 视口内部。

### 10.2 视口范围切片与 GPU 零掉帧优化
若盲目遍历几千行代码的图元提交给 GPU，滚动时必然产生微掉帧。
通过 `calculate_minimap_scroll` 计算得到的 `visible_range`：
```rust
let start_row = (scroll_y / line_height).floor().max(0.0) as usize;
let end_row = (((scroll_y + container_height) / line_height).ceil() as usize).min(total_lines);
```
无论文件有 1000 行还是 50000 行，每帧仅为可见的 200~300 行提交轻量矩形，渲染耗时恒定在微秒级。

### 10.3 GPUI 滚轮与拖拽事件组合
- `on_scroll_wheel`：监听 `ScrollWheelEvent`，提取 `delta: ScrollDelta`（支持 `Lines` 和 `Pixels` 两种模式），将垂直滚轮增量换算为行推进量并更新 `InputState`；
- `on_mouse_down` + `on_mouse_move` + `on_mouse_up` / `on_mouse_up_out`：维护简单的 `minimap_dragging: bool` 状态机，鼠标按下后在移动过程中持续调用 `minimap_coord_to_line` 实施 Scrubbing，实现雷达导航般的平滑滑动体验。
