use super::Dialog;

/// Said in About, so nobody takes GridKu for an ArtCraft release.
const DISCLAIMER: &str = "GridKu is not an official ArtCraft release and is not endorsed by the ArtCraft team.";

/// Help ▸ About GridKu: the app, its contributors and the AI models that helped. The credits
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
            ui.heading("GridKu");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(4.0);
            ui.label("A free, open-source spreadsheet written from scratch in Rust.");
            ui.label("GridKu is a modified version of GridCraft, by andriyanto.");
            ui.add_space(4.0);
            ui.hyperlink_to("GridKu on GitHub: github.com/andriyanto/gridku", crate::REPO_URL);
            ui.hyperlink_to("Downloads: github.com/andriyanto/gridku/releases", crate::RELEASES_URL);
            ui.hyperlink_to("Original project: github.com/storytold/gridcraft", "https://github.com/storytold/gridcraft");
            ui.add_space(4.0);
            ui.hyperlink_to("Join the community (WhatsApp group)", crate::COMMUNITY_URL);
            ui.hyperlink_to("Discussions: github.com/andriyanto/gridku/discussions", crate::DISCUSSIONS_URL);
            ui.add_space(6.0);
            // The licences ask that the original copyright notice stay with the program.
            ui.label(
                egui::RichText::new(
                    "MIT OR Apache-2.0. Copyright (c) 2026 ArtCraft Team and the GridCraft contributors; modifications by andriyanto.",
                )
                .small()
                .weak(),
            );
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
        assert_eq!(crate::REPO_URL, format!("https://github.com/{}", crate::UPDATE_REPO));
        assert_eq!(crate::RELEASES_URL, format!("{}/releases", crate::REPO_URL));
    }
}
