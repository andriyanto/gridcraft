//! A whole column or row read as an array (`=A:A`, `=TRANSPOSE(4:4)`, `=A:A*B:B`). It holds a
//! million cells, so it is cut down to its filled cells, and it used to be cut to the sheet's
//! used range instead: a value typed anywhere else on the sheet made the array longer, but only
//! at the next full recalculation, because nothing that changed was read by the formula. Found by
//! soaking `command_sequence_fuzz` (seed 26 at 120 steps).

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use serde_json::{Value as Json, json};

fn workbook() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 10], [2, 20], [3, 30]]})).unwrap();
    s
}

fn run(s: &mut Session, id: &str, params: Json) {
    s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn value(s: &Session, cell: &str) -> Value {
    s.doc().unwrap().wb.sheets[0].value(CellRef::parse(cell).unwrap())
}

/// The cells the array formula in `anchor` fills.
fn spill(s: &Session, anchor: &str) -> String {
    let anchor = CellRef::parse(anchor).unwrap();
    let range = s.doc().unwrap().wb.sheets[0].spill_ranges.get(&anchor).copied().unwrap_or(RangeRef::new(anchor, anchor));
    range.to_string()
}

/// Every cell whose value in memory differs from the one a full recalculation gives.
fn stale(s: &Session) -> Vec<String> {
    let wb = &s.doc().unwrap().wb;
    let mut fresh = (**wb).clone();
    Calc::new().recalc_all(&mut fresh);
    let mut out = Vec::new();
    for (x, y) in wb.sheets.iter().zip(&fresh.sheets) {
        let mut cells: Vec<CellRef> = x.cells.iter().chain(y.cells.iter()).map(|(c, _)| c).collect();
        cells.extend(x.spill.keys().chain(y.spill.keys()).copied());
        cells.sort();
        cells.dedup();
        for c in cells {
            let (a, b) = (x.value(c), y.value(c));
            if a != b {
                out.push(format!("{}!{c}: {a:?} in memory, {b:?} after a full recalc", x.name));
            }
        }
    }
    out
}

#[test]
fn a_column_read_as_an_array_ends_where_its_own_values_end() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=A:A"}));
    assert_eq!(spill(&s, "D1"), "D1:D3");
    // A value somewhere else on the sheet is not part of column A.
    run(&mut s, "cell.set", json!({"cell": "F40", "input": "elsewhere"}));
    assert_eq!(spill(&s, "D1"), "D1:D3");
    assert_eq!(stale(&s), Vec::<String>::new());
    // One in the column is, blanks above it included.
    run(&mut s, "cell.set", json!({"cell": "A6", "input": "6"}));
    assert_eq!(spill(&s, "D1"), "D1:D6");
    assert_eq!((value(&s, "D5"), value(&s, "D6")), (Value::Number(0.0), Value::Number(6.0)));
    assert_eq!(stale(&s), Vec::<String>::new());
    run(&mut s, "edit.clearContents", json!({"range": "A6"}));
    assert_eq!(spill(&s, "D1"), "D1:D3");
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// Rows work the same way (two of them: one whole row is small enough to keep its full size).
#[test]
fn rows_read_as_an_array_end_where_their_own_values_end() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"cell": "D5", "input": "=TRANSPOSE(1:2)"}));
    assert_eq!(spill(&s, "D5"), "D5:E6");
    run(&mut s, "cell.set", json!({"cell": "J9", "input": "elsewhere"}));
    assert_eq!(spill(&s, "D5"), "D5:E6");
    assert_eq!(stale(&s), Vec::<String>::new());
    run(&mut s, "cell.set", json!({"cell": "C2", "input": "200"}));
    assert_eq!(spill(&s, "D5"), "D5:E7");
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// The fuzzer's sequence: the replace turns `D1:F1` into the whole columns `DB:FB`, and the fill
/// puts values in columns the formula doesn't read.
#[test]
fn a_fill_beside_whole_columns_read_as_an_array_leaves_the_array_as_it_is() {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "grades"})).unwrap();
    for (id, params) in [
        ("cell.set", json!({"cell": "H1", "input": "=TRANSPOSE(D1:F1)"})),
        ("edit.replace", json!({"all": true, "what": "1", "with": "B"})),
        ("range.fill", json!({"input": "=COUNTA(Grades[#All])", "range": "I23:K24"})),
    ] {
        let _ = s.execute(id, params);
    }
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// Two columns of one formula are cut at the same row, the longer one's, so that they still pair
/// up cell by cell.
#[test]
fn columns_read_together_keep_the_same_length() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"cell": "B5", "input": "50"}));
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=A:A*B:B"}));
    assert_eq!(spill(&s, "D1"), "D1:D5");
    assert_eq!((value(&s, "D3"), value(&s, "D5")), (Value::Number(90.0), Value::Number(0.0)));
    run(&mut s, "cell.set", json!({"cell": "F1", "input": "=SUMPRODUCT(A:A,B:B)"}));
    assert_eq!(value(&s, "F1"), Value::Number(140.0));
    run(&mut s, "cell.set", json!({"cell": "A8", "input": "8"}));
    run(&mut s, "cell.set", json!({"cell": "B8", "input": "2"}));
    assert_eq!(spill(&s, "D1"), "D1:D8");
    assert_eq!(value(&s, "F1"), Value::Number(156.0));
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// A cell that only carries a format holds nothing to read.
#[test]
fn a_formatted_blank_cell_does_not_make_the_array_longer() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=A:A"}));
    run(&mut s, "home.bold", json!({"range": "A9"}));
    assert_eq!(spill(&s, "D1"), "D1:D3");
    assert_eq!(stale(&s), Vec::<String>::new());
}

/// What another formula spills into the column counts as the column's values.
#[test]
fn values_spilled_into_the_column_are_part_of_it() {
    let mut s = workbook();
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=A:A"}));
    run(&mut s, "cell.set", json!({"cell": "A5", "input": "=SEQUENCE(4)"}));
    assert_eq!(spill(&s, "D1"), "D1:D8");
    assert_eq!(value(&s, "D8"), Value::Number(4.0));
    assert_eq!(stale(&s), Vec::<String>::new());
    run(&mut s, "cell.set", json!({"cell": "A5", "input": "=SEQUENCE(2)"}));
    assert_eq!(spill(&s, "D1"), "D1:D6");
    assert_eq!(stale(&s), Vec::<String>::new());
}
