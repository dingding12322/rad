use super::helpers::*;
use super::*;
use crate::quota::note_should_render;
use crate::theme::tokens::{
    context_menu, input, modal, picker, popover, BufferLineHeight, ButtonSize, DynamicSpacing,
    IconSize, Radius, StyledExt, TextSize, BORDER_WIDTH,
};
use gpui::{canvas, Bounds, DispatchPhase, Hitbox, HitboxBehavior, MouseMoveEvent};

/// A Plugins-page button action, dispatched through one entry point.
#[derive(Clone)]
pub(super) enum PluginAction {
    Install,
    Update { source: String },
    Remove { source: String },
    ConfirmRemove { source: String, project: bool },
    CancelRemove,
    SetScope { project: bool },
    Refresh,
}

/// Toolbar vs. card button geometry. Settings' pinned toolbars size their
/// controls to `ButtonSize::Large`, the row Providers and Models use; card
/// actions stay compact at `ButtonSize::Medium`.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PluginButtonSize {
    Compact,
    Toolbar,
}

/// How long the Plugins Refresh button turns after a click, so a synchronous
/// reload still reads as a deliberate action.
const PLUGIN_REFRESH_FEEDBACK: Duration = Duration::from_millis(650);

/// The settings select chip's size. Its dropdown is offset by this height
/// plus `popover::MENU_OFFSET`, so the two must move together.
const SELECT_CHIP_SIZE: ButtonSize = ButtonSize::Medium;

/// Backdrop blur slider geometry: the painted track width and handle
/// diameter. The drag math reads the same pair, so the handle always lands
/// under the pointer rather than a few pixels off it.
const BLUR_TRACK_W: f32 = 184.;
const BLUR_KNOB: f32 = 14.;

/// The settings nav in display order: section, nav icon, and label key.
/// Grouped so the app's own preferences lead (General / Appearance /
/// Shortcuts), the agent and its resources follow (Runtime / Agent /
/// Providers / Models), the extensions pi loads close the list (Skills /
/// Plugins / MCP), and About and Report a bug sit alone at the end.
const SETTINGS_SECTIONS: &[(SettingsSection, &str, &str)] = &[
    (
        SettingsSection::General,
        "icons/settings.svg",
        "settings.general",
    ),
    (
        SettingsSection::Appearance,
        "icons/contrast.svg",
        "settings.appearance",
    ),
    (
        SettingsSection::Shortcuts,
        "icons/keyboard.svg",
        "settings.shortcuts",
    ),
    (
        SettingsSection::Privacy,
        "icons/lock.svg",
        "settings.privacy",
    ),
    (
        SettingsSection::Runtime,
        "icons/server-stack.svg",
        "settings.runtime",
    ),
    (SettingsSection::Agent, "icons/spark.svg", "settings.agent"),
    (
        SettingsSection::Worktrees,
        "icons/branch.svg",
        "worktree.settings.title",
    ),
    (
        SettingsSection::Providers,
        "icons/cloud.svg",
        "settings.providers",
    ),
    (
        SettingsSection::Models,
        "icons/tag-01.svg",
        "settings.models",
    ),
    (
        SettingsSection::Skills,
        "icons/magic-wand.svg",
        "settings.skills",
    ),
    (
        SettingsSection::Plugins,
        "icons/extensions.svg",
        "settings.plugins",
    ),
    (SettingsSection::Mcp, "icons/tools/mcp.svg", "settings.mcp"),
    (SettingsSection::About, "icons/info.svg", "settings.about"),
    (
        SettingsSection::ReportBug,
        "icons/github.svg",
        "settings.report_bug",
    ),
];

/// Every searchable setting, flattened: section, label key, and the extra
/// terms a query can match. The label is translated when rendered; the
/// keywords are the synonyms people type that the label does not spell out.
/// It lives beside the nav so a new section has one obvious place to
/// register its searchable rows.
const SETTINGS_SEARCH_INDEX: &[(SettingsSection, &str, &str)] = &[
    // General
    (
        SettingsSection::General,
        "settings.pi_agent",
        "pi agent process stdio connection child spawn",
    ),
    (
        SettingsSection::General,
        "settings.local_by_default",
        "local data store sessions disk offline",
    ),
    (
        SettingsSection::General,
        "settings.workspace",
        "workspace directory folder path project",
    ),
    (
        SettingsSection::General,
        "updater_ui.automatic_updates",
        "updates update automatic release",
    ),
    (
        SettingsSection::General,
        "updater_ui.check_for_updates",
        "updates update check release version",
    ),
    (
        SettingsSection::General,
        "settings.desktop_notifications",
        "notifications banner desktop system",
    ),
    (
        SettingsSection::General,
        "settings.in_app_toasts",
        "notifications toast in-app",
    ),
    (
        SettingsSection::General,
        "settings.notification_sound",
        "notifications sound alert audio",
    ),
    // Runtime
    (
        SettingsSection::Runtime,
        "settings.status",
        "runtime process status start stop restart",
    ),
    (
        SettingsSection::Runtime,
        "settings.process_id",
        "runtime process id pid",
    ),
    (
        SettingsSection::Runtime,
        "settings.binary",
        "runtime binary executable pi",
    ),
    (
        SettingsSection::Runtime,
        "settings.uptime",
        "runtime uptime elapsed",
    ),
    (
        SettingsSection::Runtime,
        "settings.transport",
        "runtime transport stdio",
    ),
    (
        SettingsSection::Runtime,
        "settings.last_error",
        "runtime error stderr last",
    ),
    (
        SettingsSection::Runtime,
        "settings.parked_processes",
        "runtime background parked sessions",
    ),
    (
        SettingsSection::Runtime,
        "settings.recent_stderr",
        "runtime stderr log output console",
    ),
    // Agent
    (
        SettingsSection::Agent,
        "settings.enter_while_running",
        "enter send submit queue steer behavior",
    ),
    (
        SettingsSection::Agent,
        "settings.follow_up_messages",
        "follow-up queue messages behavior",
    ),
    (
        SettingsSection::Agent,
        "settings.auto_compaction",
        "compaction compact context auto",
    ),
    (
        SettingsSection::Agent,
        "settings.auto_retry",
        "retry auto error transient",
    ),
    (
        SettingsSection::Agent,
        "settings.auto_session_titles",
        "titles session name auto",
    ),
    (
        SettingsSection::Agent,
        "settings.title_model",
        "title model session names",
    ),
    (
        SettingsSection::Agent,
        "settings.default_model",
        "default model session new",
    ),
    (
        SettingsSection::Agent,
        "settings.default_thinking",
        "thinking level default session reasoning",
    ),
    // Worktrees
    (
        SettingsSection::Worktrees,
        "worktree.settings.directory",
        "worktree worktrees directory location .wt folder path git branch",
    ),
    (
        SettingsSection::Worktrees,
        "worktree.settings.setup_script",
        "worktree worktrees setup script install bootstrap hook",
    ),
    (
        SettingsSection::Worktrees,
        "worktree.settings.run_setup_auto",
        "worktree worktrees setup script automatic auto run",
    ),
    (
        SettingsSection::Worktrees,
        "worktree.settings.open_after_create",
        "worktree worktrees open after create switch workspace",
    ),
    // Appearance
    (
        SettingsSection::Appearance,
        "settings.appearance",
        "appearance mode light dark system theme",
    ),
    (
        SettingsSection::Appearance,
        "settings.theme",
        "theme palette color light dark editor",
    ),
    (
        SettingsSection::Appearance,
        "settings.background_image",
        "background image backdrop dither picture",
    ),
    (
        SettingsSection::Appearance,
        "settings.blur",
        "background blur backdrop soft",
    ),
    (
        SettingsSection::Appearance,
        "settings.pixel_size",
        "background pixel size backdrop dither cell",
    ),
    (
        SettingsSection::Appearance,
        "settings.bottom_fade",
        "background fade backdrop gradient",
    ),
    (
        SettingsSection::Appearance,
        "language.title",
        "language locale translation interface",
    ),
    (
        SettingsSection::Appearance,
        "settings.interface_font",
        "font interface ui typeface",
    ),
    (
        SettingsSection::Appearance,
        "settings.code_font",
        "font code mono typeface",
    ),
    (
        SettingsSection::Appearance,
        "settings.ui_font_size",
        "font size ui text scale",
    ),
    (
        SettingsSection::Appearance,
        "settings.terminal_size",
        "font size terminal",
    ),
    (
        SettingsSection::Appearance,
        "settings.editor_size",
        "font size editor",
    ),
    (
        SettingsSection::Appearance,
        "settings.spacing_density",
        "spacing density compact comfortable",
    ),
    (
        SettingsSection::Appearance,
        "settings.show_sidebar",
        "sidebar show layout navigation",
    ),
    (
        SettingsSection::Appearance,
        "settings.reduce_motion",
        "motion animation reduce layout",
    ),
    // Resources and extensions
    (
        SettingsSection::Skills,
        "settings.skills",
        "skills skill.md instructions agent",
    ),
    (
        SettingsSection::Plugins,
        "settings.plugins",
        "plugins packages extensions install npm git",
    ),
    (
        SettingsSection::Mcp,
        "settings.mcp",
        "mcp model context protocol servers tools",
    ),
    (
        SettingsSection::Models,
        "settings.models",
        "models catalog favorites providers",
    ),
    (
        SettingsSection::Providers,
        "settings.providers",
        "providers api key auth credentials",
    ),
    (
        SettingsSection::Shortcuts,
        "settings.shortcuts",
        "shortcuts keyboard keys chords",
    ),
    (
        SettingsSection::Privacy,
        "settings.privacy",
        "privacy session retention cleanup local data",
    ),
    (
        SettingsSection::Privacy,
        "settings.temporary_files",
        "temporary temp cache scratch files cleanup remove folder",
    ),
    (
        SettingsSection::Privacy,
        "settings.ai_scratch_files",
        "ai pi agent scratch logs codemode images share editor cleanup temp",
    ),
    (
        SettingsSection::About,
        "settings.about",
        "about version updates source github",
    ),
    (
        SettingsSection::ReportBug,
        "settings.report_bug",
        "bug report issue crash problem broken github feedback",
    ),
    (
        SettingsSection::ReportBug,
        "bug_report.heading",
        "bug report issue title description steps reproduce diagnostics",
    ),
];

/// The nav icon and label key for a section, from [`SETTINGS_SECTIONS`].
fn settings_section_meta(section: SettingsSection) -> (&'static str, &'static str) {
    SETTINGS_SECTIONS
        .iter()
        .find(|(candidate, _, _)| *candidate == section)
        .map(|&(_, icon, label_key)| (icon, label_key))
        .unwrap_or(("icons/settings.svg", "settings.general"))
}

/// Settings whose translated label or keyword list contains the already
/// lowercased `needle`, in index order. An empty needle matches nothing —
/// callers only mount results while a query is active.
fn settings_search_hits(needle: &str) -> Vec<(SettingsSection, &'static str, &'static str)> {
    if needle.is_empty() {
        return Vec::new();
    }
    SETTINGS_SEARCH_INDEX
        .iter()
        .filter(|(_, label_key, keywords)| {
            tr!(*label_key).to_lowercase().contains(needle) || keywords.contains(needle)
        })
        .copied()
        .collect()
}

/// The operation a background plugin task runs.
#[derive(Clone, Copy)]
enum PluginOp {
    Install,
    Update,
    Remove,
}

impl OrbitApp {
    // ── settings surface ───────────────────────────────────────────
    // Left nav (Back + sections), right column of setting
    // rows. Every control maps to real app state; read-only rows show
    // real pi/runtime facts (PRODUCT.md: nothing decorative that
    // pretends to be functional).

    pub(super) fn render_settings(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity();
        let theme = *theme::get(cx);
        let sections = SETTINGS_SECTIONS;

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .relative()
            .bg(theme.background)
            .font_family(theme::ui_font_family())
            // ── nav column ──
            .child(
                div()
                    .w(px(240.))
                    .h_full()
                    .flex_shrink_0()
                    .bg(theme.surface)
                    .border_r_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    // Window drag strip — same height as the session
                    // sidebar's so the two nav columns line up. It holds the
                    // macOS traffic lights inside the transparent titlebar;
                    // elsewhere it is simply how the column can be dragged.
                    .child(window_drag_region(
                        div().h(px(super::view::TOP_BAR_H)).w_full(),
                    ))
                    // Back — same 28px row language as the section list.
                    // The list below sits in an 8px container with 8px row
                    // insets, so the icon column lands at 16px here, on the
                    // eyebrow, and in the footer: one left edge, read once.
                    .child(
                        div()
                            .px(DynamicSpacing::Base08.px(&theme))
                            .pt(DynamicSpacing::Base04.px(&theme))
                            .pb(DynamicSpacing::Base12.px(&theme))
                            .child(
                                press(
                                    div()
                                        .id("settings-back")
                                        .w_full()
                                        .h(ButtonSize::Medium.height(&theme))
                                        .px(DynamicSpacing::Base08.px(&theme))
                                        .rounded(Radius::Large.px(&theme))
                                        .flex()
                                        .items_center()
                                        .gap(DynamicSpacing::Base08.px(&theme))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.surface_hover)),
                                )
                                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_settings_back))
                                .child(icon(
                                    "icons/arrow-left.svg",
                                    IconSize::Small.px(&theme),
                                    theme.text_muted,
                                ))
                                .child(
                                    div()
                                        .text_size(TextSize::Default.px(&theme))
                                        .text_color(theme.text_secondary)
                                        .child(tr!("settings.back")),
                                ),
                            ),
                    )
                    // Search — the first control in the column, just above
                    // the list it reaches into, so a query is one click from
                    // any setting in any section.
                    .child(self.settings_search_bar(theme))
                    // Eyebrow: a small caps label gives the flat list a
                    // heading to read under, the way a sidebar in a
                    // crafted app does. No accent marker — the accent is
                    // reserved for data emphasis, not chrome. Inset matches
                    // the rows' icon column (8px container + 8px row pad).
                    .child(
                        div()
                            .pl(DynamicSpacing::Base16.px(&theme))
                            .pb(DynamicSpacing::Base06.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::XSmall.px(&theme))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_muted)
                                    .child(tr!("settings.settings")),
                            ),
                    )
                    // Section rows — 28px pills with a 2px rhythm (the
                    // density Linear uses for flat nav lists), grouped by
                    // what they belong to: the app (General / Appearance /
                    // Shortcuts), the agent and its resources (Runtime /
                    // Agent / Providers / Models), the extensions it loads
                    // (Skills / Plugins / MCP), and About on its own. Groups
                    // are set off by a hairline with 8px of air on each side
                    // — 16px of separation total, so the groups read as
                    // set-off but related. Selection is a single low-opacity
                    // accent wash — no bar, no weight change — with the icon
                    // and ink stepping toward the accent so the row reads as
                    // one quiet highlight, never a button that took over.
                    .child(
                        div()
                            .px(DynamicSpacing::Base08.px(&theme))
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base02.px(&theme))
                            .children(sections.iter().map(
                                |&(section, section_icon, label_key)| {
                                    let this = this.clone();
                                    let label = tr!(label_key).to_string();
                                    let selected = self.settings_section == section;
                                    // Group starts: Runtime opens the agent
                                    // block, Skills opens the extensions
                                    // block, and About closes the list alone.
                                    let group_start = matches!(
                                        section,
                                        SettingsSection::Runtime
                                            | SettingsSection::Skills
                                            | SettingsSection::About
                                            | SettingsSection::ReportBug
                                    );
                                    let row = div()
                                        .id(ElementId::Name(format!("settings-nav-{label}").into()))
                                        .w_full()
                                        .h(ButtonSize::Medium.height(&theme))
                                        .px(DynamicSpacing::Base08.px(&theme))
                                        .rounded(Radius::Medium.px(&theme))
                                        .text_size(TextSize::Default.px(&theme))
                                        .flex()
                                        .items_center()
                                        .gap(DynamicSpacing::Base08.px(&theme))
                                        .cursor_pointer()
                                        .when(selected, |row| {
                                            row.bg(theme.selection).hover(|s| s.bg(theme.selection))
                                        })
                                        .when(!selected, |row| {
                                            row.hover(|s| s.bg(theme.surface_hover))
                                        })
                                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                                            this.update(cx, |app, cx| {
                                                app.set_settings_section(section, cx);
                                            });
                                        })
                                        .child(icon(
                                            section_icon,
                                            IconSize::Small.px(&theme),
                                            if selected {
                                                theme.accent_text
                                            } else {
                                                theme.text_muted
                                            },
                                        ))
                                        .child(
                                            div()
                                                .text_color(if selected {
                                                    theme.selection_fg
                                                } else {
                                                    theme.text_secondary
                                                })
                                                .child(label.to_string()),
                                        );
                                    if group_start {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(DynamicSpacing::Base02.px(&theme))
                                            .pt(DynamicSpacing::Base08.px(&theme))
                                            .mt(DynamicSpacing::Base06.px(&theme))
                                            .border_t_1()
                                            .border_color(theme.border)
                                            .child(row)
                                            .into_any_element()
                                    } else {
                                        row.into_any_element()
                                    }
                                },
                            )),
                    )
                    // Footer: the build identity, pinned bottom-left in the
                    // same tertiary register the About page uses — a quiet
                    // closer for the column. A hairline caps the scrollable
                    // area above it, and the left inset keeps the icon
                    // column's 16px edge.
                    .child(
                        div()
                            .mt_auto()
                            .w_full()
                            .px(DynamicSpacing::Base16.px(&theme))
                            .py(DynamicSpacing::Base12.px(&theme))
                            .border_t_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_muted)
                                    .child(tr!("app.name")),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .font(crate::usage::view::num_font())
                                    .text_color(theme.text_muted)
                                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                            ),
                    ),
            )
            // ── content column ──
            // While the sidebar search carries a query the column shows
            // matches instead of the active section. Skills owns a full-bleed
            // master-detail surface; every other section is a fixed header + a
            // scrolling column of cards.
            .child({
                let searching = !self.settings_search.read(cx).text().trim().is_empty();
                if searching {
                    self.settings_search_results(theme, this.clone(), cx)
                } else if self.settings_section == SettingsSection::Skills {
                    self.render_skills_page(theme, this.clone(), cx)
                } else {
                    self.settings_body(theme, this.clone(), cx)
                }
            })
            // ── provider editor modals (models.json + API key) ──
            .children(self.provider_editor_layer(theme, this.clone(), cx))
            .children(self.provider_usage_layer(theme, this.clone(), cx))
            // ── MCP add/edit modal ──
            .children(self.mcp_editor_layer(theme, this.clone(), cx))
            .children(self.provider_key_layer(theme, this, cx))
    }

    /// The standard settings content column: a pinned header (title +
    /// section toolbar) over a scrolling body of cards.
    fn settings_body(
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
            .flex_col()
            // The title (and, on Providers/Plugins, the toolbar) stay pinned;
            // gpui has no sticky positioning, so they live outside the scroll
            // container.
            .child(
                div()
                    .w_full()
                    .max_w(px(CONTENT_MAX_W))
                    .mx_auto()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .px(DynamicSpacing::Base24.px(&theme))
                            .pt(DynamicSpacing::Base20.px(&theme))
                            .pb(DynamicSpacing::Base12.px(&theme))
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base12.px(&theme))
                            .child(self.settings_header(theme))
                            .children(self.settings_toolbar(theme, this.clone(), cx)),
                    )
                    .child(
                        div()
                            .w_full()
                            .px(DynamicSpacing::Base24.px(&theme))
                            .child(div().w_full().h(BORDER_WIDTH).bg(theme.border)),
                    ),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(px(CONTENT_MAX_W))
                    .mx_auto()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .px(DynamicSpacing::Base24.px(&theme))
                            .pt(DynamicSpacing::Base20.px(&theme))
                            .pb(DynamicSpacing::Base24.px(&theme))
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base20.px(&theme))
                            .children(self.error_banner(theme, cx))
                            .children(self.settings_rows(&this, theme, cx)),
                    ),
            )
            .into_any_element()
    }

    /// The global settings search, at the top of the nav column so a query
    /// can reach any setting without hunting through the section list.
    fn settings_search_bar(&self, theme: Theme) -> AnyElement {
        let search = picker_search_frame(div(), &theme)
            .w_full()
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.settings_search.clone()));
        div()
            .w_full()
            .flex_shrink_0()
            .px(DynamicSpacing::Base08.px(&theme))
            .pb(DynamicSpacing::Base12.px(&theme))
            .child(search)
            .into_any_element()
    }

    /// The search results: every setting whose label or keywords match,
    /// grouped as one board. A row opens its section and clears the query, so
    /// the section it points at is visible immediately.
    fn settings_search_results(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let needle = self.settings_search.read(cx).text().trim().to_lowercase();
        let hits = settings_search_hits(&needle);
        let body: AnyElement = if hits.is_empty() {
            self.empty_resource_card(
                theme,
                "icons/search.svg",
                &tr!("settings.no_matches"),
                &tr!("settings.try_a_different_search"),
            )
        } else {
            let rows = hits
                .into_iter()
                .map(|(section, label_key, _)| {
                    self.settings_search_row(section, label_key, theme, this.clone())
                })
                .collect();
            self.settings_group(theme, rows)
        };
        div()
            .id("settings-search-results")
            .flex_1()
            .min_h_0()
            .w_full()
            .max_w(px(CONTENT_MAX_W))
            .mx_auto()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .px(DynamicSpacing::Base24.px(&theme))
                    .pt(DynamicSpacing::Base20.px(&theme))
                    .pb(DynamicSpacing::Base24.px(&theme))
                    .child(body),
            )
            .into_any_element()
    }

    /// One search result: the setting's label leading, its section trailing,
    /// with the section's nav icon at the start of the row.
    fn settings_search_row(
        &self,
        section: SettingsSection,
        label_key: &'static str,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let (section_icon, section_key) = settings_section_meta(section);
        press(
            div()
                .id(ElementId::Name(
                    format!("settings-result-{label_key}").into(),
                ))
                .w_full()
                .px(DynamicSpacing::Base16.px(&theme))
                .py(DynamicSpacing::Base12.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base12.px(&theme))
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_hover)),
        )
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| {
                app.settings_search.update(cx, |input, cx| input.clear(cx));
                app.set_settings_section(section, cx);
            });
        })
        .child(icon(
            section_icon,
            IconSize::Small.px(&theme),
            theme.text_muted,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(TextSize::Default.px(&theme))
                .text_color(theme.text_primary)
                .child(tr!(label_key).to_string()),
        )
        .child(
            div()
                .flex_none()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!(section_key).to_string()),
        )
        .into_any_element()
    }

    pub(super) fn settings_header(&self, theme: Theme) -> impl IntoElement + use<> {
        let (title, subtitle): (String, String) = match self.settings_section {
            SettingsSection::General => {
                (tr!("settings.general"), tr!("settings.general_description"))
            }
            SettingsSection::Runtime => {
                (tr!("settings.runtime"), tr!("settings.runtime_description"))
            }
            SettingsSection::Agent => (tr!("settings.agent"), tr!("settings.agent_description")),
            SettingsSection::Worktrees => (
                tr!("worktree.settings.title"),
                tr!("worktree.settings.description"),
            ),
            SettingsSection::Skills => (tr!("settings.skills"), tr!("settings.skills_description")),
            SettingsSection::Plugins => {
                (tr!("settings.plugins"), tr!("settings.plugins_description"))
            }
            SettingsSection::Mcp => (tr!("settings.mcp"), tr!("settings.mcp_description")),
            SettingsSection::Models => (tr!("settings.models"), tr!("settings.models_description")),
            SettingsSection::Appearance => (
                tr!("settings.appearance"),
                tr!("settings.appearance_description"),
            ),
            SettingsSection::Providers => (
                tr!("settings.providers"),
                tr!("settings.providers_description"),
            ),
            SettingsSection::Shortcuts => (
                tr!("settings.shortcuts"),
                tr!("settings.shortcuts_description"),
            ),
            SettingsSection::Privacy => {
                (tr!("settings.privacy"), tr!("settings.privacy_description"))
            }
            SettingsSection::About => (tr!("settings.about"), tr!("settings.about_description")),
            SettingsSection::ReportBug => (
                tr!("settings.report_bug"),
                tr!("settings.report_bug_description"),
            ),
        };
        div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme))
            .when(self.settings_section == SettingsSection::About, |header| {
                header.child(embedded_image(
                    crate::app_icon::ASSET,
                    f32::from(IconSize::XLarge.px(&theme)),
                ))
            })
            .child(
                div()
                    .text_size(TextSize::Large.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(BufferLineHeight::Standard.relative())
                    .text_color(theme.text_primary)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(subtitle.to_string()),
            )
    }

    /// The rows for the active section, as card elements. `this` rides
    /// along for closures in interactive controls (rows themselves are
    /// built read-only from app state).
    pub(super) fn settings_rows(
        &self,
        this: &Entity<OrbitApp>,
        theme: Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        match self.settings_section {
            SettingsSection::General => {
                let workspace = self
                    .current_workspace
                    .clone()
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_default();
                let mut rows = vec![self.settings_section(
                    theme,
                    &tr!("settings.this_machine"),
                    vec![
                        self.setting_row(
                            theme,
                            &tr!("settings.pi_agent"),
                            Some(&tr!(
                                "settings.spawned_as_a_child_process_newline_delimited_jso"
                            )),
                            None,
                            Some(self.connection_status(theme)),
                        ),
                        self.setting_row(
                            theme,
                            &tr!("settings.local_by_default"),
                            Some(&tr!(
                                "settings.sessions_live_in_pi_s_own_store_on_this_computer"
                            )),
                            Some(&sessions::sessions_dir().to_string_lossy()),
                            None,
                        ),
                        self.setting_row(
                            theme,
                            &tr!("settings.workspace"),
                            Some(&tr!("settings.new_tasks_start_in_this_directory")),
                            Some(&workspace.to_string_lossy()),
                            None,
                        ),
                    ],
                )];
                if let Some(updates) = self.updater_section(theme, this.clone(), cx) {
                    rows.push(updates);
                }
                rows.push(self.notification_rows(theme, this.clone()));
                rows
            }
            SettingsSection::Runtime => self.runtime_rows(theme, this.clone(), cx),
            SettingsSection::Agent => self.agent_rows(theme, this.clone(), cx),
            SettingsSection::Worktrees => self.worktree_settings_rows(theme, this.clone(), cx),
            // Rendered by `skills_ui::render_skills_page`, not the card body.
            SettingsSection::Skills => Vec::new(),
            SettingsSection::Plugins => self.plugin_rows(theme, this.clone(), cx),
            // The MCP page's rows are owned by `mcp_ui`; it needs the cursor
            // and filter state that lives beside the manager.
            SettingsSection::Mcp => self.mcp_rows(theme, this.clone(), cx),
            SettingsSection::Models => self.model_rows(theme, this.clone(), cx),
            SettingsSection::Appearance => self.appearance_rows(theme, this.clone(), cx),
            SettingsSection::Providers => self.provider_rows(theme, this.clone(), cx),
            SettingsSection::Shortcuts => self.shortcut_rows(theme),
            SettingsSection::Privacy => self.privacy_rows(theme, this.clone(), cx),
            SettingsSection::About => {
                let mut about = vec![self.setting_row(
                    theme,
                    &tr!("settings.orbit_pi"),
                    Some(&tr!("settings.native_workbench_for_the_pi_coding_agent")),
                    None,
                    Some(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .into_any_element(),
                    ),
                )];
                if let Some(update) = self.about_update_row(theme, this.clone(), cx) {
                    about.push(update);
                }
                about.push(
                    self.setting_row(
                        theme,
                        &tr!("settings.gpui"),
                        Some(&tr!(
                            "settings.gpu_accelerated_ui_framework_pinned_runtime_shad"
                        )),
                        None,
                        Some(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child("0.2.2")
                                .into_any_element(),
                        ),
                    ),
                );
                about.push(self.setting_row(
                    theme,
                    &tr!("settings.pi_cli"),
                    Some(&tr!(
                        "settings.the_only_agent_runtime_pi_speaks_its_own_rpc_pro"
                    )),
                    None,
                    Some(self.connection_status(theme)),
                ));
                about.push(self.setting_row(
                    theme,
                    &tr!("settings.source"),
                    Some(&tr!(
                        "settings.open_source_under_apache_2_0_code_issues_and_rel"
                    )),
                    None,
                    Some(self.about_github_button(theme)),
                ));
                // The page header already reads "About"; a board header of
                // the same name would just repeat it, so the group is
                // headerless.
                vec![
                    star_project_banner(theme, "about-star-banner", None),
                    self.settings_group(theme, about),
                ]
            }
            SettingsSection::ReportBug => self.bug_report_rows(theme, cx),
        }
    }

    /// The pinned toolbar for the active section (Providers search + add,
    /// Plugins install + refresh). `None` on sections without one.
    pub(super) fn settings_toolbar(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        match self.settings_section {
            SettingsSection::Providers => Some(self.provider_toolbar(theme, this, cx)),
            SettingsSection::Plugins => Some(self.plugin_toolbar(theme, this, cx)),
            SettingsSection::Mcp => Some(self.mcp_toolbar(theme, this, cx)),
            SettingsSection::Models => Some(self.model_toolbar(theme, this, cx)),
            _ => None,
        }
    }

    // ── Settings → Models ──────────────────────────────────────────────

    /// The pinned Models header: search, a live count, and the favorites
    /// filter. The catalog is pi's own (`get_available_models`); favorites
    /// toggle the same store the composer picker reads.
    pub(super) fn model_toolbar(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let search = picker_search_frame(div(), &theme)
            .w_full()
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.models_filter.clone()));

        let needle = self.models_filter.read(cx).text().trim().to_lowercase();
        let favorites = crate::favorites::all();
        let provider_filter = self.models_provider_filter.as_deref();
        let total = self.available_models.len();
        let favorite_count = self
            .available_models
            .iter()
            .filter(|model| favorites.contains(&model.provider, &model.id))
            .count();
        let shown = self
            .available_models
            .iter()
            .filter(|model| {
                model_visible(
                    model,
                    &needle,
                    self.models_favorites_only,
                    &favorites,
                    provider_filter,
                )
            })
            .count();
        let filtered =
            self.models_favorites_only || !needle.is_empty() || provider_filter.is_some();

        // A filter chip in the toolbar's button language: `surface_elevated` + a
        // hairline when off, the standard `selection` fill when on — never an
        // accent wash, which is reserved for the favorite stars themselves.
        let favorites_button = button_frame(
            div().id("models-favorites-filter"),
            &theme,
            ButtonSize::Large,
        )
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .when(self.models_favorites_only, |button| {
            button
                .raised(theme.surface_active, &theme)
                .text_color(theme.text_primary)
        })
        .when(!self.models_favorites_only, |button| {
            button
                .raised(theme.surface_elevated, &theme)
                .hover(|s| s.raised(theme.surface_hover, &theme))
        })
        .on_mouse_up(MouseButton::Left, {
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |app, cx| app.toggle_models_favorites_only(cx));
            }
        })
        .child(icon(
            "icons/star.svg",
            ButtonSize::Large.icon_size().px(&theme),
            if self.models_favorites_only {
                theme.text_primary
            } else {
                theme.text_secondary
            },
        ))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if self.models_favorites_only {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .child(tr!("settings.favorites")),
        );

        // Refresh pi's provider/model catalog (`pi update --models`) and
        // reload it. The catalog is pi's own data, so this is the one control
        // that fetches new providers/models rather than filtering what we have.
        let update_button = button_frame(div().id("models-update"), &theme, ButtonSize::Large)
            .group(BUTTON_GROUP)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .when(self.models_refreshing, |button| button.opacity(0.6))
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| app.models_refresh(cx));
                }
            })
            .child(refresh_glyph(
                "models-update-spin",
                ButtonSize::Large.icon_size().px(&theme),
                self.models_refreshing,
                theme.text_secondary,
                theme,
            ))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.models_update")),
            );

        // Provider filter: a dropdown instead of a badge per provider, so the
        // control scales to any catalog. Hidden when there is nothing to choose.
        let providers = self.model_providers();
        let provider_select = (providers.len() > 1)
            .then(|| self.model_provider_select(&providers, theme, this.clone(), cx));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            // Search leads, like Providers and Plugins — the filter belongs
            // with the list it narrows.
            .child(search)
            // Status and actions, matching Providers: a live count fills the
            // left, the 32px controls sit flush right. The provider dropdown
            // shares their height so the row reads on one baseline.
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(model_count_label(total, shown, favorite_count, filtered)),
                    )
                    .children(provider_select)
                    .child(update_button)
                    .child(favorites_button),
            )
            .into_any_element()
    }

    /// The active model as a settings-style row: label on the left, selector
    /// flush right, on a raised hairline board. Owns the page's "current
    /// selection" so the grid below stays purely a catalog.
    fn active_model_row(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let control = self.active_model_select(theme, this, cx);
        div()
            .w_full()
            .rounded(Radius::Large.px(&theme))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_elevated)
            .overflow_hidden()
            .child(self.setting_row(
                theme,
                &tr!("settings.active_model"),
                Some(&tr!("settings.active_model_description")),
                None,
                Some(control),
            ))
            .into_any_element()
    }

    /// The catalog's providers in first-appearance order (the grid's group
    /// order), for the scope chips.
    fn model_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = Vec::new();
        for model in &self.available_models {
            if !providers.contains(&model.provider) {
                providers.push(model.provider.clone());
            }
        }
        providers
    }

    /// The provider filter dropdown: `All providers` plus every provider in
    /// the catalog, as a 32px select matching the toolbar's action buttons. A
    /// dropdown scales to any number of providers where a badge per provider
    /// would wrap into a wall.
    fn model_provider_select(
        &self,
        providers: &[String],
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut options = vec![tr!("settings.all_providers")];
        options.extend(
            providers
                .iter()
                .map(|provider| crate::providers::provider_display_name(provider)),
        );
        let selected = self
            .models_provider_filter
            .as_ref()
            .and_then(|current| providers.iter().position(|provider| provider == current))
            .map(|ix| ix + 1)
            .unwrap_or(0);
        self.select_control_sized(
            "models-provider-select",
            SettingsSelect::ModelProvider,
            ButtonSize::Large,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// The active-model selector: a dropdown over the reported catalog that
    /// sets the session's model directly, so the Models page can both browse
    /// and switch.
    pub(super) fn active_model_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        if self.available_models.is_empty() {
            return button_frame(div().id("active-model-select"), &theme, SELECT_CHIP_SIZE)
                .border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .text_color(theme.text_muted)
                .child(tr!("settings.no_models_reported"))
                .into_any_element();
        }
        let options: Vec<String> = self
            .available_models
            .iter()
            .map(|model| {
                format!(
                    "{} · {}",
                    crate::providers::provider_display_name(&model.provider),
                    model.name
                )
            })
            .collect();
        let selected = self
            .available_models
            .iter()
            .position(|model| model.id == self.model_id && model.provider == self.model_provider)
            .unwrap_or(0);
        self.select_control(
            "active-model-select",
            SettingsSelect::ActiveModel,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// Flip the Models page's favorites-only filter.
    pub(super) fn toggle_models_favorites_only(&mut self, cx: &mut Context<Self>) {
        self.models_favorites_only = !self.models_favorites_only;
        cx.notify();
    }

    /// Update the provider/model catalog: run pi's documented `pi update
    /// --models`, then restart the agent so it loads the refreshed catalog and
    /// re-query. Refused mid-run, like every other restart.
    pub(super) fn models_refresh(&mut self, cx: &mut Context<Self>) {
        if self.models_refreshing {
            return;
        }
        // A live run still holds the process; swapping pi's catalog files under
        // it and restarting would kill the turn.
        if self.busy || self.transcript.is_streaming() {
            self.toast_warning(tr!("settings.finish_turn_before_restart"));
            cx.notify();
            return;
        }
        self.models_refreshing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { crate::pi_install::run_model_catalog_refresh() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.models_refreshing = false;
                match result {
                    Ok(output) if output.status.success() => {
                        // pi keeps the catalog it loaded at startup, so reload
                        // it by restarting and resuming the active session.
                        app.restart_pi_for_catalog(cx);
                        app.toast_success(tr!("settings.models_updated"));
                    }
                    Ok(output) => {
                        let detail =
                            crate::pi_update::failure_detail(&output.stdout, &output.stderr);
                        app.toast_error(tr!("settings.models_update_failed", detail = detail));
                    }
                    Err(err) => {
                        app.toast_error(tr!(
                            "settings.models_update_failed",
                            detail = err.to_string()
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Restart the agent so a catalog refreshed on disk is loaded, resuming the
    /// active session when one exists. Mirrors the credential-reload restart.
    fn restart_pi_for_catalog(&mut self, cx: &mut Context<Self>) {
        let resume = self
            .current_session_path
            .clone()
            .and_then(|path| self.sessions.iter().find(|s| s.path == path).cloned());
        self.drop_client();
        self.current_session_path = None;
        if let Some(session) = resume {
            self.switch_to_session(session, false, cx);
        } else {
            self.runtime_start(cx);
        }
    }

    /// The Models page: the catalog grouped by provider — a hairline header
    /// over a three-column grid of cards. A card click sets the active
    /// model; the star toggles the favorite the composer picker reads.
    pub(super) fn model_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if self.available_models.is_empty() {
            return vec![self.empty_resource_card(
                theme,
                "icons/tag-01.svg",
                &tr!("settings.no_models_reported"),
                &tr!("settings.no_models_reported_hint"),
            )];
        }
        let needle = self.models_filter.read(cx).text().trim().to_lowercase();
        let favorites = crate::favorites::all();
        let favorites_only = self.models_favorites_only;
        let provider_filter = self.models_provider_filter.as_deref();

        // Group in catalog order: a stable provider order with its models.
        let mut providers: Vec<String> = Vec::new();
        for model in &self.available_models {
            if !providers.contains(&model.provider) {
                providers.push(model.provider.clone());
            }
        }

        // The active-selection row leads: a page-level "current model"
        // control, then the browsable catalog groups.
        let mut sections: Vec<AnyElement> = vec![self.active_model_row(theme, this.clone(), cx)];
        for provider in providers {
            let models: Vec<&ModelEntry> = self
                .available_models
                .iter()
                .filter(|model| {
                    model.provider == provider
                        && model_visible(
                            model,
                            &needle,
                            favorites_only,
                            &favorites,
                            provider_filter,
                        )
                })
                .collect();
            if models.is_empty() {
                continue;
            }
            let cards: Vec<AnyElement> = models
                .iter()
                .map(|model| self.model_card(model, &favorites, theme, this.clone()))
                .collect();
            sections.push(
                div()
                    .id(ElementId::NamedInteger(
                        "model-group".into(),
                        sections.len() as u64,
                    ))
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        // Provider header: glyph, name, then the group's
                        // count, closed by a hairline. The grid below has no
                        // frame of its own, so the header draws the rule.
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .pb(DynamicSpacing::Base06.px(&theme))
                            .border_b_1()
                            .border_color(theme.border)
                            .child(icon_dyn(
                                provider_icon(&provider),
                                IconSize::Small.px(&theme),
                                provider_mark_color(&provider, &theme, theme.text_muted),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(TextSize::Small.px(&theme))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_secondary)
                                    .child(crate::providers::provider_display_name(&provider)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(tr!("settings.n_models", count = models.len())),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .grid()
                            .grid_cols(3)
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .children(cards),
                    )
                    .into_any_element(),
            );
        }

        if sections.is_empty() {
            let hint = if favorites_only {
                tr!("settings.no_models_match_favorites_hint")
            } else {
                tr!("settings.no_models_match_hint")
            };
            sections.push(self.empty_resource_card(
                theme,
                "icons/search.svg",
                &tr!("settings.no_models_match"),
                &hint,
            ));
        }
        sections
    }

    /// One card in the Models grid: name + star, the model id, and a footer
    /// with the context window and the active mark. Clicking anywhere sets
    /// the model; the star stops propagation so favoriting never switches.
    fn model_card(
        &self,
        model: &ModelEntry,
        favorites: &crate::favorites::Favorites,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let (provider, id) = (model.provider.clone(), model.id.clone());
        let active = self.model_id == model.id && self.model_provider == model.provider;
        let is_favorite = favorites.contains(&model.provider, &model.id);
        // Ink on the active fill comes from `selection_fg`, stepped down for the
        // secondary lines — `text_muted` is authored for the canvas, not a
        // selected card.
        let meta_ink = if active {
            theme.text_primary.opacity(0.72)
        } else {
            theme.text_muted
        };

        let star = icon_button_frame(
            div().id(ElementId::Name(
                format!("model-star-{provider}-{id}").into(),
            )),
            &theme,
            ButtonSize::Default,
        )
        .tip(if is_favorite {
            tr!("settings.unfavorite")
        } else {
            tr!("settings.favorite")
        })
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_hover))
        .on_mouse_up(MouseButton::Left, {
            let this = this.clone();
            let (provider, id) = (provider.clone(), id.clone());
            move |_, _, cx| {
                // Keep the star from bubbling to the card's
                // "set active model" handler.
                cx.stop_propagation();
                crate::favorites::toggle(&provider, &id);
                this.update(cx, |_, cx| cx.notify());
            }
        })
        .child(icon(
            "icons/star.svg",
            ButtonSize::Default.icon_size().px(&theme),
            if is_favorite {
                theme.accent_text
            } else {
                meta_ink
            },
        ));

        let mut footer = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme));
        if let Some(window) = model.context_window {
            footer = footer.child(
                div()
                    .h(DynamicSpacing::Base20.px(&theme))
                    .px(DynamicSpacing::Base06.px(&theme))
                    .rounded(Radius::Small.px(&theme))
                    .bg(theme.overlay_strong)
                    .flex()
                    .items_center()
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(meta_ink)
                    .child(tr!(
                        "settings.n_ctx",
                        count = crate::context_meter::format_tokens(window)
                    )),
            );
        }
        footer = footer.child(div().flex_1());
        if active {
            footer = footer.child(
                div()
                    .h(DynamicSpacing::Base20.px(&theme))
                    .px(DynamicSpacing::Base06.px(&theme))
                    .rounded(Radius::Small.px(&theme))
                    .bg(theme.overlay_strong)
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(icon(
                        "icons/check.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_primary,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::XSmall.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("settings.active")),
                    ),
            );
        } else {
            // Revealed on card hover; the reserved space keeps every card's
            // footer the same height.
            footer = footer.child(
                div()
                    .h(DynamicSpacing::Base20.px(&theme))
                    .flex()
                    .items_center()
                    .opacity(0.)
                    .group_hover("model-card", |s| s.opacity(1.))
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(tr!("settings.set_active")),
            );
        }

        div()
            .id(ElementId::Name(
                format!("model-card-{provider}-{id}").into(),
            ))
            .group("model-card")
            .w_full()
            .min_w_0()
            .bg(if active {
                theme.surface_active
            } else {
                theme.surface_secondary
            })
            .border_1()
            .border_color(if active {
                theme.border_strong
            } else {
                theme.border
            })
            .rounded(Radius::XLarge.px(&theme))
            .p(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .cursor_pointer()
            .when(!active, |card| {
                card.hover(|s| s.bg(theme.surface_hover).border_color(theme.border_strong))
            })
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.set_model(id.clone(), provider.clone(), cx)
                    });
                }
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(model.name.clone()),
                    )
                    .child(star),
            )
            .child(
                div()
                    .truncate()
                    .font_family(theme::code_font_family())
                    .text_size(theme.code_px(10.5))
                    .text_color(meta_ink)
                    .child(model.id.clone()),
            )
            .child(div().flex_1())
            .child(footer)
            .into_any_element()
    }

    // ── Settings → Plugins ─────────────────────────────────────────────

    /// Installed pi packages plus the install field. Plugins are managed
    /// through `pi install/remove/update`, so pi owns the network fetch and
    /// the settings write.
    pub(super) fn plugin_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(error) = &self.plugins_error {
            rows.push(self.provider_error_card(
                theme,
                &tr!("settings.could_not_read_settings"),
                error,
            ));
        }
        if self.plugins.is_empty() {
            rows.push(self.empty_resource_card(
                theme,
                "icons/extensions.svg",
                &tr!("settings.no_plugins_installed"),
                &tr!("settings.no_plugins_installed_hint"),
            ));
            return rows;
        }

        let needle = self.plugins_filter.read(cx).text().trim().to_lowercase();
        let visible: Vec<&PluginPackage> = self
            .plugins
            .iter()
            .filter(|package| {
                needle.is_empty()
                    || package.source.to_lowercase().contains(&needle)
                    || package
                        .version
                        .as_deref()
                        .is_some_and(|version| version.to_lowercase().contains(&needle))
            })
            .collect();

        if visible.is_empty() {
            rows.push(self.empty_resource_card(
                theme,
                "icons/search.svg",
                &tr!("settings.no_plugins_match"),
                &tr!("settings.no_plugins_match_hint"),
            ));
            return rows;
        }
        // The pinned toolbar already states the total; spell the count out
        // only when a search narrows the list, on the rows it describes.
        if !needle.is_empty() {
            let installed = self
                .plugins
                .iter()
                .filter(|package| package.installed)
                .count();
            rows.push(
                div()
                    .w_full()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!(
                        "settings.plugins_shown_installed",
                        shown = visible.len(),
                        total = self.plugins.len(),
                        installed = installed
                    ))
                    .into_any_element(),
            );
        }
        let mut cards = Vec::new();
        for package in visible {
            cards.push(self.plugin_card(package, theme, this.clone()));
        }
        rows.push(self.settings_group(theme, cards));
        rows
    }

    fn plugin_card(
        &self,
        package: &PluginPackage,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let source = package.source.clone();
        let project = package.scope == PackageScope::Project;
        let confirming = self.plugin_remove_confirm.as_deref() == Some(source.as_str());

        let mut badges = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .pl(DynamicSpacing::Base40.px(&theme))
            .child(self.provider_badge(
                package.scope.label(),
                theme.text_muted,
                theme.overlay_strong,
                theme,
            ))
            .child(self.provider_badge(
                package.kind.label(),
                theme.text_muted,
                theme.overlay_strong,
                theme,
            ));
        if let Some(version) = &package.version {
            badges = badges.child(self.provider_badge(
                &format!("v{version}"),
                theme.text_muted,
                theme.overlay_strong,
                theme,
            ));
        }
        if !package.installed {
            badges = badges.child(self.provider_badge(
                &tr!("settings.not_installed"),
                theme.error,
                theme.error.opacity(0.12),
                theme,
            ));
        }

        let actions: AnyElement = if confirming {
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.error)
                        .child(tr!("settings.remove_this_plugin")),
                )
                .child(self.plugin_button(
                    format!("plugin-remove-confirm-{source}"),
                    "Remove",
                    false,
                    PluginButtonSize::Compact,
                    Some("icons/trash.svg"),
                    false,
                    theme,
                    this.clone(),
                    PluginAction::ConfirmRemove {
                        source: source.clone(),
                        project,
                    },
                ))
                .child(self.plugin_button(
                    format!("plugin-remove-cancel-{source}"),
                    "Cancel",
                    false,
                    PluginButtonSize::Compact,
                    None,
                    false,
                    theme,
                    this,
                    PluginAction::CancelRemove,
                ))
                .into_any_element()
        } else {
            let mut actions = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme));
            if package.installed {
                actions = actions.child(self.plugin_button(
                    format!("plugin-update-{source}"),
                    "Update",
                    false,
                    PluginButtonSize::Compact,
                    Some("icons/refresh.svg"),
                    false,
                    theme,
                    this.clone(),
                    PluginAction::Update {
                        source: source.clone(),
                    },
                ));
            }
            actions = actions.child(self.plugin_button(
                format!("plugin-remove-{source}"),
                "Remove",
                false,
                PluginButtonSize::Compact,
                Some("icons/trash.svg"),
                false,
                theme,
                this,
                PluginAction::Remove {
                    source: source.clone(),
                },
            ));
            actions.into_any_element()
        };

        div()
            .w_full()
            .min_w_0()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(Radius::Large.px(&theme))
                            .bg(theme.overlay)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(
                                "icons/extensions.svg",
                                IconSize::Small.px(&theme),
                                theme.text_secondary,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base02.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::Default.px(&theme))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .truncate()
                                    .child(package.name.clone()),
                            )
                            .child(
                                div()
                                    .font_family(theme::code_font_family())
                                    .text_size(theme.code_px(10.5))
                                    .text_color(theme.text_muted)
                                    .truncate()
                                    .child(source),
                            ),
                    )
                    .child(actions),
            )
            .child(badges)
            .when(!package.install_path.as_os_str().is_empty(), |row| {
                row.child(
                    div()
                        .pl(DynamicSpacing::Base40.px(&theme))
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(10.5))
                        .text_color(theme.text_muted)
                        .truncate()
                        .child(package.install_path.to_string_lossy().into_owned()),
                )
            })
            .into_any_element()
    }

    /// The pinned Plugins toolbar: the search field, then the install field,
    /// scope target, and Install, then the status line and Refresh.
    pub(super) fn plugin_toolbar(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let field = input_field_frame(div(), &theme)
            .flex_1()
            .min_w_0()
            .bg(theme.background)
            .child(icon(
                "icons/extensions.svg",
                IconSize::Small.px(&theme),
                theme.text_muted,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.plugin_source_input.clone()),
            );

        let scope = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base04.px(&theme))
            .child(self.plugin_scope_chip(
                "Global",
                !self.plugin_install_project,
                false,
                theme,
                this.clone(),
            ))
            .child(self.plugin_scope_chip(
                "Project",
                self.plugin_install_project,
                true,
                theme,
                this.clone(),
            ));

        let install = self.plugin_button(
            "plugin-install".to_string(),
            "Install",
            true,
            PluginButtonSize::Toolbar,
            Some("icons/plus.svg"),
            false,
            theme,
            this.clone(),
            PluginAction::Install,
        );

        let refresh = self.plugin_button(
            "plugin-refresh".to_string(),
            "Refresh",
            false,
            PluginButtonSize::Toolbar,
            Some("icons/refresh.svg"),
            self.plugin_refresh_spin_until.is_some(),
            theme,
            this,
            PluginAction::Refresh,
        );

        let plugin_spinner: AnyElement = crate::app::spinner(
            "plugin-action-spin",
            IconSize::Small.px(&theme),
            theme.text_secondary,
            theme,
        );
        let status: AnyElement = match &self.plugin_action {
            Some(action) => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_secondary)
                .child(plugin_spinner)
                .child(action.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("settings.n_packages", count = self.plugins.len()))
                .into_any_element(),
        };

        let search = picker_search_frame(div(), &theme)
            .w_full()
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.plugins_filter.clone()));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            // Search leads, like Providers and Models — the filter belongs
            // with the list it narrows, above the controls that add to it.
            .child(search)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(field)
                    .child(scope)
                    .child(install),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(status)
                    .child(refresh),
            )
            .into_any_element()
    }

    fn plugin_scope_chip(
        &self,
        label: &str,
        active: bool,
        project: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let id = format!(
            "plugin-scope-{}",
            if project { "project" } else { "global" }
        );
        let mut chip = button_frame(
            div().id(ElementId::Name(id.into())),
            &theme,
            ButtonSize::Large,
        )
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM);
        chip = if active {
            chip.raised(theme.surface_active, &theme)
                .text_color(theme.text_primary)
        } else {
            chip.border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .text_color(theme.text_muted)
                .hover(|style| style.raised(theme.surface_hover, &theme))
        };
        chip.on_mouse_up(MouseButton::Left, move |_, _, cx| {
            this.update(cx, |app, cx| {
                app.apply_plugin_action(PluginAction::SetScope { project }, cx)
            });
        })
        .child(label.to_string())
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn plugin_button(
        &self,
        id: String,
        label: &str,
        primary: bool,
        size: PluginButtonSize,
        icon_path: Option<&'static str>,
        spinning: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
        action: PluginAction,
    ) -> AnyElement {
        let button_size = match size {
            PluginButtonSize::Compact => ButtonSize::Medium,
            PluginButtonSize::Toolbar => ButtonSize::Large,
        };
        let icon_size = button_size.icon_size().px(&theme);
        let spin_id = ElementId::Name(format!("{id}-spin").into());
        let base = button_frame(div().id(ElementId::Name(id.into())), &theme, button_size)
            .group(BUTTON_GROUP)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM);
        let (button, icon_color) = if primary {
            (
                base.raised(theme.action, &theme)
                    .text_color(theme.action_fg)
                    .hover(|style| style.raised(theme.action_hover, &theme))
                    .active(|style| style.raised(theme.action_pressed, &theme)),
                theme.action_fg,
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
            .when_some(icon_path, move |button, path| {
                if spinning {
                    // Refresh turns its own glyph in place; the synchronous
                    // reload still reads as a deliberate action.
                    button.child(refresh_glyph(spin_id, icon_size, true, icon_color, theme))
                } else {
                    button.child(icon(path, icon_size, icon_color))
                }
            })
            .child(div().child(label.to_string()))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                let action = action.clone();
                this.update(cx, |app, cx| app.apply_plugin_action(action, cx));
            })
            .into_any_element()
    }

    fn empty_resource_card(
        &self,
        theme: Theme,
        icon_path: &'static str,
        title: &str,
        body: &str,
    ) -> AnyElement {
        div()
            .w_full()
            .bg(theme.surface_secondary)
            .border_1()
            .border_color(theme.border)
            .rounded(Radius::XLarge.px(&theme))
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base40.px(&theme))
            .flex()
            .flex_col()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(empty_state_glyph(theme, icon_path))
            .child(
                div()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(body.to_string()),
            )
            .into_any_element()
    }

    /// The sticky provider header: the search field plus the
    /// count / Refresh / Add toolbar. Rendered outside the scroll container
    /// so it stays put while the grid scrolls (gpui 0.2.2 has no
    /// `position: sticky`).
    pub(super) fn provider_toolbar(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let all = self.provider_views();
        let active = all.iter().filter(|view| view.active).count();
        let connected = all.iter().filter(|view| view.connected()).count();

        let search = picker_search_frame(div(), &theme)
            .w_full()
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(self.provider_filter.clone()));

        let refresh_icon = refresh_glyph(
            "providers-refresh-spin",
            ButtonSize::Large.icon_size().px(&theme),
            self.providers_refreshing,
            theme.text_secondary,
            theme,
        );
        let refresh_button = button_frame(div().id("providers-refresh"), &theme, ButtonSize::Large)
            .group(BUTTON_GROUP)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .when(self.providers_refreshing, |button| button.opacity(0.6))
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| app.provider_refresh(cx));
                }
            })
            .child(refresh_icon)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.refresh")),
            );

        let add_button = button_frame(div().id("providers-add"), &theme, ButtonSize::Large)
            .raised(theme.action, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.action_hover, &theme))
            .active(|style| style.raised(theme.action_pressed, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, window, cx| {
                    this.update(cx, |app, cx| app.provider_editor_open(None, window, cx));
                }
            })
            .child(icon(
                "icons/plus.svg",
                ButtonSize::Large.icon_size().px(&theme),
                theme.action_fg,
            ))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.action_fg)
                    .child(tr!("settings.add_provider")),
            );

        let toolbar = div()
            .w_full()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!(
                        "settings.providers_active_connected",
                        all = all.len(),
                        active = active,
                        connected = connected
                    )),
            )
            .child(refresh_button)
            .child(add_button);

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(search)
            .child(toolbar)
            .into_any_element()
    }

    /// The Providers page: every provider pi ships with (plus models.json-only
    /// endpoints) as a 3-column grid, each with real auth status and actions.
    pub(super) fn provider_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let all = self.provider_views();
        let needle = self.provider_filter.read(cx).text().trim().to_lowercase();

        let mut views: Vec<&ProviderView> = all
            .iter()
            .filter(|view| {
                needle.is_empty()
                    || view.name.to_lowercase().contains(&needle)
                    || view.id.to_lowercase().contains(&needle)
                    || view
                        .env_names
                        .iter()
                        .any(|name| name.to_lowercase().contains(&needle))
            })
            .collect();
        let rank = |view: &ProviderView| -> u8 {
            if view.active {
                0
            } else if view.connected() {
                1
            } else if view.custom {
                2
            } else {
                3
            }
        };
        views.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        let mut rows: Vec<AnyElement> = Vec::new();

        // ── credential-changed banner ──
        if self.provider_auth_dirty {
            rows.push(
                div()
                    .w_full()
                    .bg(theme.accent.opacity(0.1))
                    .border_1()
                    .border_color(theme.accent.opacity(0.35))
                    .rounded(Radius::XLarge.px(&theme))
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(icon(
                        "icons/info.svg",
                        IconSize::Medium.px(&theme),
                        theme.accent_text,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(DynamicSpacing::Base02.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .child(tr!("settings.credentials_changed")),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_secondary)
                                    .child(tr!("settings.restart_pi_credentials_hint")),
                            ),
                    )
                    .child(self.provider_button(
                        "providers-apply".into(),
                        &tr!("settings.restart_pi"),
                        ProviderButtonStyle::Primary,
                        theme,
                        this.clone(),
                        ProviderAction::Restart,
                    ))
                    .into_any_element(),
            );
        }

        if let Some(error) = &self.custom_providers_error {
            rows.push(self.provider_error_card(
                theme,
                &tr!("settings.models_json_read_error"),
                error,
            ));
        }
        if let Some(error) = &self.provider_auth_error {
            rows.push(self.provider_error_card(
                theme,
                &tr!("settings.auth_json_read_error"),
                error,
            ));
        }

        // ── grid ──
        if views.is_empty() {
            rows.push(
                div()
                    .w_full()
                    .bg(theme.surface_secondary)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Radius::XLarge.px(&theme))
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base40.px(&theme))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(icon(
                        "icons/search.svg",
                        IconSize::Custom(26. / 16.).px(&theme),
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("settings.no_providers_match")),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(tr!("settings.try_a_different_search")),
                    )
                    .into_any_element(),
            );
        } else {
            let cards: Vec<AnyElement> = views
                .iter()
                .map(|view| self.provider_card(view, theme, this.clone()))
                .collect();
            rows.push(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(3)
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .children(cards)
                    .into_any_element(),
            );
        }

        rows
    }

    /// A red note card for a config read error.
    pub(super) fn provider_error_card(&self, theme: Theme, title: &str, error: &str) -> AnyElement {
        div()
            .w_full()
            .bg(theme.error.opacity(0.08))
            .border_1()
            .border_color(theme.error.opacity(0.35))
            .rounded(Radius::XLarge.px(&theme))
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_start()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(icon(
                "icons/info.svg",
                IconSize::Medium.px(&theme),
                theme.error,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(error.to_string()),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(
                                "settings.fix_or_remove_the_file_before_editing_providers_"
                            )),
                    ),
            )
            .into_any_element()
    }

    /// Every provider pi ships with, merged with the live catalog, models.json,
    /// and auth.json. Built-ins come first so nothing is hidden behind auth.
    pub(super) fn provider_views(&self) -> Vec<ProviderView> {
        let mut views: Vec<ProviderView> = Vec::new();
        // Prefer pi's own provider metadata (introspected from pi-ai). Fall
        // back to the curated table when node/pi-ai aren't reachable.
        if !self.provider_metadata.is_empty() {
            for provider in &self.provider_metadata {
                views.push(self.provider_view_for(
                    &provider.id,
                    Some(provider.name.clone()),
                    provider.oauth,
                    provider.api_key,
                    &provider.env_vars,
                    providers::provider_note(&provider.id),
                    true,
                ));
            }
        } else {
            for builtin in providers::BUILTIN_PROVIDERS {
                let env_vars: Vec<String> = if builtin.env_var.is_empty() {
                    Vec::new()
                } else {
                    vec![builtin.env_var.to_string()]
                };
                views.push(self.provider_view_for(
                    builtin.id,
                    Some(builtin.name.to_string()),
                    builtin.oauth,
                    builtin.api_key,
                    &env_vars,
                    builtin.note,
                    true,
                ));
            }
        }
        // Providers pi ships but neither source knows yet — discovered from
        // pi's bundled catalog data, so a new pi release lists its providers
        // without an Orbit change.
        for id in self.provider_catalog_counts.keys() {
            if !views.iter().any(|view| &view.id == id) {
                views.push(self.provider_view_for(
                    id,
                    None,
                    false,
                    true,
                    &[],
                    providers::provider_note(id),
                    true,
                ));
            }
        }
        // Catalog providers that aren't built-ins (extension providers, or
        // custom endpoints pi has already loaded).
        for model in &self.available_models {
            if !views.iter().any(|view| view.id == model.provider) {
                views.push(self.provider_view_for(
                    &model.provider,
                    None,
                    false,
                    true,
                    &[],
                    "",
                    false,
                ));
            }
        }
        // models.json providers pi hasn't loaded yet.
        for provider in &self.custom_providers {
            if !views.iter().any(|view| view.id == provider.id) {
                views.push(self.provider_view_for(&provider.id, None, false, true, &[], "", false));
            }
        }
        // Providers the running pi advertises through `auth.list` but no other
        // source knows about still get a card and capability-driven buttons.
        for capability in self.auth.providers() {
            if !views.iter().any(|view| view.id == capability.id) {
                views.push(self.provider_view_for(
                    &capability.id,
                    Some(capability.name.clone()),
                    capability.supports_oauth(),
                    capability.supports_api_key(),
                    &[],
                    providers::provider_note(&capability.id),
                    true,
                ));
            }
        }
        views
    }

    /// Build one grid row from all sources.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn provider_view_for(
        &self,
        id: &str,
        name: Option<String>,
        oauth: bool,
        api_key: bool,
        env_names: &[String],
        note: &'static str,
        builtin: bool,
    ) -> ProviderView {
        let custom = self
            .custom_providers
            .iter()
            .find(|provider| provider.id == id);
        let live_count = self
            .available_models
            .iter()
            .filter(|model| model.provider == id)
            .count();
        let catalog_count = self.provider_catalog_counts.get(id).copied().unwrap_or(0);
        let custom_count = custom.map(|provider| provider.model_ids.len()).unwrap_or(0);
        let env_hit = env_names
            .iter()
            .find(|name| std::env::var_os(name.as_str()).is_some())
            .cloned();
        ProviderView {
            id: id.to_string(),
            name: custom
                .and_then(|provider| provider.name.clone())
                .or(name)
                .unwrap_or_else(|| providers::provider_display_name(id)),
            active: live_count > 0,
            model_count: if live_count > 0 {
                live_count
            } else if catalog_count > 0 {
                catalog_count
            } else {
                custom_count
            },
            catalog_count,
            custom: custom.is_some(),
            has_api_key: custom.is_some_and(|provider| provider.has_api_key),
            base_url: custom
                .map(|provider| provider.base_url.clone())
                .unwrap_or_default(),
            api: custom
                .map(|provider| provider.api.clone())
                .unwrap_or_default(),
            builtin,
            oauth,
            api_key,
            env_names: env_names.to_vec(),
            env_authed: env_hit.is_some(),
            env_var: env_hit,
            note,
            auth: self.provider_auth.get(id).copied(),
            live_status: self.auth.status(id).cloned(),
            quota: self.quota.report(id).cloned(),
        }
    }

    /// The live auth block shown on a card while a login is Connecting,
    /// waiting on a device code, or showing a Success/Error/Cancelled result.
    /// `None` means the provider has no session and the card renders its
    /// normal Connect / Disconnect actions.
    pub(super) fn provider_auth_section(
        &self,
        view: &ProviderView,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> Option<AnyElement> {
        let session = self.auth.login_for(&view.id)?;
        let (headline, detail, tint) = match session.phase {
            LoginPhase::Connecting => (
                tr!("status.connecting"),
                tr!("auth.asking_pi"),
                theme.warning,
            ),
            LoginPhase::AwaitingBrowser => (
                tr!("auth.waiting_browser"),
                tr!("auth.finish_in_browser"),
                theme.accent_text,
            ),
            LoginPhase::AwaitingDeviceCode => (
                tr!("auth.enter_device_code"),
                tr!("auth.approve_in_browser"),
                theme.accent_text,
            ),
            LoginPhase::Succeeded => (
                tr!("status.connected"),
                tr!("auth.credential_saved"),
                theme.success,
            ),
            LoginPhase::Error => (
                tr!("status.sign_in_failed"),
                session
                    .error
                    .as_ref()
                    .map(|(_, message)| message.clone())
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| tr!("auth.sign_in_incomplete")),
                theme.error,
            ),
            LoginPhase::Cancelled => (
                tr!("status.cancelled"),
                session
                    .message
                    .clone()
                    .unwrap_or_else(|| tr!("auth.sign_in_cancelled")),
                theme.text_muted,
            ),
        };

        let mut body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(div().size(px(7.)).rounded_full().bg(tint))
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tint)
                            .child(headline),
                    ),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(detail),
            );

        if let Some(device) = &session.device_code {
            body = body.child(
                div()
                    .w_full()
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::XSmall.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!("settings.device_code")),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(device.user_code.clone()),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(10.5))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(device.verification_uri.clone()),
                    ),
            );
        }
        if session.device_code.is_none() {
            if let Some(url) = &session.url {
                body = body.child(
                    div()
                        .truncate()
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(10.5))
                        .text_color(theme.text_muted)
                        .child(url.clone()),
                );
            }
        }

        let id = view.id.clone();
        let name = view.name.clone();
        let method = session.method.clone();
        let mut buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme));
        match session.phase {
            LoginPhase::Connecting
            | LoginPhase::AwaitingBrowser
            | LoginPhase::AwaitingDeviceCode => {
                if let Some(device) = &session.device_code {
                    let open = device
                        .verification_uri_complete
                        .clone()
                        .unwrap_or_else(|| device.verification_uri.clone());
                    buttons = buttons
                        .child(self.provider_button(
                            format!("provider-auth-open-{}", view.id),
                            &tr!("auth.open_page"),
                            ProviderButtonStyle::Primary,
                            theme,
                            this.clone(),
                            ProviderAction::AuthOpenUrl(open),
                        ))
                        .child(self.provider_button(
                            format!("provider-auth-copy-{}", view.id),
                            &tr!("auth.copy_code"),
                            ProviderButtonStyle::Ghost,
                            theme,
                            this.clone(),
                            ProviderAction::AuthCopy(device.user_code.clone()),
                        ));
                }
                buttons = buttons.child(self.provider_button(
                    format!("provider-auth-cancel-{}", view.id),
                    &tr!("git_panel.cancel"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::AuthCancel,
                ));
            }
            LoginPhase::Succeeded => {
                buttons = buttons.child(self.provider_button(
                    format!("provider-auth-done-{}", view.id),
                    &tr!("common.done"),
                    ProviderButtonStyle::Primary,
                    theme,
                    this.clone(),
                    ProviderAction::AuthDismiss,
                ));
            }
            LoginPhase::Error | LoginPhase::Cancelled => {
                buttons = buttons
                    .child(self.provider_button(
                        format!("provider-auth-retry-{}", view.id),
                        &tr!("view.try_again"),
                        ProviderButtonStyle::Primary,
                        theme,
                        this.clone(),
                        ProviderAction::AuthStart {
                            id: id.clone(),
                            name: name.clone(),
                            method: method.clone(),
                        },
                    ))
                    .child(self.provider_button(
                        format!("provider-auth-dismiss-{}", view.id),
                        "Dismiss",
                        ProviderButtonStyle::Ghost,
                        theme,
                        this.clone(),
                        ProviderAction::AuthDismiss,
                    ));
            }
        }
        body = body.child(buttons);
        Some(body.into_any_element())
    }

    /// Account quota/balance/spend for a connected provider, from the
    /// `quota.*` RPC namespace. `None` when there is nothing to render, so a
    /// provider with no usage surface adds no empty block to its card.
    pub(super) fn provider_quota_section(
        &self,
        view: &ProviderView,
        theme: Theme,
    ) -> Option<AnyElement> {
        let report = view.quota.as_ref()?;
        if !report.has_data() && report.error.is_none() && report.note.is_none() {
            return None;
        }

        let amount = |value: f64| -> String {
            if value.fract() == 0.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.2}")
            }
        };
        let reset_label = |ms: i64| tr!("session.resets_at", time = format_epoch_ms(ms));

        let mut head = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(tr!("settings.usage")),
            );
        if let Some(plan) = &report.plan {
            head = head.child(self.provider_badge(
                plan,
                theme.accent_text,
                theme.accent.opacity(0.12),
                theme,
            ));
        }

        let mut body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base08.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(head);

        for window in &report.windows {
            let value = if let Some(percent) = window.used_percent {
                tr!("session.percent_used", percent = format!("{percent:.0}"))
            } else if let (Some(used), Some(limit)) = (window.used, window.limit) {
                tr!(
                    "session.used_of_limit",
                    used = amount(used),
                    limit = amount(limit)
                )
            } else if let Some(used) = window.used {
                match &window.unit {
                    Some(unit) => tr!("session.amount_unit", amount = amount(used), unit = unit),
                    None => amount(used),
                }
            } else {
                continue;
            };

            let mut row = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base04.px(&theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child(window.label.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_primary)
                                .child(value),
                        ),
                );

            if let Some(fraction) = window.fraction() {
                let tint = if fraction >= 0.9 {
                    theme.error
                } else if fraction >= 0.75 {
                    theme.warning
                } else {
                    theme.success
                };
                row = row.child(
                    div()
                        .w_full()
                        .h(px(4.))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(theme.overlay_strong)
                        .child(div().h_full().rounded_full().bg(tint).w(relative(fraction))),
                );
            }
            if let Some(resets_at) = window.resets_at {
                row = row.child(
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.text_muted)
                        .child(reset_label(resets_at)),
                );
            }
            body = body.child(row);
        }

        for balance in &report.balances {
            let text = if balance.currency.is_empty() {
                amount(balance.amount)
            } else {
                format!("{} {}", amount(balance.amount), balance.currency)
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(balance.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_primary)
                            .child(text),
                    ),
            );
        }

        if let Some(error) = &report.error {
            body = body.child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.error)
                    .child(error.to_string()),
            );
        } else if note_should_render(report) {
            if let Some(note) = &report.note {
                body = body.child(
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.text_muted)
                        .child(note.to_string()),
                );
            }
        }

        Some(body.into_any_element())
    }

    /// One large provider card: huge brand mark, status, auth facts, actions.
    pub(super) fn provider_card(
        &self,
        view: &ProviderView,
        theme: Theme,
        this: Entity<OrbitApp>,
    ) -> AnyElement {
        let confirming = self.provider_remove_confirm.as_deref() == Some(view.id.as_str());
        let update_key_label = tr!("settings.update_key");
        let add_key_label = tr!("settings.add_api_key");
        let reconnect_label = tr!("settings.reconnect");
        let sign_in_label = tr!("settings.sign_in");
        // The card no longer renders quota inline: usage opens in a popup so a
        // connected card stays compact (see `provider_usage_layer`).
        let has_usage = view.quota.as_ref().is_some_and(|report| {
            report.has_data() || report.error.is_some() || report.note.is_some()
        });
        let session = self.auth.login_for(&view.id);
        let (status_label, status_color) = if let Some(session) = session {
            match session.phase {
                LoginPhase::Connecting => (tr!("status.connecting"), theme.warning),
                LoginPhase::AwaitingBrowser | LoginPhase::AwaitingDeviceCode => {
                    (tr!("status.connecting"), theme.accent_text)
                }
                LoginPhase::Succeeded => (tr!("status.connected"), theme.success),
                LoginPhase::Error => (tr!("status.sign_in_failed"), theme.error),
                LoginPhase::Cancelled => (tr!("status.cancelled"), theme.text_muted),
            }
        } else if view.active {
            (tr!("status.active"), theme.success)
        } else if view.connected() {
            (tr!("status.connected"), theme.accent_text)
        } else if view.custom {
            (tr!("status.not_loaded"), theme.warning)
        } else {
            (tr!("status.not_configured"), theme.text_muted)
        };

        let tile = div()
            .size(px(52.))
            .flex_none()
            .rounded(Radius::XLarge.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .child(icon_dyn(
                provider_icon(&view.id),
                IconSize::Custom(30. / 16.).px(&theme),
                provider_mark_color(
                    &view.id,
                    &theme,
                    if view.active || view.connected() {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    },
                ),
            ));

        let status_pill = div()
            .h(ButtonSize::Default.height(&theme))
            .px(DynamicSpacing::Base08.px(&theme))
            .rounded_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .bg(status_color.opacity(0.12))
            .child(div().size(px(6.)).rounded_full().bg(status_color))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(status_color)
                    .child(status_label),
            );

        let header = div()
            .flex()
            .items_start()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(tile)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(view.name.clone()),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(10.5))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(view.id.clone()),
                    ),
            );

        // Status and source badges share a row under the name so the name
        // column keeps the card's full width (a right-aligned pill in the
        // header squeezes it on narrow columns).
        let mut badges = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(status_pill);
        if view.oauth {
            badges = badges.child(self.provider_badge(
                "OAuth",
                theme.accent_text,
                theme.accent.opacity(0.12),
                theme,
            ));
        }
        if view.api_key {
            badges = badges.child(self.provider_badge(
                &tr!("settings.api_key"),
                theme.text_muted,
                theme.overlay_strong,
                theme,
            ));
        }
        badges = badges.child(self.provider_badge(
            if view.custom {
                "Custom"
            } else if view.builtin {
                "Built-in"
            } else {
                "Provider"
            },
            if view.custom {
                theme.accent_text
            } else {
                theme.text_muted
            },
            if view.custom {
                theme.accent.opacity(0.12)
            } else {
                theme.overlay_strong
            },
            theme,
        ));

        // Active providers show what pi is serving; unconfigured built-ins show
        // their full built-in catalog size (pi's RPC never reports those).
        let count_label = if !view.active && view.catalog_count > 0 {
            tr!("settings.n_in_catalog", count = view.catalog_count)
        } else {
            tr!("settings.n_models", count = view.model_count)
        };
        let mut facts = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(count_label);
        if let Some(kind) = view.credential_kind() {
            facts = facts
                .child(div().size(px(3.)).rounded_full().bg(theme.text_muted))
                .child(match kind {
                    "oauth" => tr!("settings.cred_oauth"),
                    "api_key" => tr!("settings.cred_api_key"),
                    "session" => tr!("settings.cred_session"),
                    other => other.to_string(),
                });
            if let Some(status) = &view.live_status {
                if let Some(account) = &status.account {
                    facts = facts
                        .child(div().size(px(3.)).rounded_full().bg(theme.text_muted))
                        .child(account.clone());
                }
                if let Some(expires_at) = status.expires_at {
                    facts = facts
                        .child(div().size(px(3.)).rounded_full().bg(theme.text_muted))
                        .child(tr!(
                            "settings.expires_at",
                            time = format_epoch_ms(expires_at)
                        ));
                }
            }
        } else if let Some(env_var) = &view.env_var {
            facts = facts
                .child(div().size(px(3.)).rounded_full().bg(theme.text_muted))
                .child(tr!("settings.via_env_var", env_var = env_var));
        }
        if view.has_api_key {
            facts = facts
                .child(div().size(px(3.)).rounded_full().bg(theme.text_muted))
                .child(tr!("settings.models_json_key"));
        }

        let endpoint = if view.base_url.is_empty() {
            if view.note.is_empty() {
                tr!("settings.pi_default_endpoint")
            } else {
                providers::localize_note(view.note)
            }
        } else if view.api.is_empty() {
            view.base_url.clone()
        } else {
            format!("{} · {}", view.base_url, view.api)
        };
        let base_url = div()
            .truncate()
            .font_family(theme::code_font_family())
            .text_size(theme.code_px(10.5))
            .text_color(theme.text_muted)
            .child(endpoint);

        let actions: AnyElement = if confirming {
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.error)
                        .child(tr!("settings.remove_this_provider")),
                )
                .child(self.provider_button(
                    format!("provider-remove-confirm-{}", view.id),
                    &tr!("settings.remove"),
                    ProviderButtonStyle::Danger,
                    theme,
                    this.clone(),
                    ProviderAction::ConfirmRemove {
                        id: view.id.clone(),
                    },
                ))
                .child(self.provider_button(
                    format!("provider-remove-cancel-{}", view.id),
                    &tr!("git_panel.cancel"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::CancelRemove,
                ))
                .into_any_element()
        } else if let Some(section) = self.provider_auth_section(view, theme, this.clone()) {
            section
        } else {
            // Methods come from pi's `auth.list` capability discovery — the
            // UI never hardcodes provider ids or assumes a login flow.
            let capability = if self.auth.support() == AuthSupport::Supported {
                self.auth.provider(&view.id)
            } else {
                None
            };
            let mut primary = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme));
            if let Some(capability) = capability {
                for method in &capability.methods {
                    if method.id == "api_key" {
                        let update = view.credential_kind() == Some("api_key");
                        primary = primary.child(self.provider_button(
                            format!("provider-key-{}", view.id),
                            if update {
                                &update_key_label
                            } else {
                                &add_key_label
                            },
                            ProviderButtonStyle::Ghost,
                            theme,
                            this.clone(),
                            ProviderAction::EditKey {
                                id: view.id.clone(),
                                name: view.name.clone(),
                                oauth: view.oauth,
                                note: view.note,
                            },
                        ));
                    } else {
                        let label = method_label(&method.id, &method.label, view.connected());
                        primary = primary.child(self.provider_button(
                            format!("provider-auth-{}-{}", method.id, view.id),
                            &label,
                            ProviderButtonStyle::Primary,
                            theme,
                            this.clone(),
                            ProviderAction::AuthStart {
                                id: view.id.clone(),
                                name: view.name.clone(),
                                method: method.id.clone(),
                            },
                        ));
                    }
                }
                if capability.methods.is_empty() {
                    primary = primary.child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!("settings.no_sign_in_methods_available")),
                    );
                }
            } else {
                // File-based fallback for a pi without the auth RPC.
                if view.oauth {
                    let reconnect = view.credential_kind() == Some("oauth");
                    primary = primary.child(self.provider_button(
                        format!("provider-signin-{}", view.id),
                        if reconnect {
                            &reconnect_label
                        } else {
                            &sign_in_label
                        },
                        ProviderButtonStyle::Primary,
                        theme,
                        this.clone(),
                        ProviderAction::SignIn {
                            id: view.id.clone(),
                            name: view.name.clone(),
                        },
                    ));
                }
                if view.api_key {
                    let update = view.credential_kind() == Some("api_key");
                    primary = primary.child(self.provider_button(
                        format!("provider-key-{}", view.id),
                        if update {
                            &update_key_label
                        } else {
                            &add_key_label
                        },
                        if view.oauth && !view.connected() {
                            ProviderButtonStyle::Ghost
                        } else {
                            ProviderButtonStyle::Primary
                        },
                        theme,
                        this.clone(),
                        ProviderAction::EditKey {
                            id: view.id.clone(),
                            name: view.name.clone(),
                            oauth: view.oauth,
                            note: view.note,
                        },
                    ));
                }
            }
            // Ollama Cloud usage is separate from the local endpoint's API key:
            // current accounts use a real cloud key (monthly credits); legacy
            // accounts expose session/weekly usage only behind a signed-in
            // settings page, which needs the user's own session cookie. Offer
            // the session editor explicitly so neither is auto-scraped.
            if providers::is_ollama_cloud_provider(&view.id) {
                primary = primary.child(self.provider_button(
                    format!("provider-ollama-session-{}", view.id),
                    &tr!("settings.usage_session"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::EditOllamaSession {
                        id: view.id.clone(),
                        name: view.name.clone(),
                    },
                ));
            }
            let connected = view.auth.is_some()
                || view
                    .live_status
                    .as_ref()
                    .is_some_and(|status| status.authenticated);
            // Secondary actions: Configure/Remove only exist when there is a
            // models.json entry to edit; Sign out only when a credential is
            // stored. Built-ins with neither show just the auth button.
            let mut secondary = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme));
            if view.custom {
                secondary = secondary.child(self.provider_button(
                    format!("provider-configure-{}", view.id),
                    &tr!("settings.configure"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::Configure {
                        id: view.id.clone(),
                    },
                ));
            }
            if connected {
                secondary = secondary.child(self.provider_button(
                    format!("provider-signout-{}", view.id),
                    &tr!("settings.disconnect"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::SignOut {
                        id: view.id.clone(),
                    },
                ));
            }
            if view.custom {
                secondary = secondary.child(self.provider_button(
                    format!("provider-remove-{}", view.id),
                    &tr!("settings.remove"),
                    ProviderButtonStyle::Danger,
                    theme,
                    this.clone(),
                    ProviderAction::Remove {
                        id: view.id.clone(),
                    },
                ));
            }
            if has_usage {
                secondary = secondary.child(self.provider_button(
                    format!("provider-usage-{}", view.id),
                    &tr!("session.usage"),
                    ProviderButtonStyle::Ghost,
                    theme,
                    this.clone(),
                    ProviderAction::ShowUsage {
                        id: view.id.clone(),
                    },
                ));
            }
            let mut actions = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(primary);
            if view.custom || connected || has_usage {
                actions = actions.child(secondary);
            }
            actions.into_any_element()
        };

        div()
            .w_full()
            .min_w_0()
            .bg(theme.surface_secondary)
            .border_1()
            .border_color(theme.border)
            .rounded(Radius::XLarge.px(&theme))
            .p(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(header)
            .child(badges)
            .child(facts)
            .child(base_url)
            .child(div().h(BORDER_WIDTH).w_full().bg(theme.border))
            .child(actions)
            .into_any_element()
    }

    /// A small status/source pill on a provider card.
    pub(super) fn provider_badge(
        &self,
        label: &str,
        fg: Hsla,
        bg: Hsla,
        theme: Theme,
    ) -> AnyElement {
        div()
            .h(DynamicSpacing::Base20.px(&theme))
            .px(DynamicSpacing::Base06.px(&theme))
            .rounded(Radius::Small.px(&theme))
            .flex_none()
            .flex()
            .items_center()
            .bg(bg)
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg)
                    .child(label.to_string()),
            )
            .into_any_element()
    }

    /// A provider-card action button. One closure per card would be a lot of
    /// near-identical code, so every button routes through
    /// [`Self::apply_provider_action`]. The glyph is derived from the action.
    pub(super) fn provider_button(
        &self,
        id: String,
        label: &str,
        style: ProviderButtonStyle,
        theme: Theme,
        this: Entity<OrbitApp>,
        action: ProviderAction,
    ) -> AnyElement {
        let icon_path = Self::provider_action_icon(&action);
        let mut button = button_frame(
            div().id(ElementId::Name(id.into())),
            &theme,
            ButtonSize::Medium,
        )
        .group(BUTTON_GROUP)
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM);
        button = match style {
            ProviderButtonStyle::Primary => button
                .raised(theme.action, &theme)
                .text_color(theme.action_fg)
                .hover(|style| style.raised(theme.action_hover, &theme))
                .active(|style| style.raised(theme.action_pressed, &theme)),
            ProviderButtonStyle::Ghost => button
                .border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .text_color(theme.text_secondary)
                .hover(|style| style.raised(theme.surface_hover, &theme)),
            ProviderButtonStyle::Danger => button
                .border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .text_color(theme.error)
                .hover(|style| {
                    style
                        .border_color(theme.error.opacity(0.6))
                        .raised(theme.error.opacity(0.08), &theme)
                }),
        };
        let icon_color = match style {
            ProviderButtonStyle::Primary => theme.action_fg,
            ProviderButtonStyle::Danger => theme.error,
            ProviderButtonStyle::Ghost => theme.text_secondary,
        };
        press(button)
            .when_some(icon_path, |button, path| {
                button.child(icon(path, IconSize::XSmall.px(&theme), icon_color))
            })
            .child(div().child(label.to_string()))
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                let action = action.clone();
                this.update(cx, |app, cx| app.apply_provider_action(action, window, cx));
            })
            .into_any_element()
    }

    /// The glyph for a provider action (none for plain text buttons).
    pub(super) fn provider_action_icon(action: &ProviderAction) -> Option<&'static str> {
        match action {
            ProviderAction::SignIn { .. } | ProviderAction::AuthStart { .. } => {
                Some("icons/lock.svg")
            }
            ProviderAction::AuthOpenUrl(_) => Some("icons/arrow-up-right.svg"),
            ProviderAction::AuthCopy(_) => Some("icons/copy.svg"),
            ProviderAction::AuthCancel => Some("icons/x.svg"),
            ProviderAction::AuthDismiss => Some("icons/check.svg"),
            ProviderAction::EditKey { .. } => Some("icons/at-sign.svg"),
            ProviderAction::EditOllamaSession { .. } => Some("icons/clock.svg"),
            ProviderAction::ShowUsage { .. } => Some("icons/usage-total.svg"),
            ProviderAction::SignOut { .. } => Some("icons/stop.svg"),
            ProviderAction::Configure { .. } => Some("icons/settings.svg"),
            ProviderAction::Remove { .. } | ProviderAction::ConfirmRemove { .. } => {
                Some("icons/trash.svg")
            }
            ProviderAction::Restart => Some("icons/refresh.svg"),
            ProviderAction::CancelRemove | ProviderAction::SaveKey => None,
        }
    }

    /// Single dispatch point for every provider-card button.
    pub(super) fn apply_provider_action(
        &mut self,
        action: ProviderAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ProviderAction::SignIn { id, name } => {
                // Signing in from the key modal replaces it.
                self.provider_key_editor = None;
                self.provider_oauth_login(id, name, cx);
            }
            ProviderAction::AuthStart { id, name, method } => {
                self.provider_key_editor = None;
                self.auth_start_login(id, name, method, cx);
            }
            ProviderAction::AuthCancel => self.auth_cancel_login(cx),
            ProviderAction::AuthDismiss => {
                self.auth.dismiss_result();
                cx.notify();
            }
            ProviderAction::AuthOpenUrl(url) => {
                if let Err(err) = platform::open_url(&url) {
                    self.toast_warning(tr!("runtime.browser_failed", error = err));
                }
            }
            ProviderAction::AuthCopy(value) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                self.toast_success(tr!("settings.copied_to_clipboard"));
            }
            ProviderAction::EditKey {
                id,
                name,
                oauth,
                note,
            } => self.provider_key_open(id, name, oauth, note, window, cx),
            ProviderAction::EditOllamaSession { id, name } => self.provider_credential_open(
                id,
                name,
                false,
                "",
                ProviderKeyKind::OllamaCloudSession,
                window,
                cx,
            ),
            ProviderAction::SignOut { id } => self.provider_sign_out(id, cx),
            ProviderAction::ShowUsage { id } => {
                self.provider_usage_open = Some(id);
                cx.notify();
            }
            ProviderAction::Configure { id } => self.provider_editor_open(Some(id), window, cx),
            ProviderAction::Remove { id } => {
                self.provider_remove_confirm = Some(id);
                cx.notify();
            }
            ProviderAction::ConfirmRemove { id } => self.provider_remove(id, cx),
            ProviderAction::CancelRemove => {
                self.provider_remove_confirm = None;
                cx.notify();
            }
            ProviderAction::SaveKey => self.provider_key_save(window, cx),
            ProviderAction::Restart => self.provider_apply_credentials(cx),
        }
    }

    /// The provider usage/quota modal, or `None` when closed. The card keeps
    /// only a Usage button; windows, balances, and spend render here so a
    /// connected card stays compact.
    pub(super) fn provider_usage_layer(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.provider_usage_open.as_deref()?;
        let view = self
            .provider_views()
            .into_iter()
            .find(|view| view.id == id)?;
        let body = self.provider_quota_section(&view, theme)?;

        let close = button_frame(div().id("provider-usage-close"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.provider_usage_open = None;
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.close")),
            );

        let card = div()
            .w_full()
            .max_w(px(460.))
            .max_h(relative(1.))
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(Self::provider_editor_cancel))
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(modal::header_gap(&theme))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(modal::HEADLINE.px(&theme))
                            .line_height(modal::HEADLINE.line_height(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("settings.usage_for", name = view.name)),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(11.))
                            .text_color(theme.text_muted)
                            .child(view.id.clone()),
                    ),
            )
            .child(
                div()
                    .id("provider-usage-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(modal::section_padding_x(&theme, false))
                    .py(DynamicSpacing::Base16.px(&theme))
                    .child(body),
            )
            .child(
                div()
                    .flex_none()
                    .px(modal::footer_padding(&theme))
                    .py(modal::footer_padding(&theme))
                    .flex()
                    .items_center()
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(close),
            );

        let scrim = match theme.mode {
            ThemeMode::Dark => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.42,
            },
            ThemeMode::Light => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.22,
            },
        };
        Some(
            div()
                .id("provider-usage-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .p(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::provider_editor_scrim))
                .child(card)
                .into_any_element(),
        )
    }

    /// The API-key modal, or `None` when closed.
    pub(super) fn provider_key_layer(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.provider_key_editor.as_ref()?;

        let (field_label, hint) = match editor.kind {
            ProviderKeyKind::ApiKey => (tr!("settings.api_key"), tr!("settings.api_key_hint")),
            ProviderKeyKind::OllamaCloudSession => (
                tr!("settings.session_cookie"),
                tr!("settings.session_cookie_hint"),
            ),
        };

        let mut body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(input::gap(&theme))
                    .child(
                        input_label(field_label, &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(
                        input_field_frame(div(), &theme)
                            .w_full()
                            .bg(theme.background)
                            .child(editor.key.clone()),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(hint),
                    ),
            );
        if !editor.note.is_empty() {
            body = body.child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(providers::localize_note(editor.note)),
            );
        }
        if editor.oauth && editor.kind == ProviderKeyKind::ApiKey {
            let id = editor.provider_id.clone();
            let name = editor.provider_name.clone();
            let this_signin = this.clone();
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(
                        div()
                            .flex_1()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(tr!(
                                "settings.prefer_a_subscription_sign_in_with_oauth_instead"
                            )),
                    )
                    .child(self.provider_button(
                        format!("provider-key-signin-{id}"),
                        &tr!("settings.sign_in"),
                        ProviderButtonStyle::Ghost,
                        theme,
                        this_signin,
                        ProviderAction::SignIn { id, name },
                    )),
            );
        }
        if let Some(error) = &editor.error {
            body = body.child(
                div()
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .bg(theme.error.opacity(0.1))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.error)
                    .child(error.clone()),
            );
        }

        let cancel = button_frame(div().id("provider-key-close"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .on_mouse_up(MouseButton::Left, {
                let this = this.clone();
                move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.provider_key_editor = None;
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.cancel")),
            );
        let save = self.provider_button(
            "provider-key-save".into(),
            &tr!("settings.save_key"),
            ProviderButtonStyle::Primary,
            theme,
            this.clone(),
            ProviderAction::SaveKey,
        );

        let card = div()
            .w_full()
            .max_w(px(460.))
            .max_h(relative(1.))
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(Self::provider_editor_cancel))
            .on_action(cx.listener(Self::provider_editor_confirm))
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(modal::header_gap(&theme))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(modal::HEADLINE.px(&theme))
                            .line_height(modal::HEADLINE.line_height(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("settings.api_key_for", name = editor.provider_name)),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(11.))
                            .text_color(theme.text_muted)
                            .child(editor.provider_id.clone()),
                    ),
            )
            .child(
                div()
                    .id("provider-key-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(modal::section_padding_x(&theme, false))
                    .py(DynamicSpacing::Base16.px(&theme))
                    .child(body),
            )
            .child(
                div()
                    .flex_none()
                    .px(modal::footer_padding(&theme))
                    .py(modal::footer_padding(&theme))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(modal::footer_gap(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(cancel)
                    .child(save),
            );

        let scrim = match theme.mode {
            ThemeMode::Dark => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.42,
            },
            ThemeMode::Light => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.22,
            },
        };
        Some(
            div()
                .id("provider-key-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .p(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::provider_editor_scrim))
                .child(card)
                .into_any_element(),
        )
    }

    /// The provider editor modal (add / configure), or `None` when closed.
    pub(super) fn provider_editor_layer(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.provider_editor.as_ref()?;
        let editing = editor.original_id.is_some();
        let is_custom_entry = editor.original_id.as_ref().is_some_and(|id| {
            self.custom_providers
                .iter()
                .any(|provider| &provider.id == id)
        });
        let subtitle = if editing && editor.in_catalog && !is_custom_entry {
            tr!("settings.builtin_provider_hint")
        } else {
            tr!("settings.saved_to_models_json")
        };

        let field = |label: &str, hint: Option<&str>, input: Entity<ComposerInput>| -> AnyElement {
            let mut column = div()
                .flex()
                .flex_col()
                .gap(input::gap(&theme))
                .child(
                    input_label(label.to_string(), &theme)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_secondary),
                )
                .child(
                    input_field_frame(div(), &theme)
                        .w_full()
                        .bg(theme.background)
                        .child(input),
                );
            if let Some(hint) = hint {
                column = column.child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(hint.to_string()),
                );
            }
            column.into_any_element()
        };

        // API family chips.
        let mut api_chips = div()
            .flex()
            .flex_wrap()
            .gap(DynamicSpacing::Base06.px(&theme));
        for api in PROVIDER_APIS {
            let selected = editor.api == api;
            let this = this.clone();
            api_chips = api_chips.child(
                button_frame(
                    div().id(ElementId::Name(format!("provider-api-{api}").into())),
                    &theme,
                    ButtonSize::Default,
                )
                .border_1()
                .border_color(if selected {
                    theme.accent.opacity(0.5)
                } else {
                    theme.border
                })
                .raised(
                    if selected {
                        theme.accent.opacity(0.12)
                    } else {
                        theme.surface_elevated
                    },
                    &theme,
                )
                .cursor_pointer()
                .hover(|style| style.raised(theme.surface_hover, &theme))
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        if let Some(editor) = app.provider_editor.as_mut() {
                            editor.api = api.to_string();
                            cx.notify();
                        }
                    });
                })
                .child(
                    div()
                        .font_family(theme::code_font_family())
                        .text_color(if selected {
                            theme.accent_text
                        } else {
                            theme.text_muted
                        })
                        .child(api),
                ),
            );
        }

        // Identity: editable only when adding.
        let identity: AnyElement = if editing {
            div()
                .flex()
                .flex_col()
                .gap(input::gap(&theme))
                .child(
                    input_label(tr!("settings.provider_id"), &theme)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_secondary),
                )
                .child(
                    div()
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(12.))
                        .text_color(theme.text_primary)
                        .child(editor.original_id.clone().unwrap_or_default()),
                )
                .into_any_element()
        } else {
            field(
                &tr!("settings.provider_id"),
                Some(&tr!("settings.provider_id_hint")),
                editor.id.clone(),
            )
        };

        let api_key_hint = if editor.had_api_key {
            tr!("settings.api_key_stored_hint")
        } else {
            tr!("settings.api_key_optional_hint")
        };
        let models_builtin_hint = tr!("settings.models_builtin_hint");
        let models_custom_hint = tr!("settings.models_custom_hint");

        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(identity)
            .child(field(
                &tr!("settings.display_name"),
                Some(&tr!("settings.optional")),
                editor.name.clone(),
            ))
            .child(field(
                &tr!("settings.base_url"),
                Some(&tr!("settings.base_url_hint")),
                editor.base_url.clone(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(input::gap(&theme))
                    .child(
                        input_label(tr!("settings.api_type"), &theme)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_secondary),
                    )
                    .child(api_chips),
            )
            .child(field(
                &tr!("settings.api_key"),
                Some(&api_key_hint),
                editor.api_key.clone(),
            ))
            .child(field(
                &tr!("settings.models_field"),
                Some(if editor.in_catalog {
                    &models_builtin_hint
                } else {
                    &models_custom_hint
                }),
                editor.models.clone(),
            ));

        let mut card = div()
            .w_full()
            .max_w(px(520.))
            .max_h(px(560.))
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(Self::provider_editor_cancel))
            .on_action(cx.listener(Self::provider_editor_confirm))
            .child(
                div()
                    .px(modal::header_padding_x(&theme))
                    .pt(modal::header_padding_top(&theme))
                    .pb(modal::header_padding_bottom(&theme))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(modal::header_gap(&theme))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(modal::HEADLINE.px(&theme))
                            .line_height(modal::HEADLINE.line_height(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(if editing {
                                tr!("settings.configure_provider")
                            } else {
                                tr!("settings.add_provider")
                            }),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .id("provider-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(modal::section_padding_x(&theme, false))
                    .py(DynamicSpacing::Base16.px(&theme))
                    .child(body),
            );

        if let Some(error) = &editor.error {
            card = card.child(
                div()
                    .mx(modal::section_padding_x(&theme, false))
                    .mb(DynamicSpacing::Base04.px(&theme))
                    .px(DynamicSpacing::Base06.px(&theme))
                    .py(DynamicSpacing::Base04.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .bg(theme.error.opacity(0.1))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.error)
                    .child(error.clone()),
            );
        }

        let cancel = {
            let this = this.clone();
            button_frame(
                div().id("provider-editor-cancel"),
                &theme,
                ButtonSize::Large,
            )
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| {
                    app.provider_editor = None;
                    cx.notify();
                });
            })
            .child(
                div()
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.cancel")),
            )
        };
        let save = {
            let this = this.clone();
            button_frame(div().id("provider-editor-save"), &theme, ButtonSize::Large)
                .raised(theme.action, &theme)
                .cursor_pointer()
                .hover(|style| style.raised(theme.action_hover, &theme))
                .active(|style| style.raised(theme.action_pressed, &theme))
                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    this.update(cx, |app, cx| app.provider_save(window, cx));
                })
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.action_fg)
                        .child(if editing {
                            tr!("settings.save_changes")
                        } else {
                            tr!("settings.add_provider")
                        }),
                )
        };
        card = card.child(
            div()
                .flex_none()
                .px(modal::footer_padding(&theme))
                .py(modal::footer_padding(&theme))
                .flex()
                .items_center()
                .justify_end()
                .gap(modal::footer_gap(&theme))
                .border_t_1()
                .border_color(theme.border)
                .child(cancel)
                .child(save),
        );

        let scrim = match theme.mode {
            ThemeMode::Dark => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.42,
            },
            ThemeMode::Light => Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.22,
            },
        };
        Some(
            div()
                .id("provider-editor-layer")
                .absolute()
                .inset_0()
                .occlude()
                .bg(scrim)
                .px(DynamicSpacing::Base24.px(&theme))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::provider_editor_scrim))
                .child(card)
                .into_any_element(),
        )
    }

    /// A settings section: an 11px uppercase label over one grouped board.
    /// The label sits closer to its board than to the section above it.
    pub(super) fn settings_section(
        &self,
        theme: Theme,
        label: &str,
        rows: Vec<AnyElement>,
    ) -> AnyElement {
        self.settings_section_desc(theme, label, None, rows)
    }

    /// Like [`Self::settings_section`], with one line of context under the
    /// label. Used where several rows share a trigger, so it is stated once
    /// instead of repeated on every row.
    pub(super) fn settings_section_desc(
        &self,
        theme: Theme,
        label: &str,
        desc: Option<&str>,
        rows: Vec<AnyElement>,
    ) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .px(DynamicSpacing::Base04.px(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base02.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::XSmall.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(label.to_uppercase()),
                    )
                    .children(desc.map(|desc| {
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(desc.to_string())
                    })),
            )
            .child(self.settings_group(theme, rows))
            .into_any_element()
    }

    /// One bordered board whose rows are divided by inset 1px hairlines —
    /// the grouped-surface pattern from DESIGN.md, never a stack of cards.
    /// No `overflow_hidden`: select popups anchor inside these rows and
    /// must escape the board's rounded box.
    pub(super) fn settings_group(&self, theme: Theme, rows: Vec<AnyElement>) -> AnyElement {
        let last = rows.len().saturating_sub(1);
        let mut board = div()
            .w_full()
            .bg(theme.surface_secondary)
            .border_1()
            .border_color(theme.border)
            .rounded(Radius::XLarge.px(&theme))
            .flex()
            .flex_col();
        for (i, row) in rows.into_iter().enumerate() {
            board = board.child(row);
            if i != last {
                board = board.child(
                    div()
                        .h(BORDER_WIDTH)
                        .flex_none()
                        .mx(DynamicSpacing::Base16.px(&theme))
                        .bg(theme.border),
                );
            }
        }
        board.into_any_element()
    }

    /// A row inside a settings board: title (and optional description or
    /// dimmed path) on the left, an optional control on the right. Every
    /// control shares one right-aligned axis down the board.
    pub(super) fn setting_row(
        &self,
        theme: Theme,
        title: &str,
        desc: Option<&str>,
        meta: Option<&str>,
        control: Option<AnyElement>,
    ) -> AnyElement {
        div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base16.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base03.px(&theme))
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(title.to_string()),
                    )
                    .children(desc.map(|d| {
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(d.to_string())
                    }))
                    .children(meta.map(|m| {
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(m.to_string())
                    })),
            )
            .children(control.map(|c| div().flex_none().child(c)))
            .into_any_element()
    }

    /// Connection state: green dot + "Connected" / red dot + "Not running".
    pub(super) fn connection_status(&self, theme: Theme) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(
                div()
                    .size(px(7.))
                    .rounded_full()
                    .bg(if self.client.is_some() {
                        theme.success
                    } else {
                        theme.error
                    }),
            )
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(if self.client.is_some() {
                        tr!("status.connected")
                    } else {
                        tr!("settings.not_running")
                    }),
            )
            .into_any_element()
    }

    // ── Settings → About: the repository link ──────────────────────────

    /// The About page's repository link: a ghost button that opens the
    /// project's GitHub page in the OS browser.
    pub(super) fn about_github_button(&self, theme: Theme) -> AnyElement {
        button_frame(div().id("about-github"), &theme, ButtonSize::Medium)
            .group(BUTTON_GROUP)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(|s| s.raised(theme.surface_hover, &theme))
            .child(icon(
                "icons/arrow-up-right.svg",
                ButtonSize::Medium.icon_size().px(&theme),
                theme.text_secondary,
            ))
            .child(tr!("settings.github"))
            .on_mouse_up(MouseButton::Left, |_, _, cx| {
                cx.open_url(env!("CARGO_PKG_REPOSITORY"));
            })
            .into_any_element()
    }

    // ── Settings → Shortcuts ─────────────────────────────────────

    /// The Shortcuts reference: every workbench-level chord, grouped by the
    /// surface it acts on. The app-command rows are generated from the
    /// command registry (`crate::commands`), so the page is complete and can
    /// never advertise a chord the keymap does not bind. Only the composer's
    /// text-entry conventions (send/newline/accept — not commands) are
    /// listed literally, because their labels depend on live preferences.
    pub(super) fn shortcut_rows(&self, theme: Theme) -> Vec<AnyElement> {
        use crate::commands::{self, Category};

        let mut rows: Vec<AnyElement> = Vec::new();
        for category in Category::ORDER {
            let section: Vec<AnyElement> = commands::COMMANDS
                .iter()
                .filter(|spec| spec.help && spec.category == category)
                .map(|spec| {
                    let title = tr!(spec.title_key);
                    match spec.shortcut() {
                        Some(chip) => self.shortcut_row(theme, &title, &[chip.as_str()]),
                        None => self.setting_row(theme, &title, None, None, None),
                    }
                })
                .collect();
            if !section.is_empty() {
                rows.push(self.settings_section(theme, &tr!(category.group_key()), section));
            }
            if category == Category::Agent {
                rows.push(self.composer_shortcut_section(theme));
            }
        }
        rows
    }

    /// The composer's text-entry conventions — not commands, so kept out of
    /// the registry — with the live send-mode labels the preference drives.
    fn composer_shortcut_section(&self, theme: Theme) -> AnyElement {
        use crate::platform::shortcuts as keys;
        self.settings_section(
            theme,
            &tr!("shortcut.group_composer"),
            vec![
                self.shortcut_row(
                    theme,
                    &tr!("shortcut.send_idle"),
                    &[keys::label(keys::SEND).as_str()],
                ),
                self.shortcut_row(
                    theme,
                    &tr!(
                        "shortcut.send_while_running",
                        mode = SendMode::FollowUp.label()
                    ),
                    &[SendMode::FollowUp
                        .shortcut_label(theme.ui.composer_send_mode)
                        .as_str()],
                ),
                self.shortcut_row(
                    theme,
                    &tr!(
                        "shortcut.send_while_running",
                        mode = SendMode::Steer.label()
                    ),
                    &[
                        SendMode::Steer
                            .shortcut_label(theme.ui.composer_send_mode)
                            .as_str(),
                        keys::label(keys::STEER).as_str(),
                    ],
                ),
                self.shortcut_row(
                    theme,
                    &tr!("shortcut.newline"),
                    &[keys::label(keys::NEWLINE).as_str()],
                ),
                self.shortcut_row(
                    theme,
                    &tr!("shortcut.accept"),
                    &[keys::label(keys::ACCEPT).as_str()],
                ),
                self.shortcut_row(
                    theme,
                    &tr!("shortcut.stop"),
                    &[keys::label(keys::STOP).as_str()],
                ),
            ],
        )
    }

    /// One shortcut reference row: the command label on the left, its chord as
    /// a keycap chip on the shared right axis. `setting_row` owns the layout so
    /// the page reads like every other settings board.
    fn shortcut_row(&self, theme: Theme, title: &str, keys: &[&str]) -> AnyElement {
        self.setting_row(
            theme,
            title,
            None,
            None,
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .children(keys.iter().map(|key| keycap(key, theme)))
                    .into_any_element(),
            ),
        )
    }

    // ── Settings → General: notifications ──────────────────────────────

    /// The notification board: three real channels plus the honest system
    /// state when macOS is blocking banners or the build is unbundled. A
    /// switch the OS ignores must not look like it is working.
    pub(super) fn notification_rows(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let mut rows = vec![
            self.setting_row(
                theme,
                &tr!("settings.desktop_notifications"),
                Some(&tr!(
                    "settings.show_a_system_banner_when_a_run_finishes_while_o"
                )),
                None,
                Some(self.settings_toggle(
                    "notification-desktop-toggle",
                    self.notification_prefs.desktop,
                    theme,
                    this.clone(),
                    Self::toggle_desktop_notifications,
                )),
            ),
            self.setting_row(
                theme,
                &tr!("settings.in_app_toasts"),
                Some(&tr!(
                    "settings.show_a_toast_in_the_window_when_a_run_finishes_o"
                )),
                None,
                Some(self.settings_toggle(
                    "notification-toasts-toggle",
                    self.notification_prefs.toasts,
                    theme,
                    this.clone(),
                    Self::toggle_toast_notifications,
                )),
            ),
            self.setting_row(
                theme,
                &tr!("settings.notification_sound"),
                Some(&tr!(
                    "settings.play_the_system_alert_sound_when_a_background_ru"
                )),
                None,
                Some(self.settings_toggle(
                    "notification-sound-toggle",
                    self.notification_prefs.sound,
                    theme,
                    this.clone(),
                    Self::toggle_notification_sound,
                )),
            ),
        ];
        if self.notification_prefs.desktop {
            match self.notification_auth {
                notifications::DesktopAuth::Denied => rows.push(self.setting_row(
                    theme,
                    &tr!("settings.blocked_in_system_settings"),
                    Some(&tr!(
                        "settings.macos_is_not_allowing_orbit_pi_to_post_notificat"
                    )),
                    None,
                    Some(self.runtime_button(
                        "notification-open-settings",
                        &tr!("settings.open_system_settings"),
                        false,
                        theme,
                        this,
                        |_, _| {
                            let _ = platform::open_notification_settings();
                        },
                    )),
                )),
                notifications::DesktopAuth::Unbundled => rows.push(self.setting_row(
                    theme,
                    &tr!("settings.developer_build"),
                    Some(&tr!(
                        "settings.orbit_is_running_from_a_bare_binary_so_banners_a"
                    )),
                    None,
                    None,
                )),
                notifications::DesktopAuth::Granted | notifications::DesktopAuth::Unknown => {}
            }
        }
        self.settings_section_desc(
            theme,
            &tr!("settings.notifications"),
            Some(&tr!("settings.notifications_description")),
            rows,
        )
    }

    /// Flip the desktop channel, ask for permission on the way on, and
    /// re-read the OS state so the board never shows a stale state.
    pub(super) fn toggle_desktop_notifications(&mut self, cx: &mut Context<Self>) {
        self.notification_prefs.desktop = !self.notification_prefs.desktop;
        notifications::Prefs::persist(self.notification_prefs);
        if self.notification_prefs.desktop {
            notifications::request_permission();
            self.refresh_notification_auth(cx);
        } else {
            self.notification_auth = notifications::DesktopAuth::Unknown;
        }
        cx.notify();
    }

    /// Flip the in-app toast channel; turning it on previews a card so the
    /// switch shows what it toggles.
    pub(super) fn toggle_toast_notifications(&mut self, cx: &mut Context<Self>) {
        self.notification_prefs.toasts = !self.notification_prefs.toasts;
        notifications::Prefs::persist(self.notification_prefs);
        if self.notification_prefs.toasts {
            self.toast_info(tr!("settings.in_app_toasts_are_on"));
        }
        cx.notify();
    }

    /// Flip the sound channel; turning it on previews the sound so the
    /// switch is heard before it matters.
    pub(super) fn toggle_notification_sound(&mut self, cx: &mut Context<Self>) {
        self.notification_prefs.sound = !self.notification_prefs.sound;
        notifications::Prefs::persist(self.notification_prefs);
        if self.notification_prefs.sound {
            notifications::play_sound();
        }
        cx.notify();
    }

    /// Read macOS's banner permission without blocking the UI. Called when
    /// General opens and after the desktop switch flips on.
    pub(super) fn refresh_notification_auth(&mut self, cx: &mut Context<Self>) {
        if self.notification_auth_pending || !self.notification_prefs.desktop {
            return;
        }
        self.notification_auth_pending = true;
        cx.spawn(async move |this, cx| {
            // The framework answers on a background queue; the blocking read
            // runs off the UI thread.
            let auth = cx
                .background_executor()
                .spawn(async { notifications::permission() })
                .await;
            this.update(cx, |app, cx| {
                app.notification_auth = auth;
                app.notification_auth_pending = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Settings → Runtime ─────────────────────────────────────────────

    /// The Runtime section: the live pi process, its details, and
    /// start/stop/restart controls. Orbit has no socket server — the
    /// transport is stdio, so there is no host or port to report.
    pub(super) fn runtime_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        _cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let state = self.runtime_state();
        let (state_label, state_color) = match state {
            RuntimeState::Running => (tr!("runtime.state_running"), theme.success),
            RuntimeState::Exited => (tr!("runtime.state_exited"), theme.error),
            RuntimeState::Stopped => (tr!("runtime.state_stopped"), theme.text_muted),
            RuntimeState::Failed => (tr!("runtime.state_failed"), theme.error),
        };
        let running = state == RuntimeState::Running;
        let has_client = self.client.is_some();

        let description = match state {
            RuntimeState::Running => {
                tr!("settings.spawned_child_process")
            }
            RuntimeState::Exited => tr!("settings.runtime_exited"),
            RuntimeState::Stopped => tr!("settings.runtime_stopped"),
            RuntimeState::Failed => tr!("settings.runtime_failed"),
        };

        let mut controls = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme));
        if has_client {
            controls = controls
                .child(self.runtime_button(
                    "runtime-restart",
                    "Restart",
                    false,
                    theme,
                    this.clone(),
                    OrbitApp::runtime_restart,
                ))
                .child(self.runtime_button(
                    "runtime-stop",
                    "Stop",
                    false,
                    theme,
                    this.clone(),
                    OrbitApp::runtime_stop,
                ));
        } else {
            controls = controls.child(self.runtime_button(
                "runtime-start",
                "Start",
                true,
                theme,
                this.clone(),
                OrbitApp::runtime_start,
            ));
        }

        let pid = self
            .client
            .as_ref()
            .map(|client| client.child_pid().to_string())
            .unwrap_or_else(|| "—".to_string());
        let uptime = if running {
            self.runtime
                .started_at
                .map(|started| format_uptime(started.elapsed()))
                .unwrap_or_else(|| "—".to_string())
        } else {
            "—".to_string()
        };

        let status = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(div().size(px(7.)).rounded_full().bg(state_color))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_primary)
                    .child(state_label),
            )
            .into_any_element();

        let mut process = vec![self.setting_row(
            theme,
            &tr!("settings.status"),
            Some(&description),
            None,
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(status)
                    .child(controls)
                    .into_any_element(),
            ),
        )];
        process.push(self.setting_row(
            theme,
            &tr!("settings.process_id"),
            None,
            None,
            Some(runtime_text(theme, pid)),
        ));
        process.push(self.setting_row(
            theme,
            &tr!("settings.binary"),
            None,
            None,
            Some(runtime_path(theme, orbit_rpc::pi_binary())),
        ));
        process.push(self.setting_row(
            theme,
            &tr!("settings.uptime"),
            None,
            None,
            Some(runtime_text(theme, uptime)),
        ));
        process.push(self.setting_row(
            theme,
            &tr!("settings.transport"),
            None,
            None,
            Some(runtime_text(theme, tr!("settings.transport_stdio"))),
        ));
        process.push(self.setting_row(
            theme,
            "RPC patches",
            Some(&self.rpc_patches.summary()),
            None,
            Some(self.settings_toggle(
                "rpc-patches-toggle",
                self.rpc_patches.enabled,
                theme,
                this.clone(),
                OrbitApp::toggle_rpc_patches,
            )),
        ));
        if let Some(root) = &self.rpc_patches.package_root {
            process.push(self.setting_row(
                theme,
                "Package root",
                None,
                None,
                Some(runtime_path(theme, root.display().to_string())),
            ));
        }
        if let Some(error) = &self.runtime.error {
            process.push(self.setting_row(
                theme,
                &tr!("settings.last_error"),
                None,
                None,
                Some(runtime_error(theme, error.clone())),
            ));
        }

        let mut rows = vec![self.settings_section(theme, &tr!("settings.process"), process)];

        // Background sessions — each owns its own pi process.
        if !self.lives.is_empty() {
            let count = self.lives.len();
            let parked_desc = tr!("settings.background_sessions", count = count);
            let mut background = vec![self.setting_row(
                theme,
                &tr!("settings.parked_processes"),
                Some(&parked_desc),
                None,
                None,
            )];
            for (path, parked) in &self.lives {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                background.push(self.setting_row(
                    theme,
                    &name,
                    None,
                    None,
                    Some(runtime_text(
                        theme,
                        tr!(
                            "settings.pid_state",
                            pid = parked.client.child_pid(),
                            state = if parked.busy {
                                tr!("settings.busy")
                            } else {
                                tr!("settings.idle")
                            }
                        ),
                    )),
                ));
            }
            rows.push(self.settings_section(theme, &tr!("settings.background"), background));
        }

        // Recent stderr — visible failures for "if any issue, show status".
        let stderr = self
            .client
            .as_ref()
            .map(|client| client.recent_stderr(8))
            .unwrap_or_default();
        let body: AnyElement = if stderr.is_empty() {
            div()
                .px(DynamicSpacing::Base16.px(&theme))
                .py(DynamicSpacing::Base12.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("settings.no_output_from_the_pi_process"))
                .into_any_element()
        } else {
            let mut block = div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base02.px(&theme));
            for line in &stderr {
                block = block.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(11.5))
                        .text_color(theme.code_text)
                        .child(line.clone()),
                );
            }
            div()
                .bg(theme.code_bg)
                .rounded(Radius::XLarge.px(&theme))
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .child(block)
                .into_any_element()
        };
        rows.push(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .px(DynamicSpacing::Base04.px(&theme))
                        .text_size(TextSize::XSmall.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(tr!("settings.recent_stderr")),
                )
                .child(body)
                .into_any_element(),
        );

        rows
    }

    // ── Settings → Agent ───────────────────────────────────────────────

    /// Agent behavior: Orbit's composer preference plus pi's queue delivery,
    /// compaction, and retry controls. The composer preference is local; it
    /// chooses which existing RPC command the next send uses.
    pub(super) fn agent_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let behavior = vec![
            self.setting_row(
                theme,
                &tr!("settings.enter_while_running"),
                Some(&tr!(
                    "settings.enter_while_running_hint",
                    keys = platform::shortcuts::label(platform::shortcuts::SEND_ALTERNATE)
                )),
                None,
                Some(self.composer_send_mode_toggle(theme, this.clone())),
            ),
            self.setting_row(
                theme,
                &tr!("settings.follow_up_messages"),
                Some(&tr!(
                    "settings.messages_sent_while_the_agent_is_running_wait_in"
                )),
                None,
                Some(self.follow_up_mode_toggle(theme, this.clone())),
            ),
            self.setting_row(
                theme,
                &tr!("settings.auto_compaction"),
                Some(&tr!(
                    "settings.compact_conversation_context_automatically_when_"
                )),
                None,
                Some(self.settings_toggle(
                    "auto-compaction-toggle",
                    self.auto_compaction,
                    theme,
                    this.clone(),
                    Self::toggle_auto_compaction,
                )),
            ),
            self.setting_row(
                theme,
                &tr!("settings.auto_retry"),
                Some(&tr!(
                    "settings.retry_automatically_on_transient_errors_overload"
                )),
                None,
                Some(self.settings_toggle(
                    "auto-retry-toggle",
                    self.auto_retry,
                    theme,
                    this.clone(),
                    Self::toggle_auto_retry,
                )),
            ),
        ];

        let mut sections = Vec::new();
        if self.retrying {
            sections.push(self.settings_section(
                theme,
                &tr!("settings.status"),
                vec![self.setting_row(
                    theme,
                    &tr!("settings.retrying"),
                    Some(&tr!(
                        "settings.pi_is_waiting_out_a_transient_provider_error_bef"
                    )),
                    None,
                    Some(self.runtime_button(
                        "abort-retry",
                        &tr!("settings.abort_retry"),
                        false,
                        theme,
                        this.clone(),
                        Self::abort_retry,
                    )),
                )],
            ));
        }
        if self.client.is_none() {
            sections.push(self.settings_section(
                theme,
                &tr!("settings.status"),
                vec![self.setting_row(
                    theme,
                    &tr!("settings.pi_is_not_connected"),
                    Some(&tr!(
                        "settings.start_the_runtime_from_settings_runtime_to_chang"
                    )),
                    None,
                    None,
                )],
            ));
        }
        sections.push(self.settings_section(theme, &tr!("settings.behavior"), behavior));

        // Auto session titles: the extension asks a model for a short title
        // from the first exchange; these two rows own whether and which.
        sections.push(self.settings_section(
            theme,
            &tr!("settings.session_titles"),
            vec![
                self.setting_row(
                    theme,
                    &tr!("settings.auto_session_titles"),
                    Some(&tr!(
                        "settings.name_each_session_from_its_first_exchange_so"
                    )),
                    None,
                    Some(self.settings_toggle(
                        "auto-title-toggle",
                        self.auto_title.enabled,
                        theme,
                        this.clone(),
                        Self::toggle_auto_title,
                    )),
                ),
                self.setting_row(
                    theme,
                    &tr!("settings.title_model"),
                    Some(&tr!(
                        "settings.model_that_writes_the_title_defaults_to_the_a"
                    )),
                    None,
                    Some(self.title_model_select(theme, this.clone(), cx)),
                ),
            ],
        ));

        // The model every new session starts on, and its thinking level.
        // Applied at session birth; existing sessions keep the model recorded
        // in their file.
        sections.push(self.settings_section(
            theme,
            &tr!("settings.session_defaults"),
            vec![
                self.setting_row(
                    theme,
                    &tr!("settings.default_model"),
                    Some(&tr!("settings.default_model_hint")),
                    None,
                    Some(self.default_model_select(theme, this.clone(), cx)),
                ),
                self.setting_row(
                    theme,
                    &tr!("settings.default_thinking"),
                    Some(&tr!("settings.default_thinking_hint")),
                    None,
                    Some(self.default_thinking_select(theme, this.clone(), cx)),
                ),
            ],
        ));
        sections
    }

    /// Flip auto session titles on/off and persist it for the extension.
    pub(super) fn toggle_auto_title(&mut self, cx: &mut Context<Self>) {
        self.auto_title.enabled = !self.auto_title.enabled;
        self.auto_title.persist();
        cx.notify();
    }

    /// Settings → Agent: the model the auto-title extension asks. The first
    /// option is the live session model; the rest is the reported catalog.
    pub(super) fn title_model_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut options: Vec<String> = vec![tr!("settings.active_session_model")];
        options.extend(self.available_models.iter().map(|model| {
            format!(
                "{} · {}",
                crate::providers::provider_display_name(&model.provider),
                model.name
            )
        }));
        let selected = self
            .auto_title
            .model
            .as_ref()
            .and_then(|wanted| {
                self.available_models
                    .iter()
                    .position(|model| model.provider == wanted.provider && model.id == wanted.id)
            })
            .map(|ix| ix + 1)
            .unwrap_or(0);
        self.select_control(
            "title-model-select",
            SettingsSelect::TitleModel,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// The model new sessions start on. The leading option clears the slot,
    /// leaving pi's own startup model untouched.
    pub(super) fn default_model_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut options: Vec<String> = vec![tr!("settings.pi_default")];
        options.extend(self.available_models.iter().map(|model| {
            format!(
                "{} · {}",
                crate::providers::provider_display_name(&model.provider),
                model.name
            )
        }));
        let slot = &self.session_default;
        let selected = slot
            .model_id
            .as_ref()
            .and_then(|id| {
                self.available_models.iter().position(|model| {
                    model.id == *id
                        && slot
                            .provider
                            .as_deref()
                            .is_none_or(|provider| model.provider == provider)
                })
            })
            .map(|ix| ix + 1)
            .unwrap_or(0);
        self.select_control(
            "default-model-select",
            SettingsSelect::DefaultModel,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// The default model's thinking level. Options are the levels pi itself
    /// derives for the selected model (see `catalog_thinking_levels`); until a
    /// model is chosen the control is a disabled `pi default` chip.
    pub(super) fn default_thinking_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(model) = self.default_model_entry() else {
            return button_frame(
                div().id("default-thinking-select"),
                &theme,
                SELECT_CHIP_SIZE,
            )
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .text_color(theme.text_muted)
            .child(tr!("settings.pi_default"))
            .into_any_element();
        };
        let mut options: Vec<String> = vec![tr!("settings.pi_default")];
        options.extend(
            model
                .thinking_levels
                .iter()
                .map(|level| crate::model_selector::thinking_display(level)),
        );
        let selected = self
            .session_default
            .thinking
            .as_ref()
            .and_then(|level| model.thinking_levels.iter().position(|l| l == level))
            .map(|ix| ix + 1)
            .unwrap_or(0);
        self.select_control(
            "default-thinking-select",
            SettingsSelect::DefaultThinking,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// Orbit-only preference; existing pi queues are left untouched.
    fn composer_send_mode_toggle(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let modes = [SendMode::FollowUp, SendMode::Steer];
        div()
            .flex()
            .items_center()
            .children(modes.into_iter().enumerate().map(|(index, mode)| {
                let this = this.clone();
                let active = mode == theme.ui.composer_send_mode;
                segmented_segment(
                    div().id(("composer-send-mode", index)),
                    &theme,
                    ButtonSize::Medium,
                    SegmentPosition::at(index, modes.len()),
                )
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(active, |b| {
                    b.raised_flat(theme.surface_active)
                        .text_color(theme.text_primary)
                })
                .when(!active, |b| {
                    b.raised_flat(theme.surface_elevated)
                        .text_color(theme.text_secondary)
                        .hover(|s| s.raised_flat(theme.surface_hover))
                })
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |app, cx| app.set_composer_send_mode(mode, cx));
                })
                .child(mode.label())
            }))
            .into_any_element()
    }

    fn set_composer_send_mode(&mut self, mode: SendMode, cx: &mut Context<Self>) {
        let mut ui = theme::get(cx).ui;
        ui.composer_send_mode = mode;
        theme::set_ui_prefs(cx, ui);
        cx.notify();
    }

    /// Two-button segmented control for the follow-up delivery mode.
    pub(super) fn follow_up_mode_toggle(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let all = self.follow_up_mode == "all";
        let segments = [
            (tr!("settings.one_at_a_time"), false, "follow-up-mode-one"),
            (tr!("settings.all"), true, "follow-up-mode-all"),
        ];
        let count = segments.len();
        // One segmented control: only its outer corners round, and the two
        // halves share the hairline between them.
        div()
            .flex()
            .items_center()
            .children(
                segments
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, value_all, id))| {
                        let this = this.clone();
                        let active = all == value_all;
                        segmented_segment(
                            div().id(id),
                            &theme,
                            ButtonSize::Medium,
                            SegmentPosition::at(index, count),
                        )
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .when(active, |b| {
                            b.border_color(theme.border)
                                .raised_flat(theme.selection)
                                .text_color(theme.text_primary)
                        })
                        .when(!active, |b| {
                            b.border_color(theme.border)
                                .raised_flat(theme.surface_elevated)
                                .text_color(theme.text_secondary)
                                .hover(|s| s.raised_flat(theme.surface_hover))
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            this.update(cx, |app, cx| app.set_follow_up_mode(value_all, cx));
                        })
                        .child(label)
                    }),
            )
            .into_any_element()
    }

    /// Update the follow-up mode locally and push it to pi.
    pub(super) fn set_follow_up_mode(&mut self, all: bool, cx: &mut Context<Self>) {
        let mode = if all { "all" } else { "one-at-a-time" }.to_string();
        self.follow_up_mode = mode.clone();
        self.send(CommandBody::SetFollowUpMode { mode }, "set_follow_up_mode");
        cx.notify();
    }

    /// A real toggle switch (accent when on), parameterized by its action.
    pub(super) fn settings_toggle(
        &self,
        id: &'static str,
        on: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
        action: fn(&mut OrbitApp, &mut Context<OrbitApp>),
    ) -> AnyElement {
        div()
            .id(id)
            .w(px(36.))
            .h(DynamicSpacing::Base20.px(&theme))
            .rounded_full()
            .p(DynamicSpacing::Base02.px(&theme))
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(on, |t| t.bg(theme.accent).justify_end())
            .when(!on, |t| t.bg(theme.surface_elevated).justify_start())
            .hover(|t| t.border_color(theme.border_strong))
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, action);
            })
            .child(div().size(px(14.)).rounded_full().bg(theme.toggle_knob))
            .into_any_element()
    }

    /// Settings → Privacy: local-only controls such as session retention.
    pub(super) fn privacy_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.settings_section(
                theme,
                &tr!("settings.session_retention"),
                vec![
                    self.setting_row(
                        theme,
                        &tr!("settings.session_retention_enabled"),
                        Some(&tr!("settings.session_retention_enabled_description")),
                        None,
                        Some(self.settings_toggle(
                            "session-retention-toggle",
                            self.retention.enabled,
                            theme,
                            this.clone(),
                            Self::toggle_session_retention,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.session_retention_action"),
                        Some(&tr!("settings.session_retention_action_description")),
                        None,
                        Some(self.retention_mode_select(theme, this.clone(), cx)),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.session_retention_age"),
                        Some(&tr!("settings.session_retention_age_description")),
                        None,
                        Some(self.retention_days_select(theme, this.clone(), cx)),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.session_retention_run"),
                        Some(&tr!("settings.session_retention_run_description")),
                        None,
                        Some(self.retention_run_button(theme, this.clone())),
                    ),
                ],
            ),
            self.temporary_files_section(theme, this.clone()),
            self.ai_scratch_files_section(theme, this),
        ]
    }

    /// Settings → Privacy: the retention action dropdown (archive / delete).
    fn retention_mode_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let options = vec![
            tr!("settings.session_retention_mode_archive"),
            tr!("settings.session_retention_mode_delete"),
        ];
        let selected = match self.retention.mode {
            crate::session_retention::RetentionMode::Archive => 0,
            crate::session_retention::RetentionMode::Delete => 1,
        };
        self.select_control(
            "session-retention-mode-select",
            SettingsSelect::RetentionMode,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// Settings → Privacy: the idle-threshold dropdown.
    fn retention_days_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::session_retention::DAY_PRESETS;
        let options: Vec<String> = DAY_PRESETS
            .iter()
            .map(|days| tr!("settings.session_retention_days", days = days))
            .collect();
        let selected = DAY_PRESETS
            .iter()
            .position(|days| *days == self.retention.days)
            .unwrap_or(0);
        self.select_control(
            "session-retention-days-select",
            SettingsSelect::RetentionDays,
            options[selected].clone(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// Settings → Privacy: run the policy immediately (the same pass the
    /// launch hook runs) and report the result as a toast.
    fn retention_run_button(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        button_frame(div().id("session-retention-run"), &theme, ButtonSize::Large)
            .border_1()
            .border_color(theme.border)
            .raised(theme.surface_elevated, &theme)
            .cursor_pointer()
            .hover(|style| style.raised(theme.surface_hover, &theme))
            .active(|style| style.raised(theme.surface_active, &theme))
            .text_color(theme.text_secondary)
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, |app, cx| app.run_session_retention(cx));
            })
            .child(tr!("settings.session_retention_run"))
            .into_any_element()
    }

    /// Flip the session-retention policy and persist it. It takes effect on
    /// the next launch, or right away from "Clean up now".
    pub(super) fn toggle_session_retention(&mut self, cx: &mut Context<Self>) {
        self.retention.enabled = !self.retention.enabled;
        self.retention.persist();
        cx.notify();
    }

    /// Toggle auto-applying pi's RPC patches. Persisted; the patches run
    /// before the next launch, so an agent restart is needed to load a change.
    pub(super) fn toggle_rpc_patches(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.rpc_patches.enabled;
        crate::rpc_patches::set_enabled(enabled);
        self.rpc_patches.enabled = enabled;
        self.set_status(if enabled {
            "RPC patches enabled — restart the agent to load them".to_string()
        } else {
            "RPC patches disabled — pi keeps the current patches until it updates".to_string()
        });
        cx.notify();
    }

    pub(super) fn toggle_auto_compaction(&mut self, cx: &mut Context<Self>) {
        self.auto_compaction = !self.auto_compaction;
        self.send(
            CommandBody::SetAutoCompaction {
                enabled: self.auto_compaction,
            },
            "set_auto_compaction",
        );
        self.set_status(if self.auto_compaction {
            tr!("settings.auto_compaction_on")
        } else {
            tr!("settings.auto_compaction_off")
        });
        cx.notify();
    }

    pub(super) fn toggle_auto_retry(&mut self, cx: &mut Context<Self>) {
        self.auto_retry = !self.auto_retry;
        self.send(
            CommandBody::SetAutoRetry {
                enabled: self.auto_retry,
            },
            "set_auto_retry",
        );
        self.set_status(if self.auto_retry {
            tr!("settings.auto_retry_on")
        } else {
            tr!("settings.auto_retry_off")
        });
        cx.notify();
    }

    /// Appearance → Reduce motion: persist the preference; every paint site
    /// reads it through `Theme.ui`, so the change is live.
    pub(super) fn toggle_reduce_motion(&mut self, cx: &mut Context<Self>) {
        let mut ui = theme::get(cx).ui;
        ui.reduce_motion = !ui.reduce_motion;
        theme::set_ui_prefs(cx, ui);
        self.set_status(if ui.reduce_motion {
            tr!("settings.reduce_motion_on")
        } else {
            tr!("settings.reduce_motion_off")
        });
        cx.notify();
    }

    pub(super) fn compact_now(&mut self, cx: &mut Context<Self>) {
        if self.is_compacting {
            return;
        }
        self.is_compacting = true;
        // The run-status strip shows the in-progress state.
        if !self.send(
            CommandBody::Compact {
                custom_instructions: None,
            },
            "compact",
        ) {
            self.is_compacting = false;
        }
        cx.notify();
    }

    pub(super) fn abort_retry(&mut self, cx: &mut Context<Self>) {
        self.send(CommandBody::AbortRetry, "abort_retry");
        self.retrying = false;
        self.retry_detail = None;
        self.set_status(tr!("settings.retry_aborted"));
        cx.notify();
    }

    pub(super) fn rename_session(&mut self, cx: &mut Context<Self>) {
        // Enter while an IME candidate window is open confirms the candidate;
        // it must not commit a half-composed name. The click path is covered
        // by the same guard, since the preedit text is not a final answer.
        if self.session_name_input.read(cx).is_composing() {
            return;
        }
        let name = self.session_name_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.toast_warning(tr!("settings.enter_session_name"));
            cx.notify();
            return;
        }
        if !self.send(
            CommandBody::SetSessionName { name: name.clone() },
            "set_session_name",
        ) {
            return;
        }
        // The header (and the open session's sidebar row) read this name
        // immediately; `session_info_changed` is not guaranteed on rename.
        self.session_name = Some(name.clone());
        self.current_title = Some(name.clone());
        if let Some(path) = &self.current_session_path {
            if let Some(session) = self.sessions.iter_mut().find(|s| &s.path == path) {
                session.title = name;
            }
        }
        // Flash a check on the Update button so a successful commit reads as
        // done rather than a silent no-op. Only the timer for this exact stamp
        // clears it, so a second click extends the flash instead of cutting it
        // short when the first timer lands.
        let saved_at = Instant::now();
        self.rename_saved_at = Some(saved_at);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RENAME_FEEDBACK).await;
            let _ = this.update(cx, |app, cx| {
                if app.rename_saved_at == Some(saved_at) {
                    app.rename_saved_at = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// A Runtime action button (Start / Stop / Restart).
    pub(super) fn runtime_button(
        &self,
        id: &'static str,
        label: &str,
        primary: bool,
        theme: Theme,
        this: Entity<OrbitApp>,
        action: fn(&mut OrbitApp, &mut Context<OrbitApp>),
    ) -> AnyElement {
        let mut button = button_frame(div().id(id), &theme, ButtonSize::Medium)
            .group(BUTTON_GROUP)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM);
        if primary {
            button = button
                .raised(theme.action, &theme)
                .text_color(theme.action_fg)
                .hover(|s| s.raised(theme.action_hover, &theme))
                .active(|s| s.raised(theme.action_pressed, &theme));
        } else {
            button = button
                .border_1()
                .border_color(theme.border)
                .raised(theme.surface_elevated, &theme)
                .text_color(theme.text_secondary)
                .hover(|s| s.raised(theme.surface_hover, &theme));
        }
        press(button)
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                this.update(cx, action);
            })
            .child(label.to_string())
            .into_any_element()
    }

    // ── Appearance ─────────────────────────────────────────────────────

    /// The Appearance section: theme, background, type & density, and
    /// layout. Grouped boards with hairline-separated rows, each section led
    /// by a live preview so a change is legible on the page before you leave
    /// it. The backdrop's tuning is its own board and appears only once an
    /// image is configured — there is nothing to tune otherwise.
    pub(super) fn appearance_rows(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let background = crate::dither::configured_label();
        // The empty 84px strip is a hole: Theme already has live swatches,
        // and the default dot grid is visible on the new-task page. Preview
        // the processed image only once there is one to judge.
        let mut theme_rows = Vec::new();
        if background.is_some() {
            theme_rows.push(self.backdrop_preview(theme));
        }
        theme_rows.push(self.setting_row(
            theme,
            &tr!("settings.appearance"),
            Some(&tr!("settings.appearance_mode_description")),
            None,
            Some(self.appearance_mode_control(theme, this.clone(), cx)),
        ));
        // One Theme control, not one per appearance mode. It shows and edits
        // the palette currently in effect, so the pair of rows that made the
        // user pick a mode before picking a colour is gone: flip Appearance to
        // edit the other mode's palette, and each is remembered separately.
        theme_rows.push(self.setting_row(
            theme,
            &tr!("settings.theme"),
            None,
            None,
            Some(self.theme_select(theme.mode, theme, this.clone(), cx)),
        ));
        theme_rows.push(self.setting_row(
            theme,
            &tr!("settings.background_image"),
            Some(&tr!("settings.a_dithered_image_behind_the_new_task_page")),
            background.as_deref(),
            Some(self.background_controls(theme, this.clone())),
        ));
        theme_rows.push(self.setting_row(
            theme,
            &tr!("language.title"),
            Some(&tr!("language.description")),
            None,
            Some(self.language_select(theme, this.clone(), cx)),
        ));
        let mut sections = vec![
            self.settings_section(theme, &tr!("settings.theme_and_background"), theme_rows),
            self.settings_section(
                theme,
                &tr!("settings.type_and_density"),
                vec![
                    self.type_preview(theme),
                    self.setting_row(
                        theme,
                        &tr!("settings.interface_font"),
                        None,
                        None,
                        Some(self.font_family_select(
                            SettingsSelect::UiFontFamily,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.code_font"),
                        None,
                        None,
                        Some(self.font_family_select(
                            SettingsSelect::CodeFontFamily,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.ui_font_size"),
                        None,
                        None,
                        Some(self.preset_select(
                            SettingsSelect::UiFontSize,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.terminal_size"),
                        None,
                        None,
                        Some(self.preset_select(
                            SettingsSelect::TerminalFont,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.editor_size"),
                        None,
                        None,
                        Some(self.preset_select(
                            SettingsSelect::EditorFont,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.spacing_density"),
                        None,
                        None,
                        Some(self.preset_select(
                            SettingsSelect::SpacingDensity,
                            theme,
                            this.clone(),
                            cx,
                        )),
                    ),
                ],
            ),
            self.settings_section(
                theme,
                &tr!("settings.layout"),
                vec![
                    self.setting_row(
                        theme,
                        &tr!("settings.show_sidebar"),
                        Some(&tr!(
                            "settings.show_the_sessions_sidebar_also_toggleable_from_t",
                            shortcut = crate::platform::shortcuts::label(
                                crate::platform::shortcuts::SIDEBAR
                            )
                        )),
                        None,
                        Some(self.sidebar_toggle(theme, this.clone())),
                    ),
                    self.setting_row(
                        theme,
                        &tr!("settings.reduce_motion"),
                        Some(&tr!(
                            "settings.stop_looping_animations_spinners_and_the_running"
                        )),
                        None,
                        Some(self.settings_toggle(
                            "reduce-motion",
                            theme.ui.reduce_motion,
                            theme,
                            this.clone(),
                            Self::toggle_reduce_motion,
                        )),
                    ),
                ],
            ),
        ];
        // Tuning only means something once there is an image to tune, so the
        // board appears with it, directly under the background it belongs to.
        if background.is_some() {
            sections.insert(
                1,
                self.settings_section(
                    theme,
                    &tr!("settings.background_tuning"),
                    vec![
                        self.setting_row(
                            theme,
                            &tr!("settings.blur"),
                            Some(&tr!(
                                "settings.softens_the_picture_before_the_dither_pass_so_a_"
                            )),
                            None,
                            Some(self.blur_slider(
                                theme,
                                this.clone(),
                                crate::dither::tuning().blur,
                            )),
                        ),
                        self.setting_row(
                            theme,
                            &tr!("settings.pixel_size"),
                            Some(&tr!(
                                "settings.the_dither_cell_edge_fine_cells_read_as_halftone"
                            )),
                            None,
                            Some(self.tuning_select(
                                SettingsSelect::BackdropCell,
                                theme,
                                this.clone(),
                                cx,
                            )),
                        ),
                        self.setting_row(
                            theme,
                            &tr!("settings.bottom_fade"),
                            Some(&tr!(
                                "settings.how_far_the_picture_fades_into_the_page_behind_t"
                            )),
                            None,
                            Some(self.tuning_select(
                                SettingsSelect::BackdropFade,
                                theme,
                                this.clone(),
                                cx,
                            )),
                        ),
                    ],
                ),
            );
        }
        sections
    }
    /// One keyboard-focusable radio group drawn as a segmented control (the
    /// same control as the follow-up delivery mode); arrows choose the
    /// adjacent mode.
    fn appearance_mode_control(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use theme::AppearanceMode;
        let current = theme::appearance_prefs(cx).mode;
        let keyboard_this = this.clone();
        let count = AppearanceMode::ALL.len();
        div()
            .id("appearance-mode")
            .focusable()
            .border_1()
            .border_color(gpui::transparent_black())
            .rounded(Radius::Large.px(&theme))
            .p(DynamicSpacing::Base04.px(&theme))
            .focus(|style| style.border_color(theme.focus_ring))
            // gpui 0.2 has no `:focus-visible`: an automatic focus transfer
            // on mouse-down would leave the accent ring behind after a click.
            // Suppress that transfer so the ring only appears for keyboard
            // focus; the segment's own `on_click` still selects on mouse-up.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .flex()
            .items_center()
            .on_key_down(move |event, _, cx| {
                let index = AppearanceMode::ALL
                    .iter()
                    .position(|mode| *mode == current)
                    .unwrap_or(0);
                let next = match event.keystroke.key.as_str() {
                    "left" | "up" => (index + 2) % 3,
                    "right" | "down" => (index + 1) % 3,
                    "home" => 0,
                    "end" => 2,
                    _ => return,
                };
                cx.stop_propagation();
                keyboard_this.update(cx, |app, cx| {
                    app.set_appearance_mode(AppearanceMode::ALL[next], cx);
                });
            })
            .children(
                AppearanceMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        let this = this.clone();
                        let active = current == mode;
                        let label = match mode {
                            AppearanceMode::Light => tr!("settings.appearance_light"),
                            AppearanceMode::Dark => tr!("settings.appearance_dark"),
                            AppearanceMode::System => tr!("settings.appearance_system"),
                        };
                        segmented_segment(
                            div().id(mode.as_str()),
                            &theme,
                            ButtonSize::Medium,
                            SegmentPosition::at(index, count),
                        )
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .when(active, |b| {
                            b.border_color(theme.border)
                                .raised_flat(theme.selection)
                                .text_color(theme.text_primary)
                        })
                        .when(!active, |b| {
                            b.border_color(theme.border)
                                .raised_flat(theme.surface_elevated)
                                .text_color(theme.text_secondary)
                                .hover(|s| s.raised_flat(theme.surface_hover))
                        })
                        .on_click(move |_, _, cx| {
                            this.update(cx, |app, cx| app.set_appearance_mode(mode, cx));
                        })
                        .child(label)
                    }),
            )
            .into_any_element()
    }

    fn set_appearance_mode(&mut self, mode: theme::AppearanceMode, cx: &mut Context<Self>) {
        let mut prefs = theme::appearance_prefs(cx);
        prefs.mode = mode;
        if let Err(error) = theme::set_appearance_prefs(cx, prefs) {
            self.toast_error(tr!("settings.appearance_save_failed", error = error));
        }
        cx.notify();
    }

    /// Live previews of the chosen faces at their current sizes: the
    /// interface sample on the left, a mock terminal (chrome + prompt) on
    /// the right so the code/terminal font is legible at a glance without
    /// opening a session. Each is labelled as a preview.
    pub(super) fn type_preview(&self, theme: Theme) -> AnyElement {
        let column = |label: &str, body: AnyElement| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base06.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::XSmall.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(label.to_uppercase()),
                )
                .child(body)
                .into_any_element()
        };

        // ── interface sample ──
        let interface = div()
            .w_full()
            .flex_1()
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .bg(theme.background)
            .flex()
            .flex_col()
            .justify_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(TextSize::Large.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(tr!("settings.new_task")),
            )
            .child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(tr!("settings.the_quick_brown_fox_jumps_over_the_lazy_dog")),
            );

        // ── terminal sample ──
        let prompt = |command: &str, cursor: bool| {
            let mut row = div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .font_family(theme::code_font_family())
                .text_size(theme.term_px(12.5))
                .child(div().text_color(theme.success).child("$"))
                .child(div().text_color(theme.code_text).child(command.to_string()));
            if cursor {
                row = row.child(
                    div()
                        .w(theme.term_px(7.))
                        .h(theme.term_px(14.))
                        .rounded_full()
                        .bg(theme.text_secondary),
                );
            }
            row
        };
        let terminal = div()
            .w_full()
            .flex_1()
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.border)
            .bg(theme.code_bg)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(26.))
                    .px(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .children(
                        [theme.error, theme.warning, theme.success]
                            .map(|color| div().size(px(7.)).rounded_full().bg(color)),
                    )
                    .child(
                        div()
                            .ml_1p5()
                            .font_family(theme::code_font_family())
                            .text_size(theme.term_px(10.5))
                            .text_color(theme.text_muted)
                            .child(tr!("settings.terminal_prompt")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base03.px(&theme))
                    .child(prompt("orbit-pi --session", false))
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.term_px(12.5))
                            .text_color(theme.text_secondary)
                            .child(tr!("settings.preview_prompt")),
                    )
                    .child(prompt("", true)),
            );

        div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .flex()
            .gap(DynamicSpacing::Base16.px(&theme))
            .child(column(
                &tr!("settings.interface_preview"),
                interface.into_any_element(),
            ))
            .child(column(
                &tr!("settings.terminal_preview"),
                terminal.into_any_element(),
            ))
            .into_any_element()
    }

    /// Appearance → "Background image": pick an image for the dithered
    /// page backdrop, or reset to the dot grid.
    pub(super) fn background_controls(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let label = crate::dither::configured_label();
        let choose_label = if label.is_some() {
            tr!("settings.replace_ellipsis")
        } else {
            tr!("settings.choose_image")
        };
        let mut controls = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(self.runtime_button(
                "background-choose",
                &choose_label,
                false,
                theme,
                this.clone(),
                OrbitApp::background_choose,
            ));
        if label.is_some() {
            controls = controls.child(self.runtime_button(
                "background-reset",
                "Reset",
                false,
                theme,
                this,
                OrbitApp::background_reset,
            ));
        }
        controls.into_any_element()
    }

    /// Appearance → the backdrop's live preview: the real processed image
    /// (blur + dither cell) under the real bottom fade, at strip scale.
    /// Only mounted when an image is configured — without one the Theme
    /// swatches already show the palette, and the empty strip was a hole.
    pub(super) fn backdrop_preview(&self, theme: Theme) -> AnyElement {
        let image = crate::dither::background();
        let dithered = image.is_some();
        div()
            .w_full()
            .px(DynamicSpacing::Base16.px(&theme))
            .py(DynamicSpacing::Base12.px(&theme))
            .child(
                div()
                    .id("background-preview")
                    .debug_selector(|| "background-preview".to_string())
                    .relative()
                    .w_full()
                    .h(px(84.))
                    .rounded(Radius::XLarge.px(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .when(!dithered, |strip| strip.child(Self::dot_backdrop(theme)))
                    .children(Self::backdrop_image(image, crate::dither::tuning(), theme)),
            )
            .into_any_element()
    }

    /// Appearance → the backdrop blur: a continuous scrubber over
    /// `0..=dither::BLUR_MAX` sigma. It replaces the old preset dropdown so
    /// the wash under the picture can be dialled in rather than chosen from
    /// five looks. The value snaps to `dither::BLUR_STEP`, so a drag rebuilds
    /// the dither pass once per step instead of once per pixel.
    pub(super) fn blur_slider(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        sigma: f32,
    ) -> AnyElement {
        let norm = crate::dither::blur_norm(sigma);
        let track_w = px(BLUR_TRACK_W);
        let knob = px(BLUR_KNOB);
        let usable = track_w - knob;
        let knob_left = usable * norm;
        let center_x = knob_left + knob / 2.;

        let events = canvas(
            // One hitbox over the painted track; `is_hovered` respects the
            // occluding select popups this board can anchor under it.
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _cx| {
                register_blur_slider(this, bounds, hitbox, window);
            },
        )
        .absolute()
        .inset_0()
        .cursor_pointer();

        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .id("backdrop-blur-slider")
                    .debug_selector(|| "backdrop-blur-slider".to_string())
                    .relative()
                    .w(track_w)
                    .h(DynamicSpacing::Base20.px(&theme))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border),
                    )
                    .child(
                        div()
                            .id("backdrop-blur-fill")
                            .debug_selector(|| "backdrop-blur-fill".to_string())
                            .absolute()
                            .left_0()
                            .h(px(4.))
                            .w(center_x)
                            .rounded_full()
                            .bg(theme.accent),
                    )
                    .child(
                        div()
                            .id("backdrop-blur-knob")
                            .debug_selector(|| "backdrop-blur-knob".to_string())
                            .absolute()
                            .left(knob_left)
                            .size(knob)
                            .rounded_full()
                            .border_1()
                            .border_color(theme.border_strong)
                            .bg(theme.toggle_knob),
                    )
                    .child(events),
            )
            .child(
                div()
                    .w(px(40.))
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_secondary)
                    .child(format!("{}%", (norm * 100.).round() as u32)),
            )
            .into_any_element()
    }

    /// The background-tuning dropdowns (pixel size / bottom fade).
    /// Each option is a named preset, never a raw number, so the chip reads
    /// as a look — "Coarse", "Deep" — rather than a parameter.
    pub(super) fn tuning_select(
        &self,
        kind: SettingsSelect,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::dither::{CELL_LABELS, FADE_LABELS};
        let tuning = crate::dither::tuning();
        let (id, selected, labels): (&'static str, usize, &[&str]) = match kind {
            SettingsSelect::BackdropCell => (
                "backdrop-cell-select",
                tuning.cell_index(),
                &CELL_LABELS[..],
            ),
            SettingsSelect::BackdropFade => (
                "backdrop-fade-select",
                tuning.fade_index(),
                &FADE_LABELS[..],
            ),
            _ => unreachable!(),
        };
        self.select_control(
            id,
            kind,
            labels[selected].to_string(),
            labels.iter().map(|label| label.to_string()).collect(),
            selected,
            theme,
            this,
            cx,
        )
    }

    /// Pick the background image (native dialog), copy + process it, and
    /// warm the dither cache so the first paint of the new-task page is
    /// costless.
    pub(super) fn background_choose(&mut self, cx: &mut Context<Self>) {
        // Async panel only: see `OrbitApp::browse_for_folder` for why a
        // blocking native dialog on the main thread aborts the app.
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(tr!("settings.choose_background_title"))
            .add_filter(
                tr!("settings.images_filter"),
                &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff"],
            );
        cx.spawn(async move |this, cx| {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |app, cx| {
                match crate::dither::choose_file(&path) {
                    Ok(label) => {
                        crate::dither::background();
                        app.set_status(tr!("settings.background_set", label = label));
                    }
                    Err(err) => app.set_status(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop the background (the dot grid returns) and forget the cache.
    pub(super) fn background_reset(&mut self, cx: &mut Context<Self>) {
        crate::dither::clear_all();
        crate::dither::background();
        self.set_status(tr!("settings.background_reset"));
        cx.notify();
    }

    /// The real sidebar toggle, wired to the same state as the top bar.
    pub(super) fn sidebar_toggle(&self, theme: Theme, this: Entity<OrbitApp>) -> AnyElement {
        let on = self.sidebar_visible;
        div()
            .id("settings-sidebar-toggle")
            .w(px(36.))
            .h(DynamicSpacing::Base20.px(&theme))
            .rounded_full()
            .p(DynamicSpacing::Base02.px(&theme))
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(on, |t| t.bg(theme.accent).justify_end())
            .when(!on, |t| t.bg(theme.surface_elevated).justify_start())
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                this.update(cx, |app, cx| {
                    app.toggle_sidebar(window, cx);
                    cx.notify();
                });
            })
            .child(div().size(px(14.)).rounded_full().bg(theme.toggle_knob))
            .into_any_element()
    }

    /// The palettes the Theme dropdown lists, in the order it paints them:
    /// every palette of `mode`, in [`theme::ThemeId::ALL`] order.
    ///
    /// Both the label builder and the click handler read this, so a row index
    /// always resolves to the palette it painted.
    pub(super) fn theme_catalog(
        &self,
        mode: ThemeMode,
        _cx: &Context<Self>,
    ) -> Vec<theme::ThemeId> {
        theme::themes_for(mode).collect()
    }

    /// Appearance → Theme. `mode` is the resolved appearance, so the row shows
    /// and edits the palette currently in effect and each mode's choice is
    /// remembered separately.
    pub(super) fn theme_select(
        &self,
        mode: ThemeMode,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let catalog = self.theme_catalog(mode, cx);
        let configured = theme::appearance_prefs(cx).theme(mode);
        let selected = catalog.iter().position(|id| *id == configured).unwrap_or(0);
        let options: Vec<String> = catalog.iter().map(|id| id.label().to_string()).collect();
        self.select_control(
            "theme-select",
            SettingsSelect::Theme(mode),
            configured.label().to_string(),
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    // ── General-settings selects (language / font sizes) ──────────────

    /// The Language dropdown, listing every shipped locale (autonyms) plus
    /// `System`.
    pub(super) fn language_select(
        &self,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::theme::Language;
        let selected = Language::ALL
            .iter()
            .position(|language| *language == theme.ui.language)
            .unwrap_or(0);
        self.select_control(
            "language-select",
            SettingsSelect::Language,
            theme.ui.language.label(),
            Language::ALL
                .iter()
                .map(|language| language.label())
                .collect(),
            selected,
            theme,
            this,
            cx,
        )
    }

    /// The percentage / px preset dropdowns in the type & density board.
    pub(super) fn preset_select(
        &self,
        kind: SettingsSelect,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::theme::{FONT_SIZES, SPACING_DENSITIES};
        let (values, current, suffix): (Vec<f32>, f32, &str) = match kind {
            SettingsSelect::UiFontSize => (FONT_SIZES.to_vec(), theme.ui.ui_font_size, "px"),
            SettingsSelect::TerminalFont => {
                (FONT_SIZES.to_vec(), theme.ui.terminal_font_size, "px")
            }
            SettingsSelect::EditorFont => (FONT_SIZES.to_vec(), theme.ui.editor_font_size, "px"),
            SettingsSelect::SpacingDensity => (
                SPACING_DENSITIES.iter().map(|v| *v as f32).collect(),
                theme.ui.spacing_density as f32,
                "%",
            ),
            SettingsSelect::Language
            | SettingsSelect::Theme(_)
            | SettingsSelect::UiFontFamily
            | SettingsSelect::CodeFontFamily
            | SettingsSelect::BackdropCell
            | SettingsSelect::BackdropFade
            | SettingsSelect::TitleModel
            | SettingsSelect::DefaultModel
            | SettingsSelect::DefaultThinking
            | SettingsSelect::ActiveModel
            | SettingsSelect::ModelProvider
            | SettingsSelect::RetentionMode
            | SettingsSelect::RetentionDays => unreachable!(),
        };
        let selected = values
            .iter()
            .position(|v| (*v - current).abs() < 0.01)
            .unwrap_or(0);
        self.select_control(
            match kind {
                SettingsSelect::UiFontSize => "ui-font-size-select",
                SettingsSelect::TerminalFont => "terminal-font-select",
                SettingsSelect::EditorFont => "editor-font-select",
                SettingsSelect::SpacingDensity => "spacing-density-select",
                _ => "settings-select",
            },
            kind,
            format!("{} {}", current as u32, suffix),
            values
                .iter()
                .map(|v| format!("{} {}", *v as u32, suffix))
                .collect(),
            selected,
            theme,
            this,
            cx,
        )
    }

    /// The Interface / Code font-family dropdowns — Orbit's curated catalog.
    pub(super) fn font_family_select(
        &self,
        kind: SettingsSelect,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::theme::{FontChoice, CODE_FONTS, UI_FONTS};
        let prefs = theme::font_prefs();
        let (id, current, choices): (&'static str, SharedString, &[FontChoice]) = match kind {
            SettingsSelect::UiFontFamily => (
                "ui-font-family-select",
                prefs.ui_font_family.clone(),
                &UI_FONTS,
            ),
            SettingsSelect::CodeFontFamily => (
                "code-font-family-select",
                prefs.code_font_family.clone(),
                &CODE_FONTS,
            ),
            _ => unreachable!(),
        };
        let selected = choices
            .iter()
            .position(|f| f.family == current.as_ref())
            .unwrap_or(0);
        self.select_control(
            id,
            kind,
            theme::font_choice_label(current.as_ref()),
            choices.iter().map(|f| f.label.to_string()).collect(),
            selected,
            theme,
            this,
            cx,
        )
    }

    /// A select control: value chip + caret, dropdown below when open.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_control(
        &self,
        id: &'static str,
        kind: SettingsSelect,
        label: String,
        options: Vec<String>,
        selected: usize,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.select_control_sized(
            id,
            kind,
            SELECT_CHIP_SIZE,
            label,
            options,
            selected,
            theme,
            this,
            cx,
        )
    }

    /// [`select_control`](Self::select_control) at an explicit chip size, so a
    /// toolbar control can match the 32px action buttons beside it while the
    /// settings rows keep the 28px chip.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_control_sized(
        &self,
        id: &'static str,
        kind: SettingsSelect,
        size: ButtonSize,
        label: String,
        options: Vec<String>,
        selected: usize,
        theme: Theme,
        this: Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.settings_select == Some(kind);
        let chip_this = this.clone();
        div()
            .flex()
            .flex_col()
            .items_end()
            // dropdown anchored below the chip when open
            .children(self.settings_select_popup(kind, size, options, selected, theme, &this, cx))
            .child(
                button_frame(div().id(ElementId::Name(id.into())), &theme, size)
                    .border_1()
                    .border_color(if open {
                        theme.border_strong
                    } else {
                        theme.border
                    })
                    .raised(
                        if open {
                            theme.overlay
                        } else {
                            theme.surface_elevated
                        },
                        &theme,
                    )
                    .cursor_pointer()
                    .text_color(theme.text_secondary)
                    .hover(|s| s.raised(theme.overlay, &theme))
                    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                        chip_this.update(cx, |app, cx| {
                            // A dismissal from this same click's mouse-down
                            // must not immediately re-open (see
                            // `toggle_session_menu`).
                            const GESTURE: Duration = Duration::from_millis(200);
                            if let Some(dismissed) = app.menu_dismissed_at.take() {
                                if dismissed.elapsed() < GESTURE {
                                    return;
                                }
                            }
                            if app.settings_select == Some(kind) {
                                app.settings_select = None;
                            } else {
                                app.settings_select = Some(kind);
                                // Open with the cursor on the chosen option,
                                // scrolled into view in the list below.
                                app.settings_select_highlight = Some(selected);
                                app.settings_select_scroll
                                    .scroll_to_item(selected, ScrollStrategy::Center);
                                app.settings_filter
                                    .update(cx, |filter, cx| filter.clear(cx));
                                let handle = app.settings_filter.read(cx).focus_handle(cx);
                                window.focus(&handle);
                            }
                            cx.notify();
                        });
                    })
                    .child(label)
                    .child(icon(
                        "icons/chevron-down.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    )),
            )
            .into_any_element()
    }

    /// The open dropdown's option list, anchored below its chip.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn settings_select_popup(
        &self,
        kind: SettingsSelect,
        size: ButtonSize,
        options: Vec<String>,
        selected: usize,
        theme: Theme,
        this: &Entity<OrbitApp>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if self.settings_select != Some(kind) {
            return None;
        }
        // Filter options, keeping the original index for click dispatch.
        // The cursor shares `settings_select_visible`, so the painted rows
        // and the keyboard cursor can never disagree about the query.
        let rows: Vec<(usize, String)> = self
            .settings_select_visible(&options, cx)
            .into_iter()
            .map(|ix| (ix, options[ix].clone()))
            .collect();
        let empty = rows.is_empty();
        let highlight = self.settings_select_highlight;
        // Virtualized list — only the visible rows are laid out and painted.
        // The font-family dropdowns can have a thousand+ entries, and the
        // whole app re-renders on every scroll tick, so rendering every row
        // per frame is what made the theme/font selectors lag while scrolling.
        //
        // `uniform_list`'s Infer sizing reads the *available* height, and this
        // popup lives inside a 0×0 anchor div, so the available height is 0 and
        // the list collapses to nothing. Give it an explicit height computed
        // from the row count instead (one picker entry per row plus the
        // list's vertical padding, capped at the old 220 px max) — a real
        // viewport. Rows and height share `row_h`, so they cannot drift.
        let row_h = picker::entry_height(&theme);
        let list_pad_y = picker::list_padding_y(&theme);
        let list_h = (row_h * rows.len() as f32 + list_pad_y * 2.).min(px(220.));
        let list: AnyElement = if empty {
            div()
                .w_full()
                .py(list_pad_y)
                .child(
                    picker_entry(div(), &theme)
                        .text_color(theme.text_muted)
                        .child(tr!("settings.no_matches")),
                )
                .into_any_element()
        } else {
            let this = this.clone();
            uniform_list(
                "settings-select-list",
                rows.len(),
                move |range, _window, _cx| {
                    let mut children = Vec::new();
                    for ix in range {
                        let (orig_ix, option) = &rows[ix];
                        let orig_ix = *orig_ix;
                        let selected_row = orig_ix == selected;
                        let highlighted_row = highlight == Some(orig_ix);
                        let this = this.clone();
                        // Every row is exactly one picker entry tall
                        // (`uniform_list` strides by the first row, and
                        // `list_h` uses the same `row_h`). The block shell
                        // fits the entry's inset margins inside the list.
                        let row_id =
                            ElementId::NamedInteger("settings-select-row".into(), orig_ix as u64);
                        children.push(
                            div().h(row_h).w_full().child(
                                picker_entry(div().id(row_id), &theme)
                                    .h(row_h)
                                    .justify_between()
                                    .cursor_pointer()
                                    // The chosen option keeps the fill; the
                                    // keyboard cursor rides the same wash.
                                    .when(highlighted_row || selected_row, |row| {
                                        row.bg(theme.surface_active)
                                    })
                                    .when(!highlighted_row && !selected_row, |row| {
                                        row.hover(|style| style.bg(theme.surface_hover))
                                    })
                                    .text_color(if selected_row || highlighted_row {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(option.clone())
                                    .when(selected_row, |row| {
                                        row.child(icon(
                                            "icons/check.svg",
                                            context_menu::ICON.px(&theme),
                                            theme.accent_text,
                                        ))
                                    })
                                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                                        this.update(cx, |app, cx| {
                                            if app.apply_settings_select(kind, orig_ix, cx) {
                                                app.settings_select = None;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            ),
                        );
                    }
                    children
                },
            )
            .w_full()
            .h(list_h)
            .track_scroll(self.settings_select_scroll.clone())
            .py(list_pad_y)
            .into_any_element()
        };
        let popup = picker_surface(div(), &theme)
            .min_w(px(360.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_mouse_down_out({
                let this = this.clone();
                move |_: &MouseDownEvent, _, cx: &mut App| {
                    this.update(cx, |app, cx| {
                        if app.settings_select.take().is_some() {
                            // Arm the click-through guard so this same click's
                            // mouse-up on the chip cannot immediately re-open
                            // the dropdown.
                            app.menu_dismissed_at = Some(Instant::now());
                            cx.notify();
                        }
                    });
                }
            })
            // The filter input carries the `Picker` context, so these ride
            // the same dispatch node as the model picker's: arrows move the
            // cursor, Enter chooses it, Escape (global) closes.
            .on_action({
                let this = this.clone();
                let options = options.clone();
                move |_: &crate::PickerSelectPrev, _, cx: &mut App| {
                    this.update(cx, |app, cx| app.settings_select_step(&options, -1, cx));
                }
            })
            .on_action({
                let this = this.clone();
                let options = options.clone();
                move |_: &crate::PickerSelectNext, _, cx: &mut App| {
                    this.update(cx, |app, cx| app.settings_select_step(&options, 1, cx));
                }
            })
            .on_action({
                let this = this.clone();
                let options = options.clone();
                move |_: &crate::PickerConfirm, _, cx: &mut App| {
                    this.update(cx, |app, cx| app.settings_select_confirm(&options, cx));
                }
            })
            // Search field — filters the options below.
            .child(
                picker_search_frame(div(), &theme)
                    .child(icon(
                        "icons/search.svg",
                        input::ICON.px(&theme),
                        theme.text_muted,
                    ))
                    .child(div().flex_1().min_w_0().child(self.settings_filter.clone())),
            )
            .child(list);
        // Anchor to the chip's top-right corner (via a zero-size point) and
        // drop the popup `popover::MENU_OFFSET` below the chip
        // (`SELECT_CHIP_SIZE`), right-aligned. `Window`
        // position mode (not `Local`) is deliberate: `anchored`'s switch-anchor
        // fit then flips the popup above the chip when the viewport bottom is
        // closer than the list is tall, instead of sliding it down over its own
        // chip (which made a second click land inside the popup). The
        // post-switch snap still clamps at the window edges as a backstop.
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .size(px(0.))
                .child(
                    anchored()
                        .position_mode(AnchoredPositionMode::Window)
                        .anchor(Corner::TopRight)
                        .offset(point(px(0.), size.height(&theme) + popover::MENU_OFFSET))
                        .child(deferred(popup)),
                )
                .into_any_element(),
        )
    }

    /// Move the settings dropdown's keyboard cursor one visible option,
    /// clamped at the ends like the model picker. A query owns the list, so
    /// a cursor the filter dropped lands on the first (or last) match rather
    /// than doing nothing, and the list keeps it in view.
    pub(super) fn settings_select_step(
        &mut self,
        options: &[String],
        dir: isize,
        cx: &mut Context<Self>,
    ) {
        let visible = self.settings_select_visible(options, cx);
        let Some(next) = stepped_visible_position(&visible, self.settings_select_highlight, dir)
        else {
            return;
        };
        self.settings_select_highlight = Some(visible[next]);
        self.settings_select_scroll
            .scroll_to_item(next, ScrollStrategy::Center);
        cx.notify();
    }

    /// Choose the option under the settings dropdown's keyboard cursor (or
    /// the first match when the query filtered the cursor away) and close
    /// the popup.
    pub(super) fn settings_select_confirm(&mut self, options: &[String], cx: &mut Context<Self>) {
        let Some(kind) = self.settings_select else {
            return;
        };
        let visible = self.settings_select_visible(options, cx);
        let ix = self
            .settings_select_highlight
            .filter(|ix| visible.contains(ix))
            .or_else(|| visible.first().copied());
        let Some(ix) = ix else {
            return;
        };
        // A disclosure row ("All themes…") only reshapes the list, so the
        // dropdown stays open on what it just revealed.
        if self.apply_settings_select(kind, ix, cx) {
            self.settings_select = None;
        }
        cx.notify();
    }

    /// The original indices of `options` that survive the filter — the rows
    /// the popup actually paints, in display order.
    fn settings_select_visible(&self, options: &[String], cx: &Context<Self>) -> Vec<usize> {
        let needle = self.settings_filter.read(cx).text().to_lowercase();
        options
            .iter()
            .enumerate()
            .filter(|(_, option)| needle.is_empty() || option.to_lowercase().contains(&needle))
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Apply a dropdown choice to the persisted UI customization.
    pub(super) fn apply_settings_select(
        &mut self,
        kind: SettingsSelect,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        match kind {
            SettingsSelect::Theme(mode) => {
                let catalog = self.theme_catalog(mode, cx);
                let Some(id) = catalog.get(ix).copied() else {
                    return true;
                };
                let mut prefs = theme::appearance_prefs(cx);
                prefs.set_theme(mode, id);
                if let Err(error) = theme::set_appearance_prefs(cx, prefs) {
                    self.toast_error(tr!("settings.appearance_save_failed", error = error));
                }
                return true;
            }
            SettingsSelect::UiFontFamily | SettingsSelect::CodeFontFamily => {
                use crate::theme::{CODE_FONTS, UI_FONTS};
                let choices = match kind {
                    SettingsSelect::UiFontFamily => &UI_FONTS[..],
                    SettingsSelect::CodeFontFamily => &CODE_FONTS[..],
                    _ => unreachable!(),
                };
                let Some(choice) = choices.get(ix) else {
                    return true;
                };
                let mut prefs = theme::font_prefs();
                match kind {
                    SettingsSelect::UiFontFamily => prefs.ui_font_family = choice.family.into(),
                    SettingsSelect::CodeFontFamily => prefs.code_font_family = choice.family.into(),
                    _ => unreachable!(),
                }
                theme::set_font_prefs(prefs);
                return true;
            }
            SettingsSelect::TitleModel => {
                let model = if ix == 0 {
                    None
                } else {
                    self.available_models
                        .get(ix - 1)
                        .map(|model| crate::auto_title::TitleModel {
                            provider: model.provider.clone(),
                            id: model.id.clone(),
                        })
                };
                self.auto_title.model = model;
                self.auto_title.persist();
                cx.notify();
                return true;
            }
            SettingsSelect::DefaultModel => {
                let model = if ix == 0 {
                    None
                } else {
                    self.available_models.get(ix - 1)
                };
                let slot = match model {
                    Some(model) => crate::session_defaults::SessionDefault {
                        provider: Some(model.provider.clone()),
                        model_id: Some(model.id.clone()),
                        // Keep the level only when the new model supports it;
                        // a bare level must not land on an arbitrary model.
                        thinking: self.session_default.thinking.clone().filter(|level| {
                            model
                                .thinking_levels
                                .iter()
                                .any(|candidate| candidate == level)
                        }),
                    },
                    None => crate::session_defaults::SessionDefault::default(),
                };
                self.set_default_model(slot);
                cx.notify();
                return true;
            }
            SettingsSelect::DefaultThinking => {
                let level = if ix == 0 {
                    None
                } else {
                    self.default_model_entry()
                        .and_then(|model| model.thinking_levels.get(ix - 1))
                        .cloned()
                };
                let mut slot = self.session_default.clone();
                slot.thinking = level;
                self.set_default_model(slot);
                cx.notify();
                return true;
            }
            SettingsSelect::ActiveModel => {
                if let Some(model) = self.available_models.get(ix) {
                    let (id, provider) = (model.id.clone(), model.provider.clone());
                    self.set_model(id, provider, cx);
                }
                return true;
            }
            SettingsSelect::ModelProvider => {
                // Index 0 is `All providers`; the rest follow the catalog's
                // provider order, the same order the dropdown was built from.
                self.models_provider_filter = if ix == 0 {
                    None
                } else {
                    self.model_providers().get(ix - 1).cloned()
                };
                cx.notify();
                return true;
            }
            SettingsSelect::BackdropCell | SettingsSelect::BackdropFade => {
                let mut tuning = crate::dither::tuning();
                match kind {
                    SettingsSelect::BackdropCell => {
                        tuning.cell = crate::dither::CELL_SIZES
                            .get(ix)
                            .copied()
                            .unwrap_or(tuning.cell);
                    }
                    SettingsSelect::BackdropFade => {
                        tuning.fade = crate::dither::FADE_HEIGHTS
                            .get(ix)
                            .copied()
                            .unwrap_or(tuning.fade);
                    }
                    _ => unreachable!(),
                }
                crate::dither::set_tuning(tuning);
                // The cache is keyed on the tuning, so rebuild the pass now:
                // this frame's preview and the next new-task paint both hit a
                // warm cache instead of stalling on the first draw.
                crate::dither::background();
                return true;
            }
            SettingsSelect::RetentionMode => {
                self.retention.mode = match ix {
                    1 => crate::session_retention::RetentionMode::Delete,
                    _ => crate::session_retention::RetentionMode::Archive,
                };
                self.retention.persist();
                cx.notify();
                return true;
            }
            SettingsSelect::RetentionDays => {
                if let Some(days) = crate::session_retention::DAY_PRESETS.get(ix) {
                    self.retention.days = *days;
                    self.retention.persist();
                }
                cx.notify();
                return true;
            }
            _ => {}
        }
        use crate::theme::{Language, FONT_SIZES, SPACING_DENSITIES};
        let mut ui = theme::get(cx).ui;
        match kind {
            SettingsSelect::Language => {
                ui.language = Language::ALL.get(ix).copied().unwrap_or(Language::System);
            }
            SettingsSelect::UiFontSize => {
                ui.ui_font_size = FONT_SIZES.get(ix).copied().unwrap_or(14.);
            }
            SettingsSelect::TerminalFont => {
                ui.terminal_font_size = FONT_SIZES.get(ix).copied().unwrap_or(13.);
            }
            SettingsSelect::EditorFont => {
                ui.editor_font_size = FONT_SIZES.get(ix).copied().unwrap_or(13.);
            }
            SettingsSelect::SpacingDensity => {
                ui.spacing_density = SPACING_DENSITIES.get(ix).copied().unwrap_or(100);
            }
            SettingsSelect::Theme(_)
            | SettingsSelect::UiFontFamily
            | SettingsSelect::CodeFontFamily
            | SettingsSelect::BackdropCell
            | SettingsSelect::BackdropFade
            | SettingsSelect::TitleModel
            | SettingsSelect::DefaultModel
            | SettingsSelect::DefaultThinking
            | SettingsSelect::ActiveModel
            | SettingsSelect::ModelProvider
            | SettingsSelect::RetentionMode
            | SettingsSelect::RetentionDays => unreachable!(),
        }
        theme::set_ui_prefs(cx, ui);
        true
    }
}

// ── controller ────────────────────────────────────────────────────
impl OrbitApp {
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.refresh_updater(cx);
        self.settings_open = true;
        self.set_settings_section(SettingsSection::General, cx);
        self.sync_terminal_active(cx);
    }

    pub(super) fn on_open_settings(
        &mut self,
        _: &crate::OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(cx);
    }

    /// App-menu “About Orbit Pi”: the same surface Settings → About owns, so
    /// there is one place that states the versions and upstream projects.
    pub(super) fn on_open_about(
        &mut self,
        _: &crate::OpenAbout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_updater(cx);
        self.settings_open = true;
        self.set_settings_section(SettingsSection::About, cx);
        self.sync_terminal_active(cx);
    }

    pub(super) fn on_settings_gear_click(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Toggle: the gear sits in the sessions sidebar, which stays visible
        // while settings is open, so clicking it again should go back.
        if self.settings_open {
            self.settings_open = false;
            self.provider_editor = None;
            self.provider_key_editor = None;
            self.sync_terminal_active(cx);
        } else {
            self.open_settings(cx);
            return;
        }
        cx.notify();
    }

    pub(super) fn on_settings_back(
        &mut self,
        _: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
        self.provider_editor = None;
        self.provider_key_editor = None;
        self.input.read(cx).focus(window);
        self.sync_terminal_active(cx);
        cx.notify();
    }

    /// Switch sections, reloading the on-disk facts each page reads so CLI
    /// edits appear without a restart.
    pub(super) fn set_settings_section(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = section;
        self.provider_remove_confirm = None;
        self.provider_editor = None;
        self.provider_key_editor = None;
        self.plugin_remove_confirm = None;
        // The MCP add/edit modal belongs to the MCP page; leaving the page
        // discards it like the provider editors above.
        self.mcp_editor = None;
        self.mcp_remove_confirm = None;
        match section {
            SettingsSection::General => self.refresh_notification_auth(cx),
            SettingsSection::Providers => {
                // Paint from cached state now; the file re-read (models.json,
                // auth.json) lands after this frame.
                self.reload_custom_providers_later(cx);
                self.refresh_auth();
            }
            SettingsSection::Privacy => self.refresh_temp_files(cx),
            SettingsSection::Skills => self.refresh_skills(cx),
            SettingsSection::Plugins => self.refresh_plugins(cx),
            SettingsSection::Mcp => self.mcp_section_opened(cx),
            _ => {}
        }
        cx.notify();
    }

    /// The directory new tasks and discovery use — the workspace when one is
    /// selected, else the process cwd.
    pub(super) fn workspace_dir(&self) -> PathBuf {
        self.current_workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// Re-read the user and project settings `packages` arrays.
    pub(super) fn refresh_plugins(&mut self, cx: &mut Context<Self>) {
        let (packages, error) = crate::plugins::discover(&self.workspace_dir());
        self.plugins = packages;
        self.plugins_error = error;
        cx.notify();
    }

    /// Install the source currently in the toolbar field.
    pub(super) fn plugin_install(&mut self, cx: &mut Context<Self>) {
        if self.plugin_action.is_some() {
            return;
        }
        let source = self.plugin_source_input.read(cx).text().trim().to_string();
        if source.is_empty() {
            self.toast_warning(tr!("settings.enter_package_source"));
            cx.notify();
            return;
        }
        self.plugin_run(PluginOp::Install, source, self.plugin_install_project, cx);
    }

    /// Run a plugin operation off the UI thread, then reload the list.
    fn plugin_run(&mut self, op: PluginOp, source: String, project: bool, cx: &mut Context<Self>) {
        if self.plugin_action.is_some() {
            return;
        }
        let verb = match op {
            PluginOp::Install => tr!("settings.verb_installing"),
            PluginOp::Update => tr!("settings.verb_updating"),
            PluginOp::Remove => tr!("settings.verb_removing"),
        };
        self.plugin_action = Some(tr!(
            "settings.plugin_progress",
            verb = verb,
            source = source
        ));
        let done = tr!(
            "settings.plugin_progress_plain",
            verb = verb,
            source = source
        );
        let workspace = self.workspace_dir();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match op {
                        PluginOp::Install => crate::plugins::install(&source, project, &workspace),
                        PluginOp::Update => crate::plugins::update(&source, &workspace),
                        PluginOp::Remove => crate::plugins::remove(&source, project, &workspace),
                    }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.plugin_action = None;
                match result {
                    Ok(output) => {
                        let last = output
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .map(str::to_string);
                        app.toast_success(
                            last.unwrap_or_else(|| tr!("settings.plugin_done", done = done)),
                        );
                        app.refresh_plugins(cx);
                    }
                    Err(err) => app.set_error(err),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Single dispatch point for every Plugins-page control.
    pub(super) fn apply_plugin_action(&mut self, action: PluginAction, cx: &mut Context<Self>) {
        match action {
            PluginAction::Install => self.plugin_install(cx),
            PluginAction::Update { source } => {
                self.plugin_run(PluginOp::Update, source, false, cx);
            }
            PluginAction::Remove { source } => {
                self.plugin_remove_confirm = Some(source);
                cx.notify();
            }
            PluginAction::ConfirmRemove { source, project } => {
                self.plugin_remove_confirm = None;
                self.plugin_run(PluginOp::Remove, source, project, cx);
            }
            PluginAction::CancelRemove => {
                self.plugin_remove_confirm = None;
                cx.notify();
            }
            PluginAction::SetScope { project } => {
                self.plugin_install_project = project;
                cx.notify();
            }
            PluginAction::Refresh => {
                self.refresh_plugins(cx);
                self.check_plugin_updates_now(cx);
                self.toast_info(tr!("settings.reloaded_installed_plugins"));
                self.plugin_refresh_spin_until = Some(Instant::now() + PLUGIN_REFRESH_FEEDBACK);
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(PLUGIN_REFRESH_FEEDBACK)
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.plugin_refresh_spin_until = None;
                        cx.notify();
                    });
                })
                .detach();
                cx.notify();
            }
        }
    }

    /// Apply the result of reading both provider files to state. Read errors
    /// are kept, not fatal, so a broken file can be seen and fixed rather than
    /// overwritten.
    fn apply_provider_reads(
        &mut self,
        custom: Result<Vec<CustomProvider>, String>,
        auth: Result<HashMap<String, providers::ProviderAuth>, String>,
    ) {
        match custom {
            Ok(list) => {
                self.custom_providers = list;
                self.custom_providers_error = None;
            }
            Err(err) => {
                self.custom_providers = Vec::new();
                self.custom_providers_error = Some(err);
            }
        }
        match auth {
            Ok(auth) => {
                self.provider_auth = auth;
                self.provider_auth_error = None;
            }
            Err(err) => {
                self.provider_auth = HashMap::new();
                self.provider_auth_error = Some(err);
            }
        }
    }

    /// Re-read `~/.pi/agent/models.json` and `~/.pi/agent/auth.json` on the
    /// calling thread. Used by explicit actions (Refresh, save/remove,
    /// credential load) where the result gates the next step.
    pub(super) fn reload_custom_providers(&mut self, cx: &mut Context<Self>) {
        let custom = providers::read_custom();
        let auth = providers::read_auth();
        self.apply_provider_reads(custom, auth);
        self.ensure_provider_metadata(cx);
        cx.notify();
    }

    /// The same re-read, but off the UI thread: the Providers page paints from
    /// cached state first and the fresh files land a frame later, so clicking
    /// the nav row never blocks on disk (no-blink page switch).
    pub(super) fn reload_custom_providers_later(&mut self, cx: &mut Context<Self>) {
        // Idempotent, already off-thread; kick it now so a first visit still
        // has metadata as soon as it arrives.
        self.ensure_provider_metadata(cx);
        cx.spawn(async move |this, cx| {
            let (custom, auth) = cx
                .background_executor()
                .spawn(async { (providers::read_custom(), providers::read_auth()) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.apply_provider_reads(custom, auth);
                cx.notify();
            });
        })
        .detach();
    }

    /// Load pi's built-in catalog sizes and authoritative provider metadata
    /// once, off the UI thread (node introspection + ~600 KB of JSON parsing
    /// would otherwise hitch the first Providers render).
    pub(super) fn ensure_provider_metadata(&mut self, cx: &mut Context<Self>) {
        if self.provider_metadata_loaded {
            return;
        }
        self.provider_metadata_loaded = true;
        cx.spawn(async move |this, cx| {
            let (counts, metadata) = cx
                .background_executor()
                .spawn(async {
                    let counts = providers::builtin_catalog_counts().clone();
                    let metadata = providers::dynamic_providers()
                        .map(<[providers::DynamicProvider]>::to_vec)
                        .unwrap_or_default();
                    (counts, metadata)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.provider_catalog_counts = counts;
                app.provider_metadata = metadata;
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh: re-query the running agent's catalog and re-read both files.
    pub(super) fn provider_refresh(&mut self, cx: &mut Context<Self>) {
        self.providers_refreshing = true;
        self.refresh_catalogs();
        self.reload_custom_providers(cx);
        self.refresh_auth();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.providers_refreshing = false;
                cx.notify();
            });
        })
        .detach();
        self.toast_info(tr!("settings.refreshed_provider_catalog"));
        cx.notify();
    }

    /// Restart pi so a newly written credential is loaded. pi reads
    /// `auth.json` only at startup, so this is the "use it" step. Resumes the
    /// active session on the fresh process when one exists.
    pub(super) fn provider_apply_credentials(&mut self, cx: &mut Context<Self>) {
        // Never yank the process out from under a live turn.
        if self.busy || self.transcript.is_streaming() {
            self.toast_warning(tr!("settings.finish_turn_before_restart"));
            cx.notify();
            return;
        }
        let resume = self
            .current_session_path
            .clone()
            .and_then(|path| self.sessions.iter().find(|s| s.path == path).cloned());
        self.drop_client();
        self.current_session_path = None;
        if let Some(session) = resume {
            self.switch_to_session(session, false, cx);
        } else {
            self.runtime_start(cx);
        }
        self.provider_auth_dirty = false;
        self.reload_custom_providers(cx);
        self.toast_success(tr!("settings.pi_restarted_credentials_loaded"));
        cx.notify();
    }

    /// Open the API-key editor for a provider.
    pub(super) fn provider_key_open(
        &mut self,
        provider_id: String,
        provider_name: String,
        oauth: bool,
        note: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.provider_credential_open(
            provider_id,
            provider_name,
            oauth,
            note,
            ProviderKeyKind::ApiKey,
            window,
            cx,
        );
    }

    /// Open the credential editor for `kind` (API key or an Ollama Cloud
    /// session). One modal handles both; the field, hint, and save path differ.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn provider_credential_open(
        &mut self,
        provider_id: String,
        provider_name: String,
        oauth: bool,
        note: &'static str,
        kind: ProviderKeyKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            ProviderKeyKind::ApiKey => "sk-…",
            ProviderKeyKind::OllamaCloudSession => "__Secure-session=…",
        };
        let key = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-key-input")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
                .with_placeholder(placeholder)
        });
        let focus = key.read(cx).focus_handle(cx);
        window.focus(&focus);
        self.provider_key_editor = Some(ProviderKeyEditor {
            provider_id,
            provider_name,
            oauth,
            note,
            kind,
            key,
            error: None,
        });
        cx.notify();
    }

    /// Save the API key, then flag that pi needs a restart to load it.
    pub(super) fn provider_key_save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.provider_key_editor.as_ref() else {
            return;
        };
        let id = editor.provider_id.clone();
        let name = editor.provider_name.clone();
        let kind = editor.kind;
        let key = editor.key.read(cx).text();
        let result = match kind {
            ProviderKeyKind::ApiKey => providers::write_api_key(&id, &key),
            ProviderKeyKind::OllamaCloudSession => providers::write_ollama_cloud_session(&key),
        };
        match result {
            Ok(()) => {
                self.provider_key_editor = None;
                self.provider_auth_dirty = true;
                self.reload_custom_providers(cx);
                let what = match kind {
                    ProviderKeyKind::ApiKey => tr!("settings.api_key"),
                    ProviderKeyKind::OllamaCloudSession => tr!("settings.ollama_cloud_session"),
                };
                self.toast_success(tr!("settings.key_saved_for", what = what, name = name));
            }
            Err(err) => {
                if let Some(editor) = self.provider_key_editor.as_mut() {
                    editor.error = Some(err);
                }
            }
        }
        cx.notify();
    }

    /// Sign in with OAuth by handing `pi /login <id>` to the user's terminal.
    pub(super) fn provider_oauth_login(
        &mut self,
        id: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        // The id reaches a shell script; only builtin ids are ever passed, but
        // validate anyway so a hand-edited file can never inject a command.
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            self.toast_error(tr!("settings.refuse_invalid_login", id = id));
            cx.notify();
            return;
        }
        let command = format!("pi /login {id}");
        match platform::open_terminal_command(&command) {
            Ok(()) => {
                self.provider_auth_dirty = true;
                self.toast_info(tr!("settings.finish_sign_in_terminal", name = name));
            }
            Err(err) => {
                self.toast_warning(tr!(
                    "settings.terminal_failed_hint",
                    error = err,
                    command = command
                ));
            }
        }
        cx.notify();
    }

    /// Sign out. With auth RPC available pi owns the credential store and
    /// removes it live; otherwise fall back to dropping the auth.json entry
    /// (env credentials are outside Orbit's reach and left alone).
    pub(super) fn provider_sign_out(&mut self, id: String, cx: &mut Context<Self>) {
        // The Ollama Cloud session lives under its own auth.json key, which
        // pi's provider logout does not know about; clear it so Disconnect
        // drops the whole credential. Both Ollama ids share the session.
        if providers::is_ollama_cloud_provider(&id) {
            if let Err(err) = providers::remove_ollama_session() {
                self.provider_auth_error = Some(err);
            }
        }
        if self.auth.support() == AuthSupport::Supported {
            self.auth.on_logout_response(true, &id);
            self.send(
                CommandBody::AuthLogout {
                    provider: id.clone(),
                },
                "auth.logout",
            );
            self.toast_info(tr!("settings.signing_out", id = id));
            cx.notify();
            return;
        }
        match providers::remove_auth(&id) {
            Ok(()) => {
                self.provider_auth_dirty = true;
                self.reload_custom_providers(cx);
                self.toast_success(tr!("settings.signed_out", id = id));
            }
            Err(err) => {
                self.provider_auth_error = Some(err);
            }
        }
        cx.notify();
    }

    /// Open the editor for an existing provider id, or a blank add form.
    pub(super) fn provider_editor_open(
        &mut self,
        provider_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = provider_id.as_ref().and_then(|id| {
            self.custom_providers
                .iter()
                .find(|provider| &provider.id == id)
        });
        let in_catalog = provider_id.as_ref().is_some_and(|id| {
            self.available_models
                .iter()
                .any(|model| &model.provider == id)
        });
        let id_text = provider_id.clone().unwrap_or_default();
        let name_text = existing
            .and_then(|provider| provider.name.clone())
            .unwrap_or_default();
        let base_url_text = existing
            .map(|provider| provider.base_url.clone())
            .unwrap_or_default();
        let api = existing
            .map(|provider| provider.api.clone())
            .filter(|api| !api.is_empty())
            .unwrap_or_else(|| {
                if in_catalog {
                    // Built-in: no override unless the user picks one.
                    String::new()
                } else {
                    "openai-completions".to_string()
                }
            });
        let models_text = existing
            .map(|provider| provider.model_ids.join(", "))
            .unwrap_or_default();
        let had_api_key = existing.is_some_and(|provider| provider.has_api_key);

        let id_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-editor-id")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
                .with_placeholder("my-gateway")
                .with_text(id_text)
        });
        let name_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-editor-name")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
                .with_placeholder_key("settings.optional_display_name")
                .with_text(name_text)
        });
        let base_url_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-editor-base-url")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
                .with_placeholder("https://api.example.com/v1")
                .with_text(base_url_text)
        });
        let api_key_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-editor-api-key")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
                .with_placeholder(if had_api_key {
                    "••••••••"
                } else {
                    "sk-…"
                })
        });
        let models_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("provider-editor-models")
                .with_key_context("Composer Picker")
                .with_max_lines(4)
                .with_placeholder_key("settings.model_id_model_id_2")
                .with_text(models_text)
        });

        let focus = if provider_id.is_some() {
            name_input.read(cx).focus_handle(cx)
        } else {
            id_input.read(cx).focus_handle(cx)
        };
        window.focus(&focus);
        self.provider_editor = Some(ProviderEditor {
            original_id: provider_id,
            in_catalog,
            id: id_input,
            name: name_input,
            base_url: base_url_input,
            api_key: api_key_input,
            models: models_input,
            api,
            had_api_key,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn provider_editor_cancel(
        &mut self,
        _: &crate::PickerCancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closed = self.provider_editor.take().is_some()
            | self.provider_key_editor.take().is_some()
            | self.provider_usage_open.take().is_some();
        if closed {
            cx.notify();
        }
    }

    pub(super) fn provider_editor_confirm(
        &mut self,
        _: &crate::PickerConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.provider_key_editor.is_some() {
            self.provider_key_save(window, cx);
        } else {
            self.provider_save(window, cx);
        }
    }

    /// Dismiss when the scrim (outside the card) is pressed.
    pub(super) fn provider_editor_scrim(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closed = self.provider_editor.take().is_some()
            | self.provider_key_editor.take().is_some()
            | self.provider_usage_open.take().is_some();
        if closed {
            cx.notify();
        }
    }

    /// Validate and write the editor's values to models.json.
    pub(super) fn provider_save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.provider_editor.as_ref() else {
            return;
        };
        let id = editor.id.read(cx).text().trim().to_string();
        let name = editor.name.read(cx).text().trim().to_string();
        let base_url = editor.base_url.read(cx).text().trim().to_string();
        let api_key = editor.api_key.read(cx).text();
        let api = editor.api.clone();
        let in_catalog = editor.in_catalog;
        let models: Vec<String> = editor
            .models
            .read(cx)
            .text()
            .split(['\n', ','])
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .collect();

        let error = if id.is_empty() {
            Some(tr!("settings.provider_id_required"))
        } else if !id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            Some(tr!("settings.provider_id_invalid"))
        } else if !(base_url.is_empty()
            || base_url.starts_with("http://")
            || base_url.starts_with("https://"))
        {
            Some(tr!("settings.base_url_scheme"))
        } else if base_url.is_empty() && !in_catalog {
            Some(tr!("settings.base_url_required"))
        } else if models.is_empty() && !in_catalog {
            Some(tr!("settings.add_model_id"))
        } else {
            None
        };

        if let Some(error) = error {
            if let Some(editor) = self.provider_editor.as_mut() {
                editor.error = Some(error);
            }
            cx.notify();
            return;
        }

        let key = (!api_key.trim().is_empty()).then_some(api_key.trim());
        match providers::write_provider(
            &id,
            (!name.is_empty()).then_some(name.as_str()),
            &base_url,
            &api,
            key,
            &models,
        ) {
            Ok(()) => {
                self.provider_editor = None;
                self.reload_custom_providers(cx);
                self.refresh_catalogs();
                self.toast_success(tr!("settings.provider_saved", id = id));
            }
            Err(err) => {
                if let Some(editor) = self.provider_editor.as_mut() {
                    editor.error = Some(err);
                }
            }
        }
        cx.notify();
    }

    /// Remove a provider entry from models.json.
    pub(super) fn provider_remove(&mut self, id: String, cx: &mut Context<Self>) {
        match providers::remove_provider(&id) {
            Ok(()) => {
                self.provider_remove_confirm = None;
                self.reload_custom_providers(cx);
                self.refresh_catalogs();
                self.toast_info(tr!("settings.provider_removed", id = id));
            }
            Err(err) => {
                self.custom_providers_error = Some(err);
            }
        }
        cx.notify();
    }
}

/// Wire the backdrop blur slider's pointer handling. A press inside the
/// track jumps the value to that point and begins a scrub; moves keep
/// updating even outside the track (a scrub that overshoots still clamps to
/// an end); release ends it. The handlers ride the slider canvas's prepaint
/// (see `OrbitApp::blur_slider`), so `bounds` and `hitbox` are exactly the
/// painted track's — not the window's or the settings column's.
fn register_blur_slider(
    this: Entity<OrbitApp>,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
) {
    let scrubbing = Rc::new(Cell::new(false));
    window.on_mouse_event({
        let scrubbing = scrubbing.clone();
        let this = this.clone();
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            scrubbing.set(true);
            apply_blur_at(&this, bounds, event.position.x, cx);
        }
    });
    window.on_mouse_event({
        let scrubbing = scrubbing.clone();
        let this = this.clone();
        move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase != DispatchPhase::Capture || !scrubbing.get() {
                return;
            }
            apply_blur_at(&this, bounds, event.position.x, cx);
        }
    });
    window.on_mouse_event({
        move |event: &MouseUpEvent, phase, window, _cx| {
            if phase != DispatchPhase::Capture
                || event.button != MouseButton::Left
                || !scrubbing.replace(false)
            {
                return;
            }
            window.refresh();
        }
    });
}

/// Set the backdrop blur from a pointer x in window coordinates, clamped to
/// the track and snapped to `dither::BLUR_STEP`. A no-op when the snapped
/// value is unchanged, so a scrub across one step rebuilds the dither pass
/// once rather than on every mouse move.
fn apply_blur_at(this: &Entity<OrbitApp>, bounds: Bounds<Pixels>, x: Pixels, cx: &mut App) {
    // The pointer tracks the handle's centre, which travels across the track
    // minus the handle — the same span the painted knob uses.
    let usable = (f32::from(bounds.size.width) - BLUR_KNOB).max(1.);
    let norm =
        ((f32::from(x) - f32::from(bounds.origin.x) - BLUR_KNOB / 2.) / usable).clamp(0., 1.);
    let sigma = crate::dither::blur_from_norm(norm);
    let mut tuning = crate::dither::tuning();
    if (tuning.blur - sigma).abs() < f32::EPSILON {
        return;
    }
    tuning.blur = sigma;
    if crate::dither::set_tuning(tuning) {
        // Rebuild now so this frame's preview is the newly tuned pass; the
        // cache key's blur field makes the next new-task paint hit it warm.
        crate::dither::background();
        this.update(cx, |_, cx| cx.notify());
    }
}

/// A keyboard chord painted as a keycap chip: the same 22px pill the command
/// palette uses, so a shortcut reads identically wherever Orbit shows one.
fn keycap(label: &str, theme: Theme) -> AnyElement {
    div()
        .h(ButtonSize::Default.height(&theme))
        .min_w(px(28.))
        .px(DynamicSpacing::Base06.px(&theme))
        .rounded(Radius::Large.px(&theme))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.overlay_strong)
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_muted)
        .child(label.to_string())
        .into_any_element()
}

/// Whether a catalog model survives the Models page's search, favorites, and
/// provider filters. Shared by the grid and the toolbar's live count so the
/// two can never disagree.
fn model_visible(
    model: &ModelEntry,
    needle: &str,
    favorites_only: bool,
    favorites: &crate::favorites::Favorites,
    provider: Option<&str>,
) -> bool {
    if provider.is_some_and(|provider| model.provider != provider) {
        return false;
    }
    if favorites_only && !favorites.contains(&model.provider, &model.id) {
        return false;
    }
    needle.is_empty()
        || model.name.to_lowercase().contains(needle)
        || model.id.to_lowercase().contains(needle)
        || model.provider.to_lowercase().contains(needle)
}

/// The Models toolbar's count line: the catalog total, or "shown of total"
/// while a search or the favorites filter is narrowing the grid.
fn model_count_label(total: usize, shown: usize, favorites: usize, filtered: bool) -> String {
    if total == 0 {
        return tr!("settings.no_models_reported_by_runtime");
    }
    let favorites = tr!("settings.n_favorites", count = favorites);
    if filtered {
        tr!(
            "settings.count_of_shown",
            shown = shown,
            total = total,
            favorites = favorites
        )
    } else {
        tr!(
            "settings.count_models",
            total = total,
            favorites = favorites
        )
    }
}

/// The position among `visible` that `dir` moves the settings dropdown's
/// keyboard cursor to: the current position ± 1, clamped at the ends. A
/// highlight the query filtered away lands on the first match going down,
/// the last going up. `None` when nothing is visible.
fn stepped_visible_position(
    visible: &[usize],
    highlight: Option<usize>,
    dir: isize,
) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    Some(
        match highlight.and_then(|ix| visible.iter().position(|visible| *visible == ix)) {
            Some(pos) => (pos as isize + dir).clamp(0, visible.len() as isize - 1) as usize,
            None if dir >= 0 => 0,
            None => visible.len() - 1,
        },
    )
}

#[cfg(test)]
mod settings_select_tests {
    use super::stepped_visible_position;

    /// The cursor is an original option index; `visible` holds the indices
    /// the current query left in the list, in display order.
    #[test]
    fn stepping_starts_at_the_first_and_last_match() {
        let visible = [0, 3, 7];
        assert_eq!(stepped_visible_position(&visible, None, 1), Some(0));
        assert_eq!(stepped_visible_position(&visible, None, -1), Some(2));
    }

    #[test]
    fn stepping_clamps_at_the_ends() {
        let visible = [0, 3, 7];
        assert_eq!(stepped_visible_position(&visible, Some(0), -1), Some(0));
        assert_eq!(stepped_visible_position(&visible, Some(3), 1), Some(2));
        assert_eq!(stepped_visible_position(&visible, Some(7), 1), Some(2));
    }

    #[test]
    fn stepping_recovers_when_the_query_filtered_the_cursor_away() {
        let visible = [2, 5];
        assert_eq!(stepped_visible_position(&visible, Some(4), 1), Some(0));
        assert_eq!(stepped_visible_position(&visible, Some(4), -1), Some(1));
    }

    #[test]
    fn stepping_an_empty_list_stays_put() {
        assert_eq!(stepped_visible_position(&[], Some(0), 1), None);
        assert_eq!(stepped_visible_position(&[], None, -1), None);
    }
}

/// The Theme dropdown's option list is the single ordering both the label
/// builder and the click resolver read, so a row index always resolves to the
/// palette it painted. Preferences are set in memory here; the real setter
/// writes the user's `~/.orbit-pi/theme` and has no business running from a
/// test.
#[cfg(test)]
mod theme_catalog_tests {
    use super::*;

    fn test_app(cx: &mut gpui::TestAppContext) -> Entity<OrbitApp> {
        cx.update(|cx| {
            cx.set_global(Theme::for_id(theme::ThemeId::Orbit));
            cx.set_global(theme::AppearancePrefs::default());
            cx.new(OrbitApp::new)
        })
    }

    fn choose_dark_theme(cx: &mut gpui::TestAppContext, id: theme::ThemeId) {
        cx.update(|cx| {
            let mut prefs = theme::appearance_prefs(cx);
            prefs.set_theme(ThemeMode::Dark, id);
            cx.set_global(prefs);
        });
    }

    #[gpui::test]
    fn catalog_lists_every_dark_palette(cx: &mut gpui::TestAppContext) {
        let app = test_app(cx);
        let cx = cx.add_empty_window();
        choose_dark_theme(cx, theme::ThemeId::Orbit);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.theme_catalog(ThemeMode::Dark, cx),
                    vec![
                        theme::ThemeId::Orbit,
                        theme::ThemeId::OrbitDarker,
                        theme::ThemeId::OrbitCobalt,
                        theme::ThemeId::OrbitAzure,
                        theme::ThemeId::Flexoki,
                    ]
                );
            });
        });
    }

    #[gpui::test]
    fn catalog_lists_every_light_palette(cx: &mut gpui::TestAppContext) {
        let app = test_app(cx);
        let cx = cx.add_empty_window();
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.theme_catalog(ThemeMode::Light, cx),
                    vec![
                        theme::ThemeId::OrbitLight,
                        theme::ThemeId::OrbitAzureLight,
                        theme::ThemeId::OrbitPaper,
                        theme::ThemeId::OrbitContrast,
                        theme::ThemeId::FlexokiLight,
                    ]
                );
            });
        });
    }
}

#[cfg(test)]
mod model_filter_tests {
    use super::{model_count_label, model_visible, ModelEntry};
    use crate::favorites::Favorites;

    fn entry(provider: &str, id: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context_window: None,
            thinking_levels: Vec::new(),
        }
    }

    #[test]
    fn search_matches_name_id_and_provider_case_insensitively() {
        // Callers hand the helper an already-lowercased needle.
        let favorites = Favorites::from_pairs(&[]);
        let model = entry("anthropic", "claude-sonnet-4-5", "Claude Sonnet 4.5");
        assert!(model_visible(&model, "", false, &favorites, None));
        assert!(model_visible(&model, "sonnet", false, &favorites, None));
        assert!(model_visible(
            &model,
            "claude-sonnet",
            false,
            &favorites,
            None
        ));
        assert!(model_visible(&model, "anthropic", false, &favorites, None));
        assert!(!model_visible(&model, "gemini", false, &favorites, None));
    }

    #[test]
    fn favorites_only_keeps_starred_models_and_still_searches() {
        let favorites = Favorites::from_pairs(&[("openai", "gpt-5")]);
        let starred = entry("openai", "gpt-5", "GPT-5");
        let other = entry("openai", "gpt-5-mini", "GPT-5 mini");
        assert!(model_visible(&starred, "", true, &favorites, None));
        assert!(!model_visible(&other, "", true, &favorites, None));
        // The search still applies inside the favorites scope.
        assert!(!model_visible(&starred, "mini", true, &favorites, None));
    }

    #[test]
    fn provider_filter_scopes_the_grid_and_composes_with_search() {
        let favorites = Favorites::from_pairs(&[]);
        let opus = entry("anthropic", "claude-opus-4", "Claude Opus 4");
        let sonnet = entry("anthropic", "claude-sonnet-4-5", "Claude Sonnet 4.5");
        let gpt = entry("openai", "gpt-5", "GPT-5");

        // A provider scope admits only that provider's models.
        assert!(model_visible(
            &opus,
            "",
            false,
            &favorites,
            Some("anthropic")
        ));
        assert!(model_visible(
            &sonnet,
            "",
            false,
            &favorites,
            Some("anthropic")
        ));
        assert!(!model_visible(
            &gpt,
            "",
            false,
            &favorites,
            Some("anthropic")
        ));
        // The search still applies inside the provider scope.
        assert!(!model_visible(
            &sonnet,
            "opus",
            false,
            &favorites,
            Some("anthropic")
        ));
        // `None` means every provider.
        assert!(model_visible(&gpt, "", false, &favorites, None));
    }

    #[test]
    fn count_label_reflects_the_active_filters() {
        assert_eq!(
            model_count_label(0, 0, 0, false),
            "No models reported by the runtime"
        );
        assert_eq!(
            model_count_label(236, 236, 12, false),
            "236 models · 12 favorite(s)"
        );
        assert_eq!(
            model_count_label(236, 1, 12, true),
            "1 of 236 shown · 12 favorite(s)"
        );
        assert_eq!(
            model_count_label(3, 3, 1, false),
            "3 models · 1 favorite(s)"
        );
    }
}

#[cfg(test)]
mod about_banner_tests {
    use super::*;

    /// Settings → About mounts the star banner permanently: it is the same
    /// card as the sidebar's, but carries no dismiss button. (The sidebar's
    /// dismiss id is absent from the About tree.)
    #[gpui::test]
    fn about_page_mounts_the_star_banner_without_a_dismiss(cx: &mut gpui::TestAppContext) {
        use crate::theme::{Theme, ThemeId};

        cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
        let cx = cx.add_empty_window();
        let app = cx.update(|_, cx| cx.new(OrbitApp::new));
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.open_settings(cx);
                app.set_settings_section(SettingsSection::About, cx);
            })
        });
        let viewport = cx.update(|window, _| window.viewport_size());
        let _ = cx.draw(point(px(0.), px(0.)), viewport, |_, _| app.clone());

        let banner = cx
            .debug_bounds("about-star-banner")
            .expect("Settings → About must show the star banner");
        assert!(
            f32::from(banner.size.width) > 300.,
            "the About banner spans the content column: {banner:?}"
        );
        assert!(
            cx.debug_bounds("sidebar-star-dismiss").is_none(),
            "the About banner is permanent — it carries no dismiss button"
        );
    }
}

#[cfg(test)]
mod settings_search_tests {
    use super::{settings_search_hits, SettingsSection};

    #[test]
    fn empty_query_matches_nothing() {
        assert!(settings_search_hits("").is_empty());
    }

    #[test]
    fn label_and_keyword_matches_span_sections() {
        // A translated label match (Auto-compaction contains "compaction").
        assert!(settings_search_hits("compaction")
            .iter()
            .any(|(section, key, _)| *section == SettingsSection::Agent
                && *key == "settings.auto_compaction"));
        // A keyword synonym the label does not spell out.
        assert!(settings_search_hits("banner")
            .iter()
            .any(|(section, _, _)| *section == SettingsSection::General));
        // A section-level keyword resolves to its page.
        assert!(settings_search_hits("protocol")
            .iter()
            .any(|(section, _, _)| *section == SettingsSection::Mcp));
    }

    #[test]
    fn no_match_is_empty() {
        assert!(settings_search_hits("zzzzzz").is_empty());
    }
}
