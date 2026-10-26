use super::*;
use crate::pi_install::{self, InstallOutcome, UnsupportedReason, UpdatePlan};
use crate::pi_update;
use gpui::Timer;

/// How long the launch check waits before touching the network, so the
/// session's own startup traffic (state, catalogs, quota) goes first.
const PI_CHECK_DELAY: Duration = Duration::from_secs(3);

/// How long the installer waits for a quiet moment before giving up. Swapping
/// pi's files under a live run can break its lazy imports, so a busy session
/// defers the update to the next launch rather than risk the turn.
const PI_QUIET_WAIT: Duration = Duration::from_secs(120);
const PI_QUIET_POLL: Duration = Duration::from_secs(5);

impl OrbitApp {
    /// Check for a newer pi at launch and apply it with the mechanism that
    /// matches how pi is installed.
    ///
    /// - Official managed install → `pi update`.
    /// - npm install → never touched silently; a migration prompt is raised
    ///   and the user must confirm on the setup page (the official installer
    ///   removes the npm package, so it is a destructive action).
    /// - Any other package manager (Nix, Homebrew, …) → reported, not touched.
    ///
    /// Quiet when pi is current, the release endpoint is unreachable, or
    /// `PI_BIN` points at a custom build — a user-managed binary is never
    /// touched. pi's own offline switch (`PI_OFFLINE`) skips the check the
    /// same way it skips pi's.
    pub(super) fn check_pi_update_on_launch(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os(orbit_rpc::PI_BIN_ENV).is_some_and(|bin| !bin.is_empty()) {
            return;
        }
        if std::env::var_os("PI_OFFLINE").is_some_and(|value| !value.is_empty()) {
            return;
        }
        // No update without an installed pi to update.
        if !self.pi_install.installed() {
            return;
        }
        let before = self.pi_install.version.clone().unwrap_or_default();

        cx.spawn(async move |this, cx| {
            Timer::after(PI_CHECK_DELAY).await;
            let (latest, installation) = cx
                .background_executor()
                .spawn(async { (pi_update::latest_version(), pi_install::detect()) })
                .await;
            // No reachable release endpoint just means no update.
            let Some(latest) = latest else { return };

            match pi_install::plan_update(&installation, Some(&latest)) {
                UpdatePlan::UpToDate => return,
                UpdatePlan::Unsupported(reason) => {
                    let _ = this.update(cx, |app, cx| {
                        app.pi_install = installation;
                        // A non-managed pi (Nix, Homebrew) is not ours to
                        // touch; say so once rather than fail every launch.
                        app.toast_info(unsupported_message(reason));
                        cx.notify();
                    });
                    return;
                }
                UpdatePlan::Migrate => {
                    let _ = this.update(cx, |app, cx| {
                        app.pi_install = installation;
                        app.pi_migration_required = Some(latest.clone());
                        app.toast_error(tr!("pi_update.migration_required", version = latest));
                        cx.notify();
                    });
                    return;
                }
                UpdatePlan::Update => {}
            }

            if this
                .update(cx, |app, cx| {
                    app.toast_info(tr!("pi_update.available_background", version = latest));
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // Install only between runs: a live session may still lazily
            // import files the package manager is about to replace.
            let mut quiet = false;
            let deadline = Instant::now() + PI_QUIET_WAIT;
            loop {
                let busy = match this.update(cx, |app, _| app.pi_busy()) {
                    Ok(busy) => busy,
                    // The app is gone; nothing left to update for.
                    Err(_) => return,
                };
                if !busy {
                    quiet = true;
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                Timer::after(PI_QUIET_POLL).await;
            }
            if !quiet {
                let _ = this.update(cx, |app, cx| {
                    app.toast_info(tr!("pi_update.available_next_launch", version = latest));
                    cx.notify();
                });
                return;
            }
            let (result, verification, deps) = cx
                .background_executor()
                .spawn(async {
                    let result = pi_install::run_pi_update();
                    let verification = pi_install::verify();
                    (result, verification, onboarding::check_dependencies())
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_pi_update(&before, result, verification, deps, cx);
            });
        })
        .detach();
    }

    /// True while any pi session (the active one, a parked one, or one still
    /// claiming its file) is running, streaming, or compacting — i.e. when
    /// pi's files must not be replaced.
    fn pi_busy(&self) -> bool {
        self.busy
            || self.transcript.is_streaming()
            || self.is_compacting
            || self.lives.values().any(|parked| parked.busy)
            || self.pending_parks.iter().any(|parked| parked.busy)
    }

    /// Apply the result of an official-installer update. Live pi processes
    /// keep the old version; only sessions started after this see the update.
    pub(super) fn finish_pi_update(
        &mut self,
        before: &str,
        result: std::io::Result<std::process::Output>,
        verification: pi_install::Verification,
        deps: Vec<onboarding::Dependency>,
        cx: &mut Context<Self>,
    ) {
        // Keep the board and the install panel truthful whatever happened.
        self.deps = deps;
        self.pi_install = verification.installation.clone();
        match result {
            Ok(output) if output.status.success() => {
                let version = verification.installation.version.clone();
                match version {
                    Some(version) if pi_update::is_newer(&version, before) => {
                        self.toast_success(update_receipt(&version, &verification));
                        if !verification.on_path() {
                            self.toast_error(tr!("pi_update.installed_not_on_path"));
                        }
                    }
                    // pi reported no change (a stale release endpoint or a
                    // managed install pi left alone): nothing to announce.
                    _ => {}
                }
            }
            Ok(output) => {
                let detail = pi_update::failure_detail(&output.stdout, &output.stderr);
                self.toast_error(tr!("pi_update.failed", detail = detail));
            }
            Err(err) => self.toast_error(tr!("pi_update.failed_detail", error = err)),
        }
        cx.notify();
    }

    /// Migrate an npm-managed pi to the official managed install. This is the
    /// click that confirms the destructive step (the official installer
    /// removes the npm package); user config, credentials, sessions, and
    /// extensions are untouched.
    pub(super) fn migrate_pi_install(&mut self, cx: &mut Context<Self>) {
        if self.pi_migrating {
            return;
        }
        // The managed installer is macOS/Linux only. Everywhere else, tell the
        // user which mechanism their platform documents instead of running
        // something that cannot work.
        if !pi_install::official_install_supported() {
            self.toast_error(tr!("pi_update.migrate_unsupported"));
            return;
        }
        // Detect before installing so a second managed install is never
        // created and a non-npm third-party install is never taken over.
        match pi_install::plan_install(&self.pi_install) {
            InstallOutcome::AlreadyManaged => {
                self.pi_migration_required = None;
                self.toast_info(tr!("pi_update.already_managed"));
                return;
            }
            InstallOutcome::Conflict(_) if !self.pi_install.kind.is_npm() => {
                self.toast_error(tr!("pi_update.unsupported_package_manager"));
                return;
            }
            InstallOutcome::Install | InstallOutcome::Conflict(_) => {}
        }
        self.pi_migrating = true;
        let before = self.pi_install.version.clone().unwrap_or_default();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let (result, verification, deps) = cx
                .background_executor()
                .spawn(async {
                    let result = pi_install::run_official_installer();
                    let verification = pi_install::verify();
                    (result, verification, onboarding::check_dependencies())
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.pi_migrating = false;
                app.deps = deps;
                app.pi_install = verification.installation.clone();
                match result {
                    Ok(output) if output.status.success() => {
                        // Verify before claiming success: the executable must
                        // now resolve to the managed install.
                        if verification.managed() {
                            app.pi_migration_required = None;
                            let version =
                                verification.installation.version.clone().unwrap_or(before);
                            app.toast_success(tr!("pi_update.migrated", version = version));
                            // A verified install can still be shadowed by an
                            // older pi earlier on PATH; surface that instead
                            // of claiming the terminal will see the new one.
                            if !verification.on_path() {
                                app.toast_error(tr!("pi_update.installed_not_on_path"));
                            }
                        } else {
                            app.toast_error(tr!("pi_update.verify_failed"));
                        }
                    }
                    Ok(output) => {
                        let detail = pi_update::failure_detail(&output.stdout, &output.stderr);
                        app.toast_error(tr!("pi_update.migrate_failed", detail = detail));
                    }
                    Err(err) => app.toast_error(tr!("pi_update.failed_detail", error = err)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// A success receipt with the resolved path, so a shadowed install (an old
/// npm shim still first on PATH) is visible rather than silently "updated".
fn update_receipt(version: &str, verification: &pi_install::Verification) -> String {
    match verification.installation.bin.as_ref() {
        Some(bin) => tr!(
            "pi_update.updated_path",
            version = version,
            path = bin.display().to_string()
        ),
        None => tr!("pi_update.updated", version = version),
    }
}

/// A translated explanation for an install Orbit will not update.
fn unsupported_message(reason: UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::PackageManager => tr!("pi_update.unsupported_package_manager"),
        UnsupportedReason::Unknown => tr!("pi_update.unsupported_unknown"),
    }
}
