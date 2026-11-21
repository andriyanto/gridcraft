//! Links to other workbooks between a file and the model (`passthrough/links.rs`,
//! `docs/external-references.md`). Packages are built in code; nothing here was made by Excel.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_model::{Cell, DefinedName, ExternalLink, Formula, Workbook};

use super::{CT, ROOT_RELS, make_zip};
use crate::xml;
use crate::{read_xlsx, write_xlsx};

const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

fn rels(items: &[(&str, &str, &str, bool)]) -> String {
    let mut s = String::from(r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, ty, target, external) in items {
        let mode = if *external { r#" TargetMode="External""# } else { "" };
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{NS_R}/{ty}" Target="{target}"{mode}/>"#));
    }
    s.push_str("</Relationships>");
    s
}

/// A link part as Excel writes one: sheet names, the other workbook's names, values of four
/// kinds, and an extension list after them.
const BUDGET: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x14"><externalBook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1"><sheetNames><sheetName val="Sheet1"/><sheetName val="Other"/><sheetName val="Gone"/></sheetNames><definedNames><definedName name="Total" refersTo="=Sheet1!$A$9"/></definedNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1"><v>7</v></cell><cell r="B1" t="str"><v>plan</v></cell></row><row r="3"><cell r="A3" t="b"><v>1</v></cell><cell r="B3" t="e"><v>#N/A</v></cell></row></sheetData><sheetData sheetId="2" refreshError="1"/></sheetDataSet></externalBook><extLst><ext uri="{keep-me}"/></extLst></externalLink>"#;
const DDE: &str = r#"<?xml version="1.0"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><ddeLink ddeService="app" ddeTopic="topic"><ddeItems><ddeItem name="item" advise="1"/></ddeItems></ddeLink></externalLink>"#;

/// A one-sheet package whose links are `links` (part XML, target), in that order, with the
/// cells `cells` (inside `<sheetData>`) and the workbook's `<definedNames>` content `names`.
fn package(links: &[(&str, Option<&str>)], cells: &str, names: &str) -> Vec<u8> {
    let mut refs = String::new();
    let mut wb_rels = vec![("rId1".to_string(), "worksheet", "worksheets/sheet1.xml".to_string())];
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    let mut types = CT.replace("</Types>", "");
    for (i, (part, target)) in links.iter().enumerate() {
        let n = i + 1;
        refs.push_str(&format!(r#"<externalReference r:id="rId{}"/>"#, n + 1));
        wb_rels.push((format!("rId{}", n + 1), "externalLink", format!("externalLinks/externalLink{n}.xml")));
        files.push((format!("xl/externalLinks/externalLink{n}.xml"), part.as_bytes().to_vec()));
        if let Some(target) = target {
            files
                .push((format!("xl/externalLinks/_rels/externalLink{n}.xml.rels"), rels(&[("rId1", "externalLinkPath", target, true)]).into_bytes()));
        }
        types.push_str(&format!(
            r#"<Override PartName="/xl/externalLinks/externalLink{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/>"#
        ));
    }
    types.push_str("</Types>");
    let refs = if refs.is_empty() { refs } else { format!("<externalReferences>{refs}</externalReferences>") };
    let names = if names.is_empty() { String::new() } else { format!("<definedNames>{names}</definedNames>") };
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>{refs}{names}</workbook>"#
    );
    let sheet = format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData>{cells}</sheetData></worksheet>"#);
    let wb_rels: Vec<(&str, &str, &str, bool)> = wb_rels.iter().map(|(id, ty, target)| (id.as_str(), *ty, target.as_str(), false)).collect();
    files.push(("[Content_Types].xml".into(), types.into_bytes()));
    files.push(("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()));
    files.push(("xl/workbook.xml".into(), workbook.into_bytes()));
    files.push(("xl/_rels/workbook.xml.rels".into(), rels(&wb_rels).into_bytes()));
    files.push(("xl/worksheets/sheet1.xml".into(), sheet.into_bytes()));
    let files: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    make_zip(&files)
}

const CELLS: &str = r#"<row r="1"><c r="A1"><f>[1]Sheet1!$A$1</f><v>7</v></c><c r="B1"><f>SUM([1]Sheet1!A1:A3)*2</f><v>14</v></c></row>"#;
const NAME: &str = r#"<definedName name="Rate">[1]Sheet1!$A$1</definedName>"#;
const TARGET: &str = "../data/Budget%20Plan.xlsx";

fn budget_package() -> Vec<u8> {
    package(&[(BUDGET, Some(TARGET))], CELLS, NAME)
}

struct Unpacked(HashMap<String, Vec<u8>>);

impl Unpacked {
    fn new(bytes: &[u8]) -> Unpacked {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut parts = HashMap::new();
        for i in 0..z.len() {
            let mut f = z.by_index(i).unwrap();
            let mut v = vec![];
            f.read_to_end(&mut v).unwrap();
            parts.insert(f.name().unwrap().into_owned(), v);
        }
        Unpacked(parts)
    }
    fn text(&self, name: &str) -> String {
        String::from_utf8(self.0.get(name).unwrap_or_else(|| panic!("no part {name} among {:?}", self.0.keys())).clone()).unwrap()
    }
    /// The link parts in the order of the workbook's `<externalReferences>`: (part name, target).
    fn links(&self) -> Vec<(String, Option<String>)> {
        let wb = xml::parse(self.0.get("xl/workbook.xml").unwrap()).unwrap();
        let wb_rels = xml::parse(self.0.get("xl/_rels/workbook.xml.rels").unwrap()).unwrap();
        let Some(refs) = wb.child("externalReferences") else { return vec![] };
        refs.kids("externalReference")
            .map(|r| {
                let rel = wb_rels.kids("Relationship").find(|x| x.attr("Id") == r.attr("id")).expect("a relationship for each link");
                assert!(rel.attr("Type").unwrap().ends_with("/externalLink"));
                let part = format!("xl/{}", rel.attr("Target").unwrap().trim_start_matches("/xl/"));
                assert!(self.0.contains_key(&part), "{part} is in the package");
                let types = self.text("[Content_Types].xml");
                assert!(
                    types.contains(&format!(
                        r#"PartName="/{part}" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml""#
                    )),
                    "{types}"
                );
                let rels_name = part.replace("externalLinks/", "externalLinks/_rels/") + ".rels";
                let target = self.0.get(&rels_name).map(|b| {
                    let x = xml::parse(b).unwrap();
                    let path = x.kids("Relationship").find(|x| x.attr("Type").unwrap().ends_with("/externalLinkPath")).unwrap();
                    assert_eq!(path.attr("TargetMode"), Some("External"));
                    path.attr("Target").unwrap().to_string()
                });
                (part, target)
            })
            .collect()
    }
}

fn formula(wb: &Workbook, a1: &str) -> String {
    wb.sheet(0).unwrap().cell(at(a1)).unwrap().formula.as_ref().unwrap().text.clone()
}

#[test]
fn a_link_of_the_file_is_read_into_the_model_and_formulas_name_it() {
    let (wb, rep) = read_xlsx(&budget_package()).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("link")), "a link that is read is nothing to warn about: {:?}", rep.warnings);
    let [link] = wb.links.as_slice() else { panic!("one link, got {:?}", wb.links) };
    assert_eq!((link.name.as_str(), link.target.as_deref(), link.number), ("Budget Plan.xlsx", Some(TARGET), Some(1)));
    // The sheets it has values of: the one whose last update failed is not among them.
    assert_eq!(link.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Sheet1", "Other"]);
    let sheet1 = link.sheet("sheet1").unwrap();
    assert_eq!(sheet1.value(at("A1")), Value::Number(7.0));
    assert_eq!(sheet1.value(at("B1")), Value::text("plan"));
    assert_eq!(sheet1.value(at("A3")), Value::Bool(true));
    assert_eq!(sheet1.value(at("B3")), Value::Error(CellError::NA));
    assert_eq!(sheet1.value(at("A2")), Value::Empty);
    assert!(link.sheet("Other").unwrap().cells.is_empty() && link.sheet("Gone").is_none());
    // Formulas and names say the workbook's name where the file has its number.
    assert_eq!(formula(&wb, "A1"), "'[Budget Plan.xlsx]Sheet1'!$A$1");
    assert_eq!(formula(&wb, "B1"), "SUM('[Budget Plan.xlsx]Sheet1'!A1:A3)*2");
    assert_eq!(wb.names[0].formula, "'[Budget Plan.xlsx]Sheet1'!$A$1");
    // The values the file saved for the formulas are there.
    assert_eq!(wb.sheet(0).unwrap().value(at("B1")), Value::Number(14.0));
}

#[test]
fn an_untouched_link_goes_back_byte_for_byte_with_its_number() {
    let (wb, _) = read_xlsx(&budget_package()).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let [(part, target)] = u.links().try_into().unwrap();
    assert_eq!(u.0.get(&part).unwrap(), BUDGET.as_bytes(), "the part is as the file had it");
    assert_eq!(target.as_deref(), Some(TARGET));
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<f>[1]Sheet1!$A$1</f>") && sheet.contains("<f>SUM([1]Sheet1!A1:A3)*2</f>"), "{sheet}");
    assert!(u.text("xl/workbook.xml").contains(r#"<definedName name="Rate">[1]Sheet1!$A$1</definedName>"#));
    // And once more: reading what was written gives the same workbook's links.
    let (again, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(again.links, wb.links);
    assert_eq!(formula(&again, "A1"), formula(&wb, "A1"));
}

#[test]
fn changed_values_are_rewritten_inside_the_kept_part() {
    let (mut wb, _) = read_xlsx(&budget_package()).unwrap();
    {
        let link = Arc::make_mut(&mut wb.links[0]);
        assert!(link.set("Sheet1", at("A1"), Value::Number(8.5)));
        assert!(link.set("Sheet1", at("B1"), Value::Empty));
        assert!(link.set("Sheet1", at("C2"), Value::text("a < b & _x0041_")));
        assert!(link.set("New", at("D4"), Value::Bool(false)));
    }
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    let [(part, target)] = u.links().try_into().unwrap();
    let text = u.text(&part);
    // What is not the sheet names or the values stays as it was.
    assert!(text.starts_with(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink "#), "{text}");
    assert!(text.contains(r#"<definedNames><definedName name="Total" refersTo="=Sheet1!$A$9"/></definedNames>"#), "{text}");
    assert!(text.ends_with(r#"</externalBook><extLst><ext uri="{keep-me}"/></extLst></externalLink>"#), "{text}");
    assert_eq!(target.as_deref(), Some(TARGET));
    // The sheets keep their places (the values count them), the new one comes last, and the
    // sheet nothing is known of stays marked.
    assert!(
        text.contains(r#"<sheetNames><sheetName val="Sheet1"/><sheetName val="Other"/><sheetName val="Gone"/><sheetName val="New"/></sheetNames>"#),
        "{text}"
    );
    assert!(text.contains(r#"<sheetData sheetId="2" refreshError="1"/>"#), "{text}");
    assert!(text.contains(r#"<sheetData sheetId="3"><row r="4"><cell r="D4" t="b"><v>0</v></cell></row></sheetData>"#), "{text}");
    xml::parse(text.as_bytes()).expect("still well formed");
    let (again, _) = read_xlsx(&out).unwrap();
    assert!(again.links[0].same_values(&wb.links[0]), "{:?}", again.links[0]);
    assert_eq!(again.links[0].sheet("Sheet1").unwrap().value(at("C2")), Value::text("a < b & _x0041_"));
}

#[test]
fn a_changed_target_is_written_to_the_kept_parts_relationship() {
    let (mut wb, _) = read_xlsx(&budget_package()).unwrap();
    Arc::make_mut(&mut wb.links[0]).target = Some("Budget%20Plan.xlsx".into());
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let [(part, target)] = u.links().try_into().unwrap();
    assert_eq!(target.as_deref(), Some("Budget%20Plan.xlsx"));
    assert_eq!(u.0.get(&part).unwrap(), BUDGET.as_bytes(), "the part itself did not change");
}

/// A workbook made here whose A1 reads `book`, with what it keeps of it.
fn reader(formula: &str, link: Option<ExternalLink>) -> Workbook {
    let mut wb = Workbook::new();
    let sheet = wb.sheet_mut(0).unwrap();
    sheet.cells.set(at("A1"), Cell { value: Value::Number(42.0), formula: Some(Arc::new(Formula::new(formula))), ..Cell::default() });
    wb.links = link.into_iter().map(Arc::new).collect();
    wb
}

fn budget_link() -> ExternalLink {
    let mut link = ExternalLink { target: Some("../data/Budget.xlsx".into()), ..ExternalLink::named("Budget.xlsx") };
    assert!(link.set("Sheet 1", at("B4"), Value::Number(42.0)));
    assert!(link.set("Sheet 1", at("B5"), Value::text("x")));
    link
}

#[test]
fn a_link_made_here_is_written_as_a_part_and_read_back() {
    let mut wb = reader("'[Budget.xlsx]Sheet 1'!B4", Some(budget_link()));
    wb.names.push(DefinedName {
        name: "Rate".into(),
        scope: None,
        formula: "'[Budget.xlsx]Sheet 1'!$B$5".into(),
        comment: String::new(),
        hidden: false,
    });
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    let [(part, target)] = u.links().try_into().unwrap();
    assert_eq!(target.as_deref(), Some("../data/Budget.xlsx"));
    let text = u.text(&part);
    assert!(text.contains(r#"<sheetNames><sheetName val="Sheet 1"/></sheetNames>"#), "{text}");
    assert!(text.contains(r#"<sheetData sheetId="0"><row r="4"><cell r="B4"><v>42</v></cell></row><row r="5"><cell r="B5" t="str"><v>x</v></cell></row></sheetData>"#), "{text}");
    assert!(u.text("xl/worksheets/sheet1.xml").contains("<f>'[1]Sheet 1'!B4</f>"), "{}", u.text("xl/worksheets/sheet1.xml"));
    let workbook = u.text("xl/workbook.xml");
    assert!(workbook.contains(r#"<definedName name="Rate">'[1]Sheet 1'!$B$5</definedName>"#), "{workbook}");
    assert!(workbook.find("</sheets>").unwrap() < workbook.find("<externalReferences>").unwrap());
    assert!(workbook.find("</externalReferences>").unwrap() < workbook.find("<definedNames>").unwrap());

    let (again, rep) = read_xlsx(&out).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert_eq!(*again.links[0], ExternalLink { number: Some(1), ..budget_link() });
    assert_eq!(formula(&again, "A1"), "'[Budget.xlsx]Sheet 1'!B4");
    assert_eq!(again.names[0].formula, "'[Budget.xlsx]Sheet 1'!$B$5");
    // Saved again, it is the file's own link now and goes back as it is.
    let twice = Unpacked::new(&write_xlsx(&again).unwrap());
    assert_eq!(twice.text(&twice.links()[0].0), text);
}

#[test]
fn a_workbook_without_a_file_is_written_by_its_name_alone() {
    let mut link = ExternalLink::named("Book 2");
    assert!(link.set("Sheet1", at("A1"), Value::Number(1.0)));
    let out = write_xlsx(&reader("'[Book 2]Sheet1'!A1", Some(link))).unwrap();
    let u = Unpacked::new(&out);
    assert_eq!(u.links()[0].1.as_deref(), Some("Book%202"));
    let (again, _) = read_xlsx(&out).unwrap();
    assert_eq!((again.links[0].name.as_str(), formula(&again, "A1")), ("Book 2", "'[Book 2]Sheet1'!A1".to_string()));
}

#[test]
fn a_formula_into_a_workbook_nothing_is_kept_of_lists_its_sheet_as_not_read() {
    // No link in the model at all (nothing synchronised it), and one that lacks the sheet.
    for link in [None, Some(budget_link())] {
        let out = write_xlsx(&reader("[Budget.xlsx]Other!A1+[Budget.xlsx]OTHER!A2", link)).unwrap();
        let u = Unpacked::new(&out);
        let text = u.text(&u.links()[0].0);
        assert!(text.contains(r#"<sheetName val="Other"/></sheetNames>"#) && !text.contains("OTHER"), "the sheet is listed once: {text}");
        assert!(text.contains(r#" refreshError="1"/>"#), "{text}");
        let (again, _) = read_xlsx(&out).unwrap();
        assert!(again.links[0].sheet("Other").is_none(), "nothing is known of it: a reference into it is #REF!, not a blank");
        assert_eq!(formula(&again, "A1"), "[Budget.xlsx]Other!A1+[Budget.xlsx]OTHER!A2");
    }
}

#[test]
fn a_link_no_formula_reads_is_not_written() {
    let out = write_xlsx(&reader("1+1", Some(budget_link()))).unwrap();
    let u = Unpacked::new(&out);
    assert!(u.links().is_empty() && !u.0.keys().any(|k| k.contains("externalLink")), "{:?}", u.0.keys());
    assert!(!u.text("xl/workbook.xml").contains("externalReferences"));
}

#[test]
fn kept_links_keep_their_numbers_and_links_made_here_follow() {
    // 1: a DDE link (not a workbook), 2: a workbook, then two made here.
    let cells = r#"<row r="1"><c r="A1"><f>[2]Sheet1!$A$1</f><v>7</v></c><c r="B1"><f>[1]!'item'</f><v>3</v></c></row>"#;
    let (mut wb, rep) = read_xlsx(&package(&[(DDE, None), (BUDGET, Some(TARGET))], cells, "")).unwrap();
    assert!(rep.warnings.iter().any(|w| w.starts_with("1 link to other sources")), "the one that is not read is said: {:?}", rep.warnings);
    assert_eq!(wb.links.iter().map(|l| (l.name.as_str(), l.number)).collect::<Vec<_>>(), [("Budget Plan.xlsx", Some(2))]);
    assert_eq!(formula(&wb, "A1"), "'[Budget Plan.xlsx]Sheet1'!$A$1");
    let set = |wb: &mut Workbook, a1: &str, f: &str| {
        wb.sheet_mut(0).unwrap().cells.set(at(a1), Cell { formula: Some(Arc::new(Formula::new(f))), ..Cell::default() });
    };
    set(&mut wb, "A2", "[Zeta.xlsx]S!A1");
    set(&mut wb, "A3", "[Alpha.xlsx]S!A1+[zeta.xlsx]S!A2");
    wb.links.push(Arc::new(ExternalLink::named("Alpha.xlsx")));
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    let links = u.links();
    assert_eq!(links.len(), 4);
    assert_eq!(u.0.get(&links[0].0).unwrap(), DDE.as_bytes());
    assert_eq!(u.0.get(&links[1].0).unwrap(), BUDGET.as_bytes());
    assert_eq!((links[2].1.as_deref(), links[3].1.as_deref()), (Some("Zeta.xlsx"), Some("Alpha.xlsx")), "in the order formulas first read them");
    let sheet = u.text("xl/worksheets/sheet1.xml");
    for f in ["<f>[2]Sheet1!$A$1</f>", "<f>[1]!'item'</f>", "<f>[3]S!A1</f>", "<f>[4]S!A1+[3]S!A2</f>"] {
        assert!(sheet.contains(f), "{f} in {sheet}");
    }
    let (again, _) = read_xlsx(&out).unwrap();
    assert_eq!(
        again.links.iter().map(|l| (l.name.as_str(), l.number)).collect::<Vec<_>>(),
        [("Budget Plan.xlsx", Some(2)), ("Zeta.xlsx", Some(3)), ("Alpha.xlsx", Some(4))]
    );
    assert_eq!(formula(&again, "A3"), "[Alpha.xlsx]S!A1+[Zeta.xlsx]S!A2");
}

#[test]
fn a_link_that_is_not_understood_stays_a_number() {
    let numbered = |links: &[(&str, Option<&str>)]| {
        let (wb, _) = read_xlsx(&package(links, CELLS, NAME)).unwrap();
        assert!(wb.links.is_empty(), "{:?}", wb.links);
        assert_eq!(formula(&wb, "A1"), "[1]Sheet1!$A$1");
        assert_eq!(wb.sheet(0).unwrap().value(at("A1")), Value::Number(7.0));
        // And it is written as it was read.
        let u = Unpacked::new(&write_xlsx(&wb).unwrap());
        assert_eq!(u.0.get(&u.links()[0].0).unwrap(), links[0].0.as_bytes());
        assert!(u.text("xl/worksheets/sheet1.xml").contains("<f>[1]Sheet1!$A$1</f>"));
    };
    // Two links with one file name can't be told apart by name.
    numbered(&[(BUDGET, Some("a/Budget.xlsx")), (BUDGET, Some("b/BUDGET.xlsx"))]);
    // A name that would read as a number, one a formula can't hold, none at all.
    numbered(&[(BUDGET, Some("../2024"))]);
    numbered(&[(BUDGET, Some("a/Bud%5Bget%5D.xlsx"))]);
    numbered(&[(BUDGET, Some("folder/"))]);
    numbered(&[(BUDGET, None)]);
    // A value of a kind that is not known, a cell without a place, a sheet that is not listed.
    numbered(&[(&BUDGET.replace(r#"t="str""#, r#"t="s""#), Some(TARGET))]);
    numbered(&[(&BUDGET.replace(r#"<cell r="A1">"#, "<cell>"), Some(TARGET))]);
    numbered(&[(&BUDGET.replace(r#"sheetId="0""#, r#"sheetId="9""#), Some(TARGET))]);
    numbered(&[(&BUDGET.replace(r#"val="Other""#, r#"val="SHEET1""#), Some(TARGET))]);
    // Not XML at all.
    numbered(&[("<externalLink><externalBook", Some(TARGET))]);
}

#[test]
fn a_part_whose_values_cant_be_found_as_text_is_written_as_it_was() {
    // The names are in a comment as well: the part parses, but is not rewritten by text.
    let odd = BUDGET.replace("<sheetNames>", "<!-- <sheetNames/> --><sheetNames>");
    let (mut wb, _) = read_xlsx(&package(&[(&odd, Some(TARGET))], CELLS, NAME)).unwrap();
    assert!(Arc::make_mut(&mut wb.links[0]).set("Sheet1", at("A1"), Value::Number(9.0)));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert_eq!(u.text(&u.links()[0].0), odd);
}

#[test]
fn link_targets_are_the_same_text_on_every_system() {
    use std::path::Path;
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let p = |names: &[&str]| names.iter().fold(std::path::PathBuf::from(root), |p, n| p.join(n));
    let target = |source: &[&str], folder: &[&str]| crate::link_target(&p(source), &p(folder));
    assert_eq!(target(&["work", "Budget.xlsx"], &["work"]).as_deref(), Some("Budget.xlsx"));
    assert_eq!(target(&["work", "data", "My Budget #1.xlsx"], &["work", "reports", "q1"]).as_deref(), Some("../../data/My%20Budget%20%231.xlsx"));
    assert_eq!(target(&["Budget.xlsx"], &["a", "b"]).as_deref(), Some("../../Budget.xlsx"));
    assert_eq!(crate::link_target(Path::new("Budget.xlsx"), &p(&["work"])), None, "a workbook without a place has no path");

    let names = |target: &str, folder: &[&str], file: &[&str]| crate::link_names(target, &p(folder), &p(file));
    assert!(names("Budget.xlsx", &["work"], &["work", "Budget.xlsx"]));
    assert!(names("../../data/My%20Budget%20%231.xlsx", &["work", "reports", "q1"], &["work", "data", "My Budget #1.xlsx"]));
    assert!(!names("Budget.xlsx", &["work"], &["other", "Budget.xlsx"]));
    assert!(!names("../../../../Budget.xlsx", &["work"], &["Budget.xlsx"]), "a path above the root names nothing");
    assert!(!names("https://example.com/Budget.xlsx", &["work"], &["work", "Budget.xlsx"]));
    // Where a target says the file is, as a path of this system, and back.
    let at = |target: &str, folder: Option<&[&str]>| crate::link_path(target, folder.map(p).as_deref());
    assert_eq!(at("../data/My%20Budget.xlsx", Some(&["work", "reports"])), Some(p(&["work", "data", "My Budget.xlsx"])));
    assert_eq!(at("Budget.xlsx", None), None, "a relative path in a workbook without a folder names nothing");
    assert_eq!(at("../../../Budget.xlsx", Some(&["work"])), None);
    assert_eq!(at("https://example.com/Budget.xlsx", Some(&["work"])), None, "a web address is not a file");
    let whole = crate::link_target_absolute(&p(&["work", "data", "My Budget.xlsx"])).expect("an absolute path has a whole address");
    assert!(whole.starts_with("file:///"), "{whole}");
    assert_eq!(at(&whole, None), Some(p(&["work", "data", "My Budget.xlsx"])), "{whole}");
    assert_eq!(crate::link_target_absolute(Path::new("Budget.xlsx")), None);
    #[cfg(unix)]
    {
        assert!(names("file:////srv/data/Budget.xlsx", &["work"], &["srv", "data", "Budget.xlsx"]));
        assert!(!names("file:///C:\\data\\Budget.xlsx", &["work"], &["data", "Budget.xlsx"]), "a path of another system names nothing here");
        assert_eq!(crate::link_target(Path::new("/srv/Budget.xlsx"), Path::new("/work")).as_deref(), Some("../srv/Budget.xlsx"));
    }
}

#[test]
#[cfg(unix)]
fn saving_into_another_folder_rebases_the_models_links_with_the_kept_ones() {
    let (mut wb, _) = read_xlsx(&budget_package()).unwrap();
    wb.links.push(Arc::new(budget_link()));
    wb.links.push(Arc::new(ExternalLink { target: Some("file:///C:\\far\\Away.xlsx".into()), ..ExternalLink::named("Away.xlsx") }));
    wb.sheet_mut(0)
        .unwrap()
        .cells
        .set(at("A5"), Cell { formula: Some(Arc::new(Formula::new("'[Budget.xlsx]Sheet 1'!B4+[Away.xlsx]S!A1"))), ..Cell::default() });
    wb.source_dir = Some("/work/reports".into());
    let moved = crate::rebase_links(&wb, std::path::Path::new("/work/reports/q1")).expect("the links move");
    assert_eq!(
        moved.links.iter().map(|l| l.target.as_deref().unwrap()).collect::<Vec<_>>(),
        ["../../data/Budget%20Plan.xlsx", "../../data/Budget.xlsx", "file:///C:\\far\\Away.xlsx"]
    );
    let u = Unpacked::new(&write_xlsx(&moved).unwrap());
    let links = u.links();
    assert_eq!(
        links.iter().map(|l| l.1.as_deref().unwrap()).collect::<Vec<_>>(),
        ["../../data/Budget%20Plan.xlsx", "../../data/Budget.xlsx", "file:///C:\\far\\Away.xlsx"]
    );
    assert_eq!(u.0.get(&links[0].0).unwrap(), BUDGET.as_bytes(), "the kept part is not rewritten for a path that moved with it");
    assert!(crate::rebase_links(&wb, std::path::Path::new("/work/reports")).is_none(), "the same folder changes nothing");
}

#[test]
fn hostile_link_parts_are_bounded_and_never_crash() {
    // More sheets than a link keeps: the link stays a number.
    let many: String = (0..=gridcraft_model::links::MAX_LINK_SHEETS).map(|i| format!(r#"<sheetName val="S{i}"/>"#)).collect();
    let big = BUDGET.replace(r#"<sheetName val="Other"/>"#, &many);
    let (wb, _) = read_xlsx(&package(&[(&big, Some(TARGET))], CELLS, NAME)).unwrap();
    assert!(wb.links.is_empty());
    // Every prefix of a link part reads (as a link or not) and writes.
    for cut in (0..BUDGET.len()).step_by(7) {
        let Some(part) = BUDGET.get(..cut) else { continue };
        if let Ok((wb, _)) = read_xlsx(&package(&[(part, Some(TARGET))], CELLS, NAME)) {
            write_xlsx(&wb).unwrap();
        }
    }
    // A character XML can't hold is not part of a name.
    let (wb, _) = read_xlsx(&package(&[(BUDGET, Some("\u{1}x"))], CELLS, NAME)).unwrap();
    assert!(wb.links.is_empty());
    write_xlsx(&wb).unwrap();
    // A name of any odd shape reads as a name or stays a number.
    for target in ["%", "%zz.xlsx", "a'b.xlsx", "a!b.xlsx", "a:b.xlsx", "..", "file:///", "[x].xlsx", "x]y", "'", "ß.xlsx"] {
        let (wb, _) = read_xlsx(&package(&[(BUDGET, Some(target))], CELLS, NAME)).unwrap();
        let out = write_xlsx(&wb).unwrap();
        let (again, _) = read_xlsx(&out).unwrap();
        assert_eq!(again.links, wb.links, "{target}");
        assert_eq!(formula(&again, "A1"), formula(&wb, "A1"), "{target}");
        assert!(Unpacked::new(&out).text("xl/worksheets/sheet1.xml").contains("<f>[1]Sheet1!$A$1</f>"), "{target}");
    }
}

#[test]
fn a_broken_link_leaves_the_file_when_it_is_the_last() {
    let cells = r#"<row r="1"><c r="A1"><f>[2]Sheet1!$A$1</f><v>7</v></c><c r="B1"><f>[1]!'item'</f><v>3</v></c></row>"#;
    let (wb, _) = read_xlsx(&package(&[(DDE, None), (BUDGET, Some(TARGET))], cells, "")).unwrap();
    assert!(crate::forget_link(&wb, 1).is_none(), "the first keeps its place: the second is known by its number");
    assert!(crate::forget_link(&wb, 3).is_none() && crate::forget_link(&wb, 0).is_none());
    // The last one goes, part and all; the other stays where it was.
    let mut without = wb.clone();
    without.preserved = Some(Arc::new(crate::forget_link(&wb, 2).expect("the last link can go")));
    without.links.clear();
    without.sheet_mut(0).unwrap().set_value(at("A1"), Value::Number(7.0));
    let u = Unpacked::new(&write_xlsx(&without).unwrap());
    let links = u.links();
    assert_eq!(links.len(), 1);
    assert_eq!(u.0.get(&links[0].0).unwrap(), DDE.as_bytes());
    assert_eq!(u.0.keys().filter(|k| k.contains("externalLink") && !k.contains("_rels")).count(), 1, "{:?}", u.0.keys());
    assert!(u.text("xl/worksheets/sheet1.xml").contains("<f>[1]!'item'</f>"));
    // And then the one before it: the file has no links left.
    let mut none = without.clone();
    none.preserved = Some(Arc::new(crate::forget_link(&without, 1).expect("it is the last now")));
    let u = Unpacked::new(&write_xlsx(&none).unwrap());
    assert!(u.links().is_empty() && !u.text("xl/workbook.xml").contains("externalReferences"));
    assert!(!u.0.keys().any(|k| k.contains("externalLink")), "{:?}", u.0.keys());
}
