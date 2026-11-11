mod diff;
mod i18n;
mod queue;
mod settings;
mod theme;
mod updater;

use diff::Row;
use fanhuaji_core::{
    Backups, ConvertFileResult, ConvertParams, EpubProgress, ModuleInfo, PreviewResult,
};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, ExternalPaths, FocusHandle, HighlightStyle,
    KeyBinding, Menu, MenuItem, OsAction, PathPromptOptions, SharedString, StyledText,
    Subscription, SystemMenuType, TitlebarOptions, Window, WindowAppearance, WindowBounds,
    WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use gpui_component::WindowExt;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::switch::Switch;
use gpui_component::{Root, Theme, ThemeMode};
use i18n::{Locale, t, tp, translate_error};
use queue::{FileId, Queue, Status};
use settings::{ModuleChoice, Naming, OutputKey, Settings, ThemePref};
use std::ops::Range;
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

const CONVERTERS: &[&str] = &[
    "Traditional",
    "Taiwan",
    "Hongkong",
    "Simplified",
    "China",
    "Bopomofo",
    "Mars",
    "Pinyin",
    "WikiSimplified",
    "WikiTraditional",
];

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
    tab: Tab,
    modules: Modules,
    module_category: String,
    armed: Option<Armed>,
    /// Files finished and total in the running batch, for the progress bar.
    batch: Option<(usize, usize)>,
    /// Files whose backup is being put back; a second Undo/Reconvert click is ignored.
    restoring: std::collections::HashSet<FileId>,
    dark: bool,
    /// The update section's status line.
    update_status: String,
    /// A newer release found by a check, shown as a chip in the top bar.
    update: Option<updater::Release>,
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
            tab: Tab::Replace,
            modules: Modules::Loading,
            module_category: String::new(),
            armed: None,
            batch: None,
            restoring: Default::default(),
            dark: false,
            update_status: String::new(),
            update: None,
            pre_replace,
            post_replace,
            protect_replace,
            custom_suffix,
            _subscriptions: subscriptions,
        };
        app.apply_theme(window, cx);
        app.check_updates(true, cx);
        app
    }

    /// `silent`: the startup check, which only speaks up when there is an update.
    fn check_updates(&mut self, silent: bool, cx: &mut Context<Self>) {
        if !silent {
            self.update_status = t("update.checking");
            cx.notify();
        }
        let client = self.client.clone();
        let job = RT.spawn(async move { updater::check(&client, env!("CARGO_PKG_VERSION")).await });
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |app, cx| {
                app.update_status = match &result {
                    Ok(Some(r)) => tp("update.downloadAvailable", &[("version", &r.version)]),
                    Ok(None) if !silent => t("update.upToDate"),
                    Err(e) if !silent => tp("update.failed", &[("error", &translate_error(e))]),
                    _ => String::new(),
                };
                if let Ok(Some(release)) = result {
                    app.update = Some(release);
                }
                cx.notify();
            });
        })
        .detach();
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
        cx.notify();
    }

    /// Re-translate what isn't rebuilt on render: menus, window title, placeholders.
    fn apply_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_menus(menus());
        window.set_window_title(&t("app.title"));
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

    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.armed = None;
        let mut first_added = None;
        for path in paths.iter().filter(|p| is_supported(p)) {
            if self.queue.add(path.clone()) && first_added.is_none() {
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
            directories: false,
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

fn button(
    p: &Pal,
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg, hover) = if primary {
        (p.accent, p.on_accent, p.accent_hover)
    } else {
        (p.surface, p.text, p.hover)
    };
    div()
        .id(id)
        .px_4()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(rgb(if primary { p.accent } else { p.border }))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(hover)))
        .child(label.into())
}

fn chip(
    p: &Pal,
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.hover;
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_full()
        .border_1()
        .border_color(rgb(if selected { p.accent } else { p.border }))
        .bg(rgb(if selected { p.accent_soft } else { p.surface }))
        .when(!selected, |d| d.hover(move |s| s.bg(rgb(hover))))
        .cursor_pointer()
        .child(label.into())
}

fn section(title: impl Into<SharedString>) -> gpui::Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title.into()),
    )
}

fn link(
    p: &Pal,
    id: &'static str,
    label: impl Into<SharedString>,
    url: &'static str,
) -> impl IntoElement {
    div()
        .id(id)
        .text_sm()
        .text_color(rgb(p.accent))
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(label.into())
        .on_click(move |_, _, cx| cx.open_url(url))
}

impl Render for Fanhuaji {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let converters = div()
            .flex_1()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(CONVERTERS.iter().map(|&key| {
                chip(p, key, converter_label(key), self.settings.converter == key).on_click(
                    cx.listener(move |app, _, _, cx| {
                        if !app.queue.converting() && app.settings.converter != key {
                            app.settings.converter = key.into();
                            app.settings_changed(cx);
                            app.refresh_preview(cx);
                        }
                    }),
                )
            }));
        let update_chip = self.update.as_ref().map(|r| {
            let url = r.url.clone();
            chip(
                p,
                "update-chip",
                tp("update.chip", &[("version", &r.version)]),
                true,
            )
            .on_click(move |_, _, cx| cx.open_url(&url))
        });
        let top = div()
            .flex()
            .items_start()
            .gap_3()
            .child(converters)
            .children(update_chip)
            .child(
                button(
                    p,
                    "settings",
                    t(if self.show_settings {
                        "settings.close"
                    } else {
                        "header.settings"
                    }),
                    false,
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    if app.show_settings {
                        app.close_settings(window, cx);
                    } else {
                        app.open_settings(app.tab, cx);
                    }
                })),
            );

        div()
            .id("root")
            .track_focus(&self.focus)
            .on_action(cx.listener(|app, _: &OpenFiles, _, cx| app.pick_files(cx)))
            .on_action(cx.listener(|app, _: &Convert, _, cx| app.start(None, cx)))
            .on_action(
                cx.listener(|app, _: &OpenSettings, _, cx| app.open_settings(Tab::Replace, cx)),
            )
            .on_action(cx.listener(|app, _: &About, _, cx| app.open_settings(Tab::General, cx)))
            .on_action(cx.listener(|app, _: &Quit, window, cx| app.request_quit(window, cx)))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ToggleFullscreen, window, _| window.toggle_fullscreen())
            // Text inputs have the deeper "Input" context, so arrows and delete stay theirs there.
            .key_context("FileList")
            // Only when the window itself has focus: a text field passes Backspace up when
            // its cursor is at the start, which must not remove a file.
            .on_action(cx.listener(|app, _: &SelectPrev, window, cx| {
                if app.focus.is_focused(window) {
                    app.select_step(false, cx)
                }
            }))
            .on_action(cx.listener(|app, _: &SelectNext, window, cx| {
                if app.focus.is_focused(window) {
                    app.select_step(true, cx)
                }
            }))
            .on_action(cx.listener(|app, _: &RemoveSelected, window, cx| {
                if app.focus.is_focused(window) {
                    app.remove_selected(cx)
                }
            }))
            .on_action(
                cx.listener(|app, _: &CloseSettings, window, cx| app.close_settings(window, cx)),
            )
            // Files can be dropped anywhere in the window, as in the Tauri app.
            .group("root")
            .on_drop(cx.listener(|app, paths: &ExternalPaths, _, cx| {
                app.add_paths(paths.paths(), cx);
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_3()
                    .child(self.render_list(cx))
                    .child(if self.show_settings {
                        self.render_settings(cx).into_any_element()
                    } else {
                        self.render_preview(cx).into_any_element()
                    }),
            )
            .child(self.render_status(cx))
            .child(self.render_drop_overlay())
    }
}

impl Fanhuaji {
    fn file_note(&self, f: &queue::FileItem) -> (String, u32) {
        let p = self.pal();
        if self.armed == Some(Armed::Remove(f.id)) {
            return (t("file.removeConfirm"), p.danger);
        }
        match &f.status {
            Status::Pending => (ext_label(&f.path), p.muted),
            Status::Converting(None) => (t("file.converting"), p.accent),
            Status::Converting(Some((i, n))) => {
                let name = if f.chapter.is_empty() {
                    String::new()
                } else {
                    format!(" {}", f.chapter)
                };
                (
                    tp(
                        "file.convertingChapter",
                        &[
                            ("current", &i.to_string()),
                            ("total", &n.to_string()),
                            ("name", &name),
                        ],
                    ),
                    p.accent,
                )
            }
            Status::Done(_) if f.overwritten => (t("file.overwritten"), p.success),
            Status::Done(out) => (tp("file.savedAs", &[("name", out)]), p.success),
            Status::Warning(w) => (
                tp(
                    "file.convertDoneWithWarnings",
                    &[("warnings", &translate_error(w))],
                ),
                p.warning,
            ),
            Status::Failed(e) => (translate_error(e), p.danger),
            Status::Restored => (t("file.restored"), p.muted),
        }
    }

    /// Shown only while files are dragged over the window: says what dropping will do.
    fn render_drop_overlay(&self) -> impl IntoElement {
        let p = self.pal();
        let mode = converter_label(&self.settings.converter);
        let text = if !self.settings.auto_convert {
            t("drop.release")
        } else if self.settings.naming == Naming::Overwrite {
            tp("drop.releaseOverwrite", &[("mode", &mode)])
        } else {
            tp("drop.releaseConvert", &[("mode", &mode)])
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .opacity(0.)
            .group_drag_over::<ExternalPaths>("root", |s| s.opacity(1.))
            .child(
                div()
                    .px_6()
                    .py_4()
                    .rounded_lg()
                    .border_2()
                    .border_dashed()
                    .border_color(rgb(p.accent))
                    .bg(rgb(p.surface))
                    .text_lg()
                    .text_color(rgb(p.accent))
                    .child(text),
            )
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let (accent, drop) = (p.accent, p.drop);
        div()
            .id("files")
            .w(px(320.))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(rgb(p.border))
            .group_drag_over::<ExternalPaths>("root", move |s| {
                s.border_color(rgb(accent)).bg(rgb(drop))
            })
            .when(self.queue.files.is_empty(), |d| {
                d.items_center()
                    .justify_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx)))
                    .child(div().text_lg().child(t("drop.title")))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.muted))
                            .child(tp("drop.shortcut", &[("key", &shortcut("o"))])),
                    )
                    .child(
                        div()
                            .px_6()
                            .text_xs()
                            .text_center()
                            .text_color(rgb(p.faint))
                            .child(t("drop.formats")),
                    )
            })
            .children(self.queue.files.iter().map(|f| {
                let id = f.id;
                let name = f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (note, color) = self.file_note(f);
                let removable = !matches!(f.status, Status::Converting(_));
                let armed = self.armed == Some(Armed::Remove(id));
                let small_action = |id: (&'static str, FileId), label: String, danger: bool| {
                    let (text, hover_bg) = (p.text, p.divider);
                    div()
                        .id(id)
                        .px_1()
                        .rounded_sm()
                        .text_sm()
                        .text_color(rgb(if danger { p.danger } else { p.muted }))
                        .hover(move |s| s.text_color(rgb(text)).bg(rgb(hover_bg)))
                        .child(label)
                };
                let (selected_bg, hover) = (p.accent_soft, p.hover);
                div()
                    .id(("file", id))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .cursor_pointer()
                    .when(self.selected == Some(id), |d| d.bg(rgb(selected_bg)))
                    .hover(move |s| s.bg(rgb(hover)))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        window.focus(&app.focus, cx);
                        app.select(id, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .w(px(40.))
                            .py_0p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(p.border))
                            .text_xs()
                            .text_center()
                            .text_color(rgb(p.muted))
                            .child(ext_label(&f.path)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(name))
                            .child(div().text_sm().text_color(rgb(color)).child(note)),
                    )
                    .when(f.is_finished() && f.output.is_some(), |d| {
                        d.child(small_action(("reveal", id), "📂".into(), false).on_click(
                            cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.reveal(id, cx);
                            }),
                        ))
                    })
                    .when(
                        f.overwritten && f.is_finished() && !self.restoring.contains(&id),
                        |d| {
                            d.child(small_action(("undo", id), t("file.undo"), false).on_click(
                                cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.restore(id, cx, |_, _| {});
                                }),
                            ))
                        },
                    )
                    .when(removable, |d| {
                        let label = if armed { "⚠" } else { "✕" };
                        d.child(small_action(("remove", id), label.into(), armed).on_click(
                            cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                let undoable = Queue::undoable(app.queue.get(id));
                                app.confirm_undo_loss(Armed::Remove(id), undoable, cx, |app, _| {
                                    app.queue.remove(id)
                                });
                            }),
                        ))
                    })
            }))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let q = &self.queue;
        let converting = q.converting();
        let c = q.counts();
        let done = q
            .files
            .iter()
            .filter(|f| f.is_finished() || f.status == Status::Restored)
            .count();
        let stat = |label: &str, n: usize, color: u32| {
            div()
                .flex()
                .gap_1()
                .child(div().text_color(rgb(p.muted)).child(t(label)))
                .child(div().text_color(rgb(color)).child(n.to_string()))
        };
        let armed_label = |armed: Armed, key: &str, leaving: usize| {
            if self.armed == Some(armed) {
                tp("status.confirmClear", &[("count", &leaving.to_string())])
            } else {
                t(key)
            }
        };
        let progress = self.batch.map(|(d, n)| d as f32 / n.max(1) as f32);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(progress, |d, ratio| {
                d.child(
                    div()
                        .h(px(3.))
                        .w_full()
                        .rounded_full()
                        .bg(rgb(p.divider))
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(rgb(p.accent))
                                .w(gpui::relative(ratio)),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap_3()
                            .text_sm()
                            .when(c.total > 0, |d| {
                                d.child(stat("status.success", c.success, p.success))
                                    .when(c.warning > 0, |d| {
                                        d.child(stat("status.warning", c.warning, p.warning))
                                    })
                                    .child(stat("status.error", c.error, p.danger))
                                    .child(div().text_color(rgb(p.muted)).child(tp(
                                        "queue.count",
                                        &[("count", &c.total.to_string())],
                                    )))
                            }),
                    )
                    .child(
                        button(p, "add", t("menu.openFiles"), false)
                            .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx))),
                    )
                    // Only worth a button when it clears less than "clear list" would.
                    .when(done > 0 && done < q.files.len(), |d| {
                        let leaving = Queue::undoable(q.files.iter().filter(|f| f.is_finished()));
                        d.child(
                            button(
                                p,
                                "clear-done",
                                armed_label(Armed::ClearDone, "status.clearDone", leaving),
                                false,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    app.confirm_undo_loss(
                                        Armed::ClearDone,
                                        leaving,
                                        cx,
                                        |app, _| app.queue.remove_completed(),
                                    );
                                },
                            )),
                        )
                    })
                    // Clearing mid-batch would hide files the running loop is still writing.
                    .when(!converting && !q.files.is_empty(), |d| {
                        let leaving = Queue::undoable(&q.files);
                        d.child(
                            button(
                                p,
                                "clear",
                                armed_label(Armed::ClearAll, "status.clear", leaving),
                                false,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    app.confirm_undo_loss(
                                        Armed::ClearAll,
                                        leaving,
                                        cx,
                                        |app, _| app.queue.files.clear(),
                                    );
                                },
                            )),
                        )
                    })
                    .when(!converting && c.error > 0, |d| {
                        d.child(
                            button(p, "retry", t("status.retry"), false).on_click(cx.listener(
                                |app, _, _, cx| {
                                    app.queue.reset_errors();
                                    app.start(None, cx);
                                },
                            )),
                        )
                    })
                    .when(converting, |d| {
                        let stopping = q.stop_requested();
                        d.child(
                            button(p, "stop", t("status.stop"), false)
                                .when(stopping, |b| b.opacity(0.5))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.queue.stop();
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!converting && c.pending > 0, |d| {
                        d.child(
                            button(
                                p,
                                "start",
                                format!("{} ({})", t("status.convert"), c.pending),
                                true,
                            )
                            .on_click(cx.listener(|app, _, _, cx| app.start(None, cx))),
                        )
                    }),
            )
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let pane = div()
            .id("preview")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.surface));
        let Some(preview) = &self.preview else {
            return pane
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().child(t("preview.emptyTitle")))
                .child(
                    div()
                        .px_8()
                        .text_sm()
                        .text_center()
                        .text_color(rgb(p.muted))
                        .child(t("preview.emptyHint")),
                );
        };
        // A finished file converted with other output settings than the current ones.
        let stale = self
            .selected
            .and_then(|id| self.queue.get(id))
            .filter(|f| f.is_finished() && !self.queue.converting())
            .and_then(|f| f.saved.as_ref())
            .is_some_and(|(_, key)| *key != self.settings.output_key());
        let name = self
            .selected
            .and_then(|id| self.queue.get(id))
            .and_then(|f| f.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pane = pane
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .child(div().truncate().child(name))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.muted))
                            .child(preview.stats.clone()),
                    ),
            )
            .when(stale, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .bg(rgb(p.row_replace))
                        .text_sm()
                        .child(div().flex_1().child(t("preview.stale")))
                        .child(
                            button(p, "reconvert", t("preview.reconvert"), false).on_click(
                                cx.listener(|app, _, _, cx| {
                                    if let Some(id) = app.selected {
                                        app.reconvert(id, cx);
                                    }
                                }),
                            ),
                        ),
                )
            });
        match &preview.body {
            PreviewBody::Message(m) => {
                pane.child(div().p_4().text_color(rgb(p.muted)).child(m.clone()))
            }
            PreviewBody::Error(m) => pane.child(
                div()
                    .p_4()
                    .text_color(rgb(p.danger))
                    .child(format!("⚠ {m}")),
            ),
            PreviewBody::Diff { rows, simplified } => {
                let mut line = 0;
                pane.children(rows.iter().map(|row| {
                    if !matches!(row, Row::Delete(_)) {
                        line += 1;
                    }
                    diff_row(p, row, line, *simplified)
                }))
            }
        }
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let tabs = div().flex().gap_2().children(
            [
                (Tab::Replace, "settings.tab.replace"),
                (Tab::Modules, "settings.tab.modules"),
                (Tab::General, "settings.tab.general"),
            ]
            .map(|(tab, key)| {
                chip(p, key, t(key), self.tab == tab).on_click(cx.listener(move |app, _, _, cx| {
                    app.tab = tab;
                    cx.notify();
                }))
            }),
        );
        let body = match self.tab {
            Tab::Replace => section(t("settings.replace.title"))
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.muted))
                        .child(t("settings.replace.preLabel")),
                )
                .child(Textarea::new(&self.pre_replace))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.muted))
                        .child(t("settings.replace.postLabel")),
                )
                .child(Textarea::new(&self.post_replace))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.muted))
                        .child(t("settings.replace.protectLabel")),
                )
                .child(Textarea::new(&self.protect_replace))
                .into_any_element(),
            Tab::Modules => self.render_modules(cx).into_any_element(),
            Tab::General => self.render_general(cx).into_any_element(),
        };
        div()
            .id("settings-pane")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scroll()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.surface))
            .child(tabs)
            .child(body)
    }

    fn render_modules(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let list = match &self.modules {
            Modules::Loading => {
                return section(t("settings.modules.title"))
                    .child(div().text_color(rgb(p.muted)).child(t("preview.loading")));
            }
            Modules::Failed(e) => {
                return section(t("settings.modules.title")).child(
                    div()
                        .text_color(rgb(p.danger))
                        .child(tp("modules.loadFailed", &[("error", &translate_error(e))])),
                );
            }
            Modules::Ready(list) => list,
        };
        let mut categories: Vec<&str> = Vec::new();
        for m in list {
            if !categories.contains(&m.category.as_str()) {
                categories.push(&m.category);
            }
        }
        let cats = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(categories.iter().enumerate().map(|(i, &c)| {
                let label = if c.is_empty() {
                    t("module.categoryUnknown")
                } else {
                    c.to_string()
                };
                let owned = c.to_string();
                chip(p, ("cat", i), label, self.module_category == c).on_click(cx.listener(
                    move |app, _, _, cx| {
                        app.module_category = owned.clone();
                        cx.notify();
                    },
                ))
            }));
        let rows = list
            .iter()
            .enumerate()
            .filter(|(_, m)| m.category == self.module_category)
            .map(|(i, m)| {
                let current = self
                    .settings
                    .modules
                    .get(&m.key)
                    .copied()
                    .unwrap_or_default();
                let choices = [
                    (ModuleChoice::Auto, "module.auto", "mod-auto"),
                    (ModuleChoice::Enable, "module.enable", "mod-on"),
                    (ModuleChoice::Disable, "module.disable", "mod-off"),
                ];
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(m.name.clone())
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(p.muted))
                                    .child(m.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .children(choices.map(|(choice, key, tag)| {
                                let module = m.key.clone();
                                chip(p, (tag, i), t(key), current == choice).on_click(cx.listener(
                                    move |app, _, _, cx| {
                                        if choice == ModuleChoice::Auto {
                                            app.settings.modules.remove(&module);
                                        } else {
                                            app.settings.modules.insert(module.clone(), choice);
                                        }
                                        app.settings_changed(cx);
                                    },
                                ))
                            })),
                    )
            });
        section(t("settings.modules.title"))
            .gap_3()
            .child(cats)
            .child(div().flex().flex_col().children(rows))
    }

    fn render_general(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let s = &self.settings;
        let folder = s
            .save_folder
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let naming_desc = match s.naming {
            Naming::Auto => "settings.save.naming.autoDesc",
            Naming::Suffix => "settings.save.naming.suffixDesc",
            Naming::Overwrite => "settings.save.naming.overwriteDesc",
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                section(t("naming.when")).child(
                    Switch::new("auto-convert")
                        .checked(s.auto_convert)
                        .label(t("settings.behavior.autoConvert"))
                        .on_click(cx.listener(|app, checked: &bool, _, cx| {
                            app.settings.auto_convert = *checked;
                            app.settings_changed(cx);
                        })),
                ),
            )
            .child(
                section(t("settings.save.folderLabel"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                chip(p, "folder-same", t("settings.save.same"), folder.is_none())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.settings.save_folder = None;
                                        app.settings_changed(cx);
                                    })),
                            )
                            .child(
                                chip(
                                    p,
                                    "folder-custom",
                                    t("settings.save.custom"),
                                    folder.is_some(),
                                )
                                .on_click(cx.listener(|app, _, _, cx| app.pick_save_folder(cx))),
                            ),
                    )
                    .when_some(folder, |d, f| {
                        d.child(div().text_sm().text_color(rgb(p.muted)).child(f))
                    }),
            )
            .child(
                section(t("naming.title"))
                    .child(
                        div().flex().gap_2().children(
                            [
                                (Naming::Auto, "settings.save.naming.auto"),
                                (Naming::Suffix, "settings.save.naming.suffix"),
                                (Naming::Overwrite, "settings.save.naming.overwrite"),
                            ]
                            .map(|(naming, key)| {
                                chip(p, key, t(key), s.naming == naming).on_click(cx.listener(
                                    move |app, _, _, cx| {
                                        app.settings.naming = naming;
                                        app.settings_changed(cx);
                                    },
                                ))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(if s.naming == Naming::Overwrite {
                                p.warning
                            } else {
                                p.muted
                            }))
                            .child(t(naming_desc)),
                    )
                    .when(s.naming == Naming::Suffix, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_sm().child(t("settings.save.suffixLabel")))
                                .child(div().w(px(200.)).child(Input::new(&self.custom_suffix))),
                        )
                    }),
            )
            .child(
                section(t("settings.language.title")).child(div().flex().gap_2().children(
                    i18n::LOCALES.map(|(locale, name)| {
                        chip(p, name, name, i18n::locale() == locale).on_click(cx.listener(
                            move |app, _, window, cx| {
                                i18n::set_locale(locale);
                                app.settings.locale = Some(locale);
                                app.settings_changed(cx);
                                app.apply_locale(window, cx);
                            },
                        ))
                    }),
                )),
            )
            .child(
                section(t("settings.appearance.title")).child(
                    div().flex().gap_2().children(
                        [
                            (ThemePref::System, "theme.system"),
                            (ThemePref::Light, "theme.light"),
                            (ThemePref::Dark, "theme.dark"),
                        ]
                        .map(|(pref, key)| {
                            chip(p, key, t(key), s.theme == pref).on_click(cx.listener(
                                move |app, _, window, cx| {
                                    app.settings.theme = pref;
                                    app.settings_changed(cx);
                                    app.apply_theme(window, cx);
                                },
                            ))
                        }),
                    ),
                ),
            )
            .child(
                section(t("settings.update.title")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            button(p, "check-update", t("settings.update.check"), false).on_click(
                                cx.listener(|app, _, _, cx| app.check_updates(false, cx)),
                            ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(p.muted))
                                .child(self.update_status.clone()),
                        )
                        .when_some(self.update.as_ref(), |d, r| {
                            let url = r.url.clone();
                            d.child(
                                div()
                                    .id("release-link")
                                    .text_sm()
                                    .text_color(rgb(p.accent))
                                    .cursor_pointer()
                                    .child("GitHub Releases")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                ),
            )
            .child(
                section(t("settings.about.title"))
                    .child(div().text_sm().child(format!(
                        "{} {}",
                        t("settings.about.version"),
                        env!("CARGO_PKG_VERSION")
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(link(
                                p,
                                "link-site",
                                "zhconvert.org",
                                "https://zhconvert.org",
                            ))
                            .child(link(
                                p,
                                "link-license",
                                t("settings.about.license"),
                                "https://docs.zhconvert.org/license/",
                            ))
                            .child(link(p, "link-repo", "GitHub", REPO_URL)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.muted))
                            .child(t("settings.about.apiNotice")),
                    ),
            )
    }
}

fn marked_text(text: &str, ranges: &[Range<usize>], mark: u32) -> StyledText {
    let style = HighlightStyle {
        background_color: Some(rgb(mark).into()),
        ..Default::default()
    };
    let text = if text.is_empty() { " " } else { text };
    StyledText::new(text.to_string()).with_highlights(ranges.iter().map(|r| (r.clone(), style)))
}

fn diff_row(p: &Pal, row: &Row, line: usize, simplified: bool) -> AnyElement {
    let conv = |el: StyledText| {
        // macOS ships PingFang SC; elsewhere the name misses and the default face is used.
        div()
            .when(simplified, |d| d.font_family("PingFang SC"))
            .child(el)
    };
    let (num, bg, body): (String, u32, AnyElement) = match row {
        Row::Equal(t) => (
            line.to_string(),
            p.surface,
            conv(marked_text(t, &[], 0)).into_any_element(),
        ),
        Row::Replace { orig, conv: c } => (
            line.to_string(),
            p.row_replace,
            div()
                .flex()
                .flex_col()
                .child(div().text_color(rgb(p.faint)).child(marked_text(
                    &orig.text,
                    &orig.changed,
                    p.mark_orig,
                )))
                .child(conv(marked_text(&c.text, &c.changed, p.mark_conv)))
                .into_any_element(),
        ),
        Row::Insert(t) => (
            line.to_string(),
            p.row_insert,
            conv(marked_text(
                t,
                std::slice::from_ref(&(0..t.len())),
                p.mark_conv,
            ))
            .into_any_element(),
        ),
        Row::Delete(t) => (
            "−".into(),
            p.row_delete,
            div()
                .text_color(rgb(p.faint))
                .child(marked_text(
                    t,
                    std::slice::from_ref(&(0..t.len())),
                    p.mark_orig,
                ))
                .into_any_element(),
        ),
    };
    div()
        .flex()
        .gap_3()
        .px_3()
        .py_0p5()
        .bg(rgb(bg))
        .child(
            div()
                .w(px(36.))
                .flex_none()
                .text_right()
                .text_color(rgb(p.faint))
                .child(num),
        )
        .child(div().flex_1().min_w_0().child(body))
        .into_any_element()
}

fn main() {
    let config = dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fanhuaji-gpui");
    let cache = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fanhuaji-gpui");
    // Cleared on every launch: Undo only covers this session, as in the Tauri app.
    let backups = Arc::new(Backups::new(cache.join("backups")).expect("backup folder"));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("HTTP client");

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
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
                window.set_window_title(&t("app.title"));
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
