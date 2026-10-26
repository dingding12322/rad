//! Regression tests for issue #63: closing the window leaves the app — and any
//! run it is holding — alive. `window_active` gates desktop notifications, so a
//! window close has to clear it; the last activation event said the window was
//! frontmost, and nothing else reports the close.

use super::*;
use crate::theme::{Theme, ThemeId};
use gpui::{point, px, size, Bounds, TestAppContext, WindowBounds, WindowOptions};

/// A closed window must not keep swallowing notifications.
#[gpui::test]
fn closing_the_window_clears_the_notification_gate(cx: &mut TestAppContext) {
    let (app, window) = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        let app = cx.new(OrbitApp::new);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1280.), px(800.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    app.update(cx, |app, cx| app.watch_window_activation(window, cx));
                    app.clone()
                },
            )
            .expect("open a window");
        (app, window)
    });

    // The test platform reports a fresh window as inactive, so stand in for
    // the case that matters: the user closes a frontmost window.
    cx.update(|cx| app.update(cx, |app, _| app.window_active = true));

    window
        .update(cx, |_, window, _| window.remove_window())
        .expect("close the window");

    cx.update(|cx| {
        assert!(
            !app.read(cx).window_active,
            "a closed window must not hold back the next run's notification"
        );
    });
}
