//! Updates: one background check at startup (and on Automate ▸ Check for Updates…), and a prompt
//! that offers the new release for download.
//!
//! **This copy never installs an update itself.** Its builds are not code-signed, and an
//! installer the app fetched and started on its own is what Windows SmartScreen stops, with the
//! app already closed for the update. So the prompt has Download and Later only: Download, and
//! Automate ▸ Download Latest… (Help ▸ Download Latest Version… in the macOS menu bar), open the
//! releases page in the browser, where the person downloads the installer and runs it
//! themselves, with the warning the system shows them.
//!
//! The check and the two windows are craft-update's (`check_for_update`, `StartupPrompt`,
//! `CheckResultDialog`); what is left out is its self-update (`updater::Updater`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use craft_update::updater::Status;
use craft_update::{CheckResultDialog, CheckResultResponse, Release, StartupPrompt, StartupPromptResponse};

use crate::SheetApp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a check came to: a newer release, none, or why it could not be asked.
type Checked = Result<Option<Release>, String>;
/// Asks for the newest release (blocking; it runs on its own thread). Tests stand in for the
/// network with one.
pub(crate) type CheckSource = Arc<dyn Fn() -> Checked + Send + Sync>;

pub(crate) struct Runtime {
    /// The check at start was asked for; `tick` starts it once, with the context.
    startup: bool,
    started: bool,
    /// Check for Updates… was chosen; the next `tick` starts it.
    manual: bool,
    /// The check running was asked for by the person: its outcome is said whatever it is.
    requested: bool,
    check: Option<Receiver<Checked>>,
    /// What a requested check came to, for the result window.
    status: Status,
    /// A newer release, on offer.
    prompt: Option<StartupPrompt>,
    /// In place of the network (tests).
    pub(crate) source: Option<CheckSource>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self { startup: false, started: false, manual: false, requested: false, check: None, status: Status::Idle, prompt: None, source: None }
    }
}

/// The version this build compares releases with: its own, or the one `GRIDCRAFT_PRETEND_VERSION`
/// names for this run (to see the prompt without waiting for a release).
fn running_version() -> String {
    craft_update::self_update::current_version(&crate::UPDATE_APP, VERSION).map_or_else(|| VERSION.to_string(), |v| v.to_string())
}

/// The page Download Latest… opens: this repository's releases, the newest first.
/// (Not `releases/latest`, which is no page while there is no release or only pre-releases.)
pub(crate) fn download_page() -> String {
    crate::UPDATE_APP.releases_url()
}

/// The page Download opens for `release`: its own page when the link is a plain web link, else
/// this repository's releases. (A `Release` can come from a cache or a feed, so its link is
/// checked here as craft-update checks it.)
fn page_of(release: &Release) -> String {
    if craft_update::release::safe_web_link(&release.html_url) { release.html_url.clone() } else { crate::UPDATE_APP.releases_url() }
}

/// Ask for a check; the next frame's `tick` starts it. A manual one reports every outcome.
pub(crate) fn start(app: &mut SheetApp, manual: bool) {
    if manual {
        app.updates_rt.manual = true;
    } else {
        app.updates_rt.startup = true;
    }
}

impl Runtime {
    fn begin(&mut self, ctx: &egui::Context, requested: bool) {
        if requested {
            self.requested = true;
            self.status = Status::Checking;
        }
        if self.check.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (source, version, ctx) = (self.source.clone(), running_version(), ctx.clone());
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let ask = || match source {
                Some(ask) => ask(),
                None => craft_update::Http::new(&format!("{}/Update", crate::UPDATE_APP.name))
                    .and_then(|http| craft_update::check_for_update(&crate::UPDATE_APP, &http, &version)),
            };
            let answer =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(ask)).unwrap_or_else(|_| Err("the update check failed unexpectedly".into()));
            let _ = tx.send(answer);
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => self.check = Some(rx),
            Err(e) => self.finish(Err(format!("couldn't start the update check ({e})"))),
        }
    }

    /// Takes in a check's answer. The check at start says nothing unless it found a release.
    fn finish(&mut self, answer: Checked) {
        let requested = std::mem::take(&mut self.requested);
        match answer {
            Ok(Some(release)) => {
                // Download and Later: never "Update and Restart" (see the module docs).
                let mut prompt = StartupPrompt::new(&crate::UPDATE_APP, release, running_version());
                prompt.is_self_updating = false;
                self.prompt = Some(prompt);
                self.status = Status::Idle;
            }
            Ok(None) if requested => self.status = Status::UpToDate,
            Ok(None) => {}
            Err(e) => {
                log::warn!("GridCraft update check failed: {e}");
                if requested {
                    self.status = Status::Failed(e);
                }
            }
        }
    }
}

pub(crate) fn tick(app: &mut SheetApp, ctx: &egui::Context) {
    let rt = &mut app.updates_rt;
    if std::mem::take(&mut rt.manual) {
        rt.begin(ctx, true);
    } else if rt.startup && !rt.started {
        rt.started = true;
        rt.begin(ctx, false);
    }
    let Some(rx) = &rt.check else { return };
    match rx.try_recv() {
        Ok(answer) => {
            rt.check = None;
            rt.finish(answer);
        }
        // The answer is on its way: look again soon, also when nothing else repaints.
        Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => {
            rt.check = None;
            rt.finish(Err("the update check stopped unexpectedly".into()));
        }
    }
}

pub(crate) fn show(app: &mut SheetApp, ctx: &egui::Context) {
    let mut open: Option<String> = None;
    let rt = &mut app.updates_rt;
    if let Some(prompt) = &mut rt.prompt {
        match prompt.show(ctx) {
            StartupPromptResponse::Pending => {}
            // The prompt has no "Update and Restart" here; were it ever answered, it downloads.
            StartupPromptResponse::Download | StartupPromptResponse::UpdateAndRestart => {
                open = Some(page_of(&prompt.release));
                rt.prompt = None;
            }
            StartupPromptResponse::Later => rt.prompt = None,
        }
    }
    // The result window answers a requested check, and never shows over the prompt.
    if rt.prompt.is_none() && !matches!(rt.status, Status::Idle) {
        match CheckResultDialog::new(&crate::UPDATE_APP, VERSION).show(ctx, &rt.status) {
            CheckResultResponse::Close | CheckResultResponse::RestartToUpdate => rt.status = Status::Idle,
            CheckResultResponse::CheckAgain => rt.manual = true,
            CheckResultResponse::None => {}
        }
    }
    if let Some(url) = open {
        open_page(app, &url);
    }
}

/// Opens `url` in the browser, where the system has one to open it with.
pub(crate) fn open_page(app: &mut SheetApp, url: &str) {
    match &app.services.open_url {
        Some(open) => open(url),
        None => app.message = Some(("GridCraft".into(), format!("Download the latest version from {url}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This copy is developed on its own: a build of it looks for the releases of its own
    /// repository, never those of the project craft-update lists as GridCraft.
    #[test]
    fn updates_come_from_this_repository() {
        assert_eq!(crate::UPDATE_APP.repo, "andriyanto/gridcraft");
        assert_ne!(crate::UPDATE_APP.repo, craft_update::suite::GRIDCRAFT.repo);
        assert_eq!(crate::UPDATE_APP.developer, "andriyanto");
        // The rest is the suite's description of the app: asset names and install ids.
        assert_eq!(crate::UPDATE_APP.id, craft_update::suite::GRIDCRAFT.id);
        assert_eq!(download_page(), "https://github.com/andriyanto/gridcraft/releases");
    }

    fn release(page: &str) -> Release {
        Release { tag: "v99.0.0".into(), html_url: page.into(), ..Release::default() }
    }

    /// An app whose check answers `answer`, and the pages it was asked to open.
    fn app_with(answer: Checked) -> (SheetApp, Arc<std::sync::Mutex<Vec<String>>>) {
        let opened: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let seen = opened.clone();
        let services =
            crate::Services { open_url: Some(Box::new(move |url: &str| seen.lock().unwrap().push(url.to_string()))), ..Default::default() };
        let mut app = SheetApp::new(gridcraft_engine::Session::new(), services);
        app.updates_rt.source = Some(Arc::new(move || answer.clone()));
        (app, opened)
    }

    /// Frames until the check running has answered.
    fn settle(app: &mut SheetApp, ctx: &egui::Context) {
        for _ in 0..2000 {
            tick(app, ctx);
            if app.updates_rt.check.is_none() && !app.updates_rt.manual {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the check never answered");
    }

    /// A newer release is offered for download: the prompt can't install it, and Download
    /// opens the release's page.
    #[test]
    fn a_newer_release_is_offered_for_download_and_never_installed() {
        let page = "https://github.com/andriyanto/gridcraft/releases/tag/v99.0.0";
        let (mut app, opened) = app_with(Ok(Some(release(page))));
        let ctx = egui::Context::default();
        start(&mut app, false);
        settle(&mut app, &ctx);
        let prompt = app.updates_rt.prompt.as_ref().expect("the release is on offer");
        assert!(!prompt.is_self_updating && !prompt.is_ready && !prompt.is_installing, "Download and Later only");
        assert_eq!(app.updates_rt.status, Status::Idle, "a check at start opens no result window");
        assert_eq!(page_of(&prompt.release), page);
        // The check at start runs once.
        tick(&mut app, &ctx);
        assert!(app.updates_rt.check.is_none());
        assert!(opened.lock().unwrap().is_empty(), "nothing opens until Download is pressed");
    }

    /// One frame of the update windows, and every text it painted with where it is.
    fn frame(app: &mut SheetApp, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.pos + t.galley.rect.center().to_vec2())),
                _ => {}
            }
        }
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| show(app, ui.ctx()));
        output.textures_delta.clear(); // Headless: no renderer takes the font atlas.
        let mut texts = Vec::new();
        output.shapes.iter().for_each(|c| collect(&c.shape, &mut texts));
        texts
    }

    /// What the person sees: Download and Later, no button that installs; Download opens the
    /// release's page in the browser and closes the prompt.
    #[test]
    fn the_prompt_has_download_and_later_and_download_opens_the_browser() {
        let page = "https://github.com/andriyanto/gridcraft/releases/tag/v99.0.0";
        let (mut app, opened) = app_with(Ok(Some(release(page))));
        let ctx = egui::Context::default();
        start(&mut app, false);
        settle(&mut app, &ctx);
        frame(&mut app, &ctx, Vec::new());
        let texts = frame(&mut app, &ctx, Vec::new());
        let has = |label: &str| texts.iter().any(|(t, _)| t.eq_ignore_ascii_case(label));
        assert!(has("Download") && has("Later"), "{texts:?}");
        assert!(!has("Update and Restart") && !has("Restart to Update"), "{texts:?}");
        let at = texts.iter().find(|(t, _)| t.eq_ignore_ascii_case("Download")).map(|(_, at)| *at).unwrap();
        let button =
            |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, &ctx, vec![button(true)]);
        frame(&mut app, &ctx, vec![button(false)]);
        assert_eq!(*opened.lock().unwrap(), [page]);
        assert!(app.updates_rt.prompt.is_none(), "the prompt closes once the page is open");
    }

    #[test]
    fn a_link_that_is_no_plain_web_link_opens_the_releases_page_instead() {
        for odd in ["file:///C:/Windows/System32/calc.exe", "javascript:alert(1)", "", "http://example.com/x"] {
            let page = page_of(&release(odd));
            assert!(page == odd || page == "https://github.com/andriyanto/gridcraft/releases", "{odd} → {page}");
            assert!(craft_update::release::safe_web_link(&page), "{odd} → {page}");
        }
    }

    /// Check for Updates… says what it found: nothing newer, or why it could not ask.
    #[test]
    fn a_requested_check_reports_its_outcome() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with(Ok(None));
        start(&mut app, true);
        tick(&mut app, &ctx);
        assert!(matches!(app.updates_rt.status, Status::Checking | Status::UpToDate));
        settle(&mut app, &ctx);
        assert_eq!(app.updates_rt.status, Status::UpToDate);
        assert!(app.updates_rt.prompt.is_none());

        let (mut app, _) = app_with(Err("offline".into()));
        start(&mut app, true);
        settle(&mut app, &ctx);
        assert_eq!(app.updates_rt.status, Status::Failed("offline".into()));
        // The same failure at start is not worth a window.
        let (mut app, _) = app_with(Err("offline".into()));
        start(&mut app, false);
        settle(&mut app, &ctx);
        assert_eq!(app.updates_rt.status, Status::Idle);
    }

    /// Download Latest… opens this repository's releases in the browser.
    #[test]
    fn the_download_command_opens_the_releases_page() {
        let (mut app, opened) = app_with(Ok(None));
        app.run("app.downloadUpdate", serde_json::json!({})).unwrap();
        assert_eq!(*opened.lock().unwrap(), ["https://github.com/andriyanto/gridcraft/releases"]);
    }
}
