//! The usage model: one normalized record per billable agent action, plus the
//! interning tables every query joins against.
//!
//! # Where this data comes from
//!
//! Orbit never invents usage. Every number on the Usage page is derived from
//! pi's own session store (`~/.pi/agent/sessions/<workspace-slug>/*.jsonl`),
//! the same files the CLI writes and the sidebar lists. Two entry shapes carry
//! all of it:
//!
//! - `{"type":"message", "message":{"role":"assistant", "usage":{…}}}`
//!   → one [`UsageRecord`] (one model request).
//! - `{"type":"message", "message":{"role":"toolResult", …}}` matched to the
//!   `toolCall` block that produced it → one [`ToolRun`].
//!
//! # Accounting rules
//!
//! These definitions are the contract for every aggregate in
//! [`super::aggregate`]. They are stated once, here, and never re-derived in a
//! view.
//!
//! - **Request** — one assistant message that carries a `usage` object: one
//!   round trip to a model provider. A request that pi records with no tokens
//!   and no cost is not counted (nothing was billed, nothing to show).
//! - **Agent turn** — one user message: one prompt you sent. A turn contains
//!   one or more requests (a tool loop makes several).
//! - **Tool run** — one `toolCall` block, paired with its `toolResult`.
//! - **Bash execution** — a tool run whose tool is `bash`.
//! - **Total tokens** — pi's own `usage.totalTokens`, which equals
//!   `input + output + cacheRead + cacheWrite` on every record we have ever
//!   seen; the four-bucket sum is used when `totalTokens` is absent. Cache
//!   tokens are therefore *included* in the total, and the composition
//!   (input/output/cache read/cache write) sums to it exactly.
//! - **Reasoning tokens** — a *subset of* `output` when a provider reports
//!   them; never added to the total (that would double-count).
//! - **Cache hit rate** — `cache_read / (cache_read + input + cache_write)`,
//!   matching [`orbit_rpc::SessionUsage::cache_read_percent`]. `input` is
//!   the uncached prompt, so the denominator is every prompt token the
//!   model processed; a request with no prompt tokens has no rate.
//! - **Cost** — pi's own per-message `cost.total`. Providers without a pricing
//!   table report `0`, which is indistinguishable from "genuinely free", so a
//!   model counts as *priced* only if some request for it reported a positive
//!   cost. Cost aggregates report their coverage so a partial total never
//!   masquerades as a complete one.
//! - **Duration** — wall-clock generation time, derived from the session
//!   file's entry timestamps: `write_time(assistant) − write_time(previous
//!   entry)`, i.e. from the prompt (or tool result) being handed to the
//!   provider until the reply was recorded. Gaps over [`IDLE_GAP_MS`] are
//!   treated as idle/sleep and excluded rather than reported as latency.
//! - **Tool duration** — `write_time(toolResult) − write_time(toolCall)`.
//! - **Errors** — a request whose `stopReason` is `error`. `aborted` (the user
//!   stopped the run) is tallied separately; a tool failure is an error on the
//!   tool run, not on the request.
//! - **Retries** — *not* in the session files: pi's automatic retries are only
//!   visible as live RPC events, so the page reports retry data as
//!   unavailable rather than guessing from error sequences.
//! - **Workspace / project** — pi records one dimension, the session's `cwd`.
//!   The page calls it "workspace"; there is no separate project level in the
//!   data, so project-level analytics are workspace-level analytics.

use std::collections::HashMap;

/// A gap longer than this between a prompt and its reply is idle time (a
/// sleeping laptop, a walked-away session), not generation time.
pub const IDLE_GAP_MS: i64 = 30 * 60 * 1000;

/// Longest error message kept for the errors table. Provider messages are
/// already short; this bounds anything pathological without truncating the
/// usual case.
pub const ERROR_MESSAGE_MAX: usize = 160;

// ── time ───────────────────────────────────────────────────────────────────

/// The selectable ranges in the date control, in menu order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RangePreset {
    Today,
    Yesterday,
    Last7,
    Last14,
    Last30,
    ThisWeek,
    ThisMonth,
    PreviousMonth,
    Custom,
    All,
}

impl RangePreset {
    pub const ALL: [Self; 10] = [
        Self::Today,
        Self::Yesterday,
        Self::Last7,
        Self::Last14,
        Self::Last30,
        Self::ThisWeek,
        Self::ThisMonth,
        Self::PreviousMonth,
        Self::Custom,
        Self::All,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Today => tr!("usage.range_today"),
            Self::Yesterday => tr!("usage.range_yesterday"),
            Self::Last7 => tr!("usage.range_last7"),
            Self::Last14 => tr!("usage.range_last14"),
            Self::Last30 => tr!("usage.range_last30"),
            Self::ThisWeek => tr!("usage.range_this_week"),
            Self::ThisMonth => tr!("usage.range_this_month"),
            Self::PreviousMonth => tr!("usage.range_previous_month"),
            Self::Custom => tr!("usage.range_custom"),
            Self::All => tr!("usage.range_all_time"),
        }
    }

    /// Persisted key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Yesterday => "yesterday",
            Self::Last7 => "last7",
            Self::Last14 => "last14",
            Self::Last30 => "last30",
            Self::ThisWeek => "this-week",
            Self::ThisMonth => "this-month",
            Self::PreviousMonth => "previous-month",
            Self::Custom => "custom",
            Self::All => "all",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw.trim())
    }
}

/// Bucket size for the time series.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Granularity {
    Hour,
    Day,
    Week,
    Month,
}

impl Granularity {
    pub fn label(self) -> String {
        match self {
            Self::Hour => tr!("usage.granularity_hour"),
            Self::Day => tr!("usage.granularity_day"),
            Self::Week => tr!("usage.granularity_week"),
            Self::Month => tr!("usage.granularity_month"),
        }
    }

    /// Stable identifier for export and search — never localized.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

/// A half-open local-time window `[start_ms, end_ms)`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DateRange {
    pub preset: RangePreset,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl DateRange {
    /// Resolve a preset against "now" in local time. Deterministic for a given
    /// `now_ms`, which is what the tests pin.
    pub fn for_preset(preset: RangePreset, now_ms: i64) -> Self {
        if preset == RangePreset::Custom {
            // A custom range without bounds is "all time" until the user picks.
            return Self::for_preset(RangePreset::Last7, now_ms);
        }
        let today = local_day_start(now_ms);
        let day = 86_400_000i64;
        let (start_ms, end_ms) = match preset {
            RangePreset::Today => (today, now_ms),
            RangePreset::Yesterday => (today - day, today),
            RangePreset::Last7 => (today - 6 * day, now_ms),
            RangePreset::Last14 => (today - 13 * day, now_ms),
            RangePreset::Last30 => (today - 29 * day, now_ms),
            RangePreset::ThisWeek => (local_week_start(now_ms), now_ms),
            RangePreset::ThisMonth => (local_month_start(now_ms), now_ms),
            RangePreset::PreviousMonth => {
                let this_month = local_month_start(now_ms);
                (local_month_start(this_month - 1), this_month)
            }
            RangePreset::All => (0, now_ms),
            RangePreset::Custom => unreachable!("custom handled above"),
        };
        Self {
            preset,
            start_ms,
            end_ms: end_ms.max(start_ms),
        }
    }

    /// An explicit custom window, day-aligned at both ends (the picker hands
    /// us two dates and the end date is inclusive). Reversed input is
    /// normalised rather than rejected.
    pub fn custom(start_day_ms: i64, end_day_ms: i64) -> Self {
        let (first, last) = if start_day_ms <= end_day_ms {
            (start_day_ms, end_day_ms)
        } else {
            (end_day_ms, start_day_ms)
        };
        let start_ms = local_day_start(first);
        Self {
            preset: RangePreset::Custom,
            start_ms,
            end_ms: next_bucket(local_day_start(last), Granularity::Day),
        }
    }

    pub fn len_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }

    pub fn contains(&self, ts_ms: i64) -> bool {
        ts_ms >= self.start_ms && ts_ms < self.end_ms
    }

    /// The immediately preceding window of the same length — the comparison
    /// baseline for every delta on the page. `None` for "All time", which has
    /// no meaningful predecessor.
    pub fn previous(&self) -> Option<Self> {
        if self.preset == RangePreset::All {
            return None;
        }
        let len = self.len_ms();
        Some(Self {
            preset: RangePreset::Custom,
            start_ms: self.start_ms - len,
            end_ms: self.start_ms,
        })
    }

    /// Bucket size for this window. Chosen so the chart never draws more than
    /// ~70 points, and never labels 500 ticks.
    pub fn granularity(&self) -> Granularity {
        let days = self.len_ms() as f64 / 86_400_000.0;
        if days <= 2.5 {
            Granularity::Hour
        } else if days <= 70.0 {
            Granularity::Day
        } else if days <= 400.0 {
            Granularity::Week
        } else {
            Granularity::Month
        }
    }

    /// Short label for the range chip: `Today`, `Sep 4 – Sep 11`, `All time`.
    pub fn label(&self) -> String {
        if self.preset != RangePreset::Custom {
            return self.preset.label().to_string();
        }
        let Some(start) = local_datetime(self.start_ms) else {
            return "Custom".into();
        };
        // The window is half-open; show the last *included* day.
        let last_ms = (self.end_ms - 1).max(self.start_ms);
        let Some(end) = local_datetime(last_ms) else {
            return "Custom".into();
        };
        if start.date_naive() == end.date_naive() {
            return start.format("%b %-d, %Y").to_string();
        }
        if start.year() == end.year() {
            format!("{} – {}", start.format("%b %-d"), end.format("%b %-d, %Y"))
        } else {
            format!(
                "{} – {}",
                start.format("%b %-d, %Y"),
                end.format("%b %-d, %Y")
            )
        }
    }
}

/// A single time bucket selected on a chart: a temporary cross-filter that
/// narrows every panel on the page without changing the surrounding date
/// range. Clearing it must restore exactly what was there before, so it is
/// kept separate from [`DateRange`] rather than folded into it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TimeFocus {
    pub start_ms: i64,
    pub end_ms: i64,
    pub granularity: Granularity,
}

impl TimeFocus {
    pub fn contains(&self, ts_ms: i64) -> bool {
        ts_ms >= self.start_ms && ts_ms < self.end_ms
    }

    /// The label shown on the active-filter chip (`Sep 12, 14:00`).
    pub fn label(&self) -> String {
        stamp_label(self.start_ms, self.granularity)
    }
}

// ── records ────────────────────────────────────────────────────────────────

/// The four token buckets pi reports, in its own accounting. `total` is pi's
/// `totalTokens` (the four-bucket sum when absent).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

impl TokenCounts {
    pub fn from_buckets(
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        total: Option<u64>,
    ) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write,
            total: total.unwrap_or(input + output + cache_read + cache_write),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    pub fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total += other.total;
    }

    /// Prompt tokens this request had to process: uncached input, what the
    /// provider served from its cache, and what it wrote to the cache. This
    /// is what "context consumed" means on the page — distinct from tokens
    /// *generated* (output).
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    /// `cache_read / (cache_read + input + cache_write)` as a percentage.
    /// `None` when the rate is undefined rather than zero: no prompt tokens
    /// at all, or a provider that reported no cache traffic whatsoever
    /// (§6/§25). Same formula as `SessionUsage::cache_read_percent`.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let prompt = self.prompt();
        (prompt > 0 && (self.cache_read > 0 || self.cache_write > 0))
            .then(|| self.cache_read as f64 / prompt as f64 * 100.0)
    }
}

/// How a request ended, from pi's `stopReason`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The model finished its answer.
    Stop,
    /// The model asked for tools; another request follows in the same turn.
    ToolUse,
    /// The model hit its output limit.
    Length,
    /// The provider returned an error.
    Error,
    /// The user stopped the run.
    Aborted,
}

impl Outcome {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("toolUse") => Self::ToolUse,
            Some("length") => Self::Length,
            Some("error") => Self::Error,
            Some("aborted") => Self::Aborted,
            _ => Self::Stop,
        }
    }

    pub fn is_error(self) -> bool {
        self == Self::Error
    }

    pub fn is_aborted(self) -> bool {
        self == Self::Aborted
    }
}

/// One model request.
#[derive(Clone, Copy, Debug)]
pub struct UsageRecord {
    /// When pi wrote the assistant message — i.e. when generation finished.
    pub ts_ms: i64,
    pub session: u16,
    pub model: u16,
    pub tokens: TokenCounts,
    /// Provider-reported reasoning tokens (a subset of `output`).
    pub reasoning: Option<u64>,
    /// pi's computed cost, when the cost object was present.
    pub cost_usd: Option<f64>,
    /// Derived generation time in milliseconds.
    pub duration_ms: Option<u32>,
    pub outcome: Outcome,
}

/// One tool invocation, paired with its result when pi recorded one.
#[derive(Clone, Copy, Debug)]
pub struct ToolRun {
    /// When the call was written (start of the run).
    pub ts_ms: i64,
    pub session: u16,
    pub model: u16,
    pub tool: u16,
    pub duration_ms: Option<u32>,
    /// `false` when the tool result carried `isError`.
    pub ok: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    /// A request failed at the provider.
    Provider,
    /// A tool returned an error result.
    Tool,
}

impl ErrorKind {
    pub fn label(self) -> String {
        match self {
            Self::Provider => tr!("usage.error_kind_provider"),
            Self::Tool => tr!("usage.error_kind_tool"),
        }
    }

    /// Stable identifier for sorting and search — never localized.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Tool => "tool",
        }
    }
}

/// One row of the errors table. Messages are provider text, trimmed — tool
/// output is never stored (it can contain anything the command printed).
#[derive(Clone, PartialEq, Debug)]
pub struct ErrorRow {
    pub ts_ms: i64,
    pub session: u16,
    pub model: u16,
    pub kind: ErrorKind,
    pub message: String,
}

// ── interning tables ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SessionEntry {
    pub id: String,
    /// First user message, as in the sidebar. Empty when the session has none.
    pub title: String,
    pub workspace: u16,
    pub started_ms: i64,
    pub ended_ms: i64,
    /// Times of the user messages in this session (one per agent turn).
    pub turns: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceEntry {
    /// Absolute path pi recorded as the session `cwd`.
    pub path: String,
    /// Basename — the label the sidebar groups by.
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub provider: u16,
    /// Provider-scoped model id (`glm-5.2:cloud`).
    pub id: String,
    /// Display name: the id with the provider's namespace stripped.
    pub label: String,
    /// Some request of this model reported a positive cost, so its price table
    /// is known and a `$0.00` really means free.
    pub priced: bool,
}

#[derive(Clone, Debug)]
pub struct ProviderEntry {
    pub id: String,
    pub label: String,
}

/// Coarse family for a tool, used to group the activity breakdown. The page
/// always shows the real tool names; this only supplies the group summary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ToolClass {
    Terminal,
    Read,
    Edit,
    Search,
    Web,
    Plan,
    Ask,
    Other,
}

impl ToolClass {
    pub const ALL: [Self; 8] = [
        Self::Terminal,
        Self::Read,
        Self::Edit,
        Self::Search,
        Self::Web,
        Self::Plan,
        Self::Ask,
        Self::Other,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Terminal => tr!("usage.class_terminal"),
            Self::Read => tr!("usage.class_read"),
            Self::Edit => tr!("usage.class_edit"),
            Self::Search => tr!("usage.class_search"),
            Self::Web => tr!("usage.class_web"),
            Self::Plan => tr!("usage.class_plan"),
            Self::Ask => tr!("usage.class_ask"),
            Self::Other => tr!("usage.class_other"),
        }
    }

    /// Stable identifier for search and export — never localized.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Search => "search",
            Self::Web => "web",
            Self::Plan => "plan",
            Self::Ask => "ask",
            Self::Other => "other",
        }
    }

    /// Map a pi tool name onto its family. Unknown tools land in `Other`
    /// rather than being hidden.
    pub fn of(tool: &str) -> Self {
        match tool {
            "bash" => Self::Terminal,
            "read" => Self::Read,
            "edit" | "write" => Self::Edit,
            "grep" | "find" | "ls" => Self::Search,
            "web_fetch" | "web_search" => Self::Web,
            "todo" | "plan_complete" | "artifact" => Self::Plan,
            "ask_user_question" | "ask_question" | "ask_user" => Self::Ask,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolEntry {
    pub id: String,
    pub label: String,
    pub class: ToolClass,
}

// ── the index ──────────────────────────────────────────────────────────────

/// Everything the analytics layer reads, built once per scan.
#[derive(Default)]
pub struct UsageIndex {
    pub sessions: Vec<SessionEntry>,
    pub workspaces: Vec<WorkspaceEntry>,
    pub models: Vec<ModelEntry>,
    pub providers: Vec<ProviderEntry>,
    pub tools: Vec<ToolEntry>,
    pub requests: Vec<UsageRecord>,
    pub tool_runs: Vec<ToolRun>,
    pub errors: Vec<ErrorRow>,
    /// When the scan that produced this index finished.
    pub scanned_at_ms: i64,
    /// Session files read.
    pub files: usize,
    /// Session files that could not be parsed (reported, never hidden).
    pub unreadable_files: usize,
}

impl UsageIndex {
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.tool_runs.is_empty()
    }

    pub fn session(&self, ix: u16) -> &SessionEntry {
        &self.sessions[ix as usize]
    }

    /// Fallible lookups for ids that arrive from *outside* the index — a
    /// filter set before a rescan, for instance. A session file the user
    /// deleted can leave a scope pointing at nothing, and that must degrade to
    /// "no match" rather than a panic.
    pub fn try_session(&self, ix: u16) -> Option<&SessionEntry> {
        self.sessions.get(ix as usize)
    }

    pub fn model(&self, ix: u16) -> &ModelEntry {
        &self.models[ix as usize]
    }

    pub fn try_model(&self, ix: u16) -> Option<&ModelEntry> {
        self.models.get(ix as usize)
    }

    pub fn provider_of(&self, model: u16) -> &ProviderEntry {
        &self.providers[self.model(model).provider as usize]
    }

    pub fn workspace_of_session(&self, session: u16) -> &WorkspaceEntry {
        &self.workspaces[self.session(session).workspace as usize]
    }

    pub fn tool(&self, ix: u16) -> &ToolEntry {
        &self.tools[ix as usize]
    }

    /// `provider / model` for a request or tool row.
    pub fn model_pair(&self, model: u16) -> (String, String) {
        let entry = self.model(model);
        (self.provider_of(model).label.clone(), entry.label.clone())
    }

    /// Whether any request in the index predates `ts_ms` — the "do we have
    /// history to compare against" test.
    pub fn has_data_before(&self, ts_ms: i64) -> bool {
        self.requests.iter().any(|r| r.ts_ms < ts_ms)
    }

    /// The oldest and newest request timestamps, for the empty/partial states.
    pub fn span(&self) -> Option<(i64, i64)> {
        let min = self.requests.iter().map(|r| r.ts_ms).min()?;
        let max = self.requests.iter().map(|r| r.ts_ms).max()?;
        Some((min, max))
    }
}

// ── filters ────────────────────────────────────────────────────────────────

/// The one shared filter state. Every chart, table, and KPI on the page reads
/// through this; no section keeps its own filters.
#[derive(Clone, PartialEq, Debug)]
pub struct UsageFilter {
    pub range: DateRange,
    /// Empty means "all" for every multi-select.
    pub workspaces: Vec<u16>,
    pub providers: Vec<u16>,
    pub models: Vec<u16>,
    /// Drill-down scope: a single session.
    pub session: Option<u16>,
    /// A single time bucket selected from a chart (drill-down).
    pub focus: Option<TimeFocus>,
    /// Show only failed requests (toggled from the Errors metric).
    pub errors_only: bool,
    /// Show only requests served partly from cache.
    pub cached_only: bool,
}

impl UsageFilter {
    pub fn new(range: DateRange) -> Self {
        Self {
            range,
            workspaces: Vec::new(),
            providers: Vec::new(),
            models: Vec::new(),
            session: None,
            focus: None,
            errors_only: false,
            cached_only: false,
        }
    }

    /// True when anything beyond the date range is narrowing the view.
    pub fn has_narrowing(&self) -> bool {
        !self.workspaces.is_empty()
            || !self.providers.is_empty()
            || !self.models.is_empty()
            || self.session.is_some()
            || self.focus.is_some()
            || self.errors_only
            || self.cached_only
    }

    /// A filter that keeps only the date range (used by "Clear filters").
    pub fn cleared(&self) -> Self {
        Self::new(self.range.clone())
    }

    /// The model ids the provider/model selects resolve to: a model select
    /// implies its provider, and a provider select includes all of its models.
    pub fn matches_model(&self, index: &UsageIndex, model: u16) -> bool {
        if !self.models.is_empty() && !self.models.contains(&model) {
            return false;
        }
        if !self.providers.is_empty() {
            let Some(entry) = index.try_model(model) else {
                return false;
            };
            if !self.providers.contains(&entry.provider) {
                return false;
            }
        }
        true
    }

    /// Session + workspace + provider + model scope, shared by requests and
    /// tool runs. The date range is checked separately by each caller so the
    /// two entry kinds can use their own timestamps.
    fn matches_scope(&self, index: &UsageIndex, session: u16, model: u16) -> bool {
        if let Some(only) = self.session {
            if session != only {
                return false;
            }
        }
        if !self.workspaces.is_empty() {
            let Some(workspace) = index.try_session(session).map(|entry| entry.workspace) else {
                return false;
            };
            if !self.workspaces.contains(&workspace) {
                return false;
            }
        }
        self.matches_model(index, model)
    }

    /// The time dimension, shared by requests, tool runs, turns, and errors:
    /// the date range, then any chart-selected focus bucket.
    pub fn matches_time(&self, ts_ms: i64) -> bool {
        if !self.range.contains(ts_ms) {
            return false;
        }
        self.focus.is_none_or(|focus| focus.contains(ts_ms))
    }

    pub fn matches_request(&self, index: &UsageIndex, r: &UsageRecord) -> bool {
        if !self.matches_time(r.ts_ms) {
            return false;
        }
        if self.errors_only && !r.outcome.is_error() {
            return false;
        }
        if self.cached_only && r.tokens.cache_read == 0 {
            return false;
        }
        self.matches_scope(index, r.session, r.model)
    }

    /// The calendar's match: the same scope and errors/cache tests as
    /// [`Self::matches_request`], but against a caller-supplied window instead
    /// of the date range. The activity calendar is a fixed trailing year, so it
    /// does not read the range — and never the focus bucket, which lives inside
    /// the range.
    pub fn matches_request_window(
        &self,
        index: &UsageIndex,
        r: &UsageRecord,
        window: &DateRange,
    ) -> bool {
        if !window.contains(r.ts_ms) {
            return false;
        }
        if self.errors_only && !r.outcome.is_error() {
            return false;
        }
        if self.cached_only && r.tokens.cache_read == 0 {
            return false;
        }
        self.matches_scope(index, r.session, r.model)
    }

    pub fn matches_tool(&self, index: &UsageIndex, t: &ToolRun) -> bool {
        if !self.matches_time(t.ts_ms) {
            return false;
        }
        if self.errors_only {
            return false; // "errors only" narrows to failed *requests*
        }
        if self.cached_only {
            return false; // tools carry no cache semantics
        }
        self.matches_scope(index, t.session, t.model)
    }

    /// Turns are prompts, so only the dimensions a prompt actually has apply:
    /// date, session, and workspace.
    pub fn matches_turn(&self, index: &UsageIndex, session: u16, ts_ms: i64) -> bool {
        if !self.matches_time(ts_ms) || self.errors_only || self.cached_only {
            return false;
        }
        if let Some(only) = self.session {
            if session != only {
                return false;
            }
        }
        if !self.workspaces.is_empty()
            && !self.workspaces.contains(&index.session(session).workspace)
        {
            return false;
        }
        true
    }
}

// ── time helpers (local zone, cached) ──────────────────────────────────────

use chrono::{Datelike, Local, TimeZone, Timelike};

pub fn local_datetime(ms: i64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// Midnight at the start of the local day containing `ms`.
pub fn local_day_start(ms: i64) -> i64 {
    let Some(dt) = local_datetime(ms) else {
        return ms;
    };
    let naive = dt.date_naive().and_hms_opt(0, 0, 0);
    match naive.and_then(|n| Local.from_local_datetime(&n).single()) {
        Some(start) => start.timestamp_millis(),
        None => ms,
    }
}

/// Monday 00:00 local for the ISO week containing `ms`.
pub fn local_week_start(ms: i64) -> i64 {
    let day = local_day_start(ms);
    let Some(dt) = local_datetime(day) else {
        return day;
    };
    let offset = dt.weekday().num_days_from_monday() as i64;
    local_day_start(day - offset * 86_400_000)
}

/// The 1st at 00:00 local for the month containing `ms`.
pub fn local_month_start(ms: i64) -> i64 {
    let Some(dt) = local_datetime(ms) else {
        return ms;
    };
    let Some(naive) = dt
        .date_naive()
        .with_day(1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
    else {
        return ms;
    };
    match Local.from_local_datetime(&naive).single() {
        Some(start) => start.timestamp_millis(),
        None => ms,
    }
}

/// The local start of the bucket containing `ms` at `granularity`.
pub fn bucket_start(ms: i64, granularity: Granularity) -> i64 {
    match granularity {
        Granularity::Hour => {
            let Some(dt) = local_datetime(ms) else {
                return ms;
            };
            let naive = dt
                .date_naive()
                .and_hms_opt(dt.hour(), 0, 0)
                .unwrap_or_else(|| dt.naive_local());
            Local
                .from_local_datetime(&naive)
                .single()
                .map(|d| d.timestamp_millis())
                .unwrap_or(ms)
        }
        Granularity::Day => local_day_start(ms),
        Granularity::Week => local_week_start(ms),
        Granularity::Month => local_month_start(ms),
    }
}

/// The start of the bucket after the one containing `start_ms`.
pub fn next_bucket(start_ms: i64, granularity: Granularity) -> i64 {
    match granularity {
        Granularity::Hour => start_ms + 3_600_000,
        Granularity::Day => {
            // Step by the local day length so DST days stay one bucket long.
            let Some(dt) = local_datetime(start_ms) else {
                return start_ms + 86_400_000;
            };
            let naive = (dt.date_naive() + chrono::Days::new(1)).and_hms_opt(0, 0, 0);
            match naive.and_then(|n| Local.from_local_datetime(&n).single()) {
                Some(next) => next.timestamp_millis(),
                None => start_ms + 86_400_000,
            }
        }
        Granularity::Week => {
            // Land on the same weekday of the following week, then snap to
            // that week's Monday (one call, and DST-safe via the day helpers).
            let mid = local_day_start(start_ms + 8 * 86_400_000);
            local_week_start(mid)
        }
        Granularity::Month => {
            let Some(dt) = local_datetime(start_ms) else {
                return start_ms + 30 * 86_400_000;
            };
            let (year, month) = if dt.month() == 12 {
                (dt.year() + 1, 1)
            } else {
                (dt.year(), dt.month() + 1)
            };
            match chrono::NaiveDate::from_ymd_opt(year, month, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .and_then(|n| Local.from_local_datetime(&n).single())
            {
                Some(next) => next.timestamp_millis(),
                None => start_ms + 30 * 86_400_000,
            }
        }
    }
}

/// Bucket label for the chart axis and table (`09:00`, `Sep 4`, `Wk of Sep 4`,
/// `Sep 2026`).
pub fn bucket_label(start_ms: i64, granularity: Granularity) -> String {
    let Some(dt) = local_datetime(start_ms) else {
        return String::new();
    };
    let now = Local::now();
    match granularity {
        Granularity::Hour => dt.format("%H:%M").to_string(),
        Granularity::Day => {
            if dt.year() == now.year() {
                dt.format("%b %-d").to_string()
            } else {
                dt.format("%b %-d, %Y").to_string()
            }
        }
        Granularity::Week => dt.format("Wk of %b %-d").to_string(),
        Granularity::Month => {
            if dt.year() == now.year() {
                dt.format("%b").to_string()
            } else {
                dt.format("%b %Y").to_string()
            }
        }
    }
}

/// Full timestamp for tooltips and tables (`Sep 11, 14:00`).
pub fn stamp_label(ms: i64, granularity: Granularity) -> String {
    let Some(dt) = local_datetime(ms) else {
        return String::new();
    };
    match granularity {
        Granularity::Hour => dt.format("%b %-d, %H:%M").to_string(),
        Granularity::Day => dt.format("%b %-d, %Y").to_string(),
        Granularity::Week => tr!("usage.week_of", date = dt.format("%b %-d, %Y").to_string()),
        Granularity::Month => dt.format("%B %Y").to_string(),
    }
}

/// Interner for the string-keyed dimensions. Small and local; the index is
/// rebuilt from scratch on every scan, so nothing outlives one build.
#[derive(Default)]
pub struct Interner {
    map: HashMap<String, u16>,
    keys: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, key: &str) -> u16 {
        if let Some(ix) = self.map.get(key) {
            return *ix;
        }
        let ix = self.keys.len() as u16;
        self.keys.push(key.to_string());
        self.map.insert(key.to_string(), ix);
        ix
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// The key behind an id (the index keeps insertion order, so this is a
    /// direct lookup).
    pub fn key(&self, ix: u16) -> String {
        self.keys.get(ix as usize).cloned().unwrap_or_default()
    }
}

/// Provider id → its display label. Provider ids are already the names pi
/// reports (`anthropic`, `github-copilot`), so the label only title-cases them.
pub fn provider_label(id: &str) -> String {
    match id {
        "" => "unknown".into(),
        "chatgpt" => "ChatGPT".into(),
        other => {
            let mut chars = other.replace(['_', '-'], " ").chars().collect::<Vec<_>>();
            if let Some(first) = chars.first_mut() {
                first.make_ascii_uppercase();
            }
            chars.into_iter().collect()
        }
    }
}

/// Model id → display label: drop a provider-style namespace prefix
/// (`cline-pass/kimi-k3` → `kimi-k3`) and keep pi's own suffix (`:cloud`).
pub fn model_label(id: &str) -> String {
    let trimmed = match id.split_once('/') {
        Some((_, rest)) if !rest.is_empty() => rest,
        _ => id,
    };
    trimmed.to_string()
}
