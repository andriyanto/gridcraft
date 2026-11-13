//! A sheet with more merged ranges than a parsed XML tree may hold elements.

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Workbook};

use crate::{read_xlsx, write_xlsx};

/// Merge Across on two whole columns makes one merged range per row: 1,048,576, more than
/// `xml::MAX_XML_NODES`. The reader took its own writer's file for a damaged one, kept the rows
/// and dropped everything the sheet part holds after them: the merged ranges, and with them the
/// print options, tables and charts.
#[test]
fn a_merged_range_on_every_row_reads_back() {
    let mut wb = Workbook::new();
    {
        let sheet = wb.sheet_mut(0).unwrap();
        sheet.cells.set(CellRef::new(0, 0), Cell::value(Value::number(1.0)));
        sheet.merges = (0..gridcraft_core::MAX_ROWS).map(|r| RangeRef::new(CellRef::new(r, 0), CellRef::new(r, 1))).collect();
        // Written after the merged ranges in the sheet part.
        sheet.print.gridlines = true;
    }
    assert!(wb.sheet(0).unwrap().merges.len() > crate::xml::MAX_XML_NODES);
    let bytes = write_xlsx(&wb).unwrap();
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let (sheet, read) = (wb.sheet(0).unwrap(), back.sheet(0).unwrap());
    assert_eq!(read.merges.len(), sheet.merges.len());
    assert_eq!(read.merges.first(), sheet.merges.first());
    assert_eq!(read.merges.last(), sheet.merges.last());
    assert!(read.print.gridlines, "what follows the merged ranges is read too");
    assert_eq!(read.cells.get(CellRef::new(0, 0)).map(|c| c.value.clone()), Some(Value::number(1.0)));
}
