use super::*;

impl OrbitApp {
    /// Run the retention policy once per launch — the auto-run path. The
    /// latch makes later heartbeats a no-op, and an opt-out policy never
    /// runs. A clean match reports nothing.
    pub(super) fn run_session_retention_once(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.retention_ran, true) {
            return;
        }
        if self.retention.enabled {
            self.run_session_retention(cx);
        }
    }

    /// Apply the retention policy now and report the result. Shared by the
    /// launch pass and Settings → Privacy's "Clean up now".
    pub(super) fn run_session_retention(&mut self, cx: &mut Context<Self>) {
        let live: HashSet<PathBuf> = self.lives.keys().cloned().collect();
        let outcome = crate::session_retention::run(&self.retention, &live, SystemTime::now());
        if !outcome.changed() {
            return;
        }
        self.sessions = sessions::load_sessions();
        self.prune_workflow_store();
        // Deleted files drop out of the usage index; archived ones stay. Mark
        // it stale so the page rebuilds on its next visit.
        if self.usage_open {
            self.usage.update(cx, |page, cx| page.mark_stale(cx));
        }
        self.toast_session_retention(&outcome);
        cx.notify();
    }

    /// One compact toast for a retention pass: what moved, what was skipped
    /// for an in-use process, and anything that failed.
    fn toast_session_retention(&mut self, outcome: &crate::session_retention::RetentionOutcome) {
        let mut parts = Vec::new();
        if outcome.archived > 0 {
            parts.push(tr!(
                "settings.session_retention_archived",
                count = outcome.archived
            ));
        }
        if outcome.deleted > 0 {
            parts.push(tr!(
                "settings.session_retention_deleted",
                count = outcome.deleted
            ));
        }
        if !outcome.skipped_live.is_empty() {
            parts.push(tr!(
                "settings.session_retention_skipped",
                count = outcome.skipped_live.len()
            ));
        }
        if !outcome.failed.is_empty() {
            parts.push(tr!(
                "settings.session_retention_failed",
                count = outcome.failed.len()
            ));
        }
        if parts.is_empty() {
            return;
        }
        let message = parts.join(" · ");
        if outcome.failed.is_empty() {
            self.toast_success(message);
        } else {
            self.toast_warning(message);
        }
    }
}
