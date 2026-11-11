//! The window, laid out after the Tauri app's `index.html` + `styles.css`: header with the
//! seal, mode segment and output pill; file queue and live diff side by side; status bar;
//! settings drawer from the right.

use crate::diff::Row;
use crate::i18n::{self, t, tp, translate_error};
use crate::queue::{FileItem, Queue, Status};
use crate::settings::{ModuleChoice, Naming, ThemePref};
use crate::theme::Pal;
use crate::*;
use gpui::{
    AnyElement, ClickEvent, Div, ElementId, FontWeight, HighlightStyle, MouseButton, Rgba,
    SharedString, Stateful, StyledText, deferred, div, prelude::*, px, rgba,
};
use gpui_component::Icon;
use gpui_component::input::{Input, Textarea};
use gpui_component::scroll::ScrollableElement;
use std::ops::Range;

/// The toolbar's segment holds these; the rest live under "More", grouped as in the Tauri app.
const MAIN_CONVERTERS: [&str; 4] = ["Traditional", "Taiwan", "Hongkong", "Simplified"];
const MORE_CONVERTERS: [(&str, &[&str]); 3] = [
    ("converter.group.localize", &["China"]),
    ("converter.group.other", &["Bopomofo", "Mars", "Pinyin"]),
    (
        "converter.group.thirdParty",
        &["WikiSimplified", "WikiTraditional"],
    ),
];

/// Serif face of the seal, wordmark and titles (the Tauri app bundles Noto Serif TC).
const SERIF: &str = "Songti TC";
const MONO: &str = "Menlo";

fn c(color: u32) -> Rgba {
    rgba(color)
}

fn icon(name: &str) -> Icon {
    Icon::default().path(SharedString::from(format!("icons/{name}.svg")))
}

type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

impl Render for Fanhuaji {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let has_files = !self.queue.files.is_empty();
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
            .on_action(cx.listener(|app, _: &CloseSettings, window, cx| {
                if app.popover.take().is_some() {
                    cx.notify();
                } else {
                    app.close_settings(window, cx)
                }
            }))
            // Files can be dropped anywhere in the window, as in the Tauri app.
            .group("root")
            .on_drop(cx.listener(|app, paths: &ExternalPaths, _, cx| {
                app.add_paths(paths.paths(), cx);
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .text_size(px(13.))
            .child(self.render_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_queue(cx))
                    .child(self.render_preview(cx)),
            )
            .when_some(self.batch, |d, (done, total)| {
                d.child(
                    div().h(px(2.)).w_full().bg(c(p.border_light)).child(
                        div()
                            .h_full()
                            .bg(c(p.accent))
                            .w(gpui::relative(done as f32 / total.max(1) as f32)),
                    ),
                )
            })
            .when(has_files, |d| d.child(self.render_status(cx)))
            .child(self.render_drop_overlay())
            .when(self.popover.is_some(), |d| {
                // Clicking anywhere outside an open popover closes it.
                d.child(
                    deferred(
                        div()
                            .id("popover-catcher")
                            .absolute()
                            .inset_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|app, _, _, cx| {
                                    app.popover = None;
                                    cx.notify();
                                }),
                            ),
                    )
                    .with_priority(1),
                )
            })
            .when(self.show_settings, |d| {
                d.child(deferred(self.render_drawer(cx)).with_priority(3))
            })
    }
}

impl Fanhuaji {
    // --- Header ---

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let brand = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .flex_none()
            .child(
                div()
                    .size(px(30.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(c(p.accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(SERIF)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(18.))
                    .text_color(c(0xffffffff))
                    .child("繁"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(SERIF)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(17.))
                            .line_height(px(20.))
                            .child(t("app.title")),
                    )
                    // In English the name already reads "Fanhuaji".
                    .when(i18n::locale() != i18n::Locale::En, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(c(p.faint))
                                .child("F A N H U A J I"),
                        )
                    }),
            );

        let mut controls = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(self.render_mode_segment(cx))
            .child(self.render_output_pill(cx));
        if let Some(r) = &self.update {
            controls = controls.child(
                div()
                    .id("update-chip")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .bg(c(p.accent_weak))
                    .text_color(c(p.accent))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(icon("download").size(px(14.)))
                    .child(tp("update.chip", &[("version", &r.version)]))
                    .on_click(cx.listener(|app, _, window, cx| app.prompt_install(window, cx))),
            );
        }
        controls = controls.child(icon_button(p, "settings-btn", "settings", 18.).on_click(
            cx.listener(|app, _, _, cx| {
                if app.show_settings {
                    app.show_settings = false;
                    cx.notify();
                } else {
                    app.open_settings(app.tab, cx);
                }
            }),
        ));

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(16.))
            .py(px(9.))
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .child(brand)
            .child(controls)
    }

    fn render_mode_segment(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let current = self.settings.converter.as_str();
        let more_active = !MAIN_CONVERTERS.contains(&current);
        let seg_button = |id: ElementId, active: bool| {
            let hover = p.text;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(13.))
                .py(px(5.))
                .rounded(px(5.))
                .cursor_pointer()
                .whitespace_nowrap()
                .when(active, |d| {
                    d.bg(c(p.surface))
                        .text_color(c(p.accent))
                        .font_weight(FontWeight::SEMIBOLD)
                        .shadow_sm()
                })
                .when(!active, |d| {
                    d.text_color(c(p.muted))
                        .font_weight(FontWeight::MEDIUM)
                        .hover(move |s| s.text_color(c(hover)))
                })
        };
        let more_label = if more_active {
            converter_label(current)
        } else {
            t("converter.more")
        };
        div()
            .relative()
            .flex()
            .bg(c(p.sunken))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(8.))
            .p(px(3.))
            .gap(px(2.))
            .children(MAIN_CONVERTERS.map(|key| {
                seg_button(key.into(), current == key)
                    .child(converter_label(key))
                    .on_click(cx.listener(move |app, _, _, cx| app.set_converter(key, cx)))
            }))
            .child(
                seg_button("mode-more".into(), more_active)
                    .child(more_label)
                    .child(icon("chevron-down").size(px(13.)).text_color(c(p.faint)))
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_popover(Popover::Mode, cx))),
            )
            .when(self.popover == Some(Popover::Mode), |d| {
                let mut pop = popover_panel(p, "mode-pop").min_w(px(180.));
                for (group, keys) in MORE_CONVERTERS {
                    pop = pop.child(pop_group(p, t(group)));
                    for &key in keys {
                        pop = pop.child(
                            pop_option(
                                p,
                                key,
                                t(&format!("converter.{key}")),
                                None,
                                current == key,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    app.popover = None;
                                    app.set_converter(key, cx);
                                },
                            )),
                        );
                    }
                }
                d.child(deferred(pop).with_priority(2))
            })
    }

    fn render_output_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let s = &self.settings;
        let overwrite = s.naming == Naming::Overwrite;
        let label = t(match s.naming {
            Naming::Auto => "settings.save.naming.auto",
            Naming::Suffix => "settings.save.naming.suffix",
            Naming::Overwrite => "settings.save.naming.overwrite",
        });
        let (border_strong, hover) = (p.border_strong, p.hover);
        let pill = div()
            .id("out-pill")
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(11.))
            .py(px(7.))
            .rounded(px(7.))
            .border_1()
            .border_color(c(if overwrite { p.accent } else { p.border }))
            .bg(c(p.surface))
            .text_color(c(if overwrite { p.accent } else { p.text }))
            .cursor_pointer()
            .whitespace_nowrap()
            .when(!overwrite, |d| {
                d.hover(move |s| s.border_color(c(border_strong)).bg(c(hover)))
            })
            .child(icon("folder").size(px(15.)).text_color(c(if overwrite {
                p.accent
            } else {
                p.muted
            })))
            .child(label)
            .when(s.auto_convert, |d| {
                d.child(icon("zap").size(px(14.)).text_color(c(p.accent)))
            })
            .child(icon("chevron-down").size(px(13.)).text_color(c(p.faint)))
            .on_click(cx.listener(|app, _, _, cx| app.toggle_popover(Popover::Output, cx)));

        let naming = [
            (
                Naming::Auto,
                "settings.save.naming.auto",
                "settings.save.naming.autoDesc",
            ),
            (
                Naming::Suffix,
                "settings.save.naming.suffix",
                "settings.save.naming.suffixDesc",
            ),
            (
                Naming::Overwrite,
                "settings.save.naming.overwrite",
                "settings.save.naming.overwriteDesc",
            ),
        ];
        let folder = s
            .save_folder
            .as_ref()
            .map(|f| f.to_string_lossy().into_owned());

        div()
            .relative()
            .child(pill)
            .when(self.popover == Some(Popover::Output), |d| {
                let mut pop = popover_panel(p, "out-pop")
                    .w(px(300.))
                    .child(pop_group(p, t("naming.title")));
                for (value, title, desc) in naming {
                    pop = pop.child(
                        pop_option(p, title, t(title), Some(t(desc)), s.naming == value).on_click(
                            cx.listener(move |app, _, _, cx| {
                                app.settings.naming = value;
                                app.settings_changed(cx);
                            }),
                        ),
                    );
                    if value == Naming::Suffix && s.naming == Naming::Suffix {
                        pop = pop.child(
                            div()
                                .px(px(9.))
                                .pt(px(2.))
                                .pb(px(8.))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(c(p.muted))
                                        .mb(px(4.))
                                        .child(t("settings.save.suffixLabel")),
                                )
                                .child(Input::new(&self.custom_suffix)),
                        );
                    }
                }
                pop = pop
                    .child(pop_group(p, t("settings.save.folderLabel")))
                    .child(
                        pop_option(
                            p,
                            "folder-same",
                            t("settings.save.same"),
                            None,
                            folder.is_none(),
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.settings.save_folder = None;
                            app.settings_changed(cx);
                        })),
                    )
                    .child(
                        pop_option(
                            p,
                            "folder-custom",
                            t("settings.save.customTitle"),
                            folder.clone(),
                            folder.is_some(),
                        )
                        .on_click(cx.listener(|app, _, _, cx| app.pick_save_folder(cx))),
                    )
                    .child(pop_group(p, t("naming.when")))
                    .child(
                        checkbox(
                            p,
                            "auto-convert",
                            s.auto_convert,
                            t("settings.behavior.autoConvert"),
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.settings.auto_convert = !app.settings.auto_convert;
                            app.settings_changed(cx);
                        })),
                    );
                d.child(deferred(pop).with_priority(2))
            })
    }

    // --- File queue ---

    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let count = self.queue.files.len();
        let (accent, accent_weak) = (p.accent, p.accent_weak);
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(c(p.border))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .child(t("queue.title")),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(c(p.muted))
                        .child(tp("queue.count", &[("count", &count.to_string())])),
                )
            })
            .child(div().flex_1())
            .child(
                pane_action(p, "add-more", Some("plus"), t("files.addMore"))
                    .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx))),
            );

        let drop_zone = div()
            .id("drop-zone")
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .mb(px(8.))
            .p(px(18.))
            .border_2()
            .border_dashed()
            .border_color(c(p.border))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.border_color(c(accent)).bg(c(accent_weak)))
            .group_drag_over::<ExternalPaths>("root", move |s| {
                s.border_color(c(accent)).bg(c(accent_weak))
            })
            .on_click(cx.listener(|app, _, _, cx| app.pick_files(cx)))
            .child(icon("cloud-upload").size(px(26.)).text_color(c(p.muted)))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t("drop.title")),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c(p.muted))
                    .child(tp("drop.shortcut", &[("key", &shortcut("o"))])),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c(p.muted))
                    .text_center()
                    .child(t("drop.formats")),
            );

        let mut cards = Vec::with_capacity(count);
        for f in &self.queue.files {
            cards.push(self.render_file(f, cx));
        }
        div()
            .w(px(360.))
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(c(p.surface))
            .border_r_1()
            .border_color(c(p.border))
            .child(head)
            .child(
                div()
                    .id("queue-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(8.))
                    .child(drop_zone)
                    .children(cards),
            )
    }

    fn file_note(&self, f: &FileItem) -> (String, u32) {
        let p = self.pal();
        if self.armed == Some(Armed::Remove(f.id)) {
            return (t("file.removeConfirm"), p.accent);
        }
        let muted = p.muted;
        match &f.status {
            Status::Pending => (ext_label(&f.path), muted),
            Status::Converting(None) => (t("file.converting"), muted),
            Status::Converting(Some((i, n))) => {
                let name = if f.chapter.is_empty() {
                    String::new()
                } else {
                    format!(" {}", f.chapter)
                };
                let (i, n) = (i.to_string(), n.to_string());
                (
                    tp(
                        "file.convertingChapter",
                        &[("current", &i), ("total", &n), ("name", &name)],
                    ),
                    muted,
                )
            }
            Status::Done(_) if f.overwritten => (t("file.overwritten"), muted),
            Status::Done(out) => (tp("file.savedAs", &[("name", out)]), muted),
            Status::Warning(w) => (
                tp(
                    "file.convertDoneWithWarnings",
                    &[("warnings", &translate_error(w))],
                ),
                p.yellow,
            ),
            Status::Failed(e) => (translate_error(e), p.accent),
            Status::Restored => (t("file.restored"), muted),
        }
    }

    fn render_file(&self, f: &FileItem, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal();
        let id = f.id;
        let group: SharedString = format!("card-{id}").into();
        let name = f
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (note, note_color) = self.file_note(f);
        let selected = self.selected == Some(id);
        let removable = !matches!(f.status, Status::Converting(_));
        let armed = self.armed == Some(Armed::Remove(id));
        let (status_icon, status_color) = match &f.status {
            Status::Pending => ("clock", p.muted),
            Status::Converting(_) => ("loader-circle", p.accent),
            Status::Done(_) => ("check", p.green),
            Status::Warning(_) => ("triangle-alert", p.yellow),
            Status::Failed(_) => ("x", p.accent),
            Status::Restored => ("undo-2", p.muted),
        };
        let hover = p.hover;

        // Remove takes the status icon's slot while the card is hovered, as in the Tauri app.
        let slot = div()
            .relative()
            .size(px(24.))
            .flex_none()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(removable || armed, |d| {
                        d.group_hover(group.clone(), |s| s.opacity(0.))
                    })
                    .when(armed, |d| d.opacity(0.))
                    .child(icon(status_icon).size(px(14.)).text_color(c(status_color))),
            )
            .when(removable, |d| {
                d.child(
                    small_icon_button(
                        p,
                        ("remove", id),
                        if armed { "triangle-alert" } else { "x" },
                        armed,
                    )
                    .absolute()
                    .inset_0()
                    .when(!armed, |b| {
                        b.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        let undoable = Queue::undoable(app.queue.get(id));
                        app.confirm_undo_loss(Armed::Remove(id), undoable, cx, |app, _| {
                            app.queue.remove(id)
                        });
                    })),
                )
            });

        div()
            .id(("file", id))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(11.))
            .px(px(10.))
            .py(px(9.))
            .mb(px(3.))
            .rounded(px(8.))
            .border_1()
            .border_color(c(if selected { p.accent_weak } else { 0 }))
            .cursor_pointer()
            .when(selected, |d| d.bg(c(p.accent_weak)))
            .when(!selected, |d| d.hover(move |s| s.bg(c(hover))))
            .on_click(cx.listener(move |app, _, window, cx| {
                window.focus(&app.focus, cx);
                app.select(id, cx);
            }))
            .child(format_badge(p, &f.path))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c(note_color))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(note),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .when(f.is_finished() && f.output.is_some(), |d| {
                        d.child(
                            small_icon_button(p, ("reveal", id), "folder-search", false).on_click(
                                cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.reveal(id, cx);
                                }),
                            ),
                        )
                    })
                    .when(
                        f.overwritten && f.is_finished() && !self.restoring.contains(&id),
                        |d| {
                            d.child(small_icon_button(p, ("undo", id), "undo-2", true).on_click(
                                cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    app.restore(id, cx, |_, _| {});
                                }),
                            ))
                        },
                    )
                    .child(slot),
            )
            .into_any_element()
    }

    // --- Preview ---

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let col = div()
            .id("preview-col")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c(p.surface2));
        let Some(preview) = &self.preview else {
            return col
                .items_center()
                .justify_center()
                .gap(px(10.))
                .p(px(24.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .size(px(56.))
                                .rounded(px(14.))
                                .bg(c(p.sunken))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("file-text").size(px(26.)).text_color(c(p.muted))),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t("preview.emptyTitle")),
                        )
                        .child(
                            div()
                                .max_w(px(280.))
                                .text_center()
                                .line_height(px(21.))
                                .text_color(c(p.faint))
                                .child(t("preview.emptyHint")),
                        ),
                );
        };
        let file = self.selected.and_then(|id| self.queue.get(id));
        // A finished file converted with other output settings than the current ones.
        let stale = file
            .filter(|f| f.is_finished() && !self.queue.converting())
            .and_then(|f| f.saved.as_ref())
            .is_some_and(|(_, key)| *key != self.settings.output_key());
        let name = file
            .and_then(|f| f.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let head = div()
            .flex()
            .items_center()
            .gap(px(11.))
            .px(px(16.))
            .py(px(13.))
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .when_some(file, |d, f| d.child(format_badge(p, &f.path)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_family(SERIF)
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(c(p.muted))
                            .child(preview.stats.clone()),
                    ),
            );

        let body = div()
            .id("preview-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .pt(px(6.))
            .pb(px(24.));
        let body = match &preview.body {
            PreviewBody::Message(m) => body.child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(40.))
                    .text_color(c(p.muted))
                    .child(m.clone()),
            ),
            PreviewBody::Error(m) => body.child(
                div()
                    .flex()
                    .justify_center()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(40.))
                    .text_color(c(p.accent))
                    .child(icon("triangle-alert").size(px(16.)))
                    .child(m.clone()),
            ),
            PreviewBody::Diff { rows, simplified } => {
                let mut line = 0;
                body.children(rows.iter().map(|row| {
                    if !matches!(row, Row::Delete(_)) {
                        line += 1;
                    }
                    diff_row(p, row, line, *simplified)
                }))
            }
        };

        col.child(head)
            .when(stale, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(16.))
                        .py(px(8.))
                        .bg(c(p.accent_weak))
                        .text_color(c(p.accent))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().flex_1().child(t("preview.stale")))
                        .child(
                            pane_action(p, "reconvert", None, t("preview.reconvert")).on_click(
                                cx.listener(|app, _, _, cx| {
                                    if let Some(id) = app.selected {
                                        app.reconvert(id, cx);
                                    }
                                }),
                            ),
                        ),
                )
            })
            .child(body)
    }

    // --- Status bar ---

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let q = &self.queue;
        let converting = q.converting();
        let counts = q.counts();
        let done = q
            .files
            .iter()
            .filter(|f| f.is_finished() || f.status == Status::Restored)
            .count();
        let stat = |label: String, n: usize, color: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_color(c(p.muted))
                .child(label)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(color))
                        .child(n.to_string()),
                )
        };
        let armed_label = |armed: Armed, key: &str, leaving: usize| {
            if self.armed == Some(armed) {
                tp("status.confirmClear", &[("count", &leaving.to_string())])
            } else {
                t(key)
            }
        };
        let show_convert =
            !(self.settings.auto_convert && !q.stopped()) && counts.pending > 0 && !converting;

        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(6.))
            .border_t_1()
            .border_color(c(p.border))
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_color(c(p.muted))
                    .child(icon("arrow-right-left").size(px(14.)))
                    .child(converter_label(&self.settings.converter)),
            )
            .child(div().w(px(1.)).h(px(13.)).bg(c(p.border)))
            .child(
                div()
                    .flex()
                    .gap(px(5.))
                    .text_color(c(p.muted))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(p.text))
                            .child(counts.total.to_string()),
                    )
                    .child(t("status.filesUnit")),
            )
            .child(stat(t("status.success"), counts.success, p.green))
            .when(counts.warning > 0, |d| {
                d.child(stat(t("status.warning"), counts.warning, p.yellow))
            })
            .child(stat(t("status.error"), counts.error, p.accent))
            .when_some(self.notice.clone(), |d, notice| {
                d.child(div().text_color(c(p.yellow)).child(notice))
            })
            .child(div().flex_1())
            // Only worth a button when it clears less than "clear list" would.
            .when(done > 0 && done < q.files.len(), |d| {
                let leaving = Queue::undoable(q.files.iter().filter(|f| f.is_finished()));
                let armed = self.armed == Some(Armed::ClearDone);
                d.child(
                    status_action(
                        p,
                        "clear-done",
                        None,
                        armed_label(Armed::ClearDone, "status.clearDone", leaving),
                        armed,
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.confirm_undo_loss(Armed::ClearDone, leaving, cx, |app, _| {
                            app.queue.remove_completed()
                        });
                    })),
                )
            })
            .child({
                let leaving = Queue::undoable(&q.files);
                let armed = self.armed == Some(Armed::ClearAll);
                // Disabled mid-batch: clearing would hide files the loop is still writing.
                status_action(
                    p,
                    "clear",
                    None,
                    armed_label(Armed::ClearAll, "status.clear", leaving),
                    armed,
                )
                .when(converting, |b| b.opacity(0.45))
                .when(!converting, |b| {
                    b.on_click(cx.listener(move |app, _, _, cx| {
                        app.confirm_undo_loss(Armed::ClearAll, leaving, cx, |app, _| {
                            app.queue.files.clear()
                        });
                    }))
                })
            })
            .when(counts.error > 0 && !converting, |d| {
                d.child(
                    status_action(p, "retry", None, t("status.retry"), false).on_click(
                        cx.listener(|app, _, _, cx| {
                            app.queue.reset_errors();
                            app.start(None, cx);
                        }),
                    ),
                )
            })
            .when(converting, |d| {
                let stopping = q.stop_requested();
                d.child(
                    status_action(p, "stop", Some("square"), t("status.stop"), false)
                        .when(stopping, |b| b.opacity(0.45))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.queue.stop();
                            cx.notify();
                        })),
                )
            })
            .when(show_convert, |d| {
                let accent_deep = p.accent_deep;
                d.child(
                    div()
                        .id("convert")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(14.))
                        .py(px(5.))
                        .rounded(px(4.))
                        .bg(c(p.accent))
                        .text_color(c(p.surface))
                        .font_weight(FontWeight::SEMIBOLD)
                        .shadow_sm()
                        .cursor_pointer()
                        .hover(move |s| s.bg(c(accent_deep)))
                        .child(icon("play").size(px(13.)))
                        .child(format!("{} ({})", t("status.convert"), counts.pending))
                        .on_click(cx.listener(|app, _, _, cx| app.start(None, cx))),
                )
            })
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
            .p(px(14.))
            .bg(c(p.veil))
            .opacity(0.)
            .group_drag_over::<ExternalPaths>("root", |s| s.opacity(1.))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .border_2()
                    .border_dashed()
                    .border_color(c(p.accent))
                    .rounded(px(14.))
                    .bg(c(p.accent_weak))
                    .text_color(c(p.accent))
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon("cloud-upload").size(px(40.)))
                    .child(text),
            )
    }

    // --- Settings drawer ---

    fn render_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let tabs = div()
            .flex()
            .border_b_1()
            .border_color(c(p.border))
            .children(
                [
                    (Tab::Replace, "settings.tab.replace"),
                    (Tab::Modules, "settings.tab.modules"),
                    (Tab::General, "settings.tab.general"),
                ]
                .map(|(tab, key)| {
                    let active = self.tab == tab;
                    let text = p.text;
                    div()
                        .id(key)
                        .flex_1()
                        .flex()
                        .justify_center()
                        .py(px(8.))
                        .border_b_2()
                        .border_color(c(if active { p.accent } else { 0 }))
                        .text_color(c(if active { p.accent } else { p.muted }))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .when(!active, |d| d.hover(move |s| s.text_color(c(text))))
                        .child(t(key))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.tab = tab;
                            cx.notify();
                        }))
                }),
            );
        let body = match self.tab {
            Tab::Replace => self.render_replace().into_any_element(),
            Tab::Modules => self.render_modules(cx).into_any_element(),
            Tab::General => self.render_general(cx).into_any_element(),
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_end()
            .child(
                div()
                    .id("drawer-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(c(p.backdrop))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, window, cx| app.close_settings(window, cx)),
                    ),
            )
            .child(
                div()
                    .id("drawer")
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, _, cx| {
                            if app.popover == Some(Popover::Language) {
                                app.popover = None;
                                cx.notify();
                            }
                        }),
                    )
                    .w(px(360.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(c(p.surface))
                    .border_l_1()
                    .border_color(c(p.border))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .p(px(16.))
                            .border_b_1()
                            .border_color(c(p.border))
                            .child(
                                div()
                                    .font_family(SERIF)
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t("settings.title")),
                            )
                            .child(icon_button(p, "close-settings", "x", 18.).on_click(
                                cx.listener(|app, _, window, cx| app.close_settings(window, cx)),
                            )),
                    )
                    .child(tabs)
                    .child(
                        div()
                            .id("drawer-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .child(body),
                    ),
            )
    }

    fn render_replace(&self) -> impl IntoElement {
        let p = self.pal();
        let label = |key: &str| {
            div()
                .text_size(px(12.))
                .text_color(c(p.muted))
                .mt(px(8.))
                .mb(px(4.))
                .child(t(key))
        };
        setting_section(p, "replace", t("settings.replace.title"))
            .child(label("settings.replace.preLabel"))
            .child(Textarea::new(&self.pre_replace))
            .child(label("settings.replace.postLabel"))
            .child(Textarea::new(&self.post_replace))
            .child(label("settings.replace.protectLabel"))
            .child(Textarea::new(&self.protect_replace))
    }

    fn render_modules(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let section = setting_section(p, "puzzle", t("settings.modules.title"));
        let list = match &self.modules {
            Modules::Loading => {
                return section.child(div().text_color(c(p.muted)).child(t("preview.loading")));
            }
            Modules::Failed(e) => {
                return section.child(
                    div()
                        .text_color(c(p.accent))
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
        let cats = div().flex().flex_wrap().gap(px(6.)).mb(px(8.)).children(
            categories.iter().enumerate().map(|(i, &cat)| {
                let active = self.module_category == cat;
                let label = if cat.is_empty() {
                    t("module.categoryUnknown")
                } else {
                    cat.to_string()
                };
                let owned = cat.to_string();
                div()
                    .id(("cat", i))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(c(if active { p.accent } else { p.border }))
                    .bg(c(if active { p.accent } else { 0 }))
                    .text_color(c(if active { p.on_accent } else { p.muted }))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.module_category = owned.clone();
                        cx.notify();
                    }))
            }),
        );
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
                    .gap(px(8.))
                    .py(px(4.))
                    .child(segmented(
                        p,
                        choices.map(|(choice, key, tag)| {
                            let module = m.key.clone();
                            (
                                ElementId::from((tag, i)),
                                None,
                                t(key),
                                current == choice,
                                Box::new(cx.listener(move |app, _, _, cx| {
                                    if choice == ModuleChoice::Auto {
                                        app.settings.modules.remove(&module);
                                    } else {
                                        app.settings.modules.insert(module.clone(), choice);
                                    }
                                    app.settings_changed(cx);
                                })) as Click,
                            )
                        }),
                        true,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(m.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c(p.muted))
                                    .child(m.description.clone()),
                            ),
                    )
            });
        section
            .child(cats)
            .child(div().flex().flex_col().children(rows))
    }

    /// A full-width dropdown, as the Tauri app's language `<select>`.
    fn render_language_select(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.pal();
        let current = i18n::LOCALES
            .iter()
            .find(|(l, _)| *l == i18n::locale())
            .map_or("", |(_, name)| name);
        let border_strong = p.border_strong;
        div()
            .relative()
            .child(
                div()
                    .id("locale-select")
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(c(if self.popover == Some(Popover::Language) {
                        p.accent
                    } else {
                        p.border
                    }))
                    .bg(c(p.bg))
                    .cursor_pointer()
                    .hover(move |s| s.border_color(c(border_strong)))
                    .child(current)
                    .child(icon("chevron-down").size(px(14.)).text_color(c(p.muted)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(|app, _, _, cx| app.toggle_popover(Popover::Language, cx)),
                    ),
            )
            .when(self.popover == Some(Popover::Language), |d| {
                let mut list = popover_panel(p, "locale-list").top(px(36.)).left_0();
                for (locale, name) in i18n::LOCALES {
                    list = list.child(
                        pop_option(p, name, name.to_string(), None, i18n::locale() == locale)
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.popover = None;
                                i18n::set_locale(locale);
                                app.settings.locale = Some(locale);
                                app.settings_changed(cx);
                                app.apply_locale(window, cx);
                            })),
                    );
                }
                // Above the drawer, which is itself deferred.
                d.child(deferred(list).with_priority(4))
            })
            .into_any_element()
    }

    fn render_general(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.pal();
        let s = &self.settings;
        let languages = self.render_language_select(cx);
        let themes = segmented(
            p,
            [
                (ThemePref::System, "theme.system", "monitor"),
                (ThemePref::Light, "theme.light", "sun"),
                (ThemePref::Dark, "theme.dark", "moon"),
            ]
            .map(|(pref, key, glyph)| {
                (
                    ElementId::from(key),
                    Some(glyph),
                    t(key),
                    s.theme == pref,
                    Box::new(cx.listener(move |app, _, window, cx| {
                        app.settings.theme = pref;
                        app.settings_changed(cx);
                        app.apply_theme(window, cx);
                    })) as Click,
                )
            }),
            false,
        );
        let hover = p.hover;
        let link = |id: &'static str, label: String, url: &'static str| {
            div()
                .id(id)
                .text_color(c(p.accent))
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(label)
                .on_click(move |_, _, cx| cx.open_url(url))
        };

        div()
            .child(setting_section(p, "languages", t("settings.language.title")).child(languages))
            .child(setting_section(p, "palette", t("settings.appearance.title")).child(themes))
            .child(
                setting_section(p, "refresh-cw", t("settings.update.title")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .id("check-update")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .px(px(14.))
                                .py(px(6.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(c(p.border))
                                .bg(c(p.surface))
                                .cursor_pointer()
                                .hover(move |s| s.bg(c(hover)))
                                .child(icon("download").size(px(14.)))
                                .child(t("settings.update.check"))
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.check_updates(false, window, cx)
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(c(p.muted))
                                .child(self.update_status.clone()),
                        )
                        .when_some(self.update.as_ref(), |d, r| {
                            let url = r.url.clone();
                            d.child(
                                div()
                                    .id("release-link")
                                    .text_size(px(12.))
                                    .text_color(c(p.accent))
                                    .cursor_pointer()
                                    .child("GitHub Releases")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                ),
            )
            .child(
                setting_section(p, "info", t("settings.about.title"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(format!(
                                "{} {}",
                                t("settings.about.version"),
                                env!("CARGO_PKG_VERSION")
                            ))
                            .child(link(
                                "link-site",
                                "zhconvert.org".into(),
                                "https://zhconvert.org",
                            ))
                            .child(link(
                                "link-license",
                                t("settings.about.license"),
                                "https://docs.zhconvert.org/license/",
                            ))
                            .child(link("link-repo", "GitHub".into(), REPO_URL)),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .text_size(px(11.))
                            .text_color(c(p.muted))
                            .child(t("settings.about.apiNotice")),
                    ),
            )
    }
}

impl Fanhuaji {
    fn set_converter(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if !self.queue.converting() && self.settings.converter != key {
            self.settings.converter = key.into();
            self.settings_changed(cx);
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    fn toggle_popover(&mut self, which: Popover, cx: &mut Context<Self>) {
        self.armed = None;
        self.popover = if self.popover == Some(which) {
            None
        } else {
            Some(which)
        };
        cx.notify();
    }
}

// --- Small pieces, named after the Tauri app's CSS classes ---

/// `.icon-btn`
fn icon_button(p: &Pal, id: &'static str, glyph: &str, size: f32) -> Stateful<Div> {
    let (hover, text) = (p.hover, p.text);
    div()
        .id(id)
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(c(p.muted))
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)).text_color(c(text)))
        .child(icon(glyph).size(px(size)))
}

/// `.fc-undo` / `.fc-reveal` / `.fc-remove`
fn small_icon_button(
    p: &Pal,
    id: impl Into<ElementId>,
    glyph: &str,
    accent: bool,
) -> Stateful<Div> {
    let hover = p.hover_strong;
    let hover_fg = if accent { p.accent } else { p.text };
    div()
        .id(id)
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_color(c(if accent { p.accent } else { p.muted }))
        .when(accent, |d| d.bg(c(p.accent_weak)))
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)).text_color(c(hover_fg)))
        .child(icon(glyph).size(px(15.)))
}

/// `.pane-act`
fn pane_action(p: &Pal, id: &'static str, glyph: Option<&str>, label: String) -> Stateful<Div> {
    let (hover, text) = (p.hover, p.text);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(c(p.muted))
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)).text_color(c(text)))
        .when_some(glyph, |d, g| d.child(icon(g).size(px(14.))))
        .child(label)
}

/// `.status-action`
fn status_action(
    p: &Pal,
    id: &'static str,
    glyph: Option<&str>,
    label: String,
    armed: bool,
) -> Stateful<Div> {
    let (hover, text) = (p.hover, p.text);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .cursor_pointer()
        .when(armed, |d| {
            d.text_color(c(p.accent))
                .bg(c(p.accent_weak))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!armed, |d| {
            d.text_color(c(p.muted))
                .hover(move |s| s.bg(c(hover)).text_color(c(text)))
        })
        .when_some(glyph, |d, g| d.child(icon(g).size(px(13.))))
        .child(label)
}

/// `.fmt-badge`, tinted per format as in the Tauri app.
fn format_badge(p: &Pal, path: &std::path::Path) -> impl IntoElement {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (bg, fg, border) = match ext.as_str() {
        "srt" => (p.accent_weak, p.accent, 0),
        "ass" | "ssa" => (p.green_weak, p.green, 0),
        "vtt" | "epub" => (p.yellow_weak, p.yellow, 0),
        _ => (p.sunken, p.muted, p.border),
    };
    div()
        .w(px(38.))
        .h(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(border))
        .bg(c(bg))
        .text_color(c(fg))
        .font_family(MONO)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(ext_label(path))
}

/// `.out-pop`
fn popover_panel(p: &Pal, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .absolute()
        .top(px(42.))
        .right_0()
        .flex()
        .flex_col()
        .p(px(5.))
        .rounded(px(8.))
        .border_1()
        .border_color(c(p.border))
        .bg(c(p.surface))
        .shadow_md()
        // Clicks inside must not reach the catcher that closes the popover.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// `.out-pop-group`
fn pop_group(p: &Pal, label: String) -> impl IntoElement {
    div()
        .px(px(9.))
        .pt(px(8.))
        .pb(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(c(p.muted))
        .child(label)
}

/// `.out-opt`: title, optional description, check when selected.
fn pop_option(
    p: &Pal,
    id: impl Into<ElementId>,
    title: String,
    desc: Option<String>,
    checked: bool,
) -> Stateful<Div> {
    let hover = p.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(9.))
        .py(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_color(c(if checked { p.accent } else { p.text }))
                        .when(checked, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(title),
                )
                .when_some(desc, |d, desc| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(c(p.muted))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(desc),
                    )
                }),
        )
        .child(
            icon("check")
                .size(px(15.))
                .text_color(c(p.accent))
                .when(!checked, |i| i.opacity(0.)),
        )
}

/// `.setting-toggle`: a checkbox row.
fn checkbox(p: &Pal, id: &'static str, checked: bool, label: String) -> Stateful<Div> {
    let hover = p.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(9.))
        .py(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)))
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(c(if checked { p.accent } else { p.border_strong }))
                .bg(c(if checked { p.accent } else { p.surface }))
                .when(checked, |d| {
                    d.child(icon("check").size(px(12.)).text_color(c(p.on_accent)))
                }),
        )
        .child(label)
}

/// `.setting-section` with its uppercase `h3`.
fn setting_section(p: &Pal, glyph: &str, title: String) -> Div {
    div()
        .px(px(16.))
        .py(px(12.))
        .border_b_1()
        .border_color(c(p.border_light))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .mb(px(10.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(p.muted))
                .child(icon(glyph).size(px(14.)))
                .child(title),
        )
}

/// `.theme-options`: a bordered segment whose active part is filled with the accent.
fn segmented<const N: usize>(
    p: &Pal,
    parts: [(ElementId, Option<&str>, String, bool, Click); N],
    small: bool,
) -> impl IntoElement {
    let hover = p.hover;
    // The outer row keeps the segment at its content width (`inline-flex` in the Tauri app).
    div().flex().child(
        div()
            .flex()
            .flex_none()
            .rounded(px(6.))
            .border_1()
            .border_color(c(p.border))
            .overflow_hidden()
            .children(parts.into_iter().enumerate().map(
                move |(i, (id, glyph, label, active, on_click))| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(if small { 7. } else { 14. }))
                        .py(px(if small { 3. } else { 6. }))
                        .text_size(px(if small { 11. } else { 12. }))
                        .when(i + 1 < N, |d| d.border_r_1().border_color(c(p.border)))
                        .cursor_pointer()
                        .when(active, |d| d.bg(c(p.accent)).text_color(c(p.on_accent)))
                        .when(!active, |d| d.hover(move |s| s.bg(c(hover))))
                        .when_some(glyph, |d, g| d.child(icon(g).size(px(14.))))
                        .child(label)
                        .on_click(on_click)
                },
            )),
    )
}

fn marked_text(text: &str, ranges: &[Range<usize>], style: HighlightStyle) -> StyledText {
    let text = if text.is_empty() { " " } else { text };
    StyledText::new(text.to_string()).with_highlights(ranges.iter().map(|r| (r.clone(), style)))
}

/// `.diff-line`: line number, then the original (muted) above the conversion.
fn diff_row(p: &Pal, row: &Row, line: usize, simplified: bool) -> AnyElement {
    let orig_chg = HighlightStyle {
        color: Some(c(p.text).into()),
        background_color: Some(c(p.hover_strong).into()),
        ..Default::default()
    };
    let conv_chg = HighlightStyle {
        color: Some(c(p.accent).into()),
        background_color: Some(c(p.accent_weak).into()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let conv = |el: StyledText| {
        // macOS ships PingFang SC; elsewhere the name misses and the default face is used.
        div()
            .text_size(px(14.))
            .text_color(c(p.text))
            .when(simplified, |d| d.font_family("PingFang SC"))
            .child(el)
    };
    let orig = |el: StyledText| div().text_size(px(14.)).text_color(c(p.muted)).child(el);
    let whole = |s: &str| std::slice::from_ref(&(0..s.len())).to_vec();

    let (num, bg, body): (String, u32, Div) = match row {
        Row::Equal(s) => (
            line.to_string(),
            0,
            div().child(conv(marked_text(s, &[], conv_chg))),
        ),
        Row::Replace { orig: o, conv: v } => (
            line.to_string(),
            p.accent_weak,
            div()
                .flex()
                .flex_col()
                .child(orig(marked_text(&o.text, &o.changed, orig_chg)))
                .child(conv(marked_text(&v.text, &v.changed, conv_chg))),
        ),
        Row::Insert(s) => (
            line.to_string(),
            p.accent_weak,
            div().child(conv(marked_text(s, &whole(s), conv_chg))),
        ),
        Row::Delete(s) => (
            "−".into(),
            p.hover,
            div()
                .line_through()
                .opacity(0.65)
                .child(orig(marked_text(s, &[], orig_chg))),
        ),
    };
    div()
        .flex()
        .gap(px(10.))
        .px(px(16.))
        .py(px(7.))
        .border_b_1()
        .border_color(c(p.border_light))
        .bg(c(bg))
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .pt(px(2.))
                .font_family(MONO)
                .text_size(px(11.))
                .text_color(c(p.faint))
                .child(num),
        )
        .child(div().flex_1().min_w_0().child(body))
        .into_any_element()
}
