use super::*;
use gpui::Timer;

/// Let the session startup work settle before checking package registries.
const PLUGIN_UPDATE_CHECK_DELAY: Duration = Duration::from_secs(5);

impl OrbitApp {
    /// Check the active project's and user's installed packages in the
    /// background. The pi runtime keeps its separate existing update flow.
    pub(super) fn check_plugin_updates_on_launch(&mut self, cx: &mut Context<Self>) {
        self.start_plugin_update_check(true, cx);
    }

    /// Let the Plugins page's Refresh control also check upstream versions.
    pub(super) fn check_plugin_updates_now(&mut self, cx: &mut Context<Self>) {
        self.start_plugin_update_check(false, cx);
    }

    fn start_plugin_update_check(&mut self, delayed: bool, cx: &mut Context<Self>) {
        if self.plugin_updates_checking
            || std::env::var_os("PI_OFFLINE").is_some_and(|value| !value.is_empty())
        {
            return;
        }
        self.plugin_updates_checking = true;
        let workspace = self.workspace_dir();
        let checked_workspace = workspace.clone();
        let skipped = self.plugin_update_skips.clone();
        cx.spawn(async move |this, cx| {
            if delayed {
                Timer::after(PLUGIN_UPDATE_CHECK_DELAY).await;
            }
            let updates = cx
                .background_executor()
                .spawn(async move { crate::plugins::find_updates(&workspace, &skipped) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.plugin_updates_checking = false;
                if app.workspace_dir() == checked_workspace {
                    app.plugin_update_prompt = (!updates.is_empty()).then_some(updates);
                    app.plugin_update_workspace =
                        app.plugin_update_prompt.as_ref().map(|_| checked_workspace);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Dismiss the current notice and remember these exact releases as
    /// skipped. A newer upstream release will be offered on a later launch.
    pub(super) fn skip_plugin_updates(&mut self, cx: &mut Context<Self>) {
        let Some(updates) = self.plugin_update_prompt.take() else {
            return;
        };
        self.plugin_update_workspace = None;
        for update in updates {
            self.plugin_update_skips
                .insert(update.source, update.latest);
        }
        if let Err(error) = crate::plugins::save_skipped_updates(&self.plugin_update_skips) {
            eprintln!("Orbit plugin updates: could not persist skipped updates: {error}");
        }
        cx.notify();
    }

    /// Update every package in the notice through pi's own package manager.
    /// This only replaces package files after the user presses Update now.
    pub(super) fn apply_plugin_updates(&mut self, cx: &mut Context<Self>) {
        if self.plugin_action.is_some() {
            return;
        }
        let Some(updates) = self.plugin_update_prompt.take() else {
            return;
        };
        let workspace = self
            .plugin_update_workspace
            .take()
            .unwrap_or_else(|| self.workspace_dir());
        let names: Vec<String> = updates.iter().map(|update| update.name.clone()).collect();
        let done = tr!(
            "settings.plugin_progress_plain",
            verb = tr!("settings.verb_updating"),
            source = names.join(", ")
        );
        self.plugin_action = Some(tr!(
            "settings.plugin_progress",
            verb = tr!("settings.verb_updating"),
            source = names.join(", ")
        ));

        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    updates
                        .into_iter()
                        .map(|update| {
                            let result = crate::plugins::update(&update.source, &workspace);
                            (update, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.plugin_action = None;
                let mut updated = 0;
                let mut failures = Vec::new();
                for (update, result) in results {
                    match result {
                        Ok(_) => {
                            updated += 1;
                            app.plugin_update_skips.remove(&update.source);
                        }
                        Err(error) => failures.push(format!("{}: {error}", update.name)),
                    }
                }
                if let Err(error) = crate::plugins::save_skipped_updates(&app.plugin_update_skips) {
                    eprintln!("Orbit plugin updates: could not persist update state: {error}");
                }
                app.refresh_plugins(cx);
                if updated > 0 {
                    app.toast_success(tr!("settings.plugin_done", done = done));
                }
                if !failures.is_empty() {
                    app.set_error(failures.join("\n"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
