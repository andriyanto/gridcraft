//! Turning a table's total row on when the row below the table is in use. The total row used to
//! be written over whatever was there: data was lost, and a table below ended up sharing cells
//! with the first (found by soaking `command_sequence_fuzz`, seeds 191, 245 and 398).

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_engine::Session;
use gridcraft_model::{Sheet, Workbook};
use serde_json::{Value as Json, json};

/// The `grades` sample (its table `Grades` is A1:G9) after `steps`, each of which must work.
fn grades(steps: &[(&str, Json)]) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "grades"})).unwrap();
    for (id, params) in steps {
        s.execute(id, params.clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    s
}

fn sheet(s: &Session) -> &Sheet {
    &s.doc().unwrap().wb.sheets[0]
}

fn text(s: &Session, cell: &str) -> String {
    sheet(s).value(CellRef::parse(cell).unwrap()).display()
}

fn formula(s: &Session, cell: &str) -> Option<String> {
    sheet(s).cell(CellRef::parse(cell).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.to_string())
}

fn table(s: &Session, name: &str) -> (String, bool) {
    let t = sheet(s).tables.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no table {name}"));
    (t.range.a1(), t.totals_row)
}

fn tables_overlap(wb: &Workbook) -> bool {
    wb.sheets.iter().any(|sh| sh.tables.iter().enumerate().any(|(i, t)| sh.tables.iter().skip(i + 1).any(|o| o.range.intersects(&t.range))))
}

const TOTAL_ROW: (&str, fn() -> Json) = ("table.totalRow", || json!({"table": "Grades", "on": true}));

#[test]
fn a_total_row_moves_what_is_below_the_table_down() {
    let mut s = grades(&[
        ("cell.set", json!({"cell": "B10", "input": "keep me"})),
        ("cell.set", json!({"cell": "C10", "input": "=1+1"})),
        ("cell.set", json!({"cell": "I10", "input": "beside"})),
    ]);
    s.execute(TOTAL_ROW.0, TOTAL_ROW.1()).unwrap();
    assert_eq!(table(&s, "Grades"), ("A1:G10".to_string(), true));
    assert_eq!(text(&s, "A10"), "Total");
    assert_eq!(text(&s, "B11"), "keep me", "the text below the table moved down");
    assert_eq!((formula(&s, "C11").as_deref(), text(&s, "C11").as_str()), (Some("1+1"), "2"), "so did the formula");
    assert_eq!(text(&s, "I10"), "beside", "cells beside the table stay where they are");

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(table(&s, "Grades"), ("A1:G9".to_string(), false), "one Undo takes the total row away");
    assert_eq!((text(&s, "B10").as_str(), formula(&s, "C10").as_deref()), ("keep me", Some("1+1")), "and puts the cells back");
    assert_eq!(text(&s, "B11"), "");
}

#[test]
fn a_total_row_moves_a_table_below_down_instead_of_sharing_its_cells() {
    let mut s = grades(&[
        ("range.setValues", json!({"range": "A10:B12", "values": [["H1", "H2"], [1, 2], [3, 4]]})),
        ("insert.table", json!({"range": "A10:B12"})),
    ]);
    let below = sheet(&s).tables.iter().find(|t| t.name != "Grades").unwrap().name.clone();
    assert_eq!(table(&s, &below).0, "A10:B12");
    s.execute(TOTAL_ROW.0, TOTAL_ROW.1()).unwrap();
    assert_eq!(table(&s, "Grades"), ("A1:G10".to_string(), true));
    assert_eq!(table(&s, &below).0, "A11:B13");
    assert_eq!((text(&s, "A11").as_str(), text(&s, "B13").as_str()), ("H1", "4"));
    assert!(!tables_overlap(&s.doc().unwrap().wb));
}

#[test]
fn a_total_row_that_would_split_a_table_below_is_refused_and_changes_nothing() {
    // The table below reaches past column G, so its cells can't all move down with the others.
    let mut s = grades(&[
        ("range.setValues", json!({"range": "F10:I12", "values": [["a", "b", "c", "d"], [1, 2, 3, 4], [5, 6, 7, 8]]})),
        ("insert.table", json!({"range": "F10:I12"})),
    ]);
    let before = (*s.doc().unwrap().wb).clone();
    let err = s.execute(TOTAL_ROW.0, TOTAL_ROW.1()).unwrap_err().to_string();
    assert!(err.contains("shift cells in a table"), "{err}");
    let after = &s.doc().unwrap().wb;
    assert_eq!(table(&s, "Grades"), ("A1:G9".to_string(), false));
    assert!(!tables_overlap(after));
    let used = RangeRef::parse("A1:I13").unwrap();
    for c in used.iter() {
        assert_eq!(after.sheets[0].value(c), before.sheets[0].value(c), "{} changed", c.a1());
    }
}

#[test]
fn a_total_row_with_a_free_row_below_moves_nothing() {
    let mut s = grades(&[("cell.set", json!({"cell": "B11", "input": "two rows down"}))]);
    s.execute(TOTAL_ROW.0, TOTAL_ROW.1()).unwrap();
    assert_eq!(table(&s, "Grades"), ("A1:G10".to_string(), true));
    assert_eq!(text(&s, "B11"), "two rows down");
    assert_eq!(text(&s, "B12"), "");
}
