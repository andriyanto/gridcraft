//! Data › Edit Links: the other workbooks this one's formulas read (`data.editLinks`), with
//! Change Source and Break Link for the one selected.

use super::{Dialog, Json, SheetApp, json};

fn text(link: &Json, key: &str) -> String {
    link.get(key).and_then(Json::as_str).unwrap_or("").to_string()
}

/// What the Status column says of a link.
fn status(link: &Json) -> &'static str {
    match link.get("status").and_then(Json::as_str) {
        Some("open") => "Open: values follow it",
        Some("saved") => "Not open: values from the file",
        _ => "Not open: nothing known of it",
    }
}

pub(super) fn edit_links(app: &mut SheetApp, ui: &mut egui::Ui, d: &mut Dialog, open: &mut bool) {
    let list = app.engine("data.editLinks", json!({})).unwrap_or(Json::Null);
    let links = list.as_array().cloned().unwrap_or_default();
    d.list_index = d.list_index.min(links.len().saturating_sub(1));
    let selected = links.get(d.list_index).cloned();
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        egui::Grid::new("links").striped(true).num_columns(4).show(ui, |ui| {
            ui.strong("Source");
            ui.strong("Status");
            ui.strong("Read by");
            ui.strong("Where");
            ui.end_row();
            if links.is_empty() {
                ui.label(egui::RichText::new("No formula of this workbook reads another workbook").italics());
                ui.end_row();
            }
            for (i, link) in links.iter().enumerate() {
                if ui.selectable_label(i == d.list_index, text(link, "name")).clicked() {
                    d.list_index = i;
                }
                ui.label(status(link));
                let reads = link.get("reads").and_then(Json::as_u64).unwrap_or(0);
                ui.label(if reads == 1 { "1 reference".to_string() } else { format!("{reads} references") });
                // Where the file is, or would be: the path as text. It is not looked for.
                let place = Some(text(link, "path")).filter(|p| !p.is_empty()).unwrap_or_else(|| text(link, "target"));
                ui.add(egui::Label::new(if place.is_empty() { "—".to_string() } else { place }).truncate());
                ui.end_row();
            }
        });
    });
    ui.add_space(8.0);
    let label = ui.label("Change the source to (a file, or the name of an open workbook):");
    let mut source = d.values.get("source").and_then(Json::as_str).unwrap_or("").to_string();
    let mut change = false;
    ui.horizontal(|ui| {
        if ui.add(egui::TextEdit::singleline(&mut source).desired_width(300.0)).labelled_by(label.id).changed() {
            d.values.insert("source".into(), json!(source));
        }
        if let Some(pick) = &app.services.pick_open
            && ui.add_enabled(selected.is_some(), egui::Button::new("Browse…")).clicked()
            && let Some(path) = pick()
        {
            source = path;
            d.values.insert("source".into(), json!(source));
            change = true;
        }
        change |= ui.add_enabled(selected.is_some() && !source.trim().is_empty(), egui::Button::new("Change Source")).clicked();
    });
    if change && let Some(link) = &selected {
        let source = source.trim();
        // An open workbook is named as its window names it; anything else is a file's path.
        let open_book = app.session.documents().iter().any(|doc| gridcraft_engine::formula::same_name(&doc.display_title(), source));
        let key = if open_book { "name" } else { "path" };
        match app.run("data.changeLinkSource", json!({"link": text(link, "name"), key: source})) {
            Ok(_) => {
                d.values.remove("source");
            }
            Err(e) => app.message = Some(("Edit Links".into(), crate::clean_error(&e))),
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Break Link"))
            .on_hover_text("The formulas that read this workbook become their values. Undo brings them back.")
            .clicked()
            && let Some(link) = &selected
        {
            match app.run("data.breakLink", json!({"link": text(link, "name")})) {
                Ok(r) => {
                    let names: Vec<&str> =
                        r.get("names").and_then(Json::as_array).map(|n| n.iter().filter_map(Json::as_str).collect()).unwrap_or_default();
                    if !names.is_empty() {
                        let list = names.join(", ");
                        app.message = Some((
                            "Edit Links".into(),
                            format!("The formulas became their values. The link stays, as these defined names still read the workbook: {list}."),
                        ));
                    }
                }
                Err(e) => app.message = Some(("Edit Links".into(), crate::clean_error(&e))),
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(" Close ").clicked() {
                *open = false;
            }
        });
    });
}
