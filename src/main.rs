mod diff;

use diff::Row;
use fanhuaji_core::{Backups, ConvertFileResult, ConvertParams, EpubProgress, PreviewResult};
use gpui::{
    AnyElement, App, Application, Bounds, Context, ExternalPaths, HighlightStyle,
    PathPromptOptions, SharedString, StyledText, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
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

enum Status {
    Pending,
    Converting(Option<(usize, usize)>),
    Done(String),
    Warning(String),
    Failed(String),
}

struct FileItem {
    path: PathBuf,
    status: Status,
    /// What the finished conversion wrote, and with which converter.
    saved: Option<(PreviewResult, &'static str)>,
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

fn converter_label(key: &str) -> &'static str {
    CONVERTERS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, l)| l)
}

fn file_params(path: &Path, converter: &str, file_id: String) -> ConvertParams {
    ConvertParams {
        file_id,
        input_path: path.to_string_lossy().into_owned(),
        converter: converter.to_string(),
        save_folder: "same".into(),
        naming: "auto".into(),
        custom_suffix: String::new(),
        pre_replace: String::new(),
        post_replace: String::new(),
        protect_replace: String::new(),
        modules: String::new(),
    }
}

struct Fanhuaji {
    files: Vec<FileItem>,
    converter: &'static str,
    converting: bool,
    client: reqwest::Client,
    backups: Arc<Backups>,
    selected: Option<usize>,
    preview: Option<Preview>,
    /// Bumped per preview request, so a slow response can't replace a newer one.
    preview_gen: u64,
}

impl Fanhuaji {
    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths.iter().filter(|p| is_supported(p)) {
            if !self.files.iter().any(|f| &f.path == path) {
                self.files.push(FileItem {
                    path: path.clone(),
                    status: Status::Pending,
                    saved: None,
                });
            }
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

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        self.selected = Some(i);
        self.refresh_preview(cx);
    }

    /// Show the selected file's diff. A finished file shows what was written; a pending
    /// one asks the API for a preview of its start with the current converter.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_gen += 1;
        cx.notify();
        let Some(file) = self.selected.and_then(|i| self.files.get(i)) else {
            self.preview = None;
            return;
        };
        self.preview = Some(match (&file.status, &file.saved) {
            (Status::Failed(e), _) => Preview::message(format!("轉換失敗：{e}")),
            _ if is_epub(&file.path) => Preview::message("EPUB 不支援預覽"),
            (_, Some((res, conv))) => {
                Preview::diff(res, &format!("已儲存（{}）", converter_label(conv)))
            }
            (Status::Converting(_), _) => Preview::message("轉換完成後顯示預覽…"),
            _ => {
                let generation = self.preview_gen;
                let params = file_params(&file.path, self.converter, String::new());
                let client = self.client.clone();
                let label = converter_label(self.converter);
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

    /// Convert pending files one at a time, like the Tauri app's queue.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.converting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |app, cx| {
                    let i = app
                        .files
                        .iter()
                        .position(|f| matches!(f.status, Status::Pending))?;
                    app.files[i].status = Status::Converting(None);
                    if app.selected == Some(i) {
                        app.refresh_preview(cx);
                    }
                    cx.notify();
                    Some((
                        i,
                        app.files[i].path.clone(),
                        app.converter,
                        app.client.clone(),
                        app.backups.clone(),
                    ))
                });
                let Ok(Some((i, path, converter, client, backups))) = next else {
                    break;
                };

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EpubProgress>();
                let job = RT.spawn(convert(path, converter, i, client, backups, tx));
                // The sender lives inside the job, so this ends when the job does.
                while let Some(p) = rx.recv().await {
                    let _ = this.update(cx, |app, cx| {
                        app.files[i].status =
                            Status::Converting(Some((p.chapter_index, p.chapter_total)));
                        cx.notify();
                    });
                }
                let (status, saved) = match job.await {
                    Ok(Ok(r)) => {
                        let saved = r.preview.map(|p| (p, converter));
                        match r.warnings {
                            Some(w) => (Status::Warning(format!("{} ({w})", r.output_name)), saved),
                            None => (Status::Done(r.output_name), saved),
                        }
                    }
                    Ok(Err(e)) => (Status::Failed(e), None),
                    Err(e) => (Status::Failed(e.to_string()), None),
                };
                let _ = this.update(cx, |app, cx| {
                    app.files[i].status = status;
                    app.files[i].saved = saved;
                    if app.selected == Some(i) {
                        app.refresh_preview(cx);
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |app, cx| {
                app.converting = false;
                cx.notify();
            });
        })
        .detach();
    }
}

async fn convert(
    path: PathBuf,
    converter: &'static str,
    index: usize,
    client: reqwest::Client,
    backups: Arc<Backups>,
    tx: tokio::sync::mpsc::UnboundedSender<EpubProgress>,
) -> Result<ConvertFileResult, String> {
    let params = file_params(&path, converter, index.to_string());
    let on_progress = move |p| {
        let _ = tx.send(p);
    };
    if is_epub(&path) {
        fanhuaji_core::convert_epub(&client, &backups, &params, on_progress).await
    } else {
        fanhuaji_core::convert_file(&client, &backups, &params, on_progress).await
    }
}

fn button(id: &'static str, label: &str, primary: bool) -> gpui::Stateful<gpui::Div> {
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

impl Render for Fanhuaji {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let converters = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(CONVERTERS.iter().map(|&(key, label)| {
                let selected = self.converter == key;
                div()
                    .id(key)
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
                    .child(label)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if !app.converting && app.converter != key {
                            app.converter = key;
                            app.refresh_preview(cx);
                        }
                    }))
            }));

        let list = div()
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
            .when(self.files.is_empty(), |d| {
                d.items_center()
                    .justify_center()
                    .text_color(rgb(0x6b7280))
                    .child("把檔案拖到這裡，或按「加入檔案」")
            })
            .children(self.files.iter().enumerate().map(|(i, f)| {
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
                    Status::Done(out) => (format!("完成 → {out}").into(), rgb(0x16a34a)),
                    Status::Warning(w) => (format!("完成（有警告）→ {w}").into(), rgb(0xd97706)),
                    // shortcut: raw error codes; map them via the Tauri app's locale JSON when adding i18n.
                    Status::Failed(e) => (format!("失敗：{e}").into(), rgb(0xdc2626)),
                };
                div()
                    .id(i)
                    .flex()
                    .flex_col()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0xe5e7eb))
                    .cursor_pointer()
                    .when(self.selected == Some(i), |d| d.bg(rgb(0xdbeafe)))
                    .hover(|s| s.bg(rgb(0xf3f4f6)))
                    .on_click(cx.listener(move |app, _, _, cx| app.select(i, cx)))
                    .child(name)
                    .child(div().text_sm().text_color(color).child(note))
            }));

        let has_pending = self
            .files
            .iter()
            .any(|f| matches!(f.status, Status::Pending));
        let actions = div()
            .flex()
            .gap_2()
            .justify_end()
            .child(
                button("add", "加入檔案", false)
                    .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx))),
            )
            .child(
                button("clear", "清除", false).on_click(cx.listener(|app, _, _, cx| {
                    if !app.converting {
                        app.files.clear();
                        app.selected = None;
                        app.refresh_preview(cx);
                    }
                })),
            )
            .child(
                button(
                    "start",
                    if self.converting {
                        "轉換中…"
                    } else {
                        "開始轉換"
                    },
                    true,
                )
                .when(!has_pending || self.converting, |b| b.opacity(0.5))
                .on_click(cx.listener(|app, _, _, cx| app.start(cx))),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xf9fafb))
            .text_color(rgb(0x111827))
            .child(converters)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_3()
                    .child(list)
                    .child(self.render_preview()),
            )
            .child(actions)
    }
}

impl Fanhuaji {
    fn render_preview(&self) -> impl IntoElement {
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
        let pane = pane.child(
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(rgb(0x6b7280))
                .border_b_1()
                .border_color(rgb(0xe5e7eb))
                .child(preview.stats.clone()),
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
    // shortcut: temp dir instead of the platform cache dir; overwrite mode isn't exposed yet.
    let backups = Backups::new(std::env::temp_dir().join("fanhuaji-gpui-backups"))
        .expect("無法建立備份資料夾");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("無法建立 HTTP 客戶端");

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Fanhuaji {
                    files: Vec::new(),
                    converter: "Taiwan",
                    converting: false,
                    client,
                    backups: Arc::new(backups),
                    selected: None,
                    preview: None,
                    preview_gen: 0,
                })
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
