//! Command palette — the window-wide ⌘P surface for jumping to a session,
//! running an app command, or opening a settings section from one field.
//!
//! Direction contract (impeccable):
//! THESIS: one keystroke puts every place and action a query away — sessions
//! first, then commands, then settings — in a single top-centered card. It
//! refuses the split between the sidebar's session switcher, the composer's
//! chip popovers, and the settings gear: one filter ranks all of them.
//! OWN-WORLD: Orbit's raised-surface language (`surface_elevated`, 1px `border_strong`,
//! layered contact+ambient shadow, `overlay_strong` highlight, accent for the
//! live session) lifted onto a dimmed modal layer — the same atoms as the
//! sidebar palette and chip popovers, at window scale.
//! STORY: the operator hits ⌘P, types two or three characters, and lands on a
//! session, a panel toggle, or a settings section; Enter executes and the
//! composer regains focus.
//! FIRST VIEWPORT: a Zed picker-width card floating near the top over a black
//! scrim; a search row with a search glyph; sectioned rows (Sessions / Commands /
//! Settings) with icon, label, inline detail, and a shortcut chip; a quiet
//! footer naming the keys.
//! FORM: a command-palette anatomy (sections, fuzzy scoring, wrap-around
//! navigation, scrim layer, in-memory results) adapted to Orbit's entity +
//! callback conventions. Opened with ⌘P or the sidebar Search row.
//!
//! Key handling reuses the existing `Picker` bindings (enter/escape/↑/↓),
//! which are registered after the composer's and therefore win at the same
//! dispatch depth; the filter input carries the `Composer Picker` context so
//! backspace/paste keep editing.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use gpui::{
    deferred, div, point, prelude::*, px, App, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, ScrollHandle,
    SharedString, Styled, Subscription, Window,
};

use crate::app::{
    icon, menu_header, picker_entry, picker_search_frame, picker_surface, SettingsSection,
};
use crate::commands::{self, CommandId, CommandSpec};
use crate::composer::ComposerInput;
use crate::platform::shortcuts;
use crate::sessions::SessionInfo;
use crate::theme::tokens::{
    context_menu, input, list, list_item, picker, BufferLineHeight, ButtonSize, DynamicSpacing,
    Radius, TextSize,
};
use crate::theme::{self, Theme};

/// The palette's layout metrics, resolved from the theme so the card's
/// height caps and the keyboard reveal math measure exactly what renders.
/// The card itself is Zed's picker width (`picker::default_width`).
#[derive(Debug, Clone, Copy)]
struct PaletteMetrics {
    /// Search field row (Zed's picker head).
    search_h: f32,
    /// Uniform row height (icon + label + inline detail + shortcut chip):
    /// a one-line picker entry.
    row_h: f32,
    /// Section header ("Sessions" / "Commands" / "Settings"): a list
    /// sub-header plus the Base12 above it that separates sections.
    header_h: f32,
    /// The list's padding above the first and below the last row.
    list_pad_y: f32,
    /// Largest results height before the list scrolls (≈ 9 rows + headers).
    list_max_h: f32,
    /// Footer hint bar: one line of Small text, Base06 above and below.
    footer_h: f32,
}

impl PaletteMetrics {
    fn new(theme: &Theme) -> Self {
        Self {
            search_h: picker::search_height(theme).into(),
            row_h: picker::entry_height(theme).into(),
            header_h: (DynamicSpacing::Base12.px(theme)
                + list::sub_header_height(theme)
                + list::sub_header_padding_bottom(theme))
            .into(),
            list_pad_y: picker::list_padding_y(theme).into(),
            list_max_h: picker::default_max_height(theme).into(),
            footer_h: (BufferLineHeight::Comfortable.resolve(TextSize::Small.px(theme))
                + DynamicSpacing::Base06.px(theme) * 2.)
                .into(),
        }
    }

    /// Tallest the whole card may grow; the list scrolls past this.
    fn card_max_h(&self) -> f32 {
        self.search_h + self.list_max_h + self.footer_h
    }
}

/// Sessions listed when the query is empty (recent activity, newest first).
const EMPTY_SESSION_ROWS: usize = 6;
/// Sessions listed for a non-empty query.
const MAX_SESSION_RESULTS: usize = 12;

/// A palette row's action: run a registry command, open a Settings
/// destination (which has no keymap row and exists only in the palette), or
/// act on one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteCommand {
    Run(CommandId),
    OpenSettings(SettingsSection),
    Mcp {
        name: String,
        action: McpPaletteAction,
    },
}

/// A per-server MCP action offered by the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPaletteAction {
    Toggle,
    Reconnect,
    Test,
}

/// One configured server as the palette sees it: enough to label a row, no
/// live state (the row's command re-reads the manager).
#[derive(Debug, Clone)]
pub struct PaletteMcpServer {
    pub name: String,
    pub enabled: bool,
}

/// Everything the palette needs to build its items, captured by the app at
/// open time. The palette never reaches back into app state — render reads
/// only this in-memory snapshot (render-purity rule).
pub struct PaletteSnapshot {
    pub sessions: Vec<SessionInfo>,
    /// The open session's path, so its row can be emphasized.
    pub active_path: Option<PathBuf>,
    /// The open session is pinned (drives the Pin/Unpin palette label).
    pub active_pinned: bool,
    /// An agent run is in flight (gates the Abort command).
    pub busy: bool,
    /// pi's session id for the active session (gates Copy Session ID).
    pub session_id: Option<String>,
    pub sidebar_visible: bool,
    pub side_panel_visible: bool,
    pub terminal_visible: bool,
    /// How many shells the terminal panel is holding: new/close/cycle are
    /// meaningless without one.
    pub terminal_tabs: usize,
    pub project_panel_visible: bool,
    pub can_choose_model: bool,
    pub can_choose_thinking: bool,
    /// Configured MCP servers, for the per-server rows.
    pub mcp_servers: Vec<PaletteMcpServer>,
}

/// Result sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    Sessions,
    Commands,
    Settings,
}

impl Section {
    fn label(self) -> String {
        match self {
            Self::Sessions => tr!("command_palette.sessions"),
            Self::Commands => tr!("command_palette.commands"),
            Self::Settings => tr!("command_palette.settings"),
        }
    }
}

/// What activating a row does.
#[derive(Debug, Clone)]
enum PaletteAction {
    OpenSession(SessionInfo),
    Run(PaletteCommand),
    /// Act on one configured MCP server (name carried per row).
    Mcp {
        name: String,
        action: McpPaletteAction,
    },
}

/// One row in the results list.
#[derive(Debug, Clone)]
struct PaletteItem {
    section: Section,
    icon: &'static str,
    label: String,
    detail: Option<String>,
    /// A display chip derived from the command's advertised chord.
    shortcut: Option<String>,
    action: PaletteAction,
    /// Lowercased haystack for fuzzy matching (label + keywords + detail).
    search: String,
    /// Declaration order — the stable tie-break for commands/settings.
    order: usize,
    /// Last-activity seconds (sessions only; 0 for commands).
    recency: u64,
}

impl PaletteItem {
    /// A registry command row: every visible field comes from
    /// [`crate::commands`], never from a hand-typed duplicate.
    fn from_spec(spec: &'static CommandSpec, snapshot: &PaletteSnapshot, order: usize) -> Self {
        let label = command_title(spec, snapshot);
        Self {
            section: Section::Commands,
            icon: spec.icon,
            label: label.clone(),
            detail: None,
            shortcut: spec.shortcut(),
            action: PaletteAction::Run(PaletteCommand::Run(spec.id)),
            search: format!("{} {}", label.to_lowercase(), spec.keywords),
            order,
            recency: 0,
        }
    }

    /// A Settings destination row (palette-only; no keymap entry).
    fn settings(
        section: SettingsSection,
        icon: &'static str,
        label: String,
        keywords: &str,
        order: usize,
    ) -> Self {
        Self {
            section: Section::Settings,
            icon,
            search: format!("{} {keywords}", label.to_lowercase()),
            label,
            detail: Some(tr!("command_palette.settings")),
            shortcut: (section == SettingsSection::General)
                .then(|| shortcuts::label(crate::platform::shortcuts::SETTINGS)),
            action: PaletteAction::Run(PaletteCommand::OpenSettings(section)),
            order,
            recency: 0,
        }
    }
}

/// The palette title for a command: the alternate key wins when the toggle
/// it describes is on (a visible sidebar reads "Hide Sidebar").
fn command_title(spec: &CommandSpec, snapshot: &PaletteSnapshot) -> String {
    let alternate = match spec.id {
        CommandId::ToggleSidebar => snapshot.sidebar_visible,
        CommandId::ToggleSidePanel => snapshot.side_panel_visible,
        CommandId::ToggleTerminal => snapshot.terminal_visible,
        CommandId::ToggleProjectPanel => snapshot.project_panel_visible,
        CommandId::PinSession => snapshot.active_pinned,
        _ => false,
    };
    commands::title(spec.id, alternate)
}

/// Whether the snapshot gives a command enough state to be worth listing.
fn command_enabled(id: CommandId, snapshot: &PaletteSnapshot) -> bool {
    match id {
        CommandId::ChooseModel => snapshot.can_choose_model,
        CommandId::ChooseThinking => snapshot.can_choose_thinking,
        CommandId::AbortRun => snapshot.busy,
        CommandId::RenameSession
        | CommandId::PinSession
        | CommandId::CloneSession
        | CommandId::DeleteSession => snapshot.active_path.is_some(),
        CommandId::CopySessionId => snapshot.session_id.is_some(),
        // Cycling needs somewhere to go.
        CommandId::NextSession | CommandId::PrevSession => snapshot.sessions.len() > 1,
        // Terminal tabs: closing needs a tab to close, cycling needs two.
        CommandId::CloseTerminalTab => snapshot.terminal_tabs > 0,
        CommandId::NextTerminalTab | CommandId::PrevTerminalTab => snapshot.terminal_tabs > 1,
        _ => true,
    }
}

/// Subsequence fuzzy score, Zed-style: every query character must appear in
/// order; word-boundary and consecutive hits score higher. Returns `None`
/// when the query is not a subsequence of the candidate.
fn fuzzy_score(query: &str, candidate: &str) -> Option<u32> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let candidate: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut score = 0u32;
    let mut qi = 0;
    let mut prev_hit = false;
    for (i, &c) in candidate.iter().enumerate() {
        if qi < query.len() && c == query[qi] {
            let boundary = i == 0 || !candidate[i - 1].is_alphanumeric();
            score += 1 + u32::from(boundary) * 6 + u32::from(prev_hit) * 3;
            qi += 1;
            prev_hit = true;
        } else {
            prev_hit = false;
        }
    }
    (qi == query.len()).then_some(score)
}

/// Open callback: the chosen session plus the ambient window.
type OpenSession = Box<dyn Fn(SessionInfo, &mut Window, &mut App)>;
/// Command callback: the chosen palette command plus the ambient window.
type RunCommand = Box<dyn Fn(PaletteCommand, &mut Window, &mut App)>;
/// Dismiss callback; `bool` is true when an outside mouse-down closed it.
type PaletteDismiss = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// The palette entity. Created by `OrbitApp` on ⌘P; talks back exclusively
/// through the callbacks it was built with.
pub struct CommandPalette {
    snapshot: PaletteSnapshot,
    filter: Entity<ComposerInput>,
    scroll: ScrollHandle,
    highlighted: usize,
    last_filter: String,
    _filter_sub: Subscription,
    on_open: OpenSession,
    on_command: RunCommand,
    /// `bool` = dismissed by an outside mouse-down (vs. escape).
    on_dismiss: PaletteDismiss,
}

impl CommandPalette {
    pub fn new(
        snapshot: PaletteSnapshot,
        on_open: OpenSession,
        on_command: RunCommand,
        on_dismiss: PaletteDismiss,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("command_palette.search_sessions_commands_settings")
                .with_key_context("Composer Picker")
        });
        // Keep the derived filter state in sync from the input's notifications
        // so `render` only reads it.
        let filter_sub = cx.observe(&filter, |palette: &mut Self, filter, cx| {
            let needle = filter.read(cx).text().to_lowercase();
            if needle != palette.last_filter {
                palette.last_filter = needle;
                palette.highlighted = 0;
                palette.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        });
        Self {
            snapshot,
            filter,
            scroll: ScrollHandle::new(),
            highlighted: 0,
            last_filter: String::new(),
            _filter_sub: filter_sub,
            on_open,
            on_command,
            on_dismiss,
        }
    }

    // ── actions (Picker key context) ─────────────────────────────────────

    fn on_cancel(&mut self, _: &crate::PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_dismiss)(false, window, cx);
    }

    fn on_confirm(
        &mut self,
        _: &crate::PickerConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.results(&self.last_filter);
        self.activate(self.highlighted, &rows, window, cx);
    }

    fn on_next(
        &mut self,
        _: &crate::PickerSelectNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step(1, f32::from(window.viewport_size().height), cx);
    }

    fn on_prev(
        &mut self,
        _: &crate::PickerSelectPrev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step(-1, f32::from(window.viewport_size().height), cx);
    }

    fn on_scrim_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_dismiss)(true, window, cx);
    }

    // ── internals ────────────────────────────────────────────────────────

    /// Session rows (newest activity first — `load_sessions` pre-sorts).
    fn session_items(&self) -> Vec<PaletteItem> {
        self.snapshot
            .sessions
            .iter()
            .enumerate()
            .map(|(order, session)| {
                let workspace = crate::sessions::workspace_label(&session.cwd);
                let active = self.snapshot.active_path.as_ref() == Some(&session.path);
                let mut detail = format!(
                    "{workspace} · {}",
                    crate::sessions::relative_time(session.modified)
                );
                if active {
                    detail.push_str(&tr!("command_palette.current"));
                }
                PaletteItem {
                    section: Section::Sessions,
                    icon: "icons/chat.svg",
                    label: session.title.clone(),
                    detail: Some(detail),
                    shortcut: None,
                    action: PaletteAction::OpenSession(session.clone()),
                    search: format!(
                        "{} {} {} session chat conversation",
                        session.title.to_lowercase(),
                        workspace.to_lowercase(),
                        session.cwd.to_string_lossy().to_lowercase(),
                    ),
                    order,
                    recency: session
                        .modified
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                }
            })
            .collect()
    }

    /// Command + settings rows, gated by the snapshot's live facts so the
    /// palette never offers an action the app can't perform right now.
    /// Rows for the registry commands the snapshot enables, then the
    /// Settings destinations. Labels, icons, keywords, and shortcut chips
    /// all come from the registry (`crate::commands`), so this list cannot
    /// drift from the keymap or the Shortcuts page.
    fn command_items(&self) -> Vec<PaletteItem> {
        let mut items: Vec<PaletteItem> = Vec::new();
        for spec in commands::COMMANDS.iter().filter(|spec| spec.palette) {
            if !command_enabled(spec.id, &self.snapshot) {
                continue;
            }
            items.push(PaletteItem::from_spec(spec, &self.snapshot, items.len()));
        }
        // Per-server MCP rows: enable/disable and reconnect, labelled with the
        // server they act on. The page remains the place for details.
        for server in &self.snapshot.mcp_servers {
            for action in [
                McpPaletteAction::Toggle,
                McpPaletteAction::Reconnect,
                McpPaletteAction::Test,
            ] {
                let order = items.len();
                let (label_key, keywords) = match action {
                    McpPaletteAction::Toggle if server.enabled => {
                        ("mcp.palette_disable", "mcp disable server")
                    }
                    McpPaletteAction::Toggle => ("mcp.palette_enable", "mcp enable server"),
                    McpPaletteAction::Reconnect => {
                        ("mcp.palette_reconnect", "mcp reconnect restart server")
                    }
                    McpPaletteAction::Test => ("mcp.palette_test", "mcp test connection server"),
                };
                let label = tr!(label_key, name = server.name.clone());
                items.push(PaletteItem {
                    section: Section::Commands,
                    icon: "icons/tools/mcp.svg",
                    search: format!("{} {keywords} mcp", label.to_lowercase()),
                    label,
                    detail: Some(tr!("mcp.palette_group")),
                    shortcut: None,
                    action: PaletteAction::Mcp {
                        name: server.name.clone(),
                        action,
                    },
                    order,
                    recency: 0,
                });
            }
        }
        for (section, icon, label, keywords) in [
            (
                SettingsSection::General,
                "icons/settings.svg",
                tr!("settings.general"),
                "settings preferences general language font",
            ),
            (
                SettingsSection::Appearance,
                "icons/contrast.svg",
                tr!("settings.appearance"),
                "settings preferences appearance theme light dark",
            ),
            (
                SettingsSection::Shortcuts,
                "icons/keyboard.svg",
                tr!("settings.shortcuts"),
                "settings keyboard shortcuts keys reference",
            ),
            (
                SettingsSection::Runtime,
                "icons/server-stack.svg",
                tr!("settings.runtime"),
                "settings preferences runtime process pi start stop restart",
            ),
            (
                SettingsSection::Agent,
                "icons/spark.svg",
                tr!("settings.agent"),
                "settings preferences agent steer follow-up compaction retry",
            ),
            (
                SettingsSection::Providers,
                "icons/cloud.svg",
                tr!("settings.providers"),
                "settings preferences providers models api",
            ),
            (
                SettingsSection::Models,
                "icons/tag-01.svg",
                tr!("settings.models"),
                "settings preferences models catalog favorites providers",
            ),
            (
                SettingsSection::Skills,
                "icons/magic-wand.svg",
                tr!("settings.skills"),
                "settings preferences skills skill.md instructions agent",
            ),
            (
                SettingsSection::Plugins,
                "icons/extensions.svg",
                tr!("settings.plugins"),
                "settings preferences plugins extensions packages install npm git",
            ),
            (
                SettingsSection::Mcp,
                "icons/tools/mcp.svg",
                tr!("settings.mcp"),
                "settings preferences mcp model context protocol servers tools",
            ),
            (
                SettingsSection::About,
                "icons/info.svg",
                tr!("settings.about"),
                "settings about version app",
            ),
            (
                SettingsSection::ReportBug,
                "icons/github.svg",
                tr!("settings.report_bug"),
                "bug report issue crash problem github feedback",
            ),
        ] {
            let order = items.len();
            items.push(PaletteItem::settings(section, icon, label, keywords, order));
        }
        items
    }

    /// Rows for the current query: an empty query lists recent sessions plus
    /// every command; a query fuzzy-filters and ranks each section.
    fn results(&self, query: &str) -> Vec<PaletteItem> {
        let query = query.trim();
        if query.is_empty() {
            let mut rows: Vec<PaletteItem> = self
                .session_items()
                .into_iter()
                .take(EMPTY_SESSION_ROWS)
                .collect();
            rows.extend(self.command_items());
            return rows;
        }
        let rank = |items: Vec<PaletteItem>, cap: usize| -> Vec<PaletteItem> {
            let mut scored: Vec<(u32, PaletteItem)> = items
                .into_iter()
                .filter_map(|item| fuzzy_score(query, &item.search).map(|s| (s, item)))
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then(b.1.recency.cmp(&a.1.recency))
                    .then(a.1.order.cmp(&b.1.order))
            });
            scored.truncate(cap);
            scored.into_iter().map(|(_, item)| item).collect()
        };
        let mut rows = rank(self.session_items(), MAX_SESSION_RESULTS);
        rows.extend(rank(self.command_items(), usize::MAX));
        rows
    }

    /// Wrap-around ↑/↓ navigation, keeping the highlighted row
    /// fully visible in the scroll window.
    fn step(&mut self, dir: isize, window_height: f32, cx: &mut Context<Self>) {
        let rows = self.results(&self.last_filter);
        if rows.is_empty() {
            return;
        }
        let pos = self.highlighted.min(rows.len() - 1) as isize;
        let next = (pos + dir).rem_euclid(rows.len() as isize) as usize;
        self.highlighted = next;
        let theme = theme::get(cx);
        let metrics = PaletteMetrics::new(theme);
        let viewport_h = Self::list_height(&rows, &metrics, theme, window_height);
        self.ensure_visible(&rows, &metrics, viewport_h);
        cx.notify();
    }

    /// Pixel offset of a row's top edge, accounting for the list's top
    /// padding and the section headers.
    fn row_top(rows: &[PaletteItem], ix: usize, metrics: &PaletteMetrics) -> f32 {
        let mut y = metrics.list_pad_y;
        let mut prev = None;
        for (i, item) in rows.iter().enumerate() {
            if prev != Some(item.section) {
                y += metrics.header_h;
                prev = Some(item.section);
            }
            if i == ix {
                break;
            }
            y += metrics.row_h;
        }
        y
    }

    /// Height the results list gets for this frame: its content height capped
    /// by the card space the current window leaves.
    fn list_height(
        rows: &[PaletteItem],
        metrics: &PaletteMetrics,
        theme: &Theme,
        viewport_h: f32,
    ) -> f32 {
        let top = (viewport_h * 0.09).clamp(48., 72.);
        let card_cap =
            (viewport_h - top - 36.).max(metrics.search_h + metrics.row_h + metrics.footer_h);
        let content_h = if rows.is_empty() {
            // The no-match state: one two-line entry inside the list padding.
            f32::from(picker::two_line_entry_height(theme)) + 2. * metrics.list_pad_y
        } else {
            Self::content_height(rows, metrics)
        };
        let list_cap = metrics.card_max_h().min(card_cap) - metrics.search_h - metrics.footer_h;
        content_h.min(list_cap)
    }

    /// Total content height of the results list (rows + headers + the
    /// list's vertical padding).
    fn content_height(rows: &[PaletteItem], metrics: &PaletteMetrics) -> f32 {
        let headers = {
            let mut count = 0;
            let mut prev = None;
            for item in rows {
                if prev != Some(item.section) {
                    count += 1;
                    prev = Some(item.section);
                }
            }
            count
        };
        rows.len() as f32 * metrics.row_h
            + headers as f32 * metrics.header_h
            + 2. * metrics.list_pad_y
    }

    fn ensure_visible(&mut self, rows: &[PaletteItem], metrics: &PaletteMetrics, viewport: f32) {
        let content_h = Self::content_height(rows, metrics);
        let viewport_h = content_h.min(if viewport > 0. {
            viewport
        } else {
            metrics.list_max_h
        });
        let row_top = Self::row_top(rows, self.highlighted, metrics);
        // `ScrollHandle` keeps the offset as the first child's top relative to
        // the container, which is **negative** once scrolled down; recover the
        // positive distance-from-top before comparing, and negate on the way
        // back out.
        let current = -f32::from(self.scroll.offset().y);
        let mut offset = current;
        if row_top < current {
            offset = row_top;
        } else if row_top + metrics.row_h > current + viewport_h {
            offset = row_top + metrics.row_h - viewport_h;
        }
        let max_offset = (content_h - viewport_h).max(0.);
        self.scroll
            .set_offset(point(px(0.), px(-offset.clamp(0., max_offset))));
    }

    fn activate(
        &mut self,
        ix: usize,
        rows: &[PaletteItem],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match rows.get(ix) {
            Some(PaletteItem {
                action: PaletteAction::OpenSession(session),
                ..
            }) => (self.on_open)(session.clone(), window, cx),
            Some(PaletteItem {
                action: PaletteAction::Run(command),
                ..
            }) => (self.on_command)(command.clone(), window, cx),
            Some(PaletteItem {
                action: PaletteAction::Mcp { name, action },
                ..
            }) => (self.on_command)(
                PaletteCommand::Mcp {
                    name: name.clone(),
                    action: *action,
                },
                window,
                cx,
            ),
            None => {}
        }
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.results(&self.last_filter);

        let this = cx.entity();
        let theme = *theme::get(cx);
        let metrics = PaletteMetrics::new(&theme);

        // Hug the content, capped so the card never owns the window.
        let viewport_h = f32::from(window.viewport_size().height);
        let top = (viewport_h * 0.09).clamp(48., 72.);
        let list_h = Self::list_height(&rows, &metrics, &theme, viewport_h);

        // ── results list ──
        let mut list = div()
            .id("command-palette-list")
            .w_full()
            .h(px(list_h))
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col();
        if rows.is_empty() {
            // Zed's no-match state: one muted picker entry.
            list =
                list.child(
                    picker_entry(div(), &theme)
                        .h(picker::two_line_entry_height(&theme))
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child(icon(
                            "icons/search.svg",
                            context_menu::ICON.px(&theme),
                            theme.text_muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(tr!("command_palette.no_results")),
                                )
                                .child(div().text_size(picker::SECONDARY_TEXT.px(&theme)).child(
                                    tr!(
                                        "command_palette.try_a_session_title_a_command_or_a_setting"
                                    ),
                                )),
                        ),
                );
        } else {
            let mut prev_section = None;
            for ix in 0..rows.len() {
                if prev_section != Some(rows[ix].section) {
                    // Base12 above each section; `PaletteMetrics::header_h`
                    // counts it.
                    list = list.child(
                        menu_header(rows[ix].section.label(), &theme)
                            .pt(DynamicSpacing::Base12.px(&theme)),
                    );
                    prev_section = Some(rows[ix].section);
                }
                let is_current = match &rows[ix].action {
                    PaletteAction::OpenSession(session) => {
                        self.snapshot.active_path.as_ref() == Some(&session.path)
                    }
                    PaletteAction::Run(_) | PaletteAction::Mcp { .. } => false,
                };
                list = list.child(render_row(
                    &rows,
                    ix,
                    ix == self.highlighted,
                    is_current,
                    &this,
                    theme,
                ));
            }
        }

        // ── card ──
        let card = picker_surface(div(), &theme)
            .w_full()
            .max_w(picker::default_width(&theme))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            // Clicks inside the card must not reach the scrim's dismiss.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            // search row
            .child(
                picker_search_frame(div(), &theme)
                    .text_color(theme.text_primary)
                    .child(icon(
                        "icons/search.svg",
                        input::ICON.px(&theme),
                        theme.text_muted,
                    ))
                    .child(div().flex_1().min_w_0().child(self.filter.clone())),
            )
            .child(list)
            // footer — the palette is new chrome; name the keys once, quietly.
            .child(
                div()
                    .h(px(metrics.footer_h))
                    .px(picker::search_padding_x(&theme))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base16.px(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!("command_palette.navigate"))
                    .child(tr!("command_palette.select"))
                    .child(tr!("command_palette.esc_close")),
            );

        // ── scrim layer ──
        let scrim = theme.scrim();
        div()
            .id("command-palette-layer")
            .absolute()
            .inset_0()
            .occlude()
            .bg(scrim)
            .px(DynamicSpacing::Base24.px(&theme))
            .pt(px(top))
            .flex()
            .items_start()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_scrim_down))
            .child(card)
    }
}

fn render_row(
    rows: &[PaletteItem],
    ix: usize,
    highlighted: bool,
    is_current_session: bool,
    this: &Entity<CommandPalette>,
    theme: Theme,
) -> impl IntoElement + use<> {
    let item = rows[ix].clone();
    let this = this.clone();
    let row = picker_entry(
        div().id(ElementId::NamedInteger(
            "command-palette-row".into(),
            ix as u64,
        )),
        &theme,
    );
    row.h(picker::entry_height(&theme))
        .border_1()
        .border_color(if highlighted {
            theme.border_strong
        } else {
            gpui::transparent_black()
        })
        .flex_none()
        .cursor_pointer()
        // Moving the pointer over a row moves the keyboard highlight; click
        // activates it. `on_mouse_move` (not `on_hover`) so a scroll that
        // slides rows under a stationary pointer can't hijack ↑/↓ navigation.
        .on_mouse_move({
            let this = this.clone();
            move |_, _, cx| {
                this.update(cx, |palette, cx| {
                    if palette.highlighted != ix {
                        palette.highlighted = ix;
                        cx.notify();
                    }
                });
            }
        })
        .on_click({
            let this = this.clone();
            move |_, window, cx| {
                this.update(cx, |palette, cx| {
                    let rows = palette.results(&palette.last_filter);
                    palette.activate(ix, &rows, window, cx);
                });
            }
        })
        .when(highlighted, |row| row.bg(theme.selection))
        .child(icon(
            item.icon,
            context_menu::ICON.px(&theme),
            if is_current_session {
                theme.accent_text
            } else {
                theme.text_secondary
            },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(if highlighted {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if highlighted {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        })
                        .child(item.label.clone()),
                )
                .when_some(item.detail.clone(), |row, detail| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(picker::SECONDARY_TEXT.px(&theme))
                            .text_color(if highlighted {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            })
                            .child(detail),
                    )
                }),
        )
        .when_some(item.shortcut, |row, shortcut| {
            row.child(
                div()
                    .ml(context_menu::keybinding_gap(&theme) - list_item::content_gap(&theme))
                    .h(ButtonSize::Default.height(&theme))
                    .min_w(px(28.))
                    .px(DynamicSpacing::Base08.px(&theme))
                    .rounded(Radius::Large.px(&theme))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.overlay_strong)
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(shortcut)),
            )
        })
}

/// The palette paints above every other floating surface (chip popovers,
/// menus) — `OrbitApp` wraps the entity in this deferred layer.
pub fn layer(palette: Entity<CommandPalette>) -> impl IntoElement {
    deferred(palette).with_priority(8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn fuzzy_subsequence_and_boundaries() {
        assert!(fuzzy_score("ns", "new session").is_some());
        assert!(fuzzy_score("sett", "Settings General").is_some());
        assert!(fuzzy_score("absent", "New Session").is_none());
        assert!(fuzzy_score("", "anything").is_some());
        // Word-boundary hits outrank mid-word hits.
        let boundary = fuzzy_score("side", "Hide Sidebar").unwrap();
        let midword = fuzzy_score("side", "considerations").unwrap();
        assert!(boundary > midword);
        // Consecutive runs beat scattered hits.
        let run = fuzzy_score("model", "Choose Model").unwrap();
        let scattered = fuzzy_score("model", "mode toggle panel").unwrap_or(0);
        assert!(run > scattered);
    }

    #[test]
    fn sections_order_sessions_first() {
        assert!(Section::Sessions < Section::Commands);
        assert!(Section::Commands < Section::Settings);
    }

    #[test]
    fn row_top_accounts_for_headers() {
        let item = |section: Section, order: usize| PaletteItem {
            section,
            icon: "icons/chat.svg",
            label: format!("Item {order}"),
            detail: None,
            shortcut: None,
            action: PaletteAction::Run(PaletteCommand::Run(crate::commands::CommandId::NewSession)),
            search: String::new(),
            order,
            recency: 0,
        };
        let rows = vec![
            item(Section::Sessions, 0),
            item(Section::Sessions, 1),
            item(Section::Commands, 2),
        ];
        let m = PaletteMetrics::new(&Theme::dark());
        assert_eq!(
            CommandPalette::row_top(&rows, 0, &m),
            m.list_pad_y + m.header_h
        );
        assert_eq!(
            CommandPalette::row_top(&rows, 1, &m),
            m.list_pad_y + m.header_h + m.row_h
        );
        assert_eq!(
            CommandPalette::row_top(&rows, 2, &m),
            m.list_pad_y + m.header_h * 2. + m.row_h * 2.
        );
        assert_eq!(
            CommandPalette::content_height(&rows, &m),
            2. * m.list_pad_y + m.header_h * 2. + m.row_h * 3.
        );
    }

    /// With the palette filter focused, the `Picker` arrows must win over the
    /// `Composer` arrows (both ride on the same input), so ↓/↑ move the
    /// highlight instead of the caret.
    #[gpui::test]
    fn arrow_keys_move_the_highlight(cx: &mut gpui::TestAppContext) {
        bind_picker_arrows(cx);
        let cx = cx.add_empty_window();
        let palette = open_test_palette(cx);
        let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            palette.clone()
        });

        assert_eq!(cx.update(|_, cx| palette.read(cx).highlighted), 0);
        cx.simulate_keystrokes("down");
        assert_eq!(
            cx.update(|_, cx| palette.read(cx).highlighted),
            1,
            "↓ moves the highlight"
        );
        cx.simulate_keystrokes("down");
        assert_eq!(cx.update(|_, cx| palette.read(cx).highlighted), 2);
        cx.simulate_keystrokes("up");
        assert_eq!(
            cx.update(|_, cx| palette.read(cx).highlighted),
            1,
            "↑ moves the highlight"
        );
    }

    /// The highlight must stay inside the *actual* (window-capped) list
    /// viewport, not just the uncapped `list_max_h`, or ↓ walks it off-screen
    /// in a short window.
    #[gpui::test]
    fn arrows_keep_the_highlight_in_view(cx: &mut gpui::TestAppContext) {
        bind_picker_arrows(cx);
        let cx = cx.add_empty_window();
        let palette = open_test_palette(cx);
        let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            palette.clone()
        });

        let metrics = PaletteMetrics::new(&Theme::dark());
        let rows = cx.update(|_, cx| palette.read(cx).results(""));
        assert!(
            rows.len() > 12,
            "need enough rows to scroll: {}",
            rows.len()
        );

        // The test window's viewport isn't capped by the draw size, so force a
        // short viewport (four rows) the way a short window would.
        let viewport = metrics.row_h * 4.;
        for highlighted in [12usize, 6, 0] {
            cx.update(|_, cx| {
                palette.update(cx, |p, _| {
                    p.highlighted = highlighted;
                    p.ensure_visible(&rows, &metrics, viewport);
                });
            });
            let offset = cx.update(|_, cx| -f32::from(palette.read(cx).scroll.offset().y));
            let top = CommandPalette::row_top(&rows, highlighted, &metrics);
            assert!(
                top >= offset - 0.5,
                "row scrolled above the viewport: {top} < {offset}"
            );
            assert!(
                top + metrics.row_h <= offset + viewport + 0.5,
                "row below the viewport: {top} + {} > {offset} + {viewport}",
                metrics.row_h
            );
        }
    }

    /// Bind the picker arrows exactly as `main::bind_keys` does: the `Composer`
    /// arrows first, the `Picker` arrows after (later registration wins ties).
    fn bind_picker_arrows(cx: &mut gpui::TestAppContext) {
        use crate::{Down, PickerSelectNext, PickerSelectPrev, Up};
        use gpui::KeyBinding;

        cx.update(|cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.bind_keys([
                KeyBinding::new("up", Up, Some("Composer")),
                KeyBinding::new("down", Down, Some("Composer")),
                KeyBinding::new("up", PickerSelectPrev, Some("Picker")),
                KeyBinding::new("down", PickerSelectNext, Some("Picker")),
            ]);
        });
    }

    /// A palette with no sessions, its filter focused, ready to draw.
    fn open_test_palette(cx: &mut gpui::VisualTestContext) -> Entity<CommandPalette> {
        open_test_palette_with(cx, Vec::new())
    }

    /// A palette over an explicit MCP server list, its filter focused.
    fn open_test_palette_with(
        cx: &mut gpui::VisualTestContext,
        mcp_servers: Vec<PaletteMcpServer>,
    ) -> Entity<CommandPalette> {
        let snapshot = PaletteSnapshot {
            sessions: Vec::new(),
            active_path: None,
            active_pinned: false,
            busy: false,
            session_id: None,
            sidebar_visible: true,
            side_panel_visible: false,
            terminal_visible: false,
            terminal_tabs: 0,
            project_panel_visible: false,
            can_choose_model: false,
            can_choose_thinking: false,
            mcp_servers,
        };
        cx.update(|window, cx| {
            let palette = cx.new(|cx| {
                CommandPalette::new(
                    snapshot,
                    Box::new(|_, _, _| {}),
                    Box::new(|_, _, _| {}),
                    Box::new(|_, _, _| {}),
                    cx,
                )
            });
            window.focus(&palette.read(cx).focus_handle(cx));
            palette
        })
    }

    /// The palette's command rows are exactly the registry specs the snapshot
    /// enables — the palette never carries its own command list.
    #[gpui::test]
    fn palette_commands_come_from_the_registry(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let palette = open_test_palette(cx);
        let (listed, expected) = cx.update(|_, cx| {
            let palette = palette.read(cx);
            let listed: Vec<crate::commands::CommandId> = palette
                .command_items()
                .into_iter()
                .filter_map(|item| match item.action {
                    PaletteAction::Run(PaletteCommand::Run(id)) => Some(id),
                    _ => None,
                })
                .collect();
            let expected: Vec<crate::commands::CommandId> = crate::commands::COMMANDS
                .iter()
                .filter(|spec| spec.palette && command_enabled(spec.id, &palette.snapshot))
                .map(|spec| spec.id)
                .collect();
            (listed, expected)
        });
        assert!(!expected.is_empty());
        assert_eq!(
            listed, expected,
            "palette commands must mirror the registry"
        );
    }

    /// Configured MCP servers get per-server rows: enable/disable (labelled
    /// from the current state), reconnect, and test.
    #[gpui::test]
    fn mcp_servers_get_palette_rows(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let palette = open_test_palette_with(
            cx,
            vec![
                PaletteMcpServer {
                    name: "github".into(),
                    enabled: true,
                },
                PaletteMcpServer {
                    name: "sentry".into(),
                    enabled: false,
                },
            ],
        );
        let rows: Vec<(String, McpPaletteAction)> = cx.update(|_, cx| {
            palette
                .read(cx)
                .command_items()
                .into_iter()
                .filter_map(|item| match item.action {
                    PaletteAction::Mcp { name, action } => Some((name, action)),
                    _ => None,
                })
                .collect()
        });
        assert_eq!(rows.len(), 6, "two servers × enable/reconnect/test");
        assert!(rows.contains(&("github".into(), McpPaletteAction::Toggle)));
        assert!(rows.contains(&("sentry".into(), McpPaletteAction::Toggle)));
        assert!(rows.contains(&("github".into(), McpPaletteAction::Reconnect)));
        assert!(rows.contains(&("sentry".into(), McpPaletteAction::Test)));
    }
}
