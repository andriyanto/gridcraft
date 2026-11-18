use super::Dialog;

/// Where this copy of GridCraft is built from and released: its own repository.
const THIS_BUILD: &str = "github.com/andriyanto/gridcraft";
/// Said in About, so nobody takes this copy for the original's release.
const DISCLAIMER: &str = "Disclaimer: this copy of GridCraft has been modified from the original. It is not an official ArtCraft release.";

/// Help ▸ About GridCraft: the app, its contributors and the AI models that helped. The credits
/// are compiled in (`crate::credits`, docs/contributors.md).
pub(super) fn about(ui: &mut egui::Ui, d: &mut Dialog) {
    ui.horizontal(|ui| {
        for tab in ["About", "Contributors", "Models"] {
            if ui.selectable_label(d.tab == tab, tab).clicked() {
                d.tab = tab.to_string();
            }
        }
    });
    ui.separator();
    match d.tab.as_str() {
        "Contributors" => crate::credits::contributors_ui(ui),
        "Models" => crate::credits::models_ui(ui),
        _ => {
            ui.heading("GridCraft");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(4.0);
            ui.label("A clean-room, open-source, Rust-native spreadsheet. Part of ArtCraft.");
            ui.hyperlink_to("getartcraft.com/apps/gridcraft", "https://getartcraft.com/apps/gridcraft");
            ui.hyperlink_to("Source code: github.com/storytold/gridcraft", "https://github.com/storytold/gridcraft");
            // This copy is built from its own repository: where to get it and its source. The
            // same place its updates come from (`crate::UPDATE_REPO`).
            ui.hyperlink_to(format!("Get this build: {THIS_BUILD}/releases"), format!("https://{THIS_BUILD}/releases"));
            ui.hyperlink_to(format!("Source code of this build: {THIS_BUILD}"), format!("https://{THIS_BUILD}"));
            ui.add_space(6.0);
            ui.label(egui::RichText::new("MIT OR Apache-2.0. Copyright (c) 2026 ArtCraft Team and the GridCraft contributors.").small().weak());
            ui.add_space(6.0);
            ui.label(egui::RichText::new(DISCLAIMER).small().weak());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn about_names_the_repository_this_build_and_its_updates_come_from() {
        assert!(super::DISCLAIMER.contains("not an official ArtCraft release"));
        // The links and the updater point at the same repository.
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(super::THIS_BUILD, format!("github.com/{}", crate::UPDATE_REPO));
    }
}
