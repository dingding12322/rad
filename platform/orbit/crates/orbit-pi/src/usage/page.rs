//! The Usage page's state: one scanner, one cached snapshot, one filter.
//!
//! The page owns everything mutable — the filter, the view preferences, the
//! popover state, and the last computed [`UsageSnapshot`]. Recompute happens
//! on data arrival or on a state change, never on a paint (§51): the render
//! path reads `snapshot()` and formats.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    point, px, App, AppContext, Context, Entity, Focusable, ScrollHandle, Subscription, Window,
};
use serde_json::Value;

use crate::composer::ComposerInput;

use super::aggregate::{
    Breakdown, BucketRow, ChartMetric, GroupRow, LatencyMetric, SeriesPoint, SessionRow, ToolRow,
    Totals, UsageSnapshot,
};
use super::collect::{now_ms, UsageScanner};
use super::model::*;
use super::table::{FailureRow, FailureSort, TableKind};

/// Page sizes offered by the sessions table footer (§26).
pub const PAGE_SIZES: [usize; 4] = [10, 25, 50, 100];
/// Default rows per page.
pub const DEFAULT_PAGE_SIZE: usize = 25;
/// Default rows per page for a breakdown tab. Shorter than the sessions table:
/// a dimension rarely has as many rows, so ten is a full screen already.
pub const DEFAULT_BREAKDOWN_PAGE_SIZE: usize = 10;

/// Everything one request to the sessions table needs, independent of GPUI
/// (§100/§101): filter/search narrow, sort orders, then pagination slices.
/// Tables never receive the full dataset.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionQuery {
    pub search: String,
    pub sort: SessionSort,
    pub desc: bool,
    /// 1-based.
    pub page: usize,
    pub page_size: usize,
}

/// One page of sessions plus the metadata the footer needs (§75).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionQueryResult {
    pub rows: Vec<SessionRow>,
    /// Rows matching the search across the whole filtered set (not the page).
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

impl SessionQueryResult {
    pub fn page_count(&self) -> usize {
        self.total.div_ceil(self.page_size.max(1)).max(1)
    }

    pub fn first_row(&self) -> usize {
        if self.total == 0 {
            0
        } else {
            (self.page - 1) * self.page_size + 1
        }
    }

    pub fn last_row(&self) -> usize {
        (self.first_row() + self.rows.len()).saturating_sub(1)
    }
}

/// Filter → search → sort → paginate, in that order (§74). Pure, so the
/// ordering guarantees are unit-tested without GPUI.
pub fn query_sessions(
    index: &UsageIndex,
    snapshot: &UsageSnapshot,
    query: &SessionQuery,
) -> SessionQueryResult {
    let needle = query.search.trim().to_lowercase();
    let mut rows: Vec<SessionRow> = snapshot
        .sessions
        .iter()
        .filter(|row| session_matches(index, row, &needle))
        .cloned()
        .collect();
    sort_session_rows(&mut rows, query.sort, query.desc, index);

    let total = rows.len();
    let page_size = query.page_size.max(1);
    let page_count = total.div_ceil(page_size).max(1);
    let page = query.page.clamp(1, page_count);
    let start = (page - 1) * page_size;
    let rows = if start < total {
        rows[start..(start + page_size).min(total)].to_vec()
    } else {
        Vec::new()
    };
    SessionQueryResult {
        rows,
        total,
        page,
        page_size,
    }
}

/// The searchable fields of a session (§30): title, session id, workspace,
/// project path, provider, model, and a coarse status.
fn session_matches(index: &UsageIndex, row: &SessionRow, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    row.title.to_lowercase().contains(needle)
        || row.id.to_lowercase().contains(needle)
        || row.workspace.to_lowercase().contains(needle)
        || row.provider.to_lowercase().contains(needle)
        || row.top_model.to_lowercase().contains(needle)
        || index
            .workspace_of_session(row.session)
            .path
            .to_lowercase()
            .contains(needle)
        || session_status(row).contains(needle)
}

fn session_status(row: &SessionRow) -> &'static str {
    if row.totals.errors > 0 || row.tool_errors > 0 {
        "error failed"
    } else {
        "ok"
    }
}

/// The one ordering used by the table, the export, and the tests (§25).
pub fn sort_session_rows(
    rows: &mut [SessionRow],
    sort: SessionSort,
    desc: bool,
    index: &UsageIndex,
) {
    rows.sort_by(|a, b| {
        let ordering = if sort.is_text() {
            sort.text(a)
                .to_lowercase()
                .cmp(&sort.text(b).to_lowercase())
        } else {
            sort.key(a)
                .partial_cmp(&sort.key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        let ordering = if desc { ordering.reverse() } else { ordering };
        ordering.then_with(|| a.title.cmp(&b.title)).then_with(|| {
            index
                .session(a.session)
                .id
                .cmp(&index.session(b.session).id)
        })
    });
}

/// How long the page may serve a stale index before it rescans on open.
const STALE_AFTER: Duration = Duration::from_secs(60);
/// Minimum gap between rescans triggered by store writes while the page is
/// open, so a streaming agent doesn't spin the scanner.
const RESCAN_INTERVAL: Duration = Duration::from_secs(3);

/// Which row the session table sorts on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionSort {
    Started,
    Title,
    Workspace,
    Provider,
    Model,
    Requests,
    Input,
    Output,
    Cache,
    Tokens,
    Duration,
    Errors,
    Tools,
}

impl SessionSort {
    /// Every sortable column, in the order the table offers them.
    pub const ALL: [Self; 13] = [
        Self::Title,
        Self::Workspace,
        Self::Provider,
        Self::Model,
        Self::Started,
        Self::Duration,
        Self::Requests,
        Self::Input,
        Self::Output,
        Self::Cache,
        Self::Tokens,
        Self::Errors,
        Self::Tools,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Started => tr!("usage.col_started"),
            Self::Title => tr!("usage.col_session"),
            Self::Workspace => tr!("usage.col_workspace"),
            Self::Provider => tr!("usage.col_provider"),
            Self::Model => tr!("usage.col_model"),
            Self::Requests => tr!("usage.metric_requests"),
            Self::Input => tr!("usage.metric_input"),
            Self::Output => tr!("usage.metric_output"),
            Self::Cache => tr!("usage.metric_cache"),
            Self::Tokens => tr!("usage.metric_tokens"),
            Self::Duration => tr!("usage.col_duration"),
            Self::Errors => tr!("usage.metric_errors"),
            Self::Tools => tr!("usage.col_tools"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Title => "title",
            Self::Workspace => "workspace",
            Self::Provider => "provider",
            Self::Model => "model",
            Self::Requests => "requests",
            Self::Input => "input",
            Self::Output => "output",
            Self::Cache => "cache",
            Self::Tokens => "tokens",
            Self::Duration => "duration",
            Self::Errors => "errors",
            Self::Tools => "tools",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw.trim())
    }

    /// Numeric value for sorting; text columns sort on their label.
    fn key(self, row: &SessionRow) -> f64 {
        match self {
            Self::Started => row.ended_ms as f64,
            Self::Requests => row.totals.requests as f64,
            Self::Input => row.totals.tokens.input as f64,
            Self::Output => row.totals.tokens.output as f64,
            Self::Cache => row.totals.tokens.cache_read as f64,
            Self::Tokens => row.totals.tokens.total as f64,
            Self::Duration => row.duration_ms() as f64,
            Self::Errors => (row.totals.errors + row.tool_errors) as f64,
            Self::Tools => row.tool_runs as f64,
            Self::Title | Self::Workspace | Self::Provider | Self::Model => 0.0,
        }
    }

    fn text(self, row: &SessionRow) -> &str {
        match self {
            Self::Title => &row.title,
            Self::Workspace => &row.workspace,
            Self::Provider => &row.provider,
            Self::Model => &row.top_model,
            _ => "",
        }
    }

    fn is_text(self) -> bool {
        matches!(
            self,
            Self::Title | Self::Workspace | Self::Provider | Self::Model
        )
    }
}

/// Which row the day/week/month table sorts on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BucketSort {
    Date,
    Requests,
    Tokens,
    Cache,
    Errors,
}

impl BucketSort {
    pub fn id(self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Requests => "requests",
            Self::Tokens => "tokens",
            Self::Cache => "cache-hit",
            Self::Errors => "errors",
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Date => tr!("usage.col_date"),
            Self::Requests => tr!("usage.metric_requests"),
            Self::Tokens => tr!("usage.metric_tokens"),
            Self::Cache => tr!("usage.col_cache_hit"),
            Self::Errors => tr!("usage.metric_errors"),
        }
    }

    fn key(self, row: &BucketRow) -> f64 {
        match self {
            Self::Date => row.start_ms as f64,
            Self::Requests => row.totals.requests as f64,
            Self::Tokens => row.totals.tokens.total as f64,
            Self::Cache => row.totals.cache_hit_rate().unwrap_or(-1.0),
            Self::Errors => row.totals.errors as f64,
        }
    }

    /// Hideable columns; Date is always shown.
    pub const HIDEABLE: [Self; 4] = [Self::Requests, Self::Tokens, Self::Cache, Self::Errors];
}

/// One page of the Daily table plus the metadata the footer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct BucketQueryResult {
    pub rows: Vec<BucketRow>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub totals: Totals,
}

impl BucketQueryResult {
    pub fn page_count(&self) -> usize {
        self.total.div_ceil(self.page_size.max(1)).max(1)
    }

    pub fn first_row(&self) -> usize {
        if self.total == 0 {
            0
        } else {
            (self.page - 1) * self.page_size + 1
        }
    }

    pub fn last_row(&self) -> usize {
        (self.first_row() + self.rows.len()).saturating_sub(1)
    }
}

/// Filter → search → sort → paginate the day/week/month buckets.
pub fn query_buckets(
    rows: &[BucketRow],
    search: &str,
    sort: BucketSort,
    desc: bool,
    page: usize,
    page_size: usize,
) -> BucketQueryResult {
    let needle = search.trim().to_lowercase();
    let mut rows: Vec<BucketRow> = rows
        .iter()
        .filter(|row| {
            needle.is_empty()
                || row.label.to_lowercase().contains(&needle)
                || row.stamp.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        let ordering = sort
            .key(a)
            .partial_cmp(&sort.key(b))
            .unwrap_or(std::cmp::Ordering::Equal);
        let ordering = if desc { ordering.reverse() } else { ordering };
        ordering.then(a.start_ms.cmp(&b.start_ms))
    });
    let mut totals = Totals::default();
    for row in &rows {
        totals.add(&row.totals);
    }
    let (rows, page, total) = paginate(&rows, page, page_size);
    BucketQueryResult {
        rows,
        total,
        page,
        page_size: page_size.max(1),
        totals,
    }
}

/// One page of the Failures table plus the metadata the footer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct FailureQueryResult {
    pub rows: Vec<FailureRow>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

impl FailureQueryResult {
    pub fn page_count(&self) -> usize {
        self.total.div_ceil(self.page_size.max(1)).max(1)
    }

    pub fn first_row(&self) -> usize {
        if self.total == 0 {
            0
        } else {
            (self.page - 1) * self.page_size + 1
        }
    }

    pub fn last_row(&self) -> usize {
        (self.first_row() + self.rows.len()).saturating_sub(1)
    }
}

/// Filter → search → sort → paginate failure events.
pub fn query_failures(
    rows: &[FailureRow],
    search: &str,
    sort: FailureSort,
    desc: bool,
    page: usize,
    page_size: usize,
) -> FailureQueryResult {
    let needle = search.trim().to_lowercase();
    let mut rows: Vec<FailureRow> = rows
        .iter()
        .filter(|row| {
            needle.is_empty()
                || row.model.to_lowercase().contains(&needle)
                || row.session_title.to_lowercase().contains(&needle)
                || row.message.to_lowercase().contains(&needle)
                || row.kind.as_str().contains(&needle)
                || super::table::failure_when(row.ts_ms)
                    .to_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        let ordering = match sort {
            FailureSort::When => a.ts_ms.cmp(&b.ts_ms),
            FailureSort::Kind => a.kind.as_str().cmp(b.kind.as_str()),
            FailureSort::Model => a.model.to_lowercase().cmp(&b.model.to_lowercase()),
            FailureSort::Session => a
                .session_title
                .to_lowercase()
                .cmp(&b.session_title.to_lowercase()),
            FailureSort::Message => a.message.to_lowercase().cmp(&b.message.to_lowercase()),
        };
        let ordering = if desc { ordering.reverse() } else { ordering };
        ordering.then(a.ts_ms.cmp(&b.ts_ms))
    });
    let (rows, page, total) = paginate(&rows, page, page_size);
    FailureQueryResult {
        rows,
        total,
        page,
        page_size: page_size.max(1),
    }
}

/// Which column the Usage-over-time data table sorts on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeriesSort {
    Time,
    Value,
    Requests,
    Tokens,
}

impl SeriesSort {
    pub fn id(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Value => "value",
            Self::Requests => "requests",
            Self::Tokens => "tokens",
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Time => tr!("usage.col_time"),
            Self::Value => tr!("usage.col_metric"),
            Self::Requests => tr!("usage.metric_requests"),
            Self::Tokens => tr!("usage.metric_tokens"),
        }
    }

    /// Hideable columns; Time is always shown.
    pub const HIDEABLE: [Self; 3] = [Self::Value, Self::Requests, Self::Tokens];
}

/// One page of the usage-over-time table plus the metadata the footer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesQueryResult {
    pub rows: Vec<SeriesPoint>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

impl SeriesQueryResult {
    pub fn page_count(&self) -> usize {
        self.total.div_ceil(self.page_size.max(1)).max(1)
    }

    pub fn first_row(&self) -> usize {
        if self.total == 0 {
            0
        } else {
            (self.page - 1) * self.page_size + 1
        }
    }

    pub fn last_row(&self) -> usize {
        (self.first_row() + self.rows.len()).saturating_sub(1)
    }
}

/// Filter → search → sort → paginate the usage-over-time buckets. Pure, so
/// the ordering is unit-tested without GPUI.
#[allow(clippy::too_many_arguments)]
pub fn query_series(
    points: &[SeriesPoint],
    search: &str,
    sort: SeriesSort,
    desc: bool,
    metric: ChartMetric,
    latency: LatencyMetric,
    page: usize,
    page_size: usize,
) -> SeriesQueryResult {
    let needle = search.trim().to_lowercase();
    let mut points: Vec<SeriesPoint> = points
        .iter()
        .filter(|point| {
            needle.is_empty()
                || point.stamp.to_lowercase().contains(&needle)
                || point.label.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    points.sort_by(|a, b| {
        let left = series_point_key(sort, metric, latency, a);
        let right = series_point_key(sort, metric, latency, b);
        let order = left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal);
        let order = if desc { order.reverse() } else { order };
        order.then(a.start_ms.cmp(&b.start_ms))
    });
    let (rows, page, total) = paginate(&points, page, page_size);
    SeriesQueryResult {
        rows,
        total,
        page,
        page_size: page_size.max(1),
    }
}

/// The open filter popover, if any. One at a time, like the app's other
/// anchored menus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Range,
    Workspace,
    Provider,
    Model,
    /// The sessions table's rows-per-page picker.
    PageSize,
    /// The sessions table's column-visibility picker.
    Columns,
    /// The breakdown table's column-visibility picker.
    BreakdownColumns,
    /// The breakdown table's rows-per-page picker.
    BreakdownPageSize,
    /// The usage-over-time table's column-visibility picker.
    SeriesColumns,
    /// The usage-over-time table's rows-per-page picker.
    SeriesPageSize,
    /// The Daily records table's column-visibility picker.
    BucketColumns,
    /// The Daily records table's rows-per-page picker.
    BucketPageSize,
    /// The Failures records table's column-visibility picker.
    FailureColumns,
    /// The Failures records table's rows-per-page picker.
    FailurePageSize,
    /// The header's export popover.
    Export,
}

/// Export flavors: the raw filtered records, or the aggregates on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    /// Stable identifier for element ids — never localized.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// Which dimension the one breakdown section ranks. The four distributions the
/// page used to stack as separate panels are the four faces of this selector.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BreakdownTab {
    Models,
    Workspaces,
    Providers,
    Tools,
}

impl BreakdownTab {
    pub const ALL: [Self; 4] = [Self::Models, Self::Workspaces, Self::Providers, Self::Tools];

    pub fn label(self) -> String {
        match self {
            Self::Models => tr!("usage.tab_models"),
            Self::Workspaces => tr!("usage.tab_workspaces"),
            Self::Providers => tr!("usage.tab_providers"),
            Self::Tools => tr!("usage.tab_tools"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Models => "models",
            Self::Workspaces => "workspaces",
            Self::Providers => "providers",
            Self::Tools => "tools",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.as_str() == raw.trim())
    }
}

/// Which column the breakdown data table orders on. Shared across the three
/// token dimensions, whose rows carry the same measures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BreakdownSort {
    Name,
    Requests,
    Input,
    Output,
    Cache,
    Tokens,
}

impl BreakdownSort {
    /// Every sortable column, in the order the table offers them.
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::Requests,
        Self::Input,
        Self::Output,
        Self::Cache,
        Self::Tokens,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Name => tr!("usage.col_name"),
            Self::Requests => tr!("usage.metric_requests"),
            Self::Input => tr!("usage.metric_input"),
            Self::Output => tr!("usage.metric_output"),
            Self::Cache => tr!("usage.metric_cache"),
            Self::Tokens => tr!("usage.metric_tokens"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Requests => "requests",
            Self::Input => "input",
            Self::Output => "output",
            Self::Cache => "cache",
            Self::Tokens => "tokens",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|column| column.as_str() == raw.trim())
    }

    /// Numeric value for sorting; the name column sorts on its own text.
    pub fn key(self, row: &super::aggregate::GroupRow) -> f64 {
        match self {
            Self::Requests => row.totals.requests as f64,
            Self::Input => row.totals.tokens.input as f64,
            Self::Output => row.totals.tokens.output as f64,
            Self::Cache => row.totals.tokens.cache_read as f64,
            Self::Tokens => row.totals.tokens.total as f64,
            Self::Name => 0.0,
        }
    }

    pub fn is_text(self) -> bool {
        self == Self::Name
    }
}

/// Which table the one records section shows. Keeps every detail table without
/// stacking three scroll-length sections on top of each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetailTab {
    Sessions,
    Daily,
    Failures,
}

impl DetailTab {
    pub const ALL: [Self; 3] = [Self::Sessions, Self::Daily, Self::Failures];

    pub fn label(self) -> String {
        match self {
            Self::Sessions => tr!("usage.tab_sessions"),
            Self::Daily => tr!("usage.tab_daily"),
            Self::Failures => tr!("usage.tab_failures"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Daily => "daily",
            Self::Failures => "failures",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.as_str() == raw.trim())
    }
}

/// Simple is the scan: headline figures and the trend. Details is the audit:
/// ranked breakdowns and the session tables. The two never share a viewport,
/// so the page stays short and each reading has its own hierarchy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UsageMode {
    Simple,
    Details,
}

impl UsageMode {
    pub const ALL: [Self; 2] = [Self::Simple, Self::Details];

    pub fn label(self) -> String {
        match self {
            Self::Simple => tr!("usage.mode_simple"),
            Self::Details => tr!("usage.mode_details"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Details => "details",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == raw.trim())
    }
}

/// Per-tab view state for the breakdown table: each dimension remembers its
/// own search, hidden columns, and page — parity with the sessions table,
/// scoped per dimension because the columns and row sets differ.
#[derive(Clone, Debug, PartialEq)]
pub struct BreakdownView {
    pub search: String,
    pub hidden_columns: Vec<&'static str>,
    /// 1-based.
    pub page: usize,
    pub page_size: usize,
}

impl Default for BreakdownView {
    fn default() -> Self {
        Self {
            search: String::new(),
            hidden_columns: Vec::new(),
            page: 1,
            page_size: DEFAULT_BREAKDOWN_PAGE_SIZE,
        }
    }
}

/// One page of breakdown rows plus the metadata its footer reports (§75). The
/// totals cover the whole filtered set, not just the visible page.
#[derive(Clone, Debug, PartialEq)]
pub struct BreakdownQueryResult {
    pub rows: Vec<GroupRow>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub totals: Totals,
}

/// One page of tool rows plus the metadata its footer reports.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolQueryResult {
    pub rows: Vec<ToolRow>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub calls: u64,
    pub errors: u64,
}

/// Slice one page out of a filtered row set, clamping the page into range.
fn paginate<T: Clone>(rows: &[T], page: usize, page_size: usize) -> (Vec<T>, usize, usize) {
    let total = rows.len();
    let page_size = page_size.max(1);
    let page_count = total.div_ceil(page_size).max(1);
    let page = page.clamp(1, page_count);
    let start = (page - 1) * page_size;
    let page_rows = if start < total {
        rows[start..(start + page_size).min(total)].to_vec()
    } else {
        Vec::new()
    };
    (page_rows, page, total)
}

fn series_point_key(
    sort: SeriesSort,
    metric: ChartMetric,
    latency: LatencyMetric,
    point: &SeriesPoint,
) -> f64 {
    match sort {
        SeriesSort::Time => point.start_ms as f64,
        SeriesSort::Value => {
            if metric == ChartMetric::Latency {
                latency
                    .value(&point.latency)
                    .or_else(|| point.totals.avg_duration_ms())
                    .unwrap_or(0.0)
            } else {
                metric.value(&point.totals)
            }
        }
        SeriesSort::Requests => point.totals.requests as f64,
        SeriesSort::Tokens => point.totals.tokens.total as f64,
    }
}

/// Opens a session in the app's chat surface (installed by `OrbitApp`).
pub type OpenSession = Rc<dyn Fn(&str, &mut Window, &mut App)>;
/// Leaves the usage page (installed by `OrbitApp`).
pub type Close = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct UsagePage {
    scanner: UsageScanner,
    index: Option<Rc<UsageIndex>>,
    snapshot: Option<Rc<UsageSnapshot>>,
    /// The snapshot no longer matches the filter or the index.
    dirty: bool,

    // ── filter + view state ──
    filter: UsageFilter,
    metric: ChartMetric,
    session_sort: SessionSort,
    session_sort_desc: bool,
    bucket_sort: BucketSort,
    bucket_sort_desc: bool,
    series_sort: SeriesSort,
    series_sort_desc: bool,
    /// 1-based usage-over-time table page and its size.
    series_page: usize,
    series_page_size: usize,
    /// Usage-over-time columns the user hid; Time is never in this list.
    series_hidden_columns: Vec<&'static str>,
    /// 1-based Daily table page and its size.
    bucket_page: usize,
    bucket_page_size: usize,
    /// Daily columns the user hid; Date is never in this list.
    bucket_hidden_columns: Vec<&'static str>,
    failure_sort: FailureSort,
    failure_sort_desc: bool,
    /// 1-based Failures table page and its size.
    failure_page: usize,
    failure_page_size: usize,
    /// Failures columns the user hid; When is never in this list.
    failure_hidden_columns: Vec<&'static str>,
    /// 1-based session-table page and its size (§26).
    page: usize,
    page_size: usize,
    /// Session columns the user hid (§41); persisted.
    hidden_columns: Vec<SessionSort>,
    /// Which latency register the response-time chart plots (§20).
    latency_metric: LatencyMetric,
    /// Which column the breakdown data table orders on, and its direction.
    breakdown_sort: BreakdownSort,
    breakdown_sort_desc: bool,
    /// Which dimension the merged breakdown section ranks.
    breakdown_tab: BreakdownTab,
    /// Per-tab breakdown table state: search, hidden columns, and page.
    breakdown_views: HashMap<BreakdownTab, BreakdownView>,
    breakdown_search: Entity<ComposerInput>,
    series_search: Entity<ComposerInput>,
    bucket_search: Entity<ComposerInput>,
    failure_search: Entity<ComposerInput>,
    /// Which table the merged records section shows.
    detail_tab: DetailTab,
    /// Simple (overview) vs Details (breakdown + records).
    usage_mode: UsageMode,

    // ── controls ──
    /// The page's own scroll position. Opening the page always starts at the
    /// top; a refresh never moves it (§82).
    scroll: ScrollHandle,
    search: Entity<ComposerInput>,
    menu: Option<MenuKind>,
    menu_query: Entity<ComposerInput>,
    menu_highlight: usize,
    /// Month shown in the custom-range calendar (epoch ms inside the month).
    calendar_month: Option<i64>,
    /// The calendar is expanded inside the range menu.
    calendar_open: bool,
    /// Draft custom-range bounds, applied when both ends are chosen.
    custom_start: Option<i64>,
    custom_end: Option<i64>,
    /// The date range in force before a calendar day click scoped the page to
    /// that day, so a second click on the same day restores it.
    calendar_restore: Option<DateRange>,
    /// When a popover was dismissed by an outside click; guards the same
    /// gesture's mouse-up from re-opening the menu it just closed.
    dismissed_at: Option<Instant>,

    // ── transient state ──
    hover_bucket: Option<usize>,
    /// The day under the pointer on the calendar heatmap, if any.
    hover_day: Option<usize>,
    loading: bool,
    refreshing: bool,
    last_request: Option<Instant>,
    last_updated_ms: Option<i64>,
    error: Option<String>,
    status: Option<(String, Instant)>,

    /// Per-column width overrides, keyed by table then column id. Empty until
    /// the user drags a divider; the computed plan is the default.
    widths: HashMap<TableKind, HashMap<&'static str, f32>>,
    /// The divider currently being dragged, if any:
    /// `(table, column id, pointer x at drag start, width at drag start)`.
    resizing: Option<(TableKind, &'static str, f32, f32)>,
    /// The session row whose right-click menu is open, if any (§43).
    context_row: Option<usize>,
    /// Re-render when the session search or the menu filter changes: those
    /// inputs live in their own entities, so the page has to watch them.
    _search_sub: Subscription,
    _menu_query_sub: Subscription,
    _breakdown_search_sub: Subscription,
    _series_search_sub: Subscription,
    _bucket_search_sub: Subscription,
    _failure_search_sub: Subscription,
    /// Width of the main area, refreshed by the shell each render.
    main_width: f32,
    /// Leading inset for the page header, refreshed by the shell each render.
    /// Wider when the sessions sidebar is collapsed: the page then owns the
    /// window's left edge, so the header's Back affordance has to clear the OS
    /// window buttons and the titlebar's left controls overlaid at the same
    /// height.
    header_leading: f32,

    on_open_session: Option<OpenSession>,
    on_close: Option<Close>,
}

impl UsagePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let prefs = Prefs::load();
        let now = now_ms();
        let mut range = match RangePreset::parse(&prefs.preset) {
            Some(RangePreset::Custom) => match (prefs.custom_start, prefs.custom_end) {
                // A saved window with nonsense bounds (an epoch stamp, an
                // inverted pair) falls back to a range that shows something.
                (Some(start), Some(end)) if end > start && start > 0 => {
                    DateRange::custom(start, end)
                }
                _ => DateRange::for_preset(RangePreset::Last7, now),
            },
            Some(preset) => DateRange::for_preset(preset, now),
            None => DateRange::for_preset(RangePreset::Last7, now),
        };
        // A persisted relative preset is re-resolved against "now" (the window
        // moves), which is the point of storing the preset and not the range.
        if range.preset != RangePreset::Custom {
            range = DateRange::for_preset(range.preset, now);
        }
        let search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-session-search")
                .with_placeholder_key("page.search_sessions")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let menu_query = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-menu-query")
                .with_placeholder_key("page.filter")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let search_sub = cx.observe(&search, |page: &mut Self, _, cx| {
            // The search narrows the sessions table, so its rows are stale and
            // the page must return to the first page of the new result (§28).
            page.page = 1;
            cx.notify();
        });
        let menu_query_sub = cx.observe(&menu_query, |_, _, cx| cx.notify());
        let breakdown_search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-breakdown-search")
                .with_placeholder_key("page.search_breakdown")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let breakdown_search_sub = cx.observe(&breakdown_search, |_, _, cx| cx.notify());
        let series_search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-series-search")
                .with_placeholder_key("page.search_buckets")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let series_search_sub = cx.observe(&series_search, |page: &mut Self, _, cx| {
            page.series_page = 1;
            cx.notify();
        });
        let bucket_search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-bucket-search")
                .with_placeholder_key("page.search_buckets")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let bucket_search_sub = cx.observe(&bucket_search, |page: &mut Self, _, cx| {
            page.bucket_page = 1;
            cx.notify();
        });
        let failure_search = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("usage-failure-search")
                .with_placeholder_key("page.search_failures")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let failure_search_sub = cx.observe(&failure_search, |page: &mut Self, _, cx| {
            page.failure_page = 1;
            cx.notify();
        });
        Self {
            scanner: UsageScanner::start(),
            index: None,
            snapshot: None,
            dirty: true,
            filter: UsageFilter::new(range),
            metric: ChartMetric::parse(&prefs.metric).unwrap_or(ChartMetric::Tokens),
            session_sort: SessionSort::parse(&prefs.session_sort).unwrap_or(SessionSort::Tokens),
            session_sort_desc: prefs.session_sort_desc,
            bucket_sort: BucketSort::Date,
            bucket_sort_desc: true,
            series_sort: SeriesSort::Time,
            series_sort_desc: false,
            series_page: 1,
            series_page_size: DEFAULT_BREAKDOWN_PAGE_SIZE,
            series_hidden_columns: Vec::new(),
            bucket_page: 1,
            bucket_page_size: DEFAULT_PAGE_SIZE,
            bucket_hidden_columns: Vec::new(),
            failure_sort: FailureSort::When,
            failure_sort_desc: true,
            failure_page: 1,
            failure_page_size: DEFAULT_PAGE_SIZE,
            failure_hidden_columns: Vec::new(),
            page: 1,
            page_size: prefs.page_size,
            hidden_columns: prefs.hidden_columns.clone(),
            latency_metric: LatencyMetric::parse(&prefs.latency_metric)
                .unwrap_or(LatencyMetric::Average),
            breakdown_sort: BreakdownSort::parse(&prefs.breakdown_sort)
                .unwrap_or(BreakdownSort::Tokens),
            breakdown_sort_desc: prefs.breakdown_sort_desc,
            breakdown_tab: BreakdownTab::parse(&prefs.breakdown_tab)
                .unwrap_or(BreakdownTab::Models),
            breakdown_views: HashMap::new(),
            breakdown_search,
            series_search,
            bucket_search,
            failure_search,
            detail_tab: DetailTab::parse(&prefs.detail_tab).unwrap_or(DetailTab::Sessions),
            usage_mode: UsageMode::parse(&prefs.usage_mode).unwrap_or(UsageMode::Simple),
            scroll: ScrollHandle::new(),
            search,
            menu: None,
            menu_query,
            menu_highlight: 0,
            calendar_month: None,
            calendar_open: false,
            custom_start: prefs.custom_start,
            custom_end: prefs.custom_end,
            calendar_restore: None,
            dismissed_at: None,
            hover_bucket: None,
            hover_day: None,
            loading: true,
            refreshing: false,
            last_request: None,
            last_updated_ms: None,
            error: None,
            status: None,
            widths: HashMap::new(),
            resizing: None,
            context_row: None,
            _search_sub: search_sub,
            _menu_query_sub: menu_query_sub,
            _breakdown_search_sub: breakdown_search_sub,
            _series_search_sub: series_search_sub,
            _bucket_search_sub: bucket_search_sub,
            _failure_search_sub: failure_search_sub,
            main_width: 1000.,
            header_leading: 12.,
            on_open_session: None,
            on_close: None,
        }
    }

    pub fn set_open_session(&mut self, handler: OpenSession) {
        self.on_open_session = Some(handler);
    }

    pub fn set_on_close(&mut self, handler: Close) {
        self.on_close = Some(handler);
    }

    /// Enter the page: load on first open, and quietly re-check the store when
    /// the last scan is old enough to matter.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        let have_none = self.index.is_none();
        let stale = self
            .last_updated_ms
            .is_none_or(|at| now_ms() - at >= STALE_AFTER.as_millis() as i64);
        self.maybe_scan(have_none || stale, cx);
    }

    pub fn close(&mut self, _cx: &mut Context<Self>) {
        self.menu = None;
        self.context_row = None;
        self.hover_bucket = None;
        self.hover_day = None;
    }

    /// The user asked for fresh numbers.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.maybe_scan(true, cx);
    }

    /// The session store changed under us (pi wrote a session while the page
    /// is open). Rate-limited so a streaming agent doesn't spin the scanner.
    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        let due = self
            .last_request
            .is_none_or(|last| Instant::now().duration_since(last) >= RESCAN_INTERVAL);
        self.maybe_scan(due, cx);
    }

    /// Called from the app heartbeat: drain finished scans and recompute.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(index) = self.scanner.take_index() {
            self.loading = false;
            self.refreshing = false;
            self.last_updated_ms = Some(index.scanned_at_ms);
            self.error = if index.is_empty() && index.unreadable_files > 0 {
                Some(tr!(
                    "usage.unreadable_files",
                    count = index.unreadable_files
                ))
            } else {
                None
            };
            let index = Rc::new(index);
            self.prune_filter(&index);
            self.index = Some(index);
            self.dirty = true;
            changed = true;
        }
        if self.dirty {
            self.recompute();
            changed = true;
        }
        if let Some((_, at)) = &self.status {
            if at.elapsed() >= Duration::from_secs(6) {
                self.status = None;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn maybe_scan(&mut self, condition: bool, cx: &mut Context<Self>) {
        if !condition {
            return;
        }
        self.scanner.request_scan();
        self.last_request = Some(Instant::now());
        if self.index.is_some() {
            self.refreshing = true;
        } else {
            self.loading = true;
        }
        cx.notify();
    }

    /// Drop filter ids that the new index does not contain. A session file
    /// removed while it was scoped is the realistic case: the scope has to
    /// clear itself (and say so) rather than leave the page empty forever.
    fn prune_filter(&mut self, index: &UsageIndex) {
        let mut filter = self.filter.clone();
        let before = filter.clone();
        if filter
            .session
            .is_some_and(|id| index.try_session(id).is_none())
        {
            filter.session = None;
            self.status = Some((tr!("usage.session_scope_cleared"), Instant::now()));
        }
        filter
            .workspaces
            .retain(|id| index.workspaces.len() > *id as usize);
        filter
            .providers
            .retain(|id| index.providers.len() > *id as usize);
        filter.models.retain(|id| index.models.len() > *id as usize);
        if filter != before {
            self.filter = filter;
        }
    }

    /// Recompute the snapshot. Only ever called when `dirty` (§51).
    fn recompute(&mut self) {
        self.dirty = false;
        let Some(index) = self.index.clone() else {
            self.snapshot = None;
            return;
        };
        self.snapshot = Some(Rc::new(UsageSnapshot::compute(&index, &self.filter)));
    }

    // ── reads used by the view ─────────────────────────────────────────────

    /// The scroll handle the body tracks, so opening the page can start at the
    /// top without disturbing a refresh.
    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub fn snapshot(&self) -> Option<&Rc<UsageSnapshot>> {
        self.snapshot.as_ref()
    }

    /// Set by `OrbitApp` on every render: the main area's width in points.
    pub fn set_main_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.main_width - width).abs() > 0.5 {
            self.main_width = width;
            cx.notify();
        }
    }

    /// The header's leading inset. The shell leaves it at the normal page
    /// padding unless a page override widens it.
    pub fn header_leading(&self) -> f32 {
        self.header_leading
    }

    /// The width of the main area the shell gave us. The page lays itself out
    /// against this instead of guessing from the window size (§58).
    pub fn main_width(&self) -> f32 {
        self.main_width
    }

    /// The width a table actually gets: the page column is capped at
    /// [`super::view::CONTENT_MAX_W`] and padded, and every table sits inside a
    /// section card (hairline + inner pad). Budgeting against the raw main-area
    /// width would over-commit and squeeze the last column into a scrollbar.
    pub(super) fn table_width(&self) -> f32 {
        /// The card's own left and right borders.
        const CARD_EDGE: f32 = 2.;
        (self.main_width.min(super::view::CONTENT_MAX_W)
            - super::view::PAGE_PAD
            - CARD_EDGE
            - super::view::SECTION_PAD * 2.)
            .max(360.)
    }

    // ── tables ─────────────────────────────────────────────────────────────

    /// The width a column has been dragged to, or its computed default.
    pub fn col_width(&self, table: TableKind, id: &'static str, default: f32) -> f32 {
        self.widths
            .get(&table)
            .and_then(|widths| widths.get(id))
            .copied()
            .unwrap_or(default)
    }

    /// A divider drag began: remember where it started, so every move is
    /// measured from the same origin.
    pub fn begin_resize(&mut self, table: TableKind, id: &'static str, x: f32, width: f32) {
        self.resizing = Some((table, id, x, width));
    }

    /// A divider is being dragged: the column takes the pointer's delta from the
    /// width it had at drag start.
    pub fn drag_resize(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some((table, id, start_x, start_width)) = self.resizing else {
            return;
        };
        let width = (start_width + (x - start_x)).max(super::table::MIN_COL_W);
        self.widths.entry(table).or_default().insert(id, width);
        cx.notify();
    }

    pub fn end_resize(&mut self) {
        self.resizing = None;
    }

    /// The session row whose right-click menu is open, if any.
    pub fn context_row(&self) -> Option<usize> {
        self.context_row
    }

    pub fn open_context_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        self.context_row = Some(row);
        cx.notify();
    }

    pub fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_row.take().is_some() {
            cx.notify();
        }
    }

    /// Failures in the range, resolved against the index. Search, sort, and
    /// pagination happen in [`query_failures`].
    fn resolved_failure_rows(&self) -> Vec<FailureRow> {
        let (Some(index), Some(snapshot)) = (self.index.as_ref(), self.snapshot.as_ref()) else {
            return Vec::new();
        };
        snapshot
            .errors
            .rows
            .iter()
            .map(|row| FailureRow {
                ts_ms: row.ts_ms,
                session: row.session,
                kind: row.kind,
                model: index.model(row.model).label.clone(),
                session_title: {
                    let entry = index.session(row.session);
                    if entry.title.is_empty() {
                        tr!(
                            "usage.session_fallback",
                            id = entry.id.chars().take(8).collect::<String>()
                        )
                    } else {
                        entry.title.clone()
                    }
                },
                message: row.message.clone(),
            })
            .collect()
    }

    pub fn failure_result(&self, cx: &App) -> FailureQueryResult {
        query_failures(
            &self.resolved_failure_rows(),
            &self.failure_search.read(cx).text(),
            self.failure_sort,
            self.failure_sort_desc,
            self.failure_page,
            self.failure_page_size,
        )
    }

    /// A header click on the failures table.
    pub fn set_failure_sort(&mut self, sort: FailureSort, desc: bool, cx: &mut Context<Self>) {
        if self.failure_sort == sort && self.failure_sort_desc == desc {
            return;
        }
        self.failure_sort = sort;
        self.failure_sort_desc = desc;
        self.failure_page = 1;
        cx.notify();
    }

    /// The directory this page reads (pi's own session store).
    pub fn store_path(&self) -> Option<String> {
        Some(self.scanner.store().to_string_lossy().to_string())
    }

    pub fn index(&self) -> Option<&Rc<UsageIndex>> {
        self.index.as_ref()
    }

    pub fn filter(&self) -> &UsageFilter {
        &self.filter
    }

    pub fn metric(&self) -> ChartMetric {
        self.metric
    }

    pub fn is_loading(&self) -> bool {
        self.loading && self.snapshot.is_none()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_ref().map(|(text, _)| text.as_str())
    }

    pub fn last_updated_ms(&self) -> Option<i64> {
        self.last_updated_ms
    }

    pub fn hover_bucket(&self) -> Option<usize> {
        self.hover_bucket
    }

    /// The calendar day under the pointer, if any.
    pub fn hover_day(&self) -> Option<usize> {
        self.hover_day
    }

    pub fn menu(&self) -> Option<MenuKind> {
        self.menu
    }

    pub fn menu_highlight(&self) -> usize {
        self.menu_highlight
    }

    pub fn search(&self) -> &Entity<ComposerInput> {
        &self.search
    }

    pub fn menu_query(&self) -> &Entity<ComposerInput> {
        &self.menu_query
    }

    pub fn calendar_month(&self) -> Option<i64> {
        self.calendar_month
    }

    pub fn calendar_open(&self) -> bool {
        self.calendar_open
    }

    pub fn custom_bounds(&self) -> (Option<i64>, Option<i64>) {
        (self.custom_start, self.custom_end)
    }

    /// The current sessions-table query, assembled from the page's controls.
    pub fn session_query(&self, cx: &App) -> SessionQuery {
        SessionQuery {
            search: self.search.read(cx).text(),
            sort: self.session_sort,
            desc: self.session_sort_desc,
            page: self.page,
            page_size: self.page_size,
        }
    }

    /// One page of sessions plus the totals the footer reports (§26/§75).
    pub fn session_page(&self, cx: &App) -> SessionQueryResult {
        let (Some(index), Some(snapshot)) = (self.index.as_ref(), self.snapshot.as_ref()) else {
            return SessionQueryResult {
                rows: Vec::new(),
                total: 0,
                page: self.page,
                page_size: self.page_size,
            };
        };
        query_sessions(index, snapshot, &self.session_query(cx))
    }

    /// Hidden session columns (§41), for the columns menu.
    pub fn hidden_columns(&self) -> &[SessionSort] {
        &self.hidden_columns
    }

    pub fn column_visible(&self, column: SessionSort) -> bool {
        !self.hidden_columns.contains(&column)
    }

    pub fn latency_metric(&self) -> LatencyMetric {
        self.latency_metric
    }

    /// The breakdown data table's current sort key and direction.
    pub fn breakdown_sort_state(&self) -> (BreakdownSort, bool) {
        (self.breakdown_sort, self.breakdown_sort_desc)
    }

    /// The rows of one breakdown, ordered by the data table's sort. The chart
    /// keeps the aggregate's own token ranking, so the table and the bars never
    /// fight over the same ordering.
    pub fn sorted_breakdown_rows(&self, breakdown: &Breakdown) -> Vec<GroupRow> {
        let mut rows = breakdown.rows.clone();
        let (sort, desc) = (self.breakdown_sort, self.breakdown_sort_desc);
        rows.sort_by(|a, b| {
            let ordering = if sort.is_text() {
                a.label.to_lowercase().cmp(&b.label.to_lowercase())
            } else {
                sort.key(a)
                    .partial_cmp(&sort.key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            };
            let ordering = if desc { ordering.reverse() } else { ordering };
            ordering.then_with(|| a.label.cmp(&b.label))
        });
        rows
    }

    pub fn bucket_result(&self, snapshot: &UsageSnapshot, cx: &App) -> BucketQueryResult {
        query_buckets(
            &snapshot.buckets.rows,
            &self.bucket_search.read(cx).text(),
            self.bucket_sort,
            self.bucket_sort_desc,
            self.bucket_page,
            self.bucket_page_size,
        )
    }

    // ── mutations ──────────────────────────────────────────────────────────

    pub fn set_filter(&mut self, filter: UsageFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        // A narrower or different result set invalidates the current page (§28).
        self.page = 1;
        self.series_page = 1;
        self.bucket_page = 1;
        self.failure_page = 1;
        self.dirty = true;
        self.recompute();
        self.persist();
        cx.notify();
    }

    pub fn set_preset(&mut self, preset: RangePreset, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.range = DateRange::for_preset(preset, now_ms());
        self.custom_start = None;
        self.custom_end = None;
        self.calendar_restore = None;
        self.set_filter(filter, cx);
    }

    pub fn set_metric(&mut self, metric: ChartMetric, cx: &mut Context<Self>) {
        if self.metric == metric {
            return;
        }
        self.metric = metric;
        self.persist();
        cx.notify();
    }

    pub fn set_hover_bucket(&mut self, bucket: Option<usize>, cx: &mut Context<Self>) {
        if self.hover_bucket != bucket {
            self.hover_bucket = bucket;
            cx.notify();
        }
    }

    /// The pointer entered a calendar day (or left the grid).
    pub fn set_hover_day(&mut self, day: Option<usize>, cx: &mut Context<Self>) {
        if self.hover_day != day {
            self.hover_day = day;
            cx.notify();
        }
    }

    pub fn open_menu(
        &mut self,
        menu: Option<MenuKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = menu;
        self.menu_highlight = 0;
        self.calendar_open = false;
        if menu == Some(MenuKind::Range) {
            // Start the calendar on the current range's month.
            self.calendar_month = Some(self.filter.range.start_ms);
        }
        self.menu_query.update(cx, |input, cx| input.clear(cx));
        // Only the multi-select menus own a text field; the range menu keeps
        // focus where it was so Escape/arrows still reach the page.
        if matches!(
            menu,
            Some(MenuKind::Workspace) | Some(MenuKind::Provider) | Some(MenuKind::Model)
        ) {
            let handle = self.menu_query.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
        cx.notify();
    }

    /// Toggle a chip's popover, ignoring the mouse-up half of a click that
    /// just dismissed a popover outside its bounds.
    pub fn toggle_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        let just_dismissed = self
            .dismissed_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
        if self.menu == Some(kind) || just_dismissed {
            self.menu = None;
            self.calendar_open = false;
            cx.notify();
            return;
        }
        self.open_menu(Some(kind), window, cx);
    }

    /// Close whatever popover is open (no window needed — used after an
    /// action taken from inside a menu).
    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_none() {
            return;
        }
        self.menu = None;
        self.calendar_open = false;
        cx.notify();
    }

    /// Any click outside a popover closes it.
    pub fn dismiss_menus(&mut self, cx: &mut Context<Self>) {
        let had_menu = self.menu.take().is_some();
        let had_context = self.context_row.take().is_some();
        if !had_menu && !had_context {
            return;
        }
        if had_menu {
            self.dismissed_at = Some(Instant::now());
        }
        self.calendar_open = false;
        cx.notify();
    }

    /// Expand the custom-range calendar inside the range menu.
    ///
    /// The draft window is seeded from what the dashboard is currently
    /// showing, and the calendar opens where the data is — an all-time range
    /// starts at the epoch, which would open the picker in 1970.
    pub fn open_calendar(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.menu = Some(MenuKind::Range);
        self.calendar_open = true;
        let range = self.filter.range.clone();
        let (mut start, mut end) = (range.start_ms, (range.end_ms - 1).max(range.start_ms));
        if range.preset == RangePreset::All {
            if let Some((oldest, newest)) = self.index.as_ref().and_then(|index| index.span()) {
                start = oldest;
                end = newest;
            }
        }
        self.custom_start = Some(local_day_start(start));
        self.custom_end = Some(local_day_start(end));
        self.calendar_month = Some(end);
        cx.notify();
    }

    pub fn set_calendar_month(&mut self, month_ms: i64, cx: &mut Context<Self>) {
        self.calendar_month = Some(month_ms);
        cx.notify();
    }

    pub fn pick_day(&mut self, day_ms: i64, cx: &mut Context<Self>) {
        // First click sets the start, second completes the range; a third
        // starts over.
        match (self.custom_start, self.custom_end) {
            (_, Some(_)) => {
                self.custom_start = Some(day_ms);
                self.custom_end = None;
                cx.notify();
            }
            (Some(start), None) if day_ms < start => {
                self.custom_start = Some(day_ms);
                self.custom_end = Some(start);
                self.apply_custom(cx);
            }
            (Some(_), None) => {
                self.custom_end = Some(day_ms);
                self.apply_custom(cx);
            }
            (None, None) => {
                self.custom_start = Some(day_ms);
                cx.notify();
            }
        }
    }

    fn apply_custom(&mut self, cx: &mut Context<Self>) {
        if let (Some(start), Some(end)) = (self.custom_start, self.custom_end) {
            let mut filter = self.filter.clone();
            filter.range = DateRange::custom(start, end);
            self.calendar_restore = None;
            self.set_filter(filter, cx);
            self.menu = None;
            self.calendar_open = false;
        }
    }

    /// A calendar day click: scope the whole page to that day, and scope back
    /// to the range it came from on a second click. The activity calendar is
    /// its own trailing year, so this is how a day is drilled into.
    pub fn scope_to_day(&mut self, day_ms: i64, cx: &mut Context<Self>) {
        let day_range = DateRange::custom(day_ms, day_ms);
        let mut filter = self.filter.clone();
        if filter.range == day_range {
            let Some(previous) = self.calendar_restore.take() else {
                return;
            };
            filter.range = previous;
            self.custom_start = None;
            self.custom_end = None;
        } else {
            self.calendar_restore = Some(filter.range.clone());
            filter.range = day_range;
            self.custom_start = Some(day_ms);
            self.custom_end = Some(day_ms);
        }
        filter.focus = None;
        self.set_filter(filter, cx);
    }

    pub fn set_menu_highlight(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.menu_highlight != ix {
            self.menu_highlight = ix;
            cx.notify();
        }
    }

    /// Toggle one value in a multi-select dimension.
    pub fn toggle_filter_value(&mut self, kind: MenuKind, id: u16, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        let list = match kind {
            MenuKind::Workspace => &mut filter.workspaces,
            MenuKind::Provider => &mut filter.providers,
            MenuKind::Model => &mut filter.models,
            MenuKind::Range
            | MenuKind::Export
            | MenuKind::PageSize
            | MenuKind::Columns
            | MenuKind::BreakdownColumns
            | MenuKind::BreakdownPageSize
            | MenuKind::SeriesColumns
            | MenuKind::SeriesPageSize
            | MenuKind::BucketColumns
            | MenuKind::BucketPageSize
            | MenuKind::FailureColumns
            | MenuKind::FailurePageSize => return,
        };
        if let Some(ix) = list.iter().position(|value| *value == id) {
            list.remove(ix);
        } else {
            list.push(id);
        }
        self.set_filter(filter, cx);
    }

    pub fn select_all(&mut self, kind: MenuKind, ids: &[u16], cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        match kind {
            MenuKind::Workspace => filter.workspaces = ids.to_vec(),
            MenuKind::Provider => filter.providers = ids.to_vec(),
            MenuKind::Model => filter.models = ids.to_vec(),
            MenuKind::Range
            | MenuKind::Export
            | MenuKind::PageSize
            | MenuKind::Columns
            | MenuKind::BreakdownColumns
            | MenuKind::BreakdownPageSize
            | MenuKind::SeriesColumns
            | MenuKind::SeriesPageSize
            | MenuKind::BucketColumns
            | MenuKind::BucketPageSize
            | MenuKind::FailureColumns
            | MenuKind::FailurePageSize => return,
        }
        self.set_filter(filter, cx);
    }

    pub fn clear_dimension(&mut self, kind: MenuKind, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        match kind {
            MenuKind::Workspace => filter.workspaces.clear(),
            MenuKind::Provider => filter.providers.clear(),
            MenuKind::Model => filter.models.clear(),
            MenuKind::Range
            | MenuKind::Export
            | MenuKind::PageSize
            | MenuKind::Columns
            | MenuKind::BreakdownColumns
            | MenuKind::BreakdownPageSize
            | MenuKind::SeriesColumns
            | MenuKind::SeriesPageSize
            | MenuKind::BucketColumns
            | MenuKind::BucketPageSize
            | MenuKind::FailureColumns
            | MenuKind::FailurePageSize => return,
        }
        self.set_filter(filter, cx);
    }

    pub fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.calendar_restore = None;
        self.set_filter(self.filter.cleared(), cx);
    }

    pub fn set_session_scope(&mut self, session: Option<u16>, cx: &mut Context<Self>) {
        self.context_row = None;
        let mut filter = self.filter.clone();
        filter.session = session;
        self.set_filter(filter, cx);
    }

    pub fn set_errors_only(&mut self, only: bool, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.errors_only = only;
        if only {
            filter.cached_only = false;
        }
        self.set_filter(filter, cx);
    }

    pub fn set_cached_only(&mut self, only: bool, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.cached_only = only;
        if only {
            filter.errors_only = false;
        }
        self.set_filter(filter, cx);
    }

    /// A header click on the sessions table. The header cycles through three
    /// states, so it hands the page both the key and the direction instead of
    /// toggling.
    pub fn set_session_sort(&mut self, sort: SessionSort, desc: bool, cx: &mut Context<Self>) {
        if self.session_sort == sort && self.session_sort_desc == desc {
            return;
        }
        self.session_sort = sort;
        self.session_sort_desc = desc;
        // Re-ordering changes what lands on page 1, so return to it (§29).
        self.page = 1;
        self.persist();
        cx.notify();
    }

    /// Same for the breakdown table.
    pub fn set_bucket_sort(&mut self, sort: BucketSort, desc: bool, cx: &mut Context<Self>) {
        if self.bucket_sort == sort && self.bucket_sort_desc == desc {
            return;
        }
        self.bucket_sort = sort;
        self.bucket_sort_desc = desc;
        self.bucket_page = 1;
        cx.notify();
    }

    pub fn set_series_sort(&mut self, sort: SeriesSort, desc: bool, cx: &mut Context<Self>) {
        if self.series_sort == sort && self.series_sort_desc == desc {
            return;
        }
        self.series_sort = sort;
        self.series_sort_desc = desc;
        self.series_page = 1;
        cx.notify();
    }

    pub fn series_search(&self) -> &Entity<ComposerInput> {
        &self.series_search
    }

    pub fn series_page_size(&self) -> usize {
        self.series_page_size
    }

    pub fn series_hidden_columns(&self) -> &[&'static str] {
        &self.series_hidden_columns
    }

    pub fn series_column_visible(&self, column: SeriesSort) -> bool {
        column == SeriesSort::Time || !self.series_hidden_columns.contains(&column.id())
    }

    pub fn toggle_series_column(&mut self, column: SeriesSort, cx: &mut Context<Self>) {
        if column == SeriesSort::Time {
            return;
        }
        let id = column.id();
        match self
            .series_hidden_columns
            .iter()
            .position(|hidden| *hidden == id)
        {
            Some(ix) => {
                self.series_hidden_columns.remove(ix);
            }
            None => self.series_hidden_columns.push(id),
        }
        cx.notify();
    }

    pub fn show_all_series_columns(&mut self, cx: &mut Context<Self>) {
        if self.series_hidden_columns.is_empty() {
            return;
        }
        self.series_hidden_columns.clear();
        cx.notify();
    }

    pub fn set_series_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.max(1);
        if self.series_page == page {
            return;
        }
        self.series_page = page;
        cx.notify();
    }

    pub fn set_series_page_size(&mut self, size: usize, cx: &mut Context<Self>) {
        if !PAGE_SIZES.contains(&size) || self.series_page_size == size {
            return;
        }
        self.series_page_size = size;
        self.series_page = 1;
        self.menu = None;
        cx.notify();
    }

    /// Filter → sort → paginate the usage-over-time buckets.
    pub fn series_result(&self, snapshot: &UsageSnapshot, cx: &App) -> SeriesQueryResult {
        query_series(
            &snapshot.series.points,
            &self.series_search.read(cx).text(),
            self.series_sort,
            self.series_sort_desc,
            self.metric,
            self.latency_metric,
            self.series_page,
            self.series_page_size,
        )
    }

    pub fn bucket_search(&self) -> &Entity<ComposerInput> {
        &self.bucket_search
    }

    pub fn bucket_page_size(&self) -> usize {
        self.bucket_page_size
    }

    pub fn bucket_hidden_columns(&self) -> &[&'static str] {
        &self.bucket_hidden_columns
    }

    pub fn bucket_column_visible(&self, column: BucketSort) -> bool {
        column == BucketSort::Date || !self.bucket_hidden_columns.contains(&column.id())
    }

    pub fn toggle_bucket_column(&mut self, column: BucketSort, cx: &mut Context<Self>) {
        if column == BucketSort::Date {
            return;
        }
        let id = column.id();
        match self
            .bucket_hidden_columns
            .iter()
            .position(|hidden| *hidden == id)
        {
            Some(ix) => {
                self.bucket_hidden_columns.remove(ix);
            }
            None => self.bucket_hidden_columns.push(id),
        }
        cx.notify();
    }

    pub fn show_all_bucket_columns(&mut self, cx: &mut Context<Self>) {
        if self.bucket_hidden_columns.is_empty() {
            return;
        }
        self.bucket_hidden_columns.clear();
        cx.notify();
    }

    pub fn set_bucket_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.max(1);
        if self.bucket_page == page {
            return;
        }
        self.bucket_page = page;
        cx.notify();
    }

    pub fn set_bucket_page_size(&mut self, size: usize, cx: &mut Context<Self>) {
        if !PAGE_SIZES.contains(&size) || self.bucket_page_size == size {
            return;
        }
        self.bucket_page_size = size;
        self.bucket_page = 1;
        self.menu = None;
        cx.notify();
    }

    pub fn failure_search(&self) -> &Entity<ComposerInput> {
        &self.failure_search
    }

    pub fn failure_page_size(&self) -> usize {
        self.failure_page_size
    }

    pub fn failure_hidden_columns(&self) -> &[&'static str] {
        &self.failure_hidden_columns
    }

    pub fn failure_column_visible(&self, column: FailureSort) -> bool {
        column == FailureSort::When || !self.failure_hidden_columns.contains(&column.id())
    }

    pub fn toggle_failure_column(&mut self, column: FailureSort, cx: &mut Context<Self>) {
        if column == FailureSort::When {
            return;
        }
        let id = column.id();
        match self
            .failure_hidden_columns
            .iter()
            .position(|hidden| *hidden == id)
        {
            Some(ix) => {
                self.failure_hidden_columns.remove(ix);
            }
            None => self.failure_hidden_columns.push(id),
        }
        cx.notify();
    }

    pub fn show_all_failure_columns(&mut self, cx: &mut Context<Self>) {
        if self.failure_hidden_columns.is_empty() {
            return;
        }
        self.failure_hidden_columns.clear();
        cx.notify();
    }

    pub fn set_failure_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.max(1);
        if self.failure_page == page {
            return;
        }
        self.failure_page = page;
        cx.notify();
    }

    pub fn set_failure_page_size(&mut self, size: usize, cx: &mut Context<Self>) {
        if !PAGE_SIZES.contains(&size) || self.failure_page_size == size {
            return;
        }
        self.failure_page_size = size;
        self.failure_page = 1;
        self.menu = None;
        cx.notify();
    }

    // ── sessions table controls (§26/§29/§30/§41) ──────────────────────────

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// The sessions table's current sort key and direction.
    pub fn session_sort_state(&self) -> (SessionSort, bool) {
        (self.session_sort, self.session_sort_desc)
    }

    /// The breakdown table's current sort key and direction.
    pub fn bucket_sort_state(&self) -> (BucketSort, bool) {
        (self.bucket_sort, self.bucket_sort_desc)
    }

    pub fn series_sort_state(&self) -> (SeriesSort, bool) {
        (self.series_sort, self.series_sort_desc)
    }

    /// The failures table's current sort key and direction.
    pub fn failure_sort_state(&self) -> (FailureSort, bool) {
        (self.failure_sort, self.failure_sort_desc)
    }

    /// Jump to a page, clamped to the current result set.
    pub fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.max(1);
        if self.page == page {
            return;
        }
        self.page = page;
        cx.notify();
    }

    pub fn set_page_size(&mut self, size: usize, cx: &mut Context<Self>) {
        if !PAGE_SIZES.contains(&size) || self.page_size == size {
            return;
        }
        self.page_size = size;
        self.page = 1;
        self.persist();
        self.menu = None;
        cx.notify();
    }

    /// Toggle a session column's visibility (§41), persisted.
    pub fn toggle_column(&mut self, column: SessionSort, cx: &mut Context<Self>) {
        match self
            .hidden_columns
            .iter()
            .position(|entry| *entry == column)
        {
            Some(ix) => {
                self.hidden_columns.remove(ix);
            }
            None => self.hidden_columns.push(column),
        }
        self.persist();
        cx.notify();
    }

    /// Restore every session column (§41).
    pub fn show_all_columns(&mut self, cx: &mut Context<Self>) {
        if self.hidden_columns.is_empty() {
            return;
        }
        self.hidden_columns.clear();
        self.persist();
        cx.notify();
    }

    // ── chart controls (§9/§20/§52) ────────────────────────────────────────

    /// Scope the page to one chart bucket, or clear it (§9/§47).
    pub fn set_focus(&mut self, focus: Option<TimeFocus>, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.focus = focus;
        self.set_filter(filter, cx);
    }

    pub fn set_latency_metric(&mut self, metric: LatencyMetric, cx: &mut Context<Self>) {
        if self.latency_metric == metric {
            return;
        }
        self.latency_metric = metric;
        self.persist();
        cx.notify();
    }

    /// A header click on the breakdown data table.
    pub fn set_breakdown_sort(&mut self, sort: BreakdownSort, desc: bool, cx: &mut Context<Self>) {
        if self.breakdown_sort == sort && self.breakdown_sort_desc == desc {
            return;
        }
        self.breakdown_sort = sort;
        self.breakdown_sort_desc = desc;
        self.persist();
        cx.notify();
    }

    // ── merged section selectors ───────────────────────────────────────────

    pub fn breakdown_tab(&self) -> BreakdownTab {
        self.breakdown_tab
    }

    pub fn set_breakdown_tab(&mut self, tab: BreakdownTab, cx: &mut Context<Self>) {
        if self.breakdown_tab == tab {
            return;
        }
        // Each dimension keeps its own search text; stash the outgoing tab's and
        // restore the incoming one, so switching back feels like returning.
        let text = self.breakdown_search.read(cx).text();
        self.breakdown_views
            .entry(self.breakdown_tab)
            .or_default()
            .search = text;
        self.breakdown_tab = tab;
        let next = self.breakdown_view(tab).search;
        self.breakdown_search
            .update(cx, |input, cx| input.set_text(next, cx));
        self.persist();
        cx.notify();
    }

    /// The per-tab breakdown view state (a clone; callers read it, the page owns it).
    pub fn breakdown_view(&self, tab: BreakdownTab) -> BreakdownView {
        self.breakdown_views.get(&tab).cloned().unwrap_or_default()
    }

    pub fn breakdown_search(&self) -> &Entity<ComposerInput> {
        &self.breakdown_search
    }

    /// One dimension's rows narrowed by the open tab's search, in the table's
    /// current sort order. Not paged — the chart and the table both build on it.
    fn filtered_breakdown_rows(&self, breakdown: &Breakdown, cx: &App) -> Vec<GroupRow> {
        let needle = self.breakdown_search.read(cx).text().trim().to_lowercase();
        let mut rows = self.sorted_breakdown_rows(breakdown);
        if !needle.is_empty() {
            rows.retain(|row| {
                row.label.to_lowercase().contains(&needle)
                    || row
                        .sub
                        .as_deref()
                        .is_some_and(|sub| sub.to_lowercase().contains(&needle))
            });
        }
        rows
    }

    /// The ranked top rows of one token dimension for its chart: the filtered
    /// set, still in rank order, trimmed to `limit`.
    pub fn breakdown_top(&self, breakdown: &Breakdown, limit: usize, cx: &App) -> Vec<GroupRow> {
        let mut rows = self.filtered_breakdown_rows(breakdown, cx);
        rows.truncate(limit);
        rows
    }

    /// The filtered, paged rows of one token dimension, with totals over the
    /// whole filtered set.
    pub fn breakdown_result(&self, breakdown: &Breakdown, cx: &App) -> BreakdownQueryResult {
        let view = self.breakdown_view(self.breakdown_tab);
        let rows = self.filtered_breakdown_rows(breakdown, cx);
        let mut totals = Totals::default();
        for row in &rows {
            totals.add(&row.totals);
        }
        let (rows, page, total) = paginate(&rows, view.page, view.page_size);
        BreakdownQueryResult {
            rows,
            total,
            page,
            page_size: view.page_size.max(1),
            totals,
        }
    }

    /// One dimension's tool rows narrowed by the open tab's search.
    fn filtered_tool_rows(&self, snapshot: &UsageSnapshot, cx: &App) -> Vec<ToolRow> {
        let needle = self.breakdown_search.read(cx).text().trim().to_lowercase();
        snapshot
            .tools
            .rows
            .iter()
            .filter(|row| {
                needle.is_empty()
                    || row.label.to_lowercase().contains(&needle)
                    || row.class.as_str().to_lowercase().contains(&needle)
            })
            .cloned()
            .collect()
    }

    /// The ranked top tools for the chart.
    pub fn tools_top(&self, snapshot: &UsageSnapshot, limit: usize, cx: &App) -> Vec<ToolRow> {
        let mut rows = self.filtered_tool_rows(snapshot, cx);
        rows.truncate(limit);
        rows
    }

    /// The filtered, paged rows of the Tools dimension.
    pub fn tools_result(&self, snapshot: &UsageSnapshot, cx: &App) -> ToolQueryResult {
        let view = self.breakdown_view(self.breakdown_tab);
        let filtered = self.filtered_tool_rows(snapshot, cx);
        let calls = filtered.iter().map(|row| row.calls).sum();
        let errors = filtered.iter().map(|row| row.errors).sum();
        let (rows, page, total) = paginate(&filtered, view.page, view.page_size);
        ToolQueryResult {
            rows,
            total,
            page,
            page_size: view.page_size.max(1),
            calls,
            errors,
        }
    }

    pub fn set_breakdown_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let tab = self.breakdown_tab;
        self.breakdown_views.entry(tab).or_default().page = page.max(1);
        cx.notify();
    }

    pub fn set_breakdown_page_size(&mut self, size: usize, cx: &mut Context<Self>) {
        if !PAGE_SIZES.contains(&size) {
            return;
        }
        let tab = self.breakdown_tab;
        let view = self.breakdown_views.entry(tab).or_default();
        view.page_size = size;
        view.page = 1;
        self.menu = None;
        cx.notify();
    }

    /// The hideable columns of the open breakdown dimension, as `(id, label)`.
    pub fn breakdown_column_options(&self) -> Vec<(&'static str, String)> {
        match self.breakdown_tab {
            BreakdownTab::Tools => vec![
                ("calls", tr!("usage.col_calls")),
                ("errors", tr!("usage.metric_errors")),
                ("avg", tr!("usage.col_avg")),
                ("max", tr!("usage.col_max")),
            ],
            _ => vec![
                ("requests", tr!("usage.metric_requests")),
                ("input", tr!("usage.slice_input")),
                ("output", tr!("usage.slice_output")),
                ("cache", tr!("usage.metric_cache")),
                ("tokens", tr!("usage.metric_tokens")),
                ("share", tr!("usage.col_share")),
            ],
        }
    }

    pub fn breakdown_column_visible(&self, id: &'static str) -> bool {
        !self
            .breakdown_view(self.breakdown_tab)
            .hidden_columns
            .contains(&id)
    }

    /// The hidden column ids of the open breakdown dimension.
    pub fn breakdown_hidden_columns(&self) -> Vec<&'static str> {
        self.breakdown_view(self.breakdown_tab).hidden_columns
    }

    pub fn toggle_breakdown_column(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let tab = self.breakdown_tab;
        let view = self.breakdown_views.entry(tab).or_default();
        match view.hidden_columns.iter().position(|hidden| *hidden == id) {
            Some(ix) => {
                view.hidden_columns.remove(ix);
            }
            None => view.hidden_columns.push(id),
        }
        cx.notify();
    }

    pub fn show_all_breakdown_columns(&mut self, cx: &mut Context<Self>) {
        let tab = self.breakdown_tab;
        if let Some(view) = self.breakdown_views.get_mut(&tab) {
            view.hidden_columns.clear();
        }
        cx.notify();
    }

    pub fn detail_tab(&self) -> DetailTab {
        self.detail_tab
    }

    pub fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if self.detail_tab == tab {
            return;
        }
        self.detail_tab = tab;
        self.persist();
        cx.notify();
    }

    pub fn usage_mode(&self) -> UsageMode {
        self.usage_mode
    }

    pub fn set_usage_mode(&mut self, mode: UsageMode, cx: &mut Context<Self>) {
        if self.usage_mode == mode {
            return;
        }
        self.usage_mode = mode;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.persist();
        cx.notify();
    }

    /// Open a session in the chat surface.
    pub fn open_session(&mut self, window: &mut Window, cx: &mut Context<Self>, session: u16) {
        let Some(index) = &self.index else {
            return;
        };
        let id = index.session(session).id.clone();
        if let Some(handler) = self.on_open_session.clone() {
            handler(&id, window, cx);
        }
    }

    pub fn close_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.on_close.clone() {
            handler(window, cx);
        }
    }

    /// Write the filtered data to disk. The save panel is opened
    /// asynchronously — a blocking native dialog would pump a nested
    /// main-thread modal loop while GPUI holds this entity's mutable borrow
    /// and abort on the next task that updates the page.
    pub fn export(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let (Some(index), Some(snapshot)) = (self.index.clone(), self.snapshot.clone()) else {
            return;
        };
        let filter = self.filter.clone();
        let search = self.search.read(cx).text();
        let default_name = match format {
            ExportFormat::Csv => "orbit-usage.csv",
            ExportFormat::Json => "orbit-usage.json",
        };
        cx.spawn(async move |this, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_file_name(default_name)
                .save_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let body = match format {
                ExportFormat::Csv => super::view::export_csv(&index, &filter),
                ExportFormat::Json => super::view::export_json(&index, &snapshot, &search),
            };
            let result = std::fs::write(&path, body);
            let _ = this.update(cx, |page, cx| {
                page.status = Some(match result {
                    Ok(()) => (
                        tr!(
                            "usage.exported",
                            what = match format {
                                ExportFormat::Csv => tr!("usage.export_filtered_requests"),
                                ExportFormat::Json => tr!("usage.export_current_view"),
                            },
                            file = path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.to_string_lossy().to_string())
                        ),
                        Instant::now(),
                    ),
                    Err(err) => (tr!("usage.export_failed", error = err), Instant::now()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn persist(&self) {
        Prefs {
            preset: self.filter.range.preset.as_str().to_string(),
            custom_start: self.custom_start,
            custom_end: self.custom_end,
            metric: self.metric.as_str().to_string(),
            session_sort: self.session_sort.as_str().to_string(),
            session_sort_desc: self.session_sort_desc,
            page_size: self.page_size,
            latency_metric: self.latency_metric.as_str().to_string(),
            hidden_columns: self.hidden_columns.clone(),
            breakdown_tab: self.breakdown_tab.as_str().to_string(),
            breakdown_sort: self.breakdown_sort.as_str().to_string(),
            breakdown_sort_desc: self.breakdown_sort_desc,
            detail_tab: self.detail_tab.as_str().to_string(),
            usage_mode: self.usage_mode.as_str().to_string(),
        }
        .persist();
    }
}

/// Persisted view preferences (§61). Data-derived state is never persisted;
/// neither is anything transient.
struct Prefs {
    preset: String,
    custom_start: Option<i64>,
    custom_end: Option<i64>,
    metric: String,
    session_sort: String,
    session_sort_desc: bool,
    page_size: usize,
    latency_metric: String,
    hidden_columns: Vec<SessionSort>,
    breakdown_tab: String,
    breakdown_sort: String,
    breakdown_sort_desc: bool,
    detail_tab: String,
    usage_mode: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            preset: String::new(),
            custom_start: None,
            custom_end: None,
            metric: String::new(),
            session_sort: String::new(),
            session_sort_desc: false,
            page_size: DEFAULT_PAGE_SIZE,
            latency_metric: String::new(),
            hidden_columns: Vec::new(),
            breakdown_tab: String::new(),
            breakdown_sort: String::new(),
            breakdown_sort_desc: true,
            detail_tab: String::new(),
            usage_mode: String::new(),
        }
    }
}

impl Prefs {
    fn path() -> PathBuf {
        crate::platform::home_dir()
            .join(".orbit-pi")
            .join("usage.json")
    }

    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::path()) else {
            return Self {
                session_sort: SessionSort::Tokens.as_str().to_string(),
                session_sort_desc: true,
                metric: ChartMetric::Tokens.as_str().to_string(),
                page_size: DEFAULT_PAGE_SIZE,
                ..Default::default()
            };
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let page_size = value
            .get("page_size")
            .and_then(Value::as_u64)
            .map(|size| size as usize)
            .filter(|size| PAGE_SIZES.contains(size))
            .unwrap_or(DEFAULT_PAGE_SIZE);
        let hidden_columns = value
            .get("hidden_columns")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(SessionSort::parse)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            preset: text("preset").unwrap_or_default(),
            custom_start: value.get("custom_start").and_then(Value::as_i64),
            custom_end: value.get("custom_end").and_then(Value::as_i64),
            metric: text("metric").unwrap_or_else(|| ChartMetric::Tokens.as_str().to_string()),
            session_sort: text("session_sort")
                .unwrap_or_else(|| SessionSort::Tokens.as_str().to_string()),
            session_sort_desc: value
                .get("session_sort_desc")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            page_size,
            latency_metric: text("latency_metric")
                .unwrap_or_else(|| LatencyMetric::Average.as_str().to_string()),
            hidden_columns,
            breakdown_tab: text("breakdown_tab").unwrap_or_default(),
            breakdown_sort: text("breakdown_sort").unwrap_or_default(),
            breakdown_sort_desc: value
                .get("breakdown_sort_desc")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            detail_tab: text("detail_tab").unwrap_or_default(),
            usage_mode: text("usage_mode").unwrap_or_default(),
        }
    }

    fn persist(self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let payload = serde_json::json!({
            "preset": self.preset,
            "custom_start": self.custom_start,
            "custom_end": self.custom_end,
            "metric": self.metric,
            "session_sort": self.session_sort,
            "session_sort_desc": self.session_sort_desc,
            "page_size": self.page_size,
            "latency_metric": self.latency_metric,
            "hidden_columns": self
                .hidden_columns
                .iter()
                .map(|column| column.as_str())
                .collect::<Vec<_>>(),
            "breakdown_tab": self.breakdown_tab,
            "breakdown_sort": self.breakdown_sort,
            "breakdown_sort_desc": self.breakdown_sort_desc,
            "detail_tab": self.detail_tab,
            "usage_mode": self.usage_mode,
        });
        let _ = std::fs::write(path, payload.to_string());
    }
}
