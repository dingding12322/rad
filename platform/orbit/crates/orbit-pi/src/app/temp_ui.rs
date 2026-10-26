use super::helpers::*;
use super::*;
use crate::temp_files::TempEntry;
use crate::theme::tokens::ButtonSize;

/// The most file rows one visit paints. Cleanup still covers everything.
const MAX_FILE_ROWS: usize = 20;

impl OrbitApp {
    /// Settings → Privacy: Orbit's scratch files in the system temp folder,
    /// with the same open / clean affordances the session-retention section
    /// uses. The list is a snapshot; this refreshes it from disk.
    pub(super) fn refresh_temp_files(&mut self, cx: &mut Context<Self>) {
        // Visiting the page is the first place the folder becomes useful;
        // create it so "Open folder" always has a target.
        let _ = crate::temp_files::ensure_dir();
        self.temp_files = crate::temp_files::TempFiles::scan();
        cx.notify();
    }

    /// Remove the listed temp entries and repaint from a fresh scan. Entries a
    /// live Orbit process still owns are reported, never touched.
    pub(super) fn clean_temp_files(&mut self, cx: &mut Context<Self>) {
        // Re-scan first: the list was built when the page opened, so anything
        // created since then would be missed by this pass.
        self.temp_files = crate::temp_files::TempFiles::scan();
        let outcome = crate::temp_files::clean(&self.temp_files);
        self.finish_temp_cleanup(outcome, cx);
    }

    /// Remove pi's older scratch entries. Files touched within the freshness
    /// window are kept so a live run keeps its logs and image paths.
    pub(super) fn clean_ai_temp_files(&mut self, cx: &mut Context<Self>) {
        self.temp_files = crate::temp_files::TempFiles::scan();
        let outcome = crate::temp_files::clean_ai(&self.temp_files);
        self.finish_temp_cleanup(outcome, cx);
    }

    fn finish_temp_cleanup(
        &mut self,
        outcome: crate::temp_files::CleanOutcome,
        cx: &mut Context<Self>,
    ) {
        self.temp_files = crate::temp_files::TempFiles::scan();

        let mut parts = Vec::new();
        if outcome.removed > 0 {
            parts.push(tr!(
                "settings.temporary_files_removed",
                count = outcome.removed,
                size = crate::skills::format_size(outcome.bytes)
            ));
        }
        if !outcome.skipped_active.is_empty() {
            parts.push(tr!(
                "settings.temporary_files_skipped",
                count = outcome.skipped_active.len()
            ));
        }
        if !outcome.failed.is_empty() {
            parts.push(tr!(
                "settings.temporary_files_failed",
                count = outcome.failed.len()
            ));
        }
        if parts.is_empty() {
            cx.notify();
            return;
        }
        let message = parts.join(" · ");
        if outcome.failed.is_empty() && outcome.removed > 0 {
            self.toast_success(message);
        } else {
            self.toast_warning(message);
        }
        cx.notify();
    }

    pub(super) fn temporary_files_section(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let dir = self.temp_files.dir();
        let mut rows = vec![self.setting_row(
            theme,
            &tr!("settings.temporary_files_location"),
            Some(&tr!("settings.temporary_files_location_description")),
            Some(&crate::skills::display_path(dir)),
            Some(self.temporary_files_open_button(
                theme,
                "temporary-files-open",
                dir.to_path_buf(),
            )),
        )];

        if self.temp_files.is_empty() {
            rows.push(self.setting_row(
                theme,
                &tr!("settings.temporary_files_empty"),
                None,
                None,
                None,
            ));
        } else {
            for entry in self.temp_files.entries().iter().take(MAX_FILE_ROWS) {
                rows.push(self.temporary_file_row(theme, entry));
            }
            if self.temp_files.len() > MAX_FILE_ROWS {
                rows.push(self.setting_row(
                    theme,
                    &tr!(
                        "settings.temporary_files_more",
                        count = self.temp_files.len() - MAX_FILE_ROWS
                    ),
                    None,
                    None,
                    None,
                ));
            }
            rows.push(self.setting_row(
                theme,
                &tr!("settings.temporary_files_clean"),
                Some(&tr!("settings.temporary_files_clean_description")),
                Some(&tr!(
                    "settings.temporary_files_summary",
                    count = self.temp_files.len(),
                    size = crate::skills::format_size(self.temp_files.total_bytes())
                )),
                Some(self.temporary_files_clean_button(theme, this)),
            ));
        }

        self.settings_section_desc(
            theme,
            &tr!("settings.temporary_files"),
            Some(&tr!("settings.temporary_files_description")),
            rows,
        )
    }

    fn temporary_file_row(&self, theme: Theme, entry: &TempEntry) -> AnyElement {
        let size = crate::skills::format_size(entry.bytes);
        let age = crate::sessions::relative_time(entry.modified);
        let kind = if entry.is_dir {
            tr!("settings.temporary_files_kind_folder")
        } else {
            tr!("settings.temporary_files_kind_file")
        };
        let meta = if entry.active {
            format!(
                "{kind} · {size} · {age} · {}",
                tr!("settings.temporary_files_in_use")
            )
        } else {
            format!("{kind} · {size} · {age}")
        };
        self.setting_row(theme, &entry.name, None, Some(&meta), None)
    }

    /// The id is a parameter because both the Orbit folder and the temp root
    /// render one of these, and two elements sharing an `ElementId` share their
    /// hover and active state.
    fn temporary_files_open_button(
        &self,
        theme: Theme,
        id: &'static str,
        dir: PathBuf,
    ) -> AnyElement {
        button_frame(div().id(id), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .text_color(theme.text_secondary)
            .on_mouse_up(MouseButton::Left, move |_, _, _| {
                let target = crate::temp_files::ensure_dir().unwrap_or_else(|_| dir.clone());
                platform::open_path_default(&target);
            })
            .child(tr!("settings.temporary_files_open"))
            .into_any_element()
    }

    fn temporary_files_clean_button(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        button_frame(div().id("temporary-files-clean"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .text_color(theme.text_secondary)
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| app.clean_temp_files(cx));
            })
            .child(tr!("settings.temporary_files_clean"))
            .into_any_element()
    }

    /// Settings → Privacy: pi's own scratch files, kept beside Orbit's temp
    /// folder. A separate cleanup action means clearing one never surprises
    /// the other.
    pub(super) fn ai_scratch_files_section(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let root = crate::temp_files::root();
        let mut rows = vec![self.setting_row(
            theme,
            &tr!("settings.ai_scratch_files_location"),
            Some(&tr!("settings.ai_scratch_files_location_description")),
            Some(&crate::skills::display_path(&root)),
            Some(self.temporary_files_open_button(theme, "ai-scratch-open", root)),
        )];

        if self.temp_files.ai_is_empty() {
            rows.push(self.setting_row(
                theme,
                &tr!("settings.ai_scratch_files_empty"),
                None,
                None,
                None,
            ));
        } else {
            for entry in self.temp_files.ai_entries().iter().take(MAX_FILE_ROWS) {
                rows.push(self.temporary_file_row(theme, entry));
            }
            if self.temp_files.ai_len() > MAX_FILE_ROWS {
                rows.push(self.setting_row(
                    theme,
                    &tr!(
                        "settings.temporary_files_more",
                        count = self.temp_files.ai_len() - MAX_FILE_ROWS
                    ),
                    None,
                    None,
                    None,
                ));
            }
            rows.push(self.setting_row(
                theme,
                &tr!("settings.ai_scratch_files_clean"),
                Some(&tr!("settings.ai_scratch_files_clean_description")),
                Some(&tr!(
                    "settings.temporary_files_summary",
                    count = self.temp_files.ai_len(),
                    size = crate::skills::format_size(self.temp_files.ai_total_bytes())
                )),
                Some(self.ai_scratch_clean_button(theme, this)),
            ));
        }

        self.settings_section_desc(
            theme,
            &tr!("settings.ai_scratch_files"),
            Some(&tr!("settings.ai_scratch_files_description")),
            rows,
        )
    }

    fn ai_scratch_clean_button(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        button_frame(div().id("ai-scratch-clean"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .text_color(theme.text_secondary)
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| app.clean_ai_temp_files(cx));
            })
            .child(tr!("settings.ai_scratch_files_clean"))
            .into_any_element()
    }
}
