//! Opening — and re-opening — the app's one main window.
//!
//! macOS does not quit an app when its last window closes. The Dock icon
//! stays (with the running dot), and clicking it asks the app to show a
//! window again: `applicationShouldHandleReopen:hasVisibleWindows:`, which
//! GPUI surfaces as [`Application::on_reopen`]. An app that never installs a
//! handler answers nothing, so the Dock click is a no-op and only a full quit
//! brings a window back. Orbit installs one here.
//!
//! Reopening restores the *same* shell rather than building a second one —
//! see [`Shell`].

use std::{cell::RefCell, time::Duration};

use gpui::{
    prelude::*, px, size, App, Application, AsyncApp, Bounds, Entity, Focusable, Global, Timer,
    WindowBounds, WindowOptions,
};

use crate::{app::OrbitApp, platform, theme};

/// The shell model, parked while no window is open.
///
/// A window only *views* this entity — the pi child process behind it, the
/// streaming transcript, the open session, every in-flight request live in
/// [`OrbitApp`]. Dropping the entity when the window closes would kill the
/// run the user closed the window over, so it is parked in a global instead
/// and the next window re-renders the state that is already there.
#[derive(Default)]
pub struct Shell(pub RefCell<Option<Entity<OrbitApp>>>);

impl Global for Shell {}

/// Answer the Dock's reopen request with a window.
///
/// Registered on the [`Application`] before it runs, because that is where
/// the platform callback lives; macOS only fires it when it reports no
/// visible window, which is exactly the windowless state this handles. The
/// emptiness check is belt-and-braces: a hidden-but-alive window already
/// responds to a Dock click on its own, and a second window is never what
/// the user asked for.
pub fn install_reopen_handler(application: &mut Application) {
    application.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open(cx);
        }
    });
}

/// The one [`OrbitApp`], parked in the [`Shell`] global across window closes.
///
/// Built on the first open and reused afterwards: the pi process and the
/// stream it feeds outlive the window that showed them, so a second window
/// must render the shell that is already there.
///
/// `default_global` installs an empty shell on the first open and leaves the
/// parked one alone on reopen.
fn shell(cx: &mut App) -> Entity<OrbitApp> {
    // Clone out and drop the borrow before creating anything that could touch
    // the global again.
    if let Some(parked) = cx.default_global::<Shell>().0.borrow().clone() {
        return parked;
    }
    let app = cx.new(OrbitApp::new);
    // Detect installed editors/terminals for the header "open in" control
    // (off-thread; icons load once).
    app.update(cx, |app, cx| app.detect_open_in_apps(cx));
    cx.default_global::<Shell>()
        .0
        .borrow_mut()
        .replace(app.clone());
    spawn_heartbeat(cx, &app);
    app
}

/// Open the main window, rendering the parked shell when there is one.
pub fn open(cx: &mut App) {
    let app = shell(cx);

    // Open maximized: full width of the screen, filling the visible
    // frame. The computed bounds are the
    // restore size macOS returns to when the window is un-zoomed,
    // sized relative to the display so it always fits even on
    // small/scaled screens.
    let (w, h) = match cx.primary_display().map(|d| d.bounds().size) {
        Some(s) => (
            (f32::from(s.width) * 0.85).min(1440.),
            (f32::from(s.height) * 0.9).min(920.),
        ),
        None => (1240., 840.),
    };
    let restore_bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let Ok(_window) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Maximized(restore_bounds)),
            // Keep the window usable when shrunk: sidebar (min
            // 200px) + a readable transcript + the composer.
            window_min_size: Some(size(px(960.), px(640.))),
            titlebar: Some(platform::titlebar_options()),
            app_id: Some("dev.orbit.pi".into()),
            focus: true,
            ..Default::default()
        },
        |window, cx| {
            theme::watch_system_appearance(window, cx);

            // Focus the composer so typing works immediately; track
            // window focus so background notifications know whether
            // the user is already looking at the transcript.
            app.update(cx, |app, cx| {
                app.watch_window_activation(window, cx);
                let handle = app.input.read(cx).focus_handle(cx);
                window.focus(&handle);
            });

            app
        },
    ) else {
        eprintln!("Orbit: could not open the main window");
        return;
    };

    // Both entries bring the app forward: a Dock click on a windowless app,
    // or a launch that opened the first window. Neither `focus: true` above
    // nor macOS's reopen notification activates `NSApp` on its own, so the
    // window would otherwise stay behind whatever is frontmost.
    cx.activate(true);
}

/// Drain pi events into the UI for the life of the app.
///
/// Spawned on the [`App`], not on a window: a parked shell keeps its pi
/// process running with no window open, so the heartbeat still has to drain
/// its stream, notify a finished run, and reap idle processes. `notify()`
/// with no window open is a no-op, so the repaint half costs nothing.
fn spawn_heartbeat(cx: &mut App, app: &Entity<OrbitApp>) {
    // Idle at ~90 ms; while a run streams, poll at ~60 Hz so streamed tokens
    // land smoothly instead of in visible 90 ms batches.
    let heartbeat = app.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut delay = Duration::from_millis(90);
        loop {
            Timer::after(delay).await;
            // The shell lives in a global until the app quits, so a failure
            // here means the app is going away and there is nothing to tick.
            let Ok(active) = heartbeat.update(cx, |app: &mut OrbitApp, cx| app.tick(cx)) else {
                return;
            };
            delay = if active {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(90)
            };
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, ThemeId};
    use gpui::TestAppContext;

    /// Regression for #63: after the window closes the app is still alive, so
    /// the next window has to render the parked shell — same `OrbitApp`, same
    /// pi process — instead of building a second one.
    ///
    /// `open()` itself cannot run here: the test platform's `zoom()` is
    /// `unimplemented!()`, so a `WindowBounds::Maximized` window never opens.
    /// This covers the shell hand-off that `open()` depends on.
    #[gpui::test]
    fn reopening_reuses_the_parked_shell(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::for_id(ThemeId::Orbit));
            let first = shell(cx).entity_id();

            assert_eq!(
                shell(cx).entity_id(),
                first,
                "a second open must reuse the shell, not build another"
            );
            assert_eq!(
                cx.global::<Shell>()
                    .0
                    .borrow()
                    .as_ref()
                    .map(Entity::entity_id),
                Some(first),
                "the shell stays parked for the next window"
            );
        });
    }
}
