//! Links to other workbooks, between a file and the model (`docs/external-references.md`).
//!
//! A file holds a link as an `externalLink` part: the sheet names of the other workbook, the
//! values this workbook read of it, and a relationship whose target is where it is. Formulas
//! name the link by its place in the workbook's `<externalReferences>`: `[1]Sheet1!A1`.
//!
//! **Reading.** The parts are kept as bytes, as before. Each one that links a workbook and is
//! understood whole is also read into [`Workbook::links`] under the other workbook's file name,
//! and the formulas that use its number are rewritten to that name. One that is not (DDE, OLE, a
//! name a formula can't hold, two links with one name, more than the limits) stays a number, and
//! its formulas keep the values the file saved.
//!
//! **Writing.** Formulas get numbers again: a link the file brought keeps its own, the links
//! made here follow in the order formulas first read them ([`Numbers`]). A kept part goes back
//! byte for byte unless the link's values, sheets or target changed; then those are rewritten
//! in it and the rest of it stays. A link made here is written as a new part, when a formula
//! reads it.
//!
//! A target is text. Nothing here opens, lists or tests for the file it names.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_formula::{fold_name, same_name};
use gridcraft_model::links::{MAX_LINK_CELLS, MAX_LINK_SHEETS, MAX_LINKS};
use gridcraft_model::{ExternalLink, LinkSheet};

use super::rebase::{decode_name, encode_name};
use super::write::{NameMap, part_rels_xml};
use super::*;

const CT_LINK: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml";
const REL_LINK_PATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath";
/// Longest file name taken as a link's name.
const MAX_NAME_CHARS: usize = 255;

// ---------------------------------------------------------------- reading

/// The kept link parts of a workbook, in the order of its `<externalReferences>`: the file's link
/// `n` is entry `n - 1`. `None` for an entry whose part is not there.
fn kept_parts(p: &Preserved) -> Vec<Option<&PreservedPart>> {
    let Some(list) = p.elements.iter().find(|e| e.slot == "externalReferences").and_then(|e| xml::parse(e.xml.as_bytes()).ok()) else {
        return Vec::new();
    };
    list.kids("externalReference")
        .map(|r| {
            let rel = p.rels.iter().find(|rel| Some(rel.id.as_str()) == r.attr("id") && !rel.external)?;
            let name = normalize(&rel.target);
            p.parts.iter().find(|part| normalize(&part.name) == name)
        })
        .collect()
}

/// What a kept link part says, when it links a workbook and all of it is understood.
struct View {
    /// Every sheet name the part lists, in its order: `sheetId` counts in this list.
    names: Vec<String>,
    /// The id of the part's relationship to the other workbook.
    rel: String,
    /// The link as the model holds it: the sheets it has values of (a sheet whose last update
    /// failed is not among them), and the target as the file writes it.
    link: ExternalLink,
}

/// The name a formula uses for the workbook a target names: its file name. `None` when a formula
/// can't hold it, or when it would read as a link's number.
fn book_name(target: &str) -> Option<String> {
    let name = decode_name(target.rsplit(['/', '\\']).next()?);
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || name.bytes().all(|b| b.is_ascii_digit()) || name.chars().any(char::is_control) {
        return None;
    }
    // It must come back from a formula as it went in.
    let e = gridcraft_formula::parse(&format!("'[{}]S'!A1", name.replace('\'', "''"))).ok()?;
    let mut read = Vec::new();
    gridcraft_formula::adjust::external_refs(&e, &mut read);
    let whole = matches!(read.as_slice(), [(book, sheet, _)] if *book == name && sheet == "S");
    (whole && gridcraft_formula::parse(&gridcraft_formula::print(&e)).ok().as_ref() == Some(&e)).then_some(name)
}

/// The value of a `<cell>` of a link part. `None` when it is not understood.
fn cell_value(cell: &xml::El) -> Option<Value> {
    let Some(v) = cell.child("v") else { return Some(Value::Empty) };
    match cell.attr("t").unwrap_or("n") {
        "n" if v.text.trim().is_empty() => Some(Value::Empty),
        "n" => v.text.trim().parse::<f64>().ok().filter(|n| n.is_finite()).map(Value::number),
        "str" => Some(Value::text(xml::decode_xstring(&v.text))),
        "b" => match v.text.trim() {
            "1" | "true" => Some(Value::Bool(true)),
            "0" | "false" => Some(Value::Bool(false)),
            _ => None,
        },
        "e" => CellError::parse(v.text.trim()).map(Value::Error),
        _ => None,
    }
}

fn view(part: &PreservedPart) -> Option<View> {
    let x = xml::parse(&part.data).ok()?;
    let book = x.child("externalBook")?;
    let rel = part.rels.iter().find(|r| Some(r.id.as_str()) == book.attr("id") && r.external && is_kind(r, &["externalLinkPath"]))?;
    let name = book_name(&rel.target)?;
    let names: Vec<String> =
        book.child("sheetNames").map(|n| n.kids("sheetName").filter_map(|s| s.attr("val")).map(str::to_string).collect()).unwrap_or_default();
    let repeated = names.iter().enumerate().any(|(i, a)| names.iter().take(i).any(|b| same_name(a, b)));
    if names.len() > MAX_LINK_SHEETS || repeated {
        return None;
    }
    let mut sheets: Vec<Option<LinkSheet>> = names.iter().map(|n| Some(LinkSheet { name: n.clone(), cells: BTreeMap::new() })).collect();
    let mut kept = 0usize;
    for data in book.child("sheetDataSet").into_iter().flat_map(|set| set.kids("sheetData")) {
        let slot = sheets.get_mut(usize::try_from(data.attr_u32("sheetId")?).ok()?)?;
        // The last update of this sheet failed: the file has no values of it.
        if data.flag("refreshError", false) {
            *slot = None;
            continue;
        }
        let sheet = slot.as_mut()?;
        for cell in data.kids("row").flat_map(|row| row.kids("cell")) {
            let at = CellRef::parse(cell.attr("r")?)?;
            let value = cell_value(cell)?;
            if value == Value::Empty {
                continue;
            }
            kept += 1;
            if kept > MAX_LINK_CELLS {
                return None;
            }
            sheet.cells.insert(at, value);
        }
    }
    let link = ExternalLink { name, target: Some(rel.target.clone()), sheets: sheets.into_iter().flatten().collect(), number: None };
    Some(View { names, rel: rel.id.clone(), link })
}

/// After a workbook is read: the links its file brought go into the model, and its formulas
/// name them instead of numbering them.
pub fn read(wb: &mut Workbook) {
    let Some(p) = wb.preserved.as_deref() else { return };
    let mut found: Vec<ExternalLink> = Vec::new();
    for (i, part) in kept_parts(p).into_iter().enumerate().take(MAX_LINKS) {
        let number = u32::try_from(i).ok().and_then(|i| i.checked_add(1));
        if let Some(v) = part.and_then(view) {
            found.push(ExternalLink { number, ..v.link });
        }
    }
    // Two links with one name (the same file name in two folders) can't be told apart by name:
    // both stay numbers.
    let single = |l: &ExternalLink| found.iter().filter(|o| same_name(&o.name, &l.name)).count() == 1;
    let links: Vec<ExternalLink> = found.iter().filter(|l| single(l)).cloned().collect();
    if links.is_empty() {
        return;
    }
    let named = |book: &str| {
        let number = book.parse::<u32>().ok().filter(|_| book.bytes().all(|b| b.is_ascii_digit()))?;
        links.iter().find(|l| l.number == Some(number)).map(|l| l.name.clone())
    };
    wb.rename_books(&named);
    wb.links = links.into_iter().map(Arc::new).collect();
}

/// The kept content of `wb` without the file's link `number`, for a link that was broken: its
/// part and its place in `<externalReferences>` go. Only the last of the file's links can go
/// this way, as formulas know the others by their place in that list. `None` for any other (it
/// keeps its place, so that the links after it keep their numbers) and for one that is not there.
pub fn forget_link(wb: &Workbook, number: u32) -> Option<Preserved> {
    let p = wb.preserved.as_deref()?;
    let at = p.elements.iter().position(|e| e.slot == "externalReferences")?;
    let list = xml::parse(p.elements.get(at)?.xml.as_bytes()).ok()?;
    let mut ids: Vec<String> = list.kids("externalReference").filter_map(|r| r.attr("id")).map(str::to_string).collect();
    if usize::try_from(number).ok() != Some(ids.len()) {
        return None;
    }
    let gone = ids.pop()?;
    let mut out = p.clone();
    out.rels.retain(|r| r.id != gone);
    if ids.is_empty() {
        out.elements.remove(at);
    } else if let Some(e) = out.elements.get_mut(at) {
        let mut x = format!("<externalReferences xmlns:r=\"{NS_REL}\">");
        for id in &ids {
            let _ = write!(x, "<externalReference r:id=\"{}\"/>", esc_attr(id));
        }
        x.push_str("</externalReferences>");
        e.xml = x;
    }
    // The parts nothing reaches any more go with it.
    let mut reached: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = out.rels.iter().chain(&out.package_rels).filter(|r| !r.external).map(|r| normalize(&r.target)).collect();
    while let Some(name) = queue.pop_front() {
        if !reached.insert(name.clone()) {
            continue;
        }
        if let Some(part) = out.parts.iter().find(|q| normalize(&q.name) == name) {
            queue.extend(part.rels.iter().filter(|r| !r.external).map(|r| normalize(&r.target)));
        }
    }
    out.parts.retain(|q| reached.contains(&normalize(&q.name)));
    Some(out)
}

// ---------------------------------------------------------------- numbers while writing

/// The numbers formulas get while one workbook is written.
#[derive(Default)]
struct Books {
    /// The links the file brought that the model has: (name, number).
    kept: Vec<(String, u32)>,
    /// How many links the file brought. The links made here are numbered after them.
    brought: u32,
    /// The links made here, in the order formulas first read them: the first is `brought + 1`.
    made: Vec<String>,
    /// The sheets formulas read of each link: (number, folded sheet name) → the name as written.
    read: BTreeMap<(u32, String), String>,
}

impl Books {
    fn number(&mut self, book: &str, sheet: &str) -> Option<u32> {
        // A number is the file's own: it stays.
        if book.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let made = |at: usize| u32::try_from(at).ok().and_then(|at| self.brought.checked_add(at)?.checked_add(1));
        let n = match self.kept.iter().find(|(name, _)| same_name(name, book)) {
            Some((_, n)) => *n,
            None => match self.made.iter().position(|m| same_name(m, book)) {
                Some(at) => made(at)?,
                None if (self.brought as usize).saturating_add(self.made.len()) < MAX_LINKS => {
                    let n = made(self.made.len())?;
                    self.made.push(book.to_string());
                    n
                }
                None => return None,
            },
        };
        if self.read.len() < MAX_LINKS.saturating_mul(MAX_LINK_SHEETS) {
            self.read.entry((n, fold_name(sheet))).or_insert_with(|| sheet.to_string());
        }
        Some(n)
    }

    /// The sheets formulas read of link `n`.
    fn read_of(&self, n: u32) -> Vec<&str> {
        self.read.iter().filter(|((m, _), _)| *m == n).map(|(_, name)| name.as_str()).collect()
    }
}

thread_local! {
    /// The numbers of the workbook being written on this thread.
    static WRITING: RefCell<Option<Books>> = const { RefCell::new(None) };
}

/// The model's links that a kept part of the file stands behind: (number, part, link).
fn held<'a>(wb: &'a Workbook, p: &'a Preserved) -> Vec<(u32, &'a PreservedPart, &'a Arc<ExternalLink>)> {
    let parts = kept_parts(p);
    let mut out: Vec<(u32, &PreservedPart, &Arc<ExternalLink>)> = Vec::new();
    for link in &wb.links {
        let Some(n) = link.number else { continue };
        let part = usize::try_from(n).ok().and_then(|n| n.checked_sub(1)).and_then(|i| parts.get(i).copied().flatten());
        if let Some(part) = part
            && !out.iter().any(|(m, _, o)| *m == n || same_name(&o.name, &link.name))
        {
            out.push((n, part, link));
        }
    }
    out
}

/// While one is alive, the formulas written on this thread name other workbooks by number
/// ([`number_of`]). Made when a workbook starts to be written and handed to [`finish`], which
/// writes the links the formulas read; dropping it (also on an early return) ends the numbering.
pub struct Numbers(std::marker::PhantomData<*const ()>);

impl Numbers {
    pub fn of(wb: &Workbook) -> Numbers {
        let mut books = Books::default();
        if let Some(p) = wb.preserved.as_deref() {
            books.brought = u32::try_from(kept_parts(p).len()).unwrap_or(u32::MAX);
            books.kept = held(wb, p).into_iter().map(|(n, _, link)| (link.name.clone(), n)).collect();
        }
        let _ = WRITING.try_with(|w| w.try_borrow_mut().map(|mut w| *w = Some(books)));
        Numbers(std::marker::PhantomData)
    }

    fn done(self) -> Books {
        WRITING.try_with(|w| w.try_borrow_mut().ok().and_then(|mut w| w.take())).ok().flatten().unwrap_or_default()
    }
}

impl Drop for Numbers {
    fn drop(&mut self) {
        let _ = WRITING.try_with(|w| w.try_borrow_mut().map(|mut w| *w = None));
    }
}

/// The number the file being written gives the workbook `book`, noting that a formula reads its
/// sheet `sheet`. `None` when nothing is being written, for a name that is a number already, and
/// past the most links a workbook holds: the name is then written as it is.
pub(crate) fn number_of(book: &str, sheet: &str) -> Option<u32> {
    WRITING.try_with(|w| w.try_borrow_mut().ok()?.as_mut()?.number(book, sheet)).ok().flatten()
}

// ---------------------------------------------------------------- writing

/// A kept link part whose link is in the model: written by [`finish`], once it is known which
/// of its sheets the formulas read.
pub(super) struct HeldOut {
    number: u32,
    /// The part's name in the written package.
    written: String,
    part: PreservedPart,
    link: Arc<ExternalLink>,
}

/// The kept link parts of `wb` that [`finish`] writes, by their original part names (normalized).
pub(super) fn held_parts(wb: &Workbook, p: &Preserved) -> HashSet<String> {
    held(wb, p).into_iter().map(|(_, part, _)| normalize(&part.name)).collect()
}

/// [`held_parts`] with the names `names` gave them.
pub(super) fn held_out(wb: &Workbook, p: &Preserved, names: &NameMap) -> Vec<HeldOut> {
    held(wb, p)
        .into_iter()
        .filter_map(|(number, part, link)| {
            Some(HeldOut { number, written: names.get(&normalize(&part.name))?.clone(), part: part.clone(), link: link.clone() })
        })
        .collect()
}

fn sheet_names_xml(names: &[String]) -> String {
    let mut x = String::from("<sheetNames>");
    for n in names {
        let _ = write!(x, "<sheetName val=\"{}\"/>", esc_attr(n));
    }
    x.push_str("</sheetNames>");
    x
}

fn push_cell(x: &mut String, at: CellRef, value: &Value) {
    let (ty, text) = match value {
        Value::Number(n) => ("", xml::num(*n)),
        Value::Text(s) => (" t=\"str\"", xml::esc(&xml::encode_xstring(s))),
        Value::Bool(b) => (" t=\"b\"", String::from(if *b { "1" } else { "0" })),
        Value::Error(e) => (" t=\"e\"", xml::esc(e.as_str())),
        Value::Empty | Value::Array(_) => return,
    };
    let _ = write!(x, "<cell r=\"{}\"{ty}><v>{text}</v></cell>", at.a1());
}

/// The values of `link` for the sheets `names`, in that order. A sheet the link has nothing of
/// is marked as not read (`refreshError`): a reference into it is `#REF!`, not a blank.
fn sheet_data_xml(names: &[String], link: &ExternalLink) -> String {
    let mut x = String::from("<sheetDataSet>");
    for (i, name) in names.iter().enumerate() {
        let Some(sheet) = link.sheet(name) else {
            let _ = write!(x, "<sheetData sheetId=\"{i}\" refreshError=\"1\"/>");
            continue;
        };
        let _ = write!(x, "<sheetData sheetId=\"{i}\">");
        let mut row: Option<u32> = None;
        for (at, value) in &sheet.cells {
            if matches!(value, Value::Empty | Value::Array(_)) {
                continue;
            }
            if row != Some(at.row) {
                if row.is_some() {
                    x.push_str("</row>");
                }
                let _ = write!(x, "<row r=\"{}\">", u64::from(at.row) + 1);
                row = Some(at.row);
            }
            push_cell(&mut x, *at, value);
        }
        if row.is_some() {
            x.push_str("</row>");
        }
        x.push_str("</sheetData>");
    }
    x.push_str("</sheetDataSet>");
    x
}

/// The sheet names a link's part lists: the ones it had, then the ones the link knows and the
/// ones formulas read (a formula's sheet has to be listed for the formula to be read back).
fn listed(had: &[String], link: &ExternalLink, read: &[&str]) -> Vec<String> {
    let mut names = had.to_vec();
    for extra in link.sheets.iter().map(|s| s.name.as_str()).chain(read.iter().copied()) {
        if names.len() < MAX_LINK_SHEETS && !names.iter().any(|n| same_name(n, extra)) {
            names.push(extra.to_string());
        }
    }
    names
}

/// Where the element `name` is in `text`: from its `<` to the end of its end tag.
fn element_range(text: &str, name: &str) -> Option<Range<usize>> {
    let open = format!("<{name}");
    let after = |i: usize| text.as_bytes().get(i + open.len()).copied();
    let start = text.match_indices(&open).map(|(i, _)| i).find(|i| matches!(after(*i), Some(b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n')))?;
    let tag_end = start + text.get(start..)?.find('>')?;
    if text.as_bytes().get(tag_end.checked_sub(1)?) == Some(&b'/') {
        return Some(start..tag_end + 1);
    }
    let close = format!("</{name}>");
    let end = tag_end + text.get(tag_end..)?.find(&close)? + close.len();
    Some(start..end)
}

/// A link part's bytes with its sheet names and values replaced by `names` and those of `link`;
/// everything else in it stays. `None` when the two elements are not found as expected: the part
/// is then written as it was.
fn spliced(data: &[u8], names: &[String], link: &ExternalLink) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(data).ok()?;
    // A `<sheetNames` inside a comment or a CDATA section is not the element.
    if text.contains("<!--") || text.contains("<![CDATA[") {
        return None;
    }
    let names_at = element_range(text, "sheetNames")?;
    // The values go where they were; in a part without any, after the names of the other
    // workbook's defined names, or after the sheet names.
    let data_at = match element_range(text, "sheetDataSet") {
        Some(at) => at,
        None => {
            let after = element_range(text, "definedNames").map_or(names_at.end, |d| d.end);
            after..after
        }
    };
    if data_at.start < names_at.end {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(text.get(..names_at.start)?);
    out.push_str(&sheet_names_xml(names));
    out.push_str(text.get(names_at.end..data_at.start)?);
    out.push_str(&sheet_data_xml(names, link));
    out.push_str(text.get(data_at.end..)?);
    Some(out.into_bytes())
}

/// The kept part of a link as it is written: as it was, unless the link's values, its sheets
/// or its target are no longer what the part says.
fn rewritten(part: &PreservedPart, link: &ExternalLink, read: &[&str]) -> Option<PreservedPart> {
    let v = view(part)?;
    let names = listed(&v.names, link, read);
    let values_changed = names.len() != v.names.len() || !v.link.same_values(link);
    let target = link.target.as_ref().filter(|t| Some(*t) != v.link.target.as_ref());
    if !values_changed && target.is_none() {
        return None;
    }
    let mut out = part.clone();
    if values_changed && let Some(data) = spliced(&part.data, &names, link) {
        out.data = data.into();
    }
    if let Some(target) = target {
        for r in out.rels.iter_mut().filter(|r| r.id == v.rel) {
            r.target.clone_from(target);
        }
    }
    Some(out)
}

/// A link made here, as a part.
fn new_part(link: &ExternalLink, read: &[&str]) -> String {
    let names = listed(&[], link, read);
    let mut x = format!("{XML_DECL}<externalLink xmlns=\"{}\"><externalBook xmlns:r=\"{NS_REL}\" r:id=\"rId1\">", crate::write::NS_MAIN);
    if !names.is_empty() {
        x.push_str(&sheet_names_xml(&names));
        x.push_str(&sheet_data_xml(&names, link));
    }
    x.push_str("</externalBook></externalLink>");
    x
}

/// Writes the links the formulas of `wb` read, once every formula is written: the kept parts
/// whose links are in the model, and a new part for each link made here. Returns the workbook's
/// `<externalReferences>` element (empty when it has no links).
pub fn finish(wb: &Workbook, kept: &WorkbookKept, wb_rels: &mut Rels, out: &mut Out, numbers: Numbers) -> String {
    let books = numbers.done();
    for h in &kept.links {
        let part = rewritten(&h.part, &h.link, &books.read_of(h.number));
        let part = part.as_ref().unwrap_or(&h.part);
        out.part(&h.written, Some(&part.content_type), part.data.to_vec());
        if !part.rels.is_empty() {
            out.part(&rels_part_name(&h.written), None, part_rels_xml(&part.rels, &kept.names).into_bytes());
        }
    }
    let mut refs = String::new();
    for (i, name) in books.made.iter().enumerate() {
        let Some(number) = u32::try_from(i).ok().and_then(|i| books.brought.checked_add(i)?.checked_add(1)) else { break };
        let unknown = ExternalLink::named(name.clone());
        let link = gridcraft_model::links::find(&wb.links, name).unwrap_or(&unknown);
        let part = out.kept.claim(&format!("xl/externalLinks/externalLink{number}.xml"));
        out.part(&part, Some(CT_LINK), new_part(link, &books.read_of(number)).into_bytes());
        // A workbook without a file is known by its name alone.
        let target = link.target.clone().unwrap_or_else(|| encode_name(&link.name));
        let path = PreservedRel { id: "rId1".into(), rel_type: REL_LINK_PATH.into(), target, external: true };
        out.part(&rels_part_name(&part), None, part_rels_xml(&[path], &NameMap::new()).into_bytes());
        let id = wb_rels.add("externalLink", &format!("/{part}"));
        let _ = write!(refs, "<externalReference r:id=\"{}\"/>", esc_attr(&id));
    }
    if refs.is_empty() {
        return kept.elements.clone();
    }
    match kept.elements.rfind("</externalReferences>") {
        Some(at) => {
            let mut x = kept.elements.clone();
            x.insert_str(at, &refs);
            x
        }
        None => format!("<externalReferences>{refs}</externalReferences>"),
    }
}
