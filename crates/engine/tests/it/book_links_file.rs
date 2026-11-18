//! References to other workbooks through files (`docs/external-references.md`, step A3): the
//! links are saved with a path relative to the workbook, read back with their values, and live
//! again once the other workbook is open.

use std::path::{Path, PathBuf};

use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// An empty folder of this test's own.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gridcraft-links-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn path(dir: &Path, names: &[&str]) -> String {
    let p = names.iter().fold(dir.to_path_buf(), |p, n| p.join(n));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

fn set(s: &mut Session, book: usize, cell: &str, input: &str) {
    s.set_active(book);
    run(s, "cell.set", json!({"cell": cell, "input": input}));
}

fn save(s: &mut Session, book: usize, to: &str) {
    s.set_active(book);
    run(s, "file.save", json!({"path": to}));
}

fn value(s: &Session, book: usize, cell: &str) -> Value {
    s.documents()[book].wb.sheets[0].value(CellRef::parse(cell).unwrap())
}

fn formula(s: &Session, book: usize, cell: &str) -> String {
    s.documents()[book].wb.sheets[0].cell(CellRef::parse(cell).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_default()
}

/// A session with one new workbook.
fn session() -> Session {
    let mut s = Session::new();
    run(&mut s, "file.new", json!({}));
    s
}

/// The targets of the link parts of the file at `file`.
fn targets(file: &str) -> Vec<String> {
    let (wb, _) = gridcraft_xlsx::read_xlsx(&std::fs::read(file).unwrap()).unwrap();
    wb.links.iter().map(|l| l.target.clone().unwrap_or_default()).collect()
}

/// A session with a saved `data/Budget.xlsx` (A1 = 10) and a report reading it, not saved yet.
fn budget_and_report(dir: &Path) -> Session {
    let mut s = session();
    set(&mut s, 0, "A1", "10");
    save(&mut s, 0, &path(dir, &["data", "Budget.xlsx"]));
    run(&mut s, "file.new", json!({}));
    set(&mut s, 1, "B1", "=[Budget.xlsx]Sheet1!A1*2");
    assert_eq!(value(&s, 1, "B1"), Value::Number(20.0));
    s
}

#[test]
fn a_link_is_saved_with_a_path_relative_to_the_workbook_and_comes_back_with_its_values() {
    let dir = folder("roundtrip");
    let mut s = budget_and_report(&dir);
    let report = path(&dir, &["reports", "Report.xlsx"]);
    save(&mut s, 1, &report);
    assert_eq!(targets(&report), ["../data/Budget.xlsx"]);
    assert!(!s.documents()[1].is_dirty(), "a saved workbook is not left changed by its own save");
    assert_eq!(s.documents()[1].wb.links[0].target.as_deref(), Some("../data/Budget.xlsx"));

    // Opened alone: the values the file saved, the formula by name.
    let mut alone = Session::new();
    run(&mut alone, "file.open", json!({"path": report}));
    let r = alone.active_index();
    assert_eq!(formula(&alone, r, "B1"), "[Budget.xlsx]Sheet1!A1*2");
    assert_eq!(value(&alone, r, "B1"), Value::Number(20.0));
    set(&mut alone, r, "C1", "=B1+[Budget.xlsx]Sheet1!A1");
    assert_eq!(value(&alone, r, "C1"), Value::Number(30.0), "a new formula reads the saved values too");
    // The other workbook opens: the reference is live again.
    run(&mut alone, "file.open", json!({"path": path(&dir, &["data", "Budget.xlsx"])}));
    let b = alone.active_index();
    set(&mut alone, b, "A1", "50");
    assert_eq!((value(&alone, r, "B1"), value(&alone, r, "C1")), (Value::Number(100.0), Value::Number(150.0)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_path_outlives_the_other_workbook_being_closed_and_follows_save_as() {
    let dir = folder("closed");
    let mut s = budget_and_report(&dir);
    let report = path(&dir, &["reports", "Report.xlsx"]);
    save(&mut s, 1, &report);
    // The other workbook is closed; the report is changed and saved again, then saved elsewhere.
    s.set_active(0);
    run(&mut s, "file.close", json!({"force": true}));
    let r = s.active_index();
    set(&mut s, r, "D1", "1");
    save(&mut s, r, &report);
    assert_eq!(targets(&report), ["../data/Budget.xlsx"]);
    let moved = path(&dir, &["Report copy.xlsx"]);
    save(&mut s, r, &moved);
    assert_eq!(targets(&moved), ["data/Budget.xlsx"]);
    // And from there once more: the path starts from where the workbook is now.
    let deeper = path(&dir, &["a", "b", "Report.xlsx"]);
    save(&mut s, r, &deeper);
    assert_eq!(targets(&deeper), ["../../data/Budget.xlsx"]);
    assert_eq!(value(&s, r, "B1"), Value::Number(20.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_link_of_a_file_keeps_the_values_formulas_here_do_not_read() {
    let dir = folder("inplace");
    let mut s = session();
    for (cell, input) in [("A1", "10"), ("A2", "11"), ("B7", "keep")] {
        set(&mut s, 0, cell, input);
    }
    let budget = path(&dir, &["Budget.xlsx"]);
    save(&mut s, 0, &budget);
    run(&mut s, "file.new", json!({}));
    set(&mut s, 1, "B1", "=SUM([Budget.xlsx]Sheet1!A1:A2)");
    set(&mut s, 1, "B2", "=[Budget.xlsx]Sheet1!B7");
    let report = path(&dir, &["Report.xlsx"]);
    save(&mut s, 1, &report);

    let mut t = Session::new();
    run(&mut t, "file.open", json!({"path": report}));
    // B2's formula goes: nothing here reads B7 any more, but the file's link has it.
    set(&mut t, 0, "B2", "");
    run(&mut t, "file.open", json!({"path": budget}));
    set(&mut t, 1, "A2", "12");
    assert_eq!(value(&t, 0, "B1"), Value::Number(22.0));
    let link = &t.documents()[0].wb.links[0];
    let sheet = link.sheet("Sheet1").unwrap();
    assert_eq!(sheet.value(CellRef::parse("B7").unwrap()), Value::text("keep"), "what the file had and nothing here reads stays");
    assert_eq!(sheet.value(CellRef::parse("A2").unwrap()), Value::Number(12.0));
    // A cell of the source that went blank goes from the copy.
    set(&mut t, 1, "A1", "");
    assert_eq!(t.documents()[0].wb.links[0].sheet("Sheet1").unwrap().value(CellRef::parse("A1").unwrap()), Value::Empty);
    assert_eq!(value(&t, 0, "B1"), Value::Number(12.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn of_two_open_workbooks_with_one_name_the_links_path_says_which() {
    let dir = folder("twins");
    let mut s = session();
    set(&mut s, 0, "A1", "1");
    let first = path(&dir, &["one", "Budget.xlsx"]);
    save(&mut s, 0, &first);
    let second = path(&dir, &["two", "Budget.xlsx"]);
    set(&mut s, 0, "A1", "2");
    save(&mut s, 0, &second);
    run(&mut s, "file.new", json!({}));
    set(&mut s, 1, "B1", "=[Budget.xlsx]Sheet1!A1");
    let report = path(&dir, &["Report.xlsx"]);
    save(&mut s, 1, &report);
    assert_eq!(targets(&report), ["two/Budget.xlsx"]);

    // Both open, the other one first: the report reads the one its link names.
    let mut t = Session::new();
    run(&mut t, "file.open", json!({"path": first}));
    run(&mut t, "file.open", json!({"path": second}));
    run(&mut t, "file.open", json!({"path": report}));
    let r = t.active_index();
    set(&mut t, 0, "A1", "9");
    assert_eq!(value(&t, r, "B1"), Value::Number(2.0), "the other Budget.xlsx is not the one read");
    set(&mut t, 1, "A1", "5");
    assert_eq!(value(&t, r, "B1"), Value::Number(5.0));
    let _ = std::fs::remove_dir_all(&dir);
}
