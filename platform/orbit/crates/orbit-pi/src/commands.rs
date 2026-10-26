//! The command registry: one table behind the keymap, the command palette,
//! and Settings → Shortcuts.
//!
//! Every user-facing command is declared once in [`COMMANDS`] — its title,
//! category, icon, search keywords, and the chord it advertises. [`BINDINGS`]
//! is the keymap side of the same table: each row names a chord, a key
//! context, and the command it runs, and [`key_bindings`] turns those rows
//! into gpui bindings. Display surfaces must render [`CommandSpec::shortcut`]
//! (or [`crate::platform::shortcuts::label`] for raw chords) instead of
//! hand-typing glyphs, so a chip cannot outlive the binding it describes.
//!
//! Text-entry conventions (caret movement, clipboard, modal navigation) are
//! deliberately not commands: they are registered literally in `bind_keys`
//! and never appear in the palette.

use gpui::KeyBinding;

use crate::platform::shortcuts;

/// Stable identity for one app command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    // Session
    NewSession,
    RefreshSessions,
    FocusSessions,
    /// ⌘1–⌘9 (payload: which slot).
    OpenSessionSlot,
    NextSession,
    PrevSession,
    RenameSession,
    PinSession,
    CloneSession,
    CopySessionId,
    DeleteSession,
    // Agent
    FocusComposer,
    ChooseModel,
    ChooseThinking,
    /// Palette-only: the composer chip is the pointer path to the guard policy,
    /// and this is the keyboard one. It deliberately has no chord — see
    /// [`action_binding`].
    ChooseAccess,
    /// Palette-only, same reason as [`Self::ChooseAccess`]: the workflow chip is
    /// a plain div with a mouse handler, so without this the session's scope
    /// could not be changed from the keyboard at all.
    ChooseWorkflowMode,
    AbortRun,
    // Navigation
    ToggleCommandPalette,
    ToggleSearch,
    PrevTurn,
    NextTurn,
    CopyLastResponse,
    ToggleUsage,
    ReviewChanges,
    OpenGit,
    OpenWorktrees,
    // Panels
    ToggleSidebar,
    ToggleSidePanel,
    ToggleTerminal,
    NewTerminalTab,
    CloseTerminalTab,
    NextTerminalTab,
    PrevTerminalTab,
    ToggleProjectPanel,
    // Review pane (context: `ReviewDiffTree`)
    ReviewTreeNext,
    ReviewTreePrev,
    ReviewTreeToggle,
    ReviewFileNext,
    ReviewFilePrev,
    ReviewHunkNext,
    ReviewHunkPrev,
    ReviewExpandAll,
    ReviewCollapseAll,
    ReviewClose,
    // Git page tabs
    GitTabChanges,
    GitTabHistory,
    GitTabGraph,
    GitTabStashes,
    GitTabIssues,
    GitTabPulls,
    // Application
    OpenSettings,
    OpenShortcutHelp,
    CheckForUpdates,
    Quit,
    // MCP (Settings → MCP; palette-only, plus dynamic per-server rows)
    OpenMcpSettings,
    AddMcpServer,
    RefreshMcpServers,
    ReconnectMcpServers,
}

/// Palette group and Settings → Shortcuts section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Session,
    Agent,
    Navigation,
    Panels,
    Review,
    Git,
    Application,
}

impl Category {
    /// Display order on the Shortcuts page.
    pub const ORDER: [Self; 7] = [
        Self::Session,
        Self::Agent,
        Self::Navigation,
        Self::Panels,
        Self::Review,
        Self::Git,
        Self::Application,
    ];

    /// Section header key.
    pub fn group_key(self) -> &'static str {
        match self {
            Self::Session => "shortcut.group_session",
            Self::Agent => "shortcut.group_agent",
            Self::Navigation => "shortcut.group_navigation",
            Self::Panels => "shortcut.group_panels",
            Self::Review => "shortcut.group_review",
            Self::Git => "shortcut.group_git",
            Self::Application => "shortcut.group_application",
        }
    }
}

/// One command's metadata.
pub struct CommandSpec {
    pub id: CommandId,
    pub category: Category,
    pub icon: &'static str,
    /// Title key; `alt_title_key` wins when the state flag it describes is on
    /// (a visible sidebar reads "Hide Sidebar").
    pub title_key: &'static str,
    pub alt_title_key: Option<&'static str>,
    /// The advertised chord. `None` means palette-only. Display strings go
    /// through [`CommandSpec::shortcut`]; the actual keymap rows live in
    /// [`BINDINGS`].
    pub chord: Option<&'static str>,
    /// Palette fuzzy-search keywords (never displayed).
    pub keywords: &'static str,
    /// List in the command palette.
    pub palette: bool,
    /// List on Settings → Shortcuts.
    pub help: bool,
}

impl CommandSpec {
    /// The platform's display chip for this command's advertised chord.
    pub fn shortcut(&self) -> Option<String> {
        match self.id {
            // One command, nine chords — show the span, not just ⌘1.
            CommandId::OpenSessionSlot => Some(format!(
                "{}–{}",
                shortcuts::label(shortcuts::FIRST_SESSION_SLOT),
                shortcuts::label(shortcuts::LAST_SESSION_SLOT)
            )),
            _ => self.chord.map(shortcuts::label),
        }
    }
}

/// One keymap row: a chord in a context running a command. Multiple rows for
/// the same command are aliases (`⌘P` and `⌘K` both open the palette).
pub struct Binding {
    pub chord: &'static str,
    /// `None` = global; otherwise the gpui key context that owns it.
    pub context: Option<&'static str>,
    pub id: CommandId,
}

const fn command(
    id: CommandId,
    category: Category,
    icon: &'static str,
    title_key: &'static str,
    keywords: &'static str,
) -> CommandSpec {
    CommandSpec {
        id,
        category,
        icon,
        title_key,
        alt_title_key: None,
        chord: None,
        keywords,
        palette: true,
        help: false,
    }
}

/// A chorded command: advertised in help as well as the palette.
const fn with_chord(mut spec: CommandSpec, chord: &'static str) -> CommandSpec {
    spec.chord = Some(chord);
    spec.help = true;
    spec
}

const fn with_alt_title(mut spec: CommandSpec, alt_title_key: &'static str) -> CommandSpec {
    spec.alt_title_key = Some(alt_title_key);
    spec
}

/// Context-local command (review tree, Git tabs): in the help reference,
/// never in the palette.
const fn help_only(mut spec: CommandSpec) -> CommandSpec {
    spec.palette = false;
    spec.help = true;
    spec
}

use Category::{Agent, Application, Git, Navigation, Panels, Review, Session};

/// Every app command, in palette and Shortcuts-page order.
pub static COMMANDS: &[CommandSpec] = &[
    // ── Session ────────────────────────────────────────────────────────
    with_chord(
        command(
            CommandId::NewSession,
            Session,
            "icons/plus.svg",
            "command_palette.new_session",
            "new session chat conversation start task",
        ),
        shortcuts::NEW_SESSION,
    ),
    with_chord(
        command(
            CommandId::RefreshSessions,
            Session,
            "icons/refresh.svg",
            "command_palette.refresh_sessions",
            "refresh reload sessions worktrees branch git explorer files usage providers mcp disk",
        ),
        shortcuts::REFRESH,
    ),
    with_chord(
        command(
            CommandId::FocusSessions,
            Session,
            "icons/layout-left.svg",
            "command_palette.focus_sessions",
            "focus navigate keyboard sessions sidebar arrow keys",
        ),
        shortcuts::FOCUS_SESSIONS,
    ),
    with_chord(
        help_only(command(
            CommandId::OpenSessionSlot,
            Session,
            "icons/chat.svg",
            "command_palette.open_session_slot",
            "go to session number switch jump",
        )),
        shortcuts::FIRST_SESSION_SLOT,
    ),
    with_chord(
        command(
            CommandId::NextSession,
            Session,
            "icons/arrow-down.svg",
            "command_palette.next_session",
            "next session cycle switch tab",
        ),
        "ctrl-tab",
    ),
    with_chord(
        command(
            CommandId::PrevSession,
            Session,
            "icons/arrow-up.svg",
            "command_palette.previous_session",
            "previous session cycle switch tab",
        ),
        "ctrl-shift-tab",
    ),
    command(
        CommandId::RenameSession,
        Session,
        "icons/pencil.svg",
        "command_palette.rename_session",
        "rename title name session",
    ),
    with_alt_title(
        command(
            CommandId::PinSession,
            Session,
            "icons/pin.svg",
            "sidebar.pin_session",
            "pin unpin shortlist favorite session",
        ),
        "sidebar.unpin_session",
    ),
    command(
        CommandId::CloneSession,
        Session,
        "icons/git-fork.svg",
        "command_palette.clone_session",
        "clone duplicate fork copy session branch conversation",
    ),
    command(
        CommandId::CopySessionId,
        Session,
        "icons/copy.svg",
        "command_palette.copy_session_id",
        "copy session id uuid identifier debug",
    ),
    command(
        CommandId::DeleteSession,
        Session,
        "icons/trash.svg",
        "sidebar.delete_session_menu",
        "delete remove session file disk",
    ),
    // ── Agent ──────────────────────────────────────────────────────────
    command(
        CommandId::FocusComposer,
        Agent,
        "icons/compose.svg",
        "command_palette.focus_composer",
        "focus composer prompt input message write",
    ),
    with_chord(
        command(
            CommandId::ChooseModel,
            Agent,
            "icons/spark.svg",
            "command_palette.choose_model",
            "choose change select model provider agent",
        ),
        shortcuts::CHOOSE_MODEL,
    ),
    with_chord(
        command(
            CommandId::ChooseThinking,
            Agent,
            "icons/thinking-medium.svg",
            "command_palette.choose_thinking_level",
            "choose change select thinking level reasoning effort",
        ),
        shortcuts::CHOOSE_THINKING,
    ),
    // No chord, deliberately: the three agent-control chips spend the
    // available ones (⌘⇧M, ⌘⇧T), and a guard policy that decides whether the
    // agent may run a command without asking is worth reaching from the
    // keyboard anyway. Palette-only is enough, and the chip stays the pointer
    // path — including when it is showing its glyph alone at the default.
    command(
        CommandId::ChooseAccess,
        Agent,
        "icons/lock.svg",
        "command_palette.choose_access",
        "access mode permission supervised auto accept edits full access guard approve tools",
    ),
    command(
        CommandId::ChooseWorkflowMode,
        Agent,
        "icons/task.svg",
        "command_palette.choose_workflow_mode",
        "workflow mode plan build ask scope read only changes",
    ),
    with_chord(
        command(
            CommandId::AbortRun,
            Agent,
            "icons/stop.svg",
            "command_palette.abort_run",
            "abort stop cancel run agent working",
        ),
        shortcuts::STOP,
    ),
    // ── Navigation ─────────────────────────────────────────────────────
    with_chord(
        help_only(command(
            CommandId::ToggleCommandPalette,
            Navigation,
            "icons/search.svg",
            "menu.command_palette",
            "command palette search quick open",
        )),
        shortcuts::PALETTE,
    ),
    with_chord(
        command(
            CommandId::ToggleSearch,
            Navigation,
            "icons/search.svg",
            "menu.find_in_transcript",
            "find search transcript messages",
        ),
        shortcuts::FIND,
    ),
    with_chord(
        command(
            CommandId::PrevTurn,
            Navigation,
            "icons/arrow-up.svg",
            "shortcut.prev_turn",
            "previous turn prompt jump transcript",
        ),
        shortcuts::PREV_TURN,
    ),
    with_chord(
        command(
            CommandId::NextTurn,
            Navigation,
            "icons/arrow-down.svg",
            "shortcut.next_turn",
            "next turn prompt jump transcript",
        ),
        shortcuts::NEXT_TURN,
    ),
    with_chord(
        command(
            CommandId::CopyLastResponse,
            Navigation,
            "icons/copy.svg",
            "shortcut.copy_last",
            "copy last response answer message",
        ),
        shortcuts::COPY_LAST_RESPONSE,
    ),
    with_chord(
        command(
            CommandId::ToggleUsage,
            Navigation,
            "icons/clock.svg",
            "menu.usage",
            "usage cost tokens analytics",
        ),
        shortcuts::USAGE,
    ),
    with_chord(
        command(
            CommandId::ReviewChanges,
            Navigation,
            "icons/file-diff.svg",
            "command_palette.review_changes",
            "review git diff changes files panel",
        ),
        shortcuts::REVIEW_CHANGES,
    ),
    with_chord(
        command(
            CommandId::OpenGit,
            Navigation,
            "icons/git-commit.svg",
            "command_palette.open_git",
            "git commit push branch history graph changes",
        ),
        shortcuts::OPEN_GIT,
    ),
    command(
        CommandId::OpenWorktrees,
        Navigation,
        "icons/branch.svg",
        "worktree.command_open",
        "worktree worktrees git branch parallel workspace create",
    ),
    // ── Panels ─────────────────────────────────────────────────────────
    with_alt_title(
        with_chord(
            command(
                CommandId::ToggleSidebar,
                Panels,
                "icons/layout-left.svg",
                "command_palette.show_sidebar",
                "toggle show hide left sidebar sessions history",
            ),
            shortcuts::SIDEBAR,
        ),
        "command_palette.hide_sidebar",
    ),
    with_alt_title(
        command(
            CommandId::ToggleSidePanel,
            Panels,
            "icons/panel-right.svg",
            "command_palette.show_side_panel",
            "toggle show hide right panel review git diff",
        ),
        "command_palette.hide_side_panel",
    ),
    with_alt_title(
        with_chord(
            command(
                CommandId::ToggleTerminal,
                Panels,
                "icons/terminal.svg",
                "command_palette.show_terminal",
                "toggle show hide terminal shell console command line pty",
            ),
            shortcuts::TERMINAL,
        ),
        "command_palette.hide_terminal",
    ),
    with_chord(
        command(
            CommandId::NewTerminalTab,
            Panels,
            "icons/plus.svg",
            "command_palette.new_terminal_tab",
            "new terminal tab shell console spawn another",
        ),
        shortcuts::NEW_TERMINAL_TAB,
    ),
    with_chord(
        command(
            CommandId::CloseTerminalTab,
            Panels,
            "icons/x.svg",
            "command_palette.close_terminal_tab",
            "close terminal tab shell console kill",
        ),
        shortcuts::CLOSE_TERMINAL_TAB,
    ),
    with_chord(
        command(
            CommandId::NextTerminalTab,
            Panels,
            "icons/arrow-down.svg",
            "command_palette.next_terminal_tab",
            "next terminal tab shell console switch cycle",
        ),
        shortcuts::NEXT_TERMINAL_TAB,
    ),
    with_chord(
        command(
            CommandId::PrevTerminalTab,
            Panels,
            "icons/arrow-up.svg",
            "command_palette.prev_terminal_tab",
            "previous terminal tab shell console switch cycle",
        ),
        shortcuts::PREV_TERMINAL_TAB,
    ),
    with_alt_title(
        with_chord(
            command(
                CommandId::ToggleProjectPanel,
                Panels,
                "icons/folder.svg",
                "explorer.show",
                "explorer files project panel tree folders workspace toggle show hide",
            ),
            shortcuts::PROJECT_PANEL,
        ),
        "explorer.hide",
    ),
    // ── Review pane (context: `ReviewDiffTree`) ────────────────────────
    with_chord(
        help_only(command(
            CommandId::ReviewTreeNext,
            Review,
            "icons/arrow-down.svg",
            "sidepane.next_row",
            "review tree next row down",
        )),
        "down",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewTreePrev,
            Review,
            "icons/arrow-up.svg",
            "sidepane.previous_row",
            "review tree previous row up",
        )),
        "up",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewTreeToggle,
            Review,
            "icons/chevron-right.svg",
            "sidepane.open_or_toggle",
            "review tree open file toggle folder expand collapse",
        )),
        "enter",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewFileNext,
            Review,
            "icons/arrow-down.svg",
            "sidepane.next_file",
            "review next changed file",
        )),
        "n",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewFilePrev,
            Review,
            "icons/arrow-up.svg",
            "sidepane.previous_file",
            "review previous changed file",
        )),
        "p",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewHunkNext,
            Review,
            "icons/arrow-down.svg",
            "sidepane.next_hunk",
            "review next hunk diff change",
        )),
        "]",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewHunkPrev,
            Review,
            "icons/arrow-up.svg",
            "sidepane.previous_hunk",
            "review previous hunk diff change",
        )),
        "[",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewExpandAll,
            Review,
            "icons/expand-all.svg",
            "sidepane.expand_all",
            "review expand all files diffs",
        )),
        "e",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewCollapseAll,
            Review,
            "icons/collapse-all.svg",
            "sidepane.collapse_all",
            "review collapse all files diffs",
        )),
        "c",
    ),
    with_chord(
        help_only(command(
            CommandId::ReviewClose,
            Review,
            "icons/arrow-left.svg",
            "sidepane.back_to_chat",
            "review back chat composer focus escape",
        )),
        "escape",
    ),
    // ── Git page tabs ──────────────────────────────────────────────────
    with_chord(
        help_only(command(
            CommandId::GitTabChanges,
            Git,
            "icons/file-diff.svg",
            "git_panel.tab_changes",
            "git changes tab working tree",
        )),
        "secondary-alt-1",
    ),
    with_chord(
        help_only(command(
            CommandId::GitTabHistory,
            Git,
            "icons/clock.svg",
            "git_panel.tab_history",
            "git history tab commits",
        )),
        "secondary-alt-2",
    ),
    with_chord(
        help_only(command(
            CommandId::GitTabGraph,
            Git,
            "icons/git-fork.svg",
            "git_panel.tab_graph",
            "git graph tab branches",
        )),
        "secondary-alt-3",
    ),
    with_chord(
        help_only(command(
            CommandId::GitTabStashes,
            Git,
            "icons/archive.svg",
            "git_panel.tab_stashes",
            "git stashes tab stash pop apply drop",
        )),
        "secondary-alt-4",
    ),
    with_chord(
        help_only(command(
            CommandId::GitTabIssues,
            Git,
            "icons/github.svg",
            "git_panel.tab_issues",
            "git issues tab github",
        )),
        "secondary-alt-5",
    ),
    with_chord(
        help_only(command(
            CommandId::GitTabPulls,
            Git,
            "icons/git-pull-request.svg",
            "git_panel.tab_pulls",
            "git pulls requests tab github",
        )),
        "secondary-alt-6",
    ),
    // ── Application ────────────────────────────────────────────────────
    with_chord(
        command(
            CommandId::OpenSettings,
            Application,
            "icons/settings.svg",
            "menu.settings",
            "settings preferences configure",
        ),
        shortcuts::SETTINGS,
    ),
    with_chord(
        command(
            CommandId::OpenShortcutHelp,
            Application,
            "icons/keyboard.svg",
            "settings.shortcuts",
            "keyboard shortcuts keys reference help",
        ),
        shortcuts::SHORTCUT_HELP,
    ),
    with_chord(
        help_only(command(
            CommandId::CheckForUpdates,
            Application,
            "icons/rotate-ccw.svg",
            "menu.check_for_updates",
            "update upgrade version release",
        )),
        shortcuts::CHECK_UPDATES,
    ),
    with_chord(
        help_only(command(
            CommandId::Quit,
            Application,
            "icons/x.svg",
            "menu.quit",
            "quit exit close application",
        )),
        shortcuts::QUIT,
    ),
    // ── MCP ────────────────────────────────────────────────────────────
    command(
        CommandId::OpenMcpSettings,
        Agent,
        "icons/tools/mcp.svg",
        "mcp.command_open",
        "mcp model context protocol servers tools settings",
    ),
    command(
        CommandId::AddMcpServer,
        Agent,
        "icons/plus.svg",
        "mcp.add_server",
        "mcp add server model context protocol tools",
    ),
    command(
        CommandId::RefreshMcpServers,
        Agent,
        "icons/refresh.svg",
        "mcp.command_refresh",
        "mcp refresh reload servers status tools",
    ),
    command(
        CommandId::ReconnectMcpServers,
        Agent,
        "icons/rotate-ccw.svg",
        "mcp.command_reconnect",
        "mcp reconnect restart servers apply configuration",
    ),
];

/// The command table's keymap rows, in registration order.
pub static BINDINGS: &[Binding] = &[
    Binding {
        chord: shortcuts::QUIT,
        context: None,
        id: CommandId::Quit,
    },
    // Escape and ⌘. both abort; Escape is registered first so a context that
    // re-binds it (Terminal, dialogs, the review tree) wins by order.
    Binding {
        chord: "escape",
        context: None,
        id: CommandId::AbortRun,
    },
    Binding {
        chord: "secondary-.",
        context: None,
        id: CommandId::AbortRun,
    },
    Binding {
        chord: shortcuts::NEW_SESSION,
        context: None,
        id: CommandId::NewSession,
    },
    Binding {
        chord: shortcuts::REFRESH,
        context: None,
        id: CommandId::RefreshSessions,
    },
    Binding {
        chord: shortcuts::FOCUS_SESSIONS,
        context: None,
        id: CommandId::FocusSessions,
    },
    // ⌘1…⌘9 (Ctrl+1…0 off macOS): open the Nth session in the sidebar's
    // visible order — numbers match what the user sees.
    Binding {
        chord: "secondary-1",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-2",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-3",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-4",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-5",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-6",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-7",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-8",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    Binding {
        chord: "secondary-9",
        context: None,
        id: CommandId::OpenSessionSlot,
    },
    // Tab cycling across sessions works on every platform (browsers made
    // Ctrl+Tab the convention); the registry help advertises both.
    Binding {
        chord: "ctrl-tab",
        context: None,
        id: CommandId::NextSession,
    },
    Binding {
        chord: "ctrl-shift-tab",
        context: None,
        id: CommandId::PrevSession,
    },
    Binding {
        chord: shortcuts::SETTINGS,
        context: None,
        id: CommandId::OpenSettings,
    },
    Binding {
        chord: shortcuts::USAGE,
        context: None,
        id: CommandId::ToggleUsage,
    },
    Binding {
        chord: shortcuts::CHOOSE_MODEL,
        context: None,
        id: CommandId::ChooseModel,
    },
    Binding {
        chord: shortcuts::CHOOSE_THINKING,
        context: None,
        id: CommandId::ChooseThinking,
    },
    Binding {
        chord: shortcuts::REVIEW_CHANGES,
        context: None,
        id: CommandId::ReviewChanges,
    },
    Binding {
        chord: shortcuts::OPEN_GIT,
        context: None,
        id: CommandId::OpenGit,
    },
    Binding {
        chord: shortcuts::SHORTCUT_HELP,
        context: None,
        id: CommandId::OpenShortcutHelp,
    },
    Binding {
        chord: shortcuts::PALETTE,
        context: None,
        id: CommandId::ToggleCommandPalette,
    },
    Binding {
        chord: shortcuts::PALETTE_ALT,
        context: None,
        id: CommandId::ToggleCommandPalette,
    },
    Binding {
        chord: shortcuts::FIND,
        context: None,
        id: CommandId::ToggleSearch,
    },
    Binding {
        chord: shortcuts::PREV_TURN,
        context: None,
        id: CommandId::PrevTurn,
    },
    Binding {
        chord: shortcuts::NEXT_TURN,
        context: None,
        id: CommandId::NextTurn,
    },
    Binding {
        chord: shortcuts::COPY_LAST_RESPONSE,
        context: None,
        id: CommandId::CopyLastResponse,
    },
    Binding {
        chord: shortcuts::CHECK_UPDATES,
        context: None,
        id: CommandId::CheckForUpdates,
    },
    Binding {
        chord: shortcuts::TERMINAL,
        context: None,
        id: CommandId::ToggleTerminal,
    },
    // Terminal tabs. Scoped to the `Terminal` key context: these chords belong
    // to the shell everywhere else.
    Binding {
        chord: shortcuts::NEW_TERMINAL_TAB,
        context: Some("Terminal"),
        id: CommandId::NewTerminalTab,
    },
    Binding {
        chord: shortcuts::CLOSE_TERMINAL_TAB,
        context: Some("Terminal"),
        id: CommandId::CloseTerminalTab,
    },
    Binding {
        chord: shortcuts::NEXT_TERMINAL_TAB,
        context: Some("Terminal"),
        id: CommandId::NextTerminalTab,
    },
    Binding {
        chord: shortcuts::PREV_TERMINAL_TAB,
        context: Some("Terminal"),
        id: CommandId::PrevTerminalTab,
    },
    Binding {
        chord: shortcuts::SIDEBAR,
        context: None,
        id: CommandId::ToggleSidebar,
    },
    Binding {
        chord: shortcuts::PROJECT_PANEL,
        context: None,
        id: CommandId::ToggleProjectPanel,
    },
    // Git page tabs: `secondary` so Windows/Linux get Ctrl+1…5 too.
    Binding {
        chord: "secondary-alt-1",
        context: None,
        id: CommandId::GitTabChanges,
    },
    Binding {
        chord: "secondary-alt-2",
        context: None,
        id: CommandId::GitTabHistory,
    },
    Binding {
        chord: "secondary-alt-3",
        context: None,
        id: CommandId::GitTabGraph,
    },
    Binding {
        chord: "secondary-alt-4",
        context: None,
        id: CommandId::GitTabStashes,
    },
    Binding {
        chord: "secondary-alt-5",
        context: None,
        id: CommandId::GitTabIssues,
    },
    Binding {
        chord: "secondary-alt-6",
        context: None,
        id: CommandId::GitTabPulls,
    },
    // Review pane: registered after the global abort so Escape returns to
    // the composer instead of aborting the run.
    Binding {
        chord: "down",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewTreeNext,
    },
    Binding {
        chord: "up",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewTreePrev,
    },
    Binding {
        chord: "enter",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewTreeToggle,
    },
    Binding {
        chord: "space",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewTreeToggle,
    },
    Binding {
        chord: "n",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewFileNext,
    },
    Binding {
        chord: "p",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewFilePrev,
    },
    Binding {
        chord: "]",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewHunkNext,
    },
    Binding {
        chord: "[",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewHunkPrev,
    },
    Binding {
        chord: "e",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewExpandAll,
    },
    Binding {
        chord: "c",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewCollapseAll,
    },
    Binding {
        chord: "escape",
        context: Some("ReviewDiffTree"),
        id: CommandId::ReviewClose,
    },
];

/// The registry spec for `id`. Panics if the id is missing — a spec-less id
/// is a programming error, caught by `every_binding_has_a_spec`.
pub fn spec(id: CommandId) -> &'static CommandSpec {
    COMMANDS
        .iter()
        .find(|spec| spec.id == id)
        .unwrap_or_else(|| panic!("command {id:?} has no spec"))
}

/// The registry title for a command. `alternate` picks the toggle's other
/// face — a visible sidebar reads "Hide Sidebar".
pub fn title(id: CommandId, alternate: bool) -> String {
    let spec = spec(id);
    if alternate {
        tr!(spec.alt_title_key.unwrap_or(spec.title_key))
    } else {
        tr!(spec.title_key)
    }
}

/// The hover tooltip for a command: its title plus the platform chord when it
/// has one ("Choose Model (⌘⇧M)"). Every command-backed control calls this,
/// so a hover hint can never drift from the keymap.
pub fn tooltip(id: CommandId, alternate: bool) -> String {
    match spec(id).shortcut() {
        Some(chord) => format!("{} ({chord})", title(id, alternate)),
        None => title(id, alternate),
    }
}

/// The typed gpui binding for one row, or `None` for palette-only commands.
fn action_binding(binding: &Binding) -> Option<KeyBinding> {
    use crate::{
        AbortRun, CheckForUpdates, CloseTerminalTab, CopyLastResponse, FocusSessions,
        GitTabChanges, GitTabGraph, GitTabHistory, GitTabIssues, GitTabPulls, GitTabStashes,
        NewSession, NewTerminalTab, NextSession, NextTerminalTab, NextTurn, OpenGit,
        OpenSessionSlot, OpenSettings, OpenShortcutHelp, PrevSession, PrevTerminalTab, PrevTurn,
        Quit, RefreshSessions, ReviewChanges, ReviewClose, ReviewCollapseAll, ReviewExpandAll,
        ReviewFileNext, ReviewFilePrev, ReviewHunkNext, ReviewHunkPrev, ReviewTreeNext,
        ReviewTreePrev, ReviewTreeToggle, ToggleCommandPalette, ToggleModelMenu,
        ToggleProjectPanel, ToggleSearch, ToggleSidebar, ToggleTerminal, ToggleThinkingMenu,
        ToggleUsage,
    };
    let chord = binding.chord;
    let context = binding.context;
    Some(match binding.id {
        CommandId::Quit => KeyBinding::new(chord, Quit, context),
        CommandId::AbortRun => KeyBinding::new(chord, AbortRun, context),
        CommandId::NewSession => KeyBinding::new(chord, NewSession, context),
        CommandId::RefreshSessions => KeyBinding::new(chord, RefreshSessions, context),
        CommandId::FocusSessions => KeyBinding::new(chord, FocusSessions, context),
        CommandId::OpenSessionSlot => {
            // The slot rides the chord (⌘1…⌘9) — the only payload command.
            let slot = chord
                .rsplit('-')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            KeyBinding::new(chord, OpenSessionSlot { slot }, context)
        }
        CommandId::NextSession => KeyBinding::new(chord, NextSession, context),
        CommandId::PrevSession => KeyBinding::new(chord, PrevSession, context),
        CommandId::OpenSettings => KeyBinding::new(chord, OpenSettings, context),
        CommandId::ToggleUsage => KeyBinding::new(chord, ToggleUsage, context),
        CommandId::ChooseModel => KeyBinding::new(chord, ToggleModelMenu, context),
        CommandId::ChooseThinking => KeyBinding::new(chord, ToggleThinkingMenu, context),
        CommandId::ReviewChanges => KeyBinding::new(chord, ReviewChanges, context),
        CommandId::OpenGit => KeyBinding::new(chord, OpenGit, context),
        CommandId::OpenShortcutHelp => KeyBinding::new(chord, OpenShortcutHelp, context),
        CommandId::ToggleCommandPalette => KeyBinding::new(chord, ToggleCommandPalette, context),
        CommandId::ToggleSearch => KeyBinding::new(chord, ToggleSearch, context),
        CommandId::PrevTurn => KeyBinding::new(chord, PrevTurn, context),
        CommandId::NextTurn => KeyBinding::new(chord, NextTurn, context),
        CommandId::CopyLastResponse => KeyBinding::new(chord, CopyLastResponse, context),
        CommandId::CheckForUpdates => KeyBinding::new(chord, CheckForUpdates, context),
        CommandId::ToggleTerminal => KeyBinding::new(chord, ToggleTerminal, context),
        CommandId::NewTerminalTab => KeyBinding::new(chord, NewTerminalTab, context),
        CommandId::CloseTerminalTab => KeyBinding::new(chord, CloseTerminalTab, context),
        CommandId::NextTerminalTab => KeyBinding::new(chord, NextTerminalTab, context),
        CommandId::PrevTerminalTab => KeyBinding::new(chord, PrevTerminalTab, context),
        CommandId::ToggleSidebar => KeyBinding::new(chord, ToggleSidebar, context),
        CommandId::ToggleProjectPanel => KeyBinding::new(chord, ToggleProjectPanel, context),
        CommandId::GitTabChanges => KeyBinding::new(chord, GitTabChanges, context),
        CommandId::GitTabHistory => KeyBinding::new(chord, GitTabHistory, context),
        CommandId::GitTabGraph => KeyBinding::new(chord, GitTabGraph, context),
        CommandId::GitTabIssues => KeyBinding::new(chord, GitTabIssues, context),
        CommandId::GitTabPulls => KeyBinding::new(chord, GitTabPulls, context),
        CommandId::GitTabStashes => KeyBinding::new(chord, GitTabStashes, context),
        CommandId::ReviewTreeNext => KeyBinding::new(chord, ReviewTreeNext, context),
        CommandId::ReviewTreePrev => KeyBinding::new(chord, ReviewTreePrev, context),
        CommandId::ReviewTreeToggle => KeyBinding::new(chord, ReviewTreeToggle, context),
        CommandId::ReviewFileNext => KeyBinding::new(chord, ReviewFileNext, context),
        CommandId::ReviewFilePrev => KeyBinding::new(chord, ReviewFilePrev, context),
        CommandId::ReviewHunkNext => KeyBinding::new(chord, ReviewHunkNext, context),
        CommandId::ReviewHunkPrev => KeyBinding::new(chord, ReviewHunkPrev, context),
        CommandId::ReviewExpandAll => KeyBinding::new(chord, ReviewExpandAll, context),
        CommandId::ReviewCollapseAll => KeyBinding::new(chord, ReviewCollapseAll, context),
        CommandId::ReviewClose => KeyBinding::new(chord, ReviewClose, context),
        // Palette-only commands have no keymap row.
        CommandId::RenameSession
        | CommandId::PinSession
        | CommandId::CloneSession
        | CommandId::CopySessionId
        | CommandId::DeleteSession
        | CommandId::FocusComposer
        | CommandId::ChooseAccess
        | CommandId::ChooseWorkflowMode
        | CommandId::ToggleSidePanel
        | CommandId::OpenMcpSettings
        | CommandId::OpenWorktrees
        | CommandId::AddMcpServer
        | CommandId::RefreshMcpServers
        | CommandId::ReconnectMcpServers => return None,
    })
}

/// The keymap rows for every command in [`BINDINGS`].
pub fn key_bindings() -> Vec<KeyBinding> {
    BINDINGS.iter().filter_map(action_binding).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every control on the composer row is a plain `div` with a mouse handler,
    /// so a chip whose command has neither a chord nor a palette row is a
    /// setting a keyboard-only user cannot change at all. PRODUCT.md promises
    /// every primary workflow is possible without a mouse, and the guard policy
    /// is the one that decides whether the agent may run a command unasked.
    #[test]
    fn composer_controls_are_reachable_without_a_pointer() {
        for id in [
            CommandId::ChooseModel,
            CommandId::ChooseThinking,
            CommandId::ChooseWorkflowMode,
            CommandId::ChooseAccess,
        ] {
            let spec = spec(id);
            assert!(
                spec.chord.is_some() || spec.palette,
                "{id:?} has a composer chip but no chord and no palette row"
            );
        }
    }

    /// GPUI resolves an icon path through the embedded asset source, and a path
    /// it cannot find is a failure the first time the row is drawn — not a
    /// missing picture. A typo here costs a user their palette, so the whole
    /// registry is checked against the assets that actually ship.
    #[test]
    fn every_command_icon_is_embedded() {
        for spec in COMMANDS {
            let bytes = <crate::assets::Assets as gpui::AssetSource>::load(
                &crate::assets::Assets,
                spec.icon,
            )
            .expect("asset source reads");
            assert!(
                bytes.is_some(),
                "command {:?} points at a missing icon: {}",
                spec.id,
                spec.icon
            );
        }
    }

    #[test]
    fn every_binding_has_a_spec() {
        for binding in BINDINGS {
            let spec = spec(binding.id);
            assert_eq!(spec.id, binding.id);
        }
    }

    #[test]
    fn every_binding_builds_a_typed_binding() {
        // `KeyBinding::new` also parses the chord and panics on a typo, so
        // this is the registry's syntax guard as well as its wiring guard.
        for binding in BINDINGS {
            assert!(
                action_binding(binding).is_some(),
                "{:?} ({}) has a keymap row but no action",
                binding.id,
                binding.chord
            );
        }
        assert_eq!(key_bindings().len(), BINDINGS.len());
    }

    #[test]
    fn chords_do_not_collide_inside_a_context() {
        let mut seen: HashSet<(&str, Option<&str>)> = HashSet::new();
        for binding in BINDINGS {
            assert!(
                seen.insert((binding.chord, binding.context)),
                "duplicate binding: {} in context {:?}",
                binding.chord,
                binding.context
            );
        }
    }

    #[test]
    fn advertised_chords_match_a_keymap_row() {
        for spec in COMMANDS {
            let Some(chord) = spec.chord else { continue };
            assert!(
                BINDINGS
                    .iter()
                    .any(|binding| { binding.id == spec.id && binding.chord == chord }),
                "{:?} advertises {chord} but no keymap row binds it",
                spec.id
            );
        }
    }

    #[test]
    fn command_ids_are_unique() {
        let mut seen: HashSet<CommandId> = HashSet::new();
        for spec in COMMANDS {
            assert!(seen.insert(spec.id), "duplicate command id {:?}", spec.id);
        }
    }

    #[test]
    fn tooltips_append_the_platform_chord() {
        assert_eq!(
            tooltip(CommandId::ChooseModel, false),
            format!(
                "{} ({})",
                tr!("command_palette.choose_model"),
                shortcuts::label(shortcuts::CHOOSE_MODEL)
            )
        );
        // A chord-less command shows its bare title.
        assert_eq!(
            tooltip(CommandId::ToggleSidePanel, false),
            tr!("command_palette.show_side_panel")
        );
        // A toggle's alternate face is what the hint names.
        assert_eq!(
            tooltip(CommandId::ToggleSidebar, true),
            format!(
                "{} ({})",
                tr!("command_palette.hide_sidebar"),
                shortcuts::label(shortcuts::SIDEBAR)
            )
        );
    }

    #[test]
    fn shortcut_help_has_a_row_for_every_help_command_with_a_chord() {
        for spec in COMMANDS.iter().filter(|spec| spec.help) {
            if spec.chord.is_some() {
                // The chip is derived from the same chord the keymap uses, so
                // help can never drift from the binding.
                assert!(spec.shortcut().is_some());
            }
        }
    }
}
