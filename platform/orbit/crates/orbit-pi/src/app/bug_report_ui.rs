//! Settings → Report a bug: an inline issue form.
//!
//! The form takes free-form context at the top and can draft the issue title
//! and description from it with the app's active model, then files a GitHub
//! issue on the project repository with the machine's build details attached.
//! The drafting lives in [`crate::issue_message`], the diagnostics and filing
//! in [`crate::bug_report`], and both run off the UI thread because they shell
//! out to `pi` and `gh`.
//!
//! Screenshots are written to a temp directory and uploaded with
//! `gh issue create --attach`; a signed-out `gh` (or one without `--attach`)
//! falls back to GitHub's web form, where the saved files are dragged in.

use super::helpers::*;
use super::*;
use crate::git_panel::widgets::action_button;
use crate::issue_message::ReportKind;
use crate::theme::tokens::{input, ButtonSize, DynamicSpacing, IconSize, Radius, TextSize};
use gpui::ClickEvent;

/// The most screenshots one report may carry.
const MAX_SCREENSHOTS: usize = 6;

impl OrbitApp {
    /// The Report a bug page body: the fields, the screenshots, the diagnostics
    /// note, and the submit controls.
    pub(super) fn bug_report_rows(&self, theme: Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        let generate_label = if self.bug_report_generating {
            tr!("bug_report.generating")
        } else {
            tr!("bug_report.generate")
        };
        let generate = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .children(
                (!self.model_label.is_empty() && self.model_label != "…").then(|| {
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!(
                            "bug_report.using_model",
                            model = self.model_label.clone()
                        ))
                }),
            )
            .child(action_button(
                "bug-report-generate",
                &generate_label,
                Some(
                    icon(
                        "icons/spark.svg",
                        ButtonSize::Medium.icon_size().px(&theme),
                        theme.text_secondary,
                    )
                    .into_any_element(),
                ),
                false,
                self.bug_report_generating || self.bug_report_busy,
                theme,
                cx.listener(|this, _: &ClickEvent, _, cx| this.generate_bug_report_draft(cx)),
            ))
            .into_any_element();

        let fields = self.settings_section_desc(
            theme,
            &tr!("bug_report.heading"),
            Some(&tr!("bug_report.intro")),
            vec![
                bug_row(
                    theme,
                    tr!("bug_report.issue_type"),
                    self.report_type_selector(theme, cx),
                ),
                bug_field(
                    theme,
                    tr!("bug_report.context"),
                    self.bug_report_context.clone(),
                    px(96.),
                    Some(generate),
                ),
                bug_field(
                    theme,
                    tr!("bug_report.title"),
                    self.bug_report_title.clone(),
                    px(0.),
                    None,
                ),
                bug_field(
                    theme,
                    tr!(report_what_key(self.bug_report_kind)),
                    self.bug_report_what.clone(),
                    px(120.),
                    None,
                ),
                bug_field(
                    theme,
                    tr!(report_steps_key(self.bug_report_kind)),
                    self.bug_report_steps.clone(),
                    px(84.),
                    None,
                ),
                self.bug_screenshots_field(theme, cx),
            ],
        );

        let mut rows = vec![fields];
        if let Some(error) = self.bug_report_error.as_deref() {
            rows.push(
                div()
                    .w_full()
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .bg(theme.error.opacity(0.10))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.error)
                    .child(error.to_string())
                    .into_any_element(),
            );
        }

        let submit_label = if self.bug_report_busy {
            tr!("bug_report.creating")
        } else {
            tr!("bug_report.submit")
        };
        let busy = self.bug_report_busy || self.bug_report_generating;
        rows.push(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(DynamicSpacing::Base12.px(&theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .child(icon(
                            "icons/github.svg",
                            IconSize::Small.px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(tr!(
                                    "bug_report.diagnostics",
                                    version = env!("CARGO_PKG_VERSION")
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(action_button(
                            "bug-report-clear",
                            &tr!("bug_report.clear"),
                            None,
                            false,
                            busy,
                            theme,
                            cx.listener(|this, _: &ClickEvent, _, cx| this.clear_bug_report(cx)),
                        ))
                        .child(action_button(
                            "bug-report-submit",
                            &submit_label,
                            Some(
                                icon(
                                    "icons/github.svg",
                                    ButtonSize::Medium.icon_size().px(&theme),
                                    theme.action_fg,
                                )
                                .into_any_element(),
                            ),
                            true,
                            busy,
                            theme,
                            cx.listener(|this, _: &ClickEvent, _, cx| this.submit_bug_report(cx)),
                        )),
                )
                .into_any_element(),
        );
        rows
    }

    /// The Issue type control: a segmented Bug report / Feature request /
    /// Other. The choice drives the GitHub label, the description section
    /// names, and the draft prompt's framing.
    fn report_type_selector(&self, theme: Theme, cx: &Context<Self>) -> AnyElement {
        const KINDS: [ReportKind; 3] = [ReportKind::Bug, ReportKind::Feature, ReportKind::Other];
        let mut row = div().flex().items_center();
        for (ix, kind) in KINDS.iter().enumerate() {
            let kind = *kind;
            let selected = self.bug_report_kind == kind;
            let seg = segmented_segment(
                div().id(ElementId::Name(format!("bug-report-type-{ix}").into())),
                &theme,
                ButtonSize::Medium,
                SegmentPosition::at(ix, KINDS.len()),
            )
            .cursor_pointer();
            let seg = if selected {
                seg.bg(theme.surface_active)
                    .text_color(theme.text_primary)
                    .border_color(theme.border)
            } else {
                seg.bg(theme.surface_elevated)
                    .text_color(theme.text_secondary)
                    .border_color(theme.border)
                    .hover(|style| style.bg(theme.surface_hover))
            };
            row = row.child(
                press(seg)
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            this.bug_report_kind = kind;
                            this.bug_report_error = None;
                            cx.notify();
                        }),
                    )
                    .child(tr!(report_kind_key(kind))),
            );
        }
        row.into_any_element()
    }

    /// The Screenshots row: an Add button plus a thumbnail chip per image.
    fn bug_screenshots_field(&self, theme: Theme, cx: &Context<Self>) -> AnyElement {
        let add = action_button(
            "bug-report-add-screenshot",
            &tr!("bug_report.add_screenshot"),
            Some(
                icon(
                    "icons/image.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.text_secondary,
                )
                .into_any_element(),
            ),
            false,
            self.bug_report_screenshots.len() >= MAX_SCREENSHOTS,
            theme,
            cx.listener(|this, _: &ClickEvent, _, cx| this.add_bug_report_screenshots(cx)),
        );
        let mut column = div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .text_size(input::LABEL.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("bug_report.screenshots")),
                    )
                    .child(add),
            );
        if !self.bug_report_screenshots.is_empty() {
            column = column.child(self.bug_screenshot_chips(theme, cx));
        }
        column
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("bug_report.screenshot_hint")),
            )
            .into_any_element()
    }

    /// The screenshot thumbnail chips, each removing itself on click.
    fn bug_screenshot_chips(&self, theme: Theme, cx: &Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap(DynamicSpacing::Base06.px(&theme))
            .children(
                self.bug_report_screenshots
                    .iter()
                    .enumerate()
                    .map(|(ix, shot)| {
                        let visual: AnyElement = match &shot.preview {
                            Some(image) => div()
                                .size(IconSize::Medium.px(&theme))
                                .flex_none()
                                .rounded(Radius::Small.px(&theme))
                                .overflow_hidden()
                                .child(
                                    img(ImageSource::Image(image.clone()))
                                        .size_full()
                                        .object_fit(ObjectFit::Cover),
                                )
                                .into_any_element(),
                            None => icon(
                                "icons/image.svg",
                                IconSize::XSmall.px(&theme),
                                theme.text_muted,
                            )
                            .into_any_element(),
                        };
                        button_frame(
                            div().id(ElementId::NamedInteger("bug-screenshot".into(), ix as u64)),
                            &theme,
                            ButtonSize::Medium,
                        )
                        .border_1()
                        .border_color(theme.border)
                        .raised(theme.surface_elevated, &theme)
                        .cursor_pointer()
                        .hover(|style| style.raised(theme.overlay, &theme))
                        .child(visual)
                        .child(
                            div()
                                .max_w(px(160.))
                                .truncate()
                                .text_color(theme.text_secondary)
                                .child(shot.name.clone()),
                        )
                        .child(icon(
                            "icons/x.svg",
                            ButtonSize::Medium.icon_size().px(&theme),
                            theme.text_muted,
                        ))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                this.remove_bug_report_screenshot(ix, cx)
                            }),
                        )
                        .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// Clear the form fields, screenshots, and any error. Used by the Clear
    /// button and after a successful filing.
    pub(super) fn clear_bug_report(&mut self, cx: &mut Context<Self>) {
        self.bug_report_title
            .update(cx, |input, cx| input.clear(cx));
        self.bug_report_what.update(cx, |input, cx| input.clear(cx));
        self.bug_report_context
            .update(cx, |input, cx| input.clear(cx));
        self.bug_report_steps
            .update(cx, |input, cx| input.clear(cx));
        self.bug_report_screenshots.clear();
        self.bug_report_error = None;
        cx.notify();
    }

    /// Pick image files to attach. The native panel opens asynchronously so it
    /// never blocks the main thread while this entity is borrowed.
    pub(super) fn add_bug_report_screenshots(&mut self, cx: &mut Context<Self>) {
        if self.bug_report_screenshots.len() >= MAX_SCREENSHOTS {
            self.bug_report_error =
                Some(tr!("bug_report.screenshot_limit", count = MAX_SCREENSHOTS));
            cx.notify();
            return;
        }
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(tr!("bug_report.add_screenshot"))
            .add_filter(
                tr!("settings.images_filter"),
                &["png", "jpg", "jpeg", "webp", "gif", "bmp"],
            );
        cx.spawn(async move |this, cx| {
            let Some(handles) = dialog.pick_files().await else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                for handle in handles {
                    if app.bug_report_screenshots.len() >= MAX_SCREENSHOTS {
                        break;
                    }
                    let path = handle.path().to_path_buf();
                    match Attachment::from_path(&path) {
                        Some(attachment) => {
                            if !app
                                .bug_report_screenshots
                                .iter()
                                .any(|shot| shot.name == attachment.name)
                            {
                                app.bug_report_screenshots.push(attachment);
                            }
                        }
                        None => {
                            app.bug_report_error =
                                Some(tr!("composer_ops.unsupported_image_format"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop the screenshot at `ix`.
    pub(super) fn remove_bug_report_screenshot(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.bug_report_screenshots.len() {
            self.bug_report_screenshots.remove(ix);
            cx.notify();
        }
    }

    /// Draft the issue title and description from the context field with the
    /// app's active model. The one-shot `pi` call runs off the UI thread; when
    /// pi is unavailable the draft falls back to the notes themselves.
    pub(super) fn generate_bug_report_draft(&mut self, cx: &mut Context<Self>) {
        if self.bug_report_generating || self.bug_report_busy {
            return;
        }
        let context = self.bug_report_context.read(cx).text();
        if context.trim().is_empty() {
            self.bug_report_error = Some(tr!("bug_report.context_required"));
            cx.notify();
            return;
        }
        let cwd = self.workspace_dir();
        let provider = self.model_provider.clone();
        let model = self.model_id.clone();
        let kind = self.bug_report_kind;
        self.bug_report_generating = true;
        self.bug_report_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let draft = cx
                .background_executor()
                .spawn(async move {
                    let provider = (!provider.is_empty()).then_some(provider.as_str());
                    let model = (!model.is_empty()).then_some(model.as_str());
                    crate::bug_report::draft(&cwd, provider, model, kind, &context)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.bug_report_generating = false;
                app.bug_report_title
                    .update(cx, |input, cx| input.set_text(draft.title, cx));
                app.bug_report_what
                    .update(cx, |input, cx| input.set_text(draft.body, cx));
                app.toast_info(tr!("bug_report.draft_ready"));
                cx.notify();
            });
        })
        .detach();
    }

    /// File the report on the background executor. The `gh` shell-out and the
    /// host probes must never block a frame. With screenshots the report opens
    /// GitHub's web form instead, because the API cannot attach binary files.
    pub(super) fn submit_bug_report(&mut self, cx: &mut Context<Self>) {
        if self.bug_report_busy {
            return;
        }
        let title = self.bug_report_title.read(cx).text().trim().to_string();
        let what = self.bug_report_what.read(cx).text();
        if title.is_empty() {
            self.bug_report_error = Some(tr!("bug_report.title_required"));
            cx.notify();
            return;
        }
        if what.trim().is_empty() {
            self.bug_report_error = Some(tr!("bug_report.body_required"));
            cx.notify();
            return;
        }
        let steps = self.bug_report_steps.read(cx).text();
        let kind = self.bug_report_kind;
        let cwd = self.workspace_dir();
        let screenshots: Vec<(String, Vec<u8>)> = self
            .bug_report_screenshots
            .iter()
            .filter_map(|shot| shot.bytes().map(|bytes| (shot.name.clone(), bytes)))
            .collect();
        self.bug_report_busy = true;
        self.bug_report_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let diagnostics = crate::bug_report::Diagnostics::collect(&cwd);
                    crate::bug_report::file(
                        &cwd,
                        kind,
                        &title,
                        &what,
                        &steps,
                        &diagnostics,
                        &screenshots,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.bug_report_busy = false;
                match result {
                    Ok(crate::bug_report::Filed::Created(url)) => {
                        if let Err(err) = platform::open_url(&url) {
                            app.toast_warning(tr!("bug_report.open_failed", error = err));
                        }
                        app.clear_bug_report(cx);
                        app.toast_success(tr!("bug_report.created"));
                    }
                    Ok(crate::bug_report::Filed::Browser { url, screenshots }) => {
                        if let Err(err) = platform::open_url(&url) {
                            app.bug_report_error = Some(tr!("bug_report.open_failed", error = err));
                        } else if let Some(directory) = screenshots {
                            app.toast_info(tr!(
                                "bug_report.screenshots_saved",
                                dir = directory.to_string_lossy()
                            ));
                        } else {
                            app.toast_info(tr!("bug_report.browser"));
                        }
                    }
                    Err(err) => {
                        app.bug_report_error = Some(tr!("bug_report.failed", error = err));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The locale key for the report kind's segmented-control label.
fn report_kind_key(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Bug => "bug_report.type_bug",
        ReportKind::Feature => "bug_report.type_feature",
        ReportKind::Other => "bug_report.type_other",
    }
}

/// The locale key for the description field's label, per report kind.
fn report_what_key(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Bug => "bug_report.what",
        ReportKind::Feature => "bug_report.what_problem",
        ReportKind::Other => "bug_report.what_details",
    }
}

/// The locale key for the second field's label, per report kind.
fn report_steps_key(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Bug => "bug_report.steps",
        ReportKind::Feature => "bug_report.steps_solution",
        ReportKind::Other => "bug_report.steps_details",
    }
}

/// A full-width board row: a label on the left and a control on the right.
fn bug_row(theme: Theme, label: String, control: AnyElement) -> AnyElement {
    div()
        .w_full()
        .px(DynamicSpacing::Base16.px(&theme))
        .py(DynamicSpacing::Base12.px(&theme))
        .flex()
        .items_center()
        .justify_between()
        .gap(DynamicSpacing::Base12.px(&theme))
        .child(
            div()
                .text_size(TextSize::Default.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(label),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// One full-width labeled input in the report board: the label (and an
/// optional trailing control) above a field that stretches to `min_h` so a
/// multi-line editor takes clicks anywhere in its box, not just on the first
/// line.
fn bug_field(
    theme: Theme,
    label: String,
    input: Entity<ComposerInput>,
    min_h: Pixels,
    action: Option<AnyElement>,
) -> AnyElement {
    div()
        .w_full()
        .px(DynamicSpacing::Base16.px(&theme))
        .py(DynamicSpacing::Base12.px(&theme))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .text_size(input::LABEL.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(label),
                )
                .children(action),
        )
        .child(
            stretch(input_field_frame(div(), &theme))
                .w_full()
                .min_h(min_h.max(input::min_height(&theme)))
                .bg(theme.surface_secondary)
                .child(div().flex_1().min_w_0().flex().flex_col().child(input)),
        )
        .into_any_element()
}

/// `input_field_frame` centers a one-line input; a multi-line field stretches
/// its input column to the box instead. GPUI 0.2.2 has no `items_stretch`, so
/// set the style directly (the same shim the Git page uses).
fn stretch<E: Styled>(mut el: E) -> E {
    el.style().align_items = Some(gpui::AlignItems::Stretch);
    el
}
