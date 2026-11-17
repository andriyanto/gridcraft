//! Commands that run off the interface thread, so the window stays alive while one calculates.
//!
//! A command that changes a workbook, asked for through [`SheetApp::run_or_alert`] or typed into
//! a cell, goes to a worker thread with the whole [`Session`]. The interface thread waits for it
//! as long as [`Config::wait`]: nearly every command is back by then, and nothing differs from
//! running it here. One that is not leaves the app *busy*:
//!
//! - `SheetApp::session` is a [`Session::stand_in`], the workbooks as they were before the
//!   command, so every window keeps painting what it showed;
//! - a "Calculating…" box covers the window, with a Cancel button (and Esc) that asks the
//!   command to stop ([`Session::interrupt`]); keys and dropped files are left out meanwhile;
//! - what is asked for through `run_or_alert` waits its turn in a queue (which Cancel empties);
//! - a command run for its result ([`SheetApp::run`]) waits for the busy one to finish, on the
//!   interface thread as before, since its caller goes on with what it returns.
//!
//! When the command comes back its session takes the stand-in's place, and what follows a
//! command (dialogs it asked for, the error it gave) happens then.
//!
//! Off by default: tests, the web app (which has no threads) and embedding hosts run every
//! command where it is asked for. The desktop app turns it on.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::Duration;

use gridcraft_engine::calc::Interrupt;
use gridcraft_engine::{EngineError, Mode, Session, find_command};
use serde_json::Value as Json;

use crate::clock::monotonic_ms;
use crate::{SheetApp, clean_error, editor, now_ms};

/// How commands run in the background.
#[derive(Clone, Debug)]
pub struct Config {
    /// How long the interface thread waits for a command before it goes on without it.
    pub wait: Duration,
    /// For tests: while this is set the worker does not start the command it was given, so the
    /// app is certainly busy when the wait is over.
    pub hold: Option<Arc<AtomicBool>>,
}

impl Default for Config {
    fn default() -> Self {
        // Long enough for nearly every command, short enough that a held frame is not felt as
        // a freeze.
        Config { wait: Duration::from_millis(250), hold: None }
    }
}

/// A command for the worker, with the session it runs in.
struct Job {
    session: Session,
    id: String,
    params: Json,
    hold: Option<Arc<AtomicBool>>,
    /// Wakes the interface thread when the command is over.
    wake: Option<egui::Context>,
    reply: Sender<Done>,
}

/// The session back from the worker, with what the command returned.
struct Done {
    session: Session,
    result: Result<Json, String>,
}

/// What the app does when a command that went on in the background is over.
pub(crate) enum After {
    /// As `run_or_alert` does: show the error, if any.
    Alert,
    /// A cell entry: on success move the selection as the key that entered it asks, on failure
    /// open the draft again.
    Edit { draft: Box<editor::EditState>, dr: i64, dc: i64 },
}

/// A command the worker is running.
pub(crate) struct Busy {
    done: Receiver<Done>,
    stop: Arc<Interrupt>,
    /// When it was started ([`monotonic_ms`]).
    since: f64,
    /// Cancel was asked for.
    cancelling: bool,
    id: String,
    /// The active window when it started.
    window: Option<u64>,
    /// The session's mode when it started: the interface changes the mode of the session it
    /// holds, which meanwhile is the stand-in.
    mode: Mode,
    /// Whether cells were copied (the marquee) and the Format Painter held a format then: the
    /// interface lets go of both on the session it holds.
    copied: bool,
    painting: bool,
    after: After,
}

/// The app's background state.
#[derive(Default)]
pub(crate) struct State {
    worker: Option<Sender<Job>>,
    pub(crate) busy: Option<Busy>,
    /// Commands asked for while one runs, in order.
    queued: VecDeque<(String, Json)>,
    /// Woken when a command is over (the context of the last frame).
    pub(crate) wake: Option<egui::Context>,
    /// Commands the worker has run, for tests.
    pub(crate) ran: usize,
}

/// What became of a command given to the worker.
pub(crate) enum Launch {
    /// It is over (or could only run here): its result.
    Done(Result<Json, String>),
    /// It is still running: the app is busy with it once `after` is known.
    Pending(Pending),
}

pub(crate) struct Pending {
    done: Receiver<Done>,
    stop: Arc<Interrupt>,
    mode: Mode,
    copied: bool,
    painting: bool,
}

/// The most commands that wait behind a busy one. What gets there is what the frame that started
/// the command asked for after it, a handful at most.
const QUEUE_LIMIT: usize = 64;

/// The worker's stack: commands were written for the main thread's, which is 8 MiB on most
/// systems (formula evaluation bounds its own depth at under one).
const WORKER_STACK: usize = 16 * 1024 * 1024;

fn worker() -> Option<Sender<Job>> {
    let (jobs, inbox) = channel::<Job>();
    let run = move || {
        for job in inbox {
            let Job { mut session, id, params, hold, wake, reply } = job;
            while hold.as_ref().is_some_and(|hold| hold.load(Ordering::Relaxed)) {
                std::thread::sleep(Duration::from_millis(1));
            }
            // `Session::execute` catches a command's panic itself. Whatever else unwinds, the
            // session goes back: it holds the person's workbooks.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.run(&id, params)))
                .unwrap_or_else(|_| Err(EngineError::Internal(id, "the command stopped unexpectedly".into()).to_string()));
            if reply.send(Done { session, result }).is_ok()
                && let Some(ctx) = wake
            {
                ctx.request_repaint();
            }
        }
    };
    std::thread::Builder::new().name("gridcraft-commands".into()).stack_size(WORKER_STACK).spawn(run).ok().map(|_| jobs)
}

/// Whether command `id` changes a workbook, so that it may take long: the commands Undo takes
/// back, Undo and Redo themselves, and a recalculation asked for. Everything else (queries,
/// selection and view commands, the interface's own) is quick and runs where it is asked for.
fn changes_a_workbook(id: &str) -> bool {
    // These reach outside the session (files, windows, the debugger's run, scripts' own steps).
    const HERE: [&str; 5] = ["file.", "window.", "macro.", "automate.", "app."];
    if HERE.iter().any(|prefix| id.starts_with(prefix)) {
        return false;
    }
    matches!(id, "edit.undo" | "edit.redo" | "formulas.calculateNow" | "formulas.calculateSheet")
        || find_command(id).is_some_and(|spec| spec.undoable)
}

/// Whether engine command `id` is more than a query: it runs in the real session, never in a
/// stand-in.
fn needs_the_session(id: &str) -> bool {
    find_command(id).is_some_and(|spec| spec.journal || spec.undoable)
}

impl SheetApp {
    /// Whether a command is running in the background.
    pub fn is_busy(&self) -> bool {
        self.background_state.busy.is_some()
    }

    /// How many commands the worker has run (for tests).
    pub fn background_runs(&self) -> usize {
        self.background_state.ran
    }

    /// Whether `id` asked for now would be given to the worker.
    pub(crate) fn runs_in_background(&self, id: &str) -> bool {
        !cfg!(target_arch = "wasm32") && self.background.is_some() && !self.is_busy() && changes_a_workbook(id)
    }

    /// Gives command `id` to the worker and waits for it as long as the configuration says.
    /// Run here instead when there is no worker, or the session can't be stood in for.
    pub(crate) fn launch(&mut self, id: &str, params: Json) -> Launch {
        let Some(config) = self.background.clone() else { return Launch::Done(self.session.run(id, params)) };
        let Some(stand_in) = self.session.stand_in() else { return Launch::Done(self.session.run(id, params)) };
        if self.background_state.worker.is_none() {
            self.background_state.worker = worker();
        }
        let Some(jobs) = self.background_state.worker.clone() else { return Launch::Done(self.session.run(id, params)) };
        let stop = self.session.interrupt();
        let (mode, copied, painting) = (self.session.mode, self.session.clipboard.is_some(), self.session.format_painter.is_some());
        let session = std::mem::replace(&mut self.session, stand_in);
        let (reply, done) = channel();
        let job = Job { session, id: id.to_string(), params, hold: config.hold.clone(), wake: self.background_state.wake.clone(), reply };
        if let Err(std::sync::mpsc::SendError(job)) = jobs.send(job) {
            // The worker is gone: here, as before.
            self.background_state.worker = None;
            self.session = job.session;
            return Launch::Done(self.session.run(&job.id, job.params));
        }
        self.background_state.ran += 1;
        match done.recv_timeout(config.wait) {
            Ok(back) => {
                self.session = back.session;
                Launch::Done(back.result)
            }
            Err(RecvTimeoutError::Timeout) => Launch::Pending(Pending { done, stop, mode, copied, painting }),
            Err(RecvTimeoutError::Disconnected) => Launch::Done(Err(self.session_lost(stop))),
        }
    }

    /// The app is busy with command `id` from here on.
    pub(crate) fn wait_in_background(&mut self, id: &str, window: Option<u64>, pending: Pending, after: After) {
        let Pending { done, stop, mode, copied, painting } = pending;
        let since = monotonic_ms();
        self.background_state.busy = Some(Busy { done, stop, since, cancelling: false, id: id.to_string(), window, mode, copied, painting, after });
    }

    /// The worker went away with the session. The stand-in is what is left of it: the workbooks
    /// as they were before the command. `stop` is the lost session's handle.
    fn session_lost(&mut self, stop: Arc<Interrupt>) -> String {
        self.background_state.worker = None;
        self.background_state.busy = None;
        self.background_state.queued.clear();
        self.session.adopt(stop);
        "The command stopped unexpectedly. The workbook is as it was before it.".to_string()
    }

    /// A command asked for while another runs: it waits its turn.
    ///
    /// Some of the interface asks every frame until the session shows what it asked for (a pane
    /// that follows a setting), and the stand-in never does: the same command asked for again
    /// right behind itself is the one already waiting. Nothing a person does reaches the queue
    /// (keys and the pointer are held), so it is kept short.
    pub(crate) fn queue_command(&mut self, id: &str, params: Json) {
        let queued = &mut self.background_state.queued;
        let waiting = queued.back().is_some_and(|(last, with)| last == id && *with == params);
        if !waiting && queued.len() < QUEUE_LIMIT {
            queued.push_back((id.to_string(), params));
        }
    }

    /// Once a frame: takes the busy command's session back when it is over.
    pub(crate) fn poll_background(&mut self, ctx: &egui::Context) {
        self.background_state.wake = Some(ctx.clone());
        let Some(busy) = &self.background_state.busy else { return };
        match busy.done.try_recv() {
            Ok(back) => self.command_over(back),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                let lost = self.session_lost(Arc::clone(&busy.stop));
                self.alert(lost);
            }
        }
    }

    /// Before an engine command that needs the real session runs here: waits for the busy
    /// command, and for the ones queued behind it, to be over.
    pub(crate) fn finish_background(&mut self, id: &str) {
        if self.is_busy() && needs_the_session(id) {
            self.wait_for_background();
        }
    }

    /// Waits for the busy command, and for the ones queued behind it, to be over. What then
    /// prepares the session for a command of its own (a window made active, a draft checked
    /// against its cell) prepares the real one.
    pub(crate) fn wait_for_background(&mut self) {
        while let Some(busy) = &self.background_state.busy {
            match busy.done.recv() {
                Ok(back) => self.command_over(back),
                Err(_) => {
                    let lost = self.session_lost(Arc::clone(&busy.stop));
                    self.alert(lost);
                }
            }
        }
    }

    /// Runs an engine command in the session, where the interface runs one without the rest of
    /// [`SheetApp::run`]. One that needs the real session waits for a command still running in
    /// the background; a query reads the stand-in.
    pub fn engine(&mut self, id: &str, params: Json) -> Result<Json, String> {
        self.finish_background(id);
        self.session.run(id, params)
    }

    /// Asks the busy command to stop.
    pub fn cancel_background(&mut self) {
        if let Some(busy) = &mut self.background_state.busy {
            busy.stop.request();
            busy.cancelling = true;
        }
    }

    /// The busy command is over: its session takes the stand-in's place, what follows the command
    /// happens, and the commands that waited behind it run.
    fn command_over(&mut self, back: Done) {
        let Some(busy) = self.background_state.busy.take() else { return };
        // What the interface sets on the session it holds, it set on the stand-in meanwhile: the
        // mode, and letting go of the copied cells and of the Format Painter (Esc, or Enter
        // after a paste).
        let (mode, copied, painting) = (self.session.mode, self.session.clipboard.is_some(), self.session.format_painter.is_some());
        // The window the person is in now, when it is not the one the command was asked in.
        let moved_to = self.session.active_window().filter(|w| Some(*w) != busy.window);
        self.session = back.session;
        if mode != busy.mode {
            self.session.mode = mode;
        }
        if busy.copied && !copied {
            self.session.clipboard = None;
        }
        if busy.painting && !painting {
            self.session.format_painter = None;
        }
        let stopped = back.result.as_ref().is_err_and(|e| *e == EngineError::Cancelled.to_string());
        match busy.after {
            After::Alert => {
                let r = self.after_command(&busy.id, busy.window, back.result);
                self.note_copy(&busy.id, &r);
                match r {
                    Ok(_) => {}
                    Err(_) if stopped => {}
                    Err(e) if self.message.as_ref().is_some_and(|(_, m)| *m == e) => {}
                    Err(e) => self.alert(e),
                }
            }
            After::Edit { draft, dr, dc } => match back.result {
                Ok(_) => {
                    if dr != 0 || dc != 0 {
                        self.move_after_enter(dr, dc);
                    }
                    self.after_engine();
                }
                Err(e) => {
                    // The entry was not made: its draft is open again, in its window.
                    self.follow_active_window();
                    if !stopped {
                        self.message = Some(("GridCraft".into(), clean_error(&e)));
                    }
                    if self.editor.is_none() {
                        // Its text field went away with it: it takes the keyboard again, with
                        // the caret where it was.
                        let mut draft = *draft;
                        draft.request_focus = true;
                        draft.sync_caret = true;
                        self.editor = Some(draft);
                        self.session.mode = Mode::Edit;
                    }
                }
            },
        }
        if stopped {
            self.toast = Some(("Cancelled".into(), now_ms()));
        }
        if stopped || busy.cancelling {
            // Cancel stops what was going on, and that includes what waited behind the command,
            // also when the command was over before it looked. (After a command that failed by
            // itself the next one runs, as it did when both ran where they were asked for.)
            self.background_state.queued.clear();
        }
        while !self.is_busy() {
            let Some((id, params)) = self.background_state.queued.pop_front() else { break };
            self.run_or_alert(&id, params);
        }
        // All of that happened in the command's window: what it asked for (a dialog, its error,
        // its draft opened again) is that window's, and so is what waited behind it. The session
        // came back with that window active. The person may have gone on to another window
        // meanwhile, and that one is the active window again: left as it came back, the command's
        // window was brought forward over theirs when the command was over, and what they then
        // asked for in their window ran in the command's workbook.
        if let Some(window) = moved_to.filter(|w| self.session.window(*w).is_some()) {
            self.enter_window(window);
        }
    }

    /// While busy: keys, text and dropped files don't reach the window, which shows the workbook
    /// as it was. Esc asks the command to stop. Key releases pass, so a key held when the
    /// command began is not left down, and so does the pointer, which the box stops.
    pub(crate) fn hold_input(&mut self, raw: &mut egui::RawInput) {
        if !self.is_busy() {
            return;
        }
        let cancel = hold_events(&mut raw.events);
        raw.dropped_files.clear();
        raw.hovered_files.clear();
        if cancel {
            self.cancel_background();
        }
    }

    /// [`SheetApp::hold_input`] for the window whose pass this is. The raw input hook sees the
    /// main window's input only: a workbook window of its own gets its keys in its own pass.
    pub(crate) fn hold_window_input(&mut self, ctx: &egui::Context) {
        if self.is_busy() && ctx.input_mut(|input| hold_events(&mut input.events)) {
            self.cancel_background();
        }
    }
}

/// Takes out the events that wait while a command runs. Whether Esc was among them.
fn hold_events(events: &mut Vec<egui::Event>) -> bool {
    let mut cancel = false;
    events.retain(|event| match event {
        egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
            cancel = true;
            false
        }
        egui::Event::Key { pressed, .. } => !pressed,
        egui::Event::Text(_)
        | egui::Event::Paste(_)
        | egui::Event::PasteImage(_)
        | egui::Event::Copy
        | egui::Event::Cut
        | egui::Event::Ime(_)
        | egui::Event::Zoom(_) => false,
        _ => true,
    });
    cancel
}

/// The box over a busy window: what is going on, for how long, and Cancel.
pub(crate) fn overlay(app: &mut SheetApp, ctx: &egui::Context) {
    let Some(busy) = app.background_state.busy.as_ref() else { return };
    let seconds = ((monotonic_ms() - busy.since) / 1000.0).max(0.0).floor();
    let cancelling = busy.cancelling;
    let title = app.ui.language.tr(if cancelling { "Cancelling…" } else { "Calculating…" }).to_owned();
    let cancel_label = app.ui.language.tr("Cancel").to_owned();
    let mut cancel = false;
    egui::Modal::new(egui::Id::unique("background_command")).show(ctx, |ui| {
        ui.set_width(260.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(title).strong());
            if seconds >= 1.0 {
                ui.label(format!("{seconds:.0} s"));
            }
        });
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(!cancelling, egui::Button::new(cancel_label)).clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        app.cancel_background();
    }
    // The seconds, and the spinner.
    ctx.request_repaint_after(Duration::from_millis(100));
}
