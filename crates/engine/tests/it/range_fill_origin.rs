//! `range.fill` with a formula. The formula is the one typed in the active cell (Ctrl+Enter), and
//! it used to be moved from the active cell even when the caller named a range the active cell is
//! not in: `=SUM(A1:J1)` filled down K1:K3 from an active cell in A1 became `=SUM(K1:T1)`, a
//! formula that reads itself.

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn workbook() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4], [5, 6], [7, 8]]})).unwrap();
    s
}

fn run(s: &mut Session, id: &str, params: Json) {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn formula(s: &Session, sheet: usize, cell: &str) -> String {
    let cell = s.doc().unwrap().wb.sheets[sheet].cell(CellRef::parse(cell).unwrap()).cloned().unwrap_or_default();
    cell.formula.map(|f| f.text.to_string()).unwrap_or_default()
}

fn number(s: &Session, cell: &str) -> f64 {
    s.doc().unwrap().wb.sheets[0].value(CellRef::parse(cell).unwrap()).as_f64().unwrap_or(f64::NAN)
}

#[test]
fn a_named_range_gets_the_formula_written_for_its_first_cell() {
    let mut s = workbook();
    // The active cell is A1, outside the range.
    run(&mut s, "range.fill", json!({"range": "D1:D3", "input": "=SUM(A1:B1)+$A$1"}));
    assert_eq!(formula(&s, 0, "D1"), "SUM(A1:B1)+$A$1");
    assert_eq!(formula(&s, 0, "D3"), "SUM(A3:B3)+$A$1");
    assert_eq!((number(&s, "D1"), number(&s, "D2"), number(&s, "D3")), (4.0, 8.0, 12.0));
    // Wherever the active cell is.
    run(&mut s, "selection.set", json!({"range": "B4"}));
    run(&mut s, "range.fill", json!({"range": "F2:G2", "input": "=A2*10"}));
    assert_eq!((formula(&s, 0, "F2"), formula(&s, 0, "G2")), ("A2*10".to_string(), "B2*10".to_string()));
}

#[test]
fn the_active_cell_in_the_range_is_where_the_formula_was_typed() {
    // Ctrl+Enter: the selection is filled, and the active cell need not be its first cell.
    let mut s = workbook();
    run(&mut s, "selection.set", json!({"range": "D1:D4", "active": "D3"}));
    assert_eq!(s.doc().unwrap().selection.active.a1(), "D3");
    run(&mut s, "range.fill", json!({"input": "=A3*2"}));
    for (cell, want) in [("D1", "A1*2"), ("D2", "A2*2"), ("D3", "A3*2"), ("D4", "A4*2")] {
        assert_eq!(formula(&s, 0, cell), want, "{cell}");
    }
    // The same range named outright, with the active cell in it.
    run(&mut s, "range.fill", json!({"range": "D1:D4", "input": "=B3"}));
    assert_eq!((formula(&s, 0, "D3"), formula(&s, 0, "D1")), ("B3".to_string(), "B1".to_string()));
}

#[test]
fn a_range_on_another_sheet_is_not_measured_from_this_sheets_active_cell() {
    let mut s = workbook();
    run(&mut s, "home.insertSheet", json!({}));
    let other = s.doc().unwrap().wb.active_sheet();
    let first = usize::from(other == 0);
    run(&mut s, "selection.set", json!({"range": "C3"}));
    // C3 is active on the new sheet; the range is on the first one and happens to hold C3 too.
    run(&mut s, "range.fill", json!({"sheet": first, "range": "C2:C4", "input": "=A2+B2"}));
    assert_eq!(formula(&s, first, "C2"), "A2+B2");
    assert_eq!(formula(&s, first, "C4"), "A4+B4");
}
