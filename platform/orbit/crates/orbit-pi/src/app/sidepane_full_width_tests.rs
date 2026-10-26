//! The hand-off between a full-page Review pane and the sessions sidebar.

use super::*;

/// Expanding the Review pane hides the session view but opens the sessions
/// sidebar, so the reader can still switch sessions while the review owns the
/// page.
#[gpui::test]
fn expanding_review_opens_the_sessions_sidebar(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));

    let viewport = cx.update(|window, _| window.viewport_size());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            // The pane is gated on the runtime probe; the test host is not
            // asked to have pi installed just to lay the window out.
            for dependency in &mut app.deps {
                dependency.installed = true;
            }
            app.sidebar_visible = false;
            app.sidepane.update(cx, |pane, cx| pane.toggle(cx));
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    assert!(
        !cx.update(|_, cx| app.read(cx).sidebar_visible),
        "the sidebar starts closed for this test"
    );

    // The rising edge is handled in the render, so expanding takes one draw.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.sidepane
                .update(cx, |pane, cx| pane.toggle_full_width(cx));
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    cx.update(|_, cx| {
        let app = app.read(cx);
        assert!(app.sidepane.read(cx).is_full_width());
        assert!(
            app.sidebar_visible,
            "expanding the review opens the sessions sidebar"
        );
        assert_eq!(
            app.sidepane.read(cx).width(),
            viewport.width - app.sidebar_width,
            "the expanded review shares the screen with the sidebar instead of covering it"
        );
    });
}

/// An expanded pane fills the whole page beside the open sidebar, and docking
/// gives the session view back at the width the reader dialed in.
#[gpui::test]
fn expanding_fills_the_page_beside_the_sidebar(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));

    let viewport = cx.update(|window, _| window.viewport_size());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            for dependency in &mut app.deps {
                dependency.installed = true;
            }
            app.sidepane.update(cx, |pane, cx| {
                pane.toggle(cx);
                pane.set_width(px(400.), cx);
            });
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());

    // Docked: the pane keeps the width the reader dialed in.
    assert_eq!(
        cx.update(|_, cx| app.read(cx).sidepane.read(cx).width()),
        px(400.)
    );

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.sidepane
                .update(cx, |pane, cx| pane.toggle_full_width(cx));
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    let sidebar_width = cx.update(|_, cx| app.read(cx).sidebar_width);
    assert_eq!(
        cx.update(|_, cx| app.read(cx).sidepane.read(cx).width()),
        viewport.width - sidebar_width,
        "an expanded pane fills the page left beside the sessions sidebar"
    );

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.sidepane
                .update(cx, |pane, cx| pane.leave_full_width(cx));
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    assert_eq!(
        cx.update(|_, cx| app.read(cx).sidepane.read(cx).width()),
        px(400.),
        "docking restores the reader's width"
    );
}

/// A wide docked Review pane must never spill past the window's right edge.
/// The pane is clamped to the space left beside the sidebar, and the chat
/// column yields by shrinking rather than forcing the row to overflow: a body
/// that refuses to shrink below its content pushes the pane (the last flex
/// child) clean out of the viewport.
#[gpui::test]
fn a_wide_docked_review_stays_inside_the_window(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));
    let viewport = cx.update(|window, _| window.viewport_size());

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            for dependency in &mut app.deps {
                dependency.installed = true;
            }
            app.sidebar_visible = true;
            app.sidepane.update(cx, |pane, cx| {
                pane.toggle(cx);
                // The widest a left-edge drag reaches at the reserve clamp.
                pane.set_width(viewport.width - px(PANE_MAX_RESERVE), cx);
            });
        });
    });
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());

    let pane = cx.debug_bounds("side-pane").expect("pane laid out");
    assert!(
        pane.right() <= viewport.width + px(1.),
        "the docked review overflows the window: pane {pane:?}, viewport {viewport:?}"
    );
}
