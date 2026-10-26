use super::*;

// ── session-birth default model push ─────────────────────────────────

/// A `set_model` that pi rejects can never complete the session-birth
/// default push. A lingering armed flag would otherwise re-apply the
/// configured thinking level the next time the same model goes live,
/// silently clobbering a manual thinking choice.
#[gpui::test]
fn failed_default_model_push_disarms_the_default(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.default_model_armed = true;
            let mut refresh_sessions = false;
            app.on_response(
                "set_model",
                false,
                None,
                Some("No API key for provider/model"),
                &mut refresh_sessions,
                cx,
            );
            assert!(
                !app.default_model_armed,
                "a rejected set_model must disarm the pending default"
            );
        });
    });
}

/// A manual model selection likewise supersedes the pending default, so a
/// later `get_state` cannot push the configured thinking level over the
/// user's choice.
#[gpui::test]
fn manual_model_selection_disarms_the_default(cx: &mut gpui::TestAppContext) {
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.default_model_armed = true;
            app.set_model("default-model".into(), "default-provider".into(), cx);
            assert!(
                !app.default_model_armed,
                "a manual set_model must disarm the pending default"
            );
        });
    });
}
