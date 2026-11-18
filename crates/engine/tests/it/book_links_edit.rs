//! Data › Edit Links (`docs/external-references.md`, stages B and C): the list of the workbooks
//! a workbook reads, Change Source, Break Link, and Update Values and Open Source, which read a
//! linked workbook's file when a person asks.

use std::path::{Path, PathBuf};

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gridcraft-editlinks-{name}-{}", std::process::id()));
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

fn value(s: &Session, book: usize, cell: &str) -> Value {
    s.documents()[book].wb.sheets[0].value(CellRef::parse(cell).unwrap())
}

fn formula(s: &Session, book: usize, cell: &str) -> String {
    s.documents()[book].wb.sheets[0].cell(CellRef::parse(cell).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_default()
}

/// A workbook saved as `file` with A1 = `a1` and A2 = `a1 + 1`, closed again.
fn saved_book(file: &str, a1: i64) {
    let mut s = Session::new();
    run(&mut s, "file.new", json!({}));
    set(&mut s, 0, "A1", &a1.to_string());
    set(&mut s, 0, "A2", &(a1 + 1).to_string());
    run(&mut s, "file.save", json!({"path": file}));
}

/// A session whose first workbook reads `Budget.xlsx` (A1 = 10) in `dir`, saved there as
/// `Report.xlsx` and opened alone: B1 `=[Budget.xlsx]Sheet1!A1*2`.
fn report_alone(dir: &Path) -> Session {
    let budget = path(dir, &["Budget.xlsx"]);
    saved_book(&budget, 10);
    let report = path(dir, &["Report.xlsx"]);
    let mut s = Session::new();
    run(&mut s, "file.open", json!({"path": budget}));
    run(&mut s, "file.new", json!({}));
    set(&mut s, 1, "B1", "=[Budget.xlsx]Sheet1!A1*2");
    run(&mut s, "file.save", json!({"path": report}));
    let mut alone = Session::new();
    run(&mut alone, "file.open", json!({"path": report}));
    assert_eq!(value(&alone, 0, "B1"), Value::Number(20.0));
    alone
}

fn links(s: &mut Session, book: usize) -> Vec<Json> {
    s.set_active(book);
    run(s, "data.editLinks", json!({})).as_array().cloned().unwrap()
}

#[test]
fn the_list_says_which_workbooks_are_read_and_what_is_known_of_each() {
    let dir = folder("list");
    let mut s = report_alone(&dir);
    set(&mut s, 0, "B2", "=[Nowhere.xlsx]Sheet1!A1");
    set(&mut s, 0, "B3", "=SUM([Budget.xlsx]Sheet1!A1:A2)");
    let list = links(&mut s, 0);
    assert_eq!(list.len(), 2);
    let budget = &list[0];
    assert_eq!(
        (budget["name"].as_str(), budget["status"].as_str(), budget["target"].as_str()),
        (Some("Budget.xlsx"), Some("saved"), Some("Budget.xlsx"))
    );
    assert_eq!(budget["path"].as_str(), Some(path(&dir, &["Budget.xlsx"]).as_str()), "where its path says it is");
    assert_eq!((budget["reads"].as_u64(), budget["fromFile"].as_bool()), (Some(2), Some(true)));
    let nowhere = &list[1];
    assert_eq!((nowhere["name"].as_str(), nowhere["status"].as_str(), nowhere["path"].as_str()), (Some("Nowhere.xlsx"), Some("unknown"), None));
    assert_eq!(nowhere["fromFile"].as_bool(), Some(false));
    // The source opens: its link says so, with the file it is open from.
    run(&mut s, "file.open", json!({"path": path(&dir, &["Budget.xlsx"])}));
    assert_eq!(links(&mut s, 0)[0]["status"].as_str(), Some("open"));
    // A workbook that reads none has none.
    assert!(links(&mut s, 1).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn change_source_points_the_formulas_at_another_workbook() {
    let dir = folder("change");
    let mut s = report_alone(&dir);
    let other = path(&dir, &["plans", "Budget 2027.xlsx"]);
    saved_book(&other, 70);
    let r = run(&mut s, "data.changeLinkSource", json!({"link": "budget.xlsx", "path": other}));
    assert_eq!((r["name"].as_str(), r["read"].as_bool()), (Some("Budget 2027.xlsx"), Some(true)));
    assert_eq!(formula(&s, 0, "B1"), "'[Budget 2027.xlsx]Sheet1'!A1*2");
    // The file was named by the person: what the formulas read of it is read at once.
    assert_eq!(value(&s, 0, "B1"), Value::Number(140.0));
    assert_eq!(s.documents().len(), 1, "it is read, not opened");
    let list = links(&mut s, 0);
    assert_eq!(list.len(), 1);
    assert_eq!((list[0]["status"].as_str(), list[0]["target"].as_str()), (Some("saved"), Some("plans/Budget%202027.xlsx")));
    // It opens: the formulas follow it.
    run(&mut s, "file.open", json!({"path": other}));
    set(&mut s, 1, "A1", "71");
    assert_eq!(value(&s, 0, "B1"), Value::Number(142.0));
    set(&mut s, 1, "A1", "70");
    // Saved and opened alone, the link is the new one, with its values.
    let again = path(&dir, &["Report 2.xlsx"]);
    s.set_active(0);
    run(&mut s, "file.save", json!({"path": again}));
    let mut alone = Session::new();
    run(&mut alone, "file.open", json!({"path": again}));
    assert_eq!((formula(&alone, 0, "B1"), value(&alone, 0, "B1")), ("'[Budget 2027.xlsx]Sheet1'!A1*2".to_string(), Value::Number(140.0)));
    assert_eq!(links(&mut alone, 0).len(), 1);
    // Undo takes the change back, formulas and values.
    s.set_active(0);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(formula(&s, 0, "B1"), "[Budget.xlsx]Sheet1!A1*2");
    assert_eq!(value(&s, 0, "B1"), Value::Number(20.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn change_source_to_an_open_workbook_and_to_one_already_linked() {
    let dir = folder("merge");
    let mut s = report_alone(&dir);
    run(&mut s, "file.new", json!({}));
    let title = s.documents()[1].display_title();
    set(&mut s, 1, "A1", "3");
    set(&mut s, 0, "C1", &format!("=[{title}]Sheet1!A1"));
    assert_eq!(links(&mut s, 0).len(), 2);
    // To the open workbook, by its name: the two links become one, and the values are its.
    run(&mut s, "data.changeLinkSource", json!({"link": "Budget.xlsx", "name": title}));
    assert_eq!(formula(&s, 0, "B1"), format!("[{title}]Sheet1!A1*2"));
    assert_eq!((value(&s, 0, "B1"), value(&s, 0, "C1")), (Value::Number(6.0), Value::Number(3.0)));
    let list = links(&mut s, 0);
    assert_eq!(
        list.iter().map(|l| (l["name"].as_str().unwrap().to_string(), l["status"].as_str().unwrap().to_string())).collect::<Vec<_>>(),
        [(title, "open".to_string())]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn change_source_refuses_what_a_formula_cant_name() {
    let dir = folder("refuse");
    let mut s = report_alone(&dir);
    let refused = |s: &mut Session, params: Json| s.execute("data.changeLinkSource", params).unwrap_err().to_string();
    assert!(refused(&mut s, json!({"link": "Other.xlsx", "name": "Book9"})).contains("no link"));
    assert!(refused(&mut s, json!({"name": "Book9"})).contains("missing `link`"));
    assert!(refused(&mut s, json!({"link": "Budget.xlsx"})).contains("new source"));
    assert!(refused(&mut s, json!({"link": "Budget.xlsx", "name": "2027"})).contains("can't name"));
    assert!(refused(&mut s, json!({"link": "Budget.xlsx", "path": path(&dir, &["a]b.xlsx"])})).contains("can't name"));
    assert!(refused(&mut s, json!({"link": "Budget.xlsx", "name": "Report.xlsx"})).contains("own cells"));
    assert_eq!(formula(&s, 0, "B1"), "[Budget.xlsx]Sheet1!A1*2", "nothing changed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn break_link_leaves_the_values_and_takes_the_formulas_and_the_link() {
    let dir = folder("break");
    let mut s = report_alone(&dir);
    run(&mut s, "file.open", json!({"path": path(&dir, &["Budget.xlsx"])}));
    set(&mut s, 0, "D1", "=[Budget.xlsx]Sheet1!A1:A2");
    set(&mut s, 0, "E1", "=B1+1");
    assert_eq!((value(&s, 0, "D1"), value(&s, 0, "D2")), (Value::Number(10.0), Value::Number(11.0)), "a formula that fills two cells");
    s.set_active(0);
    let r = run(&mut s, "data.breakLink", json!({"link": "Budget.xlsx"}));
    assert_eq!((r["cells"].as_u64(), r["names"].as_array().map(Vec::len)), (Some(2), Some(0)));
    for (cell, was) in [("B1", 20.0), ("D1", 10.0), ("D2", 11.0)] {
        assert_eq!((formula(&s, 0, cell), value(&s, 0, cell)), (String::new(), Value::Number(was)), "{cell}");
    }
    assert_eq!((formula(&s, 0, "E1"), value(&s, 0, "E1")), ("B1+1".to_string(), Value::Number(21.0)), "a formula that reads no other workbook stays");
    assert!(links(&mut s, 0).is_empty());
    // Saved and opened again there is no link, and nothing of the other workbook in the file.
    let saved = path(&dir, &["Broken.xlsx"]);
    s.set_active(0);
    run(&mut s, "file.save", json!({"path": saved}));
    let mut alone = Session::new();
    run(&mut alone, "file.open", json!({"path": saved}));
    assert!(links(&mut alone, 0).is_empty());
    assert_eq!(value(&alone, 0, "D2"), Value::Number(11.0));
    let (kept, _) = gridcraft_xlsx::read_xlsx(&std::fs::read(&saved).unwrap()).unwrap();
    assert!(kept.preserved.as_ref().is_none_or(|p| p.parts.is_empty()), "the link's part is not in the file");
    // Undo brings the formulas and the link back.
    s.set_active(0);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!((formula(&s, 0, "B1"), formula(&s, 0, "D1")), ("[Budget.xlsx]Sheet1!A1*2".to_string(), "[Budget.xlsx]Sheet1!A1:A2".to_string()));
    assert_eq!(links(&mut s, 0).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_defined_name_that_reads_the_workbook_keeps_its_link() {
    let dir = folder("names");
    let mut s = report_alone(&dir);
    run(&mut s, "file.open", json!({"path": path(&dir, &["Budget.xlsx"])}));
    s.set_active(0);
    run(&mut s, "formulas.defineName", json!({"name": "Rate", "refersTo": "=[Budget.xlsx]Sheet1!$A$2"}));
    set(&mut s, 0, "C1", "=Rate");
    assert_eq!(value(&s, 0, "C1"), Value::Number(11.0));
    let r = run(&mut s, "data.breakLink", json!({"link": "Budget.xlsx"}));
    assert_eq!(r["names"], json!(["Rate"]));
    assert_eq!((formula(&s, 0, "B1"), value(&s, 0, "B1")), (String::new(), Value::Number(20.0)));
    assert_eq!((formula(&s, 0, "C1"), value(&s, 0, "C1")), ("Rate".to_string(), Value::Number(11.0)), "the name still has its value");
    assert_eq!(links(&mut s, 0).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn breaking_a_link_that_is_not_the_files_last_keeps_the_others_right() {
    let dir = folder("middle");
    let (first, second) = (path(&dir, &["First.xlsx"]), path(&dir, &["Second.xlsx"]));
    saved_book(&first, 1);
    saved_book(&second, 5);
    let report = path(&dir, &["Report.xlsx"]);
    let mut s = Session::new();
    run(&mut s, "file.open", json!({"path": first}));
    run(&mut s, "file.open", json!({"path": second}));
    run(&mut s, "file.new", json!({}));
    set(&mut s, 2, "B1", "=[First.xlsx]Sheet1!A1");
    set(&mut s, 2, "B2", "=[Second.xlsx]Sheet1!A1");
    run(&mut s, "file.save", json!({"path": report}));

    let mut alone = Session::new();
    run(&mut alone, "file.open", json!({"path": report}));
    run(&mut alone, "data.breakLink", json!({"link": "First.xlsx"}));
    let saved = path(&dir, &["Report 2.xlsx"]);
    run(&mut alone, "file.save", json!({"path": saved}));
    let mut again = Session::new();
    run(&mut again, "file.open", json!({"path": saved}));
    assert_eq!((formula(&again, 0, "B1"), value(&again, 0, "B1")), (String::new(), Value::Number(1.0)));
    assert_eq!(
        (formula(&again, 0, "B2"), value(&again, 0, "B2")),
        ("[Second.xlsx]Sheet1!A1".to_string(), Value::Number(5.0)),
        "the other link is still the right one"
    );
    // And the other way round: the file's last link goes from the file.
    run(&mut again, "data.breakLink", json!({"link": "Second.xlsx"}));
    assert!(!links(&mut again, 0).iter().any(|l| l["name"] == "Second.xlsx"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Changes `cell` of the workbook at `file` and saves it, in a session of its own.
fn change_file(file: &str, cell: &str, input: &str) {
    let mut s = Session::new();
    run(&mut s, "file.open", json!({"path": file}));
    set(&mut s, 0, cell, input);
    run(&mut s, "file.save", json!({"path": file}));
}

fn update(s: &mut Session, params: Json) -> Vec<(String, String, String)> {
    let text = |r: &Json, k: &str| r[k].as_str().unwrap_or("").to_string();
    run(s, "data.updateLinks", params).as_array().unwrap().iter().map(|r| (text(r, "name"), text(r, "status"), text(r, "message"))).collect()
}

#[test]
fn update_values_reads_a_closed_workbooks_file_when_asked() {
    let dir = folder("update");
    let mut s = report_alone(&dir);
    let budget = path(&dir, &["Budget.xlsx"]);
    // A cell the file's link never had, and a result another workbook's formula spills.
    set(&mut s, 0, "B2", "=[Budget.xlsx]Sheet1!A2");
    set(&mut s, 0, "B3", "=SUM([Budget.xlsx]Sheet1!C1:C3)");
    assert_eq!((value(&s, 0, "B2"), value(&s, 0, "B3")), (Value::Number(0.0), Value::Number(0.0)), "not known while the workbook is closed");
    change_file(&budget, "A1", "50");
    change_file(&budget, "C1", "=SEQUENCE(3)");
    assert_eq!(value(&s, 0, "B1"), Value::Number(20.0), "nothing is read until it is asked for");
    let steps = s.documents()[0].undo.len();
    assert_eq!(update(&mut s, json!({})), [("Budget.xlsx".to_string(), "updated".to_string(), String::new())]);
    assert_eq!((value(&s, 0, "B1"), value(&s, 0, "B2"), value(&s, 0, "B3")), (Value::Number(100.0), Value::Number(11.0), Value::Number(6.0)));
    assert_eq!(s.documents().len(), 1, "the workbook is read, not opened");
    assert_eq!(s.documents()[0].undo.len(), steps, "as when a source is followed: no Undo step");
    assert!(s.documents()[0].is_dirty());
    // Asked again with nothing new in the file, and for a workbook that is open.
    assert_eq!(update(&mut s, json!({"link": "budget.xlsx"}))[0].1, "unchanged");
    run(&mut s, "file.open", json!({"path": budget}));
    s.set_active(0);
    assert_eq!(update(&mut s, json!({}))[0].1, "open");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_workbook_on_this_computer_is_read_and_only_when_a_person_asks() {
    let dir = folder("refused");
    let mut s = report_alone(&dir);
    let failed = |s: &mut Session| {
        let r = update(s, json!({}));
        assert_eq!(r[0].1, "failed", "{r:?}");
        r[0].2.clone()
    };
    // A script's steps read no file: a workbook's macro can't have a linked file read.
    change_file(&path(&dir, &["Budget.xlsx"]), "A1", "50");
    {
        let _sandbox = gridcraft_engine::io::ScriptSandbox::enter();
        failed(&mut s);
    }
    assert_eq!(value(&s, 0, "B1"), Value::Number(20.0));
    // The file is gone.
    std::fs::remove_file(path(&dir, &["Budget.xlsx"])).unwrap();
    assert!(failed(&mut s).contains("Budget.xlsx"));
    // A file that is not a workbook is not taken in as cells, whatever it is called.
    let secret = path(&dir, &["notes.xlsx"]);
    std::fs::write(&secret, "password,hunter2\n").unwrap();
    let r = run(&mut s, "data.changeLinkSource", json!({"link": "Budget.xlsx", "path": secret}));
    assert_eq!(r["read"].as_bool(), Some(false));
    assert!(r["message"].as_str().unwrap().contains("not a workbook"), "{r}");
    assert!(failed(&mut s).contains("not a workbook"));
    assert_eq!(value(&s, 0, "B1"), Value::Error(CellError::Ref));
    let text = serde_json::to_string(&*s.documents()[0].wb).unwrap();
    assert!(!text.contains("hunter2"), "nothing of the file is in the workbook");
    // A link that has no path (its workbook was only ever named) has nothing to read.
    set(&mut s, 0, "C1", "=[Nowhere]Sheet1!A1");
    let r = update(&mut s, json!({"link": "Nowhere"}));
    assert!(r[0].1 == "failed" && r[0].2.contains("no path"), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_source_opens_the_linked_workbook() {
    let dir = folder("open");
    let mut s = report_alone(&dir);
    let r = run(&mut s, "data.openLinkSource", json!({"link": "Budget.xlsx"}));
    assert_eq!(r["path"].as_str(), Some(path(&dir, &["Budget.xlsx"]).as_str()));
    assert_eq!(s.documents().len(), 2);
    assert_eq!(s.active().map(|d| d.display_title()).as_deref(), Some("Budget.xlsx"));
    assert_eq!(links(&mut s, 0)[0]["status"].as_str(), Some("open"));
    // Asked again, it goes to the workbook that is open.
    s.set_active(0);
    let r = run(&mut s, "data.openLinkSource", json!({"link": "Budget.xlsx"}));
    assert_eq!((r["open"].as_bool(), s.documents().len()), (Some(true), 2));
    assert_eq!(s.active().map(|d| d.display_title()).as_deref(), Some("Budget.xlsx"));
    let _ = std::fs::remove_dir_all(&dir);
}
