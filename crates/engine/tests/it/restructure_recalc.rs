//! Inserting and deleting rows, columns and cells. These commands recalculated every formula in
//! the workbook. They now recalculate the sheet whose cells moved, the formulas on other sheets
//! that read that sheet, and what reads those; a formula that reads none of it did not move and
//! reads the same cells, so it keeps its value. Every value must still be the one a full
//! recalculation gives, and the dependency graph the one a rebuild gives.

use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use serde_json::{Value as Json, json};

/// Three sheets: `Data` (numbers 1 to 10 in A1:A10 and their doubles in B, a spilled array in
/// D1:D3), `Report`, which reads Data in every way a formula can, and `Other`, three hundred
/// formulas that read neither.
fn workbook() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    run(&mut s, "sheet.rename", json!({"name": "Data"}));
    run(&mut s, "home.insertSheet", json!({"name": "Report", "before": 1}));
    run(&mut s, "home.insertSheet", json!({"name": "Other", "before": 2}));
    let rows: Vec<Json> = (1..=10).map(|n| json!([n, format!("=A{n}*2")])).collect();
    run(&mut s, "range.setValues", json!({"sheet": 0, "range": "A1", "values": rows}));
    run(&mut s, "cell.set", json!({"sheet": 0, "cell": "D1", "input": "=SEQUENCE(3)*10"}));
    let report = [
        "=SUM(Data!A1:A10)",
        "=Data!B7",
        "=ROW(Data!A7)",
        "=SUM(Data!A:A)",
        "=SUM(Data!D1#)",
        "=A2+1",
        "=FORMULATEXT(A2)",
        "=INDIRECT(\"Data!A3\")",
        "=SUBTOTAL(109,Data!A1:A10)",
        "=COUNTA(Data!1:1)",
        "=Data!A2:A4",
    ];
    for (i, f) in report.iter().enumerate() {
        run(&mut s, "cell.set", json!({"sheet": 1, "cell": format!("A{}", i + 1), "input": f}));
    }
    run(&mut s, "range.setValues", json!({"sheet": 2, "range": "A1", "values": [[1, 2], [3, 4]]}));
    run(&mut s, "range.fill", json!({"sheet": 2, "range": "D1:D300", "input": "=ROW()+$A$1+$B$2"}));
    run(&mut s, "cell.set", json!({"sheet": 2, "cell": "F1", "input": "=SUM(D1:D300)"}));
    s
}

fn run(s: &mut Session, id: &str, params: Json) {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn value(s: &Session, sheet: usize, cell: &str) -> Value {
    s.doc().unwrap().wb.sheets[sheet].value(CellRef::parse(cell).unwrap())
}

fn number(s: &Session, sheet: usize, cell: &str) -> f64 {
    value(s, sheet, cell).as_f64().unwrap_or(f64::NAN)
}

/// What a full recalculation or a rebuild of the graph gives differently: values, the formulas
/// each formula reads, and the formulas that read each cell.
fn stale(s: &Session) -> Vec<String> {
    let d = s.doc().unwrap();
    let mut fresh = (*d.wb).clone();
    let mut calc = Calc::new();
    calc.recalc_all(&mut fresh);
    let mut out = Vec::new();
    if d.calc.graph.len() != calc.graph.len() {
        out.push(format!("{} formulas in the graph, {} after a rebuild", d.calc.graph.len(), calc.graph.len()));
    }
    for (si, (x, y)) in d.wb.sheets.iter().zip(&fresh.sheets).enumerate() {
        let mut cells: Vec<CellRef> = x.cells.iter().map(|(c, _)| c).chain(x.spill.keys().copied()).chain(y.spill.keys().copied()).collect();
        cells.sort();
        cells.dedup();
        for c in cells {
            if x.value(c) != y.value(c) {
                out.push(format!("{}!{c}: {:?} in memory, {:?} after a full recalculation", x.name, x.value(c), y.value(c)));
            }
            if d.calc.graph.precedents_of((si, c)) != calc.graph.precedents_of((si, c)) {
                out.push(format!("{}!{c}: reads other cells in the graph than after a rebuild", x.name));
            }
            let (mut kept, mut built) = (Vec::new(), Vec::new());
            d.calc.graph.dependents(si, c, &mut kept);
            calc.graph.dependents(si, c, &mut built);
            for list in [&mut kept, &mut built] {
                list.sort_unstable_by_key(|(sheet, k)| (*sheet, k.row, k.col));
                list.dedup();
            }
            if kept != built {
                out.push(format!("{}!{c}: read by {kept:?} in the graph, {built:?} after a rebuild", x.name));
            }
        }
    }
    out
}

#[test]
fn a_row_inserted_on_one_sheet_leaves_an_unrelated_sheet_alone() {
    let mut s = workbook();
    run(&mut s, "home.insertRows", json!({"sheet": 0, "rows": "5:5"}));
    let calculated = s.doc().unwrap().calc.last_recalc_cells;
    // Data has 11 formulas and Report 11; Other's 301 read neither sheet.
    assert!((22..=40).contains(&calculated), "{calculated} formulas calculated");
    assert_eq!(number(&s, 2, "F1"), (1..=300).map(|row| row as f64 + 5.0).sum::<f64>());
    // The references followed the cells: B7 is B8 now, and its row number is 8.
    assert_eq!((number(&s, 1, "A1"), number(&s, 1, "A2"), number(&s, 1, "A3")), (55.0, 14.0, 8.0));
    assert_eq!(value(&s, 1, "A7"), Value::from("=Data!B8"), "FORMULATEXT reads a formula on its own sheet that was rewritten");
    assert_eq!(number(&s, 1, "A6"), 15.0);
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// Once as the commands run by themselves (the edit of `Other`, which holds most of the formulas,
/// recalculates everything), and once with the graph updated formula by formula on every sheet.
#[test]
fn every_command_that_moves_cells_leaves_no_stale_value() {
    moving_cells_leaves_no_stale_value(false);
    moving_cells_leaves_no_stale_value(true);
}

fn moving_cells_leaves_no_stale_value(always_scoped: bool) {
    let mut s = workbook();
    s.doc_mut().unwrap().calc.always_scoped = always_scoped;
    let steps: Vec<(&str, Json)> = vec![
        ("home.insertRows", json!({"sheet": 0, "rows": "5:6"})),
        ("home.deleteRows", json!({"sheet": 0, "rows": "5:5"})),
        ("home.insertColumns", json!({"sheet": 0, "cols": "B:B"})),
        ("home.deleteColumns", json!({"sheet": 0, "cols": "B:B"})),
        ("home.insertCells", json!({"sheet": 0, "range": "A3:B3", "shift": "down"})),
        ("home.deleteCells", json!({"sheet": 0, "range": "A3:B3", "shift": "up"})),
        ("home.insertCells", json!({"sheet": 0, "range": "A2:A3", "shift": "right"})),
        ("home.deleteCells", json!({"sheet": 0, "range": "A2:A3", "shift": "left"})),
        ("sheet.moveColumns", json!({"sheet": 0, "cols": "B:B", "before": 0})),
        // Above the spilled array, which moves down with its formula.
        ("home.insertRows", json!({"sheet": 0, "rows": "1:1"})),
        // The row Report!A2 reads by now (Data!A9): the reference becomes #REF!.
        ("home.deleteRows", json!({"sheet": 0, "rows": "9:9"})),
        // A row on the sheet that reads, and on the one that doesn't.
        ("home.insertRows", json!({"sheet": 1, "rows": "3:3"})),
        ("home.deleteColumns", json!({"sheet": 2, "cols": "A:A"})),
    ];
    for (i, (id, params)) in steps.into_iter().enumerate() {
        run(&mut s, id, params);
        assert_eq!(stale(&s), Vec::<String>::new(), "step {i}, {id} (always scoped: {always_scoped})");
    }
    assert_eq!(value(&s, 1, "A2"), Value::Error(gridcraft_core::CellError::Ref));
    assert_eq!(value(&s, 2, "E1"), Value::Error(gridcraft_core::CellError::Ref), "Other!F1, moved left, read the deleted column through D");
}

#[test]
fn the_graph_still_leads_to_every_reader_afterwards() {
    let mut s = workbook();
    run(&mut s, "home.insertRows", json!({"sheet": 0, "rows": "5:5"}));
    run(&mut s, "home.deleteColumns", json!({"sheet": 0, "cols": "C:C"}));
    // A value typed on the sheet that moved reaches the formulas that read it from another sheet,
    run(&mut s, "cell.set", json!({"sheet": 0, "cell": "A8", "input": "100"}));
    assert_eq!((number(&s, 1, "A1"), number(&s, 1, "A2"), number(&s, 1, "A6")), (148.0, 200.0, 201.0));
    // hiding rows there reaches SUBTOTAL,
    run(&mut s, "home.hideRows", json!({"sheet": 0, "rows": "8:8"}));
    assert_eq!(number(&s, 1, "A9"), 48.0);
    // and a value typed on the sheet that was left alone reaches its own formulas.
    run(&mut s, "cell.set", json!({"sheet": 2, "cell": "A1", "input": "11"}));
    assert_eq!(number(&s, 2, "D1"), 16.0);
    assert_eq!(stale(&s), Vec::<String>::new());
}

#[test]
fn a_loop_on_another_sheet_stays_listed() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"sheet": 2, "cell": "H1", "input": "=H2+1"}));
    run(&mut s, "cell.set", json!({"sheet": 2, "cell": "H2", "input": "=H1+1"}));
    let loops = |s: &Session| s.doc().unwrap().calc.circular.len();
    assert_eq!(loops(&s), 1);
    run(&mut s, "home.insertRows", json!({"sheet": 0, "rows": "2:2"}));
    assert_eq!(loops(&s), 1, "a loop the insert has nothing to do with");
    run(&mut s, "home.insertRows", json!({"sheet": 2, "rows": "1:1"}));
    assert_eq!(loops(&s), 1, "the loop moved down a row with its formulas");
    assert_eq!(value(&s, 2, "H2"), Value::Error(gridcraft_core::CellError::Circ));
    assert_eq!(stale(&s), Vec::<String>::new());
}
