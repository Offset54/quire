use proptest::prelude::*;

use super::*;

fn doc(text: &str) -> TextDocument {
    TextDocument::from_bytes(text.as_bytes())
}

fn all(d: &TextDocument) -> String {
    d.text(0, d.len()).unwrap()
}

fn act(d: &mut TextDocument, a: Action) {
    d.apply(&a).unwrap();
}

fn mv(to: Move) -> Action {
    Action::Move { to, extend: false }
}

fn typ(s: &str) -> Action {
    Action::Type { text: s.into() }
}

fn heads(d: &TextDocument) -> Vec<u64> {
    d.selections().iter().map(|s| s.head).collect()
}

#[test]
fn multi_cursor_typing_and_backspace() {
    let mut d = doc("one\ntwo\nthree");
    act(
        &mut d,
        Action::SetSelections {
            ranges: vec![(3, 3), (7, 7), (13, 13)],
        },
    );
    act(&mut d, typ("!"));
    assert_eq!(all(&d), "one!\ntwo!\nthree!");
    assert_eq!(heads(&d), vec![4, 9, 16]);
    act(&mut d, Action::Backspace);
    act(&mut d, Action::Backspace);
    assert_eq!(all(&d), "on\ntw\nthre");
    // Typing and deleting coalesce into one step each.
    act(&mut d, Action::Undo);
    assert_eq!(all(&d), "one!\ntwo!\nthree!");
    act(&mut d, Action::Undo);
    assert_eq!(all(&d), "one\ntwo\nthree");
    assert!(!d.is_dirty());
    act(&mut d, Action::Redo);
    act(&mut d, Action::Redo);
    assert_eq!(all(&d), "on\ntw\nthre");
    assert!(d.is_dirty());
}

#[test]
fn crlf_and_combining_marks_are_single_steps() {
    let mut d = doc("a\r\nb\u{0301}c");
    act(
        &mut d,
        Action::SetSelections {
            ranges: vec![(3, 3)],
        },
    );
    act(&mut d, Action::Backspace);
    assert_eq!(all(&d), "ab\u{0301}c");
    act(&mut d, mv(Move::Right));
    assert_eq!(heads(&d), vec![4]);
    act(&mut d, Action::Backspace);
    assert_eq!(all(&d), "ac");
}

#[test]
fn word_moves_and_deletes() {
    let mut d = doc("foo_bar  (baz) 한국어\nnext");
    act(&mut d, mv(Move::WordRight));
    assert_eq!(heads(&d), vec![7]);
    act(&mut d, mv(Move::WordRight));
    assert_eq!(heads(&d), vec![10]);
    act(&mut d, mv(Move::WordRight));
    assert_eq!(heads(&d), vec![13]);
    act(&mut d, mv(Move::WordRight));
    assert_eq!(heads(&d), vec![14]);
    act(&mut d, mv(Move::WordRight));
    // Wide characters are one word each.
    assert_eq!(heads(&d), vec![18]);
    act(&mut d, mv(Move::WordLeft));
    assert_eq!(heads(&d), vec![15]);
    act(&mut d, Action::DeleteWordLeft);
    assert_eq!(all(&d), "foo_bar  (baz한국어\nnext");
    act(&mut d, mv(Move::End));
    act(&mut d, mv(Move::WordRight));
    assert_eq!(heads(&d), vec![23]);
}

#[test]
fn home_is_smart_and_up_down_keep_the_column() {
    let mut d = doc("    indented line\nx\nanother long line");
    act(&mut d, mv(Move::End));
    act(&mut d, mv(Move::Home));
    assert_eq!(heads(&d), vec![4]);
    act(&mut d, mv(Move::Home));
    assert_eq!(heads(&d), vec![0]);
    act(
        &mut d,
        Action::SetSelections {
            ranges: vec![(10, 10)],
        },
    );
    act(&mut d, mv(Move::Down));
    assert_eq!(heads(&d), vec![19]); // end of "x"
    act(&mut d, mv(Move::Down));
    assert_eq!(heads(&d), vec![30]); // column 10 again
    act(&mut d, mv(Move::Down));
    assert_eq!(heads(&d), vec![d.len()]);
    act(
        &mut d,
        Action::Move {
            to: Move::Up,
            extend: true,
        },
    );
    assert_eq!(d.primary().anchor, d.len());
}

#[test]
fn paste_distributes_lines_over_cursors() {
    let mut d = doc("a\nb\nc");
    act(
        &mut d,
        Action::SetSelections {
            ranges: vec![(1, 1), (3, 3), (5, 5)],
        },
    );
    act(
        &mut d,
        Action::Paste {
            text: "1\n2\n3".into(),
        },
    );
    assert_eq!(all(&d), "a1\nb2\nc3");
    act(&mut d, Action::Paste { text: "xy".into() });
    assert_eq!(all(&d), "a1xy\nb2xy\nc3xy");
}

#[test]
fn next_occurrence_column_select_and_cut() {
    let mut d = doc("cat dog cat\ncat bird");
    act(&mut d, Action::AddNextOccurrence);
    assert_eq!(d.selections().len(), 1);
    assert_eq!((d.primary().start(), d.primary().end()), (0, 3));
    act(&mut d, Action::AddNextOccurrence);
    act(&mut d, Action::AddNextOccurrence);
    assert_eq!(d.selections().len(), 3);
    act(&mut d, typ("cow"));
    assert_eq!(all(&d), "cow dog cow\ncow bird");

    let mut d = doc("abcdef\nab\nabcdefgh");
    act(
        &mut d,
        Action::ColumnSelect {
            from_line: 0,
            from_col: 1,
            to_line: 2,
            to_col: 4,
        },
    );
    assert_eq!(d.selections().len(), 3);
    let out = d.apply(&Action::Cut).unwrap();
    assert_eq!(out.clipboard.as_deref(), Some("bcd\nb\nbcd"));
    assert_eq!(all(&d), "aef\na\naefgh");
}

#[test]
fn click_word_and_line() {
    let mut d = doc("hello world\nsecond");
    act(
        &mut d,
        Action::Click {
            offset: 8,
            mode: ClickMode::Set,
            unit: ClickUnit::Word,
        },
    );
    assert_eq!((d.primary().start(), d.primary().end()), (6, 11));
    act(
        &mut d,
        Action::Click {
            offset: 2,
            mode: ClickMode::Set,
            unit: ClickUnit::Line,
        },
    );
    assert_eq!((d.primary().start(), d.primary().end()), (0, 12));
    act(
        &mut d,
        Action::Click {
            offset: 15,
            mode: ClickMode::Add,
            unit: ClickUnit::Char,
        },
    );
    assert_eq!(d.selections().len(), 2);
}

fn action() -> impl Strategy<Value = Action> {
    let text = prop::collection::vec(
        prop_oneof![Just('a'), Just(' '), Just('\n'), Just('\r'), Just('한')],
        1..4,
    )
    .prop_map(|v| v.into_iter().collect::<String>());
    prop_oneof![
        text.clone().prop_map(|text| Action::Type { text }),
        text.prop_map(|text| Action::Paste { text }),
        Just(Action::Backspace),
        Just(Action::Delete),
        Just(Action::DeleteWordLeft),
        Just(Action::Newline),
        Just(Action::Undo),
        Just(Action::Redo),
        Just(Action::AddCursorBelow),
        Just(Action::SelectLine),
        (0..6usize).prop_map(|i| Action::Move {
            to: [
                Move::Left,
                Move::Right,
                Move::WordRight,
                Move::Up,
                Move::Down,
                Move::End
            ][i],
            extend: i % 2 == 0,
        }),
        (0..1000u64, 0..1000u64).prop_map(|(a, b)| Action::SetSelections {
            ranges: vec![(a % 40, b % 40)]
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// Undo is an exact inverse: undoing everything restores the original
    /// bytes; redoing everything restores the final ones. Replaying the
    /// journal records on a fresh document reproduces the content and the
    /// undo history.
    #[test]
    fn undo_is_exact_and_journal_replays(
        original in "[a \\n\\r한]{0,40}",
        actions in prop::collection::vec(action(), 1..40),
    ) {
        let mut d = TextDocument::from_bytes(original.as_bytes());
        let mut records = Vec::new();
        for a in &actions {
            d.apply(a).unwrap();
            records.extend(d.take_records());
            // Carets stay on character boundaries.
            let text = d.snapshot().read(0..d.len()).unwrap();
            for s in d.selections() {
                prop_assert!(s.head <= d.len());
                let h = s.head as usize;
                prop_assert!(h == text.len() || (text[h] & 0xC0) != 0x80);
            }
        }
        let final_bytes = d.snapshot().read(0..d.len()).unwrap();

        let mut replay = TextDocument::from_bytes(original.as_bytes());
        for r in &records {
            replay.apply_record(r).unwrap();
        }
        prop_assert_eq!(replay.snapshot().read(0..replay.len()).unwrap(), final_bytes.clone());

        for doc in [&mut d, &mut replay] {
            let mut undone = 0;
            while doc.apply(&Action::Undo).unwrap().changed {
                undone += 1;
            }
            prop_assert_eq!(doc.snapshot().read(0..doc.len()).unwrap(), original.as_bytes().to_vec());
            for _ in 0..undone {
                doc.apply(&Action::Redo).unwrap();
            }
            prop_assert_eq!(doc.snapshot().read(0..doc.len()).unwrap(), final_bytes.clone());
        }
    }
}
