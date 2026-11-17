//! [`Session::stand_in`]: the copy an interface shows while the session itself is away on
//! another thread running a command, and [`Session::adopt`], which makes that copy the session
//! when the real one is lost.

use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn number(s: &Session, cell: &str) -> f64 {
    s.doc().unwrap().wb.sheets[0].value(CellRef::parse(cell).unwrap()).as_f64().unwrap_or(f64::NAN)
}

/// Two workbooks; the first has a formula, a selection, a copied range and two undo steps.
fn session() -> Session {
    let mut s = Session::new();
    run(&mut s, "file.new", json!({}));
    run(&mut s, "file.new", json!({}));
    s.set_active(0);
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "4"}));
    run(&mut s, "cell.set", json!({"cell": "B1", "input": "=A1*10"}));
    run(&mut s, "selection.set", json!({"range": "A1:B1"}));
    run(&mut s, "edit.copy", json!({}));
    s
}

#[test]
fn a_stand_in_shows_what_the_session_shows() {
    let mut s = session();
    let copy = s.stand_in().unwrap();
    assert_eq!(copy.documents().len(), 2);
    assert_eq!(copy.active_index(), s.active_index());
    assert_eq!(copy.active_window(), s.active_window());
    assert_eq!(copy.windows().len(), s.windows().len());
    assert_eq!(number(&copy, "B1"), 40.0);
    let (a, b) = (s.doc().unwrap(), copy.doc().unwrap());
    assert_eq!(a.selection, b.selection);
    assert_eq!((a.uid, a.revision, &a.title), (b.uid, b.revision, &b.title));
    assert_eq!((a.undo.len(), a.redo.len()), (b.undo.len(), b.redo.len()));
    // Unsaved changes are told by identity (`DocState::is_dirty`): the copy shares the
    // workbooks, so it says of each what the session says.
    for (a, b) in s.documents().iter().zip(copy.documents()) {
        assert_eq!(a.is_dirty(), b.is_dirty());
    }
    assert!(copy.documents()[0].is_dirty() && !copy.documents()[1].is_dirty());
    assert_eq!(copy.clipboard.as_ref().map(|c| c.range), s.clipboard.as_ref().map(|c| c.range));

    // The session goes on without it: what the command changes, the copy does not see.
    run(&mut s, "cell.set", json!({"cell": "A1", "input": "5"}));
    assert_eq!((number(&s, "A1"), number(&s, "B1")), (5.0, 50.0));
    assert_eq!((number(&copy, "A1"), number(&copy, "B1")), (4.0, 40.0));
}

#[test]
fn a_query_on_the_stand_in_does_not_forget_a_request_to_stop_the_session() {
    let s = session();
    let mut copy = s.stand_in().unwrap();
    s.interrupt().request();
    // A command forgets the requests made before it started: on the copy that is the copy's own.
    run(&mut copy, "selection.set", json!({"range": "C3"}));
    assert!(s.interrupt().is_requested());
    assert!(!copy.interrupt().is_requested());
}

#[test]
fn an_adopted_stand_in_calculates_and_undoes() {
    let s = session();
    let mut copy = s.stand_in().unwrap();
    let handle = s.interrupt();
    drop(s);
    // It gets its dependency graphs, and listens to the handle others hold on the lost session.
    copy.adopt(Arc::clone(&handle));
    assert!(Arc::ptr_eq(&copy.interrupt(), &handle));
    run(&mut copy, "cell.set", json!({"cell": "A1", "input": "7"}));
    assert_eq!(number(&copy, "B1"), 70.0);
    run(&mut copy, "edit.undo", json!({}));
    assert_eq!((number(&copy, "A1"), number(&copy, "B1")), (4.0, 40.0));
    run(&mut copy, "edit.undo", json!({}));
    assert!(number(&copy, "B1").is_nan(), "the history came along");
}
