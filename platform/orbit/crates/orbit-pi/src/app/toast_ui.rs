use super::helpers::*;
use super::*;
use crate::theme::tokens::{toast as toast_tokens, IconSize};
use crate::toast::{Toast, ToastKind};

fn short_revision(revision: &str) -> &str {
    if revision.len() > 12 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        &revision[..7]
    } else {
        revision
    }
}

impl OrbitApp {
    /// Queue an in-app toast. A repeat of a card still on screen refreshes it
    /// instead of stacking a twin. The caller owns the repaint — push from a
    /// path that ends in `cx.notify()`.
    pub(super) fn push_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<String>,
        body: Option<String>,
    ) {
        self.toasts.push(kind, title, body);
    }

    /// A one-line confirmation — the common case.
    pub(super) fn toast_success(&mut self, message: impl Into<String>) {
        self.push_toast(ToastKind::Success, message, None);
    }

    pub(super) fn toast_info(&mut self, message: impl Into<String>) {
        self.push_toast(ToastKind::Info, message, None);
    }

    pub(super) fn toast_warning(&mut self, message: impl Into<String>) {
        self.push_toast(ToastKind::Warning, message, None);
    }

    pub(super) fn toast_error(&mut self, message: impl Into<String>) {
        self.push_toast(ToastKind::Error, message, None);
    }

    /// Dismiss one card: the click handler for the whole card (and its ×).
    pub(super) fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.toasts.dismiss(id) {
            cx.notify();
        }
    }

    /// The toast stack: bottom-right, newest nearest the corner, oldest at
    /// the top. It sits above every surface, so a fact that lands while
    /// Settings / Git / Usage owns the main area is still announced.
    pub(super) fn toast_layer(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.toasts.is_empty() && self.plugin_update_prompt.is_none() {
            return None;
        }
        let theme = *theme::get(cx);
        let reduce_motion = theme.ui.reduce_motion;
        Some(
            div()
                .debug_selector(|| "toast-stack".to_string())
                .absolute()
                .bottom(toast_tokens::stack_inset(&theme))
                .right(toast_tokens::stack_inset(&theme))
                .flex()
                .flex_col()
                .items_end()
                .gap(toast_tokens::stack_gap(&theme))
                // The container is not itself a hitbox: only the cards below
                // take clicks, so the area around them stays click-through.
                .children(
                    self.toasts
                        .items()
                        .iter()
                        .map(|toast| Self::toast_card(toast, theme, reduce_motion, cx)),
                )
                .children(self.plugin_update_prompt.as_ref().map(|updates| {
                    self.plugin_update_prompt_card(updates, theme, reduce_motion, cx)
                }))
                .into_any_element(),
        )
    }

    /// The update notice stays visible until the user chooses Update now,
    /// Skip, or closes it; ordinary toasts retain their short TTL.
    fn plugin_update_prompt_card(
        &self,
        updates: &[crate::plugins::PluginUpdate],
        theme: Theme,
        reduce_motion: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let title = tr!("updater_ui.update_available");
        let mut package_rows: Vec<AnyElement> = Vec::new();
        for (index, update) in updates.iter().take(3).enumerate() {
            if index > 0 {
                package_rows.push(
                    div()
                        .w_full()
                        .h(toast_tokens::separator_size())
                        .bg(theme.border)
                        .into_any_element(),
                );
            }
            package_rows.push(
                div()
                    .w_full()
                    .min_w_0()
                    .py(toast_tokens::package_row_padding_y(&theme))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(toast_tokens::package_row_gap(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(toast_tokens::BODY.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(update.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(toast_tokens::version_gap(&theme))
                            .child(
                                div()
                                    .font_family(theme::code_font_family())
                                    .text_size(toast_tokens::version_text_size(&theme))
                                    .text_color(theme.text_muted)
                                    .child(short_revision(&update.current).to_string()),
                            )
                            .child(icon(
                                "icons/arrow-right.svg",
                                IconSize::XSmall.px(&theme),
                                theme.text_muted,
                            ))
                            .child(
                                div()
                                    .font_family(theme::code_font_family())
                                    .text_size(toast_tokens::version_text_size(&theme))
                                    .text_color(theme.text_primary)
                                    .child(short_revision(&update.latest).to_string()),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let close_button = press(icon_button_frame(
            div()
                .id(ElementId::Name("plugin-update-close".into()))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.skip_plugin_updates(cx)),
                ),
            &theme,
            toast_tokens::CLOSE_BUTTON,
        ))
        .child(icon(
            "icons/x.svg",
            toast_tokens::CLOSE_BUTTON.icon_size().px(&theme),
            theme.text_muted,
        ));

        let this = cx.entity();
        let skip_button = press(button_frame(
            div().id(ElementId::Name("plugin-update-skip".into())),
            &theme,
            toast_tokens::ACTION_BUTTON,
        ))
        .cursor_pointer()
        .text_color(theme.text_muted)
        .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| app.skip_plugin_updates(cx));
        })
        .child(tr!("git_panel.recover_skip"));

        let this = cx.entity();
        let update_button = press(button_frame(
            div().id(ElementId::Name("plugin-update-now".into())),
            &theme,
            toast_tokens::ACTION_BUTTON,
        ))
        .cursor_pointer()
        .raised(theme.action, &theme)
        .text_color(theme.action_fg)
        .hover(|style| style.raised(theme.action_hover, &theme))
        .active(|style| style.raised(theme.action_pressed, &theme))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| app.apply_plugin_updates(cx));
        })
        .child(icon(
            "icons/refresh.svg",
            IconSize::XSmall.px(&theme),
            theme.action_fg,
        ))
        .child(tr!("updater_ui.update_now"));

        let card = toast_tokens::surface(
            div()
                .id(ElementId::Name("plugin-update-toast".into()))
                .debug_selector(|| "plugin-update-toast".to_string()),
            &theme,
        )
        .w(toast_tokens::WIDTH)
        .px(toast_tokens::padding_x(&theme))
        .py(toast_tokens::padding_y(&theme))
        .flex()
        .items_start()
        .gap(toast_tokens::content_gap(&theme))
        .child(
            div()
                .flex_none()
                .size(toast_tokens::icon_tile_size(&theme))
                .rounded(toast_tokens::ICON_RADIUS.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.accent.opacity(0.12))
                .child(icon(
                    "icons/extensions.svg",
                    toast_tokens::ICON.px(&theme),
                    theme.accent_text,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(toast_tokens::content_gap(&theme))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(toast_tokens::header_gap(&theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(toast_tokens::TITLE.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .min_w(toast_tokens::badge_min_width(&theme))
                                .h(toast_tokens::badge_height(&theme))
                                .px(toast_tokens::badge_padding_x(&theme))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(toast_tokens::BADGE_RADIUS.px(&theme))
                                .bg(theme.overlay)
                                .text_size(toast_tokens::BODY.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(updates.len().to_string()),
                        )
                        .child(close_button),
                )
                .child(div().w_full().flex().flex_col().children(package_rows))
                .child(
                    div()
                        .w_full()
                        .h(toast_tokens::separator_size())
                        .bg(theme.border),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(toast_tokens::action_gap(&theme))
                        .child(skip_button)
                        .child(update_button),
                ),
        );

        if reduce_motion {
            card.into_any_element()
        } else {
            card.with_animation(
                ElementId::Name("plugin-update-toast-enter".into()),
                Animation::new(toast_tokens::ENTER),
                |card, progress| card.opacity(progress),
            )
            .into_any_element()
        }
    }

    /// One toast card: kind glyph, title (+ optional body), and a ×. Clicking
    /// anywhere dismisses it before its TTL lapses.
    fn toast_card(
        toast: &Toast,
        theme: Theme,
        reduce_motion: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (glyph, tint) = match toast.kind {
            ToastKind::Info => ("icons/info.svg", theme.info),
            ToastKind::Success => ("icons/check.svg", theme.success),
            ToastKind::Warning => ("icons/info.svg", theme.warning),
            ToastKind::Error => ("icons/stop.svg", theme.error),
        };
        let id = toast.id;
        let close_button = press(icon_button_frame(
            div()
                .id(ElementId::Name(format!("toast-close-{id}").into()))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover)),
            &theme,
            toast_tokens::CLOSE_BUTTON,
        ))
        .child(icon(
            "icons/x.svg",
            toast_tokens::CLOSE_BUTTON.icon_size().px(&theme),
            theme.text_muted,
        ));
        let card = toast_tokens::surface(
            div()
                .id(ElementId::Name(
                    format!("toast-{id}-{}", toast.revision).into(),
                ))
                .debug_selector(move || format!("toast-{id}")),
            &theme,
        )
        .w(toast_tokens::WIDTH)
        .px(toast_tokens::padding_x(&theme))
        .py(toast_tokens::padding_y(&theme))
        .flex()
        .items_start()
        .gap(toast_tokens::content_gap(&theme))
        .cursor_pointer()
        .hover(|style| style.border_color(theme.border_strong))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.dismiss_toast(id, cx)),
        )
        .child(
            div()
                .flex_none()
                .size(toast_tokens::icon_tile_size(&theme))
                .rounded(toast_tokens::ICON_RADIUS.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .bg(tint.opacity(0.14))
                .child(icon(glyph, toast_tokens::ICON.px(&theme), tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(toast_tokens::title_body_gap(&theme))
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(toast_tokens::TITLE.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(toast.title.clone()),
                )
                .children(toast.body.as_ref().map(|body| {
                    div()
                        .whitespace_normal()
                        .line_clamp(3)
                        .text_size(toast_tokens::BODY.px(&theme))
                        .text_color(theme.text_muted)
                        .child(body.clone())
                })),
        )
        .child(close_button);

        // The animation spans the card's whole life: fade in, hold through
        // the TTL, then fade out so the eventual removal is never a snap.
        // Reduce-motion keeps the card static (and `tick` expires it at the
        // bare TTL, with no fade grace).
        if reduce_motion {
            return card.into_any_element();
        }
        let ttl = toast.kind.ttl().as_secs_f32();
        let fade = crate::toast::FADE.as_secs_f32();
        let enter = toast_tokens::ENTER.as_secs_f32();
        let total = ttl + fade;
        card.with_animation(
            ElementId::Name(format!("toast-life-{id}-{}", toast.revision).into()),
            Animation::new(Duration::from_secs_f32(total)),
            move |card, delta| {
                let elapsed = delta * total;
                let opacity = if elapsed < enter {
                    elapsed / enter
                } else if elapsed > ttl {
                    ((total - elapsed) / fade).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                card.opacity(opacity)
            },
        )
        .into_any_element()
    }
}
