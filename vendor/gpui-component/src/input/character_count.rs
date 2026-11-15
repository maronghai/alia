use std::ops::Range;
use super::Rope;

// Only the edited byte range is visited; unchanged document chunks are not scanned.
pub(super) fn character_count_after_edit(text: &Rope, total: usize, range: Range<usize>, inserted: &str) -> usize {
    total - text.slice(range).chars().count() + inserted.chars().count()
}

#[cfg(test)]
mod character_count_tests {
    use super::*;

    #[test]
    fn incremental_character_count_tracks_unicode_and_replacements() {
        let mut rope = Rope::from("a中😀\r\nlast");
        let mut count = rope.chars().count();
        for (range, inserted) in [(1..8, "你好"), (1..7, ""), (1..1, "é😀"), (0..1, "粘贴\n多行")] {
            count = character_count_after_edit(&rope, count, range.clone(), inserted);
            rope = Rope::from(format!("{}{}{}", rope.slice(..range.start), inserted, rope.slice(range.end..)).as_str());
            assert_eq!(count, rope.chars().count());
        }
    }

    #[test]
    fn incremental_character_count_tracks_ime_and_large_deletions() {
        let initial = "中😀abc\n".repeat(20000);
        let mut rope = Rope::from(initial.as_str());
        let mut count = initial.chars().count();
        for (range, inserted) in [(0..0, "n"), (0..1, "ni"), (0..2, "你"), (0..3, "你好"), (6..10006, "x")] {
            // Derive a valid Unicode boundary for the large deletion.
            let end = if range.end == 10006 { rope.to_string().char_indices().nth(10000).unwrap().0 } else { range.end };
            let range = range.start..end;
            count = character_count_after_edit(&rope, count, range.clone(), inserted);
            rope = Rope::from(format!("{}{}{}", rope.slice(..range.start), inserted, rope.slice(range.end..)).as_str());
            assert_eq!(count, rope.chars().count());
        }
    }
}
