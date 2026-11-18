//! The interface state each window keeps for itself.
//!
//! [`SheetApp`]'s per-window fields (the cell draft, dialog, Backstage view, grid gestures…) hold
//! the state of one window: the engine's active window. Every other window's state is parked in a
//! [`WindowUi`] and swapped in when that window is activated or drawn, the way the engine swaps a
//! window's selection and sheet into its workbook. So a draft, a dialog or a message stays in the
//! window it belongs to.

use gridcraft_engine::core::CellRef;

use crate::{Notice, SheetApp, backstage, dialogs, editor, grid, keytips, text_box};

/// One window's parked interface state (see the module docs).
#[derive(Default)]
pub struct WindowUi {
    editor: Option<editor::EditState>,
    text_box_editor: Option<text_box::EditState>,
    dialog: Option<dialogs::Dialog>,
    backstage: Option<backstage::Backstage>,
    message: Option<(String, String)>,
    toast: Option<(String, f64)>,
    notice: Option<Notice>,
    grid: grid::GridState,
    name_box: Option<String>,
    keytips: keytips::KeyTips,
    selected_object: Option<gridcraft_engine::model::ObjRef>,
    tab_run: Option<(u64, usize, u32, CellRef)>,
    open_switch_windows: bool,
    /// The ribbon tab this window shows. A contextual tab depends on this window's selection, so
    /// another window's frame must not reset it.
    ribbon_tab: String,
    /// The status bar's Ready/Enter/Edit, which follows the window's cell draft.
    mode: gridcraft_engine::Mode,
    /// A key tip's control request waiting for the frame that draws this window's ribbon.
    keytip_invoke_next: Option<&'static str>,
}

impl WindowUi {
    /// A new window's state. The last border, fill and font colour picked carry over, as they are
    /// the user's recent choices rather than the window's; so does the ribbon tab.
    fn fresh(like: &grid::GridState, ribbon_tab: &str) -> WindowUi {
        WindowUi {
            ribbon_tab: ribbon_tab.to_string(),
            grid: grid::GridState {
                last_border: like.last_border.clone(),
                last_fill: like.last_fill.clone(),
                last_font_color: like.last_font_color.clone(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl SheetApp {
    /// Exchanges the per-window fields with `w`. Destructured in full, so a field added to
    /// [`WindowUi`] can't be left out of the swap.
    fn swap_window_ui(&mut self, w: &mut WindowUi) {
        let WindowUi {
            editor,
            text_box_editor,
            dialog,
            backstage,
            message,
            toast,
            notice,
            grid,
            name_box,
            keytips,
            selected_object,
            tab_run,
            open_switch_windows,
            ribbon_tab,
            mode,
            keytip_invoke_next,
        } = w;
        std::mem::swap(&mut self.editor, editor);
        std::mem::swap(&mut self.text_box_editor, text_box_editor);
        std::mem::swap(&mut self.dialog, dialog);
        std::mem::swap(&mut self.backstage, backstage);
        std::mem::swap(&mut self.message, message);
        std::mem::swap(&mut self.toast, toast);
        std::mem::swap(&mut self.notice, notice);
        std::mem::swap(&mut self.grid, grid);
        std::mem::swap(&mut self.name_box, name_box);
        std::mem::swap(&mut self.keytips, keytips);
        std::mem::swap(&mut self.selected_object, selected_object);
        std::mem::swap(&mut self.tab_run, tab_run);
        std::mem::swap(&mut self.open_switch_windows, open_switch_windows);
        std::mem::swap(&mut self.ui.ribbon_tab, ribbon_tab);
        std::mem::swap(&mut self.session.mode, mode);
        std::mem::swap(&mut self.ui.keytip_invoke_next, keytip_invoke_next);
    }

    /// The window whose interface state is in the per-window fields.
    pub fn ui_window(&self) -> Option<u64> {
        self.ui_window
    }

    /// Brings the per-window fields in line with the engine's active window, after a command (or
    /// a script) switched windows: the previous window's state is parked, the active one's swapped
    /// in (a window never seen before starts fresh). A closed window's state is dropped.
    pub fn follow_active_window(&mut self) {
        let active = self.session.active_window();
        // A window hidden while parked (from another window's View › Hide) isn't drawn either:
        // its waiting key tip request goes, as it does for a window hidden as it is left.
        let session = &self.session;
        for (id, parked) in &mut self.parked {
            if session.window(*id).is_some_and(|w| w.hidden) {
                parked.keytip_invoke_next = None;
            }
        }
        if active == self.ui_window {
            return;
        }
        let Some(previous) = self.ui_window else {
            // The first window: the fields are already its state.
            self.ui_window = active;
            return;
        };
        let mut outgoing = WindowUi::fresh(&self.grid, &self.ui.ribbon_tab);
        self.swap_window_ui(&mut outgoing);
        if let Some(window) = self.session.window(previous) {
            // A hidden window isn't drawn: a key tip request waiting for its next frame would
            // fire whenever it is shown again, so it goes.
            if window.hidden {
                outgoing.keytip_invoke_next = None;
            }
            self.parked.insert(previous, outgoing);
        }
        if let Some(mut incoming) = active.and_then(|w| self.parked.remove(&w)) {
            self.swap_window_ui(&mut incoming);
        }
        self.ui_window = active;
        let session = &self.session;
        self.parked.retain(|w, _| session.window(*w).is_some());
    }

    /// Activates window `id` (it becomes the most recently active) with its interface state.
    pub fn enter_window(&mut self, id: u64) {
        self.session.activate_window(id);
        self.follow_active_window();
    }

    /// Makes window `id` and its interface state current while it is drawn, without changing
    /// which window counts as most recently active.
    pub fn show_window(&mut self, id: u64) {
        self.session.show_window(id);
        self.follow_active_window();
    }

    /// Window `to`, which is being drawn, has just got the keyboard from window `from`. A
    /// formula open there at a place where a reference can go comes along instead of being
    /// entered: cells clicked in this window go into it (`[Budget.xlsx]Sheet1!B4` from another
    /// workbook), its text and keys are in this window's formula bar, and Enter or Esc take it
    /// back to its own window. A draft that is away keeps going from window to window, and is
    /// at home again when its own window gets the keyboard.
    ///
    /// Any other draft stays where it is, to be entered as before; so does this one when the
    /// window has something of its own that takes the keyboard, or shows a workbook whose
    /// name a formula can't hold. Whether the draft came along.
    pub(crate) fn carry_draft(&mut self, from: u64, to: u64) -> bool {
        if from == to || self.ui_window != Some(to) || self.is_busy() {
            return false;
        }
        if self.editor.is_some() || self.text_box_editor.is_some() || self.dialog.is_some() || self.backstage.is_some() {
            return false;
        }
        let Some(shown) = self.session.active().map(|d| (d.uid, d.display_title())) else { return false };
        let Some(parked) = self.parked.get_mut(&from) else { return false };
        let comes = parked.editor.as_ref().is_some_and(|ed| {
            let home = ed.window == Some(to);
            let named = shown.0 == ed.doc || editor::book_can_be_named(&shown.1);
            !ed.python && ed.is_formula() && (ed.away || ed.can_point()) && (home || named)
        });
        if !comes || parked.dialog.is_some() {
            return false;
        }
        let Some(mut ed) = parked.editor.take() else { return false };
        parked.mode = gridcraft_engine::Mode::Ready;
        ed.away = ed.window != Some(to);
        // Both name a place in the window left behind.
        ed.point_cell = None;
        ed.point_anchor = None;
        ed.request_focus = true;
        ed.sync_caret = true;
        self.session.mode = if ed.can_point() { gridcraft_engine::Mode::Point } else { gridcraft_engine::Mode::Edit };
        self.editor = Some(ed);
        true
    }

    /// The formula that left this window for its references, while it is away: this window
    /// still shows it in its cell.
    pub(crate) fn draft_gone_away(&self) -> Option<&editor::EditState> {
        let here = self.ui_window?;
        self.parked.values().filter_map(|w| w.editor.as_ref()).find(|ed| ed.away && ed.window == Some(here))
    }

    /// Puts back a dialog that was taken out of window `home`'s fields while it ran. A command
    /// run from the dialog may have switched windows meanwhile; the dialog stays in the window it
    /// was opened in (dropped if that window closed).
    pub(crate) fn return_dialog(&mut self, home: Option<u64>, d: dialogs::Dialog) {
        if home == self.ui_window {
            self.dialog = Some(d);
        } else if let Some(w) = home.and_then(|h| self.parked.get_mut(&h))
            && w.dialog.is_none()
        {
            w.dialog = Some(d);
        }
    }

    /// The window that shows the dialog `name`: the current one or a parked one. A window that has
    /// closed shows nothing, even while its parked state is still kept.
    pub(crate) fn window_with_dialog(&self, name: &str) -> Option<u64> {
        if self.dialog.as_ref().is_some_and(|d| d.name == name) {
            return self.ui_window;
        }
        self.parked
            .iter()
            .filter(|(id, w)| w.dialog.as_ref().is_some_and(|d| d.name == name) && self.session.window(**id).is_some())
            .map(|(id, _)| *id)
            .min()
    }

    /// Whether a window other than the current one has a cell or text-box draft open.
    pub fn parked_window_with_draft(&self) -> Option<u64> {
        self.parked.iter().filter(|(_, w)| w.editor.is_some() || w.text_box_editor.is_some()).map(|(id, _)| *id).min()
    }
}
