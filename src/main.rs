mod diff;
mod i18n;
mod queue;
mod settings;

use diff::Row;
use fanhuaji_core::{
    Backups, ConvertFileResult, ConvertParams, EpubProgress, ModuleInfo, PreviewResult,
};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, ExternalPaths, HighlightStyle, PathPromptOptions,
    SharedString, StyledText, Subscription, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size,
};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::switch::Switch;
use gpui_component::{Root, Theme, ThemeMode};
use queue::{FileId, Queue, Status};
use settings::{ModuleChoice, Naming, OutputKey, Settings};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

/// fanhuaji-core is built on reqwest + tokio, while GPUI runs its own executor.
static RT: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().expect("無法建立 tokio runtime"));

const CONVERTERS: &[(&str, &str)] = &[
    ("Traditional", "繁體化"),
    ("Taiwan", "台灣化"),
    ("Hongkong", "香港化"),
    ("Simplified", "簡體化"),
    ("China", "中國化"),
    ("Bopomofo", "注音化"),
    ("Mars", "火星化"),
    ("Pinyin", "拼音化"),
    ("WikiSimplified", "維基簡體"),
    ("WikiTraditional", "維基繁體"),
];

const SUPPORTED: &[&str] = &[
    "txt", "srt", "ass", "ssa", "lrc", "vtt", "sub", "csv", "tsv", "json", "xml", "html", "htm",
    "md", "epub",
];

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

fn converter_label(key: &str) -> &str {
    CONVERTERS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(key, |(_, l)| l)
}

enum PreviewBody {
    Message(SharedString),
    Diff(Vec<Row>),
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

    fn diff(res: &PreviewResult, label: &str) -> Self {
        let rows = diff::diff_lines(&res.original, &res.converted);
        let changed = rows
            .iter()
            .filter(|r| !matches!(r, Row::Equal(_) | Row::Delete(_)))
            .count();
        let chars = res.original.chars().count();
        let mut stats = format!("{label} · {chars} 字 · {changed} 行有變更");
        if res.truncated {
            stats.push_str(" · 僅顯示開頭");
        }
        Self {
            stats: stats.into(),
            body: PreviewBody::Diff(rows),
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

struct Fanhuaji {
    queue: Queue,
    settings: Settings,
    settings_path: PathBuf,
    client: reqwest::Client,
    backups: Arc<Backups>,
    selected: Option<FileId>,
    preview: Option<Preview>,
    /// Bumped per preview request, so a slow response can't replace a newer one.
    preview_gen: u64,
    show_settings: bool,
    tab: Tab,
    modules: Modules,
    module_category: String,
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
        let textarea =
            |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let (value, placeholder) = (value.to_string(), placeholder.to_string());
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .rows(4)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let pre_replace = textarea(&settings.pre_replace, "搜尋=取代（每行一組）", window, cx);
        let post_replace = textarea(&settings.post_replace, "搜尋=取代（每行一組）", window, cx);
        let protect_replace = textarea(&settings.protect_replace, "每行一個詞彙", window, cx);
        let suffix = settings.custom_suffix.clone();
        let custom_suffix = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("converted")
                .default_value(suffix)
        });

        // Saved on every edit, like the Tauri app, so a rule being typed survives quitting.
        let _subscriptions = vec![
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

        Self {
            queue: Queue::default(),
            settings,
            settings_path,
            client,
            backups,
            selected: None,
            preview: None,
            preview_gen: 0,
            show_settings: false,
            tab: Tab::Replace,
            modules: Modules::Loading,
            module_category: String::new(),
            pre_replace,
            post_replace,
            protect_replace,
            custom_suffix,
            _subscriptions,
        }
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            // shortcut: logged only; show it in the UI once there is a notification area.
            eprintln!("無法儲存設定 {}: {e}", self.settings_path.display());
        }
        cx.notify();
    }

    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths.iter().filter(|p| is_supported(p)) {
            self.queue.add(path.clone());
        }
        if self.settings.auto_convert && !self.queue.stopped() {
            self.start(None, cx);
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("選擇要轉換的檔案".into()),
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
            prompt: Some("選擇儲存資料夾".into()),
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

    fn select(&mut self, id: FileId, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.show_settings = false;
        self.refresh_preview(cx);
    }

    /// After files leave the list: drop the selection if its file is gone.
    fn removed(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some_and(|id| self.queue.get(id).is_none()) {
            self.selected = None;
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    /// Put an overwritten file's original back, then run `then` on success.
    fn restore(
        &mut self,
        id: FileId,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let backups = self.backups.clone();
        let job = RT.spawn_blocking(move || backups.restore(&id.to_string()));
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |app, cx| {
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
            (Status::Failed(e), _) => Preview::message(format!("轉換失敗：{e}")),
            _ if is_epub(&file.path) => Preview::message("EPUB 不支援預覽"),
            (_, Some((res, key))) => Preview::diff(
                res,
                &format!("已儲存（{}）", converter_label(&key.converter)),
            ),
            (Status::Converting(_), _) => Preview::message("轉換完成後顯示預覽…"),
            // It converts soon and brings its own preview; don't pay for a second request.
            (Status::Pending, _) if self.queue.converting() => {
                Preview::message("等待轉換，完成後顯示預覽…")
            }
            _ => {
                let generation = self.preview_gen;
                let params = self.settings.params(&file.path, String::new());
                let client = self.client.clone();
                let label = converter_label(&self.settings.converter).to_string();
                let job =
                    RT.spawn(async move { fanhuaji_core::preview_convert(&client, &params).await });
                cx.spawn(async move |this, cx| {
                    let preview = match job.await {
                        Ok(Ok(res)) => Preview::diff(&res, &format!("預覽（{label}）")),
                        Ok(Err(e)) => Preview::message(format!("預覽失敗：{e}")),
                        Err(e) => Preview::message(format!("預覽失敗：{e}")),
                    };
                    let _ = this.update(cx, |app, cx| {
                        if app.preview_gen == generation {
                            app.preview = Some(preview);
                            cx.notify();
                        }
                    });
                })
                .detach();
                Preview::message("產生預覽中…")
            }
        });
    }

    /// Convert pending files one at a time, or only `only` (Reconvert). Stop takes effect
    /// before the next file; files removed meanwhile are skipped.
    fn start(&mut self, only: Option<FileId>, cx: &mut Context<Self>) {
        let Some(ids) = self.queue.begin(only) else {
            return;
        };
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
                    continue;
                };

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EpubProgress>();
                let job = RT.spawn(convert(params, epub, client, backups, tx));
                // The sender lives inside the job, so this ends when the job does.
                while let Some(p) = rx.recv().await {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(f) = app.queue.get_mut(id) {
                            f.status = Status::Converting(Some((p.chapter_index, p.chapter_total)));
                            cx.notify();
                        }
                    });
                }
                let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
                let _ = this.update(cx, |app, cx| {
                    if let Some(f) = app.queue.get_mut(id) {
                        apply_result(f, result, key);
                    }
                    if app.selected == Some(id) {
                        app.refresh_preview(cx);
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |app, cx| {
                app.queue.end(only);
                // Files dropped in mid-batch are still waiting.
                if app.queue.should_resume(app.settings.auto_convert) {
                    app.start(None, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn apply_result(
    f: &mut queue::FileItem,
    result: Result<ConvertFileResult, String>,
    key: OutputKey,
) {
    match result {
        Ok(r) => {
            f.overwritten = r.backup_path.is_some();
            f.saved = r.preview.map(|p| (p, key));
            f.status = match r.warnings {
                Some(w) => Status::Warning(format!("{} ({w})", r.output_name)),
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

fn button(id: impl Into<gpui::ElementId>, label: &str, primary: bool) -> gpui::Stateful<gpui::Div> {
    let (bg, fg, hover) = if primary {
        (rgb(0x2563eb), rgb(0xffffff), rgb(0x1d4ed8))
    } else {
        (rgb(0xffffff), rgb(0x1f2937), rgb(0xf3f4f6))
    };
    div()
        .id(id)
        .px_4()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(bg)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label.to_string())
}

fn chip(id: impl Into<gpui::ElementId>, label: &str, selected: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_full()
        .border_1()
        .border_color(if selected {
            rgb(0x2563eb)
        } else {
            rgb(0xd1d5db)
        })
        .bg(if selected {
            rgb(0xdbeafe)
        } else {
            rgb(0xffffff)
        })
        .cursor_pointer()
        .child(label.to_string())
}

fn section(title: &str) -> gpui::Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title.to_string()),
    )
}

impl Render for Fanhuaji {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let converters = div()
            .flex_1()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(CONVERTERS.iter().map(|&(key, label)| {
                chip(key, label, self.settings.converter == key).on_click(cx.listener(
                    move |app, _, _, cx| {
                        if !app.queue.converting() && app.settings.converter != key {
                            app.settings.converter = key.into();
                            app.settings_changed(cx);
                            app.refresh_preview(cx);
                        }
                    },
                ))
            }));
        let top = div().flex().items_start().gap_3().child(converters).child(
            button(
                "settings",
                if self.show_settings {
                    "關閉設定"
                } else {
                    "設定"
                },
                false,
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.show_settings = !app.show_settings;
                // Settings may have changed what the pending preview should show.
                if !app.show_settings {
                    app.refresh_preview(cx);
                }
                cx.notify();
            })),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xf9fafb))
            .text_color(rgb(0x111827))
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
            .child(self.render_actions(cx))
    }
}

impl Fanhuaji {
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .border_color(rgb(0xd1d5db))
            .drag_over::<ExternalPaths>(|s, _, _, _| {
                s.border_color(rgb(0x2563eb)).bg(rgb(0xeff6ff))
            })
            .on_drop(cx.listener(|app, paths: &ExternalPaths, _, cx| {
                app.add_paths(paths.paths(), cx);
            }))
            .when(self.queue.files.is_empty(), |d| {
                d.items_center()
                    .justify_center()
                    .text_color(rgb(0x6b7280))
                    .child("把檔案拖到這裡，或按「加入檔案」")
            })
            .children(self.queue.files.iter().map(|f| {
                let id = f.id;
                let name = f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (note, color): (SharedString, _) = match &f.status {
                    Status::Pending => ("等待中".into(), rgb(0x6b7280)),
                    Status::Converting(None) => ("轉換中…".into(), rgb(0x2563eb)),
                    Status::Converting(Some((i, n))) => {
                        (format!("轉換中 {i}/{n}").into(), rgb(0x2563eb))
                    }
                    Status::Done(_) if f.overwritten => ("已覆寫原檔".into(), rgb(0x16a34a)),
                    Status::Done(out) => (format!("完成 → {out}").into(), rgb(0x16a34a)),
                    Status::Warning(w) => (format!("完成（有警告）→ {w}").into(), rgb(0xd97706)),
                    // shortcut: raw error codes; map them via the Tauri app's locale JSON when adding i18n.
                    Status::Failed(e) => (format!("失敗：{e}").into(), rgb(0xdc2626)),
                    Status::Restored => ("已還原原檔".into(), rgb(0x6b7280)),
                };
                let removable = !matches!(f.status, Status::Converting(_));
                let small_action = |id: (&'static str, FileId), label: &'static str| {
                    div()
                        .id(id)
                        .px_1()
                        .rounded_sm()
                        .text_sm()
                        .text_color(rgb(0x6b7280))
                        .hover(|s| s.text_color(rgb(0x111827)).bg(rgb(0xe5e7eb)))
                        .child(label)
                };
                div()
                    .id(("file", id))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0xe5e7eb))
                    .cursor_pointer()
                    .when(self.selected == Some(id), |d| d.bg(rgb(0xdbeafe)))
                    .hover(|s| s.bg(rgb(0xf3f4f6)))
                    .on_click(cx.listener(move |app, _, _, cx| app.select(id, cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(name)
                            .child(div().text_sm().text_color(color).child(note)),
                    )
                    .when(f.overwritten && f.is_finished(), |d| {
                        d.child(small_action(("undo", id), "還原").on_click(cx.listener(
                            move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.restore(id, cx, |_, _| {});
                            },
                        )))
                    })
                    .when(removable, |d| {
                        d.child(small_action(("remove", id), "✕").on_click(cx.listener(
                            move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.queue.remove(id);
                                app.removed(cx);
                            },
                        )))
                    })
            }))
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = &self.queue;
        let converting = q.converting();
        let has_pending = q.files.iter().any(|f| f.status == Status::Pending);
        let has_failed = q
            .files
            .iter()
            .any(|f| matches!(f.status, Status::Failed(_)));
        let done = q
            .files
            .iter()
            .filter(|f| f.is_finished() || f.status == Status::Restored)
            .count();
        div()
            .flex()
            .gap_2()
            .justify_end()
            .child(
                button("add", "加入檔案", false)
                    .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx))),
            )
            // Only worth a button when it clears less than "清除列表" would.
            .when(done > 0 && done < q.files.len(), |d| {
                d.child(
                    button("clear-done", "清除已完成", false).on_click(cx.listener(
                        |app, _, _, cx| {
                            app.queue.remove_completed();
                            app.removed(cx);
                        },
                    )),
                )
            })
            // Clearing mid-batch would hide files the running loop is still writing.
            .when(!converting && !q.files.is_empty(), |d| {
                d.child(button("clear", "清除列表", false).on_click(cx.listener(
                    |app, _, _, cx| {
                        app.queue.files.clear();
                        app.removed(cx);
                    },
                )))
            })
            .when(!converting && has_failed, |d| {
                d.child(button("retry", "重試失敗", false).on_click(cx.listener(
                    |app, _, _, cx| {
                        app.queue.reset_errors();
                        app.start(None, cx);
                    },
                )))
            })
            .when(converting, |d| {
                let label = if q.stop_requested() {
                    "停止中…"
                } else {
                    "停止"
                };
                d.child(
                    button("stop", label, false).on_click(cx.listener(|app, _, _, cx| {
                        app.queue.stop();
                        cx.notify();
                    })),
                )
            })
            .when(!converting && has_pending, |d| {
                d.child(
                    button("start", "開始轉換", true)
                        .on_click(cx.listener(|app, _, _, cx| app.start(None, cx))),
                )
            })
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div()
            .id("preview")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0xd1d5db))
            .bg(rgb(0xffffff));
        let Some(preview) = &self.preview else {
            return pane
                .items_center()
                .justify_center()
                .text_color(rgb(0x6b7280))
                .child("點選左側檔案以預覽轉換差異");
        };
        // A finished file converted with other output settings than the current ones.
        let stale = self
            .selected
            .and_then(|id| self.queue.get(id))
            .filter(|f| f.is_finished() && !self.queue.converting())
            .and_then(|f| f.saved.as_ref())
            .is_some_and(|(_, key)| *key != self.settings.output_key());
        let pane =
            pane.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(0x6b7280))
                    .border_b_1()
                    .border_color(rgb(0xe5e7eb))
                    .child(div().flex_1().child(preview.stats.clone()))
                    .when(stale, |d| {
                        d.child(button("reconvert", "以目前設定重新轉換", false).on_click(
                            cx.listener(|app, _, _, cx| {
                                if let Some(id) = app.selected {
                                    app.reconvert(id, cx);
                                }
                            }),
                        ))
                    }),
            );
        match &preview.body {
            PreviewBody::Message(m) => {
                pane.child(div().p_4().text_color(rgb(0x6b7280)).child(m.clone()))
            }
            PreviewBody::Diff(rows) => {
                let mut line = 0;
                pane.children(rows.iter().map(|row| {
                    if !matches!(row, Row::Delete(_)) {
                        line += 1;
                    }
                    diff_row(row, line)
                }))
            }
        }
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = div().flex().gap_2().children(
            [
                (Tab::Replace, "取代"),
                (Tab::Modules, "模組"),
                (Tab::General, "一般"),
            ]
            .map(|(tab, label)| {
                chip(label, label, self.tab == tab).on_click(cx.listener(move |app, _, _, cx| {
                    app.tab = tab;
                    cx.notify();
                }))
            }),
        );
        let body = match self.tab {
            Tab::Replace => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(section("轉換前取代").child(Textarea::new(&self.pre_replace)))
                .child(section("轉換後取代").child(Textarea::new(&self.post_replace)))
                .child(section("保護詞彙（不轉換）").child(Textarea::new(&self.protect_replace)))
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
            .border_color(rgb(0xd1d5db))
            .bg(rgb(0xffffff))
            .child(tabs)
            .child(body)
    }

    fn render_modules(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let list = match &self.modules {
            Modules::Loading => return div().text_color(rgb(0x6b7280)).child("載入模組清單中…"),
            Modules::Failed(e) => {
                return div()
                    .text_color(rgb(0xdc2626))
                    .child(format!("無法載入模組清單：{e}"));
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
                let label = if c.is_empty() { "其他" } else { c };
                let owned = c.to_string();
                chip(("cat", i), label, self.module_category == c).on_click(cx.listener(
                    move |app, _, _, cx| {
                        app.module_category = owned.clone();
                        cx.notify();
                    },
                ))
            }));
        let rows =
            list.iter()
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
                        (ModuleChoice::Auto, "自動", "mod-auto"),
                        (ModuleChoice::Enable, "啟用", "mod-on"),
                        (ModuleChoice::Disable, "停用", "mod-off"),
                    ];
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(0xf3f4f6))
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
                                        .text_color(rgb(0x6b7280))
                                        .child(m.description.clone()),
                                ),
                        )
                        .child(div().flex().gap_1().children(choices.map(
                            |(choice, label, tag)| {
                                let key = m.key.clone();
                                chip((tag, i), label, current == choice).on_click(cx.listener(
                                    move |app, _, _, cx| {
                                        if choice == ModuleChoice::Auto {
                                            app.settings.modules.remove(&key);
                                        } else {
                                            app.settings.modules.insert(key.clone(), choice);
                                        }
                                        app.settings_changed(cx);
                                    },
                                ))
                            },
                        )))
                });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(cats)
            .child(div().flex().flex_col().children(rows))
    }

    fn render_general(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.settings;
        let folder = s
            .save_folder
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                section("轉換").child(
                    Switch::new("auto-convert")
                        .checked(s.auto_convert)
                        .label("加入檔案後自動開始轉換")
                        .on_click(cx.listener(|app, checked: &bool, _, cx| {
                            app.settings.auto_convert = *checked;
                            app.settings_changed(cx);
                        })),
                ),
            )
            .child(
                section("儲存位置")
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                chip("folder-same", "與原檔相同", folder.is_none()).on_click(
                                    cx.listener(|app, _, _, cx| {
                                        app.settings.save_folder = None;
                                        app.settings_changed(cx);
                                    }),
                                ),
                            )
                            .child(
                                chip("folder-custom", "自訂資料夾…", folder.is_some()).on_click(
                                    cx.listener(|app, _, _, cx| app.pick_save_folder(cx)),
                                ),
                            ),
                    )
                    .when_some(folder, |d, f| {
                        d.child(div().text_sm().text_color(rgb(0x6b7280)).child(f))
                    }),
            )
            .child(
                section("輸出檔名")
                    .child(
                        div().flex().gap_2().children(
                            [
                                (Naming::Auto, "加上模式名稱"),
                                (Naming::Suffix, "自訂後綴"),
                                (Naming::Overwrite, "覆寫原檔"),
                            ]
                            .map(|(naming, label)| {
                                chip(label, label, s.naming == naming).on_click(cx.listener(
                                    move |app, _, _, cx| {
                                        app.settings.naming = naming;
                                        app.settings_changed(cx);
                                    },
                                ))
                            }),
                        ),
                    )
                    .when(s.naming == Naming::Suffix, |d| {
                        d.child(div().w(px(240.)).child(Input::new(&self.custom_suffix)))
                    })
                    .when(s.naming == Naming::Overwrite, |d| {
                        d.child(div().text_sm().text_color(rgb(0xd97706)).child(
                            "覆寫前會備份原檔，可在清單中按「還原」。備份在關閉程式後清除。",
                        ))
                    }),
            )
    }
}

const ORIG_MARK: u32 = 0xfecaca;
const CONV_MARK: u32 = 0xbbf7d0;

fn marked_text(text: &str, ranges: &[Range<usize>], mark: u32) -> StyledText {
    let style = HighlightStyle {
        background_color: Some(rgb(mark).into()),
        ..Default::default()
    };
    let text = if text.is_empty() { " " } else { text };
    StyledText::new(text.to_string()).with_highlights(ranges.iter().map(|r| (r.clone(), style)))
}

fn diff_row(row: &Row, line: usize) -> AnyElement {
    let (num, bg, body): (String, u32, AnyElement) = match row {
        Row::Equal(t) => (
            line.to_string(),
            0xffffff,
            marked_text(t, &[], 0).into_any_element(),
        ),
        Row::Replace { orig, conv } => (
            line.to_string(),
            0xfffbeb,
            div()
                .flex()
                .flex_col()
                .child(div().text_color(rgb(0x9ca3af)).child(marked_text(
                    &orig.text,
                    &orig.changed,
                    ORIG_MARK,
                )))
                .child(marked_text(&conv.text, &conv.changed, CONV_MARK))
                .into_any_element(),
        ),
        Row::Insert(t) => (
            line.to_string(),
            0xf0fdf4,
            marked_text(t, std::slice::from_ref(&(0..t.len())), CONV_MARK).into_any_element(),
        ),
        Row::Delete(t) => (
            "−".into(),
            0xfef2f2,
            div()
                .text_color(rgb(0x9ca3af))
                .child(marked_text(
                    t,
                    std::slice::from_ref(&(0..t.len())),
                    ORIG_MARK,
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
                .text_color(rgb(0x9ca3af))
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
    let backups = Arc::new(Backups::new(cache.join("backups")).expect("無法建立備份資料夾"));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("無法建立 HTTP 客戶端");

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        // The app's own colors are light-only for now.
        Theme::change(ThemeMode::Light, None, cx);
        let bounds = Bounds::centered(None, size(px(1040.), px(680.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let app = cx.new(|cx| {
                    Fanhuaji::new(config.join("settings.json"), client, backups, window, cx)
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
}
