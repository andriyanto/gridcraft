//! Stopping a command while it calculates ([`Session::interrupt`]). A long recalculation ran to
//! its end whatever happened, with the thread that asked for it. It now stops at the next formula
//! when another thread asks, and the command fails with `EngineError::Cancelled`, leaving the
//! workbook, the selection, the undo history and the dependency graph as they were.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gridcraft_core::CellRef;
use gridcraft_engine::calc::Calc;
use gridcraft_engine::{EngineError, Session};
use serde_json::{Value as Json, json};

/// Twenty thousand numbers in column A, and three thousand formulas in column C that each
/// multiply all of them: a change to A1 recalculates for a good part of a second.
fn workbook() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    run(&mut s, "range.fill", json!({"range": "A1:A20000", "input": "2"}));
    run(&mut s, "range.fill", json!({"range": "C1:C3000", "input": "=SUMPRODUCT($A$1:$A$20000,$A$1:$A$20000)+ROW()"}));
    run(&mut s, "cell.set", json!({"cell": "E1", "input": "=SUM(C1:C3000)"}));
    run(&mut s, "cell.set", json!({"cell": "E2", "input": "10"}));
    run(&mut s, "cell.set", json!({"cell": "E3", "input": "=E2*2+C1"}));
    // Hiding rows recalculates this one, and through it column C.
    run(&mut s, "cell.set", json!({"cell": "A20000", "input": "=SUBTOTAL(109,E2)/5"}));
    s
}

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn number(s: &Session, cell: &str) -> f64 {
    s.doc().unwrap().wb.sheets[0].value(CellRef::parse(cell).unwrap()).as_f64().unwrap_or(f64::NAN)
}

/// Runs `id` while another thread keeps asking the session to stop.
///
/// The request is made over and over because a command forgets the requests made before it
/// started. The other thread is spinning before the command begins, and the recalculation takes
/// far longer than the thread could be kept waiting for a processor.
fn run_asked_to_stop(s: &mut Session, id: &str, params: Json) -> Result<Json, EngineError> {
    let interrupt = s.interrupt();
    let (spinning, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let asker = {
        let (spinning, done) = (Arc::clone(&spinning), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                interrupt.request();
                spinning.store(true, Ordering::Relaxed);
                std::hint::spin_loop();
            }
        })
    };
    while !spinning.load(Ordering::Relaxed) {
        std::hint::spin_loop();
    }
    let result = s.execute(id, params);
    done.store(true, Ordering::Relaxed);
    asker.join().unwrap();
    result
}

/// What differs from a workbook calculated and a graph built from scratch.
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
        for (c, cell) in x.cells.iter() {
            if cell.value != y.value(c) {
                out.push(format!("{}!{c}: {:?} in memory, {:?} after a full recalculation", x.name, cell.value, y.value(c)));
            }
            if d.calc.graph.precedents_of((si, c)) != calc.graph.precedents_of((si, c)) {
                out.push(format!("{}!{c}: reads other cells in the graph than after a rebuild", x.name));
            }
            // Who reads each cell, for the rows the edits touch (every formula of column C reads
            // every row of column A: listing them for all of it would take the test minutes).
            if c.row >= 16 && c.row != 19_999 {
                continue;
            }
            let (mut kept, mut built) = (Vec::new(), Vec::new());
            d.calc.graph.dependents(si, c, &mut kept);
            calc.graph.dependents(si, c, &mut built);
            for list in [&mut kept, &mut built] {
                list.sort_unstable_by_key(|(sheet, k)| (*sheet, k.row, k.col));
                list.dedup();
            }
            if kept != built {
                out.push(format!("{}!{c}: read by {} formulas in the graph, {} after a rebuild", x.name, kept.len(), built.len()));
            }
        }
    }
    out
}

#[test]
fn a_command_asked_to_stop_leaves_everything_as_it_was() {
    let mut s = workbook();
    run(&mut s, "selection.set", json!({"range": "B7"}));
    let before = s.doc().unwrap().clone();
    let total = number(&s, "E1");
    let steps: Vec<(&str, Json)> = vec![
        // A value every formula reads, typed; a formula entered; rows inserted (every formula of
        // the sheet is read again); F9; and Goal Seek, which calculates on copies.
        ("cell.set", json!({"cell": "A1", "input": "3"})),
        ("range.fill", json!({"range": "A5:A9", "input": "=ROW()"})),
        // Formulas replaced by values and by other formulas, and a value by a formula that reads
        // hidden rows: the graph is put back for each of them.
        ("range.setValues", json!({"range": "A10", "values": [[7, null, 5], [8, null, "=A1*2"], [9, null, "=SUBTOTAL(109,A1:A9)"]]})),
        ("edit.clearContents", json!({"range": "A1:A3"})),
        ("home.hideRows", json!({"rows": "4:6"})),
        ("home.insertRows", json!({"rows": "2:3"})),
        ("formulas.calculateNow", json!({})),
        ("data.goalSeek", json!({"set": "E3", "to": 500000, "changing": "A2"})),
    ];
    for (id, params) in steps {
        let result = run_asked_to_stop(&mut s, id, params);
        assert!(matches!(result, Err(EngineError::Cancelled)), "{id}: {result:?}");
        let d = s.doc().unwrap();
        assert!(Arc::ptr_eq(&d.wb, &before.wb), "{id}: the workbook is the one from before the command");
        assert_eq!((d.selection.active, d.undo.len(), d.redo.len()), (before.selection.active, before.undo.len(), before.redo.len()), "{id}");
        assert_eq!(stale(&s), Vec::<String>::new(), "{id}");
    }
    assert_eq!(number(&s, "E1"), total);
    // Nothing asks any more: the same edit goes through, on a graph that still leads everywhere.
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "3"}));
    assert_eq!(number(&s, "C1"), 80_006.0);
    assert_eq!(number(&s, "E1"), total + 3000.0 * 5.0);
    assert_eq!(s.doc().unwrap().undo.len(), before.undo.len() + 1);
    assert_eq!(stale(&s), Vec::<String>::new());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(number(&s, "E1"), total);
}

#[test]
fn a_request_made_between_commands_is_forgotten() {
    let mut s = workbook();
    s.interrupt().request();
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "3"}));
    assert_eq!(number(&s, "C1"), 80_006.0);
    // Workbooks opened later are stopped by the same handle.
    s.interrupt().request();
    run(&mut s, "file.new", json!({}));
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "=1+1"}));
    assert_eq!(number(&s, "A1"), 2.0);
    assert!(s.documents().iter().all(|d| d.calc.interrupt.is_some()));
}

/// Commands run inside the stopped one are undone with it: a macro's earlier steps, here the
/// first of a script's two edits.
#[test]
fn the_steps_a_stopped_command_already_made_are_taken_back() {
    let mut s = workbook();
    // The first step calculates nothing; the second recalculates every formula in column C.
    let commands = json!([
        {"command": "cell.set", "params": {"cell": "H1", "input": "first"}},
        {"command": "cell.set", "params": {"cell": "A1", "input": "3"}},
    ]);
    run(&mut s, "automate.newScript", json!({"name": "Two", "commands": commands}));
    let before = s.doc().unwrap().wb.clone();
    let undo = s.doc().unwrap().undo.len();
    let result = run_asked_to_stop(&mut s, "automate.runScript", json!({"name": "Two"}));
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    let d = s.doc().unwrap();
    assert!(Arc::ptr_eq(&d.wb, &before), "H1 is blank again");
    assert_eq!(d.undo.len(), undo);
    assert_eq!(stale(&s), Vec::<String>::new());
}
