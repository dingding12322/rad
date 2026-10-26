//! The Worktrees page's "How worktrees work" card shows for newcomers and
//! stays hidden once dismissed.

use super::*;
use crate::theme::{Theme, ThemeId};

#[gpui::test]
fn howto_card_shows_until_dismissed(cx: &mut gpui::TestAppContext) {
    let app = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    let cx = cx.add_empty_window();
    cx.update(|_window, cx| {
        app.update(cx, |app, cx| {
            app.worktree_howto_dismissed = false;
            assert!(
                app.worktree_howto_card(*theme::get(cx), cx.entity())
                    .is_some(),
                "the card is present until dismissed"
            );
            app.worktree_howto_dismissed = true;
            assert!(
                app.worktree_howto_card(*theme::get(cx), cx.entity())
                    .is_none(),
                "dismissing hides the card"
            );
        });
    });
}

/// The card fits its column and stays content-height: before `min_w_0`, the
/// intro paragraph's min-content width forced the card wider than the page and
/// inflated its height to thousands of pixels, leaving a mostly-empty card.
#[gpui::test]
fn howto_card_fits_the_page_column(cx: &mut gpui::TestAppContext) {
    let app = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    let cx = cx.add_empty_window();
    let viewport = cx.update(|window, _| window.viewport_size());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            for dependency in &mut app.deps {
                dependency.installed = true;
            }
            app.worktrees_open = true;
            app.worktree_howto_dismissed = false;
            cx.notify();
        });
    });
    // The page settles over a frame (workspace discovery, then a refresh).
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());
    let _ = cx.draw(gpui::point(px(0.), px(0.)), viewport, |_, _| app.clone());

    let card = cx.debug_bounds("worktree-howto-card").expect("howto card");
    let list = cx.debug_bounds("worktree-page-list").expect("list section");
    let column = cx
        .debug_bounds("worktree-page-content")
        .expect("page content");

    assert!(
        card.size.width <= column.size.width,
        "the card must not overflow its column (card {card:?}, column {column:?})"
    );
    assert!(
        card.size.height < px(400.),
        "the card must stay content-height, got {card:?}"
    );
    assert!(
        list.origin.y >= card.origin.y + card.size.height,
        "the worktree list must sit below the card (card {card:?}, list {list:?})"
    );
}
