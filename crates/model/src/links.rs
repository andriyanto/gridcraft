//! Links to other workbooks: the workbooks this one's formulas read cells of
//! (`=[Budget.xlsx]Sheet1!B4`), each with a copy of the values read.
//!
//! The copy is what a formula evaluates against: calculation never reaches into another workbook.
//! Whoever holds both workbooks keeps the copy current; a file saves it, so the values are there
//! when the source is not. See `docs/external-references.md`.
//!
//! Everything here can come from a file, so it is bounded ([`MAX_LINKS`], [`MAX_LINK_SHEETS`],
//! [`MAX_LINK_CELLS`]): past a limit nothing more is taken in, and what asks for it is told.

use std::collections::BTreeMap;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use serde::{Deserialize, Serialize};

/// Most links one workbook keeps.
pub const MAX_LINKS: usize = 1024;
/// Most sheets one link knows of its source.
pub const MAX_LINK_SHEETS: usize = 4096;
/// Most values one link keeps of its source, over all its sheets.
pub const MAX_LINK_CELLS: usize = 2_000_000;

/// One other workbook, as this workbook knows it.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ExternalLink {
    /// The name formulas use between the brackets: the source's file name, or its title while it
    /// has no file.
    pub name: String,
    /// Where the source is, as a file writes it: relative to this workbook's folder with forward
    /// slashes, or a `file:///` address. `None`: known by its name alone. Text only: nothing
    /// opens, lists or tests for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The sheets of the source this workbook reads, with the values read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheets: Vec<LinkSheet>,
    /// For a link the workbook's file brought: its number there (`[1]Sheet1!A1`, the first of
    /// the file's `<externalReferences>`), which the file content kept in
    /// [`Workbook::preserved`](crate::Workbook::preserved) knows it by. `None` for a link made
    /// here. Not in JSON, as that content is not.
    #[serde(skip)]
    pub number: Option<u32>,
}

/// One sheet of a source: the values this workbook has of it. A cell that is not here is blank.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct LinkSheet {
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", with = "crate::cellmap")]
    pub cells: BTreeMap<CellRef, Value>,
}

impl ExternalLink {
    /// A link to the workbook `name`, with nothing read yet.
    pub fn named(name: impl Into<String>) -> ExternalLink {
        ExternalLink { name: name.into(), target: None, sheets: Vec::new(), number: None }
    }

    /// The sheet `name` of the source (names match as sheet names do, without regard to case).
    pub fn sheet(&self, name: &str) -> Option<&LinkSheet> {
        self.sheets.iter().find(|s| gridcraft_formula::same_name(&s.name, name))
    }

    /// Notes that the source has a sheet `name`, with nothing read of it yet (a reference into
    /// a sheet the link does not know is `#REF!`; into one it knows, blank). `false` at the limit.
    pub fn touch(&mut self, name: &str) -> bool {
        if self.sheet(name).is_some() {
            return true;
        }
        if self.sheets.len() >= MAX_LINK_SHEETS {
            return false;
        }
        self.sheets.push(LinkSheet { name: name.to_string(), cells: BTreeMap::new() });
        true
    }

    /// Forgets the sheet `name`: the source has none (a reference into it is `#REF!`).
    pub fn forget(&mut self, name: &str) {
        self.sheets.retain(|s| !gridcraft_formula::same_name(&s.name, name));
    }

    /// Drops the values kept for `range` of the sheet `name`, before they are read again.
    pub fn clear(&mut self, name: &str, range: RangeRef) {
        let Some(sheet) = self.sheets.iter_mut().find(|s| gridcraft_formula::same_name(&s.name, name)) else { return };
        let gone: Vec<CellRef> = sheet.cells_in(range).map(|(cell, _)| cell).collect();
        for cell in gone {
            sheet.cells.remove(&cell);
        }
    }

    /// Whether this link and `other` know the same sheets with the same values, in any order.
    pub fn same_values(&self, other: &ExternalLink) -> bool {
        self.sheets.len() == other.sheets.len() && self.sheets.iter().all(|s| other.sheet(&s.name).is_some_and(|o| o.cells == s.cells))
    }

    /// How many values the link keeps.
    pub fn cell_count(&self) -> usize {
        self.sheets.iter().map(|s| s.cells.len()).sum()
    }

    /// Sets what this workbook has of `cell` on the source's sheet `sheet` (blank: nothing is
    /// kept). `false` when a limit is reached and the value was not taken in.
    pub fn set(&mut self, sheet: &str, cell: CellRef, value: Value) -> bool {
        let at = match self.sheets.iter().position(|s| gridcraft_formula::same_name(&s.name, sheet)) {
            Some(at) => at,
            None if self.sheets.len() >= MAX_LINK_SHEETS => return false,
            None => {
                self.sheets.push(LinkSheet { name: sheet.to_string(), cells: BTreeMap::new() });
                self.sheets.len() - 1
            }
        };
        let full = self.cell_count() >= MAX_LINK_CELLS;
        let Some(kept) = self.sheets.get_mut(at) else { return false };
        if value == Value::Empty {
            kept.cells.remove(&cell);
            return true;
        }
        if full && !kept.cells.contains_key(&cell) {
            return false;
        }
        kept.cells.insert(cell, value);
        true
    }
}

impl LinkSheet {
    /// The value kept for `cell` (blank when none is).
    pub fn value(&self, cell: CellRef) -> Value {
        self.cells.get(&cell).cloned().unwrap_or(Value::Empty)
    }

    /// The last row and column of `range` that hold a kept value, or `None` when none does.
    pub fn filled_end(&self, range: RangeRef) -> Option<CellRef> {
        let mut end: Option<CellRef> = None;
        for cell in self.cells_in(range).map(|(cell, _)| cell) {
            end = Some(end.map_or(cell, |e| CellRef::new(e.row.max(cell.row), e.col.max(cell.col))));
        }
        end
    }

    /// The kept values inside `range`, in row order.
    pub fn cells_in(&self, range: RangeRef) -> impl Iterator<Item = (CellRef, &Value)> {
        let (first, last) = (CellRef::new(range.start.row, 0), CellRef::new(range.end.row, u32::MAX));
        self.cells.range(first..=last).filter(move |(c, _)| c.col >= range.start.col && c.col <= range.end.col).map(|(c, v)| (*c, v))
    }
}

/// The links of a workbook. Each is shared between the workbook's versions until it changes.
pub type Links = Vec<Arc<ExternalLink>>;

/// The link to the workbook `name` among `links` (names match without regard to case).
pub fn find<'a>(links: &'a [Arc<ExternalLink>], name: &str) -> Option<&'a ExternalLink> {
    links.iter().map(|l| l.as_ref()).find(|l| gridcraft_formula::same_name(&l.name, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(a1: &str) -> CellRef {
        CellRef::parse(a1).unwrap_or_default()
    }

    #[test]
    fn a_link_keeps_values_by_sheet_and_cell_and_forgets_blanks() {
        let mut link = ExternalLink::named("Budget.xlsx");
        assert!(link.set("Sheet1", at("B4"), Value::Number(7.0)));
        assert!(link.set("sheet1", at("B5"), Value::text("x")));
        assert!(link.set("Other", at("A1"), Value::Bool(true)));
        assert_eq!(link.sheets.len(), 2, "sheet names match without regard to case");
        let sheet = link.sheet("SHEET1").unwrap();
        assert_eq!(sheet.value(at("B4")), Value::Number(7.0));
        assert_eq!(sheet.value(at("Z9")), Value::Empty);
        assert!(link.set("Sheet1", at("B4"), Value::Empty));
        assert_eq!(link.cell_count(), 2);
        assert!(link.sheet("Nowhere").is_none());
    }

    #[test]
    fn the_filled_end_of_a_range_is_its_last_kept_row_and_column() {
        let mut link = ExternalLink::named("b");
        for (a1, v) in [("A1", 1.0), ("C2", 2.0), ("B7", 3.0), ("H3", 4.0)] {
            link.set("s", at(a1), Value::Number(v));
        }
        let sheet = link.sheet("s").unwrap();
        let cols = RangeRef::cols(0, 2);
        assert_eq!(sheet.filled_end(cols), Some(at("C7")));
        assert_eq!(sheet.cells_in(cols).map(|(c, _)| c.a1()).collect::<Vec<_>>(), ["A1", "C2", "B7"]);
        assert_eq!(sheet.filled_end(RangeRef::new(at("D4"), at("F9"))), None);
    }

    #[test]
    fn a_link_survives_json_and_an_older_workbook_reads_as_having_none() {
        let mut link = ExternalLink::named("Budget.xlsx");
        link.target = Some("../data/Budget.xlsx".into());
        link.set("Sheet1", at("B4"), Value::Number(7.0));
        let json = serde_json::to_string(&link).unwrap_or_default();
        assert_eq!(serde_json::from_str::<ExternalLink>(&json).ok(), Some(link));
        let mut wb = crate::Workbook::new();
        let plain = serde_json::to_string(&wb).unwrap_or_default();
        assert!(!plain.contains("\"links\":"), "a workbook without links writes none");
        assert_eq!(serde_json::from_str::<crate::Workbook>(&plain).map(|w| w.links.len()).ok(), Some(0));
        wb.links.push(Arc::new(ExternalLink::named("Book2")));
        let with = serde_json::to_string(&wb).unwrap_or_default();
        assert_eq!(serde_json::from_str::<crate::Workbook>(&with).ok().and_then(|w| w.links.first().map(|l| l.name.clone())), Some("Book2".into()));
    }
}
