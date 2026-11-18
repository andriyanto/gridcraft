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
