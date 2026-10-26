use super::*;

struct ComposerStatusProbe {
    app: Entity<OrbitApp>,
    width: Pixels,
}

impl Render for ComposerStatusProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            div().w(self.width).child(app.status_bar("orbit", cx))
        })
    }
}

struct QueueBarProbe {
    app: Entity<OrbitApp>,
    width: Pixels,
}

/// The composer's own row, so the chips' presence rules can be read off the
/// laid-out tree rather than inferred from the state they are built from.
struct ComposerRowProbe {
    app: Entity<OrbitApp>,
}

impl Render for ComposerRowProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            div().w(px(900.)).child(app.composer_row(false, cx))
        })
    }
}

impl Render for QueueBarProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            div().w(self.width).children(app.queue_bar(cx))
        })
    }
}

/// The queue strip above the composer is one grouped surface: a 30px label
/// header plus one ~30px row per pending message. Pin that shape so the rows
/// stay flat (no card nested inside the card) and the strip stays compact as
/// messages accumulate.
#[gpui::test]
fn queue_bar_is_one_compact_grouped_surface(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};
    use orbit_rpc::PendingQueue;

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| {
        cx.new(|cx| {
            let mut app = OrbitApp::new(cx);
            app.branch = None;
            app.status_at = None;
            app
        })
    });

    let width = px(600.);
    let draw = |queue: PendingQueue, cx: &mut gpui::VisualTestContext| {
        app.update(cx, |app, _| app.queue = queue);
        let _ = cx.draw(
            point(px(0.), px(0.)),
            gpui::size(width, px(400.)),
            |_, cx| {
                cx.new(|_| QueueBarProbe {
                    app: app.clone(),
                    width,
                })
            },
        );
        cx.debug_bounds("queue-bar")
    };

    // Empty: no strip at all.
    assert!(draw(PendingQueue::default(), cx).is_none());

    // One follow-up: header + one row, compact — not two stacked cards.
    let one = draw(
        PendingQueue {
            steering: vec![],
            follow_up: vec!["i need the simple and minimal website design".into()],
        },
        cx,
    )
    .expect("queue strip drawn");
    assert!((one.size.width - width).abs() <= px(1.));
    assert!(
        one.size.height >= px(58.) && one.size.height <= px(64.),
        "one row should be ~60px, was {}",
        one.size.height
    );

    // The clear control is the header's one action, inset from the right.
    let clear = cx.debug_bounds("clear-queue").expect("clear button drawn");
    assert!(clear.center().y < one.top() + px(30.));
    assert!((one.right() - px(12.) - clear.right()).abs() <= px(1.));

    // A second message adds one hairline-separated row, not a second card.
    let two = draw(
        PendingQueue {
            steering: vec!["tighten the hero".into()],
            follow_up: vec!["i need the simple and minimal website design".into()],
        },
        cx,
    )
    .expect("queue strip drawn");
    let row = two.size.height - one.size.height;
    assert!(
        row >= px(27.) && row <= px(33.),
        "one more row should add ~30px, added {row}"
    );
}
#[gpui::test]
fn sending_hints_match_footer_metrics_and_share_the_context_row(cx: &mut gpui::TestAppContext) {
    use crate::theme::{
        tokens::{button, ButtonSize},
        Theme, ThemeId,
    };

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| {
        cx.new(|cx| {
            let mut app = OrbitApp::new(cx);
            app.branch = None;
            app.status_at = None;
            app.context = Some(orbit_rpc::ContextUsage {
                tokens: Some(64_000),
                context_window: 128_000,
                percent: Some(50.0),
            });
            // Only the running half of the rule is measured here — the idle
            // half has its own test, because an absence can only be observed
            // in a window that has never painted the element.
            app.busy = true;
            app
        })
    });

    for mode in [SendMode::FollowUp, SendMode::Steer] {
        cx.update(|_, cx| {
            cx.global_mut::<Theme>().ui.composer_send_mode = mode;
        });
        for (width, font_size) in [(320., 14.), (600., 14.), (960., 14.), (600., 18.)] {
            let theme = cx.update(|_, cx| {
                let theme = cx.global_mut::<Theme>();
                theme.ui.ui_font_size = font_size;
                *theme
            });
            let width = px(width);
            let _ = cx.draw(
                point(px(0.), px(0.)),
                gpui::size(width, px(80.)),
                |_, cx| {
                    cx.new(|_| ComposerStatusProbe {
                        app: app.clone(),
                        width,
                    })
                },
            );
            let hints = cx
                .debug_bounds("composer-send-hints")
                .expect("sending hints laid out in the status row");
            let indicator = cx
                .debug_bounds("composer-context-indicator")
                .expect("context indicator laid out");
            let label = cx
                .debug_bounds("composer-hint-label")
                .expect("hint label laid out");
            let keys = cx
                .debug_bounds("composer-hint-keys")
                .expect("hint shortcut laid out");
            let label_size = button::label_size(ButtonSize::Default).px(&theme);
            let key_size = if cfg!(target_os = "macos") {
                ButtonSize::Default.icon_size().px(&theme)
            } else {
                label_size
            };

            // GPUI rounds layout to whole pixels, including scaled UI sizes.
            assert!((label.size.height - label_size).abs() <= px(1.));
            assert!((keys.size.height - key_size).abs() <= px(1.));
            assert!((hints.size.height - ButtonSize::Default.height(&theme)).abs() <= px(1.));
            assert!(hints.size.width > px(0.));
            assert!(hints.right() < indicator.left());
            assert!((hints.center().y - indicator.center().y).abs() <= px(1.));
            assert_eq!(indicator.right(), width);
        }
    }
}

/// The hints teach the steering chords, and those exist only while a run is in
/// flight. Idle, they restate the send button a few pixels above, so they stay
/// out of the chrome and the context indicator carries the right edge alone.
///
/// Its own test because `debug_bounds` reads a registry that survives a redraw:
/// a window that has once painted the hints keeps reporting them, so an absence
/// has to be observed in a window that never has.
#[gpui::test]
fn an_idle_status_row_keeps_the_send_hints_out_of_the_chrome(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| {
        cx.new(|cx| {
            let mut app = OrbitApp::new(cx);
            app.branch = None;
            app.status_at = None;
            app
        })
    });

    for mode in [SendMode::FollowUp, SendMode::Steer] {
        for width in [320., 600., 960.] {
            cx.update(|_, cx| {
                cx.global_mut::<Theme>().ui.composer_send_mode = mode;
            });
            let width = px(width);
            let _ = cx.draw(
                point(px(0.), px(0.)),
                gpui::size(width, px(80.)),
                |_, cx| {
                    cx.new(|_| ComposerStatusProbe {
                        app: app.clone(),
                        width,
                    })
                },
            );
            assert!(
                !app.update(cx, |app, _| app.is_running()),
                "the fixture is idle, so the hints have nothing to teach"
            );
            assert!(
                cx.debug_bounds("composer-send-hints").is_none(),
                "an idle status row keeps the send hints out of the chrome"
            );
            let indicator = cx
                .debug_bounds("composer-context-indicator")
                .expect("context indicator laid out");
            assert_eq!(indicator.right(), width);
        }
    }
}

/// `["off"]` is what pi reports for a model without reasoning and an empty list
/// is what it reports before the levels arrive, so neither offers a level to
/// pick. The chip, the ⌘T shortcut, and the palette entry all read the same
/// predicate, so they cannot disagree about whether the picker has options.
///
/// Each case gets its own window: `debug_bounds` keeps reporting an element once
/// the window has painted it, so presence and absence cannot be measured against
/// each other in one frame registry.
#[gpui::test]
fn the_thinking_chip_appears_only_when_the_model_has_a_ladder(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let app = cx.update(|cx| cx.new(OrbitApp::new));

    for (levels, expected) in [
        (vec!["off".to_string()], false),
        (Vec::new(), false),
        (vec!["off".into(), "low".into(), "high".into()], true),
        // A provider that drops `off` still leaves more than one choice.
        (vec!["low".into(), "high".into(), "max".into()], true),
    ] {
        let window = cx.add_empty_window();
        let can_choose = window.update(|_, cx| {
            app.update(cx, |app, _| {
                app.available_thinking_levels = levels.clone();
                app.can_choose_thinking()
            })
        });
        let _ = window.draw(
            point(px(0.), px(0.)),
            gpui::size(px(900.), px(60.)),
            |_, cx| cx.new(|_| ComposerRowProbe { app: app.clone() }),
        );
        assert_eq!(
            can_choose,
            expected,
            "levels {levels:?} should {} a thinking choice",
            if expected { "offer" } else { "not offer" }
        );
        assert_eq!(
            window.debug_bounds("thinking-chip").is_some(),
            expected,
            "levels {levels:?} should {} the thinking chip",
            if expected { "show" } else { "hide" }
        );
    }
}

/// Full access is the default, so its name is not news and the chip keeps the
/// glyph alone; a tightened mode is a departure from the default and keeps the
/// label that says so. The chip itself never leaves either way — hiding it
/// outright would strand the mode on a control that no longer exists.
#[gpui::test]
fn the_access_chip_names_the_mode_only_when_it_is_not_the_default(cx: &mut gpui::TestAppContext) {
    use crate::access::AccessMode;
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let app = cx.update(|cx| cx.new(OrbitApp::new));

    for (mode, labelled) in [
        (AccessMode::FullAccess, false),
        (AccessMode::AutoAcceptEdits, true),
        (AccessMode::Supervised, true),
    ] {
        let window = cx.add_empty_window();
        window.update(|_, cx| {
            app.update(cx, |app, _| app.access_mode = mode);
        });
        let _ = window.draw(
            point(px(0.), px(0.)),
            gpui::size(px(900.), px(60.)),
            |_, cx| cx.new(|_| ComposerRowProbe { app: app.clone() }),
        );
        assert!(
            window.debug_bounds("access-chip").is_some(),
            "{mode:?} keeps the access chip on the row"
        );
        assert_eq!(
            window.debug_bounds("access-chip-label").is_some(),
            labelled,
            "{mode:?} should {} the mode name",
            if labelled { "show" } else { "drop" }
        );

        // The chip must not resize when the picker opens: a control that grows
        // a label on click shoves the workflow chip sideways under the pointer.
        let closed = window
            .debug_bounds("access-chip")
            .expect("access chip laid out");
        window.update(|window, cx| {
            app.update(cx, |app, cx| app.toggle_access_menu(window, cx));
        });
        let _ = window.draw(
            point(px(0.), px(0.)),
            gpui::size(px(900.), px(60.)),
            |_, cx| cx.new(|_| ComposerRowProbe { app: app.clone() }),
        );
        let open = window
            .debug_bounds("access-chip")
            .expect("access chip still laid out");
        assert_eq!(
            open.size.width, closed.size.width,
            "{mode:?} keeps the chip's width while its picker is open"
        );
        window.update(|window, cx| {
            app.update(cx, |app, cx| app.close_access_menu(window, cx));
        });
    }
}
