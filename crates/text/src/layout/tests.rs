use super::*;
use crate::buffer::TextBuffer;

fn snap(text: &[u8]) -> Snapshot {
    TextBuffer::from_bytes(text).snapshot()
}

fn all_rows(s: &Snapshot, layout: &Layout) -> Vec<VisualRow> {
    let mut rows = vec![row_containing(s, 0, layout).unwrap()];
    while let Some(n) = row_after(s, rows.last().unwrap(), layout).unwrap() {
        rows.push(n);
        assert!(rows.len() < 100_000);
    }
    rows
}

#[test]
fn wraps_at_spaces_and_wide_characters() {
    assert_eq!(wrap_rows(b"aaaa bbbb cccc", 10, 4), vec![0, 10]);
    assert_eq!(wrap_rows(b"aaaaaaaaaaaaaaa", 10, 4), vec![0, 10]);
    // Wide characters: 2 columns each, breakable anywhere.
    let s = "가나다라마바사".as_bytes();
    assert_eq!(wrap_rows(s, 5, 4), vec![0, 6, 12, 18]);
    assert_eq!(wrap_rows(b"", 10, 4), vec![0]);
    // A tab fills to the next stop.
    assert_eq!(wrap_rows(b"ab\tcd", 5, 4), vec![0, 3]);
}

#[test]
fn rows_tile_the_document_and_walk_both_ways() {
    let text = "short\nthis line is long enough to wrap a few times over\n\n한국어 줄도 넘치면 접힌다 그렇다\r\nlast";
    let s = snap(text.as_bytes());
    for layout in [
        Layout { wrap: None, tab: 4 },
        Layout {
            wrap: Some(12),
            tab: 4,
        },
    ] {
        let rows = all_rows(&s, &layout);
        // Contiguous within lines; lines follow each other.
        for w in rows.windows(2) {
            if w[0].end < w[0].line_end {
                assert_eq!(w[1].start, w[0].end);
            } else {
                assert_eq!(w[1].start, w[0].next_line);
            }
            assert_eq!(row_before(&s, &w[1], &layout).unwrap(), Some(w[0]));
        }
        assert_eq!(rows.last().unwrap().end, s.len());
        for r in &rows {
            assert_eq!(row_containing(&s, r.start, &layout).unwrap(), *r);
        }
        if layout.wrap.is_none() {
            assert_eq!(rows.len(), 5);
        } else {
            assert!(rows.len() > 8);
            for r in &rows {
                let end_col = column_in_row(&s, r, r.end, 4).unwrap();
                assert!(end_col <= 12, "{r:?} has {end_col} columns");
            }
        }
    }
}

#[test]
fn rows_around_an_anchor() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let s = snap(text.as_bytes());
    let layout = Layout::default();
    let anchor = text.find("line 50").unwrap() as u64 + 3;
    let (rows, i) = rows_around(&s, anchor, 5, 10, &layout).unwrap();
    assert_eq!(i, 5);
    assert_eq!(rows.len(), 16);
    assert_eq!(rows[i].start, text.find("line 50").unwrap() as u64);
    let (rows, i) = rows_around(&s, 0, 5, 3, &layout).unwrap();
    assert_eq!((i, rows.len()), (0, 4));
}

#[test]
fn long_lines_wrap_per_segment() {
    let line = "abcdefghij".repeat(30_000); // 300 KB, one line
    let s = snap(line.as_bytes());
    let layout = Layout {
        wrap: Some(100),
        tab: 4,
    };
    let mid = row_containing(&s, 150_005, &layout).unwrap();
    assert!(mid.start <= 150_005 && 150_005 < mid.end);
    assert!(mid.end - mid.start <= 100);
    // A segment boundary starts a new row.
    let at = row_containing(&s, SEG, &layout).unwrap();
    assert_eq!(at.start, SEG);
    let before = row_before(&s, &at, &layout).unwrap().unwrap();
    assert_eq!(before.end, SEG);
    // Columns past the first segment are byte distances for ASCII.
    let whole = row_containing(&s, 200_000, &Layout::default()).unwrap();
    assert_eq!(column_in_row(&s, &whole, 200_000, 4).unwrap(), 200_000);
    assert_eq!(offset_at_column(&s, &whole, 200_000, 4).unwrap(), 200_000);
}

#[test]
fn columns_and_offsets_round_trip() {
    let s = snap("a\tb한c\u{0301}d".as_bytes());
    let row = row_containing(&s, 0, &Layout::default()).unwrap();
    let mut cols = vec![];
    for off in [0u64, 1, 2, 3, 6, 9, 10] {
        let c = column_in_row(&s, &row, off, 4).unwrap();
        cols.push(c);
        assert_eq!(offset_at_column(&s, &row, c, 4).unwrap(), off, "col {c}");
    }
    assert_eq!(cols, vec![0, 1, 4, 5, 7, 8, 9]);
}
