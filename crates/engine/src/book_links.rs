//! Keeps what each open workbook holds of the other open workbooks' cells current
//! (`gridcraft_model::links`, `docs/external-references.md`).
//!
//! A formula such as `=[Budget.xlsx]Sheet1!B4` is calculated from a copy of that cell kept in
//! the workbook that reads it. After every command the session looks at the workbooks that read
//! others ([`Session::sync_links`]): what their formulas read is collected once per version of
//! the workbook, the cells are copied from the source when it is open, and the readers whose
//! copy changed are recalculated. That is no Undo step of the reader, and it leaves the reader
//! changed, as its saved values are no longer what its file has.
//!
//! Nothing here opens, lists or tests for a file: a source is another workbook open in this
//! session, found by its name.

use std::sync::{Arc, Weak};

use gridcraft_core::RangeRef;
use gridcraft_formula::same_name;
use gridcraft_model::{ExternalLink, Workbook};

use crate::{DocState, Session};

/// How many times the open workbooks are gone through for one command: a chain (C reads B reads
/// A) settles in as many rounds as it is long, and workbooks that read each other in a loop
/// stop here with the values of the last round.
const MAX_ROUNDS: usize = 8;
/// A command that changed more cells than this is not searched for formulas that read another
/// workbook cell by cell: the whole workbook is looked through once instead.
const SEARCH_CHANGED: usize = 10_000;

/// Cells of another workbook that a workbook's formulas read.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Need {
    book: String,
    sheet: String,
    range: RangeRef,
}

impl Need {
    /// What the needs are kept in order by, so the same formulas always give the same list.
    fn key(&self) -> (&str, &str, gridcraft_core::CellRef, gridcraft_core::CellRef) {
        (&self.book, &self.sheet, self.range.start, self.range.end)
    }
}

/// What a document knows about its links between commands.
#[derive(Clone, Debug, Default)]
pub struct LinkState {
    /// Whether its formulas may read another workbook: set when such a formula may have come
    /// in, cleared when the workbook was looked through and has none. A workbook that reads no
    /// other costs nothing here.
    may_read: bool,
    /// The version of the workbook `needs` was collected from.
    of: Weak<Workbook>,
    needs: Vec<Need>,
    /// For each workbook read: the version of it the kept values were copied from (`None`: it
    /// was not open).
    from: Vec<(String, Option<Weak<Workbook>>)>,
}

impl LinkState {
    /// For a workbook that just arrived (a file, a recovery copy): its formulas are not known.
    pub(crate) fn unknown() -> LinkState {
        LinkState { may_read: true, ..LinkState::default() }
    }

    /// After a command changed the cells `changed` of `wb`: notes whether a formula that reads
    /// another workbook may have come in.
    pub(crate) fn note_changed(&mut self, wb: &Workbook, changed: &[gridcraft_calc::Key]) {
        if self.may_read {
            return;
        }
        let reads = |(si, c): &gridcraft_calc::Key| {
            wb.sheet(*si).and_then(|s| s.cell(*c)).and_then(|cell| cell.formula.as_ref()).is_some_and(|f| may_read(&f.text))
        };
        self.may_read = changed.len() > SEARCH_CHANGED || changed.iter().any(reads);
    }
}

/// Whether formula text can hold a reference into another workbook: it has the brackets and the
/// `!`. (A structured reference has brackets too; the formula is parsed to tell.)
fn may_read(text: &str) -> bool {
    text.contains('[') && text.contains('!')
}

/// A link's number as a file writes it (`[1]Sheet1!A1`). Its link is the file's to bring
/// (values and all): none is made up for it, so its formulas keep the values the file saved.
fn is_file_number(book: &str) -> bool {
    !book.is_empty() && book.bytes().all(|b| b.is_ascii_digit())
}

/// The cells of other workbooks that the formulas and defined names of `wb` read, in order.
fn needs_of(wb: &Workbook) -> Vec<Need> {
    let mut found = Vec::new();
    for sheet in &wb.sheets {
        for (_, cell) in sheet.cells.iter() {
            if let Some(f) = cell.formula.as_ref().filter(|f| may_read(&f.text)) {
                f.with_expr(|e| gridcraft_formula::adjust::external_refs(e, &mut found));
            }
        }
    }
    for name in wb.names.iter().filter(|n| may_read(&n.formula)) {
        if let Ok(e) = gridcraft_formula::parse_name(&name.formula) {
            gridcraft_formula::adjust::external_refs(&e, &mut found);
        }
    }
    let mut needs: Vec<Need> = found.into_iter().map(|(book, sheet, range)| Need { book, sheet, range }).collect();
    needs.sort_by(|a, b| a.key().cmp(&b.key()));
    needs.dedup();
    needs
}

/// What a reader keeps of the open workbook `source` for `needs` (all of one workbook): the
/// sheets named that the source has, and the values of the cells read that are not blank. A
/// whole column or row reads the used part of the sheet. Past the link's limits no more is
/// taken in.
fn copy_of(source: &Workbook, name: &str, target: Option<String>, needs: &[&Need]) -> ExternalLink {
    let mut link = ExternalLink { name: name.to_string(), target, sheets: Vec::new() };
    for need in needs {
        let Some(sheet) = source.sheet_index(&need.sheet).and_then(|i| source.sheet(i)) else { continue };
        if !link.touch(&sheet.name) {
            continue;
        }
        let Some(range) = sheet.used_range().and_then(|used| used.intersection(&need.range)) else { continue };
        for (at, cell) in sheet.cells.iter_range(range) {
            if cell.value != gridcraft_core::Value::Empty && !link.set(&sheet.name, at, cell.value.clone()) {
                break;
            }
        }
    }
    link
}

impl Session {
    /// Brings what the open workbooks keep of each other's cells up to date, and recalculates
    /// the ones whose copy changed. Called after every command; a session in which no workbook
    /// reads another does nothing here.
    pub(crate) fn sync_links(&mut self) {
        if !self.docs.iter().any(|d| d.links.may_read || !d.wb.links.is_empty()) {
            self.titles.clear();
            return;
        }
        self.follow_renames();
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for reader in 0..self.docs.len() {
                match self.sync_reader(reader) {
                    Some(c) => changed |= c,
                    // Asked to stop: the rest waits for the next command.
                    None => return,
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// A workbook saved under another name is that name from now on in the formulas of the open
    /// workbooks that read it (unless another open workbook has the old name).
    fn follow_renames(&mut self) {
        let now: Vec<(u64, String)> = self.docs.iter().map(|d| (d.uid, d.display_title())).collect();
        let before = std::mem::replace(&mut self.titles, now.clone());
        for (uid, old) in before {
            let Some((_, new)) = now.iter().find(|(u, _)| *u == uid) else { continue };
            if same_name(&old, new) || now.iter().any(|(_, title)| same_name(title, &old)) {
                continue;
            }
            for reader in self.docs.iter_mut().filter(|d| d.uid != uid) {
                if gridcraft_model::links::find(&reader.wb.links, &old).is_some() {
                    rename_in(reader, &old, new);
                }
            }
        }
    }

    /// Updates the copies one workbook keeps. Whether anything changed; `None` when its
    /// recalculation was asked to stop.
    fn sync_reader(&mut self, reader: usize) -> Option<bool> {
        let d = self.docs.get_mut(reader)?;
        if !d.links.may_read && d.wb.links.is_empty() {
            return Some(false);
        }
        let mut fresh = false;
        if !std::ptr::eq(d.links.of.as_ptr(), Arc::as_ptr(&d.wb)) || d.links.of.strong_count() == 0 {
            d.links.needs = needs_of(&d.wb);
            d.links.of = Arc::downgrade(&d.wb);
            d.links.may_read = !d.links.needs.is_empty();
            fresh = true;
        }
        let d = self.docs.get(reader)?;
        let mut books: Vec<&str> = Vec::new();
        for need in d.links.needs.iter().filter(|n| !is_file_number(&n.book)) {
            if !books.iter().any(|b| same_name(b, &need.book)) {
                books.push(&need.book);
            }
        }
        // The open workbook each name stands for, by the name its window shows.
        let source_of = |book: &str| self.docs.iter().enumerate().find(|(i, s)| *i != reader && same_name(&s.display_title(), book)).map(|(_, s)| s);
        let from: Vec<(String, Option<Weak<Workbook>>)> =
            books.iter().map(|b| (b.to_string(), source_of(b).map(|s| Arc::downgrade(&s.wb)))).collect();
        let same = |a: &Option<Weak<Workbook>>, b: &Option<Weak<Workbook>>| match (a, b) {
            (Some(a), Some(b)) => a.ptr_eq(b) && a.strong_count() > 0,
            (None, None) => true,
            _ => false,
        };
        if !fresh && d.links.from.len() == from.len() && d.links.from.iter().zip(&from).all(|(a, b)| a.0 == b.0 && same(&a.1, &b.1)) {
            return Some(false);
        }
        let mut links = d.wb.links.clone();
        let mut changed = false;
        for book in &books {
            let at = links.iter().position(|l| same_name(&l.name, book));
            match (source_of(book), at) {
                // Not open, and nothing kept of it: the link is made, empty, so its formulas
                // say `#REF!` instead of keeping a blank.
                (None, None) if links.len() < gridcraft_model::links::MAX_LINKS => {
                    links.push(Arc::new(ExternalLink::named(*book)));
                    changed = true;
                }
                (None, _) => {}
                (Some(source), at) => {
                    let needs: Vec<&Need> = d.links.needs.iter().filter(|n| same_name(&n.book, book)).collect();
                    let (name, target) = match at.and_then(|a| links.get(a)) {
                        Some(link) => (link.name.clone(), link.target.clone()),
                        None => (book.to_string(), None),
                    };
                    let copy = copy_of(&source.wb, &name, target, &needs);
                    let room = links.len() < gridcraft_model::links::MAX_LINKS;
                    match at.and_then(|a| links.get_mut(a)) {
                        Some(link) if **link == copy => {}
                        Some(link) => {
                            *link = Arc::new(copy);
                            changed = true;
                        }
                        None if room => {
                            links.push(Arc::new(copy));
                            changed = true;
                        }
                        None => {}
                    }
                }
            }
        }
        let d = self.docs.get_mut(reader)?;
        d.links.from = from;
        if !changed {
            return Some(false);
        }
        let mut wb = (*d.wb).clone();
        wb.links = links;
        // The formulas that read another workbook are calculated at every recalculation
        // (`gridcraft_calc::precedents`), with what depends on them.
        d.calc.cells_changed(&mut wb, &[]);
        if d.calc.interrupt.as_ref().is_some_and(|interrupt| interrupt.was_hit()) {
            d.calc.restore_after_stop(&d.wb, Some(&[]));
            d.links.from.clear();
            return None;
        }
        d.wb = Arc::new(wb);
        d.revision += 1;
        // The formulas are the ones `needs` was collected from.
        d.links.of = Arc::downgrade(&d.wb);
        Some(true)
    }
}

/// Renames the workbook `old` to `new` in `reader`: in its link, its formulas and its names.
fn rename_in(reader: &mut DocState, old: &str, new: &str) {
    let mut wb = (*reader.wb).clone();
    for link in wb.links.iter_mut().filter(|l| same_name(&l.name, old)) {
        Arc::make_mut(link).name = new.to_string();
    }
    for si in 0..wb.sheets.len() {
        let formulas: Vec<_> = wb
            .sheets
            .get(si)
            .map(|s| s.cells.iter().filter_map(|(c, cell)| cell.formula.clone().filter(|f| may_read(&f.text)).map(|f| (c, f))).collect())
            .unwrap_or_default();
        let Some(sheet) = wb.sheet_mut(si) else { continue };
        for (at, formula) in formulas {
            let renamed = formula.with_expr(|e| Some(gridcraft_formula::adjust::rename_book(e.clone(), old, new)).filter(|ne| ne != e));
            if let Some(Some(e)) = renamed
                && let Some(cell) = sheet.cells.get_mut(at)
            {
                cell.formula = Some(Arc::new(gridcraft_model::Formula {
                    array: formula.array,
                    dynamic: formula.dynamic,
                    ..gridcraft_model::Formula::from_expr(e)
                }));
            }
        }
    }
    for name in wb.names.iter_mut().filter(|n| may_read(&n.formula)) {
        if let Ok(e) = gridcraft_formula::parse_name(&name.formula) {
            name.formula = gridcraft_formula::print(&gridcraft_formula::adjust::rename_book(e, old, new));
        }
    }
    // Formulas changed their text, not what they read: the graph is built again and nothing
    // needs calculating for it.
    reader.calc.rebuild(&wb);
    reader.wb = Arc::new(wb);
    reader.revision += 1;
}
