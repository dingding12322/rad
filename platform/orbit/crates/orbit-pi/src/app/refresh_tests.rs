//! Regression test: the global Refresh (⌘R) started as "Refresh Sessions"
//! but must now reach every live surface, not just the session list.

use super::*;
use crate::theme::{Theme, ThemeId};

fn test_app(cx: &mut gpui::TestAppContext) -> Entity<OrbitApp> {
    cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    })
}

#[gpui::test]
fn refresh_reaches_every_open_surface(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // No agent: the RPC-backed refreshes are skipped, so this test
            // exercises only the local surfaces.
            app.client = None;

            // Worktrees: an open page re-lists (busy flips synchronously).
            app.worktrees_open = true;
            assert!(!app.worktrees_busy);
            // MCP: a probe re-reads the config, clearing the external-change
            // notice that prompted the refresh.
            app.settings_open = true;
            app.settings_section = SettingsSection::Mcp;
            app.mcp_external_change = true;

            app.on_refresh(&crate::RefreshSessions, window, cx);

            assert!(
                app.worktrees_busy,
                "Refresh must re-list the open Worktrees page"
            );
            assert!(
                !app.mcp_external_change,
                "Refresh must re-read the MCP config"
            );
        });
    });
}

/// A refresh with every page closed is a cheap no-op — it must not start a
/// worktree list or touch the MCP manager just because ⌘R was pressed.
#[gpui::test]
fn refresh_is_quiet_with_no_page_open(cx: &mut gpui::TestAppContext) {
    let app = test_app(cx);
    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.client = None;
            app.worktrees_open = false;
            app.settings_open = false;
            app.on_refresh(&crate::RefreshSessions, window, cx);
            assert!(!app.worktrees_busy, "no worktree list without the page");
        });
    });
}
