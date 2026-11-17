//! Commands that run off the interface thread ([`gridcraft_ui_egui::background`]).
//!
//! A command that calculated for long held the window with it: nothing was painted, and nothing
//! could be clicked, Cancel least of all. With the background on, the window goes on showing the
//! workbook as it was, under a box that offers Cancel, and takes the command's result when it is
//! over. Tests hold the worker back with [`Config::hold`], so "the command is still running" is
//! a fact and not a matter of timing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::common::Harness;
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::background::Config;
use serde_json::json;

/// A1 = 1 and B1 = A1 * 2, with the background on and the worker held back by the flag.
fn held() -> (Harness, Arc<AtomicBool>) {
    let mut session = crate::support::blank_session();
    session.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    session.execute("cell.set", json!({"cell": "B1", "input": "=A1*2"})).unwrap();
    let hold = Arc::new(AtomicBool::new(true));
    let mut h = crate::support::harness(session, 4);
    h.state_mut().background = Some(Config { wait: Duration::ZERO, hold: Some(Arc::clone(&hold)) });
    (h, hold)
}

fn text(session: &Session, cell: &str) -> String {
    let d = session.active().unwrap();
    d.wb.active().unwrap().cell(CellRef::parse(cell).unwrap()).map(|c| format!("{:?}", c.value)).unwrap_or_else(|| "empty".into())
}

fn cell(app: &SheetApp, cell: &str) -> String {
    text(&app.session, cell)
}

fn active_cell(app: &SheetApp) -> String {
    app.session.active().unwrap().selection.active.a1()
}

/// Frames until the app is no longer busy.
fn until_idle(h: &mut Harness) {
    for _ in 0..5000 {
        h.run_steps(1);
        if !h.state().is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the command never came back");
}

fn shows(h: &Harness, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

#[test]
fn a_quick_command_runs_on_the_worker_and_is_over_within_the_call() {
    let mut h = crate::support::blank_harness(4);
    h.state_mut().background = Some(Config::default());
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "B1", "input": "=A1*2"}));
    assert!(!h.state().is_busy());
    assert_eq!(cell(h.state(), "B1"), "Number(10.0)");
    assert_eq!(h.state().background_runs(), 2, "both ran on the worker");
    // Moving the selection changes no workbook: it runs where it is asked for.
    h.state_mut().run_or_alert("selection.move", json!({"dr": 1, "dc": 0}));
    assert_eq!(h.state().background_runs(), 2);
    assert_eq!(active_cell(h.state()), "A2");
    // The session that came back is the one with the history.
    h.state_mut().run_or_alert("edit.undo", json!({}));
    assert_eq!(h.state().background_runs(), 3);
    assert_eq!(cell(h.state(), "B1"), "empty");
    h.run_steps(2);
    assert!(!shows(&h, "Calculating…"));
}

#[test]
fn with_the_background_off_nothing_runs_on_the_worker() {
    let mut h = crate::support::blank_harness(4);
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    assert_eq!(h.state().background_runs(), 0);
}

#[test]
fn a_slow_command_leaves_the_window_painting_what_was_there() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    // The window is alive: it paints, it shows the workbook as it was and says what it waits for.
    h.run_steps(3);
    assert!(h.state().is_busy());
    assert_eq!(cell(h.state(), "A1"), "Number(1.0)");
    assert_eq!(cell(h.state(), "B1"), "Number(2.0)");
    assert!(shows(&h, "Calculating…"));
    // What is asked for meanwhile waits its turn, in order.
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A2", "input": "=A1+B1"}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "7"}));
    h.run_steps(1);
    assert_eq!(cell(h.state(), "A2"), "empty");

    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "A1"), "Number(7.0)");
    assert_eq!(cell(h.state(), "B1"), "Number(14.0)");
    assert_eq!(cell(h.state(), "A2"), "Number(21.0)");
    assert_eq!(h.state().background_runs(), 3);
    h.run_steps(2);
    assert!(!shows(&h, "Calculating…"));
    // Each is its own step back. (With the usual wait, so that each undo is over within its call.)
    h.state_mut().background = Some(Config::default());
    h.state_mut().run_or_alert("edit.undo", json!({}));
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    h.state_mut().run_or_alert("edit.undo", json!({}));
    h.state_mut().run_or_alert("edit.undo", json!({}));
    assert_eq!((cell(h.state(), "A1"), cell(h.state(), "A2")), ("Number(1.0)".to_string(), "empty".to_string()));
}

#[test]
fn a_command_run_for_its_result_waits_for_the_busy_one() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        hold.store(false, Ordering::Relaxed);
    });
    // Its caller goes on with the result, so it can't wait its turn in the queue: it runs
    // after the busy command, in the session that command left.
    let r = h.state_mut().run("cell.set", json!({"cell": "A2", "input": "=A1+1"}));
    release.join().unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(!h.state().is_busy());
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    assert_eq!(cell(h.state(), "A2"), "Number(6.0)");
}

#[test]
fn the_interface_s_own_engine_calls_wait_too() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        hold.store(false, Ordering::Relaxed);
    });
    // A change made to the stand-in would be lost with it.
    let r = h.state_mut().engine("cell.set", json!({"cell": "C1", "input": "kept"}));
    release.join().unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(!h.state().is_busy());
    assert_eq!(cell(h.state(), "C1"), "Text(\"kept\")");
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
}

#[test]
fn a_cell_entry_moves_the_selection_when_it_is_over() {
    let (mut h, hold) = held();
    h.state_mut().begin_edit(Some("5".into()), false);
    assert!(h.state_mut().commit_edit_from_keys(1, 0, false, false, false));
    assert!(h.state().is_busy());
    assert!(h.state().editor.is_none(), "the draft is on its way");
    h.run_steps(2);
    assert_eq!(active_cell(h.state()), "A1", "the selection waits for the entry");
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    assert_eq!(cell(h.state(), "B1"), "Number(10.0)");
    assert_eq!(active_cell(h.state()), "A2", "Enter moved down");
}

#[test]
fn a_refused_cell_entry_opens_its_draft_again() {
    let (mut h, hold) = held();
    // C1:C2 holds one array formula: a part of it can't be changed.
    h.state_mut().run("selection.set", json!({"range": "C1:C2"})).unwrap();
    h.state_mut().run("cell.set", json!({"cell": "C1", "input": "=A1:A2", "array": true})).unwrap();
    h.state_mut().run("selection.set", json!({"range": "C2"})).unwrap();
    let before = cell(h.state(), "C2");
    h.state_mut().begin_edit(Some("9".into()), false);
    assert!(h.state_mut().commit_edit_from_keys(1, 0, false, false, false));
    assert!(h.state().is_busy());
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "C2"), before);
    assert!(h.state().message.is_some(), "the reason is shown");
    let draft = h.state().editor.as_ref().map(|ed| ed.text.clone());
    assert_eq!(draft.as_deref(), Some("9"), "what was typed is not lost");
    assert_eq!(h.state().session.mode, gridcraft_engine::Mode::Edit);
    assert_eq!(active_cell(h.state()), "C2", "the selection stays with the draft");
}

#[test]
fn keys_wait_while_busy_and_esc_asks_to_stop() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    h.run_steps(1);
    let ctx = h.ctx.clone();
    let key = |key, pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    let mut raw = egui::RawInput {
        events: vec![
            key(egui::Key::Delete, true),
            key(egui::Key::Delete, false),
            egui::Event::Text("x".into()),
            egui::Event::Paste("pasted".into()),
            egui::Event::PointerMoved(egui::pos2(300.0, 300.0)),
        ],
        ..Default::default()
    };
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    // A key that went down before the command must still be seen going up; the pointer reaches
    // the box, which stops it.
    assert_eq!(raw.events.len(), 2, "{:?}", raw.events);
    assert!(matches!(raw.events[0], egui::Event::Key { pressed: false, .. }));
    assert!(matches!(raw.events[1], egui::Event::PointerMoved(_)));
    h.run_steps(1);
    assert!(shows(&h, "Calculating…"));

    let mut raw = egui::RawInput { events: vec![key(egui::Key::Escape, true)], ..Default::default() };
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    assert!(raw.events.is_empty());
    h.run_steps(1);
    assert!(shows(&h, "Cancelling…"));
    assert!(!shows(&h, "Calculating…"));

    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    // Once idle the hook leaves keys alone.
    let mut raw = egui::RawInput { events: vec![key(egui::Key::Delete, true)], ..Default::default() };
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    assert_eq!(raw.events.len(), 1);
}

/// Twenty thousand numbers in column A and two thousand formulas in column C that each multiply
/// all of them: a change to A1 calculates for many frames, long enough to be stopped.
fn heavy() -> Session {
    let mut session = crate::support::blank_session();
    let mut run = |id: &str, params| session.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
    run("range.fill", json!({"range": "A1:A20000", "input": "2"}));
    run("range.fill", json!({"range": "C1:C2000", "input": "=SUMPRODUCT($A$1:$A$20000,$A$1:$A$20000)+ROW()"}));
    run("cell.set", json!({"cell": "E1", "input": "=SUM(C1:C2000)"}));
    session
}

#[test]
fn cancel_stops_the_calculation_and_keeps_the_workbook() {
    let session = heavy();
    let (c1, e1) = (text(&session, "C1"), text(&session, "E1"));
    let mut h = crate::support::harness(session, 4);
    h.state_mut().background = Some(Config { wait: Duration::ZERO, hold: None });
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "3"}));
    assert!(h.state().is_busy(), "the calculation takes longer than no time at all");
    h.state_mut().run_or_alert("cell.set", json!({"cell": "G1", "input": "asked for behind it"}));
    // A command forgets what was asked before it began, and the worker may not have begun: the
    // request is made until the command is back.
    for _ in 0..100_000 {
        h.state_mut().cancel_background();
        h.run_steps(1);
        if !h.state().is_busy() {
            break;
        }
    }
    assert!(!h.state().is_busy());
    assert_eq!(cell(h.state(), "A1"), "Number(2.0)", "the entry was taken back");
    assert_eq!((cell(h.state(), "C1"), cell(h.state(), "E1")), (c1, e1));
    assert_eq!(h.state().toast.as_ref().map(|(text, _)| text.as_str()), Some("Cancelled"));
    assert!(h.state().message.is_none(), "stopping is not an error to explain");
    assert_eq!(cell(h.state(), "G1"), "empty", "Cancel stops what waited behind the command too");
    // Nothing is left over: the same entry goes through, and is one step back.
    h.state_mut().background = Some(Config::default());
    h.state_mut().run("cell.set", json!({"cell": "A1", "input": "3"})).unwrap();
    assert_eq!(cell(h.state(), "C1"), "Number(80006.0)");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(cell(h.state(), "C1"), "Number(80001.0)");
}

#[test]
fn a_failure_is_shown_and_the_next_command_runs_as_it_always_did() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("edit.undo", json!({}));
    assert!(h.state().is_busy());
    // The second fails (that is no cell). The third runs all the same, as it does when each
    // runs where it is asked for: the background changes when, not what.
    h.state_mut().run_or_alert("cell.set", json!({"cell": "not a cell", "input": "1"}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "D1", "input": "after the failure"}));
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "B1"), "empty", "the undo ran");
    assert!(h.state().message.is_some(), "the failure is shown");
    assert_eq!(cell(h.state(), "D1"), "Text(\"after the failure\")");

    // The same three with every command run here.
    let (mut h, _) = held();
    h.state_mut().background = None;
    h.state_mut().run_or_alert("edit.undo", json!({}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "not a cell", "input": "1"}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "D1", "input": "after the failure"}));
    assert!(h.state().message.is_some());
    assert_eq!((cell(h.state(), "B1"), cell(h.state(), "D1")), ("empty".to_string(), "Text(\"after the failure\")".to_string()));
}

#[test]
fn clearing_the_copy_marquee_while_busy_is_kept() {
    let (mut h, hold) = held();
    h.state_mut().run("edit.copy", json!({})).unwrap();
    assert!(h.state().session.clipboard.is_some());
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    // As the grid does after Enter pastes: it lets go of the copied cells itself.
    h.state_mut().session.clipboard = None;
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert!(h.state().session.clipboard.is_none());
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
}

#[test]
fn the_close_button_waits_for_the_command() {
    // A new workbook, with nothing to save yet.
    let hold = Arc::new(AtomicBool::new(true));
    let mut h = crate::support::blank_harness(4);
    h.state_mut().background = Some(Config { wait: Duration::ZERO, hold: Some(Arc::clone(&hold)) });
    assert!(!h.state().session.active().unwrap().is_dirty());
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    // The workbook in the window is as it was before the command: unchanged. Asked then, the
    // close would find nothing to save and close the window over a change still on its way.
    let close = |h: &mut Harness| {
        h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
        h.run_steps(1);
        h.output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|out| out.commands.iter().any(|c| matches!(c, egui::ViewportCommand::CancelClose)))
    };
    assert!(close(&mut h), "the close is held while the command runs");
    assert!(h.state().is_busy());
    assert!(h.state().dialog.is_none(), "and nothing is asked about a workbook that is about to change");
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    // Once it is over a close asks about the change, as always.
    assert!(close(&mut h));
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name().to_string()).as_deref(), Some("saveChanges"));
}

#[test]
fn a_draft_is_not_entered_into_the_stand_in() {
    let (mut h, hold) = held();
    h.state_mut().run("selection.set", json!({"range": "D4"})).unwrap();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "6"}));
    assert!(h.state().is_busy());
    // A draft committed while busy (keys are held, so only code gets here: a window that comes
    // forward, a file command) waits for the command and is entered in the session it left. An
    // entry made in the stand-in would be lost with it.
    h.state_mut().begin_edit(Some("typed".into()), false);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        hold.store(false, Ordering::Relaxed);
    });
    assert!(h.state_mut().commit_edit(0, 0, false, false));
    release.join().unwrap();
    assert!(!h.state().is_busy());
    assert_eq!(cell(h.state(), "D4"), "Text(\"typed\")");
    assert_eq!(cell(h.state(), "B1"), "Number(12.0)");
}

#[test]
fn a_command_asked_for_every_frame_waits_once() {
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    // As a pane does that follows a setting: it asks each frame until the session shows it, and
    // the stand-in never does. Every one of these used to wait, and run, when the command was over.
    for _ in 0..500 {
        h.state_mut().run_or_alert("cell.set", json!({"cell": "A3", "input": "9"}));
    }
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A4", "input": "1"}));
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A3", "input": "=A4+1"}));
    let before = h.state().background_runs();
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(h.state().background_runs() - before, 3, "the repeated one ran once, and the others in their order");
    assert_eq!(cell(h.state(), "A3"), "Number(2.0)");
}

#[test]
fn a_cancelled_cell_entry_is_a_draft_again_that_takes_the_keyboard() {
    let mut h = crate::support::harness(heavy(), 4);
    h.state_mut().background = Some(Config { wait: Duration::ZERO, hold: None });
    h.state_mut().begin_edit(Some("3".into()), false);
    h.run_steps(2);
    assert!(h.state_mut().commit_edit_from_keys(1, 0, false, false, false));
    assert!(h.state().is_busy());
    for _ in 0..100_000 {
        h.state_mut().cancel_background();
        h.run_steps(1);
        if !h.state().is_busy() {
            break;
        }
    }
    assert_eq!(cell(h.state(), "A1"), "Number(2.0)");
    assert!(h.state().message.is_none());
    assert_eq!(active_cell(h.state()), "A1", "the selection did not move");
    assert_eq!(h.state().editor.as_ref().map(|ed| ed.text.as_str()), Some("3"), "what was typed is a draft again");
    // The draft's text field was gone while the command ran. It has the keyboard again: typing
    // goes on where it stopped.
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("1".into()));
    h.run_steps(2);
    assert_eq!(h.state().editor.as_ref().map(|ed| ed.text.as_str()), Some("31"));
}

#[test]
fn keys_wait_in_a_window_whose_input_the_raw_hook_does_not_see() {
    // A workbook window of its own gets its keys in its own pass, not through the app's raw
    // input hook; the harness feeds its input the same way. Before the pass held them itself,
    // Delete was taken by the grid, waited behind the command and cleared the cell it had set.
    let (mut h, hold) = held();
    h.state_mut().run_or_alert("cell.set", json!({"cell": "A1", "input": "5"}));
    assert!(h.state().is_busy());
    h.key_press(egui::Key::Delete);
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("x".into()));
    h.run_steps(2);
    assert!(h.state().editor.is_none(), "typing starts no draft over the workbook as it was");
    // Esc is the one key taken: it asks the command to stop.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(shows(&h, "Cancelling…"));
    hold.store(false, Ordering::Relaxed);
    until_idle(&mut h);
    assert_eq!(cell(h.state(), "A1"), "Number(5.0)");
    assert!(h.state().editor.is_none());
}
