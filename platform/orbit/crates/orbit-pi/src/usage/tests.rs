//! Aggregation tests.
//!
//! These pin the semantics the page depends on: what a request counts, how the
//! four token buckets combine, how cache hit rate is computed, how filters
//! compose, how the comparison window is chosen, and how the result is ordered
//! (determinism). Everything is built on a hand-made [`UsageIndex`], so the
//! math is tested without touching the disk.

use super::aggregate::{
    BucketRow, ChartMetric, Direction, LatencyMetric, LatencyStats, SeriesPoint, Totals,
    UsageSnapshot,
};
use super::model::*;
use super::page::{
    query_buckets, query_failures, query_series, query_sessions, BucketSort, SeriesSort,
    SessionQuery, SessionSort,
};
use super::table::{FailureRow, FailureSort};

/// A tiny builder for synthetic indexes.
#[derive(Default)]
struct Fixture {
    index: UsageIndex,
}

impl Fixture {
    fn workspace(&mut self, path: &str) -> u16 {
        if let Some(ix) = self.index.workspaces.iter().position(|w| w.path == path) {
            return ix as u16;
        }
        self.index.workspaces.push(WorkspaceEntry {
            path: path.to_string(),
            label: path.rsplit('/').next().unwrap_or(path).to_string(),
        });
        (self.index.workspaces.len() - 1) as u16
    }

    fn model(&mut self, provider: &str, id: &str) -> u16 {
        let provider_ix = match self.index.providers.iter().position(|p| p.id == provider) {
            Some(ix) => ix as u16,
            None => {
                self.index.providers.push(ProviderEntry {
                    id: provider.to_string(),
                    label: provider_label(provider),
                });
                (self.index.providers.len() - 1) as u16
            }
        };
        if let Some(ix) = self
            .index
            .models
            .iter()
            .position(|m| m.id == id && m.provider == provider_ix)
        {
            return ix as u16;
        }
        self.index.models.push(ModelEntry {
            provider: provider_ix,
            id: id.to_string(),
            label: model_label(id),
            priced: false,
        });
        (self.index.models.len() - 1) as u16
    }

    fn session(&mut self, workspace: &str, title: &str, started_ms: i64, turns: Vec<i64>) -> u16 {
        let workspace = self.workspace(workspace);
        self.index.sessions.push(SessionEntry {
            id: format!("session-{}", self.index.sessions.len()),
            title: title.to_string(),
            workspace,
            started_ms,
            ended_ms: started_ms,
            turns,
        });
        (self.index.sessions.len() - 1) as u16
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        &mut self,
        session: u16,
        model: (&str, &str),
        ts_ms: i64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> &mut Self {
        self.request_full(
            session,
            model,
            ts_ms,
            input,
            output,
            cache_read,
            cache_write,
            None,
            None,
            Outcome::Stop,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn request_full(
        &mut self,
        session: u16,
        model: (&str, &str),
        ts_ms: i64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost_usd: Option<f64>,
        duration_ms: Option<u32>,
        outcome: Outcome,
    ) -> &mut Self {
        let model = self.model(model.0, model.1);
        if cost_usd.is_some_and(|cost| cost > 0.0) {
            self.index.models[model as usize].priced = true;
        }
        let ended = &mut self.index.sessions[session as usize].ended_ms;
        *ended = (*ended).max(ts_ms);
        self.index.requests.push(UsageRecord {
            ts_ms,
            session,
            model,
            tokens: TokenCounts::from_buckets(input, output, cache_read, cache_write, None),
            reasoning: None,
            cost_usd,
            duration_ms,
            outcome,
        });
        self
    }

    fn tool(&mut self, session: u16, ts_ms: i64, tool: &str, duration_ms: Option<u32>, ok: bool) {
        let tool_ix = match self.index.tools.iter().position(|t| t.id == tool) {
            Some(ix) => ix as u16,
            None => {
                self.index.tools.push(ToolEntry {
                    id: tool.to_string(),
                    label: tool.to_string(),
                    class: ToolClass::of(tool),
                });
                (self.index.tools.len() - 1) as u16
            }
        };
        let model = self.index.requests.last().map(|r| r.model).unwrap_or(0);
        self.index.tool_runs.push(ToolRun {
            ts_ms,
            session,
            model,
            tool: tool_ix,
            duration_ms,
            ok,
        });
    }

    fn error(&mut self, session: u16, ts_ms: i64, message: &str) {
        let model = self.index.requests.last().map(|r| r.model).unwrap_or(0);
        self.index.errors.push(ErrorRow {
            ts_ms,
            session,
            model,
            kind: ErrorKind::Provider,
            message: message.to_string(),
        });
    }

    /// Finish with records in timestamp order, as the scanner would leave them.
    fn build(mut self) -> UsageIndex {
        self.index.requests.sort_by(|a, b| {
            a.ts_ms
                .cmp(&b.ts_ms)
                .then_with(|| a.session.cmp(&b.session))
        });
        self.index
            .tool_runs
            .sort_by(|a, b| a.ts_ms.cmp(&b.ts_ms).then_with(|| a.tool.cmp(&b.tool)));
        self.index
    }
}

/// A fixed local-time anchor: 2026-09-11 12:00 local.
fn anchor() -> i64 {
    let day = local_day_start(1_787_000_000_000);
    day + 12 * 3_600_000
}

fn day_start(ms: i64) -> i64 {
    local_day_start(ms)
}

fn scope(index: &UsageIndex, range: DateRange) -> UsageSnapshot {
    UsageSnapshot::compute(index, &UsageFilter::new(range))
}

#[test]
fn basic_aggregation_sums_input_and_output() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "fix oauth", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude-sonnet"),
        now - 60_000,
        100,
        50,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert_eq!(snapshot.summary.totals.requests, 1);
    assert_eq!(snapshot.summary.totals.tokens.input, 100);
    assert_eq!(snapshot.summary.totals.tokens.output, 50);
    assert_eq!(snapshot.summary.totals.tokens.total, 150);
}

#[test]
fn requests_group_by_local_day_across_midnight() {
    let now = anchor();
    let midnight = day_start(now);
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "late night", midnight - 3_600_000, vec![]);
    // 23:30 the previous day, 00:30 and 23:00 today.
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight - 1_800_000,
        10,
        1,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight + 1_800_000,
        20,
        2,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight + 23 * 3_600_000,
        30,
        3,
        0,
        0,
    );
    let index = fixture.build();

    let today = scope(
        &index,
        DateRange {
            preset: RangePreset::Today,
            start_ms: midnight,
            end_ms: midnight + 24 * 3_600_000,
        },
    );
    assert_eq!(
        today.summary.totals.requests, 2,
        "two requests fall on today"
    );
    assert_eq!(today.summary.totals.tokens.total, 55);
    assert_eq!(today.buckets.rows.len(), 1);
    assert_eq!(today.series.points.len(), 24, "today buckets by hour");
    assert_eq!(today.series.granularity, Granularity::Hour);
}

#[test]
fn workspace_and_model_grouping() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let orbit = fixture.session("/Users/dev/orbit", "orbit work", now - 7_200_000, vec![]);
    let site = fixture.session("/Users/dev/website", "site work", now - 3_600_000, vec![]);
    fixture.request(
        orbit,
        ("anthropic", "claude-sonnet"),
        now - 300_000,
        1_000,
        200,
        100,
        10,
    );
    fixture.request(orbit, ("openai", "gpt-5"), now - 200_000, 500, 100, 0, 0);
    fixture.request(site, ("openai", "gpt-5"), now - 100_000, 2_000, 400, 0, 0);
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    // Workspaces: orbit 1_910 (1_310 claude + 600 gpt), website 2_400.
    assert_eq!(snapshot.workspaces.rows.len(), 2);
    assert_eq!(snapshot.workspaces.rows[0].label, "website");
    assert_eq!(snapshot.workspaces.rows[0].totals.tokens.total, 2_400);
    assert_eq!(snapshot.workspaces.rows[1].label, "orbit");
    assert_eq!(snapshot.workspaces.totals.tokens.total, 4_310);

    // Models: gpt-5 (3_000) leads claude-sonnet (1_310).
    assert_eq!(snapshot.models.rows.len(), 2);
    assert_eq!(snapshot.models.rows[0].label, "gpt-5");
    assert_eq!(snapshot.models.rows[0].sub.as_deref(), Some("Openai"));
    assert_eq!(snapshot.models.rows[0].totals.tokens.total, 3_000);
    assert_eq!(snapshot.models.rows[0].totals.requests, 2);

    // Providers hold their share of the same totals.
    assert_eq!(snapshot.providers.rows.len(), 2);
    assert_eq!(snapshot.providers.rows[0].label, "Openai");
    assert_eq!(snapshot.providers.rows[0].totals.tokens.total, 3_000);
    assert_eq!(snapshot.providers.totals.tokens.total, 4_310);

    // Shares are computed against the same denominator the rows sum to.
    let sum: f64 = snapshot.models.rows.iter().map(|row| row.share).sum();
    assert!((sum - 1.0).abs() < 1e-9, "shares sum to 1, got {sum}");
}

#[test]
fn cache_math_matches_the_documented_formula() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "cache", now - 3_600_000, vec![]);
    // Uncached prompt 250, cached 750 → 75% hit rate.
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        250,
        100,
        750,
        40,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert_eq!(snapshot.cache.cache_read, 750);
    assert_eq!(snapshot.cache.cache_write, 40);
    assert_eq!(snapshot.cache.uncached_input, 250);
    assert_eq!(snapshot.cache.cached_requests, 1);
    let rate = snapshot.cache.hit_rate.expect("hit rate");
    // Cache writes are prompt tokens too: 750 reads over 250 + 750 + 40.
    assert!(
        (rate - 750.0 / 1_040.0 * 100.0).abs() < 1e-6,
        "750 / 1040, got {rate}"
    );
    assert!(snapshot.cache.is_available());
}

#[test]
fn cache_is_unavailable_without_cache_tokens() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "local model", now - 3_600_000, vec![]);
    fixture.request(session, ("ollama", "local"), now - 60_000, 500, 100, 0, 0);
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    // A 0% hit rate would be a made-up number: no prompt tokens were cached-capable.
    assert!(snapshot.cache.hit_rate.is_none());
    assert!(!snapshot.cache.is_available());
    assert_eq!(snapshot.cache.uncached_input, 500);
}

#[test]
fn filters_compose_across_dimensions() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let orbit = fixture.session("/Users/dev/orbit", "orbit", now - 7_200_000, vec![]);
    let site = fixture.session("/Users/dev/website", "site", now - 3_600_000, vec![]);
    fixture.request(orbit, ("anthropic", "claude"), now - 300_000, 100, 10, 0, 0);
    fixture.request(orbit, ("openai", "gpt"), now - 200_000, 200, 20, 0, 0);
    fixture.request(site, ("anthropic", "claude"), now - 100_000, 400, 40, 0, 0);
    let index = fixture.build();

    let range = DateRange::for_preset(RangePreset::Last7, now);
    let claude = index.models.iter().position(|m| m.id == "claude").unwrap() as u16;
    let openai = index
        .providers
        .iter()
        .position(|p| p.id == "openai")
        .unwrap() as u16;
    let orbit_ix = index
        .workspaces
        .iter()
        .position(|w| w.label == "orbit")
        .unwrap() as u16;

    let mut filter = UsageFilter::new(range.clone());
    filter.models.push(claude);
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert_eq!(snapshot.summary.totals.requests, 2);
    assert_eq!(snapshot.summary.totals.tokens.total, 550);

    // Provider + workspace together: only orbit's gpt request survives.
    let mut filter = UsageFilter::new(range.clone());
    filter.providers.push(openai);
    filter.workspaces.push(orbit_ix);
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert_eq!(snapshot.summary.totals.requests, 1);
    assert_eq!(snapshot.summary.totals.tokens.total, 220);

    // Workspace scope: both of orbit's requests.
    let mut filter = UsageFilter::new(range.clone());
    filter.workspaces.push(orbit_ix);
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert_eq!(snapshot.summary.totals.requests, 2);
    assert_eq!(snapshot.summary.totals.tokens.total, 330);

    // Date range: only the newest request.
    let narrow = DateRange {
        preset: RangePreset::Custom,
        start_ms: now - 150_000,
        end_ms: now,
    };
    let snapshot = scope(&index, narrow);
    assert_eq!(snapshot.summary.totals.requests, 1);
    assert_eq!(snapshot.summary.totals.tokens.total, 440);

    // Error and cache toggles.
    let mut filter = UsageFilter::new(range.clone());
    filter.errors_only = true;
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert!(snapshot.is_empty());
    let mut filter = UsageFilter::new(range);
    filter.cached_only = true;
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert!(snapshot.is_empty());
}

#[test]
fn no_matches_is_distinct_from_no_data() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "session", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        100,
        10,
        0,
        0,
    );
    let index = fixture.build();

    // A range with no data at all.
    let empty = scope(
        &index,
        DateRange {
            preset: RangePreset::Custom,
            start_ms: now - 30 * 86_400_000,
            end_ms: now - 20 * 86_400_000,
        },
    );
    assert!(empty.is_empty());
    assert!(!empty.filtered_out(), "nothing in range at all");

    // Data in range, excluded by a filter.
    let mut filter = UsageFilter::new(DateRange::for_preset(RangePreset::Last7, now));
    filter.workspaces.push(99);
    let filtered = UsageSnapshot::compute(&index, &filter);
    assert!(filtered.is_empty());
    assert!(filtered.filtered_out());
    assert_eq!(filtered.requests_in_range, 1);

    // A *model* filter that matches nothing must also read as "nothing
    // matches", even though the range's prompts still exist: prompts are not
    // model calls and do not keep the page alive.
    let mut fixture = Fixture::default();
    let session = fixture.session(
        "/tmp/orbit",
        "prompt only",
        now - 3_600_000,
        vec![now - 1_000],
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        100,
        10,
        0,
        0,
    );
    let index = fixture.build();
    let mut filter = UsageFilter::new(DateRange::for_preset(RangePreset::Last7, now));
    filter.models.push(999);
    let snapshot = UsageSnapshot::compute(&index, &filter);
    assert_eq!(snapshot.summary.turns, 1, "the prompt is still in range");
    assert!(snapshot.is_empty(), "but there is no usage to show");
    assert!(snapshot.filtered_out());
    assert!(snapshot.requests_in_range > 0);
}

#[test]
fn comparison_uses_the_preceding_window() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "compare", now - 20 * 86_400_000, vec![]);
    // Yesterday: 100 tokens. Today: 150 tokens.
    let midnight = day_start(now);
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight - 3_600_000,
        80,
        20,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight + 3_600_000,
        100,
        50,
        0,
        0,
    );
    let index = fixture.build();

    let today = UsageSnapshot::compute(
        &index,
        &UsageFilter::new(DateRange {
            preset: RangePreset::Today,
            start_ms: midnight,
            end_ms: midnight + 12 * 3_600_000,
        }),
    );
    let previous = today.previous.as_ref().expect("previous window");
    assert_eq!(previous.totals.tokens.total, 100);
    let delta = today.delta(ChartMetric::Tokens);
    assert!(!delta.unavailable);
    assert_eq!(delta.direction, Direction::Up);
    assert!((delta.pct.expect("percent") - 50.0).abs() < 1e-6);

    // No history before the window → no comparison at all.
    let early = UsageSnapshot::compute(
        &index,
        &UsageFilter::new(DateRange {
            preset: RangePreset::Custom,
            start_ms: midnight - 4 * 86_400_000,
            end_ms: midnight - 2 * 86_400_000,
        }),
    );
    assert!(early.previous.is_none(), "no baseline, no comparison");
    assert!(early.delta(ChartMetric::Tokens).unavailable);
}

#[test]
fn partial_data_keeps_what_is_known() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "partial", now - 3_600_000, vec![]);
    // A priced request with latency, and an unpriced one without.
    fixture.request_full(
        session,
        ("anthropic", "claude"),
        now - 300_000,
        1_000,
        200,
        500,
        0,
        Some(0.42),
        Some(4_000),
        Outcome::Stop,
    );
    fixture.request_full(
        session,
        ("ollama", "local"),
        now - 100_000,
        300,
        50,
        0,
        0,
        None,
        None,
        Outcome::Error,
    );
    fixture.error(session, now - 100_000, "provider error");
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert_eq!(snapshot.summary.totals.requests, 2);
    assert_eq!(snapshot.summary.totals.errors, 1);
    // Cost covers half the requests, and says so.
    assert!((snapshot.summary.totals.cost_usd - 0.42).abs() < 1e-9);
    assert!((snapshot.summary.totals.cost_coverage() - 0.5).abs() < 1e-9);
    // Latency averages only over what was measured.
    assert_eq!(snapshot.latency.samples, 1);
    assert_eq!(snapshot.summary.totals.avg_duration_ms(), Some(4_000.0));
    assert!(
        !snapshot.latency.has_percentiles(),
        "one sample is not a p95"
    );
    // The error surface is populated.
    assert_eq!(snapshot.errors.provider, 1);
    assert_eq!(snapshot.errors.rows.len(), 1);
    assert_eq!(snapshot.errors.rows[0].message, "provider error");
}

#[test]
fn tools_count_calls_errors_and_durations() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "tools", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 900_000,
        100,
        10,
        0,
        0,
    );
    fixture.tool(session, now - 800_000, "bash", Some(5_000), true);
    fixture.tool(session, now - 700_000, "bash", Some(1_000), false);
    fixture.tool(session, now - 600_000, "read", Some(50), true);
    fixture.tool(session, now - 500_000, "edit", None, true);
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert_eq!(snapshot.summary.tool_runs, 4);
    assert_eq!(snapshot.summary.bash_runs, 2);
    assert_eq!(snapshot.summary.tool_errors, 1);
    let bash = snapshot
        .tools
        .rows
        .iter()
        .find(|row| row.label == "bash")
        .expect("bash row");
    assert_eq!(bash.calls, 2);
    assert_eq!(bash.errors, 1);
    assert_eq!(bash.avg_duration_ms(), Some(3_000.0));
    assert_eq!(bash.max_ms, 5_000);
    // Families group the same calls.
    let terminal = snapshot
        .tools
        .by_class
        .iter()
        .find(|(class, _)| *class == ToolClass::Terminal)
        .map(|(_, count)| *count)
        .unwrap_or(0);
    assert_eq!(terminal, 2);
    assert_eq!(snapshot.tools.calls, 4);
    assert_eq!(snapshot.tools.errors, 1);
}

#[test]
fn granularity_follows_the_window() {
    let now = anchor();
    let cases = [
        (1.0, Granularity::Hour),
        (3.0, Granularity::Day),
        (30.0, Granularity::Day),
        (120.0, Granularity::Week),
        (800.0, Granularity::Month),
    ];
    for (days, expected) in cases {
        let range = DateRange {
            preset: RangePreset::Custom,
            start_ms: now - (days * 86_400_000.0) as i64,
            end_ms: now,
        };
        assert_eq!(range.granularity(), expected, "{days} days");
    }
    // The bucket table coarsens relative to the chart on long ranges.
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "long", now - 200 * 86_400_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 100 * 86_400_000,
        10,
        1,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(
        &index,
        DateRange {
            preset: RangePreset::Custom,
            start_ms: now - 200 * 86_400_000,
            end_ms: now,
        },
    );
    assert_eq!(snapshot.series.granularity, Granularity::Week);
    assert_eq!(snapshot.buckets.granularity, Granularity::Week);
}

#[test]
fn turns_are_counted_as_prompts() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session(
        "/tmp/orbit",
        "turns",
        now - 3_600_000,
        vec![now - 3_000_000, now - 1_000_000],
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 2_900_000,
        100,
        10,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 900_000,
        200,
        20,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    assert_eq!(snapshot.summary.turns, 2);
    assert_eq!(snapshot.summary.totals.requests, 2);
}

#[test]
fn empty_dataset_produces_an_empty_snapshot() {
    let index = UsageIndex::default();
    let now = anchor();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert!(snapshot.is_empty());
    assert!(!snapshot.filtered_out());
    assert_eq!(snapshot.summary.totals.requests, 0);
    assert_eq!(snapshot.summary.totals.tokens.total, 0);
    assert!(snapshot.models.rows.is_empty());
    assert!(snapshot.workspaces.rows.is_empty());
    assert!(snapshot.sessions.is_empty());
    assert!(snapshot.errors.rows.is_empty());
    assert!(snapshot.cache.hit_rate.is_none());
    assert!(snapshot.latency.avg_ms.abs() < f64::EPSILON);
    assert!(snapshot.insights.is_empty());
    assert!(snapshot
        .series
        .points
        .iter()
        .all(|p| p.totals.requests == 0));
}

#[test]
fn snapshots_are_deterministic() {
    let now = anchor();
    let build = || {
        let mut fixture = Fixture::default();
        for (ix, workspace) in ["/tmp/a", "/tmp/b", "/tmp/c"].iter().enumerate() {
            let session = fixture.session(workspace, "s", now - 3_600_000, vec![]);
            fixture.request(
                session,
                ("anthropic", "claude"),
                now - (ix as i64 + 1) * 60_000,
                100,
                10,
                0,
                0,
            );
            fixture.request(
                session,
                ("openai", "gpt"),
                now - (ix as i64 + 1) * 30_000,
                100,
                10,
                0,
                0,
            );
        }
        fixture.build()
    };
    let first = scope(&build(), DateRange::for_preset(RangePreset::Last7, now));
    let second = scope(&build(), DateRange::for_preset(RangePreset::Last7, now));
    assert_eq!(first, second);
    // Ordering is by value then label, never by hash iteration.
    // Equal totals: the tiebreak is the label, and the two runs agree.
    let labels: Vec<&str> = first
        .models
        .rows
        .iter()
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(labels, vec!["claude", "gpt"]);
    assert_eq!(
        first.models.rows[0].totals.tokens.total,
        first.models.rows[1].totals.tokens.total
    );
}

#[test]
fn large_dataset_stays_linear_enough() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let sessions: Vec<u16> = (0..40)
        .map(|ix| {
            fixture.session(
                &format!("/tmp/workspace-{ix}"),
                "bulk",
                now - 86_400_000,
                vec![],
            )
        })
        .collect();
    for ix in 0..60_000u64 {
        let session = sessions[(ix % sessions.len() as u64) as usize];
        fixture.request(
            session,
            ("anthropic", if ix % 2 == 0 { "claude" } else { "opus" }),
            now - (60_000_000 - ix as i64 * 900),
            1_000 + ix,
            100,
            ix % 500,
            10,
        );
    }
    let index = fixture.build();
    let started = std::time::Instant::now();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    let elapsed = started.elapsed();
    assert_eq!(snapshot.summary.totals.requests, 60_000);
    assert!(
        elapsed.as_millis() < 1_500,
        "60k records should aggregate in well under a second, took {elapsed:?}"
    );
}

#[test]
fn csv_export_quotes_and_respects_the_filter() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session(
        "/tmp/orbit",
        "fix, the \"oauth\" flow",
        now - 3_600_000,
        vec![],
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        100,
        50,
        0,
        0,
    );
    let session = fixture.session("/tmp/other", "other", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        900,
        50,
        0,
        0,
    );
    let index = fixture.build();

    let mut filter = UsageFilter::new(DateRange::for_preset(RangePreset::Last7, now));
    filter.workspaces.push(
        index
            .workspaces
            .iter()
            .position(|w| w.label == "orbit")
            .unwrap() as u16,
    );
    let csv = super::view::export_csv(&index, &filter);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 2, "header + one filtered row");
    assert!(lines[0].starts_with("timestamp,session_id,session,"));
    assert!(lines[1].contains("\"fix, the \"\"oauth\"\" flow\""));
    assert!(lines[1].contains(",100,50,0,0,150,"), "row: {}", lines[1]);
}

#[test]
fn json_export_carries_the_aggregates_including_unavailable_metrics() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "export", now - 3_600_000, vec![]);
    fixture.request_full(
        session,
        ("ollama", "local"),
        now - 60_000,
        100,
        50,
        0,
        0,
        None,
        None,
        Outcome::Stop,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    let json: serde_json::Value =
        serde_json::from_str(&super::view::export_json(&index, &snapshot, "")).expect("valid json");

    assert_eq!(json["summary"]["requests"], 1);
    assert_eq!(json["summary"]["total"], 150);
    assert_eq!(json["summary"]["priced_requests"], 0);
    assert!(json["summary"]["avg_duration_ms"].is_null());
    assert!(json["summary"]["cache_hit_rate"].is_null());
    assert_eq!(json["previous"], serde_json::Value::Null);
    assert_eq!(json["sessions"][0]["title"], "export");
    assert_eq!(json["range"]["granularity"], "day");
}

#[test]
fn session_rows_rank_by_tokens_and_carry_their_model() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let small = fixture.session(
        "/tmp/orbit",
        "small",
        now - 7_200_000,
        vec![now - 7_000_000],
    );
    let big = fixture.session("/tmp/orbit", "big", now - 3_600_000, vec![now - 3_500_000]);
    fixture.request(
        small,
        ("anthropic", "claude"),
        now - 6_000_000,
        100,
        10,
        0,
        0,
    );
    fixture.request(big, ("openai", "gpt"), now - 3_000_000, 5_000, 500, 0, 0);
    fixture.tool(big, now - 2_900_000, "bash", Some(1_000), true);
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert_eq!(snapshot.sessions.len(), 2);
    assert_eq!(snapshot.sessions[0].title, "big");
    assert_eq!(snapshot.sessions[0].top_model, "gpt · Openai");
    assert_eq!(snapshot.sessions[0].tool_runs, 1);
    assert_eq!(snapshot.sessions[0].totals.tokens.total, 5_500);
    // Started at -3_600_000, last request at -3_000_000.
    assert_eq!(snapshot.sessions[0].duration_ms(), 600_000);
    assert_eq!(snapshot.sessions[0].workspace, "orbit");
}

#[test]
fn session_query_filters_searches_sorts_then_paginates() {
    let now = anchor();
    let mut fixture = Fixture::default();
    for (title, tokens) in [("alpha", 100u64), ("beta", 300), ("gamma", 200)] {
        let session = fixture.session("/tmp/orbit", title, now - 3_600_000, vec![]);
        fixture.request(
            session,
            ("anthropic", "claude"),
            now - 60_000,
            tokens,
            0,
            0,
            0,
        );
    }
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    let query = |page, size, search: &str| SessionQuery {
        search: search.into(),
        sort: SessionSort::Tokens,
        desc: true,
        page,
        page_size: size,
    };

    // Sort applies across the whole set before pagination (§74): page 1 holds
    // the largest by tokens, page 2 the next.
    let first = query_sessions(&index, &snapshot, &query(1, 1, ""));
    assert_eq!(first.total, 3);
    assert_eq!(first.rows[0].title, "beta");
    assert_eq!((first.first_row(), first.last_row()), (1, 1));
    let second = query_sessions(&index, &snapshot, &query(2, 1, ""));
    assert_eq!(second.rows[0].title, "gamma");

    // A page past the end clamps to the last real page rather than showing
    // nothing (§29).
    let past = query_sessions(&index, &snapshot, &query(99, 1, ""));
    assert_eq!(past.page, 3);
    assert_eq!(past.rows[0].title, "alpha");

    // Search narrows before pagination.
    let searched = query_sessions(&index, &snapshot, &query(1, 25, "gam"));
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].title, "gamma");

    // Search also matches the session id and the provider (§30).
    let by_id = query_sessions(
        &index,
        &snapshot,
        &SessionQuery {
            search: "session-1".into(),
            ..query(1, 25, "")
        },
    );
    assert_eq!(by_id.total, 1);
    assert_eq!(by_id.rows[0].title, "beta");
    let by_provider = query_sessions(&index, &snapshot, &query(1, 25, "anthropic"));
    assert_eq!(by_provider.total, 3);
}

#[test]
fn time_focus_narrows_without_changing_the_range() {
    let now = anchor();
    let midnight = day_start(now);
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "focus", midnight, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight + 3_600_000,
        100,
        10,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        midnight + 5 * 3_600_000,
        200,
        20,
        0,
        0,
    );
    let index = fixture.build();

    let range = DateRange {
        preset: RangePreset::Today,
        start_ms: midnight,
        end_ms: midnight + 12 * 3_600_000,
    };
    let mut filter = UsageFilter::new(range.clone());
    filter.focus = Some(TimeFocus {
        start_ms: midnight + 3_600_000,
        end_ms: midnight + 4 * 3_600_000,
        granularity: Granularity::Hour,
    });
    let focused = UsageSnapshot::compute(&index, &filter);
    assert_eq!(focused.summary.totals.requests, 1);
    assert_eq!(focused.summary.totals.tokens.total, 110);
    // The range still owns both requests, so the page can say "filtered out".
    assert_eq!(focused.requests_in_range, 2);

    // Clearing the focus restores the full window.
    filter.focus = None;
    let all = UsageSnapshot::compute(&index, &filter);
    assert_eq!(all.summary.totals.requests, 2);
    assert_eq!(all.summary.totals.tokens.total, 330);
}

#[test]
fn latency_metric_switches_between_average_and_percentiles() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "latency", now - 3_600_000, vec![]);
    for ix in 0..25u32 {
        fixture.request_full(
            session,
            ("anthropic", "claude"),
            now - i64::from(ix) * 1_000,
            100,
            10,
            0,
            0,
            None,
            Some(1_000 + ix * 100),
            Outcome::Stop,
        );
    }
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));

    assert!(snapshot.latency.has_percentiles());
    let point = snapshot
        .series
        .points
        .iter()
        .find(|point| point.totals.duration_samples > 0)
        .expect("a bucket with latency samples");
    assert!(point.latency.has_percentiles());
    let average = LatencyMetric::Average
        .value(&point.latency)
        .expect("average");
    let p95 = LatencyMetric::P95.value(&point.latency).expect("p95");
    assert!(
        p95 >= average,
        "p95 {p95} should not trail the mean {average}"
    );

    // A bucket with no samples offers neither a mean nor a percentile.
    assert_eq!(LatencyMetric::Average.value(&LatencyStats::default()), None);
    assert_eq!(LatencyMetric::P99.value(&LatencyStats::default()), None);
}

#[test]
fn insights_only_appear_with_signal() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "quiet", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        100,
        10,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    // A single tiny data point produces no headline.
    assert!(snapshot.insights.is_empty(), "{:?}", snapshot.insights);

    // A dominant model does, once there is enough traffic to mean anything.
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "loud", now - 3_600_000, vec![]);
    for ix in 0..30 {
        fixture.request(
            session,
            ("anthropic", "claude"),
            now - 300_000 + ix * 1_000,
            10_000,
            1_000,
            0,
            0,
        );
    }
    fixture.request(session, ("openai", "gpt"), now - 200_000, 100, 10, 0, 0);
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    assert!(
        snapshot
            .insights
            .iter()
            .any(|insight| insight.text.contains("claude") && insight.text.contains('%')),
        "{:?}",
        snapshot.insights
    );
}

#[test]
fn calendar_is_monday_aligned_and_carries_daily_totals() {
    use chrono::Datelike;

    let now = anchor();
    let today = day_start(now);
    let yesterday = local_day_start(today - 86_400_000);
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "calendar", now - 3_600_000, vec![]);
    // Two requests today, one yesterday.
    fixture.request(
        session,
        ("anthropic", "claude"),
        today + 3_600_000,
        100,
        10,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        today + 7_200_000,
        200,
        20,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        yesterday + 3_600_000,
        50,
        5,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::Last7, now));
    let calendar = &snapshot.calendar;

    assert_eq!(calendar.days.len() % 7, 0, "the grid is whole weeks");
    assert_eq!(calendar.weeks(), calendar.days.len() / 7);
    // Always a trailing year, whatever the date range says.
    assert!(
        calendar.in_range().count() >= 365 && calendar.in_range().count() <= 366,
        "{} in-range days",
        calendar.in_range().count()
    );
    assert!(calendar.weeks() >= 53, "{} week columns", calendar.weeks());
    // The first cell is a Monday (the store's own week boundary).
    let first = local_datetime(calendar.start_ms).unwrap();
    assert_eq!(first.weekday().num_days_from_monday(), 0);
    // Contiguous, one local day apart.
    for pair in calendar.days.windows(2) {
        assert_eq!(
            next_bucket(pair[0].start_ms, Granularity::Day),
            pair[1].start_ms
        );
    }
    let day = |start: i64| {
        calendar
            .days
            .iter()
            .find(|cell| cell.start_ms == start)
            .expect("day is in the grid")
    };
    assert_eq!(day(today).totals.requests, 2);
    assert_eq!(day(today).totals.tokens.total, 330);
    assert_eq!(day(yesterday).totals.requests, 1);
    // Padding days complete the shape but claim no activity.
    for cell in calendar.days.iter().filter(|cell| !cell.in_range) {
        assert_eq!(cell.totals.requests, 0);
        assert_eq!(cell.totals.tokens.total, 0);
    }
}

#[test]
fn calendar_ignores_the_date_range_but_honors_scope() {
    let now = anchor();
    let six_months = 180 * 86_400_000;
    let mut fixture = Fixture::default();
    let old = fixture.session("/tmp/orbit", "old", now - six_months, vec![]);
    let today = fixture.session("/tmp/orbit", "today", now - 3_600_000, vec![]);
    let other = fixture.session("/tmp/other", "other", now - 3_600_000, vec![]);
    fixture.request(
        old,
        ("anthropic", "claude"),
        now - six_months,
        100,
        10,
        0,
        0,
    );
    fixture.request(today, ("anthropic", "claude"), now - 60_000, 200, 20, 0, 0);
    fixture.request(other, ("anthropic", "claude"), now - 120_000, 400, 40, 0, 0);
    let index = fixture.build();
    let workspace = index.session(old).workspace;

    // The range is a single day, but the calendar still reaches six months back.
    let narrow = scope(
        &index,
        DateRange {
            preset: RangePreset::Today,
            start_ms: day_start(now),
            end_ms: day_start(now) + 24 * 3_600_000,
        },
    );
    let requests: u64 = narrow
        .calendar
        .in_range()
        .map(|cell| cell.totals.requests)
        .sum();
    assert_eq!(requests, 3, "the calendar ignores the date range");

    // A workspace filter still narrows it, even though the range does not.
    let filter = UsageFilter {
        range: DateRange::for_preset(RangePreset::Today, now),
        workspaces: vec![workspace],
        ..UsageFilter::new(DateRange::for_preset(RangePreset::Today, now))
    };
    let scoped = UsageSnapshot::compute(&index, &filter);
    let requests: u64 = scoped
        .calendar
        .in_range()
        .map(|cell| cell.totals.requests)
        .sum();
    assert_eq!(
        requests, 2,
        "the workspace scope still applies (excludes /tmp/other)"
    );
}

#[test]
fn calendar_drops_records_older_than_a_year() {
    let now = anchor();
    let mut fixture = Fixture::default();
    let session = fixture.session("/tmp/orbit", "old and new", now - 3_600_000, vec![]);
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 400 * 86_400_000,
        100,
        10,
        0,
        0,
    );
    fixture.request(
        session,
        ("anthropic", "claude"),
        now - 60_000,
        200,
        20,
        0,
        0,
    );
    let index = fixture.build();
    let snapshot = scope(&index, DateRange::for_preset(RangePreset::All, now));
    let calendar = &snapshot.calendar;

    assert_eq!(calendar.days.len() % 7, 0);
    assert!(calendar.weeks() >= 53 && calendar.weeks() <= 54);
    // The 400-day-old request is outside the year; the recent one is inside.
    let requests: u64 = calendar.in_range().map(|cell| cell.totals.requests).sum();
    assert_eq!(requests, 1);
    // The snapshot itself still counts both — the calendar is a calendar, not
    // the page's totals.
    assert_eq!(snapshot.summary.totals.requests, 2);
}

#[test]
fn series_query_filters_sorts_then_paginates() {
    let point = |stamp: &str, start: i64, tokens: u64| SeriesPoint {
        start_ms: start,
        label: stamp.to_string(),
        stamp: stamp.to_string(),
        totals: Totals {
            requests: 1,
            tokens: TokenCounts {
                total: tokens,
                ..Default::default()
            },
            ..Default::default()
        },
        latency: LatencyStats::default(),
    };
    let points = [
        point("Mon", 1, 100),
        point("Tue", 2, 300),
        point("Wed", 3, 200),
    ];
    let query = |page, search: &str| {
        query_series(
            &points,
            search,
            SeriesSort::Tokens,
            true,
            ChartMetric::Tokens,
            LatencyMetric::Average,
            page,
            1,
        )
    };

    let first = query(1, "");
    assert_eq!(first.total, 3);
    assert_eq!(first.rows[0].stamp, "Tue");
    assert_eq!((first.first_row(), first.last_row()), (1, 1));
    let second = query(2, "");
    assert_eq!(second.rows[0].stamp, "Wed");

    let past = query(99, "");
    assert_eq!(past.page, 3);
    assert_eq!(past.rows[0].stamp, "Mon");

    let searched = query(1, "wed");
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].stamp, "Wed");
}

#[test]
fn bucket_query_filters_sorts_then_paginates() {
    let row = |label: &str, start: i64, tokens: u64| BucketRow {
        start_ms: start,
        label: label.to_string(),
        stamp: label.to_string(),
        totals: Totals {
            requests: 1,
            tokens: TokenCounts {
                total: tokens,
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let rows = [row("Mon", 1, 100), row("Tue", 2, 300), row("Wed", 3, 200)];
    let query =
        |page, search: &str| query_buckets(&rows, search, BucketSort::Tokens, true, page, 1);

    let first = query(1, "");
    assert_eq!(first.total, 3);
    assert_eq!(first.rows[0].label, "Tue");
    assert_eq!(first.totals.tokens.total, 600);
    let second = query(2, "");
    assert_eq!(second.rows[0].label, "Wed");

    let past = query(99, "");
    assert_eq!(past.page, 3);
    assert_eq!(past.rows[0].label, "Mon");

    let searched = query(1, "wed");
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].label, "Wed");
    assert_eq!(searched.totals.tokens.total, 200);
}

#[test]
fn failure_query_filters_sorts_then_paginates() {
    let row = |title: &str, ts: i64, message: &str| FailureRow {
        ts_ms: ts,
        session: 0,
        kind: ErrorKind::Provider,
        model: "claude".into(),
        session_title: title.to_string(),
        message: message.to_string(),
    };
    let rows = [
        row("alpha", 1, "timeout"),
        row("beta", 2, "overloaded"),
        row("gamma", 3, "rate limit"),
    ];
    let query =
        |page, search: &str| query_failures(&rows, search, FailureSort::When, true, page, 1);

    let first = query(1, "");
    assert_eq!(first.total, 3);
    assert_eq!(first.rows[0].session_title, "gamma");
    let second = query(2, "");
    assert_eq!(second.rows[0].session_title, "beta");

    let past = query(99, "");
    assert_eq!(past.page, 3);
    assert_eq!(past.rows[0].session_title, "alpha");

    let searched = query(1, "over");
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].session_title, "beta");
}
