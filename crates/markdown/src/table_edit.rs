//! Table helpers (M2 task 10): format and align a GFM table, insert or
//! delete a row or a column, and move between cells (Tab, Shift+Tab).
//!
//! Each helper takes the table's source (its block) and the caret, and
//! returns one replacement within it plus where the caret goes: the caller
//! applies it as one undoable edit. Only the table's own lines change; the
//! replacement is trimmed to the bytes that differ, so everything else
//! (blank lines after it, the rest of the document) stays byte-exact.
//! Cell contents are kept as written (escaped pipes included), trimmed of
//! surrounding spaces; cells a row has beyond the header's count are kept.

use quire_text::chars::width;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAction {
    Format,
    InsertRowBelow,
    DeleteRow,
    InsertColumnRight,
    DeleteColumn,
    /// Tab: the next cell (a new row after the last cell).
    NextCell,
    /// Shift+Tab: the previous cell.
    PrevCell,
}

impl TableAction {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "format" => Self::Format,
            "insertRowBelow" => Self::InsertRowBelow,
            "deleteRow" => Self::DeleteRow,
            "insertColumnRight" => Self::InsertColumnRight,
            "deleteColumn" => Self::DeleteColumn,
            "nextCell" => Self::NextCell,
            "prevCell" => Self::PrevCell,
            _ => return None,
        })
    }
}

/// Replace `from..to` of the table's source with `text`; then the caret
/// goes to `caret` (offsets into the source as it is after the edit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableEdit {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub caret: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

struct Table {
    indent: String,
    eol: String,
    /// The last line had no line ending.
    open_end: bool,
    /// Bytes of the table's lines in the source.
    len: usize,
    header: Vec<String>,
    aligns: Vec<Align>,
    body: Vec<Vec<String>>,
}

/// Splits a row into trimmed cells at unescaped pipes; one leading and one
/// trailing pipe are optional, as in GFM. Also returns each cell's content
/// span in `line` (for the caret).
fn cells(line: &str) -> Vec<(String, usize, usize)> {
    let b = line.as_bytes();
    let mut bounds = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'|' => {
                bounds.push(i);
                i += 1;
            }
            _ => i += 1,
        }
    }
    let start = line.len() - line.trim_start().len();
    let end = line.trim_end().len();
    let mut edges = vec![start];
    for &p in &bounds {
        if p == start {
            edges[0] = p + 1;
        } else if p + 1 == end {
            edges.push(p);
            edges.push(usize::MAX);
        } else {
            edges.push(p);
            edges.push(p + 1);
        }
    }
    if edges.last() != Some(&usize::MAX) {
        edges.push(end);
    } else {
        edges.pop();
    }
    edges
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| {
            let (a, z) = (c[0].min(end), c[1].min(end).max(c[0].min(end)));
            let raw = &line[a..z];
            let lead = raw.len() - raw.trim_start().len();
            let t = raw.trim();
            (t.to_owned(), a + lead, a + lead + t.len())
        })
        .collect()
}

fn delimiter_align(cell: &str) -> Option<Align> {
    let t = cell.trim();
    let l = t.starts_with(':');
    let r = t.ends_with(':') && t.len() > 1;
    let dashes = t.trim_matches(':');
    if dashes.is_empty() || !dashes.bytes().all(|c| c == b'-') {
        return None;
    }
    Some(match (l, r) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    })
}

/// The table's lines: up to the first blank line.
fn lines(src: &str) -> Vec<(usize, &str, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for raw in src.split_inclusive('\n') {
        let body = raw.trim_end_matches(['\n', '\r']);
        if body.trim().is_empty() {
            break;
        }
        out.push((at, body, &raw[body.len()..]));
        at += raw.len();
    }
    out
}

fn parse(src: &str) -> Option<Table> {
    let ls = lines(src);
    if ls.len() < 2 {
        return None;
    }
    let header: Vec<String> = cells(ls[0].1).into_iter().map(|c| c.0).collect();
    let aligns: Vec<Align> = cells(ls[1].1)
        .iter()
        .map(|c| delimiter_align(&c.0))
        .collect::<Option<_>>()?;
    if aligns.len() != header.len() {
        return None;
    }
    let indent: String = ls[0].1.chars().take_while(|c| *c == ' ').collect();
    let eol = if ls[0].2.is_empty() { "\n" } else { ls[0].2 }.to_owned();
    let (last_at, last_body, last_end) = ls[ls.len() - 1];
    Some(Table {
        indent,
        eol,
        open_end: last_end.is_empty(),
        len: last_at + last_body.len() + last_end.len(),
        header,
        aligns,
        body: ls[2..]
            .iter()
            .map(|l| cells(l.1).into_iter().map(|c| c.0).collect())
            .collect(),
    })
}

fn text_width(s: &str) -> usize {
    s.chars().map(|c| width(Some(c), 0, 4) as usize).sum()
}

/// (row, column, byte offset within the cell's content) of `caret`; rows
/// count the header as 0 and the delimiter as 1.
fn locate(src: &str, caret: usize) -> (usize, usize, usize) {
    let ls = lines(src);
    let r = ls
        .iter()
        .rposition(|l| l.0 <= caret)
        .unwrap_or(0)
        .min(ls.len().saturating_sub(1));
    let (at, line, _) = ls[r];
    let k = caret.saturating_sub(at).min(line.len());
    let cs = cells(line);
    let mut c = 0;
    for (i, cell) in cs.iter().enumerate() {
        if k >= cell.1 || i == 0 {
            c = i;
        }
        if k <= cell.2 && k >= cell.1 {
            return (r, i, k - cell.1);
        }
    }
    let within = cs
        .get(c)
        .map_or(0, |cell| if k > cell.2 { cell.2 - cell.1 } else { 0 });
    (r, c, within)
}

/// The table written out aligned; returns the text and the content span of
/// every cell, by (row, column).
fn render(t: &Table) -> (String, Vec<Vec<(usize, usize)>>) {
    let n = t.header.len();
    let mut widths: Vec<usize> = t.header.iter().map(|h| text_width(h).max(3)).collect();
    for row in &t.body {
        for (i, c) in row.iter().enumerate().take(n) {
            widths[i] = widths[i].max(text_width(c));
        }
    }
    let mut out = String::new();
    let mut spans = Vec::new();
    let row_out = |out: &mut String, row: &[String], aligns: &[Align]| {
        let mut sp = Vec::new();
        out.push_str(&t.indent);
        out.push('|');
        for (i, c) in row.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(0);
            let pad = w.saturating_sub(text_width(c));
            let (l, r) = match aligns.get(i).copied().unwrap_or(Align::None) {
                Align::Right => (pad, 0),
                Align::Center => (pad / 2, pad - pad / 2),
                _ => (0, pad),
            };
            out.push(' ');
            out.push_str(&" ".repeat(l));
            sp.push((out.len(), out.len() + c.len()));
            out.push_str(c);
            out.push_str(&" ".repeat(r));
            out.push_str(" |");
        }
        out.push_str(&t.eol);
        sp
    };
    spans.push(row_out(&mut out, &t.header, &t.aligns));
    let mut delim = Vec::new();
    for (i, a) in t.aligns.iter().enumerate() {
        let w = widths[i];
        let (l, r) = match a {
            Align::None => (false, false),
            Align::Left => (true, false),
            Align::Right => (false, true),
            Align::Center => (true, true),
        };
        let dashes = w - usize::from(l) - usize::from(r);
        delim.push(format!(
            "{}{}{}",
            if l { ":" } else { "" },
            "-".repeat(dashes),
            if r { ":" } else { "" }
        ));
    }
    spans.push(row_out(&mut out, &delim, &[]));
    for row in &t.body {
        let mut full = row.clone();
        while full.len() < n {
            full.push(String::new());
        }
        spans.push(row_out(&mut out, &full, &t.aligns));
    }
    if t.open_end {
        out.truncate(out.len() - t.eol.len());
    }
    (out, spans)
}

/// Applies `action` to the table in `src` with the caret at `caret`.
/// `None` when `src` doesn't start with a table, or the action doesn't
/// apply (deleting the header row, the last column).
pub fn edit_table(src: &str, caret: usize, action: TableAction) -> Option<TableEdit> {
    let mut t = parse(src)?;
    let (mut r, mut c, mut k) = locate(src, caret);
    let n = t.header.len();
    match action {
        TableAction::Format => {}
        TableAction::InsertRowBelow => {
            let at = r.saturating_sub(1).min(t.body.len());
            t.body.insert(at, vec![String::new(); n]);
            (r, c, k) = (at + 2, c.min(n - 1), 0);
        }
        TableAction::DeleteRow => {
            if r < 2 {
                return None;
            }
            t.body.remove(r - 2);
            r = if t.body.is_empty() {
                0
            } else {
                r.min(t.body.len() + 1)
            };
            k = 0;
        }
        TableAction::InsertColumnRight => {
            let at = (c + 1).min(n);
            t.header.insert(at, String::new());
            t.aligns.insert(at, Align::None);
            for row in &mut t.body {
                if row.len() >= at {
                    row.insert(at, String::new());
                }
            }
            (c, k) = (at, 0);
        }
        TableAction::DeleteColumn => {
            if n < 2 || c >= n {
                return None;
            }
            t.header.remove(c);
            t.aligns.remove(c);
            for row in &mut t.body {
                if row.len() > c {
                    row.remove(c);
                }
            }
            (c, k) = (c.min(n - 2), 0);
        }
        TableAction::NextCell => {
            if r == 1 {
                r = 0;
            }
            if c + 1 < n {
                c += 1;
            } else {
                c = 0;
                r = if r == 0 { 2 } else { r + 1 };
                if r - 2 >= t.body.len() {
                    t.body.push(vec![String::new(); n]);
                }
            }
            k = usize::MAX;
        }
        TableAction::PrevCell => {
            if r == 1 {
                r = 0;
            }
            if c > 0 {
                c -= 1;
            } else if r > 0 {
                r = if r == 2 { 0 } else { r - 1 };
                c = n - 1;
            }
            k = usize::MAX;
        }
    }
    let (text, spans) = render(&t);
    let span = spans
        .get(r)
        .and_then(|row| row.get(c).or(row.last()))
        .copied()
        .unwrap_or((0, 0));
    let mut caret = span.0 + k.min(span.1 - span.0);
    while !text.is_char_boundary(caret) {
        caret -= 1;
    }
    // Only the bytes that differ.
    let old = &src[..t.len];
    let pre = old
        .bytes()
        .zip(text.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let suf = old[pre..]
        .bytes()
        .rev()
        .zip(text[pre..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (mut from, mut to_new) = (pre, text.len() - suf);
    while !text.is_char_boundary(from) || !old.is_char_boundary(from) {
        from -= 1;
    }
    while !text.is_char_boundary(to_new) || !old.is_char_boundary(to_new + old.len() - text.len()) {
        to_new += 1;
    }
    let to = to_new + old.len() - text.len();
    Some(TableEdit {
        from,
        to,
        text: text[from..to_new].to_owned(),
        caret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source after the edit.
    fn apply(src: &str, e: &TableEdit) -> String {
        format!("{}{}{}", &src[..e.from], e.text, &src[e.to..])
    }

    fn run(src: &str, caret: usize, a: TableAction) -> (String, usize) {
        let e = edit_table(src, caret, a).expect("applies");
        (apply(src, &e), e.caret)
    }

    #[test]
    fn format_aligns_columns_by_display_width() {
        let src = "| a | bb |\n|:-|-:|\n| 한국어 | 1 |\n| x |\n\nAfter.\n";
        let (out, _) = run(src, 0, TableAction::Format);
        assert_eq!(
            out,
            "| a      |  bb |\n| :----- | --: |\n| 한국어 |   1 |\n| x      |     |\n\nAfter.\n"
        );
        // Formatting a formatted table changes nothing.
        let e = edit_table(&out, 0, TableAction::Format).unwrap();
        assert_eq!(e.from, e.to);
        assert!(e.text.is_empty());
    }

    #[test]
    fn keeps_crlf_indent_escapes_and_the_text_after() {
        let src = "  a|b\\|c\r\n  ---|:-:\r\n  1|2\r\n\r\nAfter.\r\n";
        let (out, _) = run(src, 0, TableAction::Format);
        assert_eq!(
            out,
            "  | a   | b\\|c |\r\n  | --- | :--: |\r\n  | 1   |  2   |\r\n\r\nAfter.\r\n"
        );
        let open = "| a |\n|---|\n| 1 |";
        let (out, _) = run(open, 0, TableAction::Format);
        assert!(!out.ends_with('\n'), "no line ending added: {out:?}");
    }

    #[test]
    fn tab_moves_through_cells_and_adds_a_row_at_the_end() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let at = |s: &str, needle: &str| s.find(needle).unwrap();
        let (out, caret) = run(src, at(src, "a"), TableAction::NextCell);
        assert_eq!(&out[caret - 1..caret], "b");
        let (out2, caret2) = run(&out, caret, TableAction::NextCell);
        assert_eq!(
            &out2[caret2 - 1..caret2],
            "1",
            "header's last cell → first body cell"
        );
        let last = at(&out2, "2");
        let (out3, caret3) = run(&out2, last, TableAction::NextCell);
        assert_eq!(
            out3,
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n|     |     |\n"
        );
        assert_eq!(caret3, out3.rfind("|     |     |").unwrap() + 2);
        let (_, back) = run(&out3, caret3, TableAction::PrevCell);
        assert_eq!(&out3[back - 1..back], "2");
    }

    #[test]
    fn rows_and_columns() {
        let src = "| a | b |\n|---|--:|\n| 1 | 2 |\n";
        let one = src.find('1').unwrap();
        let (out, _) = run(src, one, TableAction::InsertRowBelow);
        assert_eq!(out.lines().count(), 4);
        let (out, _) = run(&out, out.find('1').unwrap(), TableAction::DeleteRow);
        assert_eq!(out.lines().count(), 3);
        assert!(
            edit_table(src, 0, TableAction::DeleteRow).is_none(),
            "the header stays"
        );
        let (out, caret) = run(src, src.find('a').unwrap(), TableAction::InsertColumnRight);
        assert_eq!(out.lines().next().unwrap(), "| a   |     |   b |");
        assert_eq!(out.lines().nth(1).unwrap(), "| --- | --- | --: |");
        assert_eq!(caret, 8);
        let (out, _) = run(&out, out.find('b').unwrap(), TableAction::DeleteColumn);
        assert_eq!(out.lines().next().unwrap(), "| a   |     |");
        assert!(edit_table("| a |\n|---|\n", 2, TableAction::DeleteColumn).is_none());
    }

    #[test]
    fn not_a_table() {
        assert!(edit_table("just text\n", 0, TableAction::Format).is_none());
        assert!(edit_table("| a |\n| b |\n", 0, TableAction::Format).is_none());
    }

    #[test]
    fn edits_are_valid_on_any_caret() {
        let src = "| α | b |\n|---|---|\n| 1 | 한 |\n\n";
        for caret in 0..=src.len() {
            if !src.is_char_boundary(caret) {
                continue;
            }
            for a in [
                TableAction::Format,
                TableAction::InsertRowBelow,
                TableAction::DeleteRow,
                TableAction::InsertColumnRight,
                TableAction::DeleteColumn,
                TableAction::NextCell,
                TableAction::PrevCell,
            ] {
                if let Some(e) = edit_table(src, caret, a) {
                    let out = apply(src, &e);
                    assert!(
                        out.is_char_boundary(e.caret) && e.caret <= out.len(),
                        "{a:?} {caret}"
                    );
                    assert!(out.ends_with("|\n\n"), "{a:?} {caret}: {out:?}");
                }
            }
        }
    }
}
