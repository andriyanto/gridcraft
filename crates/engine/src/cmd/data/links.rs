//! Data › Edit Links: the other workbooks this one's formulas read, and what can be done about
//! each (`docs/external-references.md`, stages B and C).
//!
//! A link is another workbook's name, where it is, and the values this workbook read of it
//! ([`gridcraft_model::links`]). The session keeps those values current while the other workbook
//! is open ([`crate::book_links`]). The commands here are for the ones that are not: the list
//! says which they are, Change Source points the formulas at another workbook, and Break Link
//! leaves their values and takes the formulas away.

use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_formula::{SheetSel, same_name};
use gridcraft_model::{Formula, Workbook};
use serde_json::{Value as Json, json};

use crate::cmd::{CommandSpec, bad, cmd, edit, has_doc, str_param};
use crate::{Result, Session};

/// Most values Break Link writes for the results of formulas that fill many cells.
const MAX_BROKEN_VALUES: u64 = 2_000_000;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "data.editLinks", "Edit Links", ["Data", "Queries & Connections"], None, "{} → [{name, target, path, status: \"open\" | \"saved\" | \"unknown\", sheets, values, reads, fromFile}]: the other workbooks this one's formulas read (`saved`: the values the file has; `unknown`: none)", has_doc, list),
        cmd!(
            "data.changeLinkSource",
            "Change Source",
            ["Data", "Queries & Connections"],
            None,
            "{link: \"Budget.xlsx\", path: a file | name: an open workbook} → {name}: the formulas that read `link` read that workbook instead",
            has_doc,
            change_source
        ),
        cmd!(
            "data.breakLink",
            "Break Link",
            ["Data", "Queries & Connections"],
            None,
            "{link} → {cells, names: [defined names that still read it]}: the formulas that read the workbook become their values",
            has_doc,
            break_link
        ),
    ]
}

/// The name of the active workbook's link that `p.link` asks for, as the link spells it.
fn link_param(s: &Session, p: &Json, id: &str) -> Result<String> {
    let asked = str_param(p, "link").ok_or_else(|| bad(id, "missing `link`: the linked workbook's name"))?;
    gridcraft_model::links::find(&s.doc()?.wb.links, asked)
        .map(|l| l.name.clone())
        .ok_or_else(|| bad(id, format!("this workbook has no link to a workbook called \"{asked}\"")))
}

fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let reader = s.active_index();
    let d = s.doc()?;
    let folder = d.wb.source_dir.as_deref();
    let links = d.wb.links.iter().map(|link| {
        let source = s.source_of(reader, &link.name);
        let status = match source {
            Some(_) => "open",
            None if link.sheets.is_empty() => "unknown",
            None => "saved",
        };
        // Where the other workbook is: the file it is open from, else what the link's path
        // names (text: not looked for).
        let path = source
            .and_then(crate::book_links::file_of)
            .or_else(|| link.target.as_deref().and_then(|t| gridcraft_xlsx::link_path(t, folder)))
            .map(|p| p.to_string_lossy().into_owned());
        json!({
            "name": link.name,
            "target": link.target,
            "path": path,
            "status": status,
            "sheets": link.sheets.len(),
            "values": link.cell_count(),
            "reads": d.links.reads(&link.name),
            "fromFile": link.number.is_some(),
        })
    });
    Ok(Json::Array(links.collect()))
}

/// Takes the link at `at` out of `wb`. One the workbook's file brought also leaves the content
/// kept from that file, when that can be done without renumbering the file's other links.
fn drop_link(wb: &mut Workbook, at: usize) {
    if at >= wb.links.len() {
        return;
    }
    let link = wb.links.remove(at);
    if let Some(kept) = link.number.and_then(|n| gridcraft_xlsx::forget_link(wb, n)) {
        wb.preserved = Some(Arc::new(kept));
    }
}

fn change_source(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "data.changeLinkSource";
    let old = link_param(s, p, ID)?;
    let d = s.doc()?;
    let (name, target) = match (str_param(p, "path"), str_param(p, "name")) {
        (Some(path), _) => {
            let file = crate::io::source_path(path).filter(|f| f.is_absolute()).ok_or_else(|| bad(ID, "`path` is not a file's path"))?;
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| bad(ID, "`path` names no file"))?;
            // Relative to this workbook's folder when it has one; a save writes it so anyway.
            let relative = d.wb.source_dir.as_deref().and_then(|folder| gridcraft_xlsx::link_target(&file, folder));
            (name, relative.or_else(|| gridcraft_xlsx::link_target_absolute(&file)))
        }
        (None, Some(name)) => (name.to_string(), None),
        (None, None) => return Err(bad(ID, "give the new source: `path` (a file) or `name` (an open workbook)")),
    };
    if !SheetSel::can_name_book(&name) {
        return Err(bad(ID, format!("a formula can't name a workbook called \"{name}\"")));
    }
    if same_name(&name, &d.display_title()) {
        return Err(bad(ID, "a workbook's formulas read its own cells without a link"));
    }
    edit(s, |cx| {
        let at = cx.wb.links.iter().position(|l| same_name(&l.name, &old)).ok_or_else(|| bad(ID, "the link is gone"))?;
        if !same_name(&old, &name) {
            cx.wb.rename_books(&|book| same_name(book, &old).then(|| name.clone()));
        }
        // A link to that workbook already: the formulas read that one from now on.
        let other = cx.wb.links.iter().position(|l| same_name(&l.name, &name)).filter(|o| *o != at);
        match (other, cx.wb.links.get_mut(at)) {
            (Some(_), _) => drop_link(&mut cx.wb, at),
            (None, Some(link)) => {
                let link = Arc::make_mut(link);
                link.name.clone_from(&name);
                link.target = target;
                // Nothing is known of the new workbook until it is open here, or read.
                link.sheets.clear();
            }
            (None, None) => {}
        }
        cx.structural = true;
        Ok(json!({"name": name}))
    })
}

/// Whether `formula` reads the workbook `book`.
fn reads(formula: &Formula, book: &str) -> bool {
    crate::book_links::may_read(&formula.text)
        && formula
            .with_expr(|e| {
                let mut found = Vec::new();
                gridcraft_formula::adjust::external_refs(e, &mut found);
                found.iter().any(|(b, _, _)| same_name(b, book))
            })
            .unwrap_or(false)
}

fn break_link(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "data.breakLink";
    let name = link_param(s, p, ID)?;
    edit(s, |cx| {
        let mut cells = 0u64;
        let mut written = 0u64;
        for si in 0..cx.wb.sheets.len() {
            let Some(sheet) = cx.wb.sheet(si) else { continue };
            // Each formula that reads the workbook, with the cells its result fills.
            let found: Vec<(CellRef, Option<RangeRef>)> = sheet
                .cells
                .iter()
                .filter(|(_, cell)| cell.formula.as_deref().is_some_and(|f| reads(f, &name)))
                .map(|(c, _)| (c, sheet.spill_ranges.get(&c).copied().filter(|r| !r.is_single())))
                .collect();
            let mut values: Vec<(CellRef, Value)> = Vec::new();
            for (c, spill) in found {
                cells += 1;
                let range = spill.unwrap_or(RangeRef::cell(c));
                written = written.saturating_add(range.count());
                if written > MAX_BROKEN_VALUES {
                    return Err(bad(ID, format!("the formulas that read \"{name}\" fill more than {MAX_BROKEN_VALUES} cells")));
                }
                for row in range.start.row..=range.end.row {
                    for col in range.start.col..=range.end.col {
                        let at = CellRef::new(row, col);
                        values.push((at, sheet.value(at)));
                    }
                }
            }
            let sheet = cx.sheet_mut(si)?;
            for (at, value) in values {
                sheet.set_value(at, value);
            }
        }
        // A defined name is not a value: one that reads the workbook stays, and the link with
        // it, as the name still needs its values.
        let reading = |formula: &str| {
            crate::book_links::may_read(formula)
                && gridcraft_formula::parse_name(formula).is_ok_and(|e| {
                    let mut found = Vec::new();
                    gridcraft_formula::adjust::external_refs(&e, &mut found);
                    found.iter().any(|(b, _, _)| same_name(b, &name))
                })
        };
        let names: Vec<String> = cx.wb.names.iter().filter(|n| reading(&n.formula)).map(|n| n.name.clone()).collect();
        if names.is_empty()
            && let Some(at) = cx.wb.links.iter().position(|l| same_name(&l.name, &name))
        {
            drop_link(&mut cx.wb, at);
        }
        cx.structural = true;
        Ok(json!({"cells": cells, "names": names}))
    })
}
