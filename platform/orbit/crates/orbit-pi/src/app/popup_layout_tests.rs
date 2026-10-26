use super::*;

// ── settings select popup anchoring ───────────────────────────────────

/// A settings select control: a 26 px chip with its popup anchored to a
/// zero-size point at the chip's top-right, exactly like `select_control`.
/// `legacy` reproduces the old `Local` + snap anchoring; the fixed code
/// uses `Window` mode so `anchored` can flip the popup when it would
/// overflow the viewport.
struct SettingsSelectAnchorProbe {
    legacy: bool,
    /// Place the chip near the viewport bottom instead of the top.
    near_bottom: bool,
}

impl Render for SettingsSelectAnchorProbe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let gap = if self.near_bottom {
            vp.height - px(80.)
        } else {
            px(120.)
        };
        let chip = div()
            .id("anchor-chip")
            .debug_selector(|| "anchor-chip".to_string())
            .h(px(26.))
            .w(px(120.))
            .bg(gpui::black());
        let popup = div()
            .id("anchor-popup")
            .debug_selector(|| "anchor-popup".to_string())
            .w(px(360.))
            .h(px(240.))
            .bg(gpui::black());
        let anchored_popup = if self.legacy {
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .anchor(Corner::BottomRight)
                .offset(point(px(0.), px(-4.)))
                .snap_to_window()
                .child(deferred(popup))
                .into_any_element()
        } else {
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .anchor(Corner::TopRight)
                .offset(point(px(0.), px(30.)))
                .child(deferred(popup))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .items_end()
            .w(px(600.))
            .child(div().h(gap).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .size(px(0.))
                            .child(anchored_popup),
                    )
                    .child(chip),
            )
    }
}

/// The old `Local` + `snap_to_window` anchor cannot flip: with no room
/// above the chip, the snap slides the popup down over the chip itself,
/// so a second click on the chip lands inside the popup instead of
/// toggling it closed.
#[gpui::test]
fn local_snap_anchor_covers_the_chip_near_the_viewport_top(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(600.), px(800.)),
        |_, cx| {
            cx.new(|_| SettingsSelectAnchorProbe {
                legacy: true,
                near_bottom: false,
            })
        },
    );
    let chip = cx.debug_bounds("anchor-chip").expect("chip laid out");
    let popup = cx.debug_bounds("anchor-popup").expect("popup laid out");
    assert!(
        popup.top() < chip.bottom() && popup.bottom() > chip.top(),
        "expected the snapped popup to cover the chip: chip {chip:?}, popup {popup:?}"
    );
}

/// The fixed anchor drops the popup below the chip, right-aligned, with a
/// 4 px gap — the platform's combobox direction and the pattern every
/// other dropdown in the app follows.
#[gpui::test]
fn settings_select_popup_opens_below_its_chip(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(600.), px(800.)),
        |_, cx| {
            cx.new(|_| SettingsSelectAnchorProbe {
                legacy: false,
                near_bottom: false,
            })
        },
    );
    let chip = cx.debug_bounds("anchor-chip").expect("chip laid out");
    let popup = cx.debug_bounds("anchor-popup").expect("popup laid out");
    assert_eq!(
        popup.right(),
        chip.right(),
        "popup right-aligns to the chip"
    );
    assert_eq!(
        popup.top() - chip.bottom(),
        px(4.),
        "4 px gap below the chip"
    );
}

/// Near the viewport bottom the popup flips above the chip rather than
/// snapping over it; it sits flush so the chip stays clickable.
#[gpui::test]
fn settings_select_popup_flips_above_near_the_viewport_bottom(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(600.), px(800.)),
        |_, cx| {
            cx.new(|_| SettingsSelectAnchorProbe {
                legacy: false,
                near_bottom: true,
            })
        },
    );
    let chip = cx.debug_bounds("anchor-chip").expect("chip laid out");
    let popup = cx.debug_bounds("anchor-popup").expect("popup laid out");
    assert_eq!(
        popup.bottom(),
        chip.top(),
        "popup sits flush above the chip"
    );
    assert!(popup.top() >= px(0.), "flipped popup stays on screen");
}

/// The settings popup's list: a `uniform_list` with an explicit height
/// (30 px row stride + 8 px vertical padding, capped at 220 px), inside
/// the popup's flex column under a 34 px search field.
struct SettingsPopupListTestView(gpui::UniformListScrollHandle);

impl Render for SettingsPopupListTestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = 100;
        let list_h = (rows as f32 * 30. + 8.).min(220.);
        div().flex().flex_col().child(div().h(px(34.))).child(
            uniform_list("settings-select-list", rows, |range, _, _| {
                range
                    .map(|ix| {
                        div()
                            .h(px(30.))
                            .w_full()
                            .child(format!("row {ix}"))
                            .into_any_element()
                    })
                    .collect()
            })
            .track_scroll(self.0.clone())
            .w_full()
            .h(px(list_h))
            .px(px(4.))
            .py(px(4.)),
        )
    }
}

/// The settings popup lives inside `anchored` inside a 0×0 div, so its
/// available height is 0. `uniform_list`'s Infer sizing reads the
/// available height and collapses to nothing there — the explicit height
/// is what keeps the list visible. Regression test for the empty
/// font/theme dropdowns.
#[gpui::test]
fn settings_popup_list_keeps_a_viewport_in_zero_height_context(cx: &mut gpui::TestAppContext) {
    use gpui::size;

    let cx = cx.add_empty_window();
    let scroll = gpui::UniformListScrollHandle::new();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(200.), px(0.)), |_, cx| {
        cx.new(|_| SettingsPopupListTestView(scroll.clone()))
    });
    let state = scroll.0.borrow();
    let size = state.last_item_size.expect("list was laid out");
    assert!(
        size.item.height > px(0.),
        "settings popup list collapsed to a 0-height viewport"
    );
}

/// The same list with `max_h` instead of an explicit height collapses to
/// 0 — this is the gpui behavior the explicit height works around.
#[gpui::test]
fn max_h_collapses_uniform_list_in_zero_height_context(cx: &mut gpui::TestAppContext) {
    use gpui::size;

    let cx = cx.add_empty_window();
    let scroll = gpui::UniformListScrollHandle::new();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(200.), px(0.)), |_, cx| {
        cx.new(|_| MaxHeightListTestView(scroll.clone()))
    });
    let state = scroll.0.borrow();
    let size = state.last_item_size.expect("list was laid out");
    assert_eq!(
        size.item.height,
        px(0.),
        "max_h + Infer sizing collapses to 0 in a 0-height available space"
    );
}

// ── worktrees page "Advanced" dropdown anchoring ──────────────────────

/// The Worktrees page's "Advanced" menu drops below its button, right-aligned,
/// rather than opening upward into the window chrome. The old `Local` +
/// `snap_to_window` anchor could not flip, so in this top toolbar the
/// off-screen "above" position made the snap slide the menu down over its own
/// button.
#[gpui::test]
fn worktree_advanced_menu_opens_below_its_button(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let viewport = cx.update(|window, _| window.viewport_size());
    cx.update(|_, cx| {
        app.update(cx, |app, _cx| {
            for dependency in &mut app.deps {
                dependency.installed = true;
            }
            app.worktrees_open = true;
            app.worktree_advanced_open = true;
        });
    });
    // The page settles over a frame (workspace discovery, then a refresh).
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());

    let button = cx
        .debug_bounds("worktrees-advanced")
        .expect("the advanced button laid out");
    let popup = cx
        .debug_bounds("worktrees-advanced-popup")
        .expect("the advanced menu laid out");

    assert!(
        popup.top() >= button.bottom(),
        "the menu must open below its button: button {button:?}, popup {popup:?}"
    );
    assert_eq!(
        popup.right(),
        button.right(),
        "the menu right-aligns to its button"
    );
}

/// A second click on the "Advanced" button closes the menu. The click's
/// mouse-down already dismissed it (via `on_mouse_down_out`), so without the
/// gesture guard the mouse-up would toggle it straight back open and the menu
/// could never be closed from its own button.
#[gpui::test]
fn worktree_advanced_button_click_toggles_the_menu_closed(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let app = cx.update(|cx| cx.new(OrbitApp::new));

    cx.update(|cx| {
        app.update(cx, |app, cx| {
            // The first click opens it.
            app.toggle_worktree_advanced(cx);
            assert!(app.worktree_advanced_open, "the first click opens the menu");

            // A click outside the menu (the button sits outside the occluded
            // popup) dismisses it on mouse-down and stamps the gesture...
            app.worktree_advanced_open = false;
            app.menu_dismissed_at = Some(std::time::Instant::now());
            // ...so the same click's mouse-up must not reopen it.
            app.toggle_worktree_advanced(cx);
            assert!(
                !app.worktree_advanced_open,
                "the dismissing click must not toggle the menu back open"
            );

            // A later, deliberate click opens it again.
            app.menu_dismissed_at = None;
            app.toggle_worktree_advanced(cx);
            assert!(app.worktree_advanced_open, "a later click reopens the menu");
        });
    });
}

// ── context menus on Zed's tokens ─────────────────────────────────────────

/// A context menu built exactly like the sidebar's: the shared shell and
/// entries, content-sized inside `anchored` + `deferred`, anchored 20px from
/// the viewport's right edge so it has to snap back into the window (the
/// snap reads the window viewport, not the drawn area).
struct ContextMenuProbe;

impl Render for ContextMenuProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::tokens::{context_menu, popover};

        let theme = *theme::get(cx);
        let anchor_x = window.viewport_size().width - px(20.);
        let entry = |id: &'static str, label: &'static str| {
            context_menu_entry(div().debug_selector(move || id.to_string()), &theme)
                .child(div().size(context_menu::ICON.px(&theme)).flex_none())
                .child(label)
        };
        let popup = context_menu_surface(div().debug_selector(|| "menu".into()), &theme)
            .flex()
            .flex_col()
            .child(entry("entry-a", "Copy path"))
            .child(context_menu_separator(&theme).debug_selector(|| "menu-sep".into()))
            .child(entry("entry-b", "Remove"));
        div().size_full().child(
            div()
                .absolute()
                .top(px(40.))
                .left(anchor_x)
                .size(px(0.))
                .child(
                    anchored()
                        .position_mode(AnchoredPositionMode::Local)
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                        .child(deferred(popup)),
                ),
        )
    }
}

#[gpui::test]
fn context_menu_shell_lays_out_on_zeds_metrics(cx: &mut gpui::TestAppContext) {
    use crate::theme::ThemeId;

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(300.), px(400.)),
        |_, cx| cx.new(|_| ContextMenuProbe),
    );
    let viewport = cx.update(|window, _| window.viewport_size());
    let menu = cx.debug_bounds("menu").expect("menu laid out");
    let a = cx.debug_bounds("entry-a").expect("first entry laid out");
    let sep = cx.debug_bounds("menu-sep").expect("separator laid out");
    let b = cx.debug_bounds("entry-b").expect("second entry laid out");

    // Content-sized, never below Zed's 200px minimum.
    assert!(
        menu.size.width >= px(200.),
        "menu width {:?}",
        menu.size.width
    );
    // Snapped back inside the window with Zed's 8px margin.
    assert_eq!(
        menu.right(),
        viewport.width - px(8.),
        "menu {menu:?} in {viewport:?}"
    );
    // Entries: one Comfortable line of 14px text (23px), inset Base04 inside
    // a 1px hairline, below `List`'s Base04 top padding.
    assert_eq!(a.size.height, px(23.));
    assert_eq!(b.size.height, px(23.));
    assert_eq!(a.top() - menu.top(), px(5.));
    assert_eq!(a.left() - menu.left(), px(5.));
    assert_eq!(menu.right() - a.right(), px(5.));
    // The separator is Zed's `ListSeparator`: 1px, Base06 above and below.
    assert_eq!(sep.size.height, px(1.));
    assert_eq!(sep.top() - a.bottom(), px(6.));
    assert_eq!(b.top() - sep.bottom(), px(6.));
    assert_eq!(menu.bottom() - b.bottom(), px(5.));
}

/// Same list as [`SettingsPopupListTestView`] but with `max_h` — documents
/// the gpui sizing behavior the explicit height works around.
struct MaxHeightListTestView(gpui::UniformListScrollHandle);

impl Render for MaxHeightListTestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        uniform_list("settings-select-list", 100, |range, _, _| {
            range
                .map(|ix| {
                    div()
                        .h(px(30.))
                        .w_full()
                        .child(format!("row {ix}"))
                        .into_any_element()
                })
                .collect()
        })
        .track_scroll(self.0.clone())
        .w_full()
        .max_h(px(220.))
        .px(px(4.))
        .py(px(4.))
    }
}

// ── modal cards on Zed's tokens ──────────────────────────────────────────

/// A modal card built like the app's dialogs: `elevation_3`, a header taking
/// Zed's `ModalHeader` insets, and a footer taking `ModalFooter`'s.
struct ModalCardProbe;

impl Render for ModalCardProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::tokens::{modal, StyledExt};

        let theme = *theme::get(cx);
        let header = div()
            .debug_selector(|| "modal-header".into())
            .px(modal::header_padding_x(&theme))
            .pt(modal::header_padding_top(&theme))
            .pb(modal::header_padding_bottom(&theme))
            .child(
                div()
                    .debug_selector(|| "modal-title".into())
                    .h(px(10.))
                    .child("Title"),
            );
        let footer = div()
            .debug_selector(|| "modal-footer".into())
            .px(modal::footer_padding(&theme))
            .py(modal::footer_padding(&theme))
            .child(div().debug_selector(|| "modal-action".into()).h(px(10.)));
        div().size_full().flex().justify_center().child(
            div()
                .debug_selector(|| "modal-card".into())
                .w(px(400.))
                .elevation_3(&theme)
                .flex()
                .flex_col()
                .child(header)
                .child(footer),
        )
    }
}

#[gpui::test]
fn modal_card_uses_zeds_header_and_footer_metrics(cx: &mut gpui::TestAppContext) {
    use crate::theme::ThemeId;

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(600.), px(400.)),
        |_, cx| cx.new(|_| ModalCardProbe),
    );
    let header = cx.debug_bounds("modal-header").expect("header laid out");
    let title = cx.debug_bounds("modal-title").expect("title laid out");
    let footer = cx.debug_bounds("modal-footer").expect("footer laid out");
    let action = cx.debug_bounds("modal-action").expect("action laid out");

    // Zed's `ModalHeader`: Base08 above, Base04 below, Base12 in from the sides.
    assert_eq!(title.top() - header.top(), px(8.));
    assert_eq!(header.bottom() - title.bottom(), px(4.));
    assert_eq!(title.left() - header.left(), px(12.));
    // Zed's `ModalFooter`: Base08 on all four sides.
    assert_eq!(action.left() - footer.left(), px(8.));
    assert_eq!(action.top() - footer.top(), px(8.));
    assert_eq!(footer.bottom() - action.bottom(), px(8.));
}

// ── model picker ↑/↓ end to end ───────────────────────────────────────

/// The real `OrbitApp`, the real `bind_keys`, the real chip popup: ↑/↓ must
/// move the model picker's highlight. The isolated picker tests pass even when
/// the app's focus or keymap wiring is wrong, so this is the one that covers
/// what a user actually sees.
#[gpui::test]
fn model_picker_arrows_move_the_highlight(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        crate::bind_keys(cx);
    });
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    // A catalog big enough to be keyboard-navigable, then open the popup the
    // way the model chip does.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.available_models = (0..30)
                .map(|ix| ModelEntry {
                    id: format!("m{ix}"),
                    name: format!("Model {ix}"),
                    provider: "opencode-go".into(),
                    context_window: Some(1_000_000),
                    thinking_levels: Vec::new(),
                })
                .collect();
            app.model_id = "m0".into();
            app.model_label = "Model 0".into();
            app.model_provider = "opencode-go".into();
            app.open_picker(PickerKind::Model, window, cx);
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    let highlighted = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            app.read(cx)
                .model_selector
                .as_ref()
                .map(|(_, selector)| selector.read(cx).highlighted_row())
        })
    };
    // The whole dispatch rests on the popup's filter holding focus: if the
    // composer still had it, `Composer`'s `Up`/`Down` would win at runtime and
    // the list would never move.
    let filter_focused = cx.update(|window, cx| {
        let (_, selector) = app
            .read(cx)
            .model_selector
            .clone()
            .expect("model picker open");
        selector.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(filter_focused, "the popup's filter holds focus");

    let before = highlighted(cx).expect("model picker open");
    cx.simulate_keystrokes("down");
    let after = highlighted(cx).expect("model picker open");
    assert!(
        after > before,
        "↓ moved the model picker highlight from {before} to {after}"
    );

    // It must keep moving on every press, not just the first.
    for expected in after + 1..after + 5 {
        cx.simulate_keystrokes("down");
        assert_eq!(
            highlighted(cx),
            Some(expected),
            "↓ kept moving to row {expected}"
        );
    }
    cx.simulate_keystrokes("up");
    assert_eq!(highlighted(cx), Some(after + 3), "↑ moved back");
}

/// The palette route: `on_command` closes the palette and *then* runs the
/// command, so the picker is opened while the palette's own filter still had
/// focus a moment ago. ↑/↓ must still reach the picker.
#[gpui::test]
fn model_picker_arrows_work_after_the_palette_route(cx: &mut gpui::TestAppContext) {
    use crate::command_palette::PaletteCommand;
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        crate::bind_keys(cx);
    });
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.available_models = (0..30)
                .map(|ix| ModelEntry {
                    id: format!("m{ix}"),
                    name: format!("Model {ix}"),
                    provider: "opencode-go".into(),
                    context_window: Some(1_000_000),
                    thinking_levels: Vec::new(),
                })
                .collect();
            app.model_id = "m0".into();
            app.model_label = "Model 0".into();
            app.model_provider = "opencode-go".into();
            app.toggle_command_palette(window, cx);
            // Exactly what `CommandPalette`'s `on_command` does.
            app.command_palette = None;
            app.run_palette_command(
                PaletteCommand::Run(crate::commands::CommandId::ChooseModel),
                window,
                cx,
            );
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    let filter_focused = cx.update(|window, cx| {
        let (_, selector) = app
            .read(cx)
            .model_selector
            .clone()
            .expect("Choose Model opened the picker");
        selector.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(filter_focused, "the picker's filter holds focus");

    let highlighted = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            app.read(cx)
                .model_selector
                .as_ref()
                .map(|(_, selector)| selector.read(cx).highlighted_row())
        })
    };
    let before = highlighted(cx).expect("model picker open");
    cx.simulate_keystrokes("down");
    assert!(
        highlighted(cx) > Some(before),
        "↓ moves the picker after the palette route"
    );
}

/// The chip route: the model picker opens from a chip nested inside the
/// composer box, whose own mouse-up handler focuses the composer input. gpui
/// bubbles mouse events leaf-first, so that ancestor handler runs *after* the
/// chip's and (before this fix) yanked focus straight back out of the popup —
/// leaving ↑/↓ and typing on the composer input. The direct `open_picker`
/// tests never see that ancestor, so this drives the chip handler and then
/// replays the composer box's bubbled mouse-up exactly as gpui would.
#[gpui::test]
fn chip_click_does_not_let_the_composer_steal_picker_focus(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};
    use gpui::{Modifiers, MouseButton, MouseUpEvent};

    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        crate::bind_keys(cx);
    });
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.available_models = (0..30)
                .map(|ix| ModelEntry {
                    id: format!("m{ix}"),
                    name: format!("Model {ix}"),
                    provider: "opencode-go".into(),
                    context_window: Some(1_000_000),
                    thinking_levels: Vec::new(),
                })
                .collect();
            app.model_id = "m0".into();
            app.model_label = "Model 0".into();
            app.model_provider = "opencode-go".into();
            cx.notify();
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    // The chip's own handler (what `on_mouse_up` calls) opens the popup.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.on_chip_trigger_click(PickerKind::Model, window, cx);
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );
    let filter_focused = cx.update(|window, cx| {
        let (_, selector) = app
            .read(cx)
            .model_selector
            .clone()
            .expect("the chip opened the model picker");
        selector.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(filter_focused, "the chip click focuses the picker filter");

    // Now replay the composer box's bubbled mouse-up, which gpui runs after
    // the chip's because the composer box is the chip's ancestor.
    let mouse_up = MouseUpEvent {
        button: MouseButton::Left,
        position: point(px(0.), px(0.)),
        modifiers: Modifiers::none(),
        click_count: 1,
    };
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.on_composer_click(&mouse_up, window, cx));
    });

    let filter_focused = cx.update(|window, cx| {
        let (_, selector) = app.read(cx).model_selector.clone().expect("still open");
        selector.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(
        filter_focused,
        "the composer box's mouse-up must not steal focus from the open picker"
    );

    let highlighted = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            app.read(cx)
                .model_selector
                .as_ref()
                .map(|(_, selector)| selector.read(cx).highlighted_row())
        })
    };
    let before = highlighted(cx).expect("model picker open");
    cx.simulate_keystrokes("down");
    let after = highlighted(cx).expect("model picker open");
    assert!(
        after > before,
        "↓ moved the model picker highlight from {before} to {after}"
    );

    // Closing the picker hands focus back to the composer, and a click on the
    // box then focuses it normally (the guard only applies while a popover is
    // open).
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.close_model_selector(window, cx));
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.on_composer_click(&mouse_up, window, cx));
    });
    let composer_focused = cx.update(|window, cx| {
        app.read(cx)
            .input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(
        composer_focused,
        "with no popover open the composer box still focuses the input"
    );
}

// ── top-bar provider-quota popover ────────────────────────────────────

/// The quota popover grows with the number of connected providers. Past its
/// height cap the card list must scroll rather than let the popover clip the
/// last card. The list is the scroll region (content-sized up to the cap):
/// inside the deferred, anchored popover the available height is zero, where a
/// `flex_1` child collapses and the cards get clipped instead.
#[gpui::test]
fn quota_popup_scrolls_instead_of_clipping_its_last_card(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    // Five providers with two windows each: comfortably past the 460px cap
    // that three providers already brush.
    cx.update(|_, cx| {
        app.update(cx, |app, _cx| {
            let providers: Vec<serde_json::Value> = (0..5)
                .map(|ix| {
                    serde_json::json!({
                        "provider": format!("provider-{ix}"),
                        "kind": "subscription",
                        "windows": [
                            {"id": "session", "label": "5-hour session", "usedPercent": 40.0, "resetsAt": 1},
                            {"id": "weekly", "label": "Weekly", "usedPercent": 70.0, "resetsAt": 1}
                        ]
                    })
                })
                .collect();
            app.quota.on_response(
                true,
                Some(&serde_json::json!({ "providers": providers })),
                None,
            );
            app.quota_popup_open = true;
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    let popup = cx
        .debug_bounds("quota-popup")
        .expect("quota popup laid out");
    let body = cx
        .debug_bounds("quota-popup-body")
        .expect("quota popup body laid out");

    // The popover stays within its cap...
    assert!(
        popup.size.height <= px(460.),
        "popup height {:?}",
        popup.size.height
    );
    // ...and the card list ends at the popover's bottom edge, so nothing is
    // clipped away below it.
    assert!(
        body.bottom() <= popup.bottom(),
        "the card list overflows the popover: body {body:?}, popup {popup:?}"
    );
    // The list left room for the header instead of stretching over it.
    assert!(
        body.size.height < popup.size.height,
        "the card list did not leave room for the header: body {body:?}, popup {popup:?}"
    );
}

/// Hiding a provider removes it from the popover's active list and parks it in
/// the "Hidden" group, so it stays one click from coming back. The visible
/// card and the restore card are distinct elements, never the same one.
#[gpui::test]
fn quota_popup_moves_hidden_providers_into_their_own_group(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    cx.update(|_, cx| {
        app.update(cx, |app, _cx| {
            let providers = serde_json::json!({
                "providers": [
                    {"provider": "alpha", "kind": "subscription",
                     "windows": [{"id": "session", "label": "5-hour session", "usedPercent": 10.0, "resetsAt": 1}]},
                    {"provider": "beta", "kind": "subscription",
                     "windows": [{"id": "session", "label": "5-hour session", "usedPercent": 20.0, "resetsAt": 1}]}
                ]
            });
            app.quota.on_response(true, Some(&providers), None);
            app.hidden_quota_providers =
                crate::quota_hidden::HiddenProviders::from_providers(&["beta"]);
            app.quota_popup_open = true;
            app.quota_hidden_open = true;
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    assert!(
        cx.debug_bounds("quota-card-visible-alpha").is_some(),
        "the visible provider card is missing"
    );
    assert!(
        cx.debug_bounds("quota-card-visible-beta").is_none(),
        "a hidden provider still drew in the active list"
    );
    assert!(
        cx.debug_bounds("quota-card-hidden-beta").is_some(),
        "the hidden provider is missing from the Hidden group"
    );
    assert!(
        cx.debug_bounds("quota-hidden-toggle").is_some(),
        "the Hidden group toggle is missing"
    );
}

/// The usage popover carries a second pane for MCP server status. The tab
/// switch must swap the body — never draw both — and an MCP-only setup (no
/// provider quota reports) must still get the pill, so the popover and the
/// pane stay reachable.
#[gpui::test]
fn quota_popup_mcp_tab_swaps_the_pane(cx: &mut gpui::TestAppContext) {
    use crate::app::mcp_ui::QuotaPopupTab;
    use crate::mcp::McpManager;
    use crate::theme::{Theme, ThemeId};
    use std::fs;

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    // A throwaway MCP configuration with one server, and no quota reports:
    // the quiet pill must still render so the popover is reachable.
    let root = std::env::temp_dir().join(format!(
        "orbit-quota-mcp-popup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let home = root.join("home");
    let workspace = root.join("project");
    fs::create_dir_all(home.join(".pi").join("agent")).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        home.join(".pi").join("agent").join("mcp.json"),
        r#"{"mcpServers":{"echo":{"command":"node","args":["--version"]}}}"#,
    )
    .unwrap();

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.mcp = McpManager::load_in(&home, Some(&workspace));
            // The exact user flow: click the top-bar pill. With no quota
            // reports and MCP configured, the pill IS the MCP chip, so the
            // popover must open on the MCP pane — otherwise the server
            // names are hidden behind an empty Providers tab.
            app.toggle_quota_popup(cx);
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    assert!(
        cx.debug_bounds("quota-popup").is_some(),
        "the popover is missing (the MCP-only pill must still render it)"
    );
    let mcp_body = cx
        .debug_bounds("quota-mcp-body")
        .expect("the MCP pane laid out");
    assert!(
        cx.debug_bounds("quota-mcp-row-echo").is_some(),
        "the configured server row is missing"
    );
    // The tab chip and the server name must have real painted width — a
    // zero-width row would read as "the MCP name is not showing".
    let tab = cx
        .debug_bounds("quota-tab-chip-mcp")
        .expect("the MCP tab chip laid out");
    assert!(
        tab.size.width > px(10.),
        "the MCP tab is invisible: {tab:?}"
    );
    // This selector sits on a content-sized line, so its width is the
    // shaped text's width rather than the cell's. A name latched as "…"
    // measures ~9px here; "echo" is ~26px at Small.
    let name = cx
        .debug_bounds("quota-mcp-name-echo")
        .expect("the server name laid out");
    assert!(
        name.size.width > px(18.) && name.size.height > px(8.),
        "the server name rendered without its text: {name:?}"
    );
    let row = cx.debug_bounds("quota-mcp-row-echo").unwrap();
    assert!(
        name.right() <= row.right() && name.left() >= row.left(),
        "the name is clipped outside its row: name {name:?}, row {row:?}"
    );
    assert!(
        cx.debug_bounds("quota-popup-body").is_none(),
        "the providers pane still drew under the MCP tab"
    );
    let popup = cx.debug_bounds("quota-popup").unwrap();
    assert!(
        mcp_body.bottom() <= popup.bottom(),
        "the MCP list overflows the popover: body {mcp_body:?}, popup {popup:?}"
    );

    // Switching back restores the providers pane.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.mcp_popup_tab(QuotaPopupTab::Providers, cx)
        });
    });
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );
    assert!(cx.debug_bounds("quota-popup-body").is_some());
    assert!(cx.debug_bounds("quota-mcp-body").is_none());
    // With no quota reports the Providers pane is an empty state that points
    // back at the MCP pane, never a blank card.
    assert!(
        cx.debug_bounds("quota-empty-mcp").is_some(),
        "the providers empty state is missing"
    );

    let _ = fs::remove_dir_all(&root);
}

// ── model / thinking pickers lock while a run is in flight ────────────

/// A running turn fixes the model and thinking level, so the chips swallow
/// clicks until it settles: neither picker may open while `is_running`.
#[gpui::test]
fn model_and_thinking_pickers_stay_shut_while_running(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let _ = cx.draw(
        point(px(0.), px(0.)),
        gpui::size(px(1200.), px(820.)),
        |_, _| app.clone(),
    );

    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.available_models = vec![ModelEntry {
                id: "m0".into(),
                name: "Model 0".into(),
                provider: "opencode-go".into(),
                context_window: Some(1_000_000),
                thinking_levels: Vec::new(),
            }];
            app.available_thinking_levels = vec!["low".into(), "high".into()];
            app.busy = true;
        });
    });

    // Both chip routes refuse while running.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.on_chip_trigger_click(PickerKind::Model, window, cx);
            app.on_chip_trigger_click(PickerKind::Thinking, window, cx);
        });
    });
    assert!(
        cx.update(|_, cx| app.read(cx).model_selector.is_none()),
        "a running turn keeps both pickers shut"
    );

    // Once the turn settles the same click opens the model picker again.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.busy = false;
            app.on_chip_trigger_click(PickerKind::Model, window, cx);
        });
    });
    assert!(
        cx.update(|_, cx| app.read(cx).model_selector.is_some()),
        "an idle turn opens the model picker"
    );
}
