use super::helpers::*;
use super::*;
use crate::theme::tokens::{
    context_menu, picker, popover, ButtonSize, DynamicSpacing, IconSize, Radius, TextSize,
};

impl OrbitApp {
    /// The filtered menu entries for the active trigger (empty when none).
    pub(super) fn autocomplete_entries(&mut self, trigger: &Trigger) -> Vec<AcEntry> {
        match trigger.kind {
            TriggerKind::Slash => mentions::filter_entries(
                &trigger.query,
                &[],
                &self.slash_commands,
                AUTOCOMPLETE_LIMIT,
            ),
            TriggerKind::At => {
                self.ensure_mention_files();
                mentions::filter_entries(
                    &trigger.query,
                    &self.mention_files,
                    &[],
                    AUTOCOMPLETE_LIMIT,
                )
            }
        }
    }

    /// (Re)build the workspace file cache when the workspace changed.
    pub(super) fn ensure_mention_files(&mut self) {
        if self.mention_files_workspace == self.current_workspace {
            return;
        }
        self.mention_files_workspace = self.current_workspace.clone();
        self.mention_files = self
            .current_workspace
            .as_deref()
            .map(mentions::list_workspace_files)
            .unwrap_or_default();
    }

    /// Derive the open/closed/highlight state from the composer text.
    /// The trigger is pure text state, so this runs every frame and the
    /// menu opens/closes by itself as the user types.
    pub(super) fn sync_autocomplete(&mut self, cx: &Context<Self>) {
        let trigger = self.input.read(cx).active_trigger();
        let key = trigger.as_ref().map(|t| (t.kind, t.query.clone()));
        if key != self.last_ac_trigger {
            // A new (or changed) token re-arms a mouse dismissal and resets
            // the highlight.
            self.autocomplete_dismissed = false;
        }
        self.last_ac_trigger = key;
        let mut state = self.autocomplete.borrow_mut();
        if !self.autocomplete_dismissed {
            if let Some(trigger) = &trigger {
                let count = match trigger.kind {
                    TriggerKind::Slash => mentions::filter_entries(
                        &trigger.query,
                        &[],
                        &self.slash_commands,
                        AUTOCOMPLETE_LIMIT,
                    )
                    .len(),
                    TriggerKind::At => {
                        drop(state);
                        self.ensure_mention_files();
                        state = self.autocomplete.borrow_mut();
                        mentions::filter_entries(
                            &trigger.query,
                            &self.mention_files,
                            &[],
                            AUTOCOMPLETE_LIMIT,
                        )
                        .len()
                    }
                };
                state.count = count;
                state.open = count > 0;
                state.highlighted = state.highlighted.min(count.saturating_sub(1));
            } else {
                state.open = false;
                state.count = 0;
                state.highlighted = 0;
            }
        } else {
            state.open = false;
        }
    }

    /// Commit the highlighted entry (Enter/click): replace the trigger
    /// token with the completed `/name ` or `@path ` text.
    pub(super) fn commit_autocomplete_if_open(&mut self, cx: &mut Context<Self>) -> bool {
        let (open, highlighted) = {
            let state = self.autocomplete.borrow();
            (state.open, state.highlighted)
        };
        if !open {
            return false;
        }
        let Some(trigger) = self.input.read(cx).active_trigger() else {
            return false;
        };
        let entries = self.autocomplete_entries(&trigger);
        let Some(entry) = entries.into_iter().nth(highlighted) else {
            return false;
        };
        self.commit_entry(entry, cx);
        true
    }

    /// Insert `entry`'s completion over the active trigger token.
    pub(super) fn commit_entry(&mut self, entry: AcEntry, cx: &mut Context<Self>) {
        let Some(trigger) = self.input.read(cx).active_trigger() else {
            return;
        };
        let text = match &entry {
            AcEntry::Command { name, .. } => format!("/{name} "),
            AcEntry::File { path } => format!("@{path} "),
        };
        self.input.update(cx, |input, cx| {
            input.replace_range(trigger.start..trigger.end, &text, cx)
        });
        // Keep the menu closed until the trigger changes (the replaced text
        // still ends in a space, but stay explicit).
        self.autocomplete_dismissed = true;
        self.last_ac_trigger = None;
        self.autocomplete.borrow_mut().open = false;
        cx.notify();
    }

    /// Outside mouse-down dismisses the menu until the trigger changes.
    pub(super) fn on_autocomplete_outside_down(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.autocomplete.borrow().open {
            self.autocomplete_dismissed = true;
            cx.notify();
        }
    }

    /// The autocomplete popup, anchored above the composer box (mirrors
    /// the model/thinking chip popovers). `None` while closed.
    pub(super) fn autocomplete_popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let highlighted = {
            let state = self.autocomplete.borrow();
            if !state.open || state.count == 0 {
                return None;
            }
            state.highlighted
        };
        let trigger = self.input.read(cx).active_trigger()?;
        let entries = self.autocomplete_entries(&trigger);
        if entries.is_empty() {
            return None;
        }
        let highlighted = highlighted.min(entries.len() - 1);
        let theme = *theme::get(cx);
        let this = cx.weak_entity();
        // Rows are one fixed height, so the list is exactly tall enough for
        // the `AUTOCOMPLETE_LIMIT` entries it ever shows.
        let row_h = picker::entry_height(&theme);
        let mut list = div()
            .id("ac-list")
            .w_full()
            .max_h(row_h * AUTOCOMPLETE_LIMIT as f32)
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (ix, entry) in entries.iter().enumerate() {
            let selected = ix == highlighted;
            let this = this.clone();
            let entry = entry.clone();
            // Leading glyph: brand glyph for commands; for files, the
            // devicons Nerd Font glyph (falls back to the extension text
            // badge when no Nerd Font is installed).
            let nerd = nerd_font_family(cx);
            let leading: AnyElement = match &entry {
                AcEntry::Command { .. } => icon(
                    "icons/extensions.svg",
                    context_menu::ICON.px(&theme),
                    theme.text_muted,
                )
                .into_any_element(),
                AcEntry::File { path } => file_glyph(
                    path.as_str(),
                    theme.mode == ThemeMode::Dark,
                    nerd.as_ref(),
                    context_menu::ICON.px(&theme),
                    file_badge(path.as_str(), theme),
                ),
            };
            let title = match &entry {
                AcEntry::Command { name, .. } => format!("/{name}"),
                AcEntry::File { path } => path.rsplit('/').next().unwrap_or(path).to_string(),
            };
            let subtitle = match &entry {
                AcEntry::Command { description, .. } => description.clone(),
                // The directory part, shown dimmed after the basename.
                AcEntry::File { path } => match path.rsplit_once('/') {
                    Some((dir, _)) if !dir.is_empty() => format!("{dir}/"),
                    _ => String::new(),
                },
            };
            // Commands carry a scope badge on the right (skills, orbit,
            // custom, the project name…); files have no scope.
            let scope_badge: Option<String> = match &entry {
                AcEntry::Command { scope, .. } => Some(scope.label()),
                AcEntry::File { .. } => None,
            };
            list = list.child(
                picker_entry(
                    div().id(ElementId::NamedInteger("ac-row".into(), ix as u64)),
                    &theme,
                )
                .h(row_h)
                .cursor_pointer()
                .when(selected, |row| row.bg(theme.selection))
                .hover(|style| style.bg(theme.surface_hover))
                .on_click(move |_, _, cx| {
                    this.update(cx, |app, cx| app.commit_entry(entry.clone(), cx))
                        .ok();
                })
                .child(leading)
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        // Fuzzy match first: the basename (or command
                        // name) leads, then — after a breath — the rest
                        // of the path / description in dimmed text.
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(CONTENT_MAX_W / 2.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if selected {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                })
                                .child(title),
                        )
                        .when(!subtitle.is_empty(), |row| {
                            row.child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(picker::SECONDARY_TEXT.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(subtitle),
                            )
                        }),
                )
                .when_some(scope_badge, |row, badge| {
                    row.child(
                        div()
                            .h(px(18.))
                            .px(DynamicSpacing::Base06.px(&theme))
                            .rounded(Radius::Medium.px(&theme))
                            .flex_none()
                            .flex()
                            .items_center()
                            .bg(theme.overlay_strong)
                            .text_size(TextSize::XSmall.px(&theme))
                            .text_color(theme.text_muted)
                            .child(badge),
                    )
                }),
            );
        }
        // Full width of the chat box, so long paths are never cut.
        let popup = picker_surface(div(), &theme)
            .w(px(CONTENT_MAX_W))
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base01.px(&theme))
            .overflow_hidden()
            .occlude()
            .on_mouse_down_out(cx.listener(Self::on_autocomplete_outside_down))
            .child(list);

        Some(
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .anchor(Corner::BottomLeft)
                .offset(point(px(0.), -popover::MENU_OFFSET))
                .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                .child(deferred(popup))
                .into_any_element(),
        )
    }

    /// Attachment chips row shown above the input (pasted/picked images).
    pub(super) fn attachments_row(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let theme = *theme::get(cx);
        let row = div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap(DynamicSpacing::Base06.px(&theme));
        Some(
            row.children(self.attachments.iter().enumerate().map(|(ix, a)| {
                // Image attachments show the decoded image; the glyph is the
                // fallback if a preview never decoded.
                let visual: AnyElement = match &a.preview {
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
                    None => file_glyph(
                        &a.name,
                        theme.mode == ThemeMode::Dark,
                        nerd_font_family(cx).as_ref(),
                        IconSize::XSmall.px(&theme),
                        icon(
                            "icons/task.svg",
                            IconSize::XSmall.px(&theme),
                            theme.text_muted,
                        )
                        .into_any_element(),
                    )
                    .into_any_element(),
                };
                let chip = div().id(ElementId::NamedInteger("attachment".into(), ix as u64));
                button_frame(chip, &theme, ButtonSize::Medium)
                    .tip(tr!("composer.remove_attachment"))
                    .border_1()
                    .border_color(theme.border)
                    .raised(theme.surface_elevated, &theme)
                    .cursor_pointer()
                    .hover(|s| s.raised(theme.overlay, &theme))
                    .child(visual)
                    .child(
                        div()
                            .max_w(px(160.))
                            .truncate()
                            .text_color(theme.text_secondary)
                            .child(a.name.clone()),
                    )
                    .child(
                        icon(
                            "icons/x.svg",
                            ButtonSize::Medium.icon_size().px(&theme),
                            theme.text_muted,
                        )
                        .into_any_element(),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            if ix < this.attachments.len() {
                                this.attachments.remove(ix);
                                cx.notify();
                            }
                        }),
                    )
            }))
            .into_any_element(),
        )
    }

    /// Files dragged over the window (external OS drag — gpui mirrors them
    /// as an `ExternalPaths` drag). `on_drag_move` fires for *every* move
    /// during the drag with this element's bounds, so one handler tracks
    /// both entering and leaving the composer.
    pub(super) fn on_file_drag_move(
        &mut self,
        event: &DragMoveEvent<ExternalPaths>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_open {
            return;
        }
        let hovered = event.bounds.contains(&event.event.position);
        if hovered != self.file_drag_hovered {
            self.file_drag_hovered = hovered;
            cx.notify();
        }
    }

    /// Files dropped anywhere on the window: images become attachments
    /// (thumbnail chips above the composer); anything else is referenced by
    /// path at the caret so the agent can read it with its tools.
    pub(super) fn on_file_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_drag_hovered = false;
        if self.settings_open {
            cx.notify();
            return;
        }
        for path in paths.paths() {
            if self.attachments.len() >= MAX_ATTACHMENTS {
                self.set_status(tr!("composer_ops.max_attachments", count = MAX_ATTACHMENTS));
                break;
            }
            match Attachment::from_path(path) {
                Some(attachment) => self.push_attachment(attachment, cx),
                None => {
                    self.input.update(cx, |input, cx| {
                        input.insert_at_caret(&format!("{} ", path.display()), cx);
                    });
                    self.input.read(cx).focus(window);
                }
            }
        }
        cx.notify();
    }

    /// Add menu → "Attach image…": pick an image file and queue it.
    pub(super) fn attach_image(&mut self, cx: &mut Context<Self>) {
        if self.attachments.len() >= MAX_ATTACHMENTS {
            self.set_status(tr!("composer_ops.max_attachments", count = MAX_ATTACHMENTS));
            cx.notify();
            return;
        }
        // Opened asynchronously: a blocking `rfd::FileDialog` pumps a nested
        // main-thread modal loop while GPUI still holds this entity's mutable
        // borrow, so a queued task that updates the app aborts with
        // `already borrowed`. The async panel shows as a sheet and resolves
        // once the user answers, by which point the borrow is released.
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(tr!("composer_ops.attach_image_title"))
            .add_filter("Image", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]);
        cx.spawn(async move |this, cx| {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            // Decode + compress off the UI thread; only the finished
            // attachment (and its preview) lands on the next update.
            let attachment = Attachment::from_path(&path);
            let _ = this.update(cx, |app, cx| {
                match attachment {
                    Some(attachment) => app.push_attachment(attachment, cx),
                    None => app.set_status(tr!("composer_ops.unsupported_image_format")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Add menu → "Attach file…": any file. Images become attachments;
    /// anything else is referenced by path at the caret (same rule as a
    /// file dropped on the window).
    pub(super) fn attach_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // See `attach_image` — never block the main thread on the native panel
        // while this entity is borrowed.
        let dialog = rfd::AsyncFileDialog::new().set_title(tr!("composer_ops.attach_file_title"));
        cx.spawn_in(window, async move |this, cx| {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            // Decode + compress off the UI thread; only the finished
            // attachment (and its preview) lands on the next update.
            let attachment = Attachment::from_path(&path);
            let _ = this.update_in(cx, |app, window, cx| {
                match attachment {
                    Some(attachment) => app.push_attachment(attachment, cx),
                    None => {
                        app.input.update(cx, |input, cx| {
                            input.insert_at_caret(&format!("{} ", path.display()), cx);
                        });
                        app.input.read(cx).focus(window);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Mouse-up on the composer's "+" button: toggle the add menu, with the
    /// same click-through guard as the model/thinking chips.
    pub(super) fn on_add_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_add_menu(window, cx);
    }

    pub(super) fn toggle_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_menu_open {
            self.close_add_menu(window, cx);
            return;
        }
        // Mutually exclusive with the other composer popovers.
        self.model_selector = None;
        self.context_popup = ContextPopup::None;
        self.access_menu_open = false;
        self.workflow_menu_open = false;
        self.work_in_menu_open = false;
        self.autocomplete_dismissed = true;
        self.autocomplete.borrow_mut().open = false;
        self.add_menu_open = true;
        self.add_menu_highlight = 0;
        // Focus the menu so ↑/↓/Enter/Escape dispatch to it.
        window.focus(&self.add_menu_focus);
        cx.notify();
    }

    /// Close the add menu and hand focus back to the composer input.
    pub(super) fn close_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_menu_open {
            self.add_menu_open = false;
            self.add_menu_highlight = 0;
            self.input.read(cx).focus(window);
            cx.notify();
        }
    }

    pub(super) fn on_add_menu_next(
        &mut self,
        _: &crate::AddMenuNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_menu_highlight = (self.add_menu_highlight + 1) % ADD_MENU_ITEMS.len();
        cx.notify();
    }

    pub(super) fn on_add_menu_prev(
        &mut self,
        _: &crate::AddMenuPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_menu_highlight =
            (self.add_menu_highlight + ADD_MENU_ITEMS.len() - 1) % ADD_MENU_ITEMS.len();
        cx.notify();
    }

    pub(super) fn on_add_menu_confirm(
        &mut self,
        _: &crate::AddMenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_add_menu_item(self.add_menu_highlight, window, cx);
    }

    pub(super) fn on_add_menu_close(
        &mut self,
        _: &crate::AddMenuClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_add_menu(window, cx);
    }

    /// Execute add-menu row `ix` (see [`ADD_MENU_ITEMS`]), then close.
    pub(super) fn run_add_menu_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_menu_open = false;
        self.add_menu_highlight = 0;
        match ix {
            0 => self.attach_image(cx),
            1 => self.attach_file(window, cx),
            // Insert `@` at the caret — the file-mention autocomplete opens
            // on its own from the text trigger.
            _ => self
                .input
                .update(cx, |input, cx| input.insert_at_caret("@", cx)),
        }
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// Mouse-up on a chip that opens the given picker. If that picker was
    /// just dismissed by this click's mouse-down (outside-click dismissal),
    /// swallow the toggle so it stays closed.
    pub(super) fn on_chip_trigger_click(
        &mut self,
        kind: PickerKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_picker(kind, window, cx);
    }

    pub(super) fn on_model_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_chip_trigger_click(PickerKind::Model, window, cx);
    }

    pub(super) fn on_thinking_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_chip_trigger_click(PickerKind::Thinking, window, cx);
    }

    pub(super) fn set_model(&mut self, id: String, provider: String, cx: &mut Context<Self>) {
        // A manual choice supersedes the session-birth default push. The push
        // itself sends `SetModel` directly so it doesn't disarm itself.
        self.default_model_armed = false;
        self.send(
            CommandBody::SetModel {
                model_id: id,
                provider,
            },
            "set_model",
        );
        cx.notify();
    }

    pub(super) fn set_thinking_level(&mut self, level: String, cx: &mut Context<Self>) {
        // A manual level choice likewise supersedes the pending default.
        self.default_model_armed = false;
        self.send(
            CommandBody::SetThinkingLevel { level },
            "set_thinking_level",
        );
        cx.notify();
    }

    // ── access mode (the guard extension's policy) ──────────────────────

    /// Mouse-up on the access chip. Swallows the toggle if the menu was just
    /// dismissed by this click's mouse-down (outside-click dismissal).
    pub(super) fn on_access_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_access_menu(window, cx);
    }

    pub(super) fn toggle_access_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.access_menu_open {
            self.close_access_menu(window, cx);
            return;
        }
        // Mutually exclusive with the other composer popovers.
        self.model_selector = None;
        self.context_popup = ContextPopup::None;
        self.add_menu_open = false;
        self.work_in_menu_open = false;
        self.autocomplete_dismissed = true;
        self.autocomplete.borrow_mut().open = false;
        self.access_menu_open = true;
        self.workflow_menu_open = false;
        self.access_menu_highlight = AccessMode::ALL
            .iter()
            .position(|mode| *mode == self.access_mode)
            .unwrap_or(0);
        // Focus the menu so ↑/↓/Enter/Escape dispatch to it.
        window.focus(&self.access_menu_focus);
        cx.notify();
    }

    /// Close the access menu and hand focus back to the composer input.
    pub(super) fn close_access_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.access_menu_open {
            self.access_menu_open = false;
            self.input.read(cx).focus(window);
            cx.notify();
        }
    }

    pub(super) fn on_access_menu_next(
        &mut self,
        _: &crate::AccessMenuNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.access_menu_highlight = (self.access_menu_highlight + 1) % AccessMode::ALL.len();
        cx.notify();
    }

    pub(super) fn on_access_menu_prev(
        &mut self,
        _: &crate::AccessMenuPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.access_menu_highlight =
            (self.access_menu_highlight + AccessMode::ALL.len() - 1) % AccessMode::ALL.len();
        cx.notify();
    }

    pub(super) fn on_access_menu_confirm(
        &mut self,
        _: &crate::AccessMenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_access_menu_item(self.access_menu_highlight, window, cx);
    }

    pub(super) fn on_access_menu_close(
        &mut self,
        _: &crate::AccessMenuClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_access_menu(window, cx);
    }

    /// Select access mode `ix` (see [`AccessMode::ALL`]) and close the menu.
    pub(super) fn run_access_menu_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mode) = AccessMode::ALL.get(ix).copied() {
            self.set_access_mode(mode, cx);
        }
        self.close_access_menu(window, cx);
    }

    /// Apply an access mode: persist it (the guard extension reads the file
    /// on every tool call, so this re-arms live sessions) and report it. When
    /// the guard extension could not be installed the mode has no effect, and
    /// the status line says so rather than implying it took.
    pub(super) fn set_access_mode(&mut self, mode: AccessMode, cx: &mut Context<Self>) {
        self.access_mode = mode;
        mode.persist();
        if self.extensions.guard().is_none() {
            self.toast_warning(tr!("access.mode_set_unavailable", mode = mode.label()));
        } else {
            self.set_status(tr!("access.mode_set", mode = mode.label()));
        }
        cx.notify();
    }

    // ── workflow mode (per session; the workflow extension's policy) ────

    /// Mouse-up on the workflow chip. Swallows the toggle if the menu was
    /// just dismissed by this click's mouse-down (outside-click dismissal).
    pub(super) fn on_workflow_trigger_click(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const GESTURE: Duration = Duration::from_millis(200);
        if let Some(dismissed) = self.menu_dismissed_at.take() {
            if dismissed.elapsed() < GESTURE {
                return;
            }
        }
        self.toggle_workflow_menu(window, cx);
    }

    pub(super) fn toggle_workflow_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workflow_menu_open {
            self.close_workflow_menu(window, cx);
            return;
        }
        // Mutually exclusive with the other composer popovers.
        self.model_selector = None;
        self.context_popup = ContextPopup::None;
        self.access_menu_open = false;
        self.add_menu_open = false;
        self.work_in_menu_open = false;
        self.autocomplete_dismissed = true;
        self.autocomplete.borrow_mut().open = false;
        self.workflow_menu_open = true;
        self.workflow_menu_highlight = WorkflowMode::ALL
            .iter()
            .position(|mode| *mode == self.workflow_mode)
            .unwrap_or(0);
        window.focus(&self.workflow_menu_focus);
        cx.notify();
    }

    /// Close the workflow menu and hand focus back to the composer input.
    pub(super) fn close_workflow_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workflow_menu_open {
            self.workflow_menu_open = false;
            self.input.read(cx).focus(window);
            cx.notify();
        }
    }

    pub(super) fn on_workflow_menu_next(
        &mut self,
        _: &crate::WorkflowMenuNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_menu_highlight = (self.workflow_menu_highlight + 1) % WorkflowMode::ALL.len();
        cx.notify();
    }

    pub(super) fn on_workflow_menu_prev(
        &mut self,
        _: &crate::WorkflowMenuPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_menu_highlight =
            (self.workflow_menu_highlight + WorkflowMode::ALL.len() - 1) % WorkflowMode::ALL.len();
        cx.notify();
    }

    pub(super) fn on_workflow_menu_confirm(
        &mut self,
        _: &crate::WorkflowMenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_workflow_menu_item(self.workflow_menu_highlight, window, cx);
    }

    pub(super) fn on_workflow_menu_close(
        &mut self,
        _: &crate::WorkflowMenuClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_workflow_menu(window, cx);
    }

    /// Select workflow mode `ix` (see [`WorkflowMode::ALL`]) and close the menu.
    pub(super) fn run_workflow_menu_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mode) = WorkflowMode::ALL.get(ix).copied() {
            self.set_workflow_mode(mode, cx);
        }
        self.close_workflow_menu(window, cx);
    }

    /// Apply a workflow mode for the active session: persist it to the
    /// per-session store (the extension reads it fresh on every hook, so this
    /// re-arms a live session) and report it. Before a session exists the
    /// choice is held as pending and applied when the id arrives.
    pub(super) fn set_workflow_mode(&mut self, mode: WorkflowMode, cx: &mut Context<Self>) {
        self.workflow_mode = mode;
        match self.session_id.clone() {
            Some(id) => crate::workflow::persist_for(&id, mode),
            None => self.workflow_pending = Some(mode),
        }
        if self.extensions.workflow().is_none() {
            self.toast_warning(tr!("workflow.mode_set_unavailable", mode = mode.label()));
        } else {
            self.set_status(tr!("workflow.mode_set", mode = mode.label()));
        }
        cx.notify();
    }

    /// Drop workflow-store entries for sessions that no longer exist, keeping
    /// the active session (which may be an unlisted draft). Called whenever
    /// the session list reloads, so the store cannot grow without bound.
    pub(super) fn prune_workflow_store(&self) {
        let mut known: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
        if let Some(id) = &self.session_id {
            known.push(id.clone());
        }
        // Never prune from an empty set: a watcher firing before the session
        // list loads (or a store read failure) must not wipe every entry.
        if known.is_empty() {
            return;
        }
        crate::workflow::prune(&known);
    }
}
