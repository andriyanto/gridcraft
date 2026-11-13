//! Values the incremental recalculation left stale, found by soaking `command_sequence_fuzz`
//! (seeds 80, 117, 156 and 177 at 120 steps). After each sequence every value in memory must be
//! the one a full recalculation gives.

use std::collections::BTreeSet;

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use gridcraft_engine::io;
use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

/// The workbook after `steps` on `sample` (a step that fails is skipped, as in the fuzzer).
fn after(sample: &str, steps: &[(&str, Json)]) -> Workbook {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": sample})).unwrap();
    for (id, params) in steps {
        let _ = s.execute(id, params.clone());
    }
    (*s.doc().unwrap().wb).clone()
}

/// Every cell whose value in memory after `steps` differs from a full recalculation's.
fn stale(sample: &str, steps: &[(&str, Json)]) -> Vec<String> {
    let wb = after(sample, steps);
    let mut fresh = wb.clone();
    Calc::new().recalc_all(&mut fresh);
    differences(&wb, &fresh, "a full recalc")
}

/// Every cell whose value in memory after `steps` differs from the one the workbook has once
/// saved as XLSX, opened and calculated.
fn changed_by_reload(sample: &str, steps: &[(&str, Json)]) -> Vec<String> {
    let wb = after(sample, steps);
    let bytes = io::save_bytes(&wb, "book.xlsx").unwrap();
    let (mut back, _) = io::open_bytes("book.xlsx", &bytes).unwrap();
    Calc::new().recalc_all(&mut back);
    differences(&wb, &back, "a save and reload")
}

fn differences(wb: &Workbook, fresh: &Workbook, what: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (x, y) in wb.sheets.iter().zip(&fresh.sheets) {
        let mut keys: BTreeSet<CellRef> = BTreeSet::new();
        for sh in [x, y] {
            keys.extend(sh.cells.iter().map(|(c, _)| c));
            keys.extend(sh.spill.keys().copied());
        }
        for c in keys {
            let (a, b) = (x.value(c), y.value(c));
            if a != b {
                out.push(format!("{}!{c}: {a:?} in memory, {b:?} after {what}", x.name));
            }
        }
    }
    out
}

#[test]
fn a_criteria_count_over_a_column_follows_an_array_formula_entered_in_it() {
    let diff = stale(
        "grades",
        &[
            ("range.setValues", json!({"range":"J2","values":[["$1,234.50"],["=XLOOKUP(J9,F1:F20,F1:F20)"],["=FILTER(F1:F20,F1:F20>3)"]]})),
            ("home.mergeCells", json!({"range":"I:J"})),
            ("cell.set", json!({"cell":"L25","input":"=COUNTIF(J:J,\">0\")"})),
            ("selection.set", json!({"range":"J1:K1"})),
            ("cell.set", json!({"array":true,"cell":"J5","input":"=FILTER(I1:I20,I1:I20>5)","sheet":0})),
        ],
    );
    assert!(diff.is_empty(), "{diff:#?}");
}

#[test]
fn a_fill_of_spilling_formulas_leaves_no_stale_spill_error() {
    let diff = stale(
        "sales",
        &[
            ("selection.set", json!({"range":"C20:D26"})),
            ("range.fill", json!({"input":"=SORT(B16:D20)","range":"K14:K19"})),
            ("cell.set", json!({"cell":"K68","input":"=B11*A8"})),
        ],
    );
    assert!(diff.is_empty(), "{diff:#?}");
}

#[test]
fn a_fill_over_spilled_values_recalculates_what_read_them() {
    let diff = stale(
        "sales",
        &[
            ("range.setValues", json!({"range":"F15","values":[[true,"=1/0","43"],[-1,"=F10#","=D5:H8"]]})),
            ("range.fill", json!({"input":"=D22:F27","range":"H12:L15"})),
        ],
    );
    assert!(diff.is_empty(), "{diff:#?}");
}

/// The array that had spilled keeps its cells when the workbook is opened again, although the
/// one that wants them too comes first in the sheet.
#[test]
fn spilled_arrays_keep_their_place_through_a_save_and_reload() {
    let diff = changed_by_reload(
        "sales",
        &[
            ("range.setValues", json!({"range":"F15","values":[[true,"=1/0","43"],[-1,"=F10#","=D5:H8"]]})),
            ("range.fill", json!({"input":"=D22:F27","range":"H12:L15"})),
        ],
    );
    assert!(diff.is_empty(), "{diff:#?}");
}

#[test]
fn fill_up_over_spilling_formulas_recalculates_their_readers() {
    let diff = stale(
        "sales",
        &[
            (
                "range.setValues",
                json!({"range":"E17","values":[[125,"1e3","17.60"],["'007",154,113],["=OFFSET(K8,1,1)","=XLOOKUP(G15,F1:F20,F1:F20)","=INDIRECT(\"I4\")"],["=I10:K12","2026-09-02","=TRANSPOSE(E19:I22)"]]}),
            ),
            ("edit.fillUp", json!({"range":"K13:O18"})),
        ],
    );
    assert!(diff.is_empty(), "{diff:#?}");
}
