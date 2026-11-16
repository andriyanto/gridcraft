//! Formulas a file holds that the parser refuses: longer than 8,192 characters, nested too
//! deeply, or not a formula. The cell keeps the text and shows the value the file saved for it,
//! which nothing recalculates. That used to happen without a word: a file could show a number its
//! formula does not give. Reading it now warns, once a sheet.

use gridcraft_core::{CellRef, Value};

use crate::read_xlsx;

fn cell(at: &str, formula: &str, cached: &str) -> String {
    format!(r#"<c r="{at}"><f>{formula}</f><v>{cached}</v></c>"#)
}

/// The workbook read from a one-sheet file with these rows, and the warnings about its formulas.
fn read(rows: &str) -> (gridcraft_model::Workbook, Vec<String>) {
    let bytes = super::minimal(&format!("<sheetData>{rows}</sheetData>"), &[], "", "");
    let (wb, report) = read_xlsx(&bytes).unwrap();
    // The package has no styles part, which the reader mentions too.
    (wb, report.warnings.into_iter().filter(|w| w != "no styles part").collect())
}

#[test]
fn a_formula_past_the_limits_is_reported_and_keeps_its_saved_value() {
    // 9,001 characters; it would give 4,501.
    let long = format!("1{}", "+1".repeat(4500));
    let (wb, warnings) = read(&format!(r#"<row r="1">{}{}</row>"#, cell("A1", &long, "777"), cell("B1", "A1*2", "1554")));
    let sheet = &wb.sheets[0];
    let a1 = sheet.cell(CellRef::new(0, 0)).unwrap();
    assert_eq!(a1.value, Value::Number(777.0), "the value the file saved");
    assert_eq!(a1.formula.as_ref().map(|f| (f.text.len(), f.expr.is_some())), Some((9001, false)), "the text is kept, unparsed");
    assert_eq!(
        warnings,
        [
            "sheet \"Data\": the formula in A1 could not be read (formula is longer than 8192 characters); its cell shows the value saved in the file, which is not recalculated"
        ]
    );
}

#[test]
fn every_kind_of_unread_formula_is_counted_in_one_warning_a_sheet() {
    let long = format!("1{}", "+1".repeat(4500));
    let deep = format!("{}1{}", "(".repeat(300), ")".repeat(300));
    let rows = format!(
        r#"<row r="1">{}{}</row><row r="2">{}{}</row>"#,
        cell("A1", "SUM(1,2)", "3"),
        cell("C1", &deep, "888"),
        cell("B2", &long, "777"),
        cell("D2", "1+*2", "5"),
    );
    let (wb, warnings) = read(&rows);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 2)), Value::Number(888.0));
    assert_eq!(
        warnings,
        [
            "sheet \"Data\": 3 formulas could not be read (the first, in C1: formula is nested too deeply); their cells show the values saved in the file, which is not recalculated"
        ]
    );
}

#[test]
fn formulas_that_parse_bring_no_warning() {
    let at_limit = format!("1{}", "+1".repeat(4095));
    assert_eq!(at_limit.len(), 8191);
    let (_, warnings) = read(&format!(r#"<row r="1">{}{}</row>"#, cell("A1", &at_limit, "4096"), cell("B1", "A1*2", "8192")));
    assert_eq!(warnings, Vec::<String>::new());
}
