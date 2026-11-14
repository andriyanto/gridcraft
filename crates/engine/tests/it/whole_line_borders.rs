//! Borders on whole rows and whole columns. `home.borders` used to create a cell for every
//! position (1,048,576 for one column, 16,384 for one row), so a single border could use up the
//! undo history's memory budget. The lines' own format takes the border now, and the cells that
//! must exist are the only ones created.

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use gridcraft_engine::io;
use gridcraft_model::{Fill, Sheet, Style, Workbook};
use serde_json::{Value as Json, json};

/// The `budget` sample (61 cells in B2:F15) after `steps`, each of which must work.
fn budget(steps: &[(&str, Json)]) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "budget"})).unwrap();
    for (id, params) in steps {
        s.execute(id, params.clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    s
}

fn borders(preset: &str, range: &str) -> (&'static str, Json) {
    ("home.borders", json!({"preset": preset, "range": range}))
}

fn wb(s: &Session) -> &Workbook {
    &s.doc().unwrap().wb
}

fn sheet(s: &Session) -> &Sheet {
    &wb(s).sheets[0]
}

fn style_in(wb: &Workbook, cell: &str) -> Style {
    wb.styles.get(wb.sheets[0].style_id(CellRef::parse(cell).unwrap())).clone()
}

/// The sides of `cell` that have a border, as letters: `T`, `B`, `L`, `R`.
fn sides_in(wb: &Workbook, cell: &str) -> String {
    let b = style_in(wb, cell).borders;
    [("T", b.top), ("B", b.bottom), ("L", b.left), ("R", b.right)].iter().filter(|(_, line)| !line.is_none()).map(|(side, _)| *side).collect()
}

fn sides(s: &Session, cell: &str) -> String {
    sides_in(wb(s), cell)
}

fn new_cells(s: &Session) -> usize {
    sheet(s).cells.len() - 61
}

#[test]
fn a_border_around_a_whole_column_formats_the_column_not_a_million_cells() {
    let s = budget(&[borders("outside", "I:I")]);
    assert!(new_cells(&s) <= 2, "{} cells were created", new_cells(&s));
    assert_eq!(sides(&s, "I1"), "TLR", "the top of the column");
    assert_eq!(sides(&s, "I5"), "LR");
    assert_eq!(sides(&s, "I500000"), "LR");
    assert_eq!(sides(&s, "I1048576"), "BLR", "the bottom of the column");
    assert_eq!((sides(&s, "H5"), sides(&s, "J5")), (String::new(), String::new()), "the columns beside it are untouched");
}

#[test]
fn borders_on_whole_rows_format_the_rows_not_every_cell() {
    let s = budget(&[borders("all", "19:21")]);
    assert_eq!(new_cells(&s), 0);
    for cell in ["A19", "Z20", "XFD21"] {
        assert_eq!(sides(&s, cell), "TBLR", "{cell}");
    }
    assert_eq!((sides(&s, "Z18"), sides(&s, "Z22")), (String::new(), String::new()));

    let s = budget(&[borders("outside", "19:21")]);
    assert!(new_cells(&s) <= 6, "{} cells were created", new_cells(&s));
    let expect =
        [("A19", "TL"), ("Z19", "T"), ("XFD19", "TR"), ("A20", "L"), ("Z20", ""), ("XFD20", "R"), ("A21", "BL"), ("Z21", "B"), ("XFD21", "BR")];
    for (cell, want) in expect {
        assert_eq!(sides(&s, cell), want, "{cell}");
    }
}

#[test]
fn a_whole_line_border_keeps_the_formats_it_crosses() {
    // An empty cell shows its row's format before its column's, so where the two cross, the cell
    // has to carry both.
    let s = budget(&[("home.bold", json!({"range": "19:19"})), borders("all", "I:I")]);
    assert!(style_in(wb(&s), "I19").font.bold && sides(&s, "I19") == "TBLR", "bold row, then a bordered column");
    assert!(style_in(wb(&s), "H19").font.bold && sides(&s, "H19").is_empty());
    assert!(new_cells(&s) <= 1, "{} cells were created", new_cells(&s));

    let s = budget(&[("home.fillColor", json!({"range": "I:I", "color": "#FFFF00"})), borders("bottom", "19:19")]);
    assert!(style_in(wb(&s), "I19").fill != Fill::default() && sides(&s, "I19") == "B", "yellow column, then a bordered row");
    assert_eq!(sides(&s, "A19"), "B");
    assert!(style_in(wb(&s), "I18").fill != Fill::default() && sides(&s, "I18").is_empty());
    assert!(new_cells(&s) <= 1, "{} cells were created", new_cells(&s));
}

/// Other formats of whole lines go the same way. A row's format used to hide the column's (and a
/// later column format never reached a formatted row) wherever the two crossed on an empty cell.
#[test]
fn a_format_on_a_whole_line_keeps_the_formats_it_crosses() {
    let yellow = ("home.fillColor", json!({"range": "I:I", "color": "#FFFF00"}));
    let bold = ("home.bold", json!({"range": "19:19"}));
    for steps in [[yellow.clone(), bold.clone()], [bold, yellow.clone()]] {
        let s = budget(&steps);
        let crossing = style_in(wb(&s), "I19");
        assert!(
            crossing.font.bold && crossing.fill != Fill::default(),
            "I19 is bold and yellow after {:?}",
            steps.iter().map(|x| x.0).collect::<Vec<_>>()
        );
        assert!(style_in(wb(&s), "H19").font.bold && style_in(wb(&s), "H19").fill == Fill::default());
        assert!(!style_in(wb(&s), "I18").font.bold && style_in(wb(&s), "I18").fill != Fill::default());
        assert!(new_cells(&s) <= 1, "{} cells were created", new_cells(&s));
    }

    let s = budget(&[borders("all", "18:19"), yellow]);
    assert!(style_in(wb(&s), "I18").fill != Fill::default() && sides(&s, "I18") == "TBLR", "bordered rows, then a yellow column");
    assert!(new_cells(&s) <= 2, "{} cells were created", new_cells(&s));
}

#[test]
fn a_whole_line_border_reaches_the_cells_that_exist() {
    let plain = budget(&[]);
    let s = budget(&[borders("all", "5:5")]);
    let (before, after) = (style_in(wb(&plain), "C5"), style_in(wb(&s), "C5"));
    assert_eq!(sides(&s, "C5"), "TBLR");
    assert_eq!((after.num_fmt, after.font), (before.num_fmt, before.font), "the cell keeps the rest of its format");
    assert_eq!(new_cells(&s), 0);
}

#[test]
fn no_border_on_whole_lines_clears_the_edges_that_face_them() {
    let s = budget(&[borders("all", "H:I"), borders("none", "I:I")]);
    assert_eq!((sides(&s, "H5"), sides(&s, "I5"), sides(&s, "J5")), ("TBL".to_string(), String::new(), String::new()));

    let s = budget(&[borders("all", "19:20"), borders("none", "20:20")]);
    assert_eq!((sides(&s, "Z19"), sides(&s, "Z20"), sides(&s, "Z21")), ("TLR".to_string(), String::new(), String::new()));
    assert_eq!(sides(&s, "C19"), "TLR");
}

#[test]
fn the_whole_sheet_takes_a_border() {
    let s = budget(&[borders("all", "A1:XFD1048576")]);
    assert_eq!(new_cells(&s), 0);
    for cell in ["A1", "C5", "Z99", "XFD1048576"] {
        assert_eq!(sides(&s, cell), "TBLR", "{cell}");
    }
}

#[test]
fn a_whole_column_border_survives_a_save_and_undo_takes_it_away() {
    let mut s = budget(&[borders("outside", "I:I")]);
    let bytes = io::save_bytes(wb(&s), "book.xlsx").unwrap();
    let (back, _) = io::open_bytes("book.xlsx", &bytes).unwrap();
    for (cell, want) in [("I1", "TLR"), ("I5", "LR"), ("I1048576", "BLR"), ("H5", "")] {
        assert_eq!(sides_in(&back, cell), want, "{cell} after a save and reload");
    }
    assert!(back.sheets[0].cells.len() <= 63, "{} cells after a save and reload", back.sheets[0].cells.len());

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((sides(&s, "I1"), sides(&s, "I5")), (String::new(), String::new()));
    assert_eq!(new_cells(&s), 0);
}
