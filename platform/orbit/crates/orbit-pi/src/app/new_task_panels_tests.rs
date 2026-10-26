//! Regression test: starting a new task must close whatever surface was
//! covering the chat column, so the fresh new-task page is what the user
//! sees. Before this, New Task only reset the session — Review, Git, the
//! Explorer, the terminal, and Settings stayed open behind/over the page.

use super::*;
use crate::theme::{Theme, ThemeId};

fn test_app(cx: &mut gpui::TestAppContext) -> Entity<OrbitApp> {
    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    })
}

/// Open one of each surface the way its toggle does, then assert the
/// new-task hand-off leaves none of them up.
#[gpui::test]
fn starting_a_new_task_closes_every_open_panel(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.git_open = true;
            app.settings_open = true;
            app.sidepane.update(cx, |pane, cx| pane.toggle(cx));
            app.project_panel.update(cx, |panel, cx| panel.toggle(cx));
            app.terminal_panel
                .update(cx, |panel, cx| panel.toggle(window, cx));

            assert!(app.git_open);
            assert!(app.settings_open);
            assert!(app.sidepane.read(cx).is_open());
            assert!(app.project_panel.read(cx).is_open());
            assert!(app.terminal_panel.read(cx).is_open());

            app.close_surfaces_for_new_task(cx);

            assert!(!app.git_open, "the Git page must close");
            assert!(!app.settings_open, "Settings must close");
            assert!(!app.sidepane.read(cx).is_open(), "Review must close");
            assert!(
                !app.project_panel.read(cx).is_open(),
                "the Explorer must close"
            );
            assert!(
                !app.terminal_panel.read(cx).is_open(),
                "the terminal must close"
            );
        });
    });
}

/// The hand-off is idempotent: running it with nothing open (the common case)
/// is a no-op rather than a panic or a spurious focus change.
#[gpui::test]
fn closing_surfaces_with_nothing_open_is_a_no_op(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.close_surfaces_for_new_task(cx);
            assert!(!app.git_open);
            assert!(!app.settings_open);
            assert!(!app.sidepane.read(cx).is_open());
            assert!(!app.project_panel.read(cx).is_open());
            assert!(!app.terminal_panel.read(cx).is_open());
        });
    });
}
