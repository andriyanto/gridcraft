//! A filter on a table. Its criteria live in the sheet filter and a file keeps them in the
//! table's own filter buttons, so a filter put on a table whose buttons were off was not saved:
//! the workbook opened with the rows still hidden and no filter to clear (found by
//! `command_sequence_fuzz`, where a SUBTOTAL over those rows changed with the reload).

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use gridcraft_engine::io;
use gridcraft_model::{Sheet, Workbook};
use serde_json::{Value as Json, json};

/// The `sales` sample: table `Sales` in B4:I11 with a header row, six regions and a total row
/// whose C11 is `=SUBTOTAL(109,Sales[Q1])`.
fn sales(steps: &[(&str, Json)]) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "sales"})).unwrap();
    s.execute("selection.set", json!({"range": "C6"})).unwrap();
    for (id, params) in steps {
        s.execute(id, params.clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    s
}

fn q1_over_50000() -> (&'static str, Json) {
    ("data.filterBy", json!({"column": "C", "custom": {"op": ">", "value": "50000"}}))
}

/// How many criteria filter the table (a workbook just opened holds them in the table, an edited
/// one in the sheet filter), the rows they hide, whether the table shows its buttons, and the
/// total of the visible Q1 cells.
fn state(sh: &Sheet) -> (usize, Vec<u32>, bool, f64) {
    let table = sh.tables.iter().find(|t| t.name == "Sales");
    let criteria = sh.autofilter.as_ref().map_or(0, |af| af.criteria.len()) + table.map_or(0, |t| t.filter.len());
    let filtered = (0..40).filter(|r| sh.is_row_filtered(*r)).map(|r| r + 1).collect();
    (criteria, filtered, table.is_some_and(|t| t.filter_button), sh.value(CellRef::parse("C11").unwrap()).as_f64().unwrap_or(f64::NAN))
}

fn filter_range(sh: &Sheet) -> Option<String> {
    sh.autofilter.as_ref().map(|af| af.range.a1())
}

fn reloaded(wb: &Workbook) -> Workbook {
    let bytes = io::save_bytes(wb, "book.xlsx").unwrap();
    let (mut back, _) = io::open_bytes("book.xlsx", &bytes).unwrap();
    Calc::new().recalc_all(&mut back);
    back
}

/// Q1 over 50,000 leaves East, West and Online: rows 5, 6 and 9 go, and the total is 188,730.
const FILTERED: (usize, [u32; 3], bool, f64) = (1, [5, 6, 9], true, 188_730.0);

fn filtered() -> (usize, Vec<u32>, bool, f64) {
    (FILTERED.0, FILTERED.1.to_vec(), FILTERED.2, FILTERED.3)
}

#[test]
fn a_filter_on_a_table_with_its_buttons_off_is_the_tables_and_is_saved() {
    // `data.filter` with the active cell in the table turns the table's buttons off.
    let s = sales(&[("data.filter", json!({})), q1_over_50000()]);
    let sh = &s.doc().unwrap().wb.sheets[0];
    assert_eq!(state(sh), filtered(), "the rows are filtered, and the buttons are back");
    assert_eq!(filter_range(sh), Some("B4:I10".to_string()), "the filter covers the table's header and data rows");
    assert_eq!(state(&reloaded(&s.doc().unwrap().wb).sheets[0]), filtered(), "after a save and reload");
}

#[test]
fn a_filter_on_a_table_with_its_buttons_on_is_saved_as_before() {
    let s = sales(&[q1_over_50000()]);
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), filtered());
    assert_eq!(state(&reloaded(&s.doc().unwrap().wb).sheets[0]), filtered(), "after a save and reload");
}

#[test]
fn turning_a_tables_buttons_off_clears_its_filter() {
    let mut s = sales(&[q1_over_50000()]);
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), filtered());
    s.execute("data.filter", json!({})).unwrap();
    let cleared = (0, vec![], false, 304_820.0);
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), cleared, "no filter is left in force, and every row is back");
    assert!(!(4..10).any(|r| s.doc().unwrap().wb.sheets[0].is_row_hidden(r)));
    assert_eq!(state(&reloaded(&s.doc().unwrap().wb).sheets[0]), cleared, "after a save and reload");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), filtered(), "undo brings the filter back");
}

#[test]
fn a_named_range_beside_the_table_gets_a_sheet_filter_whatever_cell_is_active() {
    // The active cell is in the table; the range is not.
    let mut s = sales(&[("range.setValues", json!({"range": "N20", "values": [["k", "v"], ["a", 1], ["b", 2], ["a", 3]]}))]);
    s.execute("selection.set", json!({"range": "C6"})).unwrap();
    s.execute("data.filter", json!({"range": "N20:O23"})).unwrap();
    s.execute("data.filterBy", json!({"column": "N", "values": ["a"]})).unwrap();
    let sh = &s.doc().unwrap().wb.sheets[0];
    assert_eq!(filter_range(sh), Some("N20:O23".to_string()));
    assert_eq!((0..40).filter(|r| sh.is_row_filtered(*r)).map(|r| r + 1).collect::<Vec<_>>(), vec![22]);
    assert!(sh.tables.iter().find(|t| t.name == "Sales").is_some_and(|t| t.filter_button), "the table keeps its buttons");
    let back = reloaded(&s.doc().unwrap().wb);
    assert_eq!(back.sheets[0].autofilter.as_ref().map(|af| (af.range.a1(), af.criteria.len())), Some(("N20:O23".to_string(), 1)));
}

/// The workbook saved and opened in a new session, as a file is.
fn opened_again(s: &mut Session) -> Session {
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let mut opened = Session::new();
    opened.execute("file.open", json!({"name": "book.xlsx", "base64": saved["base64"]})).unwrap();
    opened.execute("selection.set", json!({"range": "C6"})).unwrap();
    opened
}

#[test]
fn a_table_filter_opened_from_a_file_can_be_cleared() {
    let mut s = opened_again(&mut sales(&[q1_over_50000()]));
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), filtered());
    s.execute("data.clearFilter", json!({})).unwrap();
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), (0, vec![], true, 304_820.0), "every row is back and no criteria are left");
    let again = opened_again(&mut s);
    assert_eq!(state(&again.doc().unwrap().wb.sheets[0]), (0, vec![], true, 304_820.0), "and it stays cleared through another save");
}

#[test]
fn a_table_filter_opened_from_a_file_takes_another_criterion_and_saves_both() {
    let mut s = opened_again(&mut sales(&[q1_over_50000()]));
    // Q2 over 60,000 leaves West and Online of the three rows still shown.
    s.execute("data.filterBy", json!({"column": "D", "custom": {"op": ">", "value": "60000"}})).unwrap();
    let both = (2, vec![5, 6, 7, 9], true, 133_430.0);
    assert_eq!(state(&s.doc().unwrap().wb.sheets[0]), both);
    let again = opened_again(&mut s);
    assert_eq!(state(&again.doc().unwrap().wb.sheets[0]), both, "both criteria reach the file");
}
