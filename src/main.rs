use fanhuaji_core::{Backups, ConvertFileResult, ConvertParams, EpubProgress};
use gpui::{
    App, Application, Bounds, Context, ExternalPaths, PathPromptOptions, SharedString, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
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
}

struct Fanhuaji {
    files: Vec<FileItem>,
    converter: &'static str,
    converting: bool,
    client: reqwest::Client,
    backups: Arc<Backups>,
}

impl Fanhuaji {
    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths.iter().filter(|p| is_supported(p)) {
            if !self.files.iter().any(|f| &f.path == path) {
                self.files.push(FileItem {
                    path: path.clone(),
                    status: Status::Pending,
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
                let status = match job.await {
                    Ok(Ok(r)) => match r.warnings {
                        Some(w) => Status::Warning(format!("{} ({w})", r.output_name)),
                        None => Status::Done(r.output_name),
                    },
                    Ok(Err(e)) => Status::Failed(e),
                    Err(e) => Status::Failed(e.to_string()),
                };
                let _ = this.update(cx, |app, cx| {
                    app.files[i].status = status;
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
    let params = ConvertParams {
        file_id: index.to_string(),
        input_path: path.to_string_lossy().into_owned(),
        converter: converter.to_string(),
        save_folder: "same".into(),
        naming: "auto".into(),
        custom_suffix: String::new(),
        pre_replace: String::new(),
        post_replace: String::new(),
        protect_replace: String::new(),
        modules: String::new(),
    };
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
                        if !app.converting {
                            app.converter = key;
                            cx.notify();
                        }
                    }))
            }));

        let list = div()
            .id("files")
            .flex_1()
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
            .children(self.files.iter().map(|f| {
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
                    .flex()
                    .flex_col()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0xe5e7eb))
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
                        cx.notify();
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
            .child(list)
            .child(actions)
    }
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
        let bounds = Bounds::centered(None, size(px(640.), px(520.)), cx);
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
