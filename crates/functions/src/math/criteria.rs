use std::borrow::Cow;
use std::sync::Arc;

use super::{Arg, Array, CellError, Criterion, Ctx, R, Value, as_array, has, num_val};
use crate::criteria::CritKey;
use crate::memo::{CritIndex, remembered, shared};

// ---------------------------------------------------------------------------------------------
// Criteria aggregates

/// The cells that meet every criterion: each cell's answer, or the positions of the ones that do
/// (row-major, ascending).
enum Matched {
    Mask(Vec<bool>),
    At(Vec<u32>),
}

impl Matched {
    fn count(&self) -> usize {
        match self {
            Matched::Mask(mask) => mask.iter().filter(|m| **m).count(),
            Matched::At(at) => at.len(),
        }
    }
}

/// The index of the range `src` holds and the key `crit` has in it, when the criterion is one an
/// index answers and the range one that many formulas test (see [`crate::memo`]): the cells that
/// may match are then the index's candidates for the key, and no others.
fn indexed(c: &mut dyn Ctx, src: &Value, crit: &Criterion) -> Option<(Arc<CritIndex>, CritKey)> {
    let key = crit.index_key()?;
    let range = shared(src)?;
    let index = c.memo()?.criteria(range)?;
    Some((index, key))
}

/// The cells of the `rows` x `cols` ranges that meet every (range, criteria) pair from
/// `first_pair` on.
fn matched(a: &[Arg], first_pair: usize, rows: usize, cols: usize, c: &mut dyn Ctx) -> R<Matched> {
    let rest = a.get(first_pair..).unwrap_or(&[]);
    if rest.is_empty() || rest.len() % 2 != 0 {
        return Err(CellError::Value);
    }
    // A text criterion over 255 characters is `#VALUE!` only when there is a single criteria
    // pair. With two or more pairs Excel matches its first 255 characters instead.
    let single_pair = rest.len() == 2;
    let mut pairs: Vec<(&Value, Cow<'_, Array>, Criterion)> = Vec::with_capacity(rest.len() / 2);
    for pair in rest.chunks(2) {
        let (Some(range), Some(crit)) = (pair.first(), pair.get(1)) else { return Err(CellError::Value) };
        let r = as_array(&range.value);
        if r.rows != rows || r.cols != cols {
            return Err(CellError::Value);
        }
        let scalar = crit.value.scalar();
        let crit = if single_pair { Criterion::parse_arg(&scalar)? } else { Criterion::parse_arg_truncated(&scalar) };
        pairs.push((&range.value, r, crit));
    }
    // With an index for one of the pairs, only its candidates are tested: the pair with the
    // fewest, against every pair (its own too: the index names cells that may match).
    let mut fewest: Option<(Arc<CritIndex>, CritKey)> = None;
    for (src, _, crit) in &pairs {
        if let Some((index, key)) = indexed(c, src, crit)
            && fewest.as_ref().is_none_or(|(i, k)| index.candidates(&key).len() < i.candidates(k).len())
        {
            fewest = Some((index, key));
        }
    }
    if let Some((index, key)) = fewest {
        let meets = |p: u32| pairs.iter().all(|(_, r, crit)| r.data.get(p as usize).is_some_and(|v| crit.matches(v)));
        return Ok(Matched::At(index.candidates(&key).iter().copied().filter(|p| meets(*p)).collect()));
    }
    let mut mask = vec![true; rows.saturating_mul(cols)];
    for (_, r, crit) in &pairs {
        for (m, v) in mask.iter_mut().zip(r.iter()) {
            if *m && !crit.matches(v) {
                *m = false;
            }
        }
    }
    Ok(Matched::Mask(mask))
}

/// Numbers in `target` at the matched cells, in row-major order (errors there propagate).
fn matched_numbers(target: &Array, matched: &Matched) -> R<Vec<f64>> {
    let mut out = Vec::new();
    let mut take = |v: &Value| match v {
        Value::Number(n) => {
            out.push(*n);
            Ok(())
        }
        Value::Error(e) => Err(*e),
        _ => Ok(()),
    };
    match matched {
        Matched::Mask(mask) => {
            for (v, m) in target.iter().zip(mask.iter()) {
                if *m {
                    take(v)?;
                }
            }
        }
        Matched::At(at) => {
            for p in at {
                if let Some(v) = target.data.get(*p as usize) {
                    take(v)?;
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn countifs(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("COUNTIFS", a, c, |a, c| {
        let first = as_array(&crate::util::arg(a, 0)?.value);
        num_val(matched(a, 0, first.rows, first.cols, c)?.count() as f64)
    })
}

/// The values of the `*IFS` target range at the cells its pairs match.
fn ifs_numbers(a: &[Arg], c: &mut dyn Ctx) -> R<Vec<f64>> {
    let target = as_array(&crate::util::arg(a, 0)?.value);
    let matched = matched(a, 1, target.rows, target.cols, c)?;
    matched_numbers(&target, &matched)
}

pub(super) fn sumifs(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("SUMIFS", a, c, |a, c| num_val(ifs_numbers(a, c)?.iter().sum()))
}

pub(super) fn averageifs(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("AVERAGEIFS", a, c, |a, c| {
        let v = ifs_numbers(a, c)?;
        if v.is_empty() {
            return Err(CellError::Div0);
        }
        num_val(v.iter().sum::<f64>() / v.len() as f64)
    })
}

pub(super) fn maxifs(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("MAXIFS", a, c, |a, c| num_val(ifs_numbers(a, c)?.iter().copied().reduce(f64::max).unwrap_or(0.0)))
}

pub(super) fn minifs(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("MINIFS", a, c, |a, c| num_val(ifs_numbers(a, c)?.iter().copied().reduce(f64::min).unwrap_or(0.0)))
}

pub(super) fn countif(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("COUNTIF", a, c, |a, c| {
        let src = &crate::util::arg(a, 0)?.value;
        let range = as_array(src);
        let crit = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
        let n = match indexed(c, src, &crit) {
            Some((index, key)) => index.candidates(&key).iter().filter(|p| range.data.get(**p as usize).is_some_and(|v| crit.matches(v))).count(),
            None => range.iter().filter(|v| crit.matches(v)).count(),
        };
        num_val(n as f64)
    })
}

/// SUMIF / AVERAGEIF: matched numbers from the sum range, read at the criteria range's positions.
/// Excel sizes a sum range reference like the criteria range, anchored at its own top-left cell;
/// the evaluator does that resizing (it has the references), so cells line up one to one here.
fn if_numbers(a: &[Arg], c: &mut dyn Ctx) -> R<Vec<f64>> {
    let src = &crate::util::arg(a, 0)?.value;
    let range = as_array(src);
    let crit = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
    let sum_range = if has(a, 2) { Some(as_array(&crate::util::arg(a, 2)?.value)) } else { None };
    let mut out = Vec::new();
    // One cell of the criteria range, by its row-major position.
    let mut cell = |p: usize| -> R<()> {
        let (r, col) = (p / range.cols, p % range.cols);
        let Some(v) = range.get(r, col) else { return Ok(()) };
        if !crit.matches(v) {
            return Ok(());
        }
        let target = match &sum_range {
            Some(s) => s.get(r, col),
            None => Some(v),
        };
        match target {
            Some(Value::Number(n)) => out.push(*n),
            Some(Value::Error(e)) => return Err(*e),
            _ => {}
        }
        Ok(())
    };
    match indexed(c, src, &crit) {
        Some((index, key)) => {
            for p in index.candidates(&key) {
                cell(*p as usize)?;
            }
        }
        None => {
            for p in 0..range.data.len() {
                cell(p)?;
            }
        }
    }
    Ok(out)
}

pub(super) fn sumif(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("SUMIF", a, c, |a, c| num_val(if_numbers(a, c)?.iter().sum()))
}

pub(super) fn averageif(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    remembered("AVERAGEIF", a, c, |a, c| {
        let v = if_numbers(a, c)?;
        if v.is_empty() {
            return Err(CellError::Div0);
        }
        num_val(v.iter().sum::<f64>() / v.len() as f64)
    })
}
