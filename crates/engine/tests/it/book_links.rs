//! References between open workbooks (`docs/external-references.md`, step A2): what a workbook
//! keeps of another open workbook's cells follows that workbook after every command.

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::{DocState, Session, Trust};
use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A session with the workbooks `titles`, each new and empty.
fn session(titles: &[&str]) -> Session {
    let mut s = Session::new();
    for title in titles {
        s.add_document(DocState::new(Workbook::new(), None, (*title).to_string(), Trust::Local));
    }
    s
}

/// Sets `cell` of the workbook `book` (it becomes the active one).
fn set(s: &mut Session, book: usize, cell: &str, input: &str) {
    s.set_active(book);
    run(s, "cell.set", json!({"cell": cell, "input": input}));
}

fn value(s: &Session, book: usize, cell: &str) -> Value {
    s.documents()[book].wb.sheets[0].value(CellRef::parse(cell).unwrap())
}

fn formula(s: &Session, book: usize, cell: &str) -> String {
    s.documents()[book].wb.sheets[0].cell(CellRef::parse(cell).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_default()
}

#[test]
fn a_formula_reads_another_open_workbook_and_follows_it() {
    let mut s = session(&["Report", "Budget"]);
    set(&mut s, 1, "A1", "10");
    set(&mut s, 0, "B1", "=[Budget]Sheet1!A1*2");
    set(&mut s, 0, "C1", "=B1+1");
    assert_eq!((value(&s, 0, "B1"), value(&s, 0, "C1")), (Value::Number(20.0), Value::Number(21.0)));
    // The source changes: the reader follows, with what depends on the formula.
    let steps = s.documents()[0].undo.len();
    set(&mut s, 1, "A1", "15");
    assert_eq!((value(&s, 0, "B1"), value(&s, 0, "C1")), (Value::Number(30.0), Value::Number(31.0)));
    assert_eq!(s.documents()[0].undo.len(), steps, "following a source is no Undo step of the reader");
    assert!(s.documents()[0].is_dirty());
    // Undo in the source is followed too.
    s.set_active(1);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(value(&s, 0, "B1"), Value::Number(20.0));
    // And an Undo in the reader takes back the reader's own last entry, not the source's values.
    s.set_active(0);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!((value(&s, 0, "C1"), value(&s, 0, "B1")), (Value::Empty, Value::Number(20.0)));
}

#[test]
fn blocks_whole_columns_text_and_errors_come_across() {
    let mut s = session(&["Report", "Budget"]);
    for (cell, input) in [("A1", "1"), ("A2", "2"), ("A3", "3"), ("B1", "x"), ("B2", "=1/0")] {
        set(&mut s, 1, cell, input);
    }
    set(&mut s, 0, "D1", "=SUM([Budget]Sheet1!A1:A3)");
    set(&mut s, 0, "D2", "=SUM([Budget]Sheet1!A:A)+ROWS([Budget]Sheet1!A:A)");
    set(&mut s, 0, "D3", "=[Budget]Sheet1!B1&\"!\"");
    set(&mut s, 0, "D4", "=[Budget]Sheet1!B2");
    set(&mut s, 0, "D5", "=[Budget]Sheet1!Z9+1");
    assert_eq!(value(&s, 0, "D1"), Value::Number(6.0));
    assert_eq!(value(&s, 0, "D2"), Value::Number(9.0));
    assert_eq!(value(&s, 0, "D3"), Value::text("x!"));
    assert_eq!(value(&s, 0, "D4"), Value::Error(CellError::Div0));
    assert_eq!(value(&s, 0, "D5"), Value::Number(1.0), "a blank cell of the source is blank");
    // A row added to the source's column is read at once.
    set(&mut s, 1, "A4", "10");
    assert_eq!(value(&s, 0, "D2"), Value::Number(20.0));
    // Only what the formulas read is kept.
    let link = &s.documents()[0].wb.links[0];
    assert_eq!(link.name, "Budget");
    assert!(link.cell_count() <= 6, "{}", link.cell_count());
}

#[test]
fn a_sheet_the_source_does_not_have_is_a_ref_error_until_it_has_it() {
    let mut s = session(&["Report", "Budget"]);
    set(&mut s, 0, "A1", "=[Budget]Totals!A1");
    assert_eq!(value(&s, 0, "A1"), Value::Error(CellError::Ref));
    s.set_active(1);
    run(&mut s, "home.insertSheet", json!({"name": "Totals"}));
    run(&mut s, "cell.set", json!({"sheet": "Totals", "cell": "A1", "input": "7"}));
    assert_eq!(value(&s, 0, "A1"), Value::Number(7.0));
}

#[test]
fn a_workbook_that_is_not_open_is_a_ref_error_and_is_read_once_it_is() {
    let mut s = session(&["Report"]);
    set(&mut s, 0, "A1", "=[Budget]Sheet1!A1+1");
    assert_eq!(value(&s, 0, "A1"), Value::Error(CellError::Ref), "not a blank that reads as 0");
    let mut budget = Workbook::new();
    budget.sheet_mut(0).unwrap().cells.set(CellRef::parse("A1").unwrap(), gridcraft_model::Cell::value(Value::Number(4.0)));
    s.add_document(DocState::new(budget, None, "Budget".into(), Trust::Local));
    // The next command, whatever it is, finds it.
    s.set_active(0);
    run(&mut s, "selection.set", json!({"range": "C3"}));
    assert_eq!(value(&s, 0, "A1"), Value::Number(5.0));
    // Closed again, the reader keeps the values it has.
    s.close_document(1);
    set(&mut s, 0, "B1", "=A1*2");
    assert_eq!((value(&s, 0, "A1"), value(&s, 0, "B1")), (Value::Number(5.0), Value::Number(10.0)));
}

#[test]
fn a_chain_of_workbooks_settles_within_the_command() {
    let mut s = session(&["C", "B", "A"]);
    set(&mut s, 2, "A1", "1");
    set(&mut s, 1, "A1", "=[A]Sheet1!A1+10");
    set(&mut s, 0, "A1", "=[B]Sheet1!A1+100");
    assert_eq!(value(&s, 0, "A1"), Value::Number(111.0));
    set(&mut s, 2, "A1", "2");
    assert_eq!((value(&s, 1, "A1"), value(&s, 0, "A1")), (Value::Number(12.0), Value::Number(112.0)));
}

#[test]
fn workbooks_that_read_each_other_stop() {
    let mut s = session(&["A", "B"]);
    set(&mut s, 0, "A1", "=[B]Sheet1!A1+1");
    set(&mut s, 1, "A1", "=[A]Sheet1!A1+1");
    // No fixed point exists; the command returns, and both hold numbers.
    assert!(matches!(value(&s, 0, "A1"), Value::Number(_)));
    assert!(matches!(value(&s, 1, "A1"), Value::Number(_)));
}

#[test]
fn a_source_saved_under_another_name_is_that_name_in_the_formulas_that_read_it() {
    let mut s = session(&["Report", "Book2"]);
    set(&mut s, 1, "A1", "10");
    set(&mut s, 0, "B1", "=[Book2]Sheet1!A1*2");
    set(&mut s, 0, "B2", "=SUM([Book2]Sheet1!A1:A2,B1)");
    // As Save As does: the workbook now has a file, and its window shows the file's name.
    s.set_active(1);
    s.active_mut().unwrap().path = Some(std::env::temp_dir().join("My Budget.xlsx").to_string_lossy().into_owned());
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "11"}));
    assert_eq!(formula(&s, 0, "B1"), "'[My Budget.xlsx]Sheet1'!A1*2");
    assert_eq!(formula(&s, 0, "B2"), "SUM('[My Budget.xlsx]Sheet1'!A1:A2,B1)");
    assert_eq!((value(&s, 0, "B1"), value(&s, 0, "B2")), (Value::Number(22.0), Value::Number(33.0)));
    assert_eq!(s.documents()[0].wb.links.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["My Budget.xlsx"]);
}

/// A file's formulas into other workbooks are written with the link's number. Their links come
/// with the file; none is made up for a number, so such a cell keeps the value the file saved.
#[test]
fn a_file_s_numbered_reference_keeps_its_saved_value() {
    let mut wb = Workbook::new();
    let cell = gridcraft_model::Cell { value: Value::Number(42.0), ..gridcraft_model::Cell::formula(gridcraft_model::Formula::new("=[1]Sheet1!B4")) };
    wb.sheet_mut(0).unwrap().cells.set(CellRef::parse("A1").unwrap(), cell);
    let mut s = Session::new();
    s.add_document(DocState::from_saved(wb, None, "FromExcel".into(), Trust::FromFile));
    s.add_document(DocState::new(Workbook::new(), None, "1".into(), Trust::Local));
    set(&mut s, 0, "B1", "=A1+1");
    set(&mut s, 0, "B2", "5");
    assert_eq!((value(&s, 0, "A1"), value(&s, 0, "B1")), (Value::Number(42.0), Value::Number(43.0)));
    assert!(s.documents()[0].wb.links.is_empty());
}

#[test]
fn a_session_in_which_no_workbook_reads_another_is_left_alone() {
    let mut s = session(&["A", "B"]);
    set(&mut s, 0, "A1", "=SUM(B1:B3)");
    set(&mut s, 1, "A1", "1");
    let (a, b) = (s.documents()[0].wb.clone(), s.documents()[1].wb.clone());
    s.set_active(0);
    run(&mut s, "selection.set", json!({"range": "C3"}));
    assert!(std::sync::Arc::ptr_eq(&a, &s.documents()[0].wb) && std::sync::Arc::ptr_eq(&b, &s.documents()[1].wb));
    assert!(s.documents().iter().all(|d| d.wb.links.is_empty()));
}

/// A command that changes nothing a reader reads leaves the reader's workbook as it is: no new
/// version, no recalculation.
#[test]
fn a_reader_is_left_alone_when_what_it_reads_did_not_change() {
    let mut s = session(&["Report", "Budget"]);
    set(&mut s, 1, "A1", "10");
    set(&mut s, 0, "B1", "=[Budget]Sheet1!A1");
    let before = s.documents()[0].wb.clone();
    set(&mut s, 1, "Z50", "elsewhere");
    assert!(std::sync::Arc::ptr_eq(&before, &s.documents()[0].wb), "nothing the reader reads changed");
    s.set_active(1);
    run(&mut s, "selection.set", json!({"range": "C3"}));
    assert!(std::sync::Arc::ptr_eq(&before, &s.documents()[0].wb));
}
