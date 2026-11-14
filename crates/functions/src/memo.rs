//! What one recalculation remembers between function calls.
//!
//! A column of `VLOOKUP(x, table, …)` or `COUNTIF(range, x)` calls reads the same range in every
//! cell. The calculation engine hands each call the same shared array for it, and the functions
//! keep here what they worked out about that array: a table's key column, where each key first
//! and last occurs, which cells hold each value, and the results of calls made with the very same
//! arguments. Searching the range once per cell made such a column quadratic in its length.
//!
//! Nothing here changes a result. An index only names candidate cells, which are then compared
//! as they always were, and a remembered result is that of a function whose value depends on its
//! arguments alone. Arrays are told apart by address, and each one is kept alive while it is
//! remembered, so an address never comes to mean another array.

use std::collections::HashMap;
use std::sync::Arc;

use gridcraft_core::{Array, CellError, Value};

use crate::Arg;
use crate::criteria::{CritKey, cell_keys, lookup_equal};

/// Shortest line or range anything is remembered for: a search of fewer entries costs less than
/// keeping track of it.
const MIN_LEN: usize = 32;
/// Most array entries indexed or copied in one recalculation; past it the functions search as
/// they did, so memory stays bounded however many distinct ranges a workbook's formulas read.
const MAX_ENTRIES: usize = 16_000_000;
/// Most results remembered in one recalculation.
const MAX_RESULTS: usize = 1_000_000;

/// A row or a column of an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Line {
    Col(usize),
    Row(usize),
}

/// A value as an exact search tells values apart ([`lookup_equal`] without wildcards): numbers by
/// value (both zeros alike), text without regard to case.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ExactKey {
    Empty,
    Number(u64),
    Text(String),
    Bool(bool),
    Error(CellError),
}

fn exact_key(v: &Value) -> Option<ExactKey> {
    Some(match v {
        Value::Empty => ExactKey::Empty,
        Value::Number(n) if n.is_nan() => return None,
        Value::Number(n) => ExactKey::Number(if *n == 0.0 { 0 } else { n.to_bits() }),
        // Text that compares equal has the same lowercase characters (`compare_text`'s last level).
        Value::Text(t) => ExactKey::Text(t.chars().flat_map(char::to_lowercase).collect()),
        Value::Bool(b) => ExactKey::Bool(*b),
        Value::Error(e) => ExactKey::Error(*e),
        Value::Array(_) => return None,
    })
}

/// Where each key of a line first and last occurs.
pub(crate) struct ExactIndex {
    ends: HashMap<ExactKey, (u32, u32)>,
}

impl ExactIndex {
    fn build(hay: &[Value]) -> Option<ExactIndex> {
        if u32::try_from(hay.len()).is_err() {
            return None;
        }
        let mut ends: HashMap<ExactKey, (u32, u32)> = HashMap::new();
        for (i, v) in hay.iter().enumerate() {
            // An entry with no key (a nested array) matches nothing an exact search looks for.
            let Some(key) = exact_key(v) else { continue };
            let i = i as u32;
            ends.entry(key).and_modify(|e| e.1 = i).or_insert((i, i));
        }
        Some(ExactIndex { ends })
    }

    /// The first (or with `reverse` the last) entry of `hay` equal to `x`, as a search of every
    /// entry finds it. The outer `None`: the index can't tell, and the caller searches.
    pub(crate) fn find(&self, x: &Value, hay: &[Value], reverse: bool) -> Option<Option<usize>> {
        let key = exact_key(x)?;
        let Some(&(first, last)) = self.ends.get(&key) else { return Some(None) };
        let at = if reverse { last } else { first } as usize;
        // Entries with one key are equal to one another, so the end one is the answer. Should
        // that ever not hold, the caller's search decides.
        hay.get(at).is_some_and(|v| lookup_equal(x, v, false)).then_some(Some(at))
    }
}

/// The cells of a range by the value an equality criterion looks for, in row-major order.
pub(crate) struct CritIndex {
    cells: HashMap<CritKey, Vec<u32>>,
}

impl CritIndex {
    fn build(a: &Array) -> Option<CritIndex> {
        if u32::try_from(a.data.len()).is_err() {
            return None;
        }
        let mut cells: HashMap<CritKey, Vec<u32>> = HashMap::new();
        for (i, v) in a.data.iter().enumerate() {
            if !cell_keys(v, &mut |key| cells.entry(key).or_default().push(i as u32)) {
                return None;
            }
        }
        Some(CritIndex { cells })
    }

    /// The cells that may match a criterion with this key, ascending: every cell that matches is
    /// among them.
    pub(crate) fn candidates(&self, key: &CritKey) -> &[u32] {
        self.cells.get(key).map_or(&[], Vec::as_slice)
    }
}

#[derive(Default)]
struct LineSlot {
    asked: u32,
    column: Option<Arc<Vec<Value>>>,
    exact: Option<Option<Arc<ExactIndex>>>,
}

#[derive(Default)]
struct CritSlot {
    asked: u32,
    index: Option<Option<Arc<CritIndex>>>,
}

/// One argument of a remembered call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ArgKey {
    Array(usize),
    Empty,
    Number(u64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
}

/// A call: the function and its arguments, each with how it was given.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CallKey(&'static str, Vec<(ArgKey, bool, bool)>);

/// See the module documentation. One per recalculation pass.
#[derive(Default)]
pub struct Memo {
    lines: HashMap<(usize, Line), LineSlot>,
    criteria: HashMap<usize, CritSlot>,
    results: HashMap<CallKey, Result<Value, CellError>>,
    /// The arrays the maps above name by address.
    kept: HashMap<usize, Arc<Array>>,
    entries: usize,
}

/// `v` when it is an array that others hold too (the engine's copy of a range read more than
/// once) and long enough to be worth remembering.
pub(crate) fn shared(v: &Value) -> Option<&Arc<Array>> {
    match v {
        Value::Array(a) if a.data.len() >= MIN_LEN && Arc::strong_count(a) > 1 => Some(a),
        _ => None,
    }
}

impl Memo {
    pub fn new() -> Memo {
        Memo::default()
    }

    fn keep(&mut self, a: &Arc<Array>) -> usize {
        let id = Arc::as_ptr(a) as usize;
        self.kept.entry(id).or_insert_with(|| Arc::clone(a));
        id
    }

    /// Whether `n` more entries fit the budget; counts them when they do.
    fn spend(&mut self, n: usize) -> bool {
        match self.entries.checked_add(n) {
            Some(total) if total <= MAX_ENTRIES => {
                self.entries = total;
                true
            }
            _ => false,
        }
    }

    /// Column `col` of `a`, copied out once.
    pub(crate) fn column(&mut self, a: &Arc<Array>, col: usize) -> Option<Arc<Vec<Value>>> {
        let id = self.keep(a);
        if let Some(c) = self.lines.get(&(id, Line::Col(col))).and_then(|s| s.column.clone()) {
            return Some(c);
        }
        if !self.spend(a.rows) {
            return None;
        }
        let column: Arc<Vec<Value>> = Arc::new((0..a.rows).map(|r| a.get(r, col).cloned().unwrap_or(Value::Empty)).collect());
        self.lines.entry((id, Line::Col(col))).or_default().column = Some(Arc::clone(&column));
        Some(column)
    }

    /// The index of `hay`, which is `line` of `a`, from the second search of it on.
    pub(crate) fn exact(&mut self, a: &Arc<Array>, line: Line, hay: &[Value]) -> Option<Arc<ExactIndex>> {
        if hay.len() < MIN_LEN {
            return None;
        }
        let id = self.keep(a);
        let slot = self.lines.entry((id, line)).or_default();
        slot.asked = slot.asked.saturating_add(1);
        if let Some(built) = &slot.exact {
            return built.clone();
        }
        if slot.asked < 2 {
            return None;
        }
        let index = if self.spend(hay.len()) { ExactIndex::build(hay).map(Arc::new) } else { None };
        self.lines.entry((id, line)).or_default().exact = Some(index.clone());
        index
    }

    /// The index of range `a`'s cells by value, from the second criterion matched against it on.
    pub(crate) fn criteria(&mut self, a: &Arc<Array>) -> Option<Arc<CritIndex>> {
        let id = self.keep(a);
        let slot = self.criteria.entry(id).or_default();
        slot.asked = slot.asked.saturating_add(1);
        if let Some(built) = &slot.index {
            return built.clone();
        }
        if slot.asked < 2 {
            return None;
        }
        let index = if self.spend(a.data.len()) { CritIndex::build(a).map(Arc::new) } else { None };
        self.criteria.entry(id).or_default().index = Some(index.clone());
        index
    }

    /// The key of a call of `name` with `args`, when it can be remembered: every array among them
    /// is a shared one. Calls with other arrays (a range read once, an array a formula computed)
    /// are not: nothing would ask for them again.
    pub(crate) fn call_key(&mut self, name: &'static str, args: &[Arg]) -> Option<CallKey> {
        let mut any = false;
        let mut keys = Vec::with_capacity(args.len());
        for a in args {
            let key = match &a.value {
                Value::Array(_) => {
                    let arr = shared(&a.value)?;
                    any = true;
                    ArgKey::Array(self.keep(arr))
                }
                Value::Empty => ArgKey::Empty,
                Value::Number(n) => ArgKey::Number(n.to_bits()),
                Value::Text(t) => ArgKey::Text(Arc::clone(t)),
                Value::Bool(b) => ArgKey::Bool(*b),
                Value::Error(e) => ArgKey::Error(*e),
            };
            keys.push((key, a.from_ref, a.blank_value));
        }
        any.then_some(CallKey(name, keys))
    }

    pub(crate) fn result(&self, key: &CallKey) -> Option<Result<Value, CellError>> {
        self.results.get(key).cloned()
    }

    pub(crate) fn remember(&mut self, key: CallKey, result: &Result<Value, CellError>) {
        if self.results.len() < MAX_RESULTS {
            self.results.insert(key, result.clone());
        }
    }
}

/// `f(args)`, or what the same call gave before in this recalculation. For functions whose result
/// depends on their arguments alone.
pub(crate) fn remembered(
    name: &'static str,
    args: &[Arg],
    c: &mut dyn crate::Ctx,
    f: impl FnOnce(&[Arg], &mut dyn crate::Ctx) -> Result<Value, CellError>,
) -> Result<Value, CellError> {
    let key = c.memo().and_then(|m| m.call_key(name, args));
    if let Some(k) = &key
        && let Some(r) = c.memo().and_then(|m| m.result(k))
    {
        return r;
    }
    let r = f(args, c);
    if let (Some(k), Some(m)) = (key, c.memo()) {
        m.remember(k, &r);
    }
    r
}

#[cfg(test)]
mod tests {
    use gridcraft_core::DateSystem;

    use super::*;
    use crate::Ctx;
    use crate::util::testutil::TestCtx;

    /// A context that remembers, as a recalculation's does.
    #[derive(Default)]
    struct Remembering {
        inner: TestCtx,
        memo: Memo,
    }

    impl Ctx for Remembering {
        fn date_system(&self) -> DateSystem {
            self.inner.date_system()
        }
        fn now_serial(&self) -> f64 {
            self.inner.now_serial()
        }
        fn random(&mut self) -> f64 {
            self.inner.random()
        }
        fn memo(&mut self) -> Option<&mut Memo> {
            Some(&mut self.memo)
        }
    }

    fn text(s: &str) -> Value {
        Value::from(s)
    }

    /// Values an index must not confuse: numbers that are equal only to 15 digits, both zeros,
    /// text that differs by case, text that reads as a number, wildcard characters, blanks,
    /// empty text, logicals and errors, each several times over.
    fn awkward() -> Vec<Value> {
        let base = vec![
            Value::Number(0.3),
            Value::Number(0.1 + 0.2),
            Value::Number(0.0),
            Value::Number(-0.0),
            Value::Number(10.0),
            Value::Number(1000.0),
            Value::Number(100.0),
            Value::Number(100.000_000_000_000_01),
            Value::Number(-3.5),
            text("10"),
            text("1e3"),
            text("1,000"),
            text(" 10 "),
            text("apple"),
            text("Apple"),
            text("APPLE"),
            text("a*b"),
            text("a?b"),
            text("~x"),
            text("Éclair"),
            text("éclair"),
            text("straße"),
            text("STRASSE"),
            text(""),
            text("TRUE"),
            text("<5"),
            Value::Empty,
            Value::Bool(true),
            Value::Bool(false),
            Value::Error(CellError::NA),
            Value::Error(CellError::Div0),
        ];
        let mut out = Vec::new();
        for round in 0..3 {
            for (i, v) in base.iter().enumerate() {
                // Different neighbours each round, so first and last occurrences differ.
                out.push(base.get((i * (round + 2) + round) % base.len()).unwrap_or(v).clone());
            }
        }
        out
    }

    fn column(v: Vec<Value>) -> Arc<Array> {
        Arc::new(Array::column(v))
    }

    /// Every search and criteria function gives, for a range many calls share, what it gives
    /// searching the range on its own: needle by needle, over values chosen to trip an index.
    #[test]
    fn shared_ranges_answer_as_ranges_searched_one_call_at_a_time() {
        let keys = awkward();
        let n = keys.len();
        let numbers: Vec<Value> =
            (0..n).map(|i| if i % 17 == 5 { Value::Error(CellError::Num) } else { Value::Number((i * 7 % 23) as f64 + 0.25) }).collect();
        let groups: Vec<Value> = (0..n).map(|i| Value::Number((i % 3) as f64)).collect();
        let table = Arc::new(Array::new(n, 2, keys.iter().cloned().zip(numbers.iter().cloned()).flat_map(|(k, v)| [k, v]).collect()).unwrap());
        let (keys_col, numbers_col, groups_col) = (column(keys.clone()), column(numbers), column(groups));
        let row = Arc::new(Array::row(keys.clone()));
        let range = |a: &Arc<Array>| Arg::reference(Value::Array(Arc::clone(a)));

        let mut needles = keys.clone();
        needles.extend(
            ["apple", "a*", "a~*b", "a?b", "<>apple", ">=3", "<5", "=10", "=", "<>", "*", "0.3", "éCLAIR", "#N/A", "false", "100", "x"].map(text),
        );
        needles.extend([0.3, 0.300_000_000_000_000_1, 1e3, 100.0, -3.5, 7.0].map(Value::Number));

        let mut ctx = Remembering::default();
        let mut calls = 0;
        for x in &needles {
            let x = || Arg::val(x.clone());
            let forms: Vec<(&str, Vec<Arg>)> = vec![
                ("VLOOKUP", vec![x(), range(&table), Arg::val(2.0), Arg::val(false)]),
                ("VLOOKUP", vec![x(), range(&table), Arg::val(2.0), Arg::val(true)]),
                ("HLOOKUP", vec![x(), range(&row), Arg::val(1.0), Arg::val(false)]),
                ("LOOKUP", vec![x(), range(&table)]),
                ("MATCH", vec![x(), range(&keys_col), Arg::val(0.0)]),
                ("MATCH", vec![x(), range(&row), Arg::val(0.0)]),
                ("MATCH", vec![x(), range(&keys_col), Arg::val(1.0)]),
                ("XLOOKUP", vec![x(), range(&keys_col), range(&numbers_col)]),
                ("XLOOKUP", vec![x(), range(&keys_col), range(&numbers_col), Arg::val("none"), Arg::val(0.0), Arg::val(-1.0)]),
                ("XLOOKUP", vec![x(), range(&keys_col), range(&numbers_col), Arg::val("none"), Arg::val(2.0)]),
                ("XLOOKUP", vec![x(), range(&keys_col), range(&numbers_col), Arg::val("none"), Arg::val(-1.0)]),
                ("XMATCH", vec![x(), range(&keys_col)]),
                ("XMATCH", vec![x(), range(&keys_col), Arg::val(0.0), Arg::val(-1.0)]),
                ("COUNTIF", vec![range(&keys_col), x()]),
                ("COUNTIF", vec![range(&table), x()]),
                ("SUMIF", vec![range(&keys_col), x(), range(&numbers_col)]),
                ("SUMIF", vec![range(&keys_col), x()]),
                ("AVERAGEIF", vec![range(&keys_col), x(), range(&groups_col)]),
                ("COUNTIFS", vec![range(&keys_col), x(), range(&groups_col), Arg::val(1.0)]),
                ("COUNTIFS", vec![range(&groups_col), Arg::val(">0"), range(&keys_col), x()]),
                ("SUMIFS", vec![range(&groups_col), range(&keys_col), x()]),
                ("SUMIFS", vec![range(&numbers_col), range(&groups_col), Arg::val(2.0), range(&keys_col), x()]),
                ("AVERAGEIFS", vec![range(&groups_col), range(&keys_col), x(), range(&groups_col), Arg::val("<2")]),
                ("MAXIFS", vec![range(&groups_col), range(&keys_col), x()]),
                ("MINIFS", vec![range(&groups_col), range(&keys_col), x()]),
            ];
            for (name, args) in forms {
                let spec = crate::lookup(name).unwrap();
                let alone = crate::call(spec, &args, &mut TestCtx::default());
                // Twice: the second call of the very same arguments is a remembered result.
                for again in 0..2 {
                    let shared = crate::call(spec, &args, &mut ctx);
                    assert_eq!(
                        format!("{shared:?}"),
                        format!("{alone:?}"),
                        "{name}{:?} (call {again})",
                        args.iter().map(|a| &a.value).collect::<Vec<_>>()
                    );
                    calls += 1;
                }
            }
        }
        assert!(calls > 2_000);
        // The comparison above is of the indexed paths, not of the same search twice.
        assert!(ctx.memo.entries > 0, "nothing was indexed");
        assert!(!ctx.memo.results.is_empty(), "no result was remembered");
        assert!(ctx.memo.lines.values().any(|s| matches!(s.exact, Some(Some(_)))), "no exact index was built");
        assert!(ctx.memo.criteria.values().any(|s| matches!(s.index, Some(Some(_)))), "no criteria index was built");
    }

    /// A range only one call reads (no one else holds its array) is not kept.
    #[test]
    fn an_array_no_one_shares_is_not_remembered() {
        let mut ctx = Remembering::default();
        let spec = crate::lookup("COUNTIF").unwrap();
        for _ in 0..3 {
            let args = vec![Arg::reference(Value::Array(column(awkward()))), Arg::val(10.0)];
            // The number 10 and the texts "10" and " 10 ", three times each.
            assert_eq!(crate::call(spec, &args, &mut ctx), Value::Number(9.0));
        }
        assert!(ctx.memo.kept.is_empty() && ctx.memo.results.is_empty());
    }
}
