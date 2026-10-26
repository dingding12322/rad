//! Reading pi's session store into a [`UsageIndex`].
//!
//! The store is the truth: `~/.pi/agent/sessions/<slug>/<ts>.jsonl`, one file
//! per session, appended to while the agent runs. Nothing here talks to the
//! RPC client — a session that finished months ago is read exactly the same
//! way as the one streaming right now, and the page keeps working while pi is
//! stopped.
//!
//! Scanning is incremental and off-thread. Each file is parsed once and cached
//! against its `(len, mtime)`; a rescan re-reads only the files that changed
//! (typically the single session currently being written), so refreshing while
//! an agent streams costs one small file, not the whole store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::model::*;

/// Session files larger than this are skipped: pi's own store has a
/// long-session ceiling, and a single pathological file must not stall the
/// scanner thread forever.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// One request as read from a file, before interning. `model`/`tool` are
/// file-local indices into [`FileUsage::models`] / [`FileUsage::tools`].
#[derive(Clone, Debug)]
struct RawRequest {
    ts_ms: i64,
    model: u16,
    tokens: TokenCounts,
    reasoning: Option<u64>,
    cost_usd: Option<f64>,
    duration_ms: Option<u32>,
    outcome: Outcome,
}

#[derive(Clone, Debug)]
struct RawToolRun {
    ts_ms: i64,
    model: u16,
    tool: u16,
    duration_ms: Option<u32>,
    ok: bool,
}

#[derive(Clone, Debug)]
struct RawError {
    ts_ms: i64,
    model: u16,
    kind: ErrorKind,
    message: String,
}

/// Everything one session file contributes.
#[derive(Clone, Debug, Default)]
struct FileUsage {
    session_id: String,
    workspace_path: String,
    title: String,
    started_ms: i64,
    ended_ms: i64,
    /// File-local model table: `(provider id, model id)`.
    models: Vec<(String, String)>,
    /// File-local tool-name table.
    tools: Vec<String>,
    requests: Vec<RawRequest>,
    tool_runs: Vec<RawToolRun>,
    errors: Vec<RawError>,
    turns: Vec<i64>,
}

struct CachedFile {
    len: u64,
    mtime_ms: i64,
    usage: FileUsage,
    unreadable: bool,
}

/// Background scanner over pi's session store.
pub struct UsageScanner {
    store: PathBuf,
    requests: Sender<()>,
    indexes: Receiver<UsageIndex>,
    sent: u64,
    received: u64,
}

impl UsageScanner {
    /// Watch pi's default session store.
    pub fn start() -> Self {
        Self::start_in(crate::sessions::sessions_dir())
    }

    /// Watch an explicit store directory (tests, alternate stores).
    pub fn start_in(store: PathBuf) -> Self {
        let (requests, request_rx) = mpsc::channel::<()>();
        let (index_tx, indexes) = mpsc::channel::<UsageIndex>();
        let scan_root = store.clone();
        std::thread::Builder::new()
            .name("orbit-usage-scan".into())
            .spawn(move || {
                let mut cache: HashMap<PathBuf, CachedFile> = HashMap::new();
                while request_rx.recv().is_ok() {
                    // Collapse a burst of requests (open + refresh + watcher)
                    // into a single scan.
                    while request_rx.try_recv().is_ok() {}
                    let index = scan_store(&scan_root, &mut cache);
                    if index_tx.send(index).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self {
            store,
            requests,
            indexes,
            sent: 0,
            received: 0,
        }
    }

    /// Ask for a scan. Cheap and idempotent: repeated calls while one is in
    /// flight collapse into a single scan.
    pub fn request_scan(&mut self) {
        if self.requests.send(()).is_ok() {
            self.sent += 1;
        }
    }

    /// The newest finished index, if the scanner produced one since the last
    /// call. Never blocks.
    pub fn take_index(&mut self) -> Option<UsageIndex> {
        let mut latest = None;
        while let Ok(index) = self.indexes.try_recv() {
            self.received += 1;
            latest = Some(index);
        }
        latest
    }

    pub fn store(&self) -> &Path {
        &self.store
    }
}

/// Walk the store and build an index, reusing cached parses for unchanged
/// files and dropping cache entries for files that disappeared.
fn scan_store(store: &Path, cache: &mut HashMap<PathBuf, CachedFile>) -> UsageIndex {
    let mut paths: Vec<(PathBuf, u64, i64)> = Vec::new();
    if let Ok(groups) = fs::read_dir(store) {
        for group in groups.flatten() {
            let Ok(files) = fs::read_dir(group.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
                {
                    continue;
                }
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                paths.push((path, meta.len(), mtime_ms));
            }
        }
    }
    // Deterministic order: ids must not depend on readdir order.
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let live: std::collections::HashSet<&PathBuf> = paths.iter().map(|(p, _, _)| p).collect();
    cache.retain(|path, _| live.contains(path));

    for (path, len, mtime_ms) in &paths {
        let fresh = cache
            .get(path)
            .is_some_and(|cached| cached.len == *len && cached.mtime_ms == *mtime_ms);
        if fresh {
            continue;
        }
        let (usage, unreadable) = match parse_session_file(path) {
            Some(usage) => (usage, false),
            None => (FileUsage::default(), true),
        };
        cache.insert(
            path.clone(),
            CachedFile {
                len: *len,
                mtime_ms: *mtime_ms,
                usage,
                unreadable,
            },
        );
    }

    let ordered: Vec<&FileUsage> = paths
        .iter()
        .filter_map(|(path, _, _)| cache.get(path).map(|cached| &cached.usage))
        .collect();
    let unreadable = cache.values().filter(|cached| cached.unreadable).count();
    let mut index = build_index(&ordered);
    index.files = paths.len();
    index.unreadable_files = unreadable;
    index.scanned_at_ms = now_ms();
    index
}

/// Intern the per-file records into one index.
fn build_index(files: &[&FileUsage]) -> UsageIndex {
    let mut index = UsageIndex::default();
    let mut workspaces = Interner::default();
    let mut models = Interner::default();
    let mut providers = Interner::default();
    let mut tools = Interner::default();
    let mut models_by_key: HashMap<u16, ModelEntry> = HashMap::new();

    for file in files {
        if file.session_id.is_empty() {
            continue;
        }
        let workspace_key = if file.workspace_path.is_empty() {
            "unknown".to_string()
        } else {
            file.workspace_path.clone()
        };
        let workspace = workspaces.intern(&workspace_key);

        // File-local model table → global ids.
        let mut model_ids = Vec::with_capacity(file.models.len());
        for (provider_id, model_id) in &file.models {
            let provider = providers.intern(provider_id);
            let key = format!("{provider}\u{1}{model_id}");
            let global = models.intern(&key);
            model_ids.push(global);
            models_by_key.entry(global).or_insert_with(|| ModelEntry {
                provider,
                id: model_id.clone(),
                label: model_label(model_id),
                priced: false,
            });
        }
        let mut tool_ids = Vec::with_capacity(file.tools.len());
        for tool in &file.tools {
            let global = tools.intern(tool);
            tool_ids.push(global);
        }

        let session = index.sessions.len() as u16;
        index.sessions.push(SessionEntry {
            id: file.session_id.clone(),
            title: file.title.clone(),
            workspace,
            started_ms: file.started_ms,
            ended_ms: file.ended_ms,
            turns: file.turns.clone(),
        });

        for raw in &file.requests {
            let model = model_ids.get(raw.model as usize).copied().unwrap_or(0);
            if raw.cost_usd.is_some_and(|cost| cost > 0.0) {
                if let Some(entry) = models_by_key.get_mut(&model) {
                    entry.priced = true;
                }
            }
            index.requests.push(UsageRecord {
                ts_ms: raw.ts_ms,
                session,
                model,
                tokens: raw.tokens,
                reasoning: raw.reasoning,
                cost_usd: raw.cost_usd,
                duration_ms: raw.duration_ms,
                outcome: raw.outcome,
            });
        }
        for raw in &file.tool_runs {
            index.tool_runs.push(ToolRun {
                ts_ms: raw.ts_ms,
                session,
                model: model_ids.get(raw.model as usize).copied().unwrap_or(0),
                tool: tool_ids.get(raw.tool as usize).copied().unwrap_or(0),
                duration_ms: raw.duration_ms,
                ok: raw.ok,
            });
        }
        for raw in &file.errors {
            index.errors.push(ErrorRow {
                ts_ms: raw.ts_ms,
                session,
                model: model_ids.get(raw.model as usize).copied().unwrap_or(0),
                kind: raw.kind,
                message: raw.message.clone(),
            });
        }
    }

    let mut model_list: Vec<(u16, ModelEntry)> = models_by_key.into_iter().collect();
    model_list.sort_by_key(|(ix, _)| *ix);
    index.models = model_list.into_iter().map(|(_, entry)| entry).collect();
    index.workspaces = (0..workspaces.len())
        .map(|ix| {
            let key = workspaces.key(ix as u16);
            WorkspaceEntry {
                label: crate::sessions::workspace_label(Path::new(&key)),
                path: key,
            }
        })
        .collect();
    index.providers = (0..providers.len())
        .map(|ix| {
            let id = providers.key(ix as u16);
            ProviderEntry {
                label: provider_label(&id),
                id,
            }
        })
        .collect();
    index.tools = (0..tools.len())
        .map(|ix| {
            let id = tools.key(ix as u16);
            ToolEntry {
                class: ToolClass::of(&id),
                label: id.clone(),
                id,
            }
        })
        .collect();

    // Session ids are referenced by every request, tool run, and error, so the
    // table's order is deliberately left to the aggregation layer: reordering
    // here would silently re-attribute every record. Ordering stays stable and
    // deterministic by construction (files are visited in sorted path order).
    index.requests.sort_by(|a, b| {
        a.ts_ms
            .cmp(&b.ts_ms)
            .then_with(|| a.session.cmp(&b.session))
    });
    index
        .tool_runs
        .sort_by(|a, b| a.ts_ms.cmp(&b.ts_ms).then_with(|| a.tool.cmp(&b.tool)));
    index.errors.sort_by_key(|a| std::cmp::Reverse(a.ts_ms));
    index
}

fn parse_session_file(path: &Path) -> Option<FileUsage> {
    let file = fs::File::open(path).ok()?;
    let mut usage = FileUsage::default();
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let mut first = true;
    // Calls awaiting their result, in issue order: (call id, tool, start, model).
    let mut pending: Vec<(String, u16, i64, u16)> = Vec::new();
    let mut prev_message_ts: Option<i64> = None;
    let mut saw_header = false;

    loop {
        line.clear();
        match std::io::BufRead::read_line(&mut reader, &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let entry_ts = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms);

        if first {
            first = false;
            if value.get("type").and_then(Value::as_str) != Some("session") {
                return None;
            }
            saw_header = true;
            usage.session_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            usage.workspace_path = value
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            usage.started_ms = entry_ts.unwrap_or(0);
            usage.ended_ms = usage.started_ms;
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let message = &value["message"];
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        // pi records the message's own start stamp in epoch ms; the entry
        // timestamp is when the entry was written.
        let message_ts = message.get("timestamp").and_then(Value::as_i64);
        let ts = entry_ts.or(message_ts).unwrap_or(usage.ended_ms);
        if ts > usage.ended_ms {
            usage.ended_ms = ts;
        }

        match role {
            "user" => {
                if usage.title.is_empty() {
                    usage.title = cap(&first_text(message), 80);
                }
                usage.turns.push(ts);
            }
            "assistant" => {
                let provider = message
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let model = message
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let model_ix = intern_pair(&mut usage.models, &provider, &model);
                if let Some(record) =
                    parse_request(message, ts, model_ix, prev_message_ts, message_ts)
                {
                    usage.requests.push(record);
                }
                // Tool calls issued by this message wait for their results.
                for block in message
                    .get("content")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                        continue;
                    }
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    let tool_ix = intern_tool(&mut usage.tools, &name);
                    let call_id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    pending.push((call_id, tool_ix, ts, model_ix));
                }
                if let Some(error) = message.get("errorMessage").and_then(Value::as_str) {
                    usage.errors.push(RawError {
                        ts_ms: ts,
                        model: model_ix,
                        kind: ErrorKind::Provider,
                        message: cap(error, ERROR_MESSAGE_MAX),
                    });
                }
            }
            "toolResult" => {
                let call_id = message
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ok = !message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                // Pair with its call; a result without a matching call (a
                // resumed file) still counts as a run at its own timestamp.
                let matched = pending
                    .iter()
                    .position(|(id, _, _, _)| id == call_id)
                    .map(|ix| pending.remove(ix));
                let (tool_ix, start_ms, model_ix) = match matched {
                    Some((_, tool, started, model)) => (tool, started, model),
                    None => {
                        let name = message
                            .get("toolName")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        (
                            intern_tool(&mut usage.tools, &name),
                            ts,
                            usage.requests.last().map(|r| r.model).unwrap_or(0),
                        )
                    }
                };
                let duration_ms = (ts > start_ms)
                    .then(|| ts - start_ms)
                    .filter(|gap| *gap <= IDLE_GAP_MS)
                    .map(|gap| gap.min(u32::MAX as i64) as u32);
                usage.tool_runs.push(RawToolRun {
                    ts_ms: start_ms,
                    model: model_ix,
                    tool: tool_ix,
                    duration_ms,
                    ok,
                });
                if !ok {
                    let label = usage
                        .tools
                        .get(tool_ix as usize)
                        .cloned()
                        .unwrap_or_else(|| "tool".into());
                    usage.errors.push(RawError {
                        ts_ms: ts,
                        model: model_ix,
                        kind: ErrorKind::Tool,
                        message: tr!("usage.tool_returned_error", tool = label),
                    });
                }
            }
            _ => {}
        }
        prev_message_ts = Some(ts);
    }

    if !saw_header {
        return None;
    }
    // Calls that never produced a result (the run was stopped, or the file was
    // cut off mid-run): the invocation still happened, so it counts, with no
    // duration and no failure verdict.
    for (_, tool, started, model) in pending {
        usage.tool_runs.push(RawToolRun {
            ts_ms: started,
            model,
            tool,
            duration_ms: None,
            ok: true,
        });
    }
    Some(usage)
}

/// One assistant message → one request. Mirrors `orbit_rpc::MessageUsage`: a
/// message with no tokens and no cost is not a request.
fn parse_request(
    message: &Value,
    ts_ms: i64,
    model: u16,
    prev_message_ts: Option<i64>,
    message_ts: Option<i64>,
) -> Option<RawRequest> {
    let usage = message.get("usage")?;
    if usage.is_null() {
        return None;
    }
    let u64_of = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let tokens = TokenCounts::from_buckets(
        u64_of("input"),
        u64_of("output"),
        u64_of("cacheRead"),
        u64_of("cacheWrite"),
        usage.get("totalTokens").and_then(Value::as_u64),
    );
    let cost_usd = usage
        .get("cost")
        .and_then(|cost| {
            cost.get("total")
                .and_then(Value::as_f64)
                .or_else(|| cost.as_f64())
        })
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    // A message with nothing billed and no failure is a streaming artifact, not
    // a request. A *failed* attempt counts even with zero tokens: it was a real
    // round trip to the provider, and the failure is the interesting part.
    let failed = message
        .get("errorMessage")
        .is_some_and(|value| !value.is_null())
        || message.get("stopReason").and_then(Value::as_str) == Some("error");
    if tokens.is_empty() && !cost_usd.is_some_and(|cost| cost > 0.0) && !failed {
        return None;
    }
    let reasoning = usage
        .get("reasoning")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0);
    // Generation time: from the previous message entry (prompt or tool
    // result), falling back to the message's own start stamp. Idle gaps are
    // dropped rather than reported as latency.
    let duration_ms = prev_message_ts
        .or(message_ts)
        .filter(|start| ts_ms > *start)
        .map(|start| ts_ms - start)
        .filter(|gap| *gap > 0 && *gap <= IDLE_GAP_MS)
        .map(|gap| gap.min(u32::MAX as i64) as u32);
    Some(RawRequest {
        ts_ms,
        model,
        tokens,
        reasoning,
        cost_usd,
        duration_ms,
        outcome: Outcome::parse(message.get("stopReason").and_then(Value::as_str)),
    })
}

fn intern_pair(models: &mut Vec<(String, String)>, provider: &str, model: &str) -> u16 {
    if let Some(ix) = models.iter().position(|(p, m)| p == provider && m == model) {
        return ix as u16;
    }
    models.push((provider.to_string(), model.to_string()));
    (models.len() - 1) as u16
}

fn intern_tool(tools: &mut Vec<String>, name: &str) -> u16 {
    if let Some(ix) = tools.iter().position(|t| t == name) {
        return ix as u16;
    }
    tools.push(name.to_string());
    (tools.len() - 1) as u16
}

/// First text block of a user message, newlines flattened.
fn first_text(message: &Value) -> String {
    let content = &message["content"];
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    title_from(&text)
}

/// A session's display title.
///
/// pi records injected blocks (a loaded skill, for instance) as ordinary user
/// messages, so the first message of a session can be a `<skill name="…">`
/// document. Showing that verbatim makes the sessions table unreadable
/// ("<skill name="impeccable" location="/Users/…") — so a recognised block is
/// reduced to what it identifies. Nothing is invented: the fallback is the
/// original text, flattened.
fn title_from(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<skill") {
        return match attribute(rest, "name") {
            Some(name) => tr!("usage.session_skill", name = name),
            None => tr!("usage.session_skill_loaded"),
        };
    }
    if let Some(rest) = trimmed.strip_prefix('<') {
        if let Some(end) = rest.find('>') {
            let inner = rest[end + 1..].trim_start();
            if !inner.is_empty() {
                return inner.replace('\n', " ");
            }
        }
    }
    text.replace('\n', " ")
}

/// The value of `key="…"` inside an already-opened tag body.
fn attribute(tag: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn cap(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push('…');
    out
}

/// Parse an ISO-8601 timestamp (`2026-09-11T14:02:03.412Z`) to epoch ms.
fn parse_iso_ms(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Mirror pi's layout: `<store>/<workspace-slug>/<name>.jsonl`.
    fn write_session(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let dir = dir.join("--tmp-orbit--");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.jsonl"));
        let mut file = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    const HEADER: &str = r#"{"type":"session","version":3,"id":"s1","timestamp":"2026-09-11T09:00:00.000Z","cwd":"/tmp/orbit"}"#;

    /// An assistant message with the given usage. `calls` is the toolCall id +
    /// tool name pairs it issued (JSON), empty for a plain reply.
    fn assistant(
        ts: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        stop: &str,
        calls: &str,
    ) -> String {
        format!(
            r#"{{"type":"message","timestamp":"{ts}","message":{{"role":"assistant","provider":"anthropic","model":"claude-sonnet-4","usage":{{"input":{input},"output":{output},"cacheRead":{cache_read},"cacheWrite":10,"reasoning":5,"totalTokens":{},"cost":{{"total":0.25}}}},"stopReason":"{stop}","content":[{calls}]}}}}"#,
            input + output + cache_read + 10
        )
    }

    #[test]
    fn parses_a_session_into_requests_tools_and_turns() {
        let dir = std::env::temp_dir().join("orbit-usage-parse-test");
        let _ = fs::remove_dir_all(&dir);
        let bash_call =
            r#"{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}"#;
        let read_call =
            r#"{"type":"toolCall","id":"c2","name":"read","arguments":{"path":"/tmp/x"}}"#;
        write_session(
            &dir,
            "a",
            &[
                HEADER,
                r#"{"type":"message","timestamp":"2026-09-11T09:00:05.000Z","message":{"role":"user","content":[{"type":"text","text":"fix the OAuth callback"}],"timestamp":1757581205000}}"#,
                &assistant(
                    "2026-09-11T09:00:10.000Z",
                    100,
                    50,
                    20,
                    "toolUse",
                    bash_call,
                ),
                r#"{"type":"message","timestamp":"2026-09-11T09:00:12.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","isError":false,"content":[{"type":"text","text":"ok"}]}}"#,
                &assistant("2026-09-11T09:00:20.000Z", 10, 5, 0, "stop", read_call),
                r#"{"type":"message","timestamp":"2026-09-11T09:00:21.000Z","message":{"role":"toolResult","toolCallId":"c2","toolName":"read","isError":true,"content":[]}}"#,
            ],
        );
        let usage = parse_session_file(&dir.join("--tmp-orbit--/a.jsonl")).expect("parsed");
        assert_eq!(usage.session_id, "s1");
        assert_eq!(usage.workspace_path, "/tmp/orbit");
        assert_eq!(usage.title, "fix the OAuth callback");
        assert_eq!(usage.turns.len(), 1);
        assert_eq!(usage.requests.len(), 2);
        // First request: 5s after the user message (prompt → reply).
        assert_eq!(usage.requests[0].duration_ms, Some(5_000));
        assert_eq!(usage.requests[0].tokens.input, 100);
        assert_eq!(usage.requests[0].tokens.total, 180);
        assert_eq!(usage.requests[0].tokens.cache_write, 10);
        assert_eq!(usage.requests[0].reasoning, Some(5));
        assert_eq!(usage.requests[0].outcome, Outcome::ToolUse);
        // Second request: 8s after the tool result.
        assert_eq!(usage.requests[1].duration_ms, Some(8_000));
        assert_eq!(usage.requests[1].outcome, Outcome::Stop);
        // Two tool runs: bash (2s, ok) and read (1s, failed).
        assert_eq!(usage.tool_runs.len(), 2);
        assert_eq!(
            usage.tools[usage.tool_runs[0].tool as usize], "bash",
            "the pairing gives the run the call's tool name"
        );
        assert_eq!(usage.tool_runs[0].duration_ms, Some(2_000));
        assert!(usage.tool_runs[0].ok);
        assert_eq!(usage.tool_runs[1].duration_ms, Some(1_000));
        assert!(!usage.tool_runs[1].ok);
        assert_eq!(usage.errors.len(), 1);
        assert_eq!(usage.errors[0].kind, ErrorKind::Tool);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_gaps_are_not_latency() {
        let dir = std::env::temp_dir().join("orbit-usage-idle-test");
        let _ = fs::remove_dir_all(&dir);
        write_session(
            &dir,
            "idle",
            &[
                HEADER,
                r#"{"type":"message","timestamp":"2026-09-11T09:00:00.000Z","message":{"role":"user","content":"hi","timestamp":1}}"#,
                // 3 hours later: a slept laptop, not a 3-hour generation.
                &assistant("2026-09-11T12:00:00.000Z", 10, 5, 0, "stop", ""),
            ],
        );
        let usage = parse_session_file(&dir.join("--tmp-orbit--/idle.jsonl")).unwrap();
        assert_eq!(usage.requests[0].duration_ms, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn injected_skill_blocks_become_readable_titles() {
        assert_eq!(
            title_from(
                "<skill name=\"impeccable\" location=\"/Users/dev/.agents/skills/impeccable/SKILL.md\">\nReferences are relative to /Users/dev."
            ),
            "Skill: impeccable"
        );
        assert_eq!(title_from("<skill>\nbody"), "Skill loaded");
        assert_eq!(
            title_from("<context>\nfix the OAuth callback"),
            "fix the OAuth callback"
        );
        assert_eq!(
            title_from("fix the OAuth callback"),
            "fix the OAuth callback"
        );
        assert_eq!(title_from("line one\nline two"), "line one line two");
    }

    #[test]
    fn zero_usage_messages_are_not_requests() {
        let dir = std::env::temp_dir().join("orbit-usage-zero-test");
        let _ = fs::remove_dir_all(&dir);
        write_session(
            &dir,
            "zero",
            &[
                HEADER,
                r#"{"type":"message","timestamp":"2026-09-11T09:00:10.000Z","message":{"role":"assistant","provider":"ollama","model":"local","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0},"stopReason":"stop","content":[]}}"#,
            ],
        );
        let usage = parse_session_file(&dir.join("--tmp-orbit--/zero.jsonl")).unwrap();
        assert!(usage.requests.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanner_reuses_unchanged_files() {
        let dir = std::env::temp_dir().join("orbit-usage-scan-cache-test");
        let _ = fs::remove_dir_all(&dir);
        write_session(
            &dir,
            "ws",
            &[
                HEADER,
                &assistant("2026-09-11T09:00:10.000Z", 100, 50, 0, "stop", ""),
            ],
        );
        let mut cache = HashMap::new();
        let first = scan_store(&dir, &mut cache);
        assert_eq!(first.files, 1);
        assert_eq!(first.requests.len(), 1);
        assert_eq!(first.sessions.len(), 1);
        assert_eq!(first.workspaces[0].label, "orbit");
        assert_eq!(first.models[0].label, "claude-sonnet-4");
        assert!(first.models[0].priced);
        // Unchanged → the cached parse is reused (same numbers, no I/O).
        let second = scan_store(&dir, &mut cache);
        assert_eq!(second.requests.len(), 1);
        assert_eq!(second.requests[0].tokens.input, 100);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanner_ignores_non_session_files() {
        let dir = std::env::temp_dir().join("orbit-usage-nonsession-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("--tmp-orbit--")).unwrap();
        fs::write(dir.join("--tmp-orbit--/notes.txt"), "not a session").unwrap();
        fs::write(
            dir.join("--tmp-orbit--/bogus.jsonl"),
            "{\"type\":\"other\"}\n",
        )
        .unwrap();
        let mut cache = HashMap::new();
        let index = scan_store(&dir, &mut cache);
        assert!(index.is_empty());
        assert_eq!(index.files, 1);
        assert_eq!(index.unreadable_files, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanner_is_deterministic_across_runs() {
        let dir = std::env::temp_dir().join("orbit-usage-determinism-test");
        let _ = fs::remove_dir_all(&dir);
        for (name, ts) in [
            ("b", "2026-09-11T10:00:00.000Z"),
            ("a", "2026-09-11T09:00:00.000Z"),
        ] {
            let header = HEADER.replace("\"s1\"", &format!("\"{name}\""));
            let request = assistant(ts, 10, 5, 0, "stop", "");
            write_session(&dir, name, &[&header, &request]);
        }
        let mut first_cache = HashMap::new();
        let first = scan_store(&dir, &mut first_cache);
        let mut second_cache = HashMap::new();
        let second = scan_store(&dir, &mut second_cache);
        assert_eq!(first.requests.len(), 2);
        assert_eq!(
            first.requests.iter().map(|r| r.ts_ms).collect::<Vec<_>>(),
            second.requests.iter().map(|r| r.ts_ms).collect::<Vec<_>>()
        );
        assert_eq!(
            first.requests.iter().map(|r| r.session).collect::<Vec<_>>(),
            second
                .requests
                .iter()
                .map(|r| r.session)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first
                .sessions
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>(),
            second
                .sessions
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
