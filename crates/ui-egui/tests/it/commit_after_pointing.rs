//! A formula started on one sheet and pointed at another sheet lands on the sheet it started on.

use egui::vec2;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

/// Two sheets, `range` selected on the first, and `=1+2` typed there; then pointing moves to the second sheet.
fn pointing_on_second_sheet(range: &str) -> egui_kittest::Harness<'static, SheetApp> {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("home.insertSheet", json!({})).unwrap();
    session.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    session.execute("selection.set", json!({"range": range})).unwrap();
    let mut h = crate::common::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        crate::common::app(session),
    );
    h.run_steps(4);
    h.state_mut().begin_edit(Some("=1+2".into()), false);
    h.run_steps(2);
    // Pointing moves to the other sheet without finishing the edit.
    h.state_mut().session.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    h
}

#[test]
fn commit_after_pointing_on_another_sheet_writes_to_the_origin_sheet() {
    let mut h = pointing_on_second_sheet("F8");
    assert!(h.state_mut().commit_edit(0, 0, false, false));
    let at = CellRef::parse("F8").unwrap();
    let wb = &h.state().session.active().unwrap().wb;
    assert_eq!(wb.active_sheet(), 0, "the origin sheet is shown again");
    assert!(wb.sheet(0).unwrap().cell(at).is_some_and(|c| c.formula.is_some()), "formula written on the origin sheet");
    assert!(wb.sheet(1).unwrap().cell(at).is_none(), "the pointed-at sheet is untouched");
}

#[test]
fn ctrl_enter_after_pointing_on_another_sheet_fills_the_origin_selection() {
    let mut h = pointing_on_second_sheet("B2:C3");
    assert!(h.state_mut().commit_edit(0, 0, false, true));
    let wb = &h.state().session.active().unwrap().wb;
    assert_eq!(wb.active_sheet(), 0, "the origin sheet is shown again");
    for a1 in ["B2", "C2", "B3", "C3"] {
        let at = CellRef::parse(a1).unwrap();
        assert!(wb.sheet(0).unwrap().cell(at).is_some_and(|c| c.formula.is_some()), "{a1} filled on the origin sheet");
        assert!(wb.sheet(1).unwrap().cell(at).is_none(), "{a1} on the pointed-at sheet is untouched");
    }
    assert!(wb.sheet(1).unwrap().cell(CellRef::parse("A1").unwrap()).is_none(), "the pointed-at sheet's selection is untouched");
}

mod keys {
    //! The keys of a formula that points at another sheet. The in-cell editor is drawn on the
    //! formula's own sheet only, and it was the one that took the draft's keys: with another
    //! sheet shown, Enter, Esc and typing did nothing until the formula bar was clicked.

    use egui::{Event, Key, Modifiers, PointerButton, Pos2};
    use gridcraft_engine::Session;
    use gridcraft_engine::core::CellRef;
    use gridcraft_ui_egui::grid::Geo;
    use serde_json::json;

    type H = crate::common::Harness;

    fn cell_pos(h: &H, cell: &str) -> Pos2 {
        let app = h.state();
        let sheet = app.session.active().unwrap().wb.active().unwrap();
        let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
        geo.cell_rect(sheet, CellRef::parse(cell).unwrap()).center()
    }

    fn click_at(h: &mut H, pos: Pos2) {
        h.input_mut().events.push(Event::PointerMoved(pos));
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.run_steps(3);
    }

    /// Where the tab of sheet `name` is painted.
    fn tab_pos(h: &H, name: &str) -> Pos2 {
        h.output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == name => Some(t.visual_bounding_rect().center()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the {name} tab is painted"))
    }

    fn key(h: &mut H, key: Key) {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        }
        h.run_steps(3);
    }

    fn text(h: &mut H, text: &str) {
        h.input_mut().events.push(Event::Text(text.into()));
        h.run_steps(3);
    }

    fn draft(h: &H) -> Option<String> {
        h.state().editor.as_ref().map(|ed| ed.text.clone())
    }

    fn formula(h: &H, sheet: usize, cell: &str) -> Option<String> {
        let wb = &h.state().session.active().unwrap().wb;
        wb.sheet(sheet).unwrap().cell(CellRef::parse(cell).unwrap()).and_then(|c| c.formula.as_ref().map(|f| f.text.clone()))
    }

    /// `=` typed in Sheet1!B2 with the keyboard, then the Sheet2 tab and Sheet2!D6 clicked.
    fn pointing_at_sheet2() -> H {
        let mut session = Session::new();
        session.new_workbook();
        session.execute("home.insertSheet", json!({})).unwrap();
        session.execute("cell.set", json!({"cell": "D6", "input": "40"})).unwrap();
        session.execute("cell.set", json!({"cell": "F9", "input": "2"})).unwrap();
        session.execute("sheet.activate", json!({"sheet": 0})).unwrap();
        let mut h = crate::support::harness(session, 4);
        let b2 = cell_pos(&h, "B2");
        click_at(&mut h, b2);
        text(&mut h, "=");
        assert_eq!(draft(&h).as_deref(), Some("="));
        let tab = tab_pos(&h, "Sheet2");
        click_at(&mut h, tab);
        assert_eq!(h.state().session.active().unwrap().wb.active_sheet(), 1, "the other sheet is shown");
        let d6 = cell_pos(&h, "D6");
        click_at(&mut h, d6);
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D6"));
        h
    }

    #[test]
    fn enter_commits_a_formula_that_points_at_another_sheet() {
        let mut h = pointing_at_sheet2();
        key(&mut h, Key::Enter);
        assert_eq!(draft(&h), None, "Enter entered the formula");
        assert_eq!(h.state().session.active().unwrap().wb.active_sheet(), 0, "its own sheet is shown again");
        assert_eq!(formula(&h, 0, "B2").as_deref(), Some("Sheet2!D6"));
        assert_eq!(h.state().session.active().unwrap().selection.active.a1(), "B3", "and Enter moved down from it");
    }

    #[test]
    fn typing_goes_on_in_a_formula_that_points_at_another_sheet() {
        let mut h = pointing_at_sheet2();
        text(&mut h, "+");
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D6+"));
        let f9 = cell_pos(&h, "F9");
        click_at(&mut h, f9);
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D6+Sheet2!F9"));
        key(&mut h, Key::Enter);
        assert_eq!(draft(&h), None);
        assert_eq!(formula(&h, 0, "B2").as_deref(), Some("Sheet2!D6+Sheet2!F9"));
    }

    #[test]
    fn arrows_still_point_and_the_cell_shows_the_value() {
        let mut h = pointing_at_sheet2();
        // The edit began by typing in a cell, so an arrow moves what is pointed at (it does not
        // move the caret, as it would had the formula bar been clicked).
        key(&mut h, Key::ArrowDown);
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D7"));
        key(&mut h, Key::ArrowUp);
        key(&mut h, Key::Enter);
        let wb = &h.state().session.active().unwrap().wb;
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("B2").unwrap()).as_f64(), Some(40.0));
    }

    #[test]
    fn back_on_its_own_sheet_the_draft_still_has_its_keys() {
        let mut h = pointing_at_sheet2();
        text(&mut h, "+");
        let tab = tab_pos(&h, "Sheet1");
        click_at(&mut h, tab);
        assert_eq!(h.state().session.active().unwrap().wb.active_sheet(), 0);
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D6+"), "the draft is still open");
        text(&mut h, "1");
        assert_eq!(draft(&h).as_deref(), Some("=Sheet2!D6+1"));
        key(&mut h, Key::Enter);
        assert_eq!(draft(&h), None);
        assert_eq!(formula(&h, 0, "B2").as_deref(), Some("Sheet2!D6+1"));
    }

    #[test]
    fn esc_cancels_a_formula_that_points_at_another_sheet() {
        let mut h = pointing_at_sheet2();
        key(&mut h, Key::Escape);
        assert_eq!(draft(&h), None, "Esc dropped the draft");
        assert_eq!(formula(&h, 0, "B2"), None);
        assert_eq!(formula(&h, 1, "B2"), None);
    }
}
