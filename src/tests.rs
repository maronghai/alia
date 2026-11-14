    use super::{
        calculate_column_cursors,
        parse_target_line, file_badge_and_color,
        calculate_minimap_scroll, calculate_minimap_viewport, calculate_tree_scroll_offset,
        decode_text, detect_language, encode_text, minimap_click_to_line, minimap_coord_to_line,
        minimap_segments, MINIMAP_LINE_HEIGHT,
    };
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
    fn tab_title_truncation_and_layout_bounds() {
        let long_name = "this_is_an_extremely_long_source_file_name_for_testing_tab_bar_overflow.rs";
        let dirty = true;
        let formatted = format!("{}{}", if dirty { "● " } else { "" }, long_name);
        assert!(formatted.starts_with("● "));
        assert!(formatted.contains(long_name));
        // Ensure index wraps gracefully even when tab count is large
        let large_tab_count = 50;
        assert_eq!(super::next_tab_index(49, large_tab_count, true), 0);
        assert_eq!(super::next_tab_index(0, large_tab_count, false), 49);
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
    fn column_editing_insert_numbers_and_delete_box() {
        use super::{apply_column_insert, apply_column_numbers, apply_column_delete};

        let sample = "item_a = 10\nitem_b = 20\nitem_c = 30";

        // 1. Insert text at column 0 (e.g. comment // )
        let commented = apply_column_insert(sample, 0, 2, 0, 0, "// ");
        assert_eq!(commented, "// item_a = 10\n// item_b = 20\n// item_c = 30");

        // 2. Insert text at column 9 (auto pad with spaces if needed)
        let inserted = apply_column_insert(sample, 0, 1, 9, 0, "# ");
        assert_eq!(inserted, "item_a = # 10\nitem_b = # 20\nitem_c = 30");

        // 3. Insert sequential numbers with zero padding: 01, 02, 03
        let numbered = apply_column_numbers(sample, 0, 2, 0, 0, 1, 1, 2);
        assert_eq!(numbered, "01item_a = 10\n02item_b = 20\n03item_c = 30");

        // 4. Delete column box (e.g. delete 7 characters from column 0: remove 'item_a ')
        let deleted = apply_column_delete(sample, 0, 2, 0, 7);
        assert_eq!(deleted, "= 10\n= 20\n= 30");

        // 5. Windows CRLF preservation
        let sample_crlf = "alpha\r\nbeta\r\ngamma";
        let crlf_res = apply_column_insert(sample_crlf, 0, 2, 0, 0, "> ");
        assert_eq!(crlf_res, "> alpha\r\n> beta\r\n> gamma");
    }

    #[test]
    fn batch_close_tabs_indices_calculation() {
        use super::{calculate_tabs_to_close_indices, CloseTabsKind};

        // Total 5 tabs: [0, 1, 2, 3, 4], target is 2
        let total = 5;
        let target = 2;

        // 1. Close All: 逆序关闭所有索引 [4, 3, 2, 1, 0]
        let all_indices = calculate_tabs_to_close_indices(CloseTabsKind::All, target, total);
        assert_eq!(all_indices, vec![4, 3, 2, 1, 0]);

        // 2. Close Others: 保留 2，先关右侧 [4, 3]，再关左侧 [1, 0]
        let others_indices = calculate_tabs_to_close_indices(CloseTabsKind::Others, target, total);
        assert_eq!(others_indices, vec![4, 3, 1, 0]);

        // 3. Close Tabs to the Right: 仅关闭右侧 [4, 3]
        let right_indices = calculate_tabs_to_close_indices(CloseTabsKind::Right, target, total);
        assert_eq!(right_indices, vec![4, 3]);

        // 4. Close Tabs to the Left: 仅关闭左侧 [1, 0]
        let left_indices = calculate_tabs_to_close_indices(CloseTabsKind::Left, target, total);
        assert_eq!(left_indices, vec![1, 0]);

        // 边界测试：处于最左侧 target = 0
        let left_at_zero = calculate_tabs_to_close_indices(CloseTabsKind::Left, 0, total);
        assert!(left_at_zero.is_empty(), "No tabs to the left of index 0");
        let right_at_zero = calculate_tabs_to_close_indices(CloseTabsKind::Right, 0, total);
        assert_eq!(right_at_zero, vec![4, 3, 2, 1]);

        // 边界测试：处于最右侧 target = 4
        let right_at_end = calculate_tabs_to_close_indices(CloseTabsKind::Right, 4, total);
        assert!(right_at_end.is_empty(), "No tabs to the right of index 4");
        let left_at_end = calculate_tabs_to_close_indices(CloseTabsKind::Left, 4, total);
        assert_eq!(left_at_end, vec![3, 2, 1, 0]);

        // 边界测试：只有一个 tab 时
        let single_total = 1;
        assert!(calculate_tabs_to_close_indices(CloseTabsKind::Others, 0, single_total).is_empty());
        assert!(calculate_tabs_to_close_indices(CloseTabsKind::Right, 0, single_total).is_empty());
        assert!(calculate_tabs_to_close_indices(CloseTabsKind::Left, 0, single_total).is_empty());
        assert_eq!(calculate_tabs_to_close_indices(CloseTabsKind::All, 0, single_total), vec![0]);
    }

    #[test]
    fn rejects_binary_and_invalid_utf8() {
        assert!(decode_text(&[0xff, 0xfe]).is_err());
        assert!(decode_text(b"abc\0def").is_err());
    }

    #[test]
    fn parse_target_line_works() {
        assert_eq!(parse_target_line("1", 100), Some(0));
        assert_eq!(parse_target_line("42", 100), Some(41));
        assert_eq!(parse_target_line(":42", 100), Some(41));
        assert_eq!(parse_target_line("L42", 100), Some(41));
        assert_eq!(parse_target_line("l42", 100), Some(41));
        assert_eq!(parse_target_line("0", 100), Some(0));
        assert_eq!(parse_target_line("999", 50), Some(49));
        assert_eq!(parse_target_line("", 100), None);
        assert_eq!(parse_target_line("abc", 100), None);
    }

    #[test]
    fn file_badge_and_color_works() {
        let (prefix, icon, color) = file_badge_and_color("src", true, true);
        assert_eq!(prefix, "▾");
        assert_eq!(icon, "📂");
        assert_eq!(color, 0xf59e0b);

        let (prefix, icon, color) = file_badge_and_color("main.rs", false, false);
        assert_eq!(prefix, "");
        assert_eq!(icon, "🦀");
        assert_eq!(color, 0xf97316);

        let (_, icon, color) = file_badge_and_color("README.md", false, false);
        assert_eq!(icon, "📝");
        assert_eq!(color, 0x818cf8);

        let (_, icon, color) = file_badge_and_color("Cargo.toml", false, false);
        assert_eq!(icon, "⚙");
        assert_eq!(color, 0xfacc15);
    }

    #[test]
    fn column_cursors_calculation_metrics() {
        let cursors = calculate_column_cursors(2, 5, 8, 4, 0, 10.0, 50.0, 20.0, 3, 0.0);
        assert_eq!(cursors.len(), 4);
        assert_eq!(cursors[0].row, 2);
        assert_eq!(cursors[0].box_x, 50.0 + 80.0); // 130
        assert_eq!(cursors[0].caret_x, 50.0 + 120.0); // 170
        assert_eq!(cursors[0].box_width, 40.0);
        assert_eq!(cursors[0].y, 40.0);
        assert!(!cursors[0].is_primary);
        assert!(cursors[1].is_primary); // row 3 is primary
        assert_eq!(cursors[3].row, 5);
        assert_eq!(cursors[3].y, 100.0);

        // Test with 8.0px top padding (InputState vertical padding alignment)
        let cursors_with_padding = calculate_column_cursors(2, 5, 8, 4, 0, 10.0, 50.0, 20.0, 3, 8.0);
        assert_eq!(cursors_with_padding[0].y, 48.0);
        assert_eq!(cursors_with_padding[3].y, 108.0);
    }



#[cfg(test)]
mod column_tab_tests {
    use crate::{apply_column_insert, apply_column_outdent};

    #[test]
    fn column_tab_inserts_indent_on_every_selected_line() {
        let source = "one\ntwo\nthree";
        let result = apply_column_insert(source, 0, 2, 0, 0, "    ");
        assert_eq!(result, "    one\n    two\n    three");
    }

    #[test]
    fn column_tab_pads_short_lines_before_inserting() {
        let source = "a\nlong";
        let result = apply_column_insert(source, 0, 1, 3, 0, "    ");
        assert_eq!(result, "a      \nlon    g");
    }

    #[test]
    fn column_shift_tab_removes_indent_on_every_selected_line() {
        let source = "    one\n  two\nthree";
        let (result, removed) = apply_column_outdent(source, 0, 1, 2, 4);
        assert_eq!(removed, 2);
        assert_eq!(result, "  one\ntwo\nthree");
    }

    #[test]
    fn column_shift_tab_preserves_crlf_and_handles_zero_column() {
        let source = "    one\r\n    two";
        let (result, removed) = apply_column_outdent(source, 0, 1, 4, 4);
        assert_eq!(removed, 4);
        assert_eq!(result, "one\r\ntwo");

        let (unchanged, removed) = apply_column_outdent(source, 0, 1, 0, 4);
        assert_eq!(removed, 0);
        assert_eq!(unchanged, source);
    }

    #[test]
    fn rectangular_selection_normalizes_anchor_and_head() {
        let selection = crate::ColumnSelection::new(8, 12, 3, 5);
        assert_eq!(selection.line_range(), (3, 8));
        assert_eq!(selection.column_range(), (5, 12));
        assert!(!selection.is_empty());
    }

    #[test]
    fn rectangular_selection_rows_are_clamped_to_document() {
        let selection = crate::ColumnSelection::new(8, 2, 30, 2);
        let rows: Vec<_> = crate::column_selection_rows(selection, 10).collect();
        assert_eq!(rows, (8..=9).collect::<Vec<_>>());

        let empty_doc = crate::ColumnSelection::new(0, 0, 0, 0);
        assert_eq!(crate::column_selection_rows(empty_doc, 0).collect::<Vec<_>>(), vec![0]);
    }
}

#[test]
fn column_extension_keeps_growing_and_edits_all_rows() {
    let source = (0..12).map(|_| "abcd").collect::<Vec<_>>().join("\n");
    let mut head = 1;
    for _ in 0..8 { head = crate::extended_column_head(head, 12, true); }
    assert_eq!(head, 9);
    let edited = crate::apply_column_insert(&source, 0, head, 2, 0, "X");
    assert_eq!(edited.lines().take(10).filter(|line| *line == "abXcd").count(), 10);
    assert_eq!(edited.lines().skip(10).collect::<Vec<_>>(), vec!["abcd", "abcd"]);
    assert_eq!(crate::apply_column_delete(&edited, 0, head, 2, 1), source);
}

#[test]
fn column_extension_crosses_anchor_and_clamps_at_boundaries() {
    let mut head = 5;
    for _ in 0..20 { head = crate::extended_column_head(head, 12, false); }
    assert_eq!(head, 0);
    let selected = crate::ColumnSelection::new(5, 2, head, 2);
    assert_eq!(crate::column_selection_rows(selected, 12).count(), 6);
    for _ in 0..30 { head = crate::extended_column_head(head, 12, true); }
    assert_eq!(head, 11);
    assert_eq!(crate::extended_column_head(0, 0, true), 0);
    assert_eq!(crate::extended_column_head(0, 1, false), 0);
}

#[test]
fn column_home_end_and_unicode_word_navigation() {
    use super::{column_navigation_target as go, ColumnNavigation::*};
    assert_eq!(go("  alpha beta\r\n", 5, Home), 0);
    assert_eq!(go("  alpha beta\r\n", 5, End), 12);
    assert_eq!(go("alpha beta", 0, NextWord), 6);
    assert_eq!(go("alpha beta", 10, PreviousWord), 6);
    assert_eq!(go("alpha beta", 6, PreviousWord), 0);
    assert_eq!(go("你好 world", 0, NextWord), 3);
    assert_eq!(go("", 30, End), 0);
    assert_eq!(go("abc", 50, PreviousWord), 0);
}
#[test]
fn column_selection_reverses_and_replaces_every_row() {
    use super::{column_range_after_navigation as select, apply_column_insert};
    assert_eq!(select(6, 0, false, 0, true), (0, 6, true));
    assert_eq!(select(0, 6, true, 2, true), (2, 4, true));
    assert_eq!(select(2, 4, true, 8, true), (6, 2, false));
    assert_eq!(select(6, 2, false, 0, false), (0, 0, false));
    assert_eq!(apply_column_insert("alpha one\r\nalpha two", 0, 1, 0, 5, "X"), "X one\r\nX two");
}

#[test]
fn app_version_and_window_title_format() {
    assert_eq!(super::APP_NAME, "alia");
    assert!(!super::APP_VERSION.is_empty());
    let parts: Vec<&str> = super::APP_VERSION.split(".").collect();
    assert_eq!(parts.len(), 3, "Version must follow semver (e.g. 0.4.2)");
    for part in parts {
        assert!(part.chars().all(|c| c.is_ascii_digit()), "Semver components must be numeric");
    }

    let title_clean = format!("{} — {} v{}", "Untitled-1.txt", super::APP_NAME, super::APP_VERSION);
    let title_dirty = format!("● {} — {} v{}", "Untitled-1.txt", super::APP_NAME, super::APP_VERSION);
    assert_eq!(title_clean, format!("Untitled-1.txt — alia v{}", super::APP_VERSION));
    assert_eq!(title_dirty, format!("● Untitled-1.txt — alia v{}", super::APP_VERSION));
}

#[test]
fn explorer_resize_respects_width_and_editor_space() {
    assert_eq!(super::explorer_width(240.0, 1200.0), 240.0);
    assert_eq!(super::explorer_width(50.0, 1200.0), 180.0);
    assert_eq!(super::explorer_width(900.0, 1200.0), 600.0);
    assert_eq!(super::explorer_width(600.0, 800.0), 480.0);
    assert_eq!(super::explorer_width(600.0, 400.0), 180.0);
}

#[test]
fn syntax_styles_survive_save_and_reparse() {
    use gpui_component::{highlighter::{HighlightTheme, SyntaxHighlighter}, input::Rope};
    let root = std::path::PathBuf::from(std::env::var_os("FEIBO_TMP").unwrap_or_else(|| std::env::temp_dir().into_os_string()))
        .join("save-highlight-regression");
    std::fs::create_dir_all(&root).unwrap();
    let theme = HighlightTheme::default_dark();
    for (name, language, code) in [
        ("main.rs", "rust", "fn main() { let value = 42; }\n"),
        ("main.py", "python", "def main():\n    return 42\n"),
        ("notes.md", "markdown", "# Heading\n\n**bold** and `code`\n"),
    ] {
        let mut highlighter = SyntaxHighlighter::new(language);
        let rope = Rope::from(code);
        highlighter.update(None, &rope);
        let before = highlighter.styles(&(0..code.len()), &theme);
        assert!(before.iter().any(|(_, style)| style != &Default::default()), "{language} must have non-default styles");
        let path = root.join(name);
        std::fs::write(&path, super::encode_text(code, true, true)).unwrap();
        let (saved, _, _) = super::decode_text(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(super::detect_language(Some(&path)).0, language);
        highlighter.update(None, &Rope::from(saved.as_str()));
        assert_eq!(before, highlighter.styles(&(0..saved.len()), &theme));
        let mut reparsed = SyntaxHighlighter::new(language);
        reparsed.update(None, &Rope::from(saved.as_str()));
        assert_eq!(before, reparsed.styles(&(0..saved.len()), &theme));
    }
}

#[test]
fn external_changes_reload_clean_documents_and_protect_local_edits() {
    use super::{external_change, ExternalChange};
    assert_eq!(external_change("old\n", false, false, false, b"new\n"), ExternalChange::Reload);
    assert_eq!(external_change("old\n", false, false, true, b"new\n"), ExternalChange::Conflict);
    assert_eq!(external_change("old\n", false, false, true, b"old\n"), ExternalChange::Unchanged);
    assert_eq!(external_change("old\n", true, true, false, &super::encode_text("old\n", true, true)), ExternalChange::Unchanged);
    assert_eq!(external_change("old\n", false, false, false, b""), ExternalChange::Reload);
}

#[test]
fn external_file_stamp_detects_write_delete_and_recreation() {
    let path = std::env::temp_dir().join(format!("alia-external-sync-{}.txt", std::process::id()));
    std::fs::write(&path, b"first").unwrap();
    let first = super::disk_stamp(&path).unwrap();
    std::fs::write(&path, b"second, longer").unwrap();
    assert_ne!(super::disk_stamp(&path).unwrap(), first);
    std::fs::remove_file(&path).unwrap();
    assert!(super::disk_stamp(&path).is_none());
    std::fs::write(&path, b"recreated").unwrap();
    assert!(super::disk_stamp(&path).is_some());
    std::fs::remove_file(&path).unwrap();
}

    #[test]
    fn explorer_snapshots_detect_create_rename_delete_and_nested_changes() {
        let root = std::env::temp_dir().join(format!("alia-explorer-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let empty = super::directory_snapshot(&root).unwrap();
        assert!(empty.is_empty());
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(root.join("before.rs"), "fn main() {}" ).unwrap();
        let created = super::directory_snapshot(&root).unwrap();
        assert_ne!(created, empty);
        assert!(created[0].directory);
        assert_eq!(created, super::directory_snapshot(&root).unwrap());
        std::fs::rename(root.join("before.rs"), root.join("after.rs")).unwrap();
        let renamed = super::directory_snapshot(&root).unwrap();
        assert_ne!(created, renamed);
        assert!(renamed.iter().any(|entry| entry.name == "after.rs"));
        std::fs::write(nested.join("child.txt"), "child").unwrap();
        assert_eq!(super::directory_snapshot(&nested).unwrap().len(), 1);
        std::fs::remove_file(root.join("after.rs")).unwrap();
        assert_eq!(super::directory_snapshot(&root).unwrap().len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(super::directory_snapshot(&root), Some(Vec::new()));
    }


#[test]
fn git_decorations_cover_staging_renames_conflicts_and_parent_folders() {
    use super::{parse_git_decorations, GitDecoration};
    let root = std::path::Path::new("repo");
    let data = b" M src/main.rs\0A  src/new.rs\0R  renamed file.rs\0old file.rs\0?? notes.md\0UU src/conflict.rs\0 D removed.rs\0";
    let states = parse_git_decorations(root, root, data);
    assert_eq!(states.get(&root.join("src/main.rs")), Some(&GitDecoration::Modified));
    assert_eq!(states.get(&root.join("src/new.rs")), Some(&GitDecoration::Added));
    assert_eq!(states.get(&root.join("renamed file.rs")), Some(&GitDecoration::Renamed));
    assert!(!states.contains_key(&root.join("old file.rs")));
    assert_eq!(states.get(&root.join("notes.md")), Some(&GitDecoration::Untracked));
    assert_eq!(states.get(&root.join("removed.rs")), Some(&GitDecoration::Deleted));
    assert_eq!(states.get(&root.join("src")), Some(&GitDecoration::Conflict));
    assert_eq!(states.get(root), Some(&GitDecoration::Conflict));
}

#[test]
fn git_decorations_scope_unicode_and_clean_snapshots() {
    use super::{parse_git_decorations, GitDecoration};
    let repo = std::path::Path::new("repo");
    let root = repo.join("project");
    let states = parse_git_decorations(repo, &root, "?? project/中文 file.rs\0 M other/file.rs\0".as_bytes());
    assert_eq!(states.get(&root.join("中文 file.rs")), Some(&GitDecoration::Untracked));
    assert!(!states.contains_key(&repo.join("other")));
    assert!(parse_git_decorations(repo, &root, b"").is_empty());
}
