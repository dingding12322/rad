//! The status row's "Work in" chip: a location control that stays out of the way
//! at the repository's main checkout, where "Local" is the default.
//!
//! The chip used to show "Local" on every Git repository, which is the state the
//! user is in by default and therefore the one state that needs no label. It now
//! appears only in a linked worktree, where it names a non-default location and
//! is the way back. The Worktrees page — which creates and switches worktrees —
//! lives in the top bar's "…" menu, so nothing is stranded by hiding the chip.

use super::*;
use crate::theme::{Theme, ThemeId};
use crate::worktree::Worktree;

/// A `git worktree list` entry, with only the fields these tests read set.
fn worktree(path: &str, name: &str, is_main: bool) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        name: name.into(),
        head: "0".repeat(40),
        branch: Some(name.into()),
        detached: false,
        is_main,
        bare: false,
        locked: false,
        lock_reason: None,
        prunable: false,
        prune_reason: None,
    }
}

#[gpui::test]
fn the_work_in_chip_is_absent_at_the_main_checkout(cx: &mut gpui::TestAppContext) {
    let app = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    let cx = cx.add_empty_window();
    cx.update(|_window, cx| {
        app.update(cx, |app, cx| {
            // Not a repository at all: there is no "work in" to choose.
            app.worktrees.clear();
            assert!(app.work_in_chip(cx).is_none(), "no worktrees means no chip");

            // A repository, active on its main checkout: "Local" is the
            // default, so the chip stays out of the status row.
            app.worktrees = vec![
                worktree("/repo", "repo", true),
                worktree("/wt/feature", "feature", false),
            ];
            app.current_workspace = Some(PathBuf::from("/repo"));
            assert!(
                app.work_in_chip(cx).is_none(),
                "the main checkout is the default and the default earns no pixels"
            );
            // A path inside the main checkout is still the main checkout.
            app.current_workspace = Some(PathBuf::from("/repo/crates/orbit-pi"));
            assert!(app.work_in_chip(cx).is_none());

            // Active on a linked worktree: the chip names the non-default
            // location and is the way back to Local.
            app.current_workspace = Some(PathBuf::from("/wt/feature"));
            assert!(
                app.work_in_chip(cx).is_some(),
                "a linked worktree is not the default, so the chip shows"
            );
        });
    });
}
