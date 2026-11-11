// Release builds on Windows are GUI apps: no console window behind the main one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod diff;
mod files;
mod i18n;
mod queue;
mod settings;
mod theme;
mod updater;
mod view;

use diff::Row;
use fanhuaji_core::{
    Backups, ConvertFileResult, ConvertParams, EpubProgress, ModuleInfo, PreviewResult,
};
use gpui::{
    App, Bounds, Context, Entity, ExternalPaths, FocusHandle, KeyBinding, Menu, MenuItem, OsAction,
    PathPromptOptions, SharedString, Subscription, SystemMenuType, TitlebarOptions, Window,
    WindowAppearance, WindowBounds, WindowOptions, actions, prelude::*, px, size,
};
use gpui_component::WindowExt;
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::{Root, Theme, ThemeMode};
use i18n::{Locale, t, tp, translate_error};
use queue::{FileId, Queue, Status};
use settings::{OutputKey, Settings, ThemePref};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use theme::Pal;

actions!(
    fanhuaji,
    [
        OpenFiles,
        Convert,
        OpenSettings,
        About,
        Quit,
        Minimize,
        ToggleFullscreen,
        SelectPrev,
        SelectNext,
        RemoveSelected,
        CloseSettings
    ]
);

/// fanhuaji-core is built on reqwest + tokio, while GPUI runs its own executor.
static RT: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().expect("tokio runtime"));

/// Simplified output gets a Simplified Chinese face: a Traditional font would draw
/// Traditional-style glyphs for characters both scripts share.
const SIMPLIFIED_CONVERTERS: &[&str] = &["Simplified", "China", "WikiSimplified"];

const SUPPORTED: &[&str] = &[
    "txt", "srt", "ass", "ssa", "lrc", "vtt", "sub", "csv", "tsv", "json", "xml", "html", "htm",
    "md", "epub",
];

/// Open files, convert, settings, quit, minimize, fullscreen.
#[cfg(target_os = "macos")]
const KEYS: [&str; 6] = [
    "cmd-o",
    "cmd-enter",
    "cmd-,",
    "cmd-q",
    "cmd-m",
    "ctrl-cmd-f",
];
#[cfg(not(target_os = "macos"))]
const KEYS: [&str; 6] = ["ctrl-o", "ctrl-enter", "ctrl-,", "ctrl-q", "ctrl-m", "f11"];

const REPO_URL: &str = "https://github.com/fanhuaji-desktop/fanhuaji-gpui";

fn is_supported(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SUPPORTED.contains(&e.to_ascii_lowercase().as_str()))
}

fn is_epub(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
}

fn ext_label(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    if ext.is_empty() {
        "—".into()
    } else {
        ext.chars().take(4).collect()
    }
}

fn converter_label(key: &str) -> String {
    t(&format!("converter.short.{key}"))
}

/// "繁化姬 0.1.0", as the Tauri app titles its window.
fn window_title() -> String {
    format!("{} {}", t("app.title"), env!("CARGO_PKG_VERSION"))
}

fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{}", key.to_uppercase())
    } else {
        format!("Ctrl+{}", key.to_uppercase())
    }
}

enum PreviewBody {
    Message(SharedString),
    Error(SharedString),
    Diff { rows: Vec<Row>, simplified: bool },
}

struct Preview {
    stats: SharedString,
    body: PreviewBody,
}

impl Preview {
    fn message(text: impl Into<SharedString>) -> Self {
        Self {
            stats: SharedString::default(),
            body: PreviewBody::Message(text.into()),
        }
    }

    fn error(text: impl Into<SharedString>) -> Self {
        Self {
            stats: SharedString::default(),
            body: PreviewBody::Error(text.into()),
        }
    }

    /// `saved`: the converter a finished file was written with.
    fn diff(res: &PreviewResult, converter: &str, saved: bool) -> Self {
        let rows = diff::diff_lines(&res.original, &res.converted);
        let changed = rows
            .iter()
            .filter(|r| !matches!(r, Row::Equal(_) | Row::Delete(_)))
            .count();
        let chars = res.original.chars().count().to_string();
        let mut stats = tp(
            "preview.stats",
            &[("chars", &chars), ("changed", &changed.to_string())],
        );
        if res.truncated {
            stats = format!("{stats} · {}", t("preview.truncated"));
        }
        if saved {
            stats = format!(
                "{} · {stats}",
                tp("preview.saved", &[("mode", &converter_label(converter))])
            );
        }
        let body = if res.original.is_empty() && res.converted.is_empty() {
            PreviewBody::Message(t("preview.empty").into())
        } else {
            PreviewBody::Diff {
                rows,
                simplified: SIMPLIFIED_CONVERTERS.contains(&converter),
            }
        };
        Self {
            stats: stats.into(),
            body,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Replace,
    Modules,
    General,
}

enum Modules {
    Loading,
    Ready(Vec<ModuleInfo>),
    Failed(String),
}

/// The main view, for app-level actions that arrive when nothing in it has focus.
struct MainView(gpui::WeakEntity<Fanhuaji>);
impl gpui::Global for MainView {}

/// The toolbar popover that is open, if any.
#[derive(Clone, Copy, PartialEq)]
enum Popover {
    Mode,
    Output,
    /// The language picker in the settings drawer.
    Language,
}

/// A destructive click waiting for its confirming second click, as in the Tauri app.
#[derive(Clone, Copy, PartialEq)]
enum Armed {
    ClearAll,
    ClearDone,
    Remove(FileId),
}

struct Fanhuaji {
    queue: Queue,
    settings: Settings,
    settings_path: PathBuf,
    client: reqwest::Client,
    backups: Arc<Backups>,
    focus: FocusHandle,
    selected: Option<FileId>,
    preview: Option<Preview>,
    /// Bumped per preview request, so a slow response can't replace a newer one.
    preview_gen: u64,
    show_settings: bool,
    popover: Option<Popover>,
    tab: Tab,
    modules: Modules,
    module_category: String,
    armed: Option<Armed>,
    /// Files finished and total in the running batch, for the progress bar.
    batch: Option<(usize, usize)>,
    /// Files whose backup is being put back; a second Undo/Reconvert click is ignored.
    restoring: std::collections::HashSet<FileId>,
    dark: bool,
    /// A one-off message for the status bar, e.g. that a drop was cut short.
    notice: Option<String>,
    /// The update section's status line.
    update_status: String,
    /// A newer release found by a check, shown as a chip in the top bar.
    update: Option<updater::Release>,
    /// The verified zip of `update`, kept so a second confirmation doesn't download it again.
    downloaded: Option<Arc<Vec<u8>>>,
    /// Downloading or installing an update; the chip ignores clicks meanwhile.
    installing: bool,
    pre_replace: Entity<TextareaState>,
    post_replace: Entity<TextareaState>,
    protect_replace: Entity<TextareaState>,
    custom_suffix: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Fanhuaji {
    fn new(
        settings_path: PathBuf,
        client: reqwest::Client,
        backups: Arc<Backups>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = Settings::load(&settings_path);
        i18n::set_locale(
            settings
                .locale
                .unwrap_or_else(|| Locale::detect(sys_locale::get_locale().as_deref())),
        );

        let textarea =
            |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let (value, placeholder) = (value.to_string(), t(placeholder));
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .rows(4)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let pre_replace = textarea(
            &settings.pre_replace,
            "settings.replace.pairPlaceholder",
            window,
            cx,
        );
        let post_replace = textarea(
            &settings.post_replace,
            "settings.replace.pairPlaceholder",
            window,
            cx,
        );
        let protect_replace = textarea(
            &settings.protect_replace,
            "settings.replace.protectPlaceholder",
            window,
            cx,
        );
        let suffix = settings.custom_suffix.clone();
        let custom_suffix = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("converted")
                .default_value(suffix)
        });

        // Saved on every edit, like the Tauri app, so a rule being typed survives quitting.
        let mut subscriptions = vec![
            cx.subscribe(&pre_replace, |app, s, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    app.settings.pre_replace = s.read(cx).value().to_string();
                    app.settings_changed(cx);
                }
            }),
            cx.subscribe(&post_replace, |app, s, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    app.settings.post_replace = s.read(cx).value().to_string();
                    app.settings_changed(cx);
                }
            }),
            cx.subscribe(&protect_replace, |app, s, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    app.settings.protect_replace = s.read(cx).value().to_string();
                    app.settings_changed(cx);
                }
            }),
            cx.subscribe(&custom_suffix, |app, s, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    app.settings.custom_suffix = s.read(cx).value().to_string();
                    app.settings_changed(cx);
                }
            }),
        ];
        subscriptions.push(cx.observe_window_appearance(window, |app, window, cx| {
            app.apply_theme(window, cx);
        }));

        let job = {
            let client = client.clone();
            RT.spawn(async move { fanhuaji_core::fetch_service_info(&client).await })
        };
        cx.spawn(async move |this, cx| {
            let modules = match job.await {
                Ok(Ok(info)) => Modules::Ready(info.modules),
                Ok(Err(e)) => Modules::Failed(e),
                Err(e) => Modules::Failed(e.to_string()),
            };
            let _ = this.update(cx, |app, cx| {
                if let Modules::Ready(list) = &modules {
                    app.module_category =
                        list.first().map(|m| m.category.clone()).unwrap_or_default();
                }
                app.modules = modules;
                cx.notify();
            });
        })
        .detach();

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let mut app = Self {
            queue: Queue::default(),
            settings,
            settings_path,
            client,
            backups,
            focus,
            selected: None,
            preview: None,
            preview_gen: 0,
            show_settings: false,
            popover: None,
            tab: Tab::Replace,
            modules: Modules::Loading,
            module_category: String::new(),
            armed: None,
            batch: None,
            restoring: Default::default(),
            dark: false,
            notice: None,
            update_status: String::new(),
            update: None,
            downloaded: None,
            installing: false,
            pre_replace,
            post_replace,
            protect_replace,
            custom_suffix,
            _subscriptions: subscriptions,
        };
        app.apply_theme(window, cx);
        app.check_updates(true, window, cx);
        app
    }

    /// `silent`: the startup check, which only shows the chip; a manual check that finds an
    /// update asks to install it right away, as in the Tauri app.
    fn check_updates(&mut self, silent: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !silent {
            self.update_status = t("update.checking");
            cx.notify();
        }
        let client = self.client.clone();
        let job = RT.spawn(async move { updater::check(&client, env!("CARGO_PKG_VERSION")).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |app, window, cx| {
                app.update_status = match &result {
                    Ok(Some(r)) if installable(r) => tp("update.found", &[("version", &r.version)]),
                    Ok(Some(r)) => tp("update.downloadAvailable", &[("version", &r.version)]),
                    Ok(None) if !silent => t("update.upToDate"),
                    Err(e) if !silent => tp("update.failed", &[("error", &translate_error(e))]),
                    _ => String::new(),
                };
                if let Ok(Some(release)) = result {
                    if app.update.as_ref() != Some(&release) {
                        app.downloaded = None;
                    }
                    app.update = Some(release);
                    if !silent {
                        app.prompt_install(window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask, then download, verify, install and restart. A running batch blocks it; Undo
    /// that the restart would lose is spelled out. Without an asset for this platform (or
    /// in a dev build) the release page opens instead.
    fn prompt_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(release) = self.update.clone() else {
            return;
        };
        if !installable(&release) {
            cx.open_url(&release.url);
            return;
        }
        if self.installing {
            return;
        }
        let converting = self.queue.converting();
        let undoable = Queue::undoable(&self.queue.files);
        let warning = if converting {
            t("update.busy")
        } else if undoable > 0 {
            tp("update.undoWarning", &[("count", &undoable.to_string())])
        } else {
            String::new()
        };
        let mut description = vec![t("update.prompt")];
        description.extend([warning, release.notes.trim().to_string()]);
        description.retain(|line| !line.is_empty());
        let description = description.join("\n\n");
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let alert = alert
                .title(tp("update.found", &[("version", &release.version)]))
                .description(description.clone());
            if converting {
                return alert.ok_text(t("update.later"));
            }
            let view = view.clone();
            alert
                .show_cancel(true)
                .ok_text(t("update.install"))
                .cancel_text(t("update.later"))
                .on_ok(move |_, window, cx| {
                    let _ = view.update(cx, |app, cx| app.download_update(undoable, window, cx));
                    true
                })
        });
    }

    /// `warned`: how many overwritten files the user agreed to lose Undo for.
    fn download_update(&mut self, warned: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asset) = self.update.as_ref().and_then(|r| r.asset.clone()) else {
            return;
        };
        self.installing = true;
        if let Some(zip) = self.downloaded.clone() {
            self.finish_update(zip, warned, window, cx);
            return;
        }
        self.update_status = t("update.downloading");
        cx.notify();
        let client = self.client.clone();
        let job = RT.spawn(async move { updater::download(&client, &asset).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |app, window, cx| match result {
                Ok(zip) => {
                    let zip = Arc::new(zip);
                    app.downloaded = Some(zip.clone());
                    app.finish_update(zip, warned, window, cx);
                }
                Err(e) => app.update_failed(&e, cx),
            });
        })
        .detach();
    }

    /// Install once no batch runs, asking again if more files became undoable meanwhile.
    fn finish_update(
        &mut self,
        zip: Arc<Vec<u8>>,
        warned: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.queue.converting() {
            self.update_status = t("update.waiting");
            cx.notify();
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let _ = this.update_in(cx, |app, window, cx| {
                    app.finish_update(zip, warned, window, cx)
                });
            })
            .detach();
            return;
        }
        if Queue::undoable(&self.queue.files) > warned {
            self.installing = false;
            self.prompt_install(window, cx);
            return;
        }
        self.update_status = t("update.restarting");
        cx.notify();
        // shortcut: a batch started during the install itself (about a second) is cut off
        // by the restart, as in the Tauri app; block `start` while installing if that matters.
        let job = RT.spawn_blocking(move || {
            let exe = std::env::current_exe().map_err(|e| format!("UPDATE_INSTALL_FAILED:{e}"))?;
            let target = updater::install_target(&exe);
            updater::install(&zip, &target)?;
            updater::relaunch(&target).map_err(|e| format!("UPDATE_INSTALL_FAILED:{e}"))
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => cx.quit(),
                Err(e) => app.update_failed(&e, cx),
            });
        })
        .detach();
    }

    fn update_failed(&mut self, error: &str, cx: &mut Context<Self>) {
        self.installing = false;
        self.update_status = tp("update.failed", &[("error", &translate_error(error))]);
        cx.notify();
    }

    fn pal(&self) -> &'static Pal {
        if self.dark {
            &theme::DARK
        } else {
            &theme::LIGHT
        }
    }

    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dark = match self.settings.theme {
            ThemePref::Light => false,
            ThemePref::Dark => true,
            ThemePref::System => matches!(
                window.appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        let mode = if self.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        Theme::change(mode, Some(window), cx);
        // Always shown, so the module list and queue visibly scroll (the Tauri app styles
        // permanent scrollbars).
        Theme::global_mut(cx).scrollbar_mode = gpui_component::scroll::ScrollbarMode::Always;
        cx.notify();
    }

    /// Re-translate what isn't rebuilt on render: menus, window title, placeholders.
    fn apply_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_menus(menus());
        window.set_window_title(&window_title());
        for (state, key) in [
            (&self.pre_replace, "settings.replace.pairPlaceholder"),
            (&self.post_replace, "settings.replace.pairPlaceholder"),
            (&self.protect_replace, "settings.replace.protectPlaceholder"),
        ] {
            state.update(cx, |s, cx| s.set_placeholder(t(key), window, cx));
        }
        self.refresh_preview(cx);
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            // shortcut: logged only; show it in the UI once there is a notification area.
            eprintln!(
                "settings not saved to {}: {e}",
                self.settings_path.display()
            );
        }
        cx.notify();
    }

    /// Folders are walked off the UI thread: a large tree would otherwise freeze the window.
    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let paths = paths.to_vec();
        let job = RT.spawn_blocking(move || files::expand(&paths, files::MAX_FILES));
        cx.spawn(async move |this, cx| {
            if let Ok((found, cut)) = job.await {
                let _ = this.update(cx, |app, cx| app.add_found(found, cut, cx));
            }
        })
        .detach();
    }

    fn add_found(&mut self, found: Vec<PathBuf>, cut: bool, cx: &mut Context<Self>) {
        self.armed = None;
        let mut first_added = None;
        self.notice = cut.then(|| tp("files.tooMany", &[("count", &files::MAX_FILES.to_string())]));
        for path in found {
            if self.queue.add(path) && first_added.is_none() {
                first_added = self.queue.files.last().map(|f| f.id);
            }
        }
        // As in the Tauri app, adding files restarts a stopped queue.
        if self.settings.auto_convert {
            self.start(None, cx);
        }
        // Started first, so the preview waits for the conversion instead of asking the API.
        // Not `select`: that would hide an open settings panel without moving focus.
        if let Some(id) = first_added {
            self.selected = Some(id);
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            // Folders are expanded to their supported files.
            directories: true,
            multiple: true,
            prompt: Some(t("dialog.openFiles").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |app, cx| app.add_paths(&paths, cx));
            }
        })
        .detach();
    }

    fn pick_save_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("dialog.pickFolder").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |app, cx| {
                    app.settings.save_folder = Some(dir);
                    app.settings_changed(cx);
                });
            }
        })
        .detach();
    }

    fn open_settings(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.armed = None;
        self.show_settings = true;
        self.tab = tab;
        cx.notify();
    }

    /// Hide settings and give focus back to the window, whose actions and keys would
    /// otherwise stop working once the focused text field is no longer drawn.
    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.show_settings {
            self.show_settings = false;
            // Settings may have changed what the pending preview should show.
            self.refresh_preview(cx);
        }
    }

    fn select(&mut self, id: FileId, cx: &mut Context<Self>) {
        self.armed = None;
        self.selected = Some(id);
        self.show_settings = false;
        self.refresh_preview(cx);
    }

    /// Move the selection up or down the list, as the arrow keys do in the Tauri app.
    fn select_step(&mut self, down: bool, cx: &mut Context<Self>) {
        let ids: Vec<FileId> = self.queue.files.iter().map(|f| f.id).collect();
        let Some(&first) = ids.first() else {
            return;
        };
        let next = match self
            .selected
            .and_then(|id| ids.iter().position(|&x| x == id))
        {
            Some(i) if down => ids[(i + 1).min(ids.len() - 1)],
            Some(i) => ids[i.saturating_sub(1)],
            None => first,
        };
        self.select(next, cx);
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            return;
        };
        if self
            .queue
            .get(id)
            .is_some_and(|f| matches!(f.status, Status::Converting(_)))
        {
            return;
        }
        let undoable = Queue::undoable(self.queue.get(id));
        self.confirm_undo_loss(Armed::Remove(id), undoable, cx, |app, _| {
            app.queue.remove(id)
        });
    }

    /// After files leave the list: drop the selection if its file is gone.
    fn removed(&mut self, cx: &mut Context<Self>) {
        self.armed = None;
        if self.selected.is_some_and(|id| self.queue.get(id).is_none()) {
            self.selected = None;
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    /// Run `action` now if it loses no Undo; otherwise arm it so a second click confirms.
    fn confirm_undo_loss(
        &mut self,
        armed: Armed,
        undoable: usize,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        if undoable > 0 && self.armed != Some(armed) {
            self.armed = Some(armed);
            cx.notify();
            return;
        }
        action(self, cx);
        self.removed(cx);
    }

    fn reveal(&mut self, id: FileId, cx: &mut Context<Self>) {
        let Some(file) = self.queue.get_mut(id) else {
            return;
        };
        let Some(out) = file.output.clone() else {
            return;
        };
        if out.exists() {
            cx.reveal_path(&out);
        } else {
            file.status = Status::Failed(format!("REVEAL_FAILED:{}", out.display()));
            cx.notify();
        }
    }

    /// Put an overwritten file's original back, then run `then` on success.
    fn restore(
        &mut self,
        id: FileId,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if !self.restoring.insert(id) {
            return;
        }
        cx.notify();
        let backups = self.backups.clone();
        let job = RT.spawn_blocking(move || backups.restore(&id.to_string()));
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |app, cx| {
                app.restoring.remove(&id);
                let Some(file) = app.queue.get_mut(id) else {
                    return;
                };
                match result {
                    Ok(()) => {
                        file.status = Status::Restored;
                        file.saved = None;
                        file.overwritten = false;
                        then(app, cx);
                    }
                    Err(e) => file.status = Status::Failed(e),
                }
                if app.selected == Some(id) {
                    app.refresh_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Convert a finished file again with the current settings. An overwritten file gets
    /// its original back first, so the backup is never replaced by converted text.
    fn reconvert(&mut self, id: FileId, cx: &mut Context<Self>) {
        if self.queue.converting() {
            return;
        }
        let requeue = move |app: &mut Self, cx: &mut Context<Self>| {
            if let Some(file) = app.queue.get_mut(id) {
                file.status = Status::Pending;
                file.saved = None;
                app.start(Some(id), cx);
            }
        };
        if self.queue.get(id).is_some_and(|f| f.overwritten) {
            self.restore(id, cx, requeue);
        } else {
            requeue(self, cx);
        }
    }

    /// Quit, after asking when that would drop running work or the session's Undo.
    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let converting = self.queue.converting();
        let undoable = Queue::undoable(&self.queue.files);
        if !converting && undoable == 0 {
            cx.quit();
            return;
        }
        let mut lines = Vec::new();
        if converting {
            lines.push(t("quit.converting"));
        }
        if undoable > 0 {
            lines.push(tp("quit.undoWarning", &[("count", &undoable.to_string())]));
        }
        let description = lines.join("\n");
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(t("quit.title"))
                .description(description.clone())
                .show_cancel(true)
                .ok_text(t("quit.confirm"))
                .cancel_text(t("quit.cancel"))
                .on_ok(|_, _, cx| {
                    cx.quit();
                    true
                })
        });
    }

    /// Show the selected file's diff. A finished file shows what was written; a pending
    /// one asks the API for a preview of its start with the current settings.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_gen += 1;
        cx.notify();
        let Some(file) = self.selected.and_then(|id| self.queue.get(id)) else {
            self.preview = None;
            return;
        };
        self.preview = Some(match (&file.status, &file.saved) {
            (Status::Failed(e), _) => Preview::error(translate_error(e)),
            _ if is_epub(&file.path) => Preview::message(t("preview.epubUnsupported")),
            (_, Some((res, key))) => Preview::diff(res, &key.converter, true),
            (Status::Converting(_), _) => Preview::message(t("preview.converting")),
            // It converts soon and brings its own preview; don't pay for a second request.
            (Status::Pending, _)
                if self.queue.converting()
                    || self.settings.auto_convert && !self.queue.stopped() =>
            {
                Preview::message(t("preview.converting"))
            }
            _ => {
                let generation = self.preview_gen;
                let params = self.settings.params(&file.path, String::new());
                let client = self.client.clone();
                let converter = self.settings.converter.clone();
                let job =
                    RT.spawn(async move { fanhuaji_core::preview_convert(&client, &params).await });
                cx.spawn(async move |this, cx| {
                    let preview = match job.await {
                        Ok(Ok(res)) => Preview::diff(&res, &converter, false),
                        Ok(Err(e)) => Preview::error(translate_error(&e)),
                        Err(e) => Preview::error(e.to_string()),
                    };
                    let _ = this.update(cx, |app, cx| {
                        if app.preview_gen == generation {
                            app.preview = Some(preview);
                            cx.notify();
                        }
                    });
                })
                .detach();
                Preview::message(t("preview.loading"))
            }
        });
    }

    /// Convert pending files one at a time, or only `only` (Reconvert). Stop takes effect
    /// before the next file; files removed meanwhile are skipped.
    fn start(&mut self, only: Option<FileId>, cx: &mut Context<Self>) {
        self.armed = None;
        let Some(ids) = self.queue.begin(only) else {
            return;
        };
        self.batch = Some((0, ids.len()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            for id in ids {
                if this
                    .update(cx, |app, _| app.queue.stop_requested())
                    .unwrap_or(true)
                {
                    break;
                }
                let next = this.update(cx, |app, cx| {
                    let file = app
                        .queue
                        .get_mut(id)
                        .filter(|f| f.status == Status::Pending)?;
                    file.status = Status::Converting(None);
                    let path = file.path.clone();
                    if app.selected == Some(id) {
                        app.refresh_preview(cx);
                    }
                    cx.notify();
                    Some((
                        app.settings.params(&path, id.to_string()),
                        is_epub(&path),
                        app.settings.output_key(),
                        app.client.clone(),
                        app.backups.clone(),
                    ))
                });
                let Ok(Some((params, epub, key, client, backups))) = next else {
                    let _ = this.update(cx, |app, _| app.batch_step());
                    continue;
                };

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EpubProgress>();
                let job = RT.spawn(convert(params, epub, client, backups, tx));
                // The sender lives inside the job, so this ends when the job does.
                while let Some(p) = rx.recv().await {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(f) = app.queue.get_mut(id) {
                            f.status = Status::Converting(Some((p.chapter_index, p.chapter_total)));
                            f.chapter = p.chapter_name;
                            cx.notify();
                        }
                    });
                }
                let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
                let _ = this.update(cx, |app, cx| {
                    if let Some(f) = app.queue.get_mut(id) {
                        apply_result(f, result, key);
                    }
                    app.batch_step();
                    if app.selected == Some(id) {
                        app.refresh_preview(cx);
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |app, cx| {
                app.queue.end(only);
                app.batch = None;
                // Files dropped in mid-batch are still waiting.
                if app.queue.should_resume(app.settings.auto_convert) {
                    app.start(None, cx);
                }
                // A selected file left pending (Stop, or auto-convert off) said "converting".
                let pending = |app: &Self| {
                    app.selected
                        .and_then(|id| app.queue.get(id))
                        .is_some_and(|f| f.status == Status::Pending)
                };
                if !app.queue.converting() && pending(app) {
                    app.refresh_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_step(&mut self) {
        if let Some((done, _)) = &mut self.batch {
            *done += 1;
        }
    }
}

fn apply_result(
    f: &mut queue::FileItem,
    result: Result<ConvertFileResult, String>,
    key: OutputKey,
) {
    f.chapter.clear();
    match result {
        Ok(r) => {
            f.overwritten = r.backup_path.is_some();
            f.output = Some(PathBuf::from(&r.output_path));
            f.saved = r.preview.map(|p| (p, key));
            f.status = match r.warnings {
                Some(w) => Status::Warning(w),
                None => Status::Done(r.output_name),
            };
        }
        Err(e) => f.status = Status::Failed(e),
    }
}

async fn convert(
    params: ConvertParams,
    epub: bool,
    client: reqwest::Client,
    backups: Arc<Backups>,
    tx: tokio::sync::mpsc::UnboundedSender<EpubProgress>,
) -> Result<ConvertFileResult, String> {
    let on_progress = move |p| {
        let _ = tx.send(p);
    };
    if epub {
        fanhuaji_core::convert_epub(&client, &backups, &params, on_progress).await
    } else {
        fanhuaji_core::convert_file(&client, &backups, &params, on_progress).await
    }
}

fn menus() -> Vec<Menu> {
    use gpui_component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    vec![
        Menu::new(t("app.title")).items([
            MenuItem::action(t("menu.about"), About),
            MenuItem::separator(),
            MenuItem::action(t("menu.settings"), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t("menu.services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t("menu.quit"), Quit),
        ]),
        Menu::new(t("menu.file")).items([
            MenuItem::action(t("menu.openFiles"), OpenFiles),
            MenuItem::action(t("menu.convert"), Convert),
            MenuItem::separator(),
            MenuItem::action(t("menu.closeWindow"), Quit),
        ]),
        Menu::new(t("menu.edit")).items([
            MenuItem::os_action(t("menu.undo"), Undo, OsAction::Undo),
            MenuItem::os_action(t("menu.redo"), Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t("menu.cut"), Cut, OsAction::Cut),
            MenuItem::os_action(t("menu.copy"), Copy, OsAction::Copy),
            MenuItem::os_action(t("menu.paste"), Paste, OsAction::Paste),
            MenuItem::os_action(t("menu.selectAll"), SelectAll, OsAction::SelectAll),
        ]),
        Menu::new(t("menu.window")).items([
            MenuItem::action(t("menu.minimize"), Minimize),
            MenuItem::action(t("menu.fullscreen"), ToggleFullscreen),
        ]),
    ]
}

/// Whether `release` can be installed in place: release builds only, so `cargo run` never
/// replaces the dev binary.
fn installable(release: &updater::Release) -> bool {
    release.asset.is_some() && !cfg!(debug_assertions)
}

/// macOS reads a bundle's icon from Info.plist; a bare binary (`cargo run`) needs it set.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    static ICON: &[u8] = include_bytes!("../assets/icon.png");
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(ICON);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // SAFETY: on the main thread (checked above) with a valid, retained NSImage.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

fn main() {
    let config = dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fanhuaji-gpui");
    let cache = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fanhuaji-gpui");
    // Cleared on every launch: Undo only covers this session, as in the Tauri app.
    if let Ok(exe) = std::env::current_exe() {
        updater::cleanup(&updater::install_target(&exe));
    }
    let backups = Arc::new(Backups::new(cache.join("backups")).expect("backup folder"));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("HTTP client");

    gpui_platform::application()
        .with_assets(gpui_kit_assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            #[cfg(target_os = "macos")]
            set_dock_icon();
            let [open, convert, settings, quit, minimize, fullscreen] = KEYS;
            cx.bind_keys([
                KeyBinding::new("up", SelectPrev, Some("FileList")),
                KeyBinding::new("down", SelectNext, Some("FileList")),
                KeyBinding::new("backspace", RemoveSelected, Some("FileList")),
                KeyBinding::new("delete", RemoveSelected, Some("FileList")),
                KeyBinding::new("escape", CloseSettings, Some("FileList")),
                KeyBinding::new(open, OpenFiles, None),
                KeyBinding::new(convert, Convert, None),
                KeyBinding::new(settings, OpenSettings, None),
                KeyBinding::new(quit, Quit, None),
                KeyBinding::new(minimize, Minimize, None),
                KeyBinding::new(fullscreen, ToggleFullscreen, None),
            ]);
            // Reached when focus is outside the view: still ask before dropping work or Undo.
            cx.on_action(|_: &Quit, cx| {
                let view = cx.try_global::<MainView>().and_then(|v| v.0.upgrade());
                let window = cx.windows().first().copied();
                if let (Some(view), Some(window)) = (view, window) {
                    let _ = window.update(cx, |_, window, cx| {
                        view.update(cx, |app, cx| app.request_quit(window, cx))
                    });
                } else {
                    cx.quit();
                }
            });

            let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Fanhuaji".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let app = cx.new(|cx| {
                        Fanhuaji::new(config.join("settings.json"), client, backups, window, cx)
                    });
                    // The locale is known now; set what depends on it.
                    window.set_window_title(&window_title());
                    cx.set_menus(menus());
                    cx.set_global(MainView(app.downgrade()));
                    let handle = app.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        handle.update(cx, |app, cx| app.request_quit(window, cx));
                        false
                    });
                    cx.new(|cx| Root::new(app, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_extension_case_insensitively() {
        let dir = std::env::temp_dir().join("fanhuaji-gpui-test");
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("a.SRT");
        let no = dir.join("a.png");
        std::fs::write(&ok, "x").unwrap();
        std::fs::write(&no, "x").unwrap();
        assert!(is_supported(&ok));
        assert!(!is_supported(&no));
        assert!(!is_supported(&dir), "directories are skipped");
        assert!(is_epub(Path::new("book.EPUB")));
    }

    #[test]
    fn ext_label_is_short_uppercase() {
        assert_eq!(ext_label(Path::new("a.srt")), "SRT");
        assert_eq!(ext_label(Path::new("a.epubx")), "EPUB");
        assert_eq!(ext_label(Path::new("noext")), "—");
    }
}
