//! Settings → Skills: a master-detail manager.
//!
//! The left column lists discovered skills (searchable, grouped by scope);
//! the right column shows the selected skill's facts, actions, and rendered
//! `SKILL.md`. Enabling/disabling writes pi's own `-<path>` override; deleting
//! removes the skill directory. Discovery and file I/O live in
//! [`crate::skills`].

use super::helpers::*;
use super::*;
use crate::skills::{self, Skill, SkillScope};
use crate::theme::tokens::{input, ButtonSize, DynamicSpacing, IconSize, Radius, TextSize};

/// A Skills-page control, dispatched through one entry point.
#[derive(Clone)]
pub(super) enum SkillAction {
    Toggle,
    Open,
    Reveal,
    CopyPath,
    /// The Delete button: show the inline confirmation.
    AskDelete,
    /// The confirmation's Delete: actually remove the skill.
    ConfirmDelete,
    CancelDelete,
}

impl OrbitApp {
    // ── page ────────────────────────────────────────────────────────

    /// The full-bleed Skills surface: list column + detail column.
    pub(super) fn render_skills_page(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .bg(theme.background)
            .font_family(theme::ui_font_family())
            .child(self.skills_list(theme, this.clone(), cx))
            .child(self.skill_detail(theme, this))
            .into_any_element()
    }

    // ── list column ─────────────────────────────────────────────────

    fn skills_list(&self, theme: Theme, this: Entity<OrbitApp>, cx: &Context<Self>) -> AnyElement {
        let needle = self.skills_filter.read(cx).text().trim().to_lowercase();
        let matches = |skill: &&Skill| {
            needle.is_empty()
                || skill.name.to_lowercase().contains(&needle)
                || skill.description.to_lowercase().contains(&needle)
        };
        let filtered: Vec<&Skill> = self.skills.iter().filter(matches).collect();

        let search = picker_search_frame(div(), &theme)
            .mx(DynamicSpacing::Base12.px(&theme))
            .mt(DynamicSpacing::Base12.px(&theme))
            .mb(DynamicSpacing::Base08.px(&theme))
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.skills_filter.clone()));

        let all_row = div()
            .mx(DynamicSpacing::Base12.px(&theme))
            .mb(px(10.))
            .h(px(30.))
            .px(DynamicSpacing::Base08.px(&theme))
            .rounded(Radius::Large.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap_2()
            .child(icon(
                "icons/magic-wand.svg",
                IconSize::Small.px(&theme),
                theme.text_secondary,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!("skills_ui.all_skills")),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(self.skills.len().to_string()),
            );

        let mut list = div()
            .id("skills-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pb(DynamicSpacing::Base08.px(&theme));

        // Project first — the most local, then global agent, then user.
        for scope in [SkillScope::Project, SkillScope::Agent, SkillScope::User] {
            let group: Vec<&Skill> = filtered
                .iter()
                .copied()
                .filter(|skill| skill.scope == scope)
                .collect();
            if group.is_empty() {
                continue;
            }
            list = list.child(self.skill_group_label(scope, group.len(), theme));
            for skill in group {
                list = list.child(self.skill_row(skill, theme, this.clone()));
            }
        }

        if filtered.is_empty() {
            list = list.child(
                div()
                    .px(DynamicSpacing::Base16.px(&theme))
                    .py(px(28.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(icon(
                        "icons/search.svg",
                        IconSize::Custom(22. / 16.).px(&theme),
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("skills_ui.no_skills_match")),
                    )
                    .into_any_element(),
            );
        }

        let footer = div()
            .px(px(14.))
            .py(DynamicSpacing::Base08.px(&theme))
            .border_t_1()
            .border_color(theme.border)
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(tr!("skills_ui.skill_count", count = filtered.len()));

        div()
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(search)
            .child(all_row)
            .child(list)
            .child(footer)
            .into_any_element()
    }

    fn skill_group_label(&self, scope: SkillScope, count: usize, theme: Theme) -> AnyElement {
        div()
            .px(DynamicSpacing::Base16.px(&theme))
            .pt(px(10.))
            .pb(DynamicSpacing::Base04.px(&theme))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(scope.label().to_uppercase()),
            )
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    fn skill_row(&self, skill: &Skill, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let selected = self.selected_skill.as_deref() == Some(skill.file.as_path());
        let file = skill.file.clone();
        let description = skill.description.trim();
        div()
            .id(ElementId::Name(format!("skill-row-{}", skill.name).into()))
            .mx(px(10.))
            .px(DynamicSpacing::Base08.px(&theme))
            .py(px(7.))
            .rounded(Radius::Medium.px(&theme))
            .flex()
            .items_start()
            .gap_2p5()
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.selection))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme.surface_hover))
            })
            .when(!skill.enabled, |row| row.opacity(0.55))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| app.select_skill(file.clone(), cx));
            })
            .child(self.skill_tile(skill, selected, theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected {
                                theme.selection_fg
                            } else {
                                theme.text_primary
                            })
                            .truncate()
                            .child(skill.name.clone()),
                    )
                    .when(!description.is_empty(), |col| {
                        col.child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(description.to_string()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn skill_tile(&self, skill: &Skill, selected: bool, theme: Theme) -> AnyElement {
        let color = if selected {
            theme.text_primary
        } else if skill.enabled {
            theme.text_secondary
        } else {
            theme.text_disabled
        };
        div()
            .size(px(26.))
            .flex_none()
            .rounded(Radius::Medium.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .child(icon(
                "icons/magic-wand.svg",
                IconSize::Small.px(&theme),
                color,
            ))
            .into_any_element()
    }

    // ── detail column ───────────────────────────────────────────────

    fn skill_detail(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let selected = self
            .selected_skill
            .as_ref()
            .and_then(|path| self.skills.iter().find(|skill| &skill.file == path))
            .cloned();

        let Some(skill) = selected else {
            return div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon(
                    "icons/magic-wand.svg",
                    IconSize::Custom(30. / 16.).px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_size(TextSize::Default.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(tr!("skills_ui.select_a_skill")),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!(
                            "skills_ui.pick_one_from_the_list_to_see_its_details_and_sk"
                        )),
                )
                .into_any_element();
        };

        let header = div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .size(px(40.))
                    .flex_none()
                    .rounded(Radius::Large.px(&theme))
                    .bg(theme.surface_elevated)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(
                        "icons/magic-wand.svg",
                        IconSize::Custom(20. / 16.).px(&theme),
                        theme.text_secondary,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(theme.ui_px(17.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(skill.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(self.skill_subtitle(&skill)),
                    ),
            )
            .child(self.skill_toggle(&skill, theme, this.clone()));

        let description = if skill.description.trim().is_empty() {
            tr!("skills_ui.no_description")
        } else {
            skill.description.clone()
        };
        let description_color = if skill.is_valid() {
            theme.text_secondary
        } else {
            theme.error
        };

        let facts = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(self.skill_fact(
                theme,
                "Invoke",
                self.skill_value(theme, &skill.invoke_command()),
            ))
            .child(self.skill_fact(
                theme,
                "Location",
                self.skill_value(
                    theme,
                    &skills::display_path(skill.dir().unwrap_or(&skill.file)),
                ),
            ))
            .child(self.skill_fact(
                theme,
                "Contents",
                self.skill_value(theme, &skills::format_size(skill.size_bytes)),
            ))
            .child(self.skill_fact(
                theme,
                "Updated",
                self.skill_value(theme, &skill_age(&skill)),
            ));

        let actions = self.skill_actions(&skill, theme, this.clone());

        let content: AnyElement = match &self.skill_content {
            Some(body) if !body.trim().is_empty() => div()
                .id("skill-content")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(div().w_full().max_w(px(760.)).child(
                    crate::transcript_view::render_markdown_document(body, theme),
                ))
                .into_any_element(),
            _ => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(tr!("skills_ui.skill_md_is_empty")),
                )
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .px(DynamicSpacing::Base24.px(&theme))
            .pt(px(18.))
            .pb(DynamicSpacing::Base12.px(&theme))
            .gap_3()
            .child(header)
            .child(
                div()
                    .max_w(px(760.))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(description_color)
                    .child(description),
            )
            .child(facts)
            .child(actions)
            .child(div().w_full().h(px(1.)).bg(theme.border))
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(tr!("skills_ui.skill_md")),
            )
            .child(content)
            .into_any_element()
    }

    fn skill_subtitle(&self, skill: &Skill) -> String {
        let scope = match skill.scope {
            SkillScope::Project => {
                let workspace = self.workspace_dir();
                tr!(
                    "skills_ui.scope_project",
                    path = skills::display_path(&workspace)
                )
            }
            SkillScope::Agent => tr!("skills_ui.scope_agent"),
            SkillScope::User => tr!("skills_ui.scope_user"),
        };
        if skill.manual_only {
            tr!("skills_ui.scope_manual_only", scope = scope)
        } else {
            scope
        }
    }

    fn skill_fact(&self, theme: Theme, label: &str, value: AnyElement) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(label.to_string()),
            )
            .child(div().flex_1().min_w_0().child(value))
            .into_any_element()
    }

    fn skill_value(&self, theme: Theme, value: &str) -> AnyElement {
        div()
            .min_w_0()
            .truncate()
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_primary)
            .child(value.to_string())
            .into_any_element()
    }

    fn skill_toggle(&self, skill: &Skill, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let on = skill.enabled;
        div()
            .id("skill-toggle")
            .w(px(36.))
            .h(px(20.))
            .rounded_full()
            .p(px(2.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(on, |track| track.bg(theme.accent).justify_end())
            .when(!on, |track| {
                track.bg(theme.surface_elevated).justify_start()
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| {
                    app.apply_skill_action(SkillAction::Toggle, cx)
                });
            })
            .child(div().size(px(14.)).rounded_full().bg(theme.toggle_knob))
            .into_any_element()
    }

    fn skill_actions(&self, skill: &Skill, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let confirming = self
            .skill_delete_confirm
            .as_deref()
            .is_some_and(|dir| Some(dir) == skill.dir());

        if confirming {
            return div()
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.error)
                        .child(tr!(
                            "skills_ui.delete_skill_hint",
                            path = skills::display_path(skill.dir().unwrap_or(&skill.file))
                        )),
                )
                .child(self.skill_button(
                    "skill-delete-confirm".into(),
                    &tr!("sidebar.delete"),
                    true,
                    Some("icons/trash.svg"),
                    theme,
                    this.clone(),
                    SkillAction::ConfirmDelete,
                ))
                .child(self.skill_button(
                    "skill-delete-cancel".into(),
                    &tr!("git_panel.cancel"),
                    false,
                    None,
                    theme,
                    this,
                    SkillAction::CancelDelete,
                ))
                .into_any_element();
        }

        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .child(self.skill_button(
                "skill-open".into(),
                &tr!("skills_ui.open_skill_md"),
                false,
                Some("icons/file.svg"),
                theme,
                this.clone(),
                SkillAction::Open,
            ))
            .child(self.skill_button(
                "skill-reveal".into(),
                &tr!("skills_ui.show_in_file_manager"),
                false,
                Some("icons/folder.svg"),
                theme,
                this.clone(),
                SkillAction::Reveal,
            ))
            .child(self.skill_button(
                "skill-copy-path".into(),
                &tr!("sidebar.copy_path"),
                false,
                Some("icons/copy.svg"),
                theme,
                this.clone(),
                SkillAction::CopyPath,
            ))
            .child(div().flex_1())
            .child(self.skill_button(
                "skill-delete".into(),
                &tr!("sidebar.delete"),
                false,
                Some("icons/trash.svg"),
                theme,
                this,
                SkillAction::AskDelete,
            ))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn skill_button(
        &self,
        id: String,
        label: &str,
        danger: bool,
        icon_path: Option<&'static str>,
        theme: Theme,
        this: Entity<OrbitApp>,
        action: SkillAction,
    ) -> AnyElement {
        let base = button_frame(
            div().id(ElementId::Name(id.into())),
            &theme,
            ButtonSize::Medium,
        )
        .group(BUTTON_GROUP)
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM);
        let (button, icon_color) = if danger {
            (
                base.border_1()
                    .border_color(theme.border)
                    .raised(theme.surface_elevated, &theme)
                    .text_color(theme.error)
                    .hover(|style| {
                        style
                            .border_color(theme.error.opacity(0.6))
                            .raised(theme.error.opacity(0.08), &theme)
                    }),
                theme.error,
            )
        } else {
            (
                base.border_1()
                    .border_color(theme.border)
                    .raised(theme.surface_elevated, &theme)
                    .text_color(theme.text_secondary)
                    .hover(|style| style.raised(theme.surface_hover, &theme)),
                theme.text_secondary,
            )
        };
        press(button)
            .when_some(icon_path, |button, path| {
                button.child(icon(
                    path,
                    ButtonSize::Medium.icon_size().px(&theme),
                    icon_color,
                ))
            })
            .child(div().child(label.to_string()))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                let action = action.clone();
                this.update(cx, |app, cx| app.apply_skill_action(action, cx));
            })
            .into_any_element()
    }

    // ── controller ──────────────────────────────────────────────────

    /// Re-scan every skill root and reload the selected skill's body.
    pub(super) fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        self.skills = skills::discover(&self.workspace_dir());
        let still_there = self
            .selected_skill
            .as_ref()
            .is_some_and(|path| self.skills.iter().any(|skill| &skill.file == path));
        if !still_there {
            self.selected_skill = self.skills.first().map(|skill| skill.file.clone());
        }
        self.skill_delete_confirm = None;
        self.load_selected_skill();
        cx.notify();
    }

    /// Select a skill and cache its rendered body.
    pub(super) fn select_skill(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_skill = Some(path);
        self.skill_delete_confirm = None;
        self.load_selected_skill();
        cx.notify();
    }

    fn load_selected_skill(&mut self) {
        self.skill_content = self
            .selected_skill
            .as_ref()
            .and_then(|path| skills::read_body(path).ok());
    }

    /// Single dispatch point for every Skills-page control.
    pub(super) fn apply_skill_action(&mut self, action: SkillAction, cx: &mut Context<Self>) {
        let selected = self.selected_skill.clone();
        match action {
            SkillAction::Toggle => {
                let Some(path) = selected else { return };
                let Some(skill) = self.skills.iter().find(|skill| skill.file == path).cloned()
                else {
                    return;
                };
                match skills::set_enabled(&skill, &self.workspace_dir(), !skill.enabled) {
                    Ok(()) => {
                        self.set_status(if skill.enabled {
                            tr!("skills_ui.disabled_skill", name = skill.name)
                        } else {
                            tr!("skills_ui.enabled_skill", name = skill.name)
                        });
                        self.refresh_skills(cx);
                    }
                    Err(err) => self.set_error(err),
                }
            }
            SkillAction::Open => {
                if let Some(path) = selected {
                    platform::open_path_default(&path);
                }
            }
            SkillAction::Reveal => {
                if let Some(path) = selected {
                    platform::reveal_in_file_manager(&path);
                }
            }
            SkillAction::CopyPath => {
                if let Some(path) = selected {
                    let text = skills::display_path(&path);
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    self.toast_success(tr!("skills_ui.copied_path", path = text));
                }
            }
            SkillAction::AskDelete => {
                if let Some(path) = selected {
                    self.skill_delete_confirm = path.parent().map(Path::to_path_buf);
                }
            }
            SkillAction::ConfirmDelete => self.delete_selected_skill(cx),
            SkillAction::CancelDelete => self.skill_delete_confirm = None,
        }
        cx.notify();
    }

    /// Remove the selected skill's directory, then reload the list.
    fn delete_selected_skill(&mut self, cx: &mut Context<Self>) {
        let Some(skill) = self
            .selected_skill
            .clone()
            .and_then(|path| self.skills.iter().find(|skill| skill.file == path).cloned())
        else {
            return;
        };
        match skills::delete(&skill, &self.workspace_dir()) {
            Ok(()) => {
                self.selected_skill = None;
                self.toast_info(tr!("skills_ui.deleted_skill", name = skill.name));
                self.refresh_skills(cx);
            }
            Err(err) => self.set_error(err),
        }
    }
}

/// "48d ago" for the skill's `SKILL.md`.
fn skill_age(skill: &Skill) -> String {
    match skill.modified {
        Some(modified) => tr!("skills_ui.n_ago", time = sessions::relative_time(modified)),
        None => tr!("skills_ui.unknown"),
    }
}
