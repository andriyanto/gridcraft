//! Data › Edit Links: the dialog lists the workbooks the active one reads and acts on the one
//! selected (`docs/external-references.md`, stage B).

use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type H = egui_kittest::Harness<'static, SheetApp>;

/// Book1 reads Book2 and Book3 (B1, B2), with Book1 active.
fn reader() -> H {
    let mut s = Session::new();
    for _ in 0..3 {
        s.new_workbook();
    }
    for (book, value) in [(1, "5"), (2, "7")] {
        s.set_active(book);
        s.execute("cell.set", json!({"cell": "A1", "input": value})).unwrap();
    }
    s.set_active(0);
    let titles: Vec<String> = s.documents().iter().map(|d| d.display_title()).collect();
    s.execute("cell.set", json!({"cell": "B1", "input": format!("=[{}]Sheet1!A1", titles[1])})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": format!("=[{}]Sheet1!A1*2", titles[2])})).unwrap();
    crate::support::harness(s, 4)
}

fn cell(h: &H, a1: &str) -> (String, Value) {
    let sheet = h.state().session.documents()[0].wb.sheet(0).unwrap();
    let at = CellRef::parse(a1).unwrap();
    (sheet.cell(at).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_default(), sheet.value(at))
}

#[test]
fn the_dialog_lists_the_links_and_breaks_the_one_selected() {
    let mut h = reader();
    let titles: Vec<String> = h.state().session.documents().iter().map(|d| d.display_title()).collect();
    h.state_mut().open_dialog("editLinks", json!({}));
    h.run_steps(3);
    assert!(h.query_by_label(&titles[1]).is_some() && h.query_by_label(&titles[2]).is_some(), "both workbooks are listed");
    assert_eq!(h.query_all_by_label("Open: values follow it").count(), 2);
    // The second link is selected and broken: its formula is its value, the other one stays.
    h.get_by_label(&titles[2]).click();
    h.run_steps(2);
    h.get_by_label("Break Link").click();
    h.run_steps(3);
    assert_eq!(cell(&h, "B2"), (String::new(), Value::Number(14.0)));
    assert_eq!(cell(&h, "B1"), (format!("[{}]Sheet1!A1", titles[1]), Value::Number(5.0)));
    assert!(h.query_by_label(&titles[2]).is_none(), "the broken link is gone from the list");
    assert!(h.state().dialog.is_some(), "the dialog stays for the next link");
}

#[test]
fn change_source_takes_the_name_of_an_open_workbook() {
    let mut h = reader();
    let titles: Vec<String> = h.state().session.documents().iter().map(|d| d.display_title()).collect();
    h.state_mut().open_dialog("editLinks", json!({}));
    h.run_steps(3);
    // The first link (selected as the dialog opens) is pointed at Book3.
    h.state_mut().dialog.as_mut().unwrap().values.insert("source".into(), json!(titles[2]));
    h.run_steps(2);
    h.get_by_label("Change Source").click();
    h.run_steps(3);
    assert_eq!(cell(&h, "B1"), (format!("[{}]Sheet1!A1", titles[2]), Value::Number(7.0)));
    assert!(h.query_by_label(&titles[1]).is_none(), "one link is left");
    // A name no formula can hold is refused with the reason, and nothing changes.
    h.state_mut().dialog.as_mut().unwrap().values.insert("source".into(), json!("2027"));
    h.run_steps(2);
    h.get_by_label("Change Source").click();
    h.run_steps(3);
    assert!(h.state().message.as_ref().is_some_and(|(_, m)| m.contains("can't name")), "{:?}", h.state().message);
    assert_eq!(cell(&h, "B1").0, format!("[{}]Sheet1!A1", titles[2]));
}

/// `Budget.xlsx` (A1 = 10) and `Report.xlsx` (B1 `=[Budget.xlsx]Sheet1!A1*2`) in a folder of
/// this test's own; returns the two paths.
fn files(name: &str) -> (std::path::PathBuf, String, String) {
    let dir = std::env::temp_dir().join(format!("gridcraft-ui-links-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = |file: &str| dir.join(file).to_string_lossy().into_owned();
    let (budget, report) = (path("Budget.xlsx"), path("Report.xlsx"));
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "10"})).unwrap();
    s.execute("file.save", json!({"path": budget})).unwrap();
    s.execute("file.new", json!({})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=[Budget.xlsx]Sheet1!A1*2"})).unwrap();
    s.execute("file.save", json!({"path": report})).unwrap();
    (dir, budget, report)
}

#[test]
fn a_workbook_with_closed_sources_says_so_and_update_values_reads_them() {
    let (dir, budget, report) = files("update");
    let mut s = Session::new();
    s.new_workbook();
    let mut h = crate::support::harness(s, 4);
    h.state_mut().open_path(&report);
    h.run_steps(3);
    // The bar under the ribbon says where the values come from and leads to Edit Links.
    let notice = h.state().notice.clone().expect("a notice about the workbooks that are not open");
    assert!(notice.text.contains("not open"), "{}", notice.text);
    assert_eq!(notice.action, Some(("Edit Links…".to_string(), "editLinks".to_string())));
    assert_eq!(cell(&h, "B1").1, Value::Number(20.0));
    // The other workbook changes on disk; nothing here reads it until Update Values is pressed.
    let mut other = Session::new();
    other.execute("file.open", json!({"path": budget})).unwrap();
    other.execute("cell.set", json!({"cell": "A1", "input": "50"})).unwrap();
    other.execute("file.save", json!({"path": budget})).unwrap();
    h.state_mut().message = None;
    h.state_mut().open_dialog("editLinks", json!({}));
    h.run_steps(3);
    assert!(h.query_by_label("Not open: values from the file").is_some());
    assert_eq!(cell(&h, "B1").1, Value::Number(20.0));
    h.get_by_label("Update Values").click();
    h.run_steps(3);
    assert_eq!(cell(&h, "B1").1, Value::Number(100.0));
    assert!(h.state().message.is_none(), "{:?}", h.state().message);
    // With the file gone, the reason is shown and the values stay.
    std::fs::remove_file(&budget).unwrap();
    h.get_by_label("Update Values").click();
    h.run_steps(3);
    assert!(h.state().message.as_ref().is_some_and(|(_, m)| m.contains("Budget.xlsx")), "{:?}", h.state().message);
    assert_eq!(cell(&h, "B1").1, Value::Number(100.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_source_opens_the_workbook_and_the_link_follows_it() {
    let (dir, _budget, report) = files("open");
    let mut s = Session::new();
    s.new_workbook();
    let mut h = crate::support::harness(s, 4);
    h.state_mut().open_path(&report);
    h.state_mut().message = None;
    h.state_mut().open_dialog("editLinks", json!({}));
    h.run_steps(3);
    h.get_by_label("Open Source").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "the dialog closes: the other workbook is in front");
    let titles: Vec<String> = h.state().session.documents().iter().map(|d| d.display_title()).collect();
    assert_eq!(titles, ["Report.xlsx", "Budget.xlsx"]);
    assert_eq!(h.state().session.active().map(|d| d.display_title()).as_deref(), Some("Budget.xlsx"));
    let _ = std::fs::remove_dir_all(&dir);
}
