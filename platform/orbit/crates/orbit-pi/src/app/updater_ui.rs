//! The updater's UI surface: drain its background events into the shell,
//! mirror the persisted preference, and drive one update modal from both the
//! Check for Updates command and the download control.
//!
//! The updater owns its own threads and status; this module only reads the
//! global and relays [`UpdaterEvent`]s. The modal is the single place a
//! release is offered: it shows the search, the changelog, and the install
//! decision, and the sidebar/settings controls only open it.

use super::helpers::*;
use super::*;
use crate::theme::tokens::{
    button, modal, AnimationDuration, ButtonSize, DynamicSpacing, IconSize, StyledExt, TextSize,
};
use crate::updater::{UpdateStatus, UpdaterEvent, UpdaterState};
use crate::usage::tooltip::Tooltip;

/// The sidebar updater pill: a `ButtonSize::Compact` disc at rest (the
/// download icon), expanding sideways to reveal the "Update" label while
/// hovered — the reference app's pattern. `UPDATER_PILL_COLLAPSED_W` is
/// shared with `OrbitApp::new` so the animation state starts parked at the
/// right width. Widths are unscaled px resolved through `theme.ui_px`, like
/// the button tokens, so the collapsed width (Compact's 18px) keeps the
/// resting pill a disc at every UI font size.
pub(super) const UPDATER_PILL_COLLAPSED_W: f32 = 18.;
pub(super) const UPDATER_PILL_EXPANDED_W: f32 = 58.;

impl OrbitApp {
    /// Whether this build can update itself at all (release, managed install,
    /// and a usable signing key).
    pub(super) fn updater_available(&self, cx: &App) -> bool {
        cx.try_global::<UpdaterState>()
            .is_some_and(|state| state.0.is_some())
    }

    /// Whether the updater can install what it reports. False for check-only
    /// builds (a forced dev run outside a managed install): the modal shows
    /// the release but offers no Update now.
    pub(super) fn updater_can_install(&self, cx: &App) -> bool {
        cx.try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
            .is_some_and(|updater| updater.can_install())
    }

    /// Re-read the updater's status, staged version and notes, history, and
    /// preference. Called when Settings opens so the controls reflect what is
    /// on disk.
    pub(super) fn refresh_updater(&mut self, cx: &App) {
        if let Some(updater) = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
        {
            self.updater_status = updater.status();
            self.updater_version = updater.available_version();
            self.updater_notes = updater.available_notes();
            self.updater_history = updater.history();
            self.automatic_updates_enabled = updater.automatically_checks_for_updates();
        }
    }

    /// Mirror the updater's feed snapshot beside the status: the staged
    /// release's version and notes, and every release for Version History.
    /// The worker stores them before it publishes `Available`, so by the time
    /// an event drains they are already there to read.
    fn sync_staged_release(&mut self, cx: &App) {
        let (version, notes, history) = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
            .map(|updater| {
                (
                    updater.available_version(),
                    updater.available_notes(),
                    updater.history(),
                )
            })
            .unwrap_or_default();
        self.updater_version = version;
        self.updater_notes = notes;
        self.updater_history = history;
    }

    /// Drain the updater channel. The heartbeat calls this each tick; it is
    /// cheap when nothing happened.
    pub(super) fn drain_updater_events(&mut self, cx: &mut Context<Self>) {
        let events: Vec<UpdaterEvent> = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
            .map(|updater| {
                let mut events = Vec::new();
                while let Some(event) = updater.try_recv_event() {
                    events.push(event);
                }
                events
            })
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }

        for event in events {
            match event {
                UpdaterEvent::StatusChanged(status) => {
                    self.updater_status = status;
                    self.reset_updater_pill_animation();
                }
                UpdaterEvent::UpToDate => {
                    self.updater_status = UpdateStatus::Idle;
                    if matches!(self.updater_dialog, Some(UpdateDialog::Checking)) {
                        self.updater_dialog = Some(UpdateDialog::UpToDate);
                    } else {
                        self.toast_info(tr!("updater_ui.orbit_is_up_to_date"));
                    }
                }
                UpdaterEvent::Failed(error) => {
                    self.updater_status = UpdateStatus::Idle;
                    if matches!(self.updater_dialog, Some(UpdateDialog::Checking)) {
                        self.updater_dialog = Some(UpdateDialog::Failed(error));
                    } else {
                        self.set_error(tr!("updater_ui.failed", error = error));
                    }
                }
                #[cfg(unix)]
                UpdaterEvent::QuitAndInstall => {
                    // The helper has the staged build and is waiting for this
                    // process to finish its normal quit handlers.
                    cx.quit();
                    return;
                }
            }
        }
        // A finished check reports its status before its event, and a staged
        // release names its version; re-read both so the controls stay in
        // step, then carry a check that found a release into the modal.
        self.sync_staged_release(cx);
        if self.updater_status == UpdateStatus::Available
            && matches!(self.updater_dialog, Some(UpdateDialog::Checking))
        {
            self.updater_dialog = Some(UpdateDialog::Available {
                version: self.updater_version.clone().unwrap_or_default(),
                notes: self.updater_notes.clone(),
                from_check: true,
            });
        }
        cx.notify();
    }

    /// Open the modal on the staged release. The download control calls this
    /// instead of installing immediately.
    pub(super) fn open_staged_update_dialog(&mut self, cx: &mut Context<Self>) {
        if self.updater_status != UpdateStatus::Available {
            return;
        }
        self.sync_staged_release(cx);
        self.updater_history_open = false;
        self.updater_dialog = Some(UpdateDialog::Available {
            version: self.updater_version.clone().unwrap_or_default(),
            notes: self.updater_notes.clone(),
            from_check: false,
        });
        self.updater_dialog_focus_pending = true;
        self.reset_updater_pill_animation();
        cx.notify();
    }

    /// Switch the open modal to the feed's Version History. Only meaningful
    /// while a release or the up-to-date state is on screen — the button that
    /// calls it is only drawn there.
    pub(super) fn open_update_history(&mut self, cx: &mut Context<Self>) {
        if self.updater_history.is_empty() || self.updater_dialog.is_none() {
            return;
        }
        self.updater_history_open = true;
        cx.notify();
    }

    /// Return from Version History to the dialog it was opened over. The
    /// dialog is untouched underneath, so Back restores it exactly.
    pub(super) fn close_update_history(&mut self, cx: &mut Context<Self>) {
        if self.updater_history_open {
            self.updater_history_open = false;
            cx.notify();
        }
    }

    /// Close the modal without acting. A staged release stays staged, so the
    /// download control keeps offering it. Focus returns to the composer so
    /// typing continues after the modal closes.
    pub(super) fn dismiss_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.updater_history_open = false;
        if self.updater_dialog.take().is_some() {
            self.input.read(cx).focus(window);
            cx.notify();
        }
    }

    /// Stage the available update; the shell quits when the helper
    /// acknowledges the handoff.
    pub(super) fn install_available_update(&mut self, cx: &mut Context<Self>) {
        if self.updater_status != UpdateStatus::Available {
            return;
        }
        let started = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
            .is_some_and(|updater| updater.install_available_update());
        if started {
            self.updater_status = UpdateStatus::Updating;
            self.updater_dialog = None;
            self.reset_updater_pill_animation();
            self.toast_info(tr!("updater_ui.preparing_the_update"));
            cx.notify();
        }
    }

    /// Run a user-initiated check. `on_check_for_updates` (the ⌘⇧U action) and
    /// the Settings button both land here. The modal reports the search and
    /// its outcome; an already-staged release opens straight away.
    pub(super) fn begin_update_check(&mut self, cx: &mut Context<Self>) {
        if self.updater_status == UpdateStatus::Available {
            self.open_staged_update_dialog(cx);
            return;
        }
        if let Some(updater) = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
        {
            self.updater_dialog = Some(UpdateDialog::Checking);
            self.updater_history_open = false;
            self.updater_dialog_focus_pending = true;
            updater.check_for_updates();
            cx.notify();
        } else {
            self.updater_history_open = false;
            self.updater_dialog = Some(UpdateDialog::Unavailable);
            self.updater_dialog_focus_pending = true;
            cx.notify();
        }
    }

    /// Escape on the update modal dismisses it (the `UpdateDialog` context
    /// outranks the global escape-to-abort binding).
    pub(super) fn on_update_dialog_close(
        &mut self,
        _: &crate::UpdateDialogClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_update_dialog(window, cx);
    }

    pub(super) fn on_check_for_updates(
        &mut self,
        _: &crate::CheckForUpdates,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_update_check(cx);
    }

    /// Persist the automatic-check preference and mirror it for the next frame.
    pub(super) fn set_automatic_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if let Some(updater) = cx
            .try_global::<UpdaterState>()
            .and_then(|state| state.0.as_ref())
        {
            updater.set_automatically_checks_for_updates(enabled);
        }
        self.automatic_updates_enabled = enabled;
        cx.notify();
    }

    /// The sidebar footer's update control, matching the reference app's pill:
    /// hidden while idle; a spinner in the same footprint while installing; a
    /// 20px download icon that expands to the "Update" label on hover when a
    /// signed release is staged. Clicking it opens the update modal.
    fn updater_pill_target(&self) -> (f32, f32) {
        if self.updater_button_hovered {
            (UPDATER_PILL_EXPANDED_W, 1.0)
        } else {
            (UPDATER_PILL_COLLAPSED_W, 0.0)
        }
    }

    /// Start (or reverse) the pill's expand animation from the values the last
    /// frame painted, so hovering mid-animation does not snap.
    fn begin_updater_pill_animation(&mut self, cx: &mut Context<Self>) {
        self.updater_button_animation_from_width = self.updater_button_width.get();
        self.updater_button_animation_from_reveal = self.updater_button_label_reveal.get();
        self.updater_button_animation_generation = self
            .updater_button_animation_generation
            .wrapping_add(1)
            .max(1);
        cx.notify();
    }

    fn set_updater_button_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.updater_button_hovered == hovered {
            return;
        }
        self.updater_button_hovered = hovered;
        self.begin_updater_pill_animation(cx);
    }

    /// Drop the hover and park the pill at its collapsed width. A status
    /// change rebuilds the button, so a stale expand would flash.
    fn reset_updater_pill_animation(&mut self) {
        self.updater_button_hovered = false;
        self.updater_button_width.set(UPDATER_PILL_COLLAPSED_W);
        self.updater_button_label_reveal.set(0.0);
        self.updater_button_animation_from_width = UPDATER_PILL_COLLAPSED_W;
        self.updater_button_animation_from_reveal = 0.0;
        self.updater_button_animation_generation = 0;
    }

    pub(super) fn sidebar_updater_button(
        &self,
        theme: Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let status = self.updater_status;
        if status == UpdateStatus::Idle {
            return None;
        }
        if status != UpdateStatus::Available {
            // The install is in flight: a spinner in the pill's footprint,
            // with the label in its tooltip.
            let label = tr!("updater_ui.updating");
            let button = icon_button_frame(div().id("sidebar-update"), &theme, ButtonSize::Compact)
                .rounded_full()
                .raised(theme.action, &theme)
                .cursor_default()
                .tooltip(move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into())
                .child(crate::app::spinner(
                    ElementId::NamedInteger("update-spin".into(), 0),
                    ButtonSize::Compact.icon_size().px(&theme),
                    theme.action_fg,
                    theme,
                ));
            return Some(button.into_any_element());
        }

        let label = tr!("updater_ui.update");
        let button = icon_button_frame(div().id("sidebar-update"), &theme, ButtonSize::Compact)
            .overflow_hidden()
            .rounded_full()
            .relative()
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .text_size(button::label_size(ButtonSize::Compact).px(&theme))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .tooltip({
                let label = label.clone();
                move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into()
            })
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                this.set_updater_button_hovered(*hovering, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.open_staged_update_dialog(cx);
                }),
            );

        let generation = self.updater_button_animation_generation;
        let button = if generation == 0 {
            button
                .w(theme.ui_px(UPDATER_PILL_COLLAPSED_W))
                .child(updater_pill_content(theme, label, 0.0))
                .into_any_element()
        } else {
            let from_width = self.updater_button_animation_from_width;
            let from_reveal = self.updater_button_animation_from_reveal;
            let (target_width, target_reveal) = self.updater_pill_target();
            let width_cell = self.updater_button_width.clone();
            let reveal_cell = self.updater_button_label_reveal.clone();
            button
                .with_animation(
                    ElementId::NamedInteger("sidebar-update-expand".into(), generation),
                    Animation::new(AnimationDuration::Fast.duration())
                        .with_easing(|d| 1.0 - (1.0 - d).powi(3)),
                    move |button, delta| {
                        let width = from_width + (target_width - from_width) * delta;
                        let reveal = from_reveal + (target_reveal - from_reveal) * delta;
                        width_cell.set(width);
                        reveal_cell.set(reveal);
                        button.w(theme.ui_px(width)).child(updater_pill_content(
                            theme,
                            label.clone(),
                            reveal,
                        ))
                    },
                )
                .into_any_element()
        };
        Some(button)
    }

    /// Settings → General rows for the updater, empty when this build cannot
    /// update itself (so debug and bare binaries never show a dead control).
    pub(super) fn updater_section(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.updater_available(cx) {
            return None;
        }
        Some(self.settings_section(
            theme,
            &tr!("updater_ui.updates"),
            vec![
                self.setting_row(
                    theme,
                    &tr!("updater_ui.automatic_updates"),
                    Some(&tr!(
                        "updater_ui.check_for_a_newer_signed_release_once_at_launch_"
                    )),
                    None,
                    Some(self.automatic_updates_toggle(theme, this.clone())),
                ),
                self.setting_row(
                    theme,
                    &tr!("updater_ui.check_for_updates"),
                    Some(&tr!(
                        "updater_ui.verify_a_new_release_now_a_staged_update_downloa"
                    )),
                    None,
                    Some(self.update_action_button(theme, this)),
                ),
            ],
        ))
    }

    /// Settings → About's update row: the General control with a description
    /// that names the release once one is staged, so the app-menu About page
    /// can check for and install an update. `None` when this build cannot
    /// update itself (matching [`Self::updater_section`]).
    pub(super) fn about_update_row(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.updater_available(cx) {
            return None;
        }
        let desc = match (self.updater_status, self.updater_version.as_deref()) {
            (UpdateStatus::Available, Some(version)) => {
                tr!("updater_ui.ready_with_version", version = version)
            }
            (UpdateStatus::Available, None) => tr!("updater_ui.ready"),
            (UpdateStatus::Updating, _) => tr!("updater_ui.installing"),
            (UpdateStatus::Idle, _) => tr!("updater_ui.check_hint"),
        };
        Some(self.setting_row(
            theme,
            &tr!("updater_ui.updates"),
            Some(&desc),
            None,
            Some(self.update_action_button(theme, this)),
        ))
    }

    /// Settings → General toggle for scheduled checks.
    pub(super) fn automatic_updates_toggle(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let on = self.automatic_updates_enabled;
        div()
            .id("settings-automatic-updates")
            .w(px(36.))
            .h(px(20.))
            .rounded_full()
            .p(px(2.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(on, |track| track.bg(theme.accent).justify_end())
            .when(!on, |track| {
                track.bg(theme.surface_elevated).justify_start()
            })
            .hover(|track| track.border_color(theme.border_strong))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| {
                    let next = !app.automatic_updates_enabled;
                    app.set_automatic_updates(next, cx);
                });
            })
            .child(div().size(px(14.)).rounded_full().bg(theme.toggle_knob))
            .into_any_element()
    }

    /// The settings update control shared by General and About: a check
    /// button at rest, an icon-only download button once a release is ready
    /// (it opens the update modal), and a quiet label while the install helper
    /// owns the swap.
    pub(super) fn update_action_button(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        match self.updater_status {
            UpdateStatus::Available => {
                let label = tr!("updater_ui.update");
                icon_button_frame(
                    div().id("settings-update-action"),
                    &theme,
                    ButtonSize::Medium,
                )
                .group(BUTTON_GROUP)
                .raised(theme.action, &theme)
                .cursor_pointer()
                .hover(|s| s.raised(theme.action_hover, &theme))
                .active(|s| s.raised(theme.action_pressed, &theme))
                .tooltip(move |_, cx| cx.new(|_| Tooltip::new(label.clone())).into())
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |app, cx| app.open_staged_update_dialog(cx));
                })
                .child(icon(
                    "icons/arrow-down.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.action_fg,
                ))
                .into_any_element()
            }
            UpdateStatus::Updating => button_frame(
                div().id("settings-update-action"),
                &theme,
                ButtonSize::Medium,
            )
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_elevated)
            .text_color(theme.text_muted)
            .cursor_default()
            .child(tr!("updater_ui.updating"))
            .into_any_element(),
            UpdateStatus::Idle => button_frame(
                div().id("settings-update-action"),
                &theme,
                ButtonSize::Medium,
            )
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(|s| s.raised(theme.surface_hover, &theme))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| app.begin_update_check(cx));
            })
            .child(tr!("updater_ui.check_for_updates"))
            .into_any_element(),
        }
    }

    /// The update modal: a scrim and a centered card shared by every update
    /// path. `None` when closed.
    pub(super) fn updater_dialog_layer(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.updater_dialog.as_ref()?;
        let theme = *theme::get(cx);
        let can_install = self.updater_can_install(cx);
        let (card_width, card_max_height) = update_dialog_card_size(window.viewport_size());

        let mut title = match dialog {
            UpdateDialog::Checking => tr!("updater_ui.checking_for_updates"),
            UpdateDialog::Available { .. } => tr!("updater_ui.update_available"),
            UpdateDialog::UpToDate => tr!("updater_ui.up_to_date_title"),
            UpdateDialog::Failed(_) => tr!("updater_ui.failed_title"),
            UpdateDialog::Unavailable => tr!("updater_ui.update_unavailable"),
        };
        if self.updater_history_open {
            title = tr!("updater_ui.version_history");
        }

        let mut card = div()
            .id("update-dialog-card")
            .w(card_width)
            .max_h(card_max_height)
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            // Clicks inside the card must not reach the scrim's dismiss.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex_none()
                    .text_size(modal::HEADLINE.px(&theme))
                    .line_height(modal::HEADLINE.line_height(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title),
            );

        let mut body: AnyElement = match dialog {
            UpdateDialog::Checking => update_dialog_body(
                theme,
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("updater_ui.searching_for_new_version")),
                    )
                    .child(update_progress_bar(theme))
                    .into_any_element(),
            ),
            UpdateDialog::Available { version, notes, .. } => {
                let intro = if version.is_empty() {
                    tr!("updater_ui.a_signed_release_is_ready")
                } else {
                    tr!("updater_ui.version_available", version = version.clone())
                };
                let notes: AnyElement = match notes.as_deref() {
                    Some(notes) if !notes.trim().is_empty() => release_notes_view(notes, theme),
                    _ => div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("updater_ui.no_release_notes"))
                        .into_any_element(),
                };
                let mut column = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .whitespace_normal()
                            .text_size(TextSize::Default.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(intro),
                    );
                if !can_install {
                    column = column.child(
                        div()
                            .whitespace_normal()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!("updater_ui.check_only")),
                    );
                }
                update_dialog_body(
                    theme,
                    column
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_muted)
                                .child(tr!("updater_ui.whats_new")),
                        )
                        .child(
                            div()
                                .id("update-notes")
                                .max_h(px(280.))
                                .overflow_y_scroll()
                                .child(notes),
                        )
                        .into_any_element(),
                )
            }
            UpdateDialog::UpToDate => update_dialog_body(
                theme,
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(TextSize::Default.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!(
                        "updater_ui.up_to_date_detail",
                        version = env!("CARGO_PKG_VERSION")
                    ))
                    .into_any_element(),
            ),
            UpdateDialog::Failed(error) => update_dialog_body(
                theme,
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(TextSize::Default.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(error.clone())
                    .into_any_element(),
            ),
            UpdateDialog::Unavailable => update_dialog_body(
                theme,
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(TextSize::Default.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!("updater_ui.update_unavailable_detail"))
                    .into_any_element(),
            ),
        };
        if self.updater_history_open {
            body = version_history_view(&self.updater_history, env!("CARGO_PKG_VERSION"), theme);
        }
        card = card.child(body);

        let secondary_label = if self.updater_history_open {
            tr!("view.back")
        } else {
            match dialog {
                UpdateDialog::Checking => tr!("view.cancel"),
                UpdateDialog::Available {
                    from_check: true, ..
                } if can_install => tr!("view.cancel"),
                UpdateDialog::Available {
                    from_check: false, ..
                } if can_install => tr!("updater_ui.later"),
                _ => tr!("settings.close"),
            }
        };
        let mut footer = div()
            .px(modal::footer_padding(&theme))
            .py(modal::footer_padding(&theme))
            .flex_none()
            .flex()
            .justify_end()
            .gap(modal::footer_gap(&theme));
        if !self.updater_history_open
            && !self.updater_history.is_empty()
            && matches!(
                dialog,
                UpdateDialog::Available { .. } | UpdateDialog::UpToDate
            )
        {
            footer = footer.child(dialog_button(
                "update-dialog-history",
                theme,
                false,
                tr!("updater_ui.version_history"),
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.open_update_history(cx)),
            ));
        }
        footer = footer.child(dialog_button(
            "update-dialog-secondary",
            theme,
            false,
            secondary_label,
            cx.listener(|this, _: &MouseUpEvent, window, cx| {
                if this.updater_history_open {
                    this.close_update_history(cx);
                } else {
                    this.dismiss_update_dialog(window, cx);
                }
            }),
        ));
        if !self.updater_history_open
            && can_install
            && matches!(dialog, UpdateDialog::Available { .. })
        {
            footer = footer.child(dialog_button(
                "update-dialog-primary",
                theme,
                true,
                tr!("updater_ui.update_now"),
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.install_available_update(cx)),
            ));
        }
        card = card.child(footer);

        // ── scrim: dimmed backdrop; a click outside dismisses the modal ──
        let scrim = theme.scrim_modal();
        Some(
            div()
                .id("update-dialog-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .px(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .key_context("UpdateDialog")
                .track_focus(&self.updater_dialog_focus)
                .on_action(cx.listener(Self::on_update_dialog_close))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.dismiss_update_dialog(window, cx)
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

/// A block of release notes parsed from the feed's plain-text description.
#[derive(Debug, PartialEq, Eq)]
enum NoteBlock {
    Heading(String),
    Bullet(String),
    Paragraph(String),
}

/// Turn the feed's plain-text notes into renderable blocks: headings (`#`),
/// bullets (`- `/`* `), and paragraphs. Wrapped continuation lines are joined
/// back onto the block they belong to.
fn parse_release_notes(notes: &str) -> Vec<NoteBlock> {
    let mut blocks = Vec::new();
    // (is_bullet, accumulated text) for the block being built.
    let mut current: Option<(bool, String)> = None;
    let flush = |blocks: &mut Vec<NoteBlock>, current: &mut Option<(bool, String)>| {
        if let Some((bullet, text)) = current.take() {
            let text = text.trim().to_owned();
            if !text.is_empty() {
                blocks.push(if bullet {
                    NoteBlock::Bullet(text)
                } else {
                    NoteBlock::Paragraph(text)
                });
            }
        }
    };
    for raw in notes.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush(&mut blocks, &mut current);
            continue;
        }
        if let Some(heading) = line.strip_prefix('#') {
            flush(&mut blocks, &mut current);
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                blocks.push(NoteBlock::Heading(heading.to_owned()));
            }
            continue;
        }
        if let Some(bullet) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            flush(&mut blocks, &mut current);
            current = Some((true, bullet.to_owned()));
            continue;
        }
        match current.as_mut() {
            Some((_, text)) => {
                text.push(' ');
                text.push_str(line);
            }
            None => current = Some((false, line.to_owned())),
        }
    }
    flush(&mut blocks, &mut current);
    blocks
}

/// Drop the light inline Markdown the changelog uses, so the plain-text notes
/// read cleanly: code ticks and bold markers, and links collapsed to their
/// label (`[#11](https://…)` → `#11`).
fn inline_notes_text(text: &str) -> String {
    let text = text.replace('`', "").replace("**", "");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.split_once(']') {
            Some((label, tail)) if tail.starts_with('(') => match tail.find(')') {
                Some(close) => {
                    out.push_str(label);
                    rest = &tail[close + 1..];
                }
                None => {
                    out.push('[');
                    rest = after;
                }
            },
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The changelog column in the update modal.
fn release_notes_view(notes: &str, theme: Theme) -> AnyElement {
    let mut column = div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme));
    for block in parse_release_notes(notes) {
        column = column.child(match block {
            NoteBlock::Heading(text) => div()
                .pt(DynamicSpacing::Base06.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(inline_notes_text(&text))
                .into_any_element(),
            NoteBlock::Bullet(text) => div()
                .flex()
                .items_start()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .mt(DynamicSpacing::Base06.px(&theme))
                        .size(px(4.))
                        .flex_none()
                        .rounded_full()
                        .bg(theme.text_muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(inline_notes_text(&text)),
                )
                .into_any_element(),
            NoteBlock::Paragraph(text) => div()
                .whitespace_normal()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_secondary)
                .child(inline_notes_text(&text))
                .into_any_element(),
        });
    }
    column.into_any_element()
}

/// The modal's Version History: every signed release the feed carries,
/// newest first, with the running build marked and each release's notes
/// rendered like the update body's changelog.
fn version_history_view(
    history: &[crate::updater::Release],
    current: &str,
    theme: Theme,
) -> AnyElement {
    let mut entries = div().flex().flex_col().gap(px(14.));
    for release in history {
        let mut heading = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(format!("v{}", release.version)),
            );
        if release.version == current {
            heading = heading.child(
                div()
                    .px(DynamicSpacing::Base06.px(&theme))
                    .py(px(1.))
                    .rounded_full()
                    .bg(theme.surface_hover)
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(tr!("updater_ui.current")),
            );
        }
        let notes: AnyElement = match release.notes.as_deref() {
            Some(notes) if !notes.trim().is_empty() => release_notes_view(notes, theme),
            _ => div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("updater_ui.no_release_notes"))
                .into_any_element(),
        };
        entries = entries.child(
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base06.px(&theme))
                .child(heading)
                .child(notes),
        );
    }
    div()
        .id("update-history")
        .max_h(px(360.))
        .overflow_y_scroll()
        .px(px(18.))
        .pb(DynamicSpacing::Base16.px(&theme))
        .child(entries)
        .into_any_element()
}

/// The modal's one button shape, tinted primary (Update now) or quiet
/// (Later / Cancel / Close).
fn dialog_button(
    id: &'static str,
    theme: Theme,
    primary: bool,
    label: String,
    on_mouse_up: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let button = button_frame(div().id(id), &theme, ButtonSize::Large)
        .group(BUTTON_GROUP)
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .on_mouse_up(MouseButton::Left, on_mouse_up);
    let button = if primary {
        button
            .raised(theme.action, &theme)
            .text_color(theme.action_fg)
            .hover(|s| s.raised(theme.action_hover, &theme))
            .active(|s| s.raised(theme.action_pressed, &theme))
    } else {
        button
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .text_color(theme.text_secondary)
            .hover(|s| s.raised(theme.surface_hover, &theme))
    };
    press(button).child(label).into_any_element()
}

/// The sidebar pill's two layers: the download icon and the "Update" label,
/// cross-faded by `reveal` while the pill expands.
fn updater_pill_content(theme: Theme, label: String, reveal: f32) -> impl IntoElement {
    div()
        .relative()
        .size_full()
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(1.0 - reveal)
                .child(icon(
                    "icons/arrow-down.svg",
                    IconSize::XSmall.px(&theme),
                    theme.background,
                )),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .opacity(reveal)
                .child(label),
        )
}

/// The modal card's definite size for a viewport: never wider than 520 px or
/// taller than the window minus the scrim's 24 px margins. The width must be
/// definite — with `w_full().max_w()` the body's text is measured against the
/// viewport before the clamp, under-reporting the card's height and pushing
/// the footer out of the clipped card. The height cap keeps the footer on
/// screen in a short window; the body scrolls when it hits.
fn update_dialog_card_size(viewport: gpui::Size<gpui::Pixels>) -> (gpui::Pixels, gpui::Pixels) {
    (
        (viewport.width - px(48.)).min(px(520.)),
        viewport.height - px(48.),
    )
}

/// The body of an update dialog: the app icon at the leading edge, then the
/// state's content. Shared by every state, so the icon lands in the same
/// place whichever dialog is open.
fn update_dialog_body(theme: Theme, content: AnyElement) -> AnyElement {
    div()
        .id("update-dialog-body")
        .debug_selector(|| "update-dialog-body".to_string())
        .px(modal::section_padding_x(&theme, false))
        .pb(modal::section_padding_bottom(&theme))
        .flex()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .items_start()
        .gap(DynamicSpacing::Base12.px(&theme))
        .child(embedded_image(
            crate::app_icon::ASSET,
            f32::from(IconSize::XLarge.px(&theme)),
        ))
        .child(content)
        .into_any_element()
}

/// A thin indeterminate bar, for the modal's search state. The moving segment
/// is a fraction of the track, so the sweep scales with the card.
fn update_progress_bar(theme: Theme) -> impl IntoElement {
    const SEGMENT: f32 = 0.28;
    div()
        .relative()
        .w_full()
        .h(px(3.))
        .rounded_full()
        .bg(theme.surface_hover)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left(gpui::relative(-SEGMENT))
                .h_full()
                .w(gpui::relative(SEGMENT))
                .rounded_full()
                .bg(theme.accent)
                .with_animation(
                    ElementId::Name("update-progress".into()),
                    Animation::new(Duration::from_millis(1100)).repeat(),
                    move |bar, delta| {
                        // Travel from just off the left edge to just off the right.
                        bar.left(gpui::relative(-SEGMENT + (1.0 + SEGMENT) * delta))
                    },
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_split_into_headings_bullets_and_paragraphs() {
        let notes = "Intro line.\n\n### Added\n\n- One wrapped\n  onto a second line\n- Two\n";
        assert_eq!(
            parse_release_notes(notes),
            vec![
                NoteBlock::Paragraph("Intro line.".into()),
                NoteBlock::Heading("Added".into()),
                NoteBlock::Bullet("One wrapped onto a second line".into()),
                NoteBlock::Bullet("Two".into()),
            ]
        );
    }

    #[test]
    fn inline_notes_text_drops_code_ticks_and_bold() {
        assert_eq!(
            inline_notes_text("the `updater` is **ready**"),
            "the updater is ready"
        );
    }

    #[test]
    fn inline_notes_text_collapses_links_to_their_label() {
        assert_eq!(
            inline_notes_text(
                "Dumitru Moloşnic ([#11](https://github.com/imrj05/orbit/pull/11)) — fixes"
            ),
            "Dumitru Moloşnic (#11) — fixes"
        );
        // An unmatched bracket is left alone rather than eaten.
        assert_eq!(inline_notes_text("an [open bracket"), "an [open bracket");
    }

    #[test]
    fn up_to_date_copy_names_the_version_it_checked() {
        let text = tr!("updater_ui.up_to_date_detail", version = "9.9.9");
        assert!(text.contains("9.9.9"), "{text}");
    }

    /// The modal's body gives its copy a constrained, shrinkable column, so
    /// long lines wrap inside the card padding instead of spilling past it.
    #[gpui::test]
    fn update_dialog_body_wraps_long_copy_inside_the_card(cx: &mut gpui::TestAppContext) {
        struct Probe;
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("probe-update-card")
                    .debug_selector(|| "probe-update-card".to_string())
                    .w(px(520.))
                    .child(update_dialog_body(
                        Theme::dark(),
                        div()
                            .id("probe-update-copy")
                            .debug_selector(|| "probe-update-copy".to_string())
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(
                                "This build can't check for updates on its own. \
                                 Installed releases check automatically.",
                            )
                            .into_any_element(),
                    ))
            }
        }

        let cx = cx.add_empty_window();
        let _ = cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(400.)),
            |_, cx| cx.new(|_| Probe),
        );
        let card = cx.debug_bounds("probe-update-card").expect("card laid out");
        let copy = cx.debug_bounds("probe-update-copy").expect("copy laid out");
        assert_eq!(
            card.size.width,
            px(520.),
            "the probe stands in for the modal card"
        );
        assert!(
            copy.right() <= card.right() - modal::section_padding_x(&Theme::dark(), false),
            "long copy must wrap inside the card padding: copy {copy:?}, card {card:?}"
        );
    }

    /// The modal's footer must be laid out *inside* the card: the card clips
    /// with `overflow_hidden`, so a body that reports a full-height content
    /// box would push the footer past the card's bottom edge and hide it.
    #[gpui::test]
    fn update_dialog_card_keeps_its_footer_inside_the_card(cx: &mut gpui::TestAppContext) {
        struct Probe;
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let theme = Theme::dark();
                let notes = "### Contributors\n\n- **Dumitru Moloșnic** ([#11](https://github.com/imrj05/orbit/pull/11)) — light,\n  dark, and system appearance modes; transcript table sizing, streaming\n  scroll-position, and multiline command-preview fixes.";
                let (card_width, card_max_height) =
                    update_dialog_card_size(gpui::size(px(800.), px(600.)));
                div()
                    .id("probe-update-scrim")
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("probe-update-card")
                            .debug_selector(|| "probe-update-card".to_string())
                            .w(card_width)
                            .max_h(card_max_height)
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(div().flex_none().p(px(12.)).child("Update available"))
                            .child(update_dialog_body(
                                theme,
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child("Orbit Pi v0.0.11 is available.")
                                    .child(
                                        div()
                                            .text_size(TextSize::Small.px(&theme))
                                            .child("What's new"),
                                    )
                                    .child(
                                        div()
                                            .id("probe-update-notes")
                                            .debug_selector(|| "probe-update-notes".to_string())
                                            .max_h(px(280.))
                                            .child(release_notes_view(notes, theme)),
                                    )
                                    .into_any_element(),
                            ))
                            .child(
                                div()
                                    .id("probe-update-footer")
                                    .debug_selector(|| "probe-update-footer".to_string())
                                    .h(px(60.))
                                    .flex_none()
                                    .child("Close"),
                            ),
                    )
            }
        }

        let cx = cx.add_empty_window();
        let _ = cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            |_, cx| cx.new(|_| Probe),
        );
        let card = cx.debug_bounds("probe-update-card").expect("card laid out");
        let footer = cx
            .debug_bounds("probe-update-footer")
            .expect("footer laid out");
        eprintln!(
            "card={card:?} body={:?} footer={footer:?} notes={:?}",
            cx.debug_bounds("update-dialog-body"),
            cx.debug_bounds("probe-update-notes"),
        );
        assert!(
            footer.bottom() <= card.bottom(),
            "the footer must stay inside the clipped card: footer {footer:?}, card {card:?}"
        );
    }
}
