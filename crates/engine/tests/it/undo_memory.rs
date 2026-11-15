//! What an undo step keeps alive. The history is bounded by the bytes its snapshots retain, so a
//! step that retains megabytes for a one-cell edit, or the whole sheet for one inserted row, uses
//! the budget up in a few dozen steps and takes the older steps with it.

use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

/// A new workbook with a 2,000 x 10 block of numbers, with an empty history.
fn grid() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    s.execute("range.fill", json!({"input": "1", "range": "A1:J2000"})).unwrap();
    let doc = s.active_mut().unwrap();
    doc.undo.clear();
    s
}

/// The bytes the step `id` retains, in a history that holds only that step.
fn retained_by(id: &str, params: Json) -> usize {
    let mut s = grid();
    s.execute(id, params).unwrap();
    let undo = &s.doc().unwrap().undo;
    assert_eq!(undo.len(), 1);
    undo[0].retained_bytes
}

#[test]
fn typing_in_one_cell_retains_its_row_not_its_block_of_rows() {
    // The 64 rows around the cell hold 640 numbers, about 50 KB.
    let retained = retained_by("cell.set", json!({"cell": "C1000", "input": "42"}));
    assert!(retained < 16 * 1024, "{retained} bytes");
}

#[test]
fn inserting_or_deleting_rows_retains_no_copy_of_the_rows_that_moved() {
    // The 20,000 numbers are about 1.5 MB. The rows below the edit move whole and stay shared.
    for (id, params) in [("home.insertRows", json!({"rows": "5:5"})), ("home.deleteRows", json!({"rows": "5:6"}))] {
        let retained = retained_by(id, params);
        assert!(retained < 256 * 1024, "{id}: {retained} bytes");
    }
}

#[test]
fn undo_and_redo_still_give_the_cells_back() {
    let mut s = grid();
    let value = |s: &Session, cell: &str| s.doc().unwrap().wb.sheets[0].value(gridcraft_core::CellRef::parse(cell).unwrap()).display();
    s.execute("cell.set", json!({"cell": "C1000", "input": "42"})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "5:6"})).unwrap();
    s.execute("cell.set", json!({"cell": "C998", "input": "43"})).unwrap();
    assert_eq!((value(&s, "C998"), s.doc().unwrap().wb.sheets[0].cells.len()), ("43".to_string(), 19_980));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&s, "C998"), "42", "the typed 42 moved up two rows with its row");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((value(&s, "C1000"), value(&s, "C998"), s.doc().unwrap().wb.sheets[0].cells.len()), ("42".to_string(), "1".to_string(), 20_000));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&s, "C1000"), "1");
    for _ in 0..3 {
        s.execute("edit.redo", json!({})).unwrap();
    }
    assert_eq!((value(&s, "C998"), s.doc().unwrap().wb.sheets[0].cells.len()), ("43".to_string(), 19_980));
}
