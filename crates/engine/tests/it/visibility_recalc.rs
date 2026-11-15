//! Hiding, showing and filtering rows. These commands move no cell and change no formula, yet
//! they used to recalculate every formula in the workbook, because SUBTOTAL and AGGREGATE leave
//! hidden rows out. Only the formulas that call those two, and what reads them, are recalculated
//! now; every value must still be the one a full recalculation gives.

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use serde_json::{Value as Json, json};

/// Numbers 1 to 10 under a header in A1:A11, and formulas in column C that read them: with and
/// without hidden rows, through a dependent, nested in LET, and one that ignores hidden rows.
fn sheet() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    let mut values = vec![json!(["n", "kind"])];
    values.extend((1..=10).map(|n| json!([n, if n % 2 == 0 { "even" } else { "odd" }])));
    s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
    let formulas = [
        "=SUBTOTAL(109,A2:A11)",
        "=SUBTOTAL(9,A2:A11)",
        "=AGGREGATE(9,5,A2:A11)",
        "=C1*2",
        "=SUM(A2:A11)",
        "=LET(x,SUBTOTAL(103,A2:A11),x+100)",
        "=IF(C4>0,C3+C5,0)",
    ];
    for (i, f) in formulas.iter().enumerate() {
        s.execute("cell.set", json!({"cell": format!("C{}", i + 1), "input": f})).unwrap();
    }
    s
}

fn run(s: &mut Session, id: &str, params: Json) {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn number(s: &Session, cell: &str) -> f64 {
    s.doc().unwrap().wb.sheets[0].value(CellRef::parse(cell).unwrap()).as_f64().unwrap_or(f64::NAN)
}

/// The cells whose value in memory is not the one a full recalculation gives.
fn stale(s: &Session) -> Vec<String> {
    let wb = &s.doc().unwrap().wb;
    let mut fresh = (**wb).clone();
    Calc::new().recalc_all(&mut fresh);
    let mut out = Vec::new();
    for (x, y) in wb.sheets.iter().zip(&fresh.sheets) {
        for (c, cell) in x.cells.iter() {
            if cell.value != y.value(c) {
                out.push(format!("{}!{c}: {:?} in memory, {:?} after a full recalculation", x.name, cell.value, y.value(c)));
            }
        }
    }
    out
}

#[test]
fn hiding_and_showing_rows_updates_what_depends_on_hidden_rows() {
    let mut s = sheet();
    assert_eq!((number(&s, "C1"), number(&s, "C2"), number(&s, "C3")), (55.0, 55.0, 55.0));
    run(&mut s, "home.hideRows", json!({"rows": "3:4"}));
    // Rows 3 and 4 hold 2 and 3.
    assert_eq!((number(&s, "C1"), number(&s, "C2"), number(&s, "C3")), (50.0, 55.0, 50.0), "109 and AGGREGATE leave hidden rows out, 9 doesn't");
    assert_eq!((number(&s, "C4"), number(&s, "C6"), number(&s, "C7")), (100.0, 108.0, 105.0), "and what reads them follows");
    assert_eq!(stale(&s), Vec::<String>::new());
    run(&mut s, "home.unhideRows", json!({"rows": "2:5"}));
    assert_eq!((number(&s, "C1"), number(&s, "C4"), number(&s, "C6")), (55.0, 110.0, 110.0));
    assert_eq!(stale(&s), Vec::<String>::new());
}

#[test]
fn every_command_that_shows_or_hides_rows_leaves_no_stale_value() {
    let mut s = sheet();
    let steps: Vec<(&str, Json, f64)> = vec![
        ("home.rowHeight", json!({"rows": "5:5", "height": 0}), 51.0),
        ("home.rowHeight", json!({"rows": "5:5", "height": 24}), 55.0),
        ("home.hideRows", json!({"rows": "6:7"}), 44.0),
        ("home.autofitRowHeight", json!({"rows": "6:7"}), 55.0),
        ("data.group", json!({"rows": "8:9"}), 55.0),
        ("data.hideDetail", json!({"rows": "8:9"}), 40.0),
        ("data.showDetail", json!({"rows": "8:9"}), 55.0),
        ("data.hideDetail", json!({"rows": "8:9"}), 40.0),
        ("data.ungroup", json!({"rows": "8:9"}), 55.0),
        ("data.group", json!({"rows": "2:4"}), 55.0),
        ("data.showLevels", json!({"rows": 1}), 49.0),
        ("data.showLevels", json!({"rows": 2}), 55.0),
        ("data.filter", json!({"range": "A1:B11"}), 55.0),
        ("data.filterBy", json!({"column": "B", "values": ["even"]}), 30.0),
        ("data.clearFilter", json!({}), 55.0),
        ("data.filterBy", json!({"column": "B", "values": ["odd"]}), 25.0),
        ("data.reapply", json!({}), 25.0),
        ("data.filter", json!({}), 55.0),
    ];
    for (i, (id, params, visible_sum)) in steps.into_iter().enumerate() {
        run(&mut s, id, params);
        assert_eq!(number(&s, "C1"), visible_sum, "step {i}, {id}: the sum of the visible rows");
        assert_eq!(number(&s, "C4"), visible_sum * 2.0, "step {i}, {id}: what reads it");
        assert_eq!(number(&s, "C5"), 55.0, "step {i}, {id}: SUM reads every row");
        assert_eq!(stale(&s), Vec::<String>::new(), "step {i}, {id}");
    }
}

#[test]
fn a_filter_leaves_the_rows_it_hides_out_of_a_plain_subtotal_too() {
    let mut s = sheet();
    run(&mut s, "data.filter", json!({"range": "A1:B11"}));
    run(&mut s, "data.filterBy", json!({"column": "B", "values": ["even"]}));
    assert_eq!((number(&s, "C2"), number(&s, "C3"), number(&s, "C6")), (30.0, 30.0, 105.0));
    assert_eq!(stale(&s), Vec::<String>::new());
}

#[test]
fn hiding_a_row_calculates_only_what_depends_on_hidden_rows() {
    let mut s = sheet();
    // Five hundred formulas that have nothing to do with hidden rows.
    run(&mut s, "selection.set", json!({"range": "E1:E500"}));
    run(&mut s, "range.fill", json!({"input": "=ROW()*2+$A$2"}));
    run(&mut s, "home.hideRows", json!({"rows": "3:4"}));
    let calculated = s.doc().unwrap().calc.last_recalc_cells;
    // C1, C2, C3 and C6 call SUBTOTAL or AGGREGATE; C4 and C7 read them.
    assert!((4..=8).contains(&calculated), "{calculated} formulas calculated for two hidden rows");
    assert_eq!(number(&s, "C1"), 50.0);
    assert_eq!(stale(&s), Vec::<String>::new());
}

#[test]
fn formulas_entered_removed_and_brought_back_are_followed() {
    let mut s = sheet();
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=SUBTOTAL(102,A2:A11)"}));
    run(&mut s, "cell.set", json!({"cell": "C3", "input": "7"}));
    run(&mut s, "home.hideRows", json!({"rows": "3:4"}));
    assert_eq!((number(&s, "D1"), number(&s, "C3")), (8.0, 7.0), "the new formula follows; the cell that lost its formula keeps its number");
    assert_eq!(stale(&s), Vec::<String>::new());
    // Undo puts cells back without calculating them; the next hide still finds C3's formula.
    for _ in 0..2 {
        run(&mut s, "edit.undo", json!({}));
    }
    assert_eq!(number(&s, "C3"), 55.0, "the hidden rows are back, and so is AGGREGATE in C3");
    run(&mut s, "home.hideRows", json!({"rows": "10:11"}));
    assert_eq!((number(&s, "C3"), number(&s, "D1")), (36.0, 8.0));
    assert_eq!(stale(&s), Vec::<String>::new());
}

#[test]
fn manual_calculation_still_recalculates_everything_when_rows_are_hidden() {
    let mut s = sheet();
    run(&mut s, "formulas.calculationOptions", json!({"mode": "manual"}));
    run(&mut s, "cell.set", json!({"cell": "A2", "input": "101"}));
    assert_eq!(number(&s, "C5"), 55.0, "manual: SUM waits");
    run(&mut s, "home.hideRows", json!({"rows": "3:4"}));
    assert_eq!((number(&s, "C5"), number(&s, "C1")), (155.0, 150.0), "hiding rows recalculates the workbook, as it did");
}
