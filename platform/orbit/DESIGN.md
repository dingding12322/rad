---
name: Orbit Pi
description: Native GPUI workbench for the pi coding agent — quiet chrome, dense data, GPU-drawn.
colors:
  canvas: "#181818"
  sidebar: "#1F1F1F"
  surface-secondary: "#242424"
  raised: "#292929"
  hover: "#303030"
  active: "#363636"
  selection: "#2F3949"
  ink: "#E5E5E5"
  ink-secondary: "#A3A3A3"
  ink-tertiary: "#909090"
  ink-disabled: "#606060"
  hairline: "#343434"
  hairline-subtle: "#292929"
  accent: "#286CF0"
  accent-hover: "#2D6FEF"
  accent-pressed: "#1F5DD8"
  accent-foreground: "#FFFFFF"
  focus-ring: "#6EA0FF"
  success: "#4CAF86"
  danger: "#E16D70"
  caution: "#E5AD55"
  info: "#65A4E8"
  canvas-light: "#F5F5F5"
  sidebar-light: "#FFFFFF"
  raised-light: "#F0F0F0"
  hover-light: "#EBEBEB"
  active-light: "#E2E2E2"
  selection-light: "#E8EEFA"
  ink-light: "#171717"
  ink-secondary-light: "#525252"
  ink-tertiary-light: "#696969"
  hairline-light: "#E5E5E5"
  accent-light: "#286CF0"
  focus-ring-light: "#286CF0"
typography:
  title:
    fontFamily: ".ZedSans, IBM Plex Sans, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 500
    lineHeight: "20px"
  body:
    fontFamily: ".ZedSans, IBM Plex Sans, system-ui, sans-serif"
    fontSize: "13px"
    lineHeight: "18px"
  label:
    fontFamily: ".ZedSans, IBM Plex Sans, system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 500
    letterSpacing: "0.02em"
  number:
    fontFamily: ".ZedSans, IBM Plex Sans, system-ui, sans-serif"
    fontSize: "20px"
    fontWeight: 500
    lineHeight: "24px"
  mono:
    fontFamily: ".ZedMono, Lilex, ui-monospace, monospace"
    fontSize: "13px"
    lineHeight: "20px"
rounded:
  sm: "6px"
  md: "8px"
  lg: "12px"
  xl: "16px"
  full: "9999px"
spacing:
  hair: "2px"
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "20px"
  xxl: "28px"
components:
  nav-row:
    backgroundColor: "{colors.canvas}"
    textColor: "{colors.ink-secondary}"
    rounded: "{rounded.sm}"
    height: "28px"
    padding: "0 10px"
  nav-row-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.ink}"
  nav-row-active:
    backgroundColor: "{colors.selection}"
    textColor: "{colors.ink}"
  primary-button:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    height: "34px"
    padding: "0 10px"
  accent-button:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-foreground}"
    rounded: "{rounded.md}"
    height: "34px"
    padding: "0 10px"
  chip:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    height: "28px"
    padding: "0 8px"
  metric-board:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.lg}"
    padding: "0"
  composer:
    backgroundColor: "{colors.sidebar}"
    textColor: "{colors.ink}"
    rounded: "{rounded.xl}"
    padding: "8px 12px"
---

# Design System: Orbit Pi

## Overview

**Creative North Star: "The Instrument Panel"**

Orbit is a workbench, not a website. Every surface is drawn by the GPU through GPUI,
so the chrome stays flat, honest, and quiet while the data — transcripts, diffs,
graphs, usage — carries the density. There is one accent (blue) and it is
rationed: primary actions, selection, focus, the caret, the active series. Everything
else is ink on canvas at four weights of emphasis.

The system is monochrome-by-default with semantic color reserved for state, so a
screenshot in any of the ten palettes still reads as Orbit. Depth is delivered by
hairlines and tonal steps rather than shadows; shadows exist only for surfaces that
genuinely float above the page (composer, popovers, modals).

**Key Characteristics:**
- One accent, rationed: blue marks the active thing and nothing else.
- Hairlines over boxes: `border_1` at `theme.border` separates; cards are the exception.
- Four ink weights: `text_primary` (primary), `text_secondary` (secondary),
  `text_muted` (tertiary/labels), and `text_disabled` (inactive controls).
- Density with air: 28–34px controls, 6–10px padding, 44px page headers. The
  Appearance panel exposes this as UI, Terminal/Editor font sizes (px), and
  Spacing Density (a global multiplier through `theme.space`).
- Ten palettes, one grammar: every color is read from `theme::get(cx)`, never
  hardcoded. Appearance lists the five dark and five light palettes for the mode in
  effect, so the common choice is never made inside a catalog-sized list.

## Colors

A charcoal canvas with a single blue accent; light mode is warm-neutral off-white, not
white. Alternate palettes keep their own signature hue inside the same grammar.

### Primary
- **Accent** (#286CF0 both modes): primary actions, selection accents, focus, links, the
  active data series, the caret, and the inline-code wash. `accent` is the fill role;
  `accent_text` (#6EA0FF dark / #1F5DD8 light) is the readable foreground role for
  links, icons, keywords, and mentions. `accent_fg` is white on the fill. Never
  decoration, never a large fill outside a primary control.
- **Selection** (#2F3949 dark / #E8EEFA light): the active navigation/list row. It is a
  restrained blue-tinted wash, not a bright accent fill, and pairs with `selection_fg`.
  The dark wash is one step below the reference #303A4B so `accent_text` labels and
  secondary metadata on a selected row still clear 4.5:1.

### Neutral
- **Canvas** (#181818 / #F5F5F5): the main content plane and window background.
- **Surface** (#1F1F1F / #FFFFFF): sidebars, panels, and the composer plane.
- **Surface Secondary** (#242424 / #F8F8F8): nested containers and quiet wells.
- **Elevated** (#292929 / #F0F0F0): chips, menus, popovers, modals, grouped rows.
- **Hover** (#303030 / #EBEBEB): pointer feedback only.
- **Active** (#363636 / #E2E2E2): pressed/open controls; distinct from `selection`.
- **Hairline** (#343434 / #E5E5E5) and **Hairline Subtle** (#292929 / #EEEEEE):
  1px separators, table rules, section boundaries.
- **Ink** (#E5E5E5 / #171717), **Ink Secondary** (#A3A3A3 / #525252),
  **Ink Muted** (#909090 / #696969), **Ink Disabled** (#606060 / #A3A3A3).

### Named Rules
**The One Accent Rule.** The accent appears on ≤10% of any screen. If two things are
accent-colored, one of them is wrong.

**The Ink Weight Rule.** Emphasis comes from ink weight and size, never from hue.
A number that must shout gets `text` + a larger size, not a color.

**The Surface Ramp Rule.** On dark palettes `raised` clears both the canvas and the
chrome plane, and each state steps up from there — `canvas ≤ raised`, `sidebar ≤
raised`, `raised ≤ hover ≤ active` — so a card, a hovered row and a selected row never
invert into holes. Light palettes mirror the ramp downward. `menu_bg` sits at `raised`;
a popover floats, it never recesses. Every palette is asserted against this
monotonicity, not just Orbit.

**The Accent-Is-Not-Status Rule.** The accent may never equal `success`, `warning`,
`error`, or `info`. If the theme's signature hue is also its success or error color,
the accent moves to the palette's second hue; a green selection over a green checkmark
reads as one state, not two.

### Composer tokens
The composer paints two inline token roles. They are read as content — siblings of the
`syn_*` editor colors — not as chrome accents, so they sit outside the accent economy:
- **`/command`** takes `accent_text` (`mention_command`): commands are actions, the same
  role terminal command words and syntax keywords already carry.
- **`@file`** takes the accent's complement (`mention_file`), so a reference never reads as
  a command.

Both are derived in `Theme`, so all ten palettes stay legible without per-palette tuning.

## Typography

**Display Font:** IBM Plex Sans (bundled as `.ZedSans`)
**Body Font:** IBM Plex Sans (`.ZedSans`)
**Label/Mono Font:** Lilex (bundled as `.ZedMono`) for code, paths, and raw values.

**Character:** One workhorse face for the whole interface — a developer tool does not
need a display pairing. Mono appears only where content is machine text: code blocks,
diffs, paths, token counts in tables.

**The Catalog.** Appearance exposes a curated Interface and Code catalog
(`theme::UI_FONTS` / `CODE_FONTS`) of bundled OFL faces — Inter, Fixel Text,
Geist Sans, Atkinson Hyperlegible, Source Sans 3, Roboto, Noto Sans, DM Sans,
Manrope, and JetBrains Mono, Fira Code, Geist Mono, Commit Mono, Source Code
Pro, Cascadia Code, Roboto Mono, Iosevka, DM Mono, IBM Plex Mono, Inconsolata,
Noto Sans Mono, Space Mono, Anonymous Pro, Martian Mono. Every family ships as
statically instanced, subset TTFs at the 400–700 weights it offers, under
`assets/fonts/bundled/` (glyph set
is Latin + punctuation + box-drawing + powerline), so a picked face never
depends on the OS. The bundled IBM Plex Sans and Lilex faces stay in the list
under their own names (their gpui aliases are `.ZedSans` / `.ZedMono`).

**The Palette Catalog.** `ThemeId::ALL` ships ten palettes: Orbit's own family plus
Flexoki. Five are dark (Orbit, Orbit Darker, Orbit Cobalt, Orbit Azure, Flexoki) and five
are light (Orbit Light, Orbit Azure Light, Orbit Paper, Orbit Contrast, Flexoki Light).
The blue design is **Orbit Azure** (dark) and **Orbit Azure Light**; the original ember
palette is simply **Orbit** / **Orbit Light**. Settings → Appearance →
**Theme** is a single control, not one row per appearance mode: it shows and edits the
palette currently in effect, listing only the palettes of that mode. `themes_for(mode)`
is the single ordering the dropdown paints and a click resolves, so a row index always
maps to the palette it painted. Retired editor themes still load from a saved
preference: dark keys fall back to Orbit, light keys to Orbit Light.

**The Primary-Action Role.** `accent` is a palette's *identity* hue — selection washes,
focus, tints, and accent-colored text derive from it. A primary button's fill is
`action` / `action_hover` / `action_pressed` with `action_fg` ink, a separate role so a
palette can key its buttons to its accent (Orbit Azure: blue on blue) or to a neutral step
(Orbit: light primary buttons with dark ink, the original ember-era look) without losing
the identity hue everywhere else. Both roles are covered by the AA floors in the theme
tests.

**Orbit Cobalt.** The graphite-and-blue dark palette shipped as "Orbit Minimal" and is
renamed for what it is: a hue. Minimal is a property of the interface, never of a
palette. The persisted key `orbit-cobalt` still accepts `orbit-minimal`, `codex`, and
`codex-dark`, so a saved preference survives the rename.

### Hierarchy
- **Title** (500, 15px): page titles in a 44px header row.
- **Body** (400, 13px): default UI text, list rows, table cells.
- **Body small** (400, 12–12.5px): secondary row text, chips, buttons.
- **Label** (500, 11px, +0.02em, uppercase): section labels in `text_3`, always paired
  with a count or meta on the right of the same row.
- **Number** (500, 20px): one per metric cell, no accent color.
- **Mono** (400, 12–13px): tokens, hashes, durations, exact figures.

### Named Rules
**The Size-Not-Weight Rule.** Sizes step 11 → 12 → 13 → 15 → 20; weights use only 400,
500, 600. No 700+, no letter-spacing tricks, no tracking below -0.01em.

**The Tabular Figure Rule.** Any column of numbers uses fixed-width rendering and is
right-aligned; compact forms in the main UI (`18.4M`), exact forms in tooltips and
detail tables (`18,421,842`).

## Layout

A three-part shell: a fixed sidebar column (200–320px, default 248px), the content
plane, and an optional right pane. The content plane is full-bleed with a centered
inner column (960px for reading surfaces, 1180px for data surfaces) and 20px gutters.

Vertical rhythm: 44px header rows with a bottom hairline, 20px between sections, 12px
inside a section's header-to-content gap, and 6–8px between dense rows. Long lists are
virtualized (`uniform_list`) and scroll inside the plane — the page itself never scrolls
as a whole when a table is present.

Responsive behavior is structural, never fluid type: grids drop columns (4 → 2), paired
columns stack into one, and the sidebar hides before the content squashes.

## Elevation & Depth

Flat by default. Depth comes from tonal steps (canvas → raised) and 1px hairlines. Only
three surfaces float, and each has one authored shadow from `Theme`:

### Shadow Vocabulary
- **`composer_shadow`** (1 tight contact layer): the composer box and the usage metric
  board — objects that sit on the page rather than in it.
- **`popover_shadow`** (contact + wide ambient): anchored menus, dropdowns, tooltips.
- **`card_shadow`** (compact contact + ambient): hover cards over dense lists.

### Named Rules
**The Border-Or-Shadow Rule.** A surface declares one: either a 1px `border`
hairline or a shadow. Never both, or it reads as a ghost card. (`border_strong`
is a focus/hover/selection edge, not a resting card edge.)

**The Rectangle-Clip Rule.** gpui 0.2.2 clips `overflow_hidden` to a rectangle,
not the border radius (`Style::overflow_mask` builds a `ContentMask` from
`bounds` alone), so a child background that reaches a rounded container's edge
paints square corners over it. Such a child must carry the matching radius
itself — `rounded_t` on the first, `rounded_b` on the last — or the container
must be a popover whose content is `deferred` (deferred draws bypass ancestor
masks). Applies to the transcript cards, the code-block title strip, and
list-board rows (`settings_group`, the MCP server rows).

## Shapes

Corners are soft but restrained: 6px for navigation and session rows and
compact controls (≤24px icon buttons), 8px for chips, action buttons,
inline inputs, and data-list rows, 12px for grouped surfaces and boards, 16px for
the composer, `full` only for status dots, avatar monograms, the send button,
and the toggle-switch track. Nothing else is a pill.

Borders are 1px at `theme.border`; `border_strong` (ink at ~14% alpha) is reserved for
focus and hover emphasis. Charts and graphs draw with 1.5px strokes; grid lines are 1px
at low-opacity hairline. No drop-shadowed text, no gradients except the one backdrop
fade on the new-task page and the raised-button fill (see Buttons).

### New-task backdrop

The new-task page is the one surface with a background treatment, and it has two forms
under one grammar — ordered dither, the same Bayer pass the photo setting uses:

- **No image configured:** a painted halftone field (`backdrop`) — an 8×8 Bayer screen of
  four dot sizes over a soft density field, densest in a band behind the composer and
  calmer — never empty — toward the window's edges through a floored elliptical
  vignette, so the screen runs edge to edge and its drift is visible to the frame. It is
  drawn in
  `text_3` at 55% (dark) / 62% (light), so it is paper texture, never a second content
  layer, and it works in all ten palettes without a per-palette asset.
- **The monogram is screened into that field.** Where the app mark sits — between the
  empty-state column and the composer — the same screen takes over its shape: a coverage
  mask sampled from the shipped `app-icon.png` (never redrawn by hand, so the backdrop
  cannot drift from the icon), glyphs holding a quiet density at rest while a soft band
  of light crosses them every twenty seconds, filling each letter with the largest dot in
  turn. The mark paints one step quieter than the grain around it (`MARK_INK`), and a
  layer too small to hold it drops it rather than stretching a wordmark into mush.
- **An image configured:** the dithered photo (`dither`), under the fade below.

Both are the page's floor, not an illustration: nothing on the page depends on them, and
the chat page has neither.

## Components

### Buttons
- **Shape:** 8px radius, 28px tall (chips) or 34px (the sidebar's primary action).
- **Primary:** `bg_raised` + 1px `border`, hover steps to `bg_hover` with a
  `border_strong` edge. One per screen region.
- **Raised:** every filled button (primary, accent, danger, chip) paints its fill
  through `RaisedExt::raised` — a top-lit gradient of the button's own color plus
  the short `button_shadow`. Segmented controls use `raised_flat` (gradient, no
  shadow). Ghost and disabled buttons stay flat; this is the one sanctioned
  exception to the no-gradient / hairline-only rules below.
- **Ghost:** no fill until hover (`bg_hover`); used for every icon action in a header.
- **Disabled:** `overlay` fill with `text_3` content; never opacity-faded color.

### Chips
- **Style:** 28px tall, 8px radius, `bg_raised` + hairline, 12px ink, optional leading
  Small (14px) icon and XSmall (12px) trailing chevron — `ButtonSize::icon_size`
  for the leading glyph, the chip-caret size for the chevron.
- **State:** open/selected = `active` fill + `active_fg` text; hover = `bg_hover`.

### Metric cells
- **Style:** a single bordered board (`bg_raised` + hairline + 12px radius) whose cells
  are divided by 1px hairlines — never a grid of separate floating cards.
- **Content order:** 11px uppercase label → 20px number → 11.5px secondary line
  (comparison or average). Missing data reads "Unavailable", never a fabricated `0`.

### Cards / Containers
- **Corner Style:** 12px.
- **Background:** `bg_composer`/`bg_raised`; nested cards are forbidden.
- **Border:** 1px `border`.
- **Internal Padding:** 12–14px; lists use full-bleed rows with 8px inner padding and a
  hover fill instead.

### Inputs / Fields
- **Style:** composer-style box — `bg_composer`, 1px hairline, 16px radius on the main
  composer, 8px on inline fields.
- **Focus:** the border steps to `border_strong`; the accent is not used for focus fills.
- **Placeholder:** `text_3` — always a real prompt ("Search sessions…"), never a label.
- **Tokens:** a leading `/command` and any `@file` mention paint inline in their token
  colors while typing; the caret, selection, and wrapping are unaffected (paint only).
- **Caret & selection:** a 2px accent caret that holds solid while the field is being
  edited and blinks at the platform cadence (~1s) once idle. The selection wash is the
  accent at 25%; double-click selects the character run, triple-click the line, and
  shift-click extends — matching native text fields.

### Navigation
- Sidebar rows: 28px tall, 6px radius, 14px icon (Small) in a fixed 20px slot, label in
  `text_2`/`text_3`; hover fills `bg_hover`; the active destination is marked by
  `active` fill with `active_fg`, never by accent color alone.
- A running session row leads with a small accent arc turning in place and its title
  carries a shimmer — an accent highlight band swept left-to-right across the ink on a 2s
  loop. This is the only per-row motion in the sidebar; a settled row is still.
- Page headers: 44px, back affordance first, title in 15px/500, actions right-aligned
  as 28px ghost controls.

### Tables
- Header row: 11px uppercase `text_3` labels, hairline underneath, sticky.
- Rows: 26px, hairline separators, hover fills `bg_hover`, selected fills `active`,
  numeric columns right-aligned in tabular figures, first column left-aligned.
- Clipped cell text always ends in `…`: the character budget is computed from the
  column's width rather than left to the renderer, so no cell is ever cut mid-glyph.
- Row-level actions are revealed on row hover, never as a permanent icon rail.
- Sort indicators are small chevrons in `text_3`; the sorted column's label steps to
  `text_2`.
- **Data tables and plots are drawn with GPUI's own elements.** The usage tables
  are built from `div` in `usage::table` and the timeline from `canvas` in
  `usage::chart`, each reading colors only from `theme::get(cx)` — the same
  surface as the rest of the page. There is no component layer and no second
  palette to synchronise.

**The Bridge Rule.** A table or chart reads every color from `theme::get(cx)`. If
it renders in a color this file does not list, the element is wrong — fix the
element, not the palette.

## Interaction System

Orbit is a native workbench. Visual hierarchy and interaction hierarchy must reinforce
the same model: navigation is quiet, the workspace is primary, and contextual tools
appear only when relevant.

### Interaction Hierarchy

Every interactive element belongs to one of five levels:

1. **Primary** — the main action for the current surface.
2. **Secondary** — supporting actions that remain visible when useful.
3. **Tertiary** — low-emphasis actions exposed through ghost controls or menus.
4. **Contextual** — actions revealed by selection, hover, right-click, or focused content.
5. **Destructive** — actions that can remove or irreversibly change user data.

Do not give two competing actions the same visual weight.

### Focus

Focus is a first-class state, not a stronger hover state.

- Pointer hover uses `bg_hover`.
- Keyboard focus uses `border_strong` or an equivalent non-color-only focus signal.
- Selection uses `active`.
- Focused selection must remain distinguishable from an unfocused selection.
- Focus must never depend on accent color alone.
- After dismissing a popover or modal, restore focus to the control that opened it.
- Opening a contextual panel must not unexpectedly steal focus from the active editor or composer.

### Selection

Selection means "the object currently being operated on"; focus means "the object currently
receiving keyboard input." They must remain visually distinct.

Selected rows use `active`. Hover uses `bg_hover`. A focused selected row adds the focus
treatment without changing its semantic color.

### Command System

The command palette is a first-class navigation surface.

- Default shortcut: `Cmd+K` on macOS, `Ctrl+K` elsewhere.
- Search commands, pages, sessions, files, and supported actions from one surface.
- Results are keyboard navigable.
- Commands expose shortcuts when one exists.
- Escape closes the palette and restores the previous focus target.
- Destructive commands require explicit confirmation.
- Commands must operate on the same underlying action handlers as buttons and menus;
  keyboard and pointer interactions must not create separate behavior paths.
- A chordless command is registered with `command(…)`, which puts it in the palette and
  in no keymap. That is the intended shape for a chip's keyboard path, not a gap.
- Every command draws an icon, and GPUI resolves it through the embedded asset source, so
  a path the source cannot find fails when the row is drawn. `every_command_icon_is_embedded`
  holds the whole registry against the assets that ship — add the missing icon rather than
  softening the reference.

### Context Menus

Context menus expose actions that are useful for the currently focused object.

- Prefer contextual actions over permanent icon rails.
- Menu order follows action importance, not implementation order.
- Destructive actions are visually separated.
- Menus use `menu_bg` / raised surface and `popover_shadow`.
- Right-click and keyboard context-menu invocation must expose the same actions.

## Workbench Layout

### Shell

The canonical Orbit shell is:

**Navigation → Workspace → Context**

- **Navigation**: sidebar, project/session navigation, global destinations.
- **Workspace**: the primary editor, transcript, table, diff, or task surface.
- **Context**: optional right or bottom pane for details, review, activity, metadata, or
  secondary controls.

The workspace owns the visual hierarchy. Navigation and context should not compete with it.

### Sidebar

The sidebar is quiet infrastructure.

- Default width: 248px.
- Supported range: 200–320px.
- Collapse/hide the sidebar before forcing the workspace to become cramped.
- Sidebar sections use whitespace and labels rather than excessive containers.
- A project group header carries a session count only when it is non-zero. A project
  with nothing in it already says so by having no rows under it.
- A session row's title line is its anchor and is always there; the first-message
  preview appears only when it says something the title does not
  (`preview_adds_information`). pi titles a session from that message, so the title is
  normally the same sentence cut short and the preview is the rest of it — a second line
  the eye reads and learns nothing from. Rows are therefore one or two lines tall, and the
  age rides whichever line is last.
- Active navigation uses `selection`, not accent color alone.
- Session state may use semantic indicators, but decorative animation is forbidden except
  for the currently running session treatment already defined above.
- Sidebar width and collapsed state should persist per workspace/window when practical.

### Status Row

The status row under the composer is a readout, not a toolbar.

- It carries live state only: where the workspace is, which branch it is on, and how much
  context is left. Anything that restates a control sitting a few pixels above it belongs
  to that control, not here.
- **Ahead/behind counts appear only when they are non-zero.** `↑0 ↓0` is a row of arrows
  that says nothing; a clean branch shows its name alone.
- **Send hints appear only while a run is in flight.** Idle they restate the send button —
  a hint the user already acted on is instruction, not information. Running they carry the
  one thing the button cannot say: which chord steers the live turn and which queues a
  follow-up.
- **The MCP chip appears only when a server needs attention.** A fully connected fleet is
  the steady state, and the steady state earns no pixels. Its label names the counts, so
  colour is never the only signal.
- **The "Work in" chip appears only in a linked worktree.** At the repository's main
  checkout the location is "Local" — the default — so the chip stays away. In a worktree it
  names a non-default location and is the way back. See `the_work_in_chip_is_absent_at_the_main_checkout`.
- Workspace and branch chips yield before the row can crowd; the context indicator holds
  the right edge.

### Context Panes

Context panes are contextual, not permanent dashboard columns.

- Open when the current task benefits from additional information.
- Preserve the workspace's primary reading/editing width.
- Allow resizing.
- Remember size during the current workspace lifecycle.
- Close with Escape where appropriate.
- Do not duplicate information already visible in the workspace.

### Resizable Splits

Resizable panels use a small, low-contrast hit target around a 1px divider.

- Divider is visually quiet at rest.
- Hover increases contrast.
- Active drag uses the focus/interaction treatment.
- Do not animate layout during manual resizing.
- Persist useful split positions where doing so does not create surprising layouts.

## Desktop Interaction Model

Orbit should behave like a native desktop application rather than a responsive website.

### Keyboard-first Behavior

Every primary workflow must be possible without a mouse.

Minimum expectations:

- Command palette.
- Session navigation.
- Project/file navigation.
- Search.
- Focus movement between major panes.
- Open/close contextual panels.
- Submit/abort/steer agent runs.
- File operations.
- Diff navigation.
- Menu dismissal with Escape.
- Standard text editing shortcuts.

Every control on the composer row is a `div` with a mouse handler, not a tab stop, so its
only routes are its chord and the command palette. A chip whose command has neither is a
setting a keyboard-only user cannot change at all — `composer_controls_are_reachable_without_a_pointer`
enforces this for the agent controls (model, thinking, workflow mode, access mode).
When a chord is not available, a palette-only command is the whole fix: `command()`
registers no keymap row, so nothing has to be invented to give a chip a keyboard path.

Do not invent custom shortcuts where standard macOS or Windows behavior already exists.

### Pointer Behavior

Pointer interaction should reveal information progressively.

- Hover reveals secondary actions.
- Selected objects retain their state after the pointer leaves.
- Tooltips explain unfamiliar icon-only actions.
- Never require hover to discover a destructive action.
- Avoid permanent action rails when row-level actions can remain contextual.

### Native Text Behavior

Text fields and editors should follow platform expectations:

- Standard selection behavior.
- Standard copy/paste/cut.
- Shift-based range extension.
- Double-click word selection.
- Triple-click line selection where supported.
- Platform-standard modifier keys.
- Correct focus and caret behavior.

## Motion System

Motion communicates state; it does not decorate the interface.

### Timing

- **Instant:** 0–80ms — focus, simple visual feedback.
- **Interaction:** 120–160ms — hover, selection, menu appearance.
- **Layout:** 160–220ms — panel open/close, disclosure, contextual transitions.
- **Streaming:** content updates are driven by incoming state and must not introduce
  unnecessary layout animation.

### Motion Rules

- **One sanctioned loop:** the new-task backdrop's dither field, whose screen drifts so
  dots cross their Bayer thresholds a few at a time (~2% of the cells a second), and whose
  light crosses the monogram on a twenty-second pass. It is the page's texture rather
  than an event, it is drawn behind everything, it never moves a glyph or a box, and
  Reduce Motion freezes it on the frame it was showing. Every other loop in the product
  remains the state it reports — the running-session shimmer, a spinner, a streaming
  transcript.
- No decorative looping animation beyond that one field.
- No parallax.
- No spring-heavy marketing motion.
- Do not animate large areas when a small state transition is sufficient.
- Do not animate text position during streaming.
- Respect reduced-motion preferences.
- Loading animation must communicate actual work.
- A transition should be interruptible by the user's next action.

## Agent Interaction States

Agent activity is a core product state and must be visually legible without relying on color.

### State Model

The UI should distinguish:

- **Idle** — no active run.
- **Thinking** — model is processing.
- **Streaming** — response content is arriving.
- **Tool running** — a tool call is executing.
- **Awaiting approval** — user action is required.
- **Awaiting input** — the agent has asked a question.
- **Completed** — run finished successfully.
- **Interrupted** — user stopped the run.
- **Failed** — run ended with an error.

Each state must have at least one non-color signal such as iconography, text, motion,
layout, or control availability.

### Streaming

Streaming should feel continuous without causing the interface to jump.

- Coalesce frequent updates.
- Preserve scroll position unless the user is already following the bottom.
- Never steal scroll position from a user who has manually scrolled upward.
- Keep the composer stable while output streams.
- Tool activity should update in place rather than repeatedly creating new cards.

### Tool Activity

Tool calls are operational information, not decorative chat bubbles.

- Group related activity when appropriate.
- Show tool name, meaningful status, and relevant output.
- Collapse verbose output by default.
- Make failure states immediately discoverable.
- Allow expansion without leaving the current session.
- Preserve a compact representation in long transcripts.
- An expanded command's input and output share **one recessed code plane**
  (`code_bg`, the same treatment a markdown code block takes), divided by a
  hairline. Sections read top-to-bottom — command, then output — the way a
  shell transcript does; they are not stacked cards, and each carries its
  label and copy control on one shared axis.

### Approval

Approval requests are high-priority contextual states.

- Clearly state what requires approval.
- Provide the available actions explicitly.
- Do not hide the approval action inside a generic menu.
- Keep the underlying session context visible.
- Restore focus to the session after the decision.

## Transcript and Conversation UX

The transcript is one workspace surface, not the definition of the application.

### Message Hierarchy

Separate:

- User intent.
- Agent response.
- Tool activity.
- System/session state.
- Approval/input requests.

Avoid making every item look like a chat card.

### Long Sessions

Long transcripts must remain scanable.

- Virtualize long lists.
- Preserve stable message anchors.
- Use compact metadata.
- Collapse verbose tool output.
- Provide in-transcript find.
- Do not repeatedly repaint unaffected content.
- Keep timestamps and secondary metadata visually subordinate.

### Composer

The composer is a primary work surface.

- Use the existing 16px radius and composer surface.
- Keep the input visually distinct from the transcript without making it look like a
  floating SaaS widget.
- Commands and file mentions remain content tokens.
- Primary submit/stop controls remain discoverable.
- Model, thinking effort, access mode, and workflow mode should be available without
  turning the composer into a control dashboard. Four chips is the ceiling at rest, and
  each one carries a rule for when it is not on the row at all:
  - **Access mode** shows its name only when it is *not* `AccessMode::default()` (Full
    access). At the default the chip keeps the glyph alone and names the mode on hover:
    the label is the warning, and the default is not a warning. The rule reads the mode
    and never the open picker, so opening a picker cannot resize the row.
  - **Thinking effort** is absent unless the active model reports a ladder with a choice
    in it. `can_choose_thinking` (`available_thinking_levels.len() > 1`) is the single
    predicate behind the chip, the ⌘T shortcut, and the palette entry, so the three can
    never disagree about whether the picker has options.
  - **Workflow mode** and **model** stay on the row in every state; both are the live
    state of the run.
  - **Steer** appears only while a run is in flight with something to send.
- Advanced controls belong behind contextual disclosure when they are not needed. A
  control that disappears keeps a keyboard or palette path — add that path before hiding
  the control, not after.

## Explorer and File UX

### Explorer

The file tree is a navigation instrument.

- Prefer indentation and whitespace over nested cards.
- Selected files use `active`.
- Git status uses semantic indicators plus text/icon where necessary.
- Hidden files remain controlled by an explicit setting.
- Directory expansion should be keyboard accessible.
- Quick-open should complement, not replace, the tree.

### File Editor

The editor is a primary workspace.

- Code uses the mono token family or user-selected code font.
- Tabs remain compact.
- Dirty state is visible without overpowering the filename.
- Save/conflict states are explicit.
- Binary, oversized, and non-UTF-8 states must be honest and actionable.
- Editor chrome should remain quieter than the code itself.

### Diff

Diffs prioritize comprehension.

- Added/removed content is differentiated by semantic treatment and non-color cues.
- File-level metadata remains compact.
- Actions are contextual.
- Line numbers and code remain the strongest visual anchors.
- Do not wrap every hunk in a card.

## Git and Review UX

Git surfaces should feel like workbench tools rather than dashboards.

- Branch, status, changed-file count, and review state use compact metadata.
- Changed files form a navigable list.

## Data Visualization

Charts are information surfaces, not decoration.

- Use one accent ramp for a single series.
- Multiple series use neutral differentiation or semantic colors only when the data
  requires categorical distinction.
- Never introduce arbitrary colors merely to make a chart visually richer.
- Grid lines remain subordinate.
- Tooltips provide exact values.
- Axes and units must be explicit.
- Missing data is represented as missing, never zero-filled without a documented reason.
- Hovering a point may reveal detail but must not permanently alter layout.

## Empty, Loading, and Error States

### Empty

An empty state explains:

1. What is empty.
2. Why it may be empty.
3. What the user can do next.

Avoid decorative illustrations in core workbench surfaces.

### Loading

Loading states represent actual asynchronous work.

- Prefer skeletons only where the final structure is known.
- Prefer progress/status text for agent and process work.
- Do not show indefinite spinners for operations whose state can be described.

### Error

Errors must be actionable.

- State what failed.
- Preserve relevant context.
- Explain the next available action.
- Avoid generic "Something went wrong" as the only message.
- Never fabricate recovery state.
- Distinguish connection/process errors from user-action errors.

## Accessibility

Accessibility is part of the component contract.

- Every interactive element must have an accessible name.
- Focus must remain visible.
- Do not encode meaning with color alone.
- Maintain sufficient contrast for text and controls.
- Hit targets must remain usable at compact density.
- Keyboard navigation must reach all primary functionality.
- Tooltips cannot be the only way to access essential information.
- Respect reduced motion.
- Dynamic agent state changes should be announced appropriately without flooding the
  accessibility tree.

## Component Contracts

Every reusable GPUI component should define:

```text
Component
├── anatomy
├── variants
├── states
├── keyboard behavior
├── pointer behavior
├── focus behavior
├── accessibility semantics
├── motion
└── data contract
```

A component is not considered complete when it only has a visual default.

### State Matrix

At minimum, interactive components should account for:

- default
- hover
- focus
- active/pressed
- selected
- disabled
- loading
- error
- destructive
- keyboard navigation

Only states that make semantic sense for a component should be implemented.

## Card Budget

Cards are reserved for conceptually singular or elevated objects.

Use cards for:

- composer
- metric board
- important grouped information
- floating/contextual surfaces

Prefer hairlines, whitespace, indentation, and hover states for:

- navigation
- transcripts
- tool activity
- file trees
- dense tables
- lists
- settings rows

Never build:

```text
card → card → row → card → button
```

If hierarchy can be communicated through spacing and typography, do not add another container.

## Design Tokens and Theme Architecture

All visual values must flow through the theme system.

- Never hardcode colors in components.
- Never hardcode semantic state colors where a theme role exists.
- Add a semantic role when a new visual meaning is required.
- Keep palette-specific values inside `Palette` / `Theme`.
- Components consume semantic roles rather than palette implementation details.
- User-selected fonts remain part of the supported theme/configuration system.
- New components must work across dark and light palettes before being considered complete.

### Token Naming

Prefer semantic names:

```text
background
surface
surface_secondary
surface_elevated
surface_hover
surface_active
border
border_subtle
text_primary
text_secondary
text_muted
text_disabled
accent
accent_hover
accent_pressed
accent_fg
accent_text
selection
selection_fg
focus_ring
success
warning
error
error_hover
info
```

Specialized roles (`code_bg`, `syn_*`, `overlay`, `border_strong`, `toggle_knob`)
exist alongside the vocabulary; they are read through the same `Theme`.

Avoid component-specific color names such as:

```text
blue_button
dark_card
orange_row
```

Semantic tokens allow the same component grammar to survive palette changes.

## Quality Bar

A new Orbit surface is ready when:

- Its hierarchy is understandable within a few seconds.
- The primary action is obvious without being visually loud.
- Hover, focus, selection, disabled, loading, and error states are defined.
- Keyboard interaction is supported.
- The surface works in both dark and light themes.
- No unnecessary cards, borders, gradients, or shadows were introduced.
- Content remains readable at Orbit's compact density.
- Long-running or streaming states do not cause layout instability.
- Color is not the sole carrier of meaning.
- All colors and dimensions come from the design system.
- The surface feels native to the existing workbench rather than like a separate mini-product.

## Design Non-Goals

Orbit should not become:

- a web dashboard translated into GPUI;
- a ChatGPT clone;
- a glassmorphism interface;
- a neon AI interface;
- a card-heavy SaaS admin panel;
- a permanently animated interface;
- an IDE clone with unnecessary chrome;
- a collection of disconnected page-specific design systems.

The goal is not maximum visual novelty.

The goal is a coherent native workbench whose interaction quality, information density,
performance, and visual restraint make long agent sessions feel natural.

## Do's and Don'ts

### Do:
- **Do** read every color, size, and radius from `theme::get(cx)`; add a role to
  `Palette` when a new semantic need appears so all ten palettes stay legible.
- **Do** use hairlines and whitespace to separate sections; reserve rounded bordered
  surfaces for objects that are conceptually singular (composer, metric board, popover).
- **Do** keep controls at 28px (compact) / 34px (primary) so a 900px-tall window shows
  15+ table rows.
- **Do** mark unavailable data in words ("Cost unavailable"): zero and unknown are
  different facts.
- **Do** pair color with a non-color signal (↑/↓, icon, label) for any state.

### Don't:
- **Don't** introduce a second accent hue; categorical series use one accent ramp
  (100% → 62% → 40% → 22% wash) plus neutral inks.
- **Don't** wrap every row in a card, nest a card inside a card, or use a card where a
  hairline-separated section reads better.
- **Don't** use display fonts, gradients, glass, glow, or emoji in the chrome.
- **Don't** animate decoration; motion is 120–200ms and only confirms a state change
  (hover, expand, selection, chart re-scale).
- **Don't** show a metric the underlying data cannot support — no invented latency,
  cost, or retry counts.
- **Don't** ship a chip that appears in every state "just in case". Give it a rule for
  when it is absent, and make that rule read the state the chip describes rather than a
  neighbouring control's open state.
- **Don't** give a control a permanent row for its own default. The access chip drops its
  label at Full access, the "Work in" chip is absent at the main checkout, and the MCP
  chip is absent while every server is connected — the default needs no announcement, and
  the row it frees is paid back on every frame.
