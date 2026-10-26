//! Aggregation: raw records in, page-ready analytics out.
//!
//! One [`UsageSnapshot`] is computed per (index, filter) pair and cached by
//! [`super::page::UsagePage`]; the GPUI layer never iterates a record. Every
//! formula lives here and nowhere else, so two panels can never disagree about
//! what "cache hit rate" or "average latency" means.
//!
//! Determinism: given the same index and filter, a snapshot is byte-identical
//! — groupings are ordered by value then label, and never by hash iteration
//! order.

use std::collections::HashMap;

use super::model::*;

// ── totals ─────────────────────────────────────────────────────────────────

/// Running totals over a filtered set of requests.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Totals {
    pub requests: u64,
    pub tokens: TokenCounts,
    /// Reasoning tokens, where reported (a subset of `output`).
    pub reasoning: u64,
    /// Requests that reported reasoning tokens at all.
    pub reasoning_reported: u64,
    pub cost_usd: f64,
    /// Requests whose model has a known price table.
    pub priced_requests: u64,
    pub duration_ms: u64,
    pub duration_samples: u64,
    /// Largest prompt (uncached input + cache read) seen.
    pub peak_prompt: u64,
    pub errors: u64,
    pub aborted: u64,
    /// Requests that asked for at least one tool.
    pub tool_use: u64,
}

impl Totals {
    pub fn add(&mut self, other: &Totals) {
        self.requests += other.requests;
        self.tokens.add(&other.tokens);
        self.reasoning += other.reasoning;
        self.reasoning_reported += other.reasoning_reported;
        self.cost_usd += other.cost_usd;
        self.priced_requests += other.priced_requests;
        self.duration_ms += other.duration_ms;
        self.duration_samples += other.duration_samples;
        self.peak_prompt = self.peak_prompt.max(other.peak_prompt);
        self.errors += other.errors;
        self.aborted += other.aborted;
        self.tool_use += other.tool_use;
    }

    /// Average generation time in milliseconds, over requests that had a
    /// measurable duration. `None` when nothing was measurable.
    pub fn avg_duration_ms(&self) -> Option<f64> {
        (self.duration_samples > 0).then(|| self.duration_ms as f64 / self.duration_samples as f64)
    }

    /// Average tokens per request.
    pub fn tokens_per_request(&self) -> Option<f64> {
        (self.requests > 0).then(|| self.tokens.total as f64 / self.requests as f64)
    }

    /// Average prompt size (input + cache read) per request.
    pub fn avg_prompt(&self) -> Option<f64> {
        (self.requests > 0).then(|| self.tokens.prompt() as f64 / self.requests as f64)
    }

    pub fn output_input_ratio(&self) -> Option<f64> {
        (self.tokens.input > 0).then(|| self.tokens.output as f64 / self.tokens.input as f64)
    }

    pub fn cache_hit_rate(&self) -> Option<f64> {
        self.tokens.cache_hit_rate()
    }

    /// Share of requests whose model has a price table, 0..=1.
    pub fn cost_coverage(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.priced_requests as f64 / self.requests as f64
        }
    }

    /// Cost per request, over priced requests only.
    pub fn cost_per_request(&self) -> Option<f64> {
        (self.priced_requests > 0).then(|| self.cost_usd / self.priced_requests as f64)
    }

    /// Cost per million tokens, over priced requests only.
    pub fn cost_per_mtok(&self) -> Option<f64> {
        let tokens: u64 = self.tokens.total;
        (tokens > 0 && self.priced_requests > 0)
            .then(|| self.cost_usd / (tokens as f64 / 1_000_000.0))
    }

    pub fn error_rate(&self) -> Option<f64> {
        (self.requests > 0).then(|| self.errors as f64 / self.requests as f64 * 100.0)
    }
}

impl UsageIndex {
    fn add_request(&self, totals: &mut Totals, record: &UsageRecord) {
        totals.requests += 1;
        totals.tokens.add(&record.tokens);
        if let Some(reasoning) = record.reasoning {
            totals.reasoning += reasoning;
            totals.reasoning_reported += 1;
        }
        if let Some(cost) = record.cost_usd {
            totals.cost_usd += cost;
        }
        if self.model(record.model).priced {
            totals.priced_requests += 1;
        }
        if let Some(duration) = record.duration_ms {
            totals.duration_ms += u64::from(duration);
            totals.duration_samples += 1;
        }
        totals.peak_prompt = totals.peak_prompt.max(record.tokens.prompt());
        if record.outcome.is_error() {
            totals.errors += 1;
        }
        if record.outcome.is_aborted() {
            totals.aborted += 1;
        }
    }
}

// ── summary ────────────────────────────────────────────────────────────────

/// The KPI board's numbers, plus the counts the secondary readout shows.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct UsageSummary {
    pub totals: Totals,
    /// Prompts sent in the window (one per agent turn).
    pub turns: u64,
    /// Distinct sessions with at least one request in the window.
    pub sessions: u64,
    /// Distinct models / providers / workspaces seen.
    pub models: u64,
    pub providers: u64,
    pub workspaces: u64,
    pub tool_runs: u64,
    pub bash_runs: u64,
    pub tool_errors: u64,
}

impl UsageSummary {
    pub fn tool_error_rate(&self) -> Option<f64> {
        (self.tool_runs > 0).then(|| self.tool_errors as f64 / self.tool_runs as f64 * 100.0)
    }
}

// ── comparison ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up,
    Down,
    Flat,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MetricDelta {
    /// Percent change against the previous period; `None` when there is no
    /// baseline to divide by.
    pub pct: Option<f64>,
    pub direction: Direction,
    /// The comparison is impossible (no history in the previous window).
    pub unavailable: bool,
}

impl MetricDelta {
    /// Compare two values. `previous == 0` has no percentage; the direction
    /// still reports growth from nothing.
    pub fn between(current: f64, previous: f64) -> Self {
        let direction = if (current - previous).abs() < f64::EPSILON {
            Direction::Flat
        } else if current > previous {
            Direction::Up
        } else {
            Direction::Down
        };
        let pct = (previous.abs() > f64::EPSILON).then(|| (current - previous) / previous * 100.0);
        Self {
            pct,
            direction,
            unavailable: false,
        }
    }

    /// No history to compare against (§14: don't show a comparison when there
    /// is insufficient data).
    pub fn unavailable() -> Self {
        Self {
            pct: None,
            direction: Direction::Flat,
            unavailable: true,
        }
    }
}

// ── time series ────────────────────────────────────────────────────────────

/// What the primary chart plots. One metric at a time, switchable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChartMetric {
    Tokens,
    Requests,
    Input,
    Output,
    Cache,
    Cost,
    Latency,
    Errors,
}

impl ChartMetric {
    pub const ALL: [Self; 8] = [
        Self::Tokens,
        Self::Requests,
        Self::Input,
        Self::Output,
        Self::Cache,
        Self::Cost,
        Self::Latency,
        Self::Errors,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Tokens => tr!("usage.metric_tokens"),
            Self::Requests => tr!("usage.metric_requests"),
            Self::Input => tr!("usage.metric_input"),
            Self::Output => tr!("usage.metric_output"),
            Self::Cache => tr!("usage.metric_cache"),
            Self::Cost => tr!("usage.metric_cost"),
            Self::Latency => tr!("usage.metric_latency"),
            Self::Errors => tr!("usage.metric_errors"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Requests => "requests",
            Self::Input => "input",
            Self::Output => "output",
            Self::Cache => "cache",
            Self::Cost => "cost",
            Self::Latency => "latency",
            Self::Errors => "errors",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == raw.trim())
    }

    /// Whether the metric can be plotted at all for this data. A metric with
    /// no source data (cost without prices, latency without durations) is not
    /// offered rather than drawn as a flat zero line.
    pub fn available(self, summary: &UsageSummary) -> bool {
        match self {
            Self::Cost => summary.totals.priced_requests > 0,
            Self::Latency => summary.totals.duration_samples > 0,
            Self::Tokens | Self::Cache => summary.totals.tokens.total > 0,
            _ => true,
        }
    }

    /// The value plotted for one bucket.
    pub fn value(self, totals: &Totals) -> f64 {
        match self {
            Self::Tokens => totals.tokens.total as f64,
            Self::Requests => totals.requests as f64,
            Self::Input => totals.tokens.input as f64,
            Self::Output => totals.tokens.output as f64,
            Self::Cache => (totals.tokens.cache_read + totals.tokens.cache_write) as f64,
            Self::Cost => totals.cost_usd,
            Self::Latency => totals.avg_duration_ms().unwrap_or(0.0),
            Self::Errors => totals.errors as f64,
        }
    }

    /// Value for the whole window (totals, not per-bucket).
    pub fn total(self, totals: &Totals) -> f64 {
        match self {
            Self::Latency => totals.avg_duration_ms().unwrap_or(0.0),
            other => other.value(totals),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct SeriesPoint {
    pub start_ms: i64,
    pub label: String,
    /// Full timestamp for the tooltip header.
    pub stamp: String,
    pub totals: Totals,
    /// Response-time distribution for this bucket, so the latency chart can
    /// switch between average and percentiles without re-reading records.
    pub latency: LatencyStats,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TimeSeries {
    pub granularity: Granularity,
    pub points: Vec<SeriesPoint>,
}

// ── breakdowns ─────────────────────────────────────────────────────────────

/// One ranked row in a breakdown: a model, provider, or workspace.
#[derive(Clone, PartialEq, Debug)]
pub struct GroupRow {
    /// Index into the matching `UsageIndex` table (model / provider /
    /// workspace / session).
    pub id: u16,
    pub label: String,
    /// Secondary line: the provider for a model, the path for a workspace.
    pub sub: Option<String>,
    pub totals: Totals,
    /// Share of the breakdown's total tokens (requests when no tokens exist).
    pub share: f64,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Breakdown {
    pub rows: Vec<GroupRow>,
    pub totals: Totals,
}

impl Breakdown {
    /// Compute each row's share of the breakdown's total. Ordering is the
    /// caller's job ([`breakdown`] sorts with a unique final tiebreak so the
    /// result never depends on hash iteration order).
    fn finish(&mut self) {
        let metric = if self.totals.tokens.total > 0 {
            self.totals.tokens.total as f64
        } else {
            self.totals.requests as f64
        };
        for row in &mut self.rows {
            let value = if self.totals.tokens.total > 0 {
                row.totals.tokens.total as f64
            } else {
                row.totals.requests as f64
            };
            row.share = if metric > 0.0 { value / metric } else { 0.0 };
        }
    }
}

/// Biggest first; ties fall back to requests, then label, then the row's own
/// id — which is unique, so the order is total and reproducible.
fn sorted_by_total(
    a: &Totals,
    b: &Totals,
    a_label: &str,
    b_label: &str,
    a_id: u16,
    b_id: u16,
) -> std::cmp::Ordering {
    b.tokens
        .total
        .cmp(&a.tokens.total)
        .then_with(|| b.requests.cmp(&a.requests))
        .then_with(|| a_label.cmp(b_label))
        .then_with(|| a_id.cmp(&b_id))
}

/// One row of the session table.
#[derive(Clone, PartialEq, Debug)]
pub struct SessionRow {
    pub session: u16,
    /// pi's own session id, for search, context-menu copy, and drill-down.
    pub id: String,
    pub title: String,
    pub workspace: String,
    /// Provider of the session's top model — searchable and shown as a cell.
    pub provider: String,
    pub provider_id: u16,
    /// The model with the most tokens in this session.
    pub top_model: String,
    /// Interning id of [`Self::top_model`], for context-menu filtering.
    pub top_model_id: Option<u16>,
    /// How many distinct models the session used.
    pub models: usize,
    /// Some request in this session reported cache traffic, so a zero here is
    /// a real zero rather than "this provider has no cache" (§6).
    pub cache_capable: bool,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub totals: Totals,
    pub tool_runs: u64,
    pub tool_errors: u64,
}

impl SessionRow {
    pub fn duration_ms(&self) -> i64 {
        (self.ended_ms - self.started_ms).max(0)
    }
}

/// One row of the day/week/month table.
#[derive(Clone, PartialEq, Debug)]
pub struct BucketRow {
    pub start_ms: i64,
    pub label: String,
    pub stamp: String,
    pub totals: Totals,
}

#[derive(Clone, PartialEq, Debug)]
pub struct BucketTable {
    pub granularity: Granularity,
    pub rows: Vec<BucketRow>,
}

// ── calendar (daily heatmap) ───────────────────────────────────────────────

/// One day of the activity calendar.
#[derive(Clone, PartialEq, Debug)]
pub struct DayCell {
    /// Local midnight of the day.
    pub start_ms: i64,
    pub totals: Totals,
    /// False for the padding days that complete the first and last weeks — the
    /// grid draws those blank rather than as a day of zero activity.
    pub in_range: bool,
}

/// Daily usage for the calendar heatmap: one cell per local day, Monday-aligned
/// and contiguous, so the grid has no holes and the day-of-week rows line up.
///
/// The grid is always a trailing year ([`CALENDAR_DAYS`]), independent of the
/// date range — a contribution graph wants a year to read, and the range can be
/// as short as a day. Scope filters (workspace / model / provider / session /
/// errors / cache) still apply, so the calendar always says what the filter
/// says; it just never narrows by date.
#[derive(Clone, PartialEq, Debug)]
pub struct DailyCalendar {
    /// Local midnight of the first cell (a Monday, possibly before the first
    /// day of the window).
    pub start_ms: i64,
    pub days: Vec<DayCell>,
}

/// How far back the calendar reaches, in days.
pub const CALENDAR_DAYS: i64 = 365;

impl DailyCalendar {
    /// Week columns in the grid. Every week is a full seven cells.
    pub fn weeks(&self) -> usize {
        self.days.len() / 7
    }

    /// The cells inside the calendar's year (the padding days at the ends are
    /// excluded).
    pub fn in_range(&self) -> impl Iterator<Item = &DayCell> {
        self.days.iter().filter(|day| day.in_range)
    }
}

/// Build the calendar grid. The window is the trailing year ending at the most
/// recent scan (or the newest record for a synthetic index), so the grid is
/// stable across range changes and a day clicked in it can always be scoped.
fn build_calendar(index: &UsageIndex, filter: &UsageFilter) -> DailyCalendar {
    let end = if index.scanned_at_ms > 0 {
        index.scanned_at_ms
    } else {
        index.span().map(|(_, newest)| newest).unwrap_or(0)
    };
    // The window covers whole local days, ending at the end of the scan's day,
    // so the newest record is inside it (a half-open window that stopped at the
    // record's own timestamp would exclude it).
    let end_day = local_day_start(end);
    let window = DateRange {
        preset: RangePreset::Custom,
        start_ms: end_day - CALENDAR_DAYS * 86_400_000,
        end_ms: next_bucket(end_day, Granularity::Day),
    };

    let mut daily: HashMap<i64, Totals> = HashMap::new();
    for record in &index.requests {
        if !filter.matches_request_window(index, record, &window) {
            continue;
        }
        index.add_request(
            daily.entry(local_day_start(record.ts_ms)).or_default(),
            record,
        );
    }

    let first_day = local_day_start(window.start_ms);
    let last_day = local_day_start((window.end_ms - 1).max(first_day));
    let grid_start = local_week_start(first_day);
    let day_after_last = next_bucket(last_day, Granularity::Day);

    let mut days: Vec<DayCell> = Vec::new();
    let mut cursor = grid_start;
    // Bounded: the year plus the six days that complete the first week.
    while cursor < day_after_last && days.len() < (CALENDAR_DAYS as usize / 7 + 2) * 7 {
        days.push(DayCell {
            start_ms: cursor,
            totals: daily.remove(&cursor).unwrap_or_default(),
            in_range: window.contains(cursor),
        });
        cursor = next_bucket(cursor, Granularity::Day);
    }
    // Pad the final week, so every row reads full height.
    while !days.len().is_multiple_of(7) {
        days.push(DayCell {
            start_ms: cursor,
            totals: Totals::default(),
            in_range: false,
        });
        cursor = next_bucket(cursor, Granularity::Day);
    }
    DailyCalendar {
        start_ms: grid_start,
        days,
    }
}

// ── cache, tools, errors, latency ──────────────────────────────────────────

#[derive(Clone, Default, PartialEq, Debug)]
pub struct CacheStats {
    pub cache_read: u64,
    pub cache_write: u64,
    pub uncached_input: u64,
    pub hit_rate: Option<f64>,
    /// Requests served at least partly from cache.
    pub cached_requests: u64,
    /// Requests whose model reported any cache activity at all.
    pub requests_with_cache_data: u64,
}

impl CacheStats {
    /// Whether the underlying data supports cache analytics at all. A provider
    /// with no cache semantics reports zeros on every request; the page then
    /// says so instead of computing a 0% hit rate.
    pub fn is_available(&self) -> bool {
        self.cache_read > 0 || self.cache_write > 0
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ToolRow {
    pub tool: u16,
    pub label: String,
    pub class: ToolClass,
    pub calls: u64,
    pub errors: u64,
    pub duration_ms: u64,
    pub duration_samples: u64,
    pub max_ms: u64,
}

impl ToolRow {
    pub fn avg_duration_ms(&self) -> Option<f64> {
        (self.duration_samples > 0).then(|| self.duration_ms as f64 / self.duration_samples as f64)
    }
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct ToolStats {
    pub calls: u64,
    pub errors: u64,
    pub rows: Vec<ToolRow>,
    /// Calls per tool family, in [`ToolClass::ALL`] order.
    pub by_class: Vec<(ToolClass, u64)>,
    pub latency: LatencyStats,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct LatencyStats {
    pub samples: u64,
    pub total_ms: u64,
    pub avg_ms: f64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub max_ms: u64,
}

impl LatencyStats {
    /// Percentiles by nearest rank. Needs a real sample to say anything:
    /// below [`LatencyStats::MIN_SAMPLES`] percentiles are left at zero and
    /// the view shows the average only.
    pub const MIN_SAMPLES: u64 = 20;

    fn from_samples(mut samples: Vec<u32>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        let pick = |quantile: f64| -> u64 {
            let rank = (quantile * samples.len() as f64).ceil() as usize;
            let ix = rank.saturating_sub(1).min(samples.len() - 1);
            u64::from(samples[ix])
        };
        let total: u64 = samples.iter().map(|s| u64::from(*s)).sum();
        Self {
            samples: samples.len() as u64,
            total_ms: total,
            avg_ms: total as f64 / samples.len() as f64,
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            p99_ms: pick(0.99),
            max_ms: u64::from(*samples.last().unwrap_or(&0)),
        }
    }

    pub fn has_percentiles(&self) -> bool {
        self.samples >= Self::MIN_SAMPLES
    }
}

/// Which response-time register the latency chart plots (§20). Percentiles are
/// only offered once there are enough observations to be meaningful.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LatencyMetric {
    Average,
    P50,
    P95,
    P99,
}

impl LatencyMetric {
    pub const ALL: [Self; 4] = [Self::Average, Self::P50, Self::P95, Self::P99];

    pub fn label(self) -> String {
        match self {
            Self::Average => tr!("usage.latency_average"),
            Self::P50 => "P50".to_string(),
            Self::P95 => "P95".to_string(),
            Self::P99 => "P99".to_string(),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == raw.trim())
    }

    /// The plotted value for one bucket. Percentiles return `None` until the
    /// bucket has [`LatencyStats::MIN_SAMPLES`] observations; the chart then
    /// plots a real gap rather than a fabricated zero.
    pub fn value(self, stats: &LatencyStats) -> Option<f64> {
        if stats.samples == 0 {
            return None;
        }
        match self {
            Self::Average => Some(stats.avg_ms),
            Self::P50 if stats.has_percentiles() => Some(stats.p50_ms as f64),
            Self::P95 if stats.has_percentiles() => Some(stats.p95_ms as f64),
            Self::P99 if stats.has_percentiles() => Some(stats.p99_ms as f64),
            _ => None,
        }
    }

    /// Whether the window as a whole supports this register.
    pub fn available(self, stats: &LatencyStats) -> bool {
        match self {
            Self::Average => stats.samples > 0,
            _ => stats.has_percentiles(),
        }
    }
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct ErrorStats {
    /// Requests that failed at the provider.
    pub provider: u64,
    /// Tool runs that returned an error.
    pub tool: u64,
    /// Requests the user stopped (not a failure, but worth counting).
    pub aborted: u64,
    pub rows: Vec<ErrorRow>,
}

/// The informative half of a workspace path: its parent directory, with the
/// home directory shortened to `~`.
fn parent_hint(path: &str) -> String {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    super::format::short_path(&parent)
}

// ── insights ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Neutral,
    Positive,
    Warning,
}

/// One derived sentence. Every insight cites a real measurement; none is
/// generated when the finding is weak (§70/§71: no manufactured conclusions).
#[derive(Clone, PartialEq, Debug)]
pub struct Insight {
    pub text: String,
    pub tone: Tone,
}

// ── the snapshot ───────────────────────────────────────────────────────────

/// Everything the page renders, computed once.
#[derive(Clone, PartialEq, Debug)]
pub struct UsageSnapshot {
    pub filter: UsageFilter,
    pub summary: UsageSummary,
    /// Totals for the immediately preceding window — `None` when there is no
    /// history to compare against.
    pub previous: Option<UsageSummary>,
    pub series: TimeSeries,
    /// Daily totals for the calendar heatmap: a fixed trailing year, independent
    /// of the date range.
    pub calendar: DailyCalendar,
    pub models: Breakdown,
    pub providers: Breakdown,
    pub workspaces: Breakdown,
    pub sessions: Vec<SessionRow>,
    /// Adaptive day/week/month table (also the weekly/monthly trend view for
    /// long ranges).
    pub buckets: BucketTable,
    pub cache: CacheStats,
    pub tools: ToolStats,
    pub latency: LatencyStats,
    pub errors: ErrorStats,
    pub insights: Vec<Insight>,
    /// Requests inside the date range before non-date filters apply — tells
    /// "no data yet" apart from "nothing matches these filters".
    pub requests_in_range: u64,
}

impl UsageSnapshot {
    pub fn compute(index: &UsageIndex, filter: &UsageFilter) -> Self {
        // "All time" starts at the epoch; the chart should start at the first
        // record instead, or every bucket label reads 1970.
        let mut effective_range = filter.range.clone();
        if filter.range.preset == RangePreset::All {
            if let Some((oldest, _)) = index.span() {
                effective_range.start_ms = effective_range
                    .start_ms
                    .max(bucket_start(oldest, effective_range.granularity()));
            }
        }
        let mut summary = UsageSummary::default();
        let mut series = TimeSeries {
            granularity: effective_range.granularity(),
            points: Vec::new(),
        };
        let mut models: HashMap<u16, Totals> = HashMap::new();
        let mut providers: HashMap<u16, Totals> = HashMap::new();
        let mut workspaces: HashMap<u16, Totals> = HashMap::new();
        let mut per_session: HashMap<u16, Totals> = HashMap::new();
        let mut per_session_models: HashMap<u16, HashMap<u16, u64>> = HashMap::new();
        let mut per_session_tools: HashMap<u16, (u64, u64)> = HashMap::new();
        let mut buckets: HashMap<i64, Totals> = HashMap::new();
        let mut durations: Vec<u32> = Vec::new();
        // Durations per chart bucket, so the latency chart can switch between
        // average and percentiles for the same points (§20).
        let mut bucket_durations: HashMap<i64, Vec<u32>> = HashMap::new();
        let mut cache = CacheStats::default();
        let mut per_session_cache: std::collections::HashSet<u16> =
            std::collections::HashSet::new();
        let mut requests_in_range = 0u64;

        for record in &index.requests {
            if filter.range.contains(record.ts_ms) {
                requests_in_range += 1;
            }
            if !filter.matches_request(index, record) {
                continue;
            }
            index.add_request(&mut summary.totals, record);
            index.add_request(models.entry(record.model).or_default(), record);
            index.add_request(
                providers
                    .entry(index.model(record.model).provider)
                    .or_default(),
                record,
            );
            index.add_request(
                workspaces
                    .entry(index.session(record.session).workspace)
                    .or_default(),
                record,
            );
            index.add_request(per_session.entry(record.session).or_default(), record);
            *per_session_models
                .entry(record.session)
                .or_default()
                .entry(record.model)
                .or_default() += record.tokens.total;
            let bucket = bucket_start(record.ts_ms, series.granularity);
            index.add_request(buckets.entry(bucket).or_default(), record);
            if let Some(duration) = record.duration_ms {
                durations.push(duration);
                bucket_durations.entry(bucket).or_default().push(duration);
            }
            if record.tokens.cache_read > 0 || record.tokens.cache_write > 0 {
                per_session_cache.insert(record.session);
            }
            cache.cache_read += record.tokens.cache_read;
            cache.cache_write += record.tokens.cache_write;
            cache.uncached_input += record.tokens.input;
            if record.tokens.cache_read > 0 {
                cache.cached_requests += 1;
            }
            if record.tokens.cache_read > 0 || record.tokens.cache_write > 0 {
                cache.requests_with_cache_data += 1;
            }
        }
        // A rate needs a denominator the provider actually reports: without
        // any cache traffic at all the hit rate is unknown, not zero (§25).
        cache.hit_rate = TokenCounts {
            input: cache.uncached_input,
            output: 0,
            cache_read: cache.cache_read,
            cache_write: cache.cache_write,
            total: 0,
        }
        .cache_hit_rate();

        // Tool runs: same filter, their own timestamps.
        let mut tool_rows: HashMap<u16, ToolRow> = HashMap::new();
        let mut tool_durations: Vec<u32> = Vec::new();
        let mut by_class: HashMap<ToolClass, u64> = HashMap::new();
        for run in &index.tool_runs {
            if !filter.matches_tool(index, run) {
                continue;
            }
            let entry = index.tool(run.tool);
            summary.tool_runs += 1;
            if entry.id == "bash" {
                summary.bash_runs += 1;
            }
            if !run.ok {
                summary.tool_errors += 1;
            }
            let row = tool_rows.entry(run.tool).or_insert_with(|| ToolRow {
                tool: run.tool,
                label: entry.label.clone(),
                class: entry.class,
                calls: 0,
                errors: 0,
                duration_ms: 0,
                duration_samples: 0,
                max_ms: 0,
            });
            row.calls += 1;
            if !run.ok {
                row.errors += 1;
            }
            if let Some(duration) = run.duration_ms {
                row.duration_ms += u64::from(duration);
                row.duration_samples += 1;
                row.max_ms = row.max_ms.max(u64::from(duration));
                tool_durations.push(duration);
            }
            *by_class.entry(entry.class).or_default() += 1;
            let counters = per_session_tools.entry(run.session).or_default();
            counters.0 += 1;
            if !run.ok {
                counters.1 += 1;
            }
        }
        let mut tool_list: Vec<ToolRow> = tool_rows.into_values().collect();
        tool_list.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.label.cmp(&b.label)));

        // Errors: provider failures come from the request records, tool
        // failures from the tool runs. `provider` is counted from the rows this
        // filter actually shows, so the panel's headline always matches its
        // table.
        let mut errors = ErrorStats {
            provider: 0,
            tool: summary.tool_errors,
            aborted: summary.totals.aborted,
            rows: Vec::new(),
        };
        for row in &index.errors {
            if !filter.matches_time(row.ts_ms) {
                continue;
            }
            if filter.session.is_some_and(|only| row.session != only) {
                continue;
            }
            if !filter.workspaces.is_empty()
                && !filter
                    .workspaces
                    .contains(&index.session(row.session).workspace)
            {
                continue;
            }
            if !filter.matches_model(index, row.model) {
                continue;
            }
            errors.rows.push(row.clone());
        }
        errors.rows.sort_by_key(|a| std::cmp::Reverse(a.ts_ms));
        errors.provider = errors
            .rows
            .iter()
            .filter(|row| row.kind == ErrorKind::Provider)
            .count() as u64;
        errors.rows.truncate(400);

        // Turns (prompts): date + session + workspace dimensions only.
        let mut turns = 0u64;
        for (ix, session) in index.sessions.iter().enumerate() {
            for ts_ms in &session.turns {
                if filter.matches_turn(index, ix as u16, *ts_ms) {
                    turns += 1;
                }
            }
        }
        summary.turns = turns;
        summary.sessions = per_session.len() as u64;
        summary.models = models.len() as u64;
        summary.providers = providers.len() as u64;
        summary.workspaces = workspaces.len() as u64;

        // Session rows.
        let mut sessions: Vec<SessionRow> = per_session
            .into_iter()
            .map(|(session, totals)| {
                let entry = index.session(session);
                let model_totals = per_session_models.remove(&session).unwrap_or_default();
                let top = model_totals
                    .iter()
                    .max_by_key(|(model, tokens)| (**tokens, **model))
                    .map(|(model, _)| *model);
                let top_model = top
                    .map(|model| {
                        let (provider, name) = index.model_pair(model);
                        format!("{name} · {provider}")
                    })
                    .unwrap_or_else(|| "—".into());
                let provider = top
                    .map(|model| index.provider_of(model).label.clone())
                    .unwrap_or_else(|| "—".into());
                let provider_id = top.map(|model| index.model(model).provider).unwrap_or(0);
                let (tool_runs, tool_errors) =
                    per_session_tools.remove(&session).unwrap_or_default();
                SessionRow {
                    session,
                    id: entry.id.clone(),
                    title: if entry.title.is_empty() {
                        tr!(
                            "usage.session_fallback",
                            id = entry.id.chars().take(8).collect::<String>()
                        )
                    } else {
                        entry.title.clone()
                    },
                    workspace: index.workspace_of_session(session).label.clone(),
                    provider,
                    provider_id,
                    top_model,
                    top_model_id: top,
                    models: model_totals.len(),
                    cache_capable: per_session_cache.contains(&session),
                    started_ms: entry.started_ms,
                    ended_ms: entry.ended_ms,
                    totals,
                    tool_runs,
                    tool_errors,
                }
            })
            .collect();
        // Totals, then title, then pi's own session id — the last one is
        // unique, so the table never reorders between identical runs (§80).
        sessions.sort_by(|a, b| {
            b.totals
                .tokens
                .total
                .cmp(&a.totals.tokens.total)
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| {
                    index
                        .session(a.session)
                        .id
                        .cmp(&index.session(b.session).id)
                })
        });

        // Breakdowns.
        let models = breakdown(models, |id| {
            let entry = index.model(id);
            (
                entry.label.clone(),
                Some(index.provider_of(id).label.clone()),
            )
        });
        let providers = breakdown(providers, |id| {
            (index.providers[id as usize].label.clone(), None)
        });
        let workspaces = breakdown(workspaces, |id| {
            let entry = &index.workspaces[id as usize];
            // The label is the folder's own name; the parent folder is the
            // informative second line ("~/Personal", not the shared prefix).
            (entry.label.clone(), Some(parent_hint(&entry.path)))
        });

        // Time series buckets, contiguous so gaps read as zero rather than
        // silently compressing the axis.
        let granularity = series.granularity;
        let mut start = bucket_start(effective_range.start_ms, granularity);
        let end = effective_range.end_ms;
        let mut guard = 0;
        while start < end && guard < 4_000 {
            let totals = buckets.remove(&start).unwrap_or_default();
            let latency =
                LatencyStats::from_samples(bucket_durations.remove(&start).unwrap_or_default());
            series.points.push(SeriesPoint {
                start_ms: start,
                label: bucket_label(start, granularity),
                stamp: stamp_label(start, granularity),
                totals,
                latency,
            });
            start = next_bucket(start, granularity);
            guard += 1;
        }

        let calendar = build_calendar(index, filter);

        // The day/week/month table uses a coarser granularity than the chart
        // for long ranges, so it stays readable (and doubles as the
        // weekly/monthly trend view).
        let table_granularity = match granularity {
            Granularity::Hour | Granularity::Day => Granularity::Day,
            Granularity::Week => Granularity::Week,
            Granularity::Month => Granularity::Month,
        };
        let mut table_buckets: HashMap<i64, Totals> = HashMap::new();
        for record in &index.requests {
            if !filter.matches_request(index, record) {
                continue;
            }
            index.add_request(
                table_buckets
                    .entry(bucket_start(record.ts_ms, table_granularity))
                    .or_default(),
                record,
            );
        }
        let mut table_rows: Vec<BucketRow> = table_buckets
            .into_iter()
            .map(|(start_ms, totals)| BucketRow {
                start_ms,
                label: bucket_label(start_ms, table_granularity),
                stamp: stamp_label(start_ms, table_granularity),
                totals,
            })
            .collect();
        // Newest first: a daily table reads like a log.
        table_rows.sort_by_key(|a| std::cmp::Reverse(a.start_ms));

        let previous = filter
            .range
            .previous()
            .filter(|prev| index.has_data_before(filter.range.start_ms) && prev.len_ms() > 0)
            .map(|prev| {
                let previous_filter = UsageFilter {
                    range: prev,
                    // The focus bucket lives inside the *current* window, so it
                    // can never apply to a comparison against the window before.
                    focus: None,
                    ..filter.clone()
                };
                summary_only(index, &previous_filter)
            });

        let latency = LatencyStats::from_samples(durations);
        let mut snapshot = Self {
            filter: filter.clone(),
            summary,
            previous,
            series,
            calendar,
            models,
            providers,
            workspaces,
            sessions,
            buckets: BucketTable {
                granularity: table_granularity,
                rows: table_rows,
            },
            cache,
            tools: ToolStats {
                calls: 0,
                errors: 0,
                rows: tool_list,
                by_class: ToolClass::ALL
                    .into_iter()
                    .map(|class| (class, by_class.remove(&class).unwrap_or(0)))
                    .collect(),
                latency: LatencyStats::from_samples(tool_durations),
            },
            latency,
            errors,
            insights: Vec::new(),
            requests_in_range,
        };
        snapshot.tools.calls = snapshot.summary.tool_runs;
        snapshot.tools.errors = snapshot.summary.tool_errors;
        snapshot.insights = derive_insights(&snapshot, index);
        snapshot
    }

    pub fn delta(&self, metric: ChartMetric) -> MetricDelta {
        let Some(previous) = &self.previous else {
            return MetricDelta::unavailable();
        };
        MetricDelta::between(
            metric.total(&self.summary.totals),
            metric.total(&previous.totals),
        )
    }

    /// Nothing to show for this filter: no requests and no tool activity.
    ///
    /// Prompts are deliberately *not* part of this test: turns ignore the
    /// model and provider filters (a prompt is not a model call), so counting
    /// them here would keep the page alive as a wall of zeros when a model
    /// filter matches nothing instead of saying so.
    pub fn is_empty(&self) -> bool {
        self.summary.totals.requests == 0 && self.summary.tool_runs == 0
    }

    /// True when the date range holds data but the filters excluded all of it.
    pub fn filtered_out(&self) -> bool {
        self.is_empty() && self.requests_in_range > 0
    }
}

/// Summary-only aggregation for the comparison window.
fn summary_only(index: &UsageIndex, filter: &UsageFilter) -> UsageSummary {
    let mut summary = UsageSummary::default();
    for record in &index.requests {
        if filter.matches_request(index, record) {
            index.add_request(&mut summary.totals, record);
        }
    }
    let mut sessions: std::collections::HashSet<u16> = std::collections::HashSet::new();
    for run in &index.tool_runs {
        if !filter.matches_tool(index, run) {
            continue;
        }
        summary.tool_runs += 1;
        if index.tool(run.tool).id == "bash" {
            summary.bash_runs += 1;
        }
        if !run.ok {
            summary.tool_errors += 1;
        }
    }
    for record in &index.requests {
        if filter.matches_request(index, record) {
            sessions.insert(record.session);
        }
    }
    summary.sessions = sessions.len() as u64;
    summary.turns = index
        .sessions
        .iter()
        .enumerate()
        .flat_map(|(ix, session)| {
            session
                .turns
                .iter()
                .filter(move |ts| filter.matches_turn(index, ix as u16, **ts))
        })
        .count() as u64;
    summary
}

fn breakdown(
    totals: HashMap<u16, Totals>,
    label: impl Fn(u16) -> (String, Option<String>),
) -> Breakdown {
    let mut breakdown = Breakdown::default();
    for (id, totals) in totals {
        breakdown.totals.add(&totals);
        let (label, sub) = label(id);
        breakdown.rows.push(GroupRow {
            id,
            label,
            sub,
            totals,
            share: 0.0,
        });
    }
    breakdown
        .rows
        .sort_by(|a, b| sorted_by_total(&a.totals, &b.totals, &a.label, &b.label, a.id, b.id));
    breakdown.finish();
    breakdown
}

// ── insights ───────────────────────────────────────────────────────────────

/// Thresholds that keep the insight strip quiet: a finding has to be real
/// before it is printed.
const INSIGHT_MIN_PCT: f64 = 10.0;
/// Below this much traffic, "X dominates" is noise rather than a finding.
const INSIGHT_MIN_REQUESTS: u64 = 20;
const INSIGHT_MIN_CACHE_POINTS: f64 = 3.0;
const INSIGHT_MIN_SHARE: f64 = 20.0;
const INSIGHT_ANOMALY_FACTOR: f64 = 2.0;

fn derive_insights(snapshot: &UsageSnapshot, index: &UsageIndex) -> Vec<Insight> {
    let mut out: Vec<Insight> = Vec::new();
    if snapshot.summary.totals.requests == 0 {
        return out;
    }
    let range_name = snapshot.filter.range.label();

    // Volume change against the previous window.
    if let Some(previous) = &snapshot.previous {
        let current = snapshot.summary.totals.tokens.total as f64;
        let before = previous.totals.tokens.total as f64;
        let delta = MetricDelta::between(current, before);
        if before > 0.0 {
            if let Some(pct) = delta.pct {
                if pct.abs() >= INSIGHT_MIN_PCT {
                    let verb = if pct > 0.0 {
                        tr!("usage.insight_up")
                    } else {
                        tr!("usage.insight_down")
                    };
                    out.push(Insight {
                        text: tr!(
                            "usage.insight_token_delta",
                            verb = verb,
                            pct = format!("{:.0}", pct.abs()),
                            now = super::format::compact_tokens(current as u64),
                            before = super::format::compact_tokens(before as u64),
                        ),
                        tone: Tone::Neutral,
                    });
                }
            }
        }
    }

    // Dominant model — only once there is enough traffic for the share to
    // mean something, and only when more than one model was actually used.
    if snapshot.summary.totals.requests >= INSIGHT_MIN_REQUESTS {
        if let Some(top) = snapshot.models.rows.first() {
            if snapshot.models.rows.len() > 1 && top.share * 100.0 >= INSIGHT_MIN_SHARE {
                out.push(Insight {
                    text: tr!(
                        "usage.insight_top_model",
                        model = top.label,
                        share = format!("{:.0}", top.share * 100.0),
                        tokens = super::format::compact_tokens(top.totals.tokens.total),
                        requests = super::format::count(top.totals.requests),
                    ),
                    tone: Tone::Neutral,
                });
            }
        }
    }

    // Cache movement, where the data supports it.
    if snapshot.cache.is_available() {
        if let Some(previous) = &snapshot.previous {
            if let (Some(now), Some(before)) =
                (snapshot.cache.hit_rate, previous.totals.cache_hit_rate())
            {
                let change = now - before;
                if change.abs() >= INSIGHT_MIN_CACHE_POINTS {
                    out.push(Insight {
                        text: tr!(
                            "usage.insight_cache_hit",
                            verb = if change > 0.0 {
                                tr!("usage.insight_improved")
                            } else {
                                tr!("usage.insight_fell")
                            },
                            before = format!("{:.0}", before),
                            now = format!("{:.0}", now),
                        ),
                        tone: if change > 0.0 {
                            Tone::Positive
                        } else {
                            Tone::Warning
                        },
                    });
                }
            }
        }
    }

    // Heaviest workspace.
    if let Some(top) = snapshot.workspaces.rows.first() {
        if snapshot.workspaces.rows.len() > 1 && top.share * 100.0 >= INSIGHT_MIN_SHARE {
            out.push(Insight {
                text: tr!(
                    "usage.insight_top_workspace",
                    workspace = top.label,
                    tokens = super::format::compact_tokens(top.totals.tokens.total),
                    share = format!("{:.0}", top.share * 100.0)
                ),
                tone: Tone::Neutral,
            });
        }
    }

    // Busiest bucket, only when it is a genuine outlier.
    let values: Vec<(i64, u64)> = snapshot
        .buckets
        .rows
        .iter()
        .filter(|row| row.totals.tokens.total > 0)
        .map(|row| (row.start_ms, row.totals.tokens.total))
        .collect();
    if values.len() >= 5 {
        let mean = values.iter().map(|(_, v)| *v as f64).sum::<f64>() / values.len() as f64;
        if let Some((start, peak)) = values.iter().max_by_key(|(_, v)| *v) {
            if mean > 0.0 && *peak as f64 >= mean * INSIGHT_ANOMALY_FACTOR {
                out.push(Insight {
                    text: tr!(
                        "usage.insight_peak_bucket",
                        bucket = bucket_label(*start, snapshot.buckets.granularity),
                        factor = format!("{:.1}", *peak as f64 / mean),
                        metric = snapshot.buckets.granularity.label()
                    ),
                    tone: Tone::Warning,
                });
            }
        }
    }

    // Failures, when they are concentrated rather than incidental.
    if snapshot.summary.totals.errors > 0 {
        if let Some(rate) = snapshot.summary.totals.error_rate() {
            if rate >= 1.0 {
                out.push(Insight {
                    text: tr!(
                        "usage.insight_failures",
                        failed = super::format::count(snapshot.summary.totals.errors),
                        total = super::format::count(snapshot.summary.totals.requests),
                        rate = format!("{:.1}", rate)
                    ),
                    tone: Tone::Warning,
                });
            }
        }
    }

    // Latency movement, when the sample is big enough to mean something.
    if snapshot.latency.has_percentiles() {
        if let Some(previous) = &snapshot.previous {
            if previous.totals.duration_samples >= LatencyStats::MIN_SAMPLES {
                let now = snapshot.latency.avg_ms;
                let before = previous.totals.avg_duration_ms().unwrap_or(0.0);
                if before > 0.0 && (now - before).abs() / before * 100.0 >= INSIGHT_MIN_PCT {
                    out.push(Insight {
                        text: tr!(
                            "usage.insight_latency",
                            before = super::format::duration_ms(before),
                            now = super::format::duration_ms(now),
                            requests =
                                super::format::count(snapshot.summary.totals.duration_samples)
                        ),
                        tone: Tone::Neutral,
                    });
                }
            }
        }
    }

    // How much of the store this window covers — orients the user when a
    // narrow range is showing.
    if let Some((_oldest, _newest)) = index.span() {
        if range_name == RangePreset::All.as_str() && snapshot.summary.sessions > 0 {
            out.push(Insight {
                text: tr!(
                    "usage.insight_all_time",
                    sessions = super::format::count(snapshot.summary.sessions),
                    requests = super::format::count(snapshot.summary.totals.requests)
                ),
                tone: Tone::Neutral,
            });
        }
    }

    // Keep the strip quiet: four lines at most.
    out.truncate(4);
    out
}
