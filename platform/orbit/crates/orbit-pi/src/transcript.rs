//! The transcript: pi `AgentMessage`s rendered as a virtualized chat list.
//!
//! Data flows in two ways and lands in the same model:
//! - **Snapshot** — `get_messages` response (`data.messages`) rebuilds the
//!   whole transcript when a session is opened.
//! - **Stream** — `message_start` / `message_update` / `message_end` events
//!   append and mutate messages live while the agent runs.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::Engine as _;
use gpui::{point, prelude::*, px, Image, Pixels, ScrollHandle};

use crate::message_scroller::MessageScrollerState;
use crate::transcript_view::{self, TranscriptView};
use orbit_rpc::{AssistantMessageEvent, Event, MessageUsage};
use serde_json::Value;

/// One streaming step of an assistant turn: the reasoning it did, the tool
/// calls it made, and the text it produced — rendered in sequence.
#[derive(Clone, Default)]
pub struct Step {
    pub thinking: String,
    pub tools: Vec<ToolCall>,
    pub text: String,
    /// Provider-reported usage for this LLM call, when pi supplies it.
    pub usage: Option<MessageUsage>,
    /// Wall time the reasoning streamed, measured client-side while live.
    /// Reloaded sessions leave this `None` and the view estimates it from
    /// [`Step::timestamp`] instead.
    pub thinking_duration: Option<Duration>,
    /// pi's per-step message timestamp (epoch millis), when supplied — the
    /// fallback for estimating reasoning time on reload.
    pub timestamp: Option<i64>,
    /// Active generation time measured client-side while this step
    /// streamed: text, thinking, and tool-call argument deltas, excluding
    /// gaps for tool execution, round-trips, and first-token latency.
    /// Reloaded sessions leave this `None` — pi does not persist per-step
    /// generation timing.
    pub generation_time: Option<Duration>,
    /// pi's own `durationMs` for this call: wall time from the provider
    /// request starting to the stream finishing, so prefill and decode are
    /// both included. The authoritative generation clock, preferred over
    /// the client-measured [`Step::generation_time`] (which only sees the
    /// bursts pi emits and so reads far too high). `None` on reload — pi
    /// does not persist it — and for user rows.
    pub duration: Option<Duration>,
}

/// A context-boundary summary row: pi compacted the conversation (or
/// summarized an abandoned branch on `/tree` navigation). It sits where the
/// summarized turns were — its own row, never merged into a user/assistant
/// turn.
#[derive(Clone)]
pub struct SummaryRow {
    /// `true` for a `/tree` branch summary, `false` for a compaction.
    pub branch: bool,
    /// The markdown summary pi generated.
    pub summary: String,
    /// Context tokens before compaction replaced them. Compaction only; pi
    /// does not report a token count for branch summaries.
    pub tokens_before: Option<u64>,
}

pub struct ChatMessage {
    pub user: bool,
    /// A compaction/branch-summary boundary card instead of a turn.
    pub summary: Option<SummaryRow>,
    /// A turn's steps in sequence — thinking/tools/text per step.
    pub steps: Vec<Step>,
    /// Images attached to a user message (live prompt or session reload).
    pub images: Vec<Arc<Image>>,
    /// Wall time of this assistant turn, recorded when the stream ends.
    pub elapsed: Option<Duration>,
    /// When this message was completed (epoch millis). Snapshot messages
    /// carry pi's own `timestamp`; live ones are stamped at `message_end`.
    pub finished_at: Option<i64>,
    /// Provider/agent error that ended this assistant turn
    /// (`stopReason: "error"` + `errorMessage`). Rendered inline so a failed
    /// turn is never an empty row.
    pub error: Option<String>,
    /// The turn was cancelled (`stopReason: "aborted"`). The partial answer
    /// stays; a quiet marker says the generation was stopped.
    pub aborted: bool,
}

impl ChatMessage {
    /// A compaction/branch-summary boundary row.
    pub fn is_summary(&self) -> bool {
        self.summary.is_some()
    }

    /// A real assistant turn — the counterpart of [`Self::user`], and false
    /// for summary rows (which are also not user messages).
    pub fn is_assistant(&self) -> bool {
        !self.user && self.summary.is_none()
    }

    /// The concatenated answer text (steps in sequence).
    pub fn text(&self) -> String {
        self.steps
            .iter()
            .map(|step| step.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Every tool call of the turn, in order.
    pub fn tools(&self) -> impl Iterator<Item = &ToolCall> {
        self.steps.iter().flat_map(|step| step.tools.iter())
    }

    pub fn has_hidden_work(&self) -> bool {
        self.steps
            .iter()
            .any(|step| !step.thinking.is_empty() || !step.tools.is_empty())
    }

    /// Turn-level usage: the sum of every step's provider-reported usage.
    /// `None` when no step carries usage (user turns, or providers that omit
    /// it), so the footer can hide the metric rather than invent zeros.
    pub fn usage(&self) -> Option<MessageUsage> {
        let mut total: Option<MessageUsage> = None;
        for usage in self.steps.iter().filter_map(|step| step.usage.as_ref()) {
            match &mut total {
                Some(total) => total.add(usage),
                None => total = Some(usage.clone()),
            }
        }
        total
    }

    /// Generation clock for this turn — the sum of its steps' values,
    /// preferring pi's own `durationMs` ([`Step::duration`]) over the
    /// client-measured fallback. `None` when no step carries either
    /// (reloaded sessions, or providers that deliver whole blocks).
    pub fn generation_time(&self) -> Option<Duration> {
        let mut total = Duration::ZERO;
        let mut any = false;
        for step in &self.steps {
            if let Some(duration) = step.duration.or(step.generation_time) {
                total += duration;
                any = true;
            }
        }
        any.then_some(total)
    }
}

/// The newest assistant turn, summarized for a background notification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnSummary {
    /// The turn's answer text, or the agent error that ended it.
    pub body: String,
    /// `stopReason: "error"` — the provider or agent failed the turn.
    pub failed: bool,
    /// The user stopped the turn; there is nothing to announce.
    pub aborted: bool,
}

/// Structured facts pi attaches to a tool result under `details`, reduced to
/// the few the transcript shows at a glance. Data pi did not send stays
/// absent — the UI never invents a count or a status.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolFacts {
    /// The result was capped (read/bash `truncation`, grep match limit, ls
    /// entry limit): the agent saw only part of the data.
    pub truncated: bool,
    /// Lines the agent actually received, when pi reported them.
    pub output_lines: Option<u64>,
    /// Lines the full result held, when pi reported them.
    pub total_lines: Option<u64>,
}

impl ToolFacts {
    /// Read facts from a raw tool-result envelope. Accepts the
    /// `{"content":[…],"details":…}` shape pi sends; a bare payload with no
    /// `details` yields empty facts.
    fn from_result(value: &Value) -> Self {
        let mut facts = Self::default();
        let Some(details) = value.get("details").filter(|details| !details.is_null()) else {
            return facts;
        };
        // read/bash attach a `truncation` object only when the output was cut;
        // it carries the line budget the agent actually saw.
        if let Some(truncation) = details.get("truncation").filter(|t| !t.is_null()) {
            facts.truncated = truncation
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            facts.output_lines = truncation.get("outputLines").and_then(Value::as_u64);
            facts.total_lines = truncation.get("totalLines").and_then(Value::as_u64);
        }
        // grep / ls cap the result without a truncation object.
        for key in ["matchLimitReached", "linesTruncated", "entryLimitReached"] {
            if details.get(key).and_then(Value::as_bool) == Some(true) {
                facts.truncated = true;
            }
        }
        facts
    }
}

/// One tool call row: name, args summary, and the file path it operates on
/// (when the tool is file-oriented — drives the devicons glyph).
#[derive(Clone)]
pub struct ToolCall {
    pub name: String,
    pub summary: String,
    pub path: Option<String>,
    /// Line counts from `edit` / `write` args. Zero for other tools.
    pub added: u64,
    pub removed: u64,
    /// pi `toolCallId` when known — links live `tool_execution_*` events
    /// (and their results) to this row.
    pub id: Option<String>,
    /// Full arguments as received; drives the expandable detail card.
    pub args: Option<Value>,
    /// Result payload captured from `tool_execution_end` (live runs).
    pub output: Option<Value>,
    /// The tool execution reported `isError`.
    pub failed: bool,
    /// Structured result facts (`details`) — surfaced at a glance on the
    /// card header instead of hiding inside the expanded output.
    pub facts: ToolFacts,
}

impl ToolCall {
    fn from_value(name: &str, args: Option<&Value>) -> Self {
        let empty = Value::Null;
        let args = args.unwrap_or(&empty);
        let (added, removed) = tool_line_stats(name, args);
        Self {
            name: name.to_string(),
            summary: summarize_value(args),
            path: args.get("path").and_then(Value::as_str).map(str::to_string),
            added,
            removed,
            id: None,
            args: if args.is_null() {
                None
            } else {
                Some(args.clone())
            },
            output: None,
            failed: false,
            facts: ToolFacts::default(),
        }
    }
}

impl ChatMessage {
    fn empty_assistant() -> Self {
        Self {
            user: false,
            summary: None,
            steps: vec![Step::default()],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        }
    }

    /// Parse one `AgentMessage` JSON value from pi. Live message events wrap
    /// the message under `"message"`; snapshots list it flat — accept both.
    /// Blocks become ONE step: the reasoning, the calls, and the text.
    fn from_value(value: &Value) -> Option<ChatMessage> {
        let value = value.get("message").unwrap_or(value);
        let role = value.get("role")?.as_str()?;
        // Compaction and branch summaries are real boundary rows (pi's TUI
        // renders both): keep them visible in the conversation.
        if role == "compactionSummary" || role == "branchSummary" {
            return Some(Self::summary_row(role, value));
        }
        // Only user and assistant turns are conversational rows. pi
        // interleaves context-only entries — the `system` loadout/tool-change
        // update it emits right before a prompt's user echo, and extension
        // `custom` messages. Parsed as rows they would land between the
        // optimistic prompt and pi's echo, defeating the identical-echo
        // dedupe and showing the prompt twice.
        if role != "user" && role != "assistant" {
            return None;
        }
        let user = role == "user";
        let error = message_error(value);
        let aborted = !user && value.get("stopReason").and_then(Value::as_str) == Some("aborted");
        let mut message = ChatMessage {
            user,
            summary: None,
            steps: vec![Step::default()],
            images: Vec::new(),
            elapsed: None,
            finished_at: parse_timestamp(value.get("timestamp")),
            error,
            aborted,
        };
        // Borrow the step field directly so image pushes below can touch
        // `message.images` without a whole-struct mutable borrow.
        let step = message.steps.first_mut()?;

        match value.get("content") {
            Some(Value::String(text)) => step.text = text.clone(),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                if !step.text.is_empty() {
                                    step.text.push_str("\n\n");
                                }
                                step.text.push_str(text);
                            }
                        }
                        "thinking" => {
                            if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                                step.thinking.push_str(text);
                            }
                        }
                        "image" => {
                            if let Some(image) = image_from_block(block) {
                                message.images.push(image);
                            }
                        }
                        "toolCall" | "tool_call" => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let mut tool = ToolCall::from_value(name, block.get("arguments"));
                            tool.id = block
                                .get("id")
                                .or_else(|| block.get("toolCallId"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            // Some finalized entries carry the result inline.
                            let result = block
                                .get("result")
                                .or_else(|| block.get("output"))
                                .filter(|result| !result.is_null());
                            tool.facts = result.map(ToolFacts::from_result).unwrap_or_default();
                            tool.output = result.cloned();
                            tool.failed = block
                                .get("isError")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            step.tools.push(tool);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if user {
            // pi appends image hints (resize/conversion notes) to the prompt
            // text it echoes and persists. They are protocol metadata for the
            // model, not what the user typed — strip them so the echo matches
            // the optimistic row and reloads don't render the note.
            message.steps[0].text = strip_image_hints(&message.steps[0].text).to_string();
            if let Some((name, trailing)) = injected_skill(&message.steps[0].text) {
                message.steps[0].text = compact_skill_prompt(&name, &trailing);
            }
        }
        message.steps[0].usage = MessageUsage::from_value(value.get("usage"));
        let timestamp = message.finished_at;
        message.steps[0].timestamp = timestamp;
        // pi measures the whole call (`durationMs`) — prefill plus decode —
        // which is the honest generation clock. Use it when it lands.
        message.steps[0].duration = value
            .get("durationMs")
            .and_then(|millis| {
                millis
                    .as_u64()
                    .or_else(|| millis.as_f64().map(|f| f.round() as u64))
            })
            .map(Duration::from_millis);
        Some(message)
    }

    /// One compaction/branch-summary boundary row. The step stays empty: the
    /// summary renders as its own card, and `text()` must stay clear of turn
    /// navigation, search, and answer-copy paths.
    fn summary_row(role: &str, value: &Value) -> Self {
        Self {
            user: false,
            summary: Some(SummaryRow {
                branch: role == "branchSummary",
                summary: value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tokens_before: value.get("tokensBefore").and_then(Value::as_u64),
            }),
            steps: vec![Step::default()],
            images: Vec::new(),
            elapsed: None,
            finished_at: parse_timestamp(value.get("timestamp")),
            error: None,
            aborted: false,
        }
    }
}

/// pi appends image hints to a user prompt when an attachment was resized or
/// converted — `[Image: original …]`, `[Image converted from … to ….]`,
/// `[Image omitted: …]`. The model needs them for coordinate mapping, but they
/// are not the user's words. Drop the trailing hint paragraph(s) so a live echo
/// dedupes against the optimistic row and a reload shows only the prompt.
fn strip_image_hints(text: &str) -> &str {
    let trimmed = text.trim_end();
    let mut end = trimmed.len();
    while let Some(sep) = trimmed[..end].rfind("\n\n") {
        let tail = trimmed[sep + 2..end].trim();
        if tail.lines().all(is_image_hint_line) {
            end = sep;
        } else {
            break;
        }
    }
    trimmed[..end].trim_end()
}

/// One line of pi's image-hint block (`[Image: …]`, `[Image converted …]`,
/// `[Image omitted: …]`).
fn is_image_hint_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("[Image:")
        || line.starts_with("[Image converted")
        || line.starts_with("[Image omitted")
}

/// pi records a loaded skill as an ordinary user message: a
/// `<skill name="…" location="…">…SKILL.md body…</skill>` document with the
/// text the user typed after the slash command appended. Returns the skill
/// name and that trailing prompt.
///
/// The close tag is required, so prose that merely starts with `<skill` is
/// left alone.
fn injected_skill(text: &str) -> Option<(String, String)> {
    let rest = text.trim_start().strip_prefix("<skill")?;
    let header = &rest[..rest.find('>')?];
    let close = rest.find("</skill>")?;
    let name = header
        .split("name=\"")
        .nth(1)
        .and_then(|value| value.split('"').next())
        .unwrap_or("")
        .trim()
        .to_string();
    let trailing = rest[close + "</skill>".len()..].trim().to_string();
    Some((name, trailing))
}

/// The readable prompt a stored skill block is reduced to: the slash command
/// the user ran, plus whatever they typed after it. The skill body itself is
/// never rendered.
fn compact_skill_prompt(name: &str, trailing: &str) -> String {
    let name = if name.is_empty() { "skill" } else { name };
    if trailing.is_empty() {
        format!("/skill:{name}")
    } else {
        format!("/skill:{name} {trailing}")
    }
}

/// Whether a pi message value is that injected skill document.
fn injected_skill_message(value: &Value) -> bool {
    let value = value.get("message").unwrap_or(value);
    if value.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    match value.get("content") {
        Some(Value::String(text)) => injected_skill(text).is_some(),
        Some(Value::Array(blocks)) => blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| injected_skill(text).is_some())
        }),
        _ => false,
    }
}

/// Decode an image content block — `{"type":"image","data":<base64>,
/// "mimeType":"image/png"}`, the shape this app prompts with — into a
/// renderable image. Covers pi echoes and session reloads from disk.
fn image_from_block(block: &Value) -> Option<Arc<Image>> {
    let mime = block
        .get("mimeType")
        .or_else(|| block.get("mime_type"))
        .and_then(Value::as_str)?;
    let format = match mime {
        "image/png" => gpui::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => gpui::ImageFormat::Jpeg,
        "image/webp" => gpui::ImageFormat::Webp,
        "image/gif" => gpui::ImageFormat::Gif,
        "image/bmp" => gpui::ImageFormat::Bmp,
        _ => return None,
    };
    let data = block.get("data").and_then(Value::as_str)?;
    // Tolerate data URLs ("data:image/png;base64,…" → raw base64).
    let data = match data.split_once(',') {
        Some((prefix, rest)) if prefix.starts_with("data:") => rest,
        _ => data,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// Extract a tool result message: `(toolCallId, output, isError, facts)`.
/// pi sends `role: "toolResult"` messages — not chat rows; their output and
/// structured `details` belong on the matching tool call (rendered on the
/// activity card and inside its detail).
fn tool_result_parts(value: &Value) -> Option<(String, Option<Value>, bool, ToolFacts)> {
    let value = value.get("message").unwrap_or(value);
    if value.get("role").and_then(Value::as_str) != Some("toolResult") {
        return None;
    }
    let id = value.get("toolCallId").and_then(Value::as_str)?.to_string();
    let output = tool_result_output(value);
    let failed = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let facts = ToolFacts::from_result(value);
    Some((id, output, failed, facts))
}

/// Normalize a `tool_execution_*` result payload for display. pi wraps
/// results as `{"content": [{"type":"text","text":…}], "details":…}` — the
/// transcript shows the joined text, never the raw envelope (protocol data
/// is presentation-sanitized). Payloads with no text blocks (structured
/// results) pass through unchanged so they still highlight as JSON — except
/// an envelope whose `content` carries no text yet (the empty array that
/// streams before the first chunk), which reads as no output at all rather
/// than leaking the protocol wrapper into the detail card.
fn normalize_tool_result(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Object(map) => match tool_result_output(value) {
            Some(text) => text,
            // Only pi's envelope — an *array* `content` that carries no text
            // yet (the empty array that streams before the first chunk) — is
            // collapsed to empty output. A bare `{"content": "text"}` from
            // an older build, or any other shape, passes through unchanged.
            None if map.get("content").is_some_and(Value::is_array) => Value::String(String::new()),
            None => value.clone(),
        },
        _ => value.clone(),
    }
}

/// Tool result payloads: joined text blocks; images are skipped.
fn tool_result_output(value: &Value) -> Option<Value> {
    let blocks = value.get("content").and_then(Value::as_array)?;
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if let Some(part) = block.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(Value::String(text))
    }
}

/// Attach a tool result to the newest tool call carrying `id` (snapshot
/// path — the live path routes through `attach_tool_result` positions).
fn attach_tool_output(
    messages: &mut [ChatMessage],
    id: &str,
    output: Option<Value>,
    failed: bool,
    facts: ToolFacts,
) {
    for message in messages.iter_mut().rev() {
        let Some(tool) = message
            .steps
            .iter_mut()
            .rev()
            .flat_map(|step| step.tools.iter_mut())
            .find(|t| t.id.as_deref() == Some(id))
        else {
            continue;
        };
        if tool.output.is_none() {
            tool.output = output;
        }
        tool.failed = failed;
        tool.facts = facts;
        return;
    }
}

/// Where the current assistant step began within its (possibly merged)
/// message — a turn's steps stream into one row, so a step's settled
/// content replaces only its own contribution.
#[derive(Clone, Copy, Default, PartialEq)]
struct StepMark {
    step: usize,
    text: usize,
    thinking: usize,
    tools: usize,
}

/// Drop a `message_start` seed when the first delta replays it. If the content
/// accumulated since `mark` starts with `delta`, the snapshot had already
/// captured that chunk, so clear back to `mark` before the delta is appended;
/// otherwise leave the seed in place. Only the first delta of a streamed block
/// passes `seeded = true`.
fn drop_seed(target: &mut String, mark: usize, delta: &str, seeded: bool) {
    if !seeded || delta.is_empty() {
        return;
    }
    if target.get(mark..).is_some_and(|t| t.starts_with(delta)) {
        target.truncate(mark);
    }
}

/// Merge one assistant step into its run's message: each step appends as
/// its own sequence entry, so the page can interleave thought/tool groups
/// with the texts between them.
fn merge_step(slot: &mut ChatMessage, step: ChatMessage) {
    let elapsed = step.elapsed;
    let finished_at = step.finished_at;
    let error = step.error.clone();
    let aborted = step.aborted;
    slot.steps.push(step.into_step());
    if elapsed.is_some() {
        slot.elapsed = elapsed;
    }
    if finished_at.is_some() {
        slot.finished_at = finished_at;
    }
    // A step that ended in an error labels the whole row.
    if error.is_some() {
        slot.error = error;
    }
    if aborted {
        slot.aborted = true;
    }
}

impl ChatMessage {
    /// A parsed message carries exactly one step — take it.
    fn into_step(mut self) -> Step {
        self.steps.pop().unwrap_or_default()
    }

    /// The step streamed text and tool calls append to. Parsed rows always
    /// carry one; a fresh default keeps a malformed stream from panicking.
    fn current_step_mut(&mut self) -> &mut Step {
        if self.steps.is_empty() {
            self.steps.push(Step::default());
        }
        let last = self.steps.len() - 1;
        &mut self.steps[last]
    }
}

/// Role of a message event payload — unwrapping the live envelope pi
/// wraps around message start/end events (`{"type":…, "message": {…}}`).
fn message_role(value: &Value) -> Option<&str> {
    value
        .get("message")
        .unwrap_or(value)
        .get("role")
        .and_then(Value::as_str)
}

/// The provider/agent error carried by a finalized assistant message, if any.
/// pi sets `stopReason: "error"` and `errorMessage` when the LLM call fails
/// (e.g. an unsupported model). The app raises the error banner from this and
/// the transcript renders the same text inline.
///
/// A user abort is not an agent failure: pi's `createAbortedMessage` marks the
/// message `stopReason: "aborted"` but still carries `errorMessage`
/// ("Request was aborted"). Treating that as an error would render both the
/// inline error card and the app banner on top of the transcript's own quiet
/// "Stopped" marker. An aborted turn therefore reports no error here.
pub fn message_error(value: &Value) -> Option<String> {
    let value = value.get("message").unwrap_or(value);
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if value.get("stopReason").and_then(Value::as_str) == Some("aborted") {
        return None;
    }
    value
        .get("errorMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

fn summarize_value(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 160 {
        let mut out: String = text.chars().take(160).collect();
        out.push('…');
        out
    } else {
        text
    }
}

/// Tolerant epoch-millis parser for message timestamps. pi may send an
/// RFC 3339 string, seconds, or milliseconds.
fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.timestamp_millis());
    }
    let seconds = value
        .as_u64()
        .or_else(|| value.as_f64().filter(|f| *f >= 0.).map(|f| f as u64))?;
    let millis = if seconds > 1_000_000_000_000 {
        seconds
    } else {
        seconds * 1000
    };
    i64::try_from(millis).ok()
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Added/removed lines from one `edit` or `write` tool's arguments.
fn tool_line_stats(name: &str, args: &Value) -> (u64, u64) {
    match name {
        "edit" => {
            let mut added = 0u64;
            let mut removed = 0u64;
            if let Some(edits) = args.get("edits").and_then(Value::as_array) {
                for edit in edits {
                    removed += count_lines(edit.get("oldText"));
                    added += count_lines(edit.get("newText"));
                }
            }
            (added, removed)
        }
        "write" => (count_lines(args.get("content")), 0),
        _ => (0, 0),
    }
}

fn count_lines(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_str)
        .map(|text| {
            if text.is_empty() {
                0
            } else {
                text.split('\n').count() as u64
            }
        })
        .unwrap_or(0)
}

/// Count added/removed lines from the `edit` / `write` tool calls inside a
/// message value. Counts are honest — they only reflect what the agent did.
pub(crate) fn diff_from_message(value: &Value) -> (u64, u64) {
    let mut added = 0u64;
    let mut removed = 0u64;
    let Some(Value::Array(blocks)) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| value.get("content"))
    else {
        return (0, 0);
    };
    for block in blocks {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "toolCall" && kind != "tool_call" {
            continue;
        }
        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
        let args = block.get("arguments").unwrap_or(&Value::Null);
        let (a, r) = tool_line_stats(name, args);
        added += a;
        removed += r;
    }
    (added, removed)
}

/// Merge edit/write tools by path for the per-message changed-files card.
pub(crate) fn changed_files(message: &ChatMessage) -> Vec<(String, u64, u64)> {
    let mut files: Vec<(String, u64, u64)> = Vec::new();
    for tool in message.tools() {
        if tool.added == 0 && tool.removed == 0 {
            continue;
        }
        let Some(path) = tool.path.as_deref() else {
            continue;
        };
        if let Some(entry) = files.iter_mut().find(|(existing, _, _)| existing == path) {
            entry.1 += tool.added;
            entry.2 += tool.removed;
        } else {
            files.push((path.to_string(), tool.added, tool.removed));
        }
    }
    files
}

type ExpandedActivities = Rc<RefCell<HashMap<(usize, usize), bool>>>;
type ExpandedTools = Rc<RefCell<HashSet<(usize, usize)>>>;
type CopiedSections = Rc<RefCell<HashMap<(usize, usize, u8), Instant>>>;
/// Tool detail sections expanded past their collapsed preview, keyed
/// `(message_ix, flat_tool_ix, section)`.
type ExpandedSections = Rc<RefCell<HashSet<(usize, usize, u8)>>>;
/// Code blocks expanded past their collapsed preview, keyed
/// `(message_ix, prose_salt, block_ix)` — the salt scopes the block index to
/// the step/user prose run it was parsed from.
type ExpandedBlocks = Rc<RefCell<HashSet<(usize, u64, usize)>>>;
/// `toolCallId` -> `(message_ix, tool_ix, flat_tool_ix)` so tool results land
/// on the right row.
type ToolPositions = Rc<RefCell<HashMap<String, (usize, usize, usize)>>>;

/// How long the one-time rail hint stays up before dismissing itself.
const RAIL_HINT_TTL: Duration = Duration::from_secs(10);

/// `~/.orbit-pi/hints.json` — one-time affordance hints, per install. Each
/// hint is a named boolean; the rail hint shows once the conversation rail
/// first appears, the sidebar star banner until it is dismissed.
fn hints_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("hints.json")
}

/// The rail hint's key in `hints.json`.
const RAIL_HINT_KEY: &str = "rail_hint_seen";

/// Read one named hint. A missing file or key reads as false (not yet seen).
pub(crate) fn hint_seen(key: &str) -> bool {
    hint_seen_at(&hints_path(), key)
}

fn hint_seen_at(path: &Path, key: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| value.get(key).and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Persist one named hint as seen, preserving every other key in the file.
/// The whole file is a single JSON object shared by every affordance hint, so
/// dismissing one must never wipe another.
pub(crate) fn persist_hint(key: &str) {
    persist_hint_at(&hints_path(), key);
}

fn persist_hint_at(path: &Path, key: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut map = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw).ok()
        })
        .unwrap_or_default();
    map.insert(key.to_string(), serde_json::Value::Bool(true));
    let _ = std::fs::write(path, serde_json::Value::Object(map).to_string());
}

/// Shared rail-hint dismissal for the model and the view layer (the view
/// holds clones of the same cells). Returns true on the first call.
pub(crate) fn dismiss_rail_hint_state(
    dismissed: &Cell<bool>,
    shown_at: &Cell<Option<Instant>>,
) -> bool {
    if dismissed.replace(true) {
        return false;
    }
    shown_at.set(None);
    persist_hint(RAIL_HINT_KEY);
    true
}

/// Transcript state shared between the view and the RPC event pump.
pub struct Transcript {
    messages: Rc<RefCell<Vec<ChatMessage>>>,
    /// Cross-block text selection + right-click copy menu state.
    text_selection: transcript_view::TextSelectionState,
    scroller: MessageScrollerState,
    /// Index of the assistant message currently being streamed, if any.
    streaming: Rc<Cell<Option<usize>>>,
    /// When the current assistant turn started (for the live Working clock).
    stream_started: Rc<Cell<Option<Instant>>>,
    /// When the current turn's provider request began (`turn_start`). This
    /// is the same clock basis as pi's `durationMs`, so the live rate does
    /// not read the bursty gap between deltas as the whole generation.
    turn_started: Rc<Cell<Option<Instant>>>,
    /// When the current step's reasoning began (the per-thought clock).
    thinking_started: Rc<Cell<Option<Instant>>>,
    /// Active generation time for the step currently streaming — the sum
    /// of inter-delta gaps short enough to be decode rather than a stall.
    generation_time: Rc<Cell<Duration>>,
    /// When the last content delta arrived (text/thinking/tool-call args);
    /// `None` after a reset or a tool begins, so the following gap is not
    /// counted as generation.
    last_delta_at: Rc<Cell<Option<Instant>>>,
    /// Settled turns whose thinking/tools are disclosed (turn fold).
    expanded_turns: Rc<RefCell<HashSet<usize>>>,
    /// Messages whose changed-files list is fully expanded.
    expanded_files: Rc<RefCell<HashSet<usize>>>,
    /// Per-message activity cluster open state. Missing means "expanded iff live".
    expanded_activities: ExpandedActivities,
    /// When each assistant reply was last copied (Copy → Copied feedback).
    copied: Rc<RefCell<HashMap<usize, Instant>>>,
    /// Per-tool detail-card open state, keyed `(message_ix, tool_ix)`.
    expanded_tools: ExpandedTools,
    /// Per detail-section copy feedback, keyed `(message_ix, tool_ix, section)`.
    copied_sections: CopiedSections,
    /// Tool output sections expanded past their collapsed preview.
    expanded_sections: ExpandedSections,
    /// Code blocks expanded past their collapsed preview.
    expanded_blocks: ExpandedBlocks,
    /// Persistent scroll handles for the expandable "Thought" cards, keyed
    /// `(message_ix, step_ix)` — the card reads its offset/limit to chain the
    /// wheel to the transcript once it bottoms out.
    thinking_scrolls: transcript_view::ThinkingScrolls,
    /// "Thought" cards the reader collapsed, keyed `(message_ix, step_ix)`.
    /// Missing means expanded — the default once the activity group is open.
    collapsed_thoughts: transcript_view::CollapsedThoughts,
    /// Live "Thought" cards the reader scrolled away from the newest line.
    /// Missing means the streaming card stays pinned to its latest line.
    thinking_detached: transcript_view::ThinkingDetached,
    /// `toolCallId` -> `(message_ix, tool_ix)` so `tool_execution_end` results
    /// land on the right row.
    tool_positions: ToolPositions,
    /// Rail tick currently hovered (drives the turn preview card).
    hovered_turn: Rc<Cell<Option<usize>>>,
    /// Assistant row whose footer usage metric is hovered (drives the
    /// per-message token/cost breakdown card).
    hovered_usage: Rc<Cell<Option<usize>>>,
    /// One-time rail hint still pending (per install, persisted). Cleared
    /// by using the rail, jumping turns, or the TTL below.
    rail_hint_dismissed: Rc<Cell<bool>>,
    /// When the hint first rendered — drives its self-dismiss timeout.
    rail_hint_shown_at: Rc<Cell<Option<Instant>>>,
    /// Scroll position of the conversation-turn rail.
    rail_scroll: ScrollHandle,
    /// Last turn the rail auto-scrolled to — re-fires only when it changes.
    rail_autoscroll: Rc<Cell<Option<usize>>>,
    /// Where the streaming step began in its (merged) message.
    step_mark: Rc<Cell<Option<StepMark>>>,
    /// Accumulated `toolcall_delta` argument text for the call currently
    /// streaming (pi sends the arguments as raw JSON text chunks). Reset on
    /// `toolcall_start`, applied to the row once the buffer parses as JSON,
    /// cleared on `toolcall_end`.
    toolcall_args: Rc<RefCell<String>>,
    /// Whether the current `message_start` snapshot already carries the first
    /// content chunk. Some providers buffer one delta and put it in the start
    /// message, then re-send that same chunk as the first delta; these flags
    /// let the first delta drop the seed instead of doubling the text.
    seed_text: Rc<Cell<bool>>,
    seed_thinking: Rc<Cell<bool>>,
    /// Whether the list currently carries the end-of-task summary row (an
    /// extra slot after the last message). Spliced in when a run settles
    /// with file edits; removed when a new turn begins.
    tail_summary: Rc<Cell<bool>>,
    /// A prompt is out and no assistant step has streamed yet. Drives the
    /// same tail slot as the summary card, showing the live "Working for…"
    /// row from the moment the user hits send instead of from the first
    /// token — otherwise a slow provider looks like a dropped keystroke.
    awaiting_reply: Rc<Cell<bool>>,
    /// Cached user-turn row indices, rebuilt only when the message count
    /// changes (messages only append or are rebuilt wholesale). The rail
    /// renders every frame; this keeps that O(message-count) pass off the
    /// hot path.
    user_turns: Rc<RefCell<(usize, Rc<Vec<usize>>)>>,
}

impl Transcript {
    pub fn new() -> Self {
        let messages = Rc::new(RefCell::new(Vec::new()));
        Self {
            messages,
            text_selection: Rc::new(RefCell::new(transcript_view::TextSelection::new())),
            scroller: MessageScrollerState::new(0),
            streaming: Rc::new(Cell::new(None)),
            stream_started: Rc::new(Cell::new(None)),
            turn_started: Rc::new(Cell::new(None)),
            thinking_started: Rc::new(Cell::new(None)),
            generation_time: Rc::new(Cell::new(Duration::ZERO)),
            last_delta_at: Rc::new(Cell::new(None)),
            expanded_turns: Rc::new(RefCell::new(HashSet::new())),
            expanded_files: Rc::new(RefCell::new(HashSet::new())),
            expanded_activities: Rc::new(RefCell::new(HashMap::new())),
            expanded_tools: Rc::new(RefCell::new(HashSet::new())),
            copied: Rc::new(RefCell::new(HashMap::new())),
            copied_sections: Rc::new(RefCell::new(HashMap::new())),
            expanded_sections: Rc::new(RefCell::new(HashSet::new())),
            expanded_blocks: Rc::new(RefCell::new(HashSet::new())),
            thinking_scrolls: Rc::new(RefCell::new(HashMap::new())),
            collapsed_thoughts: Rc::new(RefCell::new(HashSet::new())),
            thinking_detached: Rc::new(RefCell::new(HashSet::new())),
            tool_positions: Rc::new(RefCell::new(HashMap::new())),
            hovered_turn: Rc::new(Cell::new(None)),
            hovered_usage: Rc::new(Cell::new(None)),
            rail_hint_dismissed: Rc::new(Cell::new(hint_seen(RAIL_HINT_KEY))),
            rail_hint_shown_at: Rc::new(Cell::new(None)),
            rail_scroll: ScrollHandle::new(),
            rail_autoscroll: Rc::new(Cell::new(None)),
            step_mark: Rc::new(Cell::new(None)),
            toolcall_args: Rc::new(RefCell::new(String::new())),
            seed_text: Rc::new(Cell::new(false)),
            seed_thinking: Rc::new(Cell::new(false)),
            tail_summary: Rc::new(Cell::new(false)),
            awaiting_reply: Rc::new(Cell::new(false)),
            user_turns: Rc::new(RefCell::new((0, Rc::new(Vec::new())))),
        }
    }

    /// User-turn row indices for the navigation rail, recomputed only when
    /// the message list actually changes. Messages only ever append (or are
    /// rebuilt wholesale via `load_from`/`clear`), so the message count is a
    /// sufficient cache key once those two paths invalidate it.
    fn cached_user_turns(&self) -> Rc<Vec<usize>> {
        let messages = self.messages.borrow();
        let mut cache = self.user_turns.borrow_mut();
        if cache.0 == messages.len() {
            return cache.1.clone();
        }
        let turns = Rc::new(
            messages
                .iter()
                .enumerate()
                .filter(|(_, message)| message.user)
                .map(|(ix, _)| ix)
                .collect::<Vec<usize>>(),
        );
        *cache = (messages.len(), turns.clone());
        turns
    }

    /// Force the user-turn cache to rebuild on the next read (`load_from`
    /// and `clear` replace the message list wholesale, so a matching length
    /// is not proof the cached indices are still valid).
    fn invalidate_user_turns(&self) {
        self.user_turns.borrow_mut().0 = usize::MAX;
    }

    /// The current transcript text selection, if any — the keyboard copy
    /// path (`cmd-c`) reads it without stealing the composer's own copy.
    pub fn selected_text(&self) -> Option<String> {
        self.text_selection.borrow().selected_text()
    }

    /// Rebuild the whole transcript from a `get_messages` response payload.
    pub fn load_from(&mut self, data: &Value) {
        self.text_selection.borrow_mut().clear();
        let mut parsed = Vec::new();
        if let Some(messages) = data.get("messages").and_then(Value::as_array) {
            for message in messages {
                // Tool results are not chat rows — attach the output to the
                // matching tool call (by id) and keep the transcript clean.
                if let Some((id, output, failed, facts)) = tool_result_parts(message) {
                    attach_tool_output(&mut parsed, &id, output, failed, facts);
                    continue;
                }
                if let Some(parsed_message) = ChatMessage::from_value(message) {
                    // Consecutive assistant messages are one logical turn
                    // one row, one fold, one footer — not a stack of
                    // "Worked" dividers per streaming step. A user message
                    // always begins a new turn — never merged into the run
                    // before it (the whole trail used to collapse into one
                    // assistant blob when prompts followed a tool-heavy run).
                    match parsed.last_mut() {
                        Some(previous)
                            if parsed_message.is_assistant() && previous.is_assistant() =>
                        {
                            merge_step(previous, parsed_message);
                        }
                        _ => parsed.push(parsed_message),
                    }
                }
            }
        }
        let count = parsed.len();
        *self.messages.borrow_mut() = parsed;
        self.invalidate_user_turns();
        self.scroller.reset(count);
        self.streaming.set(None);
        self.seed_text.set(false);
        self.seed_thinking.set(false);
        self.stream_started.set(None);
        self.turn_started.set(None);
        self.thinking_started.set(None);
        self.reset_generation();
        self.expanded_turns.borrow_mut().clear();
        self.expanded_files.borrow_mut().clear();
        self.expanded_activities.borrow_mut().clear();
        self.expanded_tools.borrow_mut().clear();
        self.copied.borrow_mut().clear();
        self.copied_sections.borrow_mut().clear();
        self.expanded_sections.borrow_mut().clear();
        self.expanded_blocks.borrow_mut().clear();
        self.thinking_scrolls.borrow_mut().clear();
        self.collapsed_thoughts.borrow_mut().clear();
        self.thinking_detached.borrow_mut().clear();
        self.tool_positions.borrow_mut().clear();
        self.hovered_turn.set(None);
        self.rail_scroll.set_offset(point(px(0.), px(0.)));
        self.rail_autoscroll.set(None);
        self.step_mark.set(None);
        self.toolcall_args.borrow_mut().clear();
        // Rebuild resets the list to the message count; re-add the summary
        // row when the loaded session touched files.
        self.tail_summary.set(false);
        self.awaiting_reply.set(false);
        let touched_files = !self.changed_files_summary().is_empty();
        self.set_tail_row(touched_files);
    }

    /// Clear for a fresh session.
    pub fn clear(&mut self) {
        *self.messages.borrow_mut() = Vec::new();
        self.invalidate_user_turns();
        self.text_selection.borrow_mut().clear();
        self.scroller.reset(0);
        self.streaming.set(None);
        self.seed_text.set(false);
        self.seed_thinking.set(false);
        self.stream_started.set(None);
        self.turn_started.set(None);
        self.thinking_started.set(None);
        self.reset_generation();
        self.expanded_turns.borrow_mut().clear();
        self.expanded_files.borrow_mut().clear();
        self.expanded_activities.borrow_mut().clear();
        self.expanded_tools.borrow_mut().clear();
        self.copied.borrow_mut().clear();
        self.copied_sections.borrow_mut().clear();
        self.expanded_sections.borrow_mut().clear();
        self.expanded_blocks.borrow_mut().clear();
        self.thinking_scrolls.borrow_mut().clear();
        self.collapsed_thoughts.borrow_mut().clear();
        self.thinking_detached.borrow_mut().clear();
        self.tool_positions.borrow_mut().clear();
        self.hovered_turn.set(None);
        self.rail_scroll.set_offset(point(px(0.), px(0.)));
        self.rail_autoscroll.set(None);
        self.step_mark.set(None);
        self.toolcall_args.borrow_mut().clear();
        self.tail_summary.set(false);
        self.awaiting_reply.set(false);
    }

    /// Apply one protocol event; returns `true` if the transcript changed.
    pub fn apply_event(&mut self, event: &Event) -> bool {
        match event {
            Event::MessageStart { value } => self.on_message_boundary(value),
            Event::MessageUpdate { assistant, usage } => self.on_message_update(assistant, usage),
            Event::MessageEnd { value } => self.on_message_end(value),
            Event::ToolExecutionStart { value } => self.on_tool_execution(value, false),
            Event::ToolExecutionUpdate { value } => self.on_tool_execution_update(value),
            Event::ToolExecutionEnd { value } => self.on_tool_execution(value, true),
            // The provider request began: start the clock the live rate
            // divides by, matching pi's settled `durationMs`.
            Event::TurnStart => {
                self.turn_started.set(Some(Instant::now()));
                false
            }
            // A settled run closes its working clock; the next run re-arms it.
            Event::AgentSettled => {
                self.end_live_run();
                false
            }
            // `agent_end` fires between loop iterations while queued
            // steering/follow-ups keep the run going — clear the live
            // stream state but leave the summary card until the settle.
            Event::AgentEnd { will_retry } => {
                if !*will_retry {
                    self.clear_live_state();
                }
                false
            }
            Event::ProcessExited => {
                self.end_live_run();
                false
            }
            _ => false,
        }
    }

    fn on_message_boundary(&mut self, value: &Value) -> bool {
        // `toolResult` messages carry the output for an existing tool call —
        // never a chat row (a row here would also corrupt the stream target).
        if message_role(value) == Some("toolResult") {
            return self.attach_tool_result(value);
        }
        let Some(message) = ChatMessage::from_value(value) else {
            return false;
        };
        // A summary boundary is not a streamed turn: land it as its own row
        // (it cannot continue a run, and nothing may stream into it).
        if message.is_summary() {
            self.set_tail_row(false);
            let mut messages = self.messages.borrow_mut();
            messages.push(message);
            drop(messages);
            self.insert_row();
            return true;
        }
        let injected = injected_skill_message(value);
        if message.user {
            // A new turn begins: the previous run's end-of-task summary row
            // steps aside before the prompt row lands.
            self.set_tail_row(false);
        } else {
            // A streamed answer retires the pending row — the live message row
            // is the indicator now. Done before the list grows below so the
            // slot count stays exact. A user echo keeps the row: the answer is
            // still on its way.
            self.retire_pending_row();
        }
        let mut messages = self.messages.borrow_mut();
        if message.user {
            // pi restates a loaded skill as its own user message (the body
            // plus the prompt text). The optimistic prompt — or the compact
            // row a reload rebuilt — already represents this turn, so the
            // restatement is dropped rather than shown as a second bubble.
            if injected && messages.last().map(|m| m.user).unwrap_or(false) {
                return false;
            }
            // pi echoes the user's own message; show it (dedupe if identical).
            if messages
                .last()
                .map(|m| m.user && m.text() == message.text())
                != Some(true)
            {
                messages.push(message);
                self.streaming.set(None);
                self.stream_started.set(None);
                self.reset_generation();
                drop(messages);
                self.insert_row();
                return true;
            }
            return false;
        }
        // Assistant message starts. A turn's steps stream into ONE row
        // when the previous row is an assistant message, this step
        // continues it instead of stacking another "Worked" fold.
        //
        // The start snapshot may already hold the first content chunk (some
        // providers buffer one delta before emitting it); the matching first
        // delta then re-sends it. Flag the seed so that delta can drop it.
        let seed = message.steps.last();
        self.seed_text.set(seed.is_some_and(|s| !s.text.is_empty()));
        self.seed_thinking
            .set(seed.is_some_and(|s| !s.thinking.is_empty()));
        // Each step starts a fresh reasoning clock; a buffered start snapshot
        // that already carries reasoning begins it now.
        self.thinking_started.set(None);
        self.reset_generation();
        if seed.is_some_and(|s| !s.thinking.is_empty()) {
            self.begin_thinking();
        }
        let continues_run = messages.last().is_some_and(ChatMessage::is_assistant);
        if continues_run {
            let ix = messages.len() - 1;
            let slot = &mut messages[ix];
            self.step_mark.set(Some(StepMark {
                step: slot.steps.len(),
                text: 0,
                thinking: 0,
                tools: 0,
            }));
            merge_step(slot, message);
            self.streaming.set(Some(ix));
            self.mark_stream_start();
            drop(messages);
            self.remeasure_row(ix);
            return true;
        }
        messages.push(message);
        let ix = messages.len() - 1;
        self.streaming.set(Some(ix));
        self.step_mark.set(Some(StepMark::default()));
        self.mark_stream_start();
        drop(messages);
        self.insert_row();
        true
    }

    fn on_message_update(
        &mut self,
        assistant: &Option<AssistantMessageEvent>,
        usage: &Option<Value>,
    ) -> bool {
        use AssistantMessageEvent as Am;
        // `message_update.usage` is the provider's latest cumulative usage
        // for the response; keep it on the streaming step so the live tok/s
        // indicator reads real output counts when the provider reports them.
        if let Some(reported) = MessageUsage::from_value(usage.as_ref()) {
            self.store_live_usage(reported);
        }
        let Some(assistant) = assistant else {
            return false;
        };
        let (changed, created) = match assistant {
            Am::TextDelta { delta } => {
                let seeded = self.seed_text.replace(false);
                let mark = self.step_mark.get().map(|k| k.text).unwrap_or(0);
                // The step moved on from reasoning to its answer.
                let thinking = self.finish_thinking();
                let generation = self.note_generation();
                self.with_streaming(move |m| {
                    let step = m.current_step_mut();
                    if let Some(duration) = thinking {
                        if !step.thinking.is_empty() {
                            step.thinking_duration = Some(duration);
                        }
                    }
                    step.generation_time = Some(generation);
                    drop_seed(&mut step.text, mark, delta, seeded);
                    step.text.push_str(delta);
                })
            }
            Am::ThinkingDelta { delta } => {
                let seeded = self.seed_thinking.replace(false);
                let mark = self.step_mark.get().map(|k| k.thinking).unwrap_or(0);
                let started = self.begin_thinking();
                let generation = self.note_generation();
                self.with_streaming(move |m| {
                    let step = m.current_step_mut();
                    step.generation_time = Some(generation);
                    drop_seed(&mut step.thinking, mark, delta, seeded);
                    step.thinking.push_str(delta);
                    // Keep the per-thought clock ticking while it streams.
                    if let Some(started) = started {
                        step.thinking_duration = Some(started.elapsed());
                    }
                })
            }
            Am::ToolcallStart { value } => {
                let name = value
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                // pi 0.85 names the call id `id` (rpc.md: "toolcall_start
                // provides the call `id` and `toolName`"); older builds sent
                // `toolCallId`. Arguments are not carried here — they stream
                // in as `toolcall_delta` chunks.
                let id = value
                    .get("id")
                    .or_else(|| value.get("toolCallId"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // A fresh call streams a fresh argument buffer.
                self.toolcall_args.borrow_mut().clear();
                // Reasoning is done once the step starts emitting tool calls.
                let thinking = self.finish_thinking();
                let (changed, created) = self.with_streaming(|m| {
                    let step = m.current_step_mut();
                    if let Some(duration) = thinking {
                        if !step.thinking.is_empty() {
                            step.thinking_duration = Some(duration);
                        }
                    }
                    let mut tool = ToolCall::from_value(name, value.get("arguments"));
                    tool.id = id.clone();
                    step.tools.push(tool);
                });
                if changed {
                    let id = id.as_deref().map(str::to_string);
                    if let (Some(mix), Some(id)) = (self.streaming.get(), id) {
                        let messages = self.messages.borrow();
                        let step_ix = messages[mix].steps.len().saturating_sub(1);
                        let tool_ix = messages[mix].steps[step_ix].tools.len().saturating_sub(1);
                        self.tool_positions
                            .borrow_mut()
                            .insert(id, (mix, step_ix, tool_ix));
                    }
                }
                (changed, created)
            }
            Am::ToolcallDelta { value } => {
                // Arguments stream as raw JSON text chunks (rpc.md: "buffer
                // toolcall_delta.delta for arguments"). Apply them to the
                // row as soon as the accumulated buffer parses, so a command
                // or file path shows while the model is still writing it.
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    let generation = self.note_generation();
                    let mut buffer = self.toolcall_args.borrow_mut();
                    buffer.push_str(delta);
                    // Only a buffer that looks like a complete JSON object
                    // is worth a parse attempt — mid-stream chunks stay
                    // cheap (no O(n²) re-parse per chunk).
                    let looks_complete = buffer.trim_end().ends_with('}');
                    let parsed = looks_complete
                        .then(|| serde_json::from_str::<Value>(&buffer).ok())
                        .flatten();
                    drop(buffer);
                    match parsed {
                        Some(args) => self.with_streaming(move |m| {
                            let step = m.current_step_mut();
                            step.generation_time = Some(generation);
                            let Some(tool) = step.tools.last_mut() else {
                                return;
                            };
                            if tool.args.as_ref() == Some(&args) {
                                return;
                            }
                            // Rebuild so summary/path/line stats derive from
                            // the parsed arguments; keep live result state.
                            let mut rebuilt = ToolCall::from_value(&tool.name, Some(&args));
                            rebuilt.id = tool.id.clone();
                            rebuilt.output = tool.output.take();
                            rebuilt.failed = tool.failed;
                            rebuilt.facts = std::mem::take(&mut tool.facts);
                            *tool = rebuilt;
                        }),
                        None => (false, false),
                    }
                } else {
                    (false, false)
                }
            }
            Am::ToolcallEnd { value } => {
                // pi 0.85 nests the completed call under `toolCall`
                // (rpc.md: "toolcall_end.toolCall contains the completed
                // call"); older builds carried the fields flat on the event.
                let call = value.get("toolCall").unwrap_or(value);
                let name = call
                    .get("name")
                    .or_else(|| call.get("toolName"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let id = call
                    .get("id")
                    .or_else(|| call.get("toolCallId"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.toolcall_args.borrow_mut().clear();
                self.with_streaming(|m| {
                    let mut tool = ToolCall::from_value(name, call.get("arguments"));
                    tool.id = id.clone();
                    // Replace the matching call (by id; the placeholder from
                    // `toolcall_start` carries it) but keep any result
                    // already captured. Without an id, adopt the newest
                    // id-less row for the same tool before falling back to
                    // the newest row at all.
                    let step = m.current_step_mut();
                    let position = id
                        .as_deref()
                        .and_then(|id| step.tools.iter().position(|t| t.id.as_deref() == Some(id)))
                        .or_else(|| {
                            step.tools
                                .iter()
                                .rposition(|t| t.id.is_none() && t.name == name)
                        });
                    match position {
                        Some(ix) => {
                            tool.output = step.tools[ix].output.take();
                            tool.failed = step.tools[ix].failed;
                            tool.facts = std::mem::take(&mut step.tools[ix].facts);
                            step.tools[ix] = tool;
                        }
                        None => {
                            if let Some(last) = step.tools.last_mut() {
                                tool.output = last.output.take();
                                tool.failed = last.failed;
                                tool.facts = std::mem::take(&mut last.facts);
                                *last = tool;
                            } else {
                                step.tools.push(tool);
                            }
                        }
                    }
                })
            }
            Am::Other { kind, value } => match kind.as_str() {
                // Providers without streaming deliver whole blocks at once —
                // land the content even though no delta ever arrived.
                "text_end" | "thinking_end" => {
                    let content = value
                        .get("content")
                        .or_else(|| value.get("thinking"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    match content {
                        Some(content) => {
                            let is_text = kind == "text_end";
                            let mark = self
                                .step_mark
                                .get()
                                .map(|m| if is_text { m.text } else { m.thinking })
                                .unwrap_or(0);
                            // A whole-block (non-streaming) provider finishes
                            // reasoning when it delivers this block.
                            let thinking = self.finish_thinking();
                            self.with_streaming(move |m| {
                                let step = m.current_step_mut();
                                if let Some(duration) = thinking {
                                    if !step.thinking.is_empty() {
                                        step.thinking_duration = Some(duration);
                                    }
                                }
                                let target = if is_text {
                                    &mut step.text
                                } else {
                                    &mut step.thinking
                                };
                                // Deltas may already have streamed this block.
                                if target.ends_with(&content) {
                                    return;
                                }
                                let streamed = target.get(mark..).unwrap_or("");
                                if content.starts_with(streamed) {
                                    target.truncate(mark);
                                } else if !target.is_empty() {
                                    target.push_str("\n\n");
                                }
                                target.push_str(&content);
                            })
                        }
                        None => (false, false),
                    }
                }
                _ => (false, false),
            },
        };
        if changed {
            if created {
                self.insert_row();
            } else if let Some(ix) = self.streaming.get() {
                self.remeasure_row(ix);
            }
        }
        changed
    }

    fn on_message_end(&mut self, value: &Value) -> bool {
        // A settled tool result lands on its call — not a chat row (and not
        // a replacement for the streaming assistant message).
        if message_role(value) == Some("toolResult") {
            return self.attach_tool_result(value);
        }
        let Some(mut final_message) = ChatMessage::from_value(value) else {
            return false;
        };
        // A summary boundary is never a streamed turn; land it as its own row.
        if final_message.is_summary() {
            return self.on_message_boundary(value);
        }
        if final_message.user {
            // A new turn arriving as a settled message (no live boundary):
            // the previous run's summary row steps aside first.
            self.set_tail_row(false);
        }
        if !final_message.user {
            // The step's wall clock runs from the start of its turn; the
            // settled step replaces only its own slice of the merged row.
            final_message.elapsed = self.stream_started.get().map(|started| started.elapsed());
            // Close the reasoning clock: the settled snapshot carries no
            // timing, so the measured value must ride its step.
            let thinking = self.finish_thinking();
            if let Some(step) = final_message.steps.last_mut() {
                if thinking.is_some() && !step.thinking.is_empty() {
                    step.thinking_duration = thinking;
                }
            }
        }
        let mut messages = self.messages.borrow_mut();
        if final_message.user {
            // pi restates a loaded skill as its own user message. Its compact
            // form can differ from the typed prompt (`/skill:a  do x` →
            // `/skill:a do x`), so the text comparison below misses it — drop
            // the restatement when it lands on the prompt's row, mirroring
            // `on_message_boundary`.
            if injected_skill_message(value) && messages.last().map(|m| m.user).unwrap_or(false) {
                return false;
            }
            // A finalized user message replaces any optimistic copy.
            if let Some(last) = messages.last_mut() {
                if last.user && last.text() == final_message.text() {
                    return false;
                }
            }
            messages.push(final_message);
            drop(messages);
            self.insert_row();
            return true;
        }
        if let Some(ix) = self.streaming.get() {
            if let Some(slot) = messages.get_mut(ix) {
                // The settled step replaces only its own slice of the merged
                // row; results captured live by `tool_execution_end` survive
                // the replacement (pi's final toolCall blocks omit them).
                let mark = self.step_mark.get().unwrap_or(StepMark {
                    step: slot.steps.len().saturating_sub(1),
                    text: 0,
                    thinking: 0,
                    tools: 0,
                });
                let live_results: Vec<(Option<String>, Option<Value>, bool, ToolFacts)> = slot
                    .steps
                    .get(mark.step)
                    .map(|step| {
                        step.tools
                            .iter()
                            .skip(mark.tools)
                            .map(|t| (t.id.clone(), t.output.clone(), t.failed, t.facts.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                // The live step's measured reasoning clock survives the
                // settled replacement (pi's snapshot has no timing).
                let live_thinking = slot
                    .steps
                    .get(mark.step)
                    .and_then(|step| step.thinking_duration);
                // A provider that reports usage only while streaming (and
                // omits it from the final message) still leaves the
                // turn's token count intact.
                let live_usage = slot
                    .steps
                    .get(mark.step)
                    .and_then(|step| step.usage.clone());
                let live_generation = slot
                    .steps
                    .get(mark.step)
                    .and_then(|step| step.generation_time);
                // The settled step carries the run's wall clock — keep it
                // on the row (merge_step does this for non-stream paths).
                let step_elapsed = final_message.elapsed.take();
                let settled_error = final_message.error.take();
                let settled_aborted = final_message.aborted;
                let mut settled_step = final_message.into_step();
                // Re-attach results pi captured live (its settled blocks omit
                // them).
                if settled_step.thinking_duration.is_none() {
                    settled_step.thinking_duration = live_thinking;
                }
                if settled_step.usage.is_none() {
                    settled_step.usage = live_usage;
                }
                if settled_step.generation_time.is_none() {
                    settled_step.generation_time = live_generation;
                }
                for tool in &mut settled_step.tools {
                    if tool.output.is_some() {
                        continue;
                    }
                    let Some(id) = &tool.id else { continue };
                    if let Some((_, output, failed, facts)) = live_results
                        .iter()
                        .rev()
                        .find(|(live_id, _, _, _)| live_id.as_deref() == Some(id))
                    {
                        tool.output = output.clone();
                        tool.failed = *failed;
                        tool.facts = facts.clone();
                    }
                }
                match slot.steps.get_mut(mark.step) {
                    Some(step) => *step = settled_step,
                    None => slot.steps.push(settled_step),
                }
                if let Some(elapsed) = step_elapsed {
                    slot.elapsed = Some(elapsed);
                }
                if settled_error.is_some() {
                    slot.error = settled_error;
                }
                if settled_aborted {
                    slot.aborted = true;
                }
                if slot.finished_at.is_none() {
                    slot.finished_at = Some(now_millis());
                }
            }
            self.streaming.set(None);
            self.step_mark.set(None);
            drop(messages);
            self.remeasure_row(ix);
            return true;
        }
        // Settled without a stream target — continue the run's last row.
        let continues_run = messages.last().is_some_and(ChatMessage::is_assistant);
        if continues_run {
            let ix = messages.len() - 1;
            merge_step(&mut messages[ix], final_message);
            drop(messages);
            self.remeasure_row(ix);
            return true;
        }
        if final_message.finished_at.is_none() {
            final_message.finished_at = Some(now_millis());
        }
        messages.push(final_message);
        drop(messages);
        self.insert_row();
        true
    }

    /// Attach a `toolResult` message's output to the tool call it answers
    /// (by `toolCallId`). The live path knows positions from
    /// `tool_execution_start`; otherwise scan backwards through the loaded
    /// messages.
    fn attach_tool_result(&mut self, value: &Value) -> bool {
        let Some((id, output, failed, facts)) = tool_result_parts(value) else {
            return false;
        };
        let position = self.tool_positions.borrow().get(&id).copied();
        if let Some((mix, step_ix, tool_ix)) = position {
            let changed = {
                let mut messages = self.messages.borrow_mut();
                match messages
                    .get_mut(mix)
                    .and_then(|m| m.steps.get_mut(step_ix))
                    .and_then(|step| step.tools.get_mut(tool_ix))
                {
                    Some(tool) => {
                        let before = (tool.output.clone(), tool.failed, tool.facts.clone());
                        if tool.output.is_none() {
                            tool.output = output;
                        }
                        tool.failed = failed;
                        tool.facts = facts;
                        before != (tool.output.clone(), tool.failed, tool.facts.clone())
                    }
                    None => false,
                }
            };
            if changed {
                self.remeasure_row(mix);
            }
            return changed;
        }
        let mut messages = self.messages.borrow_mut();
        for message in messages.iter_mut().rev() {
            let Some(tool) = message
                .steps
                .iter_mut()
                .rev()
                .flat_map(|step| step.tools.iter_mut())
                .find(|t| t.id.as_deref() == Some(id.as_str()))
            else {
                continue;
            };
            let before = (tool.output.clone(), tool.failed, tool.facts.clone());
            if tool.output.is_none() {
                tool.output = output;
            }
            tool.failed = failed;
            tool.facts = facts;
            return before != (tool.output.clone(), tool.failed, tool.facts.clone());
        }
        false
    }

    /// `tool_execution_start` upserts the call on the streaming message and
    /// records where its result should land; `tool_execution_end` attaches it.
    fn on_tool_execution(&mut self, value: &Value, end: bool) -> bool {
        let id = value
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if end {
            let Some(id) = id.as_deref() else {
                return false;
            };
            let Some(&(mix, step_ix, tool_ix)) = self.tool_positions.borrow().get(id) else {
                return false;
            };
            let changed = {
                let mut messages = self.messages.borrow_mut();
                let Some(tool) = messages
                    .get_mut(mix)
                    .and_then(|m| m.steps.get_mut(step_ix))
                    .and_then(|step| step.tools.get_mut(tool_ix))
                else {
                    return false;
                };
                let result = value.get("result").or_else(|| value.get("partialResult"));
                let before = (tool.output.clone(), tool.failed, tool.facts.clone());
                tool.facts = result.map(ToolFacts::from_result).unwrap_or_default();
                tool.output = result.map(normalize_tool_result);
                tool.failed = value
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                before != (tool.output.clone(), tool.failed, tool.facts.clone())
            };
            if changed {
                self.remeasure_row(mix);
            }
            return changed;
        }
        // Start: upsert on the streaming message (creating it if a tool
        // execution begins before any `message_start`).
        // The gap until the next delta is execution, not decode.
        self.pause_generation();
        let name = value
            .get("toolName")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let mut created = false;
        let mut messages = self.messages.borrow_mut();
        let mix = match self.streaming.get() {
            Some(mix) => mix,
            None => {
                // Execution starts after the assistant step settled — adopt
                // the run's merged row instead of stacking a new one.
                if messages.last().is_some_and(ChatMessage::is_assistant) {
                    let ix = messages.len() - 1;
                    self.step_mark.set(Some(StepMark {
                        step: messages[ix].steps.len().saturating_sub(1),
                        text: 0,
                        thinking: 0,
                        tools: 0,
                    }));
                    self.streaming.set(Some(ix));
                    self.mark_stream_start();
                    ix
                } else {
                    messages.push(ChatMessage::empty_assistant());
                    let mix = messages.len() - 1;
                    self.streaming.set(Some(mix));
                    self.mark_stream_start();
                    created = true;
                    mix
                }
            }
        };
        let Some(message) = messages.get_mut(mix) else {
            return false;
        };
        let step_ix = message.steps.len().saturating_sub(1);
        let Some(step) = message.steps.get_mut(step_ix) else {
            return false;
        };
        let position = id
            .as_deref()
            .and_then(|id| step.tools.iter().position(|t| t.id.as_deref() == Some(id)))
            .or_else(|| {
                // Adopt an id-less row created by `toolcall_start` for the
                // same call instead of duplicating it.
                let last = step.tools.last_mut()?;
                if last.id.is_none() && last.name == name {
                    if let Some(id) = &id {
                        last.id = Some(id.clone());
                    }
                    Some(step.tools.len() - 1)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                let mut tool = ToolCall::from_value(&name, value.get("args"));
                tool.id = id.clone();
                step.tools.push(tool);
                step.tools.len() - 1
            });
        if let Some(id) = &id {
            self.tool_positions
                .borrow_mut()
                .insert(id.clone(), (mix, step_ix, position));
        }
        drop(messages);
        if created {
            self.insert_row();
        } else {
            self.remeasure_row(mix);
        }
        true
    }

    /// `tool_execution_update` streams the accumulated `partialResult` —
    /// replace the call's output in place so an open detail card follows the
    /// command's progress. The row only remeasures when the growing output
    /// is visible (the tool's detail card is open); a closed card's height
    /// does not change, so streaming output never churns the list layout.
    fn on_tool_execution_update(&mut self, value: &Value) -> bool {
        let Some(id) = value.get("toolCallId").and_then(Value::as_str) else {
            return false;
        };
        let Some(partial) = value.get("partialResult") else {
            return false;
        };
        let Some(&(mix, step_ix, tool_ix)) = self.tool_positions.borrow().get(id) else {
            return false;
        };
        let output = normalize_tool_result(partial);
        let facts = ToolFacts::from_result(partial);
        let mut messages = self.messages.borrow_mut();
        let Some(message) = messages.get_mut(mix) else {
            return false;
        };
        // Flat tool index across the message's steps — the detail/copy key
        // space the view uses.
        let flat_ix = message
            .steps
            .iter()
            .take(step_ix)
            .map(|step| step.tools.len())
            .sum::<usize>()
            + tool_ix;
        let Some(tool) = message
            .steps
            .get_mut(step_ix)
            .and_then(|step| step.tools.get_mut(tool_ix))
        else {
            return false;
        };
        if tool.output.as_ref() == Some(&output) && tool.facts == facts {
            return false;
        }
        tool.output = Some(output);
        tool.facts = facts;
        let detail_open = self.expanded_tools.borrow().contains(&(mix, flat_ix));
        drop(messages);
        if detail_open {
            self.remeasure_row(mix);
        }
        true
    }

    /// Mutate the assistant message currently being streamed. A turn's
    /// steps share one row: with no stream target, a previous assistant row
    /// continues (with a step mark) instead of stacking a new one. Returns
    /// `(changed, created)` — `created` inserts a list row.
    fn with_streaming(&mut self, mutate: impl FnOnce(&mut ChatMessage)) -> (bool, bool) {
        let mut messages = self.messages.borrow_mut();
        let (ix, created) = match self.streaming.get() {
            Some(ix) => (ix, false),
            None => {
                if messages.last().is_some_and(ChatMessage::is_assistant) {
                    // Continue the settled step's row with a fresh step.
                    let ix = messages.len() - 1;
                    self.step_mark.set(Some(StepMark {
                        step: messages[ix].steps.len(),
                        text: 0,
                        thinking: 0,
                        tools: 0,
                    }));
                    self.reset_generation();
                    self.streaming.set(Some(ix));
                    (ix, false)
                } else {
                    messages.push(ChatMessage::empty_assistant());
                    let ix = messages.len() - 1;
                    self.streaming.set(Some(ix));
                    self.step_mark.set(Some(StepMark::default()));
                    self.reset_generation();
                    (ix, true)
                }
            }
        };
        self.mark_stream_start();
        let changed = messages
            .get_mut(ix)
            .map(|message| {
                // Deltas stream into the run's current (last) step.
                if message.steps.is_empty() {
                    message.steps.push(Step::default());
                }
                mutate(message);
                true
            })
            .unwrap_or(false);
        (changed, created)
    }

    /// Keep the provider's latest cumulative `message_update.usage` on the
    /// step currently streaming. Never opens a row: a usage-only update is
    /// not a reason to create one.
    fn store_live_usage(&self, usage: MessageUsage) {
        let Some(ix) = self.streaming.get() else {
            return;
        };
        let mut messages = self.messages.borrow_mut();
        if let Some(step) = messages
            .get_mut(ix)
            .and_then(|message| message.steps.last_mut())
        {
            step.usage = Some(usage);
        }
    }

    /// Longest inter-delta gap still counted as generation. Tool execution,
    /// provider stalls, and the round-trip before a step's first token all
    /// exceed this, so they stay out of the rate.
    const GENERATION_GAP: Duration = Duration::from_millis(1_000);

    /// Begin a fresh step's generation clock.
    fn reset_generation(&self) {
        self.generation_time.set(Duration::ZERO);
        self.last_delta_at.set(None);
    }

    /// Stop counting until the next delta — a tool is running, so the gap
    /// is execution, not decode.
    fn pause_generation(&self) {
        self.last_delta_at.set(None);
    }

    /// Account for one content delta (text, thinking, or tool-call
    /// arguments) and return the step's accumulated generation time.
    fn note_generation(&self) -> Duration {
        let now = Instant::now();
        if let Some(previous) = self.last_delta_at.replace(Some(now)) {
            let gap = now.saturating_duration_since(previous);
            if gap <= Self::GENERATION_GAP {
                self.generation_time.set(self.generation_time.get() + gap);
            }
        }
        self.generation_time.get()
    }

    fn mark_stream_start(&self) {
        if self.stream_started.get().is_none() {
            self.stream_started.set(Some(Instant::now()));
        }
    }

    /// Mark the start of the current step's reasoning (once) and return the
    /// clock — `None` only if reasoning already finished.
    fn begin_thinking(&self) -> Option<Instant> {
        if self.thinking_started.get().is_none() {
            self.thinking_started.set(Some(Instant::now()));
        }
        self.thinking_started.get()
    }

    /// Close the current step's reasoning clock, returning how long it ran.
    fn finish_thinking(&self) -> Option<Duration> {
        self.thinking_started
            .take()
            .map(|started| started.elapsed())
    }

    fn insert_row(&self) {
        self.scroller.append(1);
        self.scroller.note_activity();
    }

    /// Same item count, new height — do not insert a blank row.
    fn remeasure_row(&self, ix: usize) {
        self.scroller.remeasure_items(ix..ix + 1);
        self.scroller.note_activity();
    }

    /// Recover if stream updates previously inserted extra list slots.
    fn resync_list(&self) {
        let count = self.messages.borrow().len() + self.wants_tail_row() as usize;
        if self.scroller.item_count() != count {
            self.scroller.reset(count);
        }
    }

    /// One slot after the last message serves two ends: the pending-reply row
    /// while a prompt is out, and the end-of-task changed-files card once the
    /// run settles. Both are "a row after the last message", so they share the
    /// slot instead of leaving an empty row between them.
    fn wants_tail_row(&self) -> bool {
        self.tail_summary.get() || self.awaiting_reply.get()
    }

    /// Add or remove the tail slot to match [`Self::wants_tail_row`], leaving
    /// the message rows alone (a pending reply must not re-scroll the reader).
    fn sync_tail_row(&self) {
        let present = self.wants_tail_row();
        let count = self.scroller.item_count();
        let occupied = count > self.messages.borrow().len();
        match (present, occupied) {
            (true, false) => {
                self.scroller.append(1);
            }
            (false, true) => {
                self.scroller.splice(count.saturating_sub(1)..count, 0);
            }
            _ => {}
        }
    }

    /// Take the pending row down. Called *before* the message list grows, so
    /// the slot bookkeeping (row count == messages + tail) stays exact.
    fn retire_pending_row(&self) {
        if !self.awaiting_reply.get() {
            return;
        }
        self.awaiting_reply.set(false);
        self.sync_tail_row();
    }

    /// The prompt is away: show the live row now, and start the turn's clock
    /// at the send rather than at the first token, so the indicator cannot sit
    /// silent while the provider is thinking (or the process is still booting).
    pub fn expect_reply(&mut self) {
        self.awaiting_reply.set(true);
        self.mark_stream_start();
        self.sync_tail_row();
    }

    /// The turn never started (pi rejected the prompt) or the run is over:
    /// drop the pending row and its clock.
    pub fn abandon_reply(&mut self) {
        if !self.awaiting_reply.get() && self.stream_started.get().is_none() {
            return;
        }
        self.awaiting_reply.set(false);
        if self.streaming.get().is_none() {
            self.stream_started.set(None);
        }
        self.sync_tail_row();
    }

    /// Every file the task changed, across all turns, merged by path with
    /// summed +/− line counts — the end-of-task summary card's data.
    pub fn changed_files_summary(&self) -> Vec<(String, u64, u64)> {
        let messages = self.messages.borrow();
        let mut files: Vec<(String, u64, u64)> = Vec::new();
        for message in messages.iter() {
            for (path, added, removed) in changed_files(message) {
                match files.iter_mut().find(|(existing, _, _)| *existing == path) {
                    Some(entry) => {
                        entry.1 += added;
                        entry.2 += removed;
                    }
                    None => files.push((path, added, removed)),
                }
            }
        }
        files
    }

    /// Add or remove the end-of-task summary row (the extra list slot after
    /// the last message row). Shown once a run settles with file edits;
    /// removed when a new turn begins or the transcript rebuilds.
    fn set_tail_row(&self, present: bool) {
        if present == self.tail_summary.get() {
            return;
        }
        self.tail_summary.set(present);
        // A pending prompt owns the slot in between: adding the summary while
        // the live row is up must not double it, and removing it must not take
        // the live row down with it.
        self.sync_tail_row();
    }

    /// True when no messages are loaded (drives the empty state).
    pub fn is_empty(&self) -> bool {
        self.messages.borrow().is_empty()
    }

    /// Text of the first real user prompt, if any. The sidebar uses it as the
    /// placeholder row's preview while the session file is not yet on disk
    /// (pi flushes lazily), so the open session never reads as title-only.
    pub fn first_user_message(&self) -> Option<String> {
        self.messages
            .borrow()
            .iter()
            .find(|message| message.user && !message.text().trim().is_empty())
            .map(|message| message.text())
    }

    /// Mark the one-time rail hint as seen — it stops rendering and the
    /// dismissal persists for this install. Returns true on the first call
    /// (i.e. the UI should repaint).
    pub fn dismiss_rail_hint(&self) -> bool {
        dismiss_rail_hint_state(&self.rail_hint_dismissed, &self.rail_hint_shown_at)
    }

    /// Heartbeat check: self-dismiss the rail hint once its TTL elapsed.
    /// Returns true when the dismissal just happened (repaint needed).
    pub fn rail_hint_timed_out(&self) -> bool {
        let Some(shown_at) = self.rail_hint_shown_at.get() else {
            return false;
        };
        if self.rail_hint_dismissed.get() || shown_at.elapsed() < RAIL_HINT_TTL {
            return false;
        }
        self.dismiss_rail_hint()
    }

    /// Text of the most recent assistant response (streaming partials
    /// included), with its row index for the copy feedback — the keyboard
    /// mirror of the footer copy button.
    pub fn last_response_text(&self) -> Option<(usize, String)> {
        let messages = self.messages.borrow();
        let ix = messages
            .iter()
            .rposition(|message| message.is_assistant() && !message.text().trim().is_empty())?;
        Some((ix, messages[ix].text()))
    }

    /// The settled run's last assistant message usage, for the debug-build
    /// per-turn diagnostic log.
    pub fn last_message_usage(&self) -> Option<MessageUsage> {
        self.messages.borrow().last().and_then(ChatMessage::usage)
    }

    /// The settled run's last message generation clock — pi's `durationMs`
    /// (prefill plus decode), or the client-measured fallback.
    pub fn last_message_duration(&self) -> Option<Duration> {
        self.messages.borrow().last().and_then(ChatMessage::generation_time)
    }

    /// Summarize the turn that just settled for a background notification:
    /// the newest assistant message's answer (or the agent error that ended
    /// it) and whether the user aborted it. A user message newer than the
    /// newest answer means this run produced none — the summary stays empty
    /// rather than reporting the previous turn's text.
    pub fn latest_turn_summary(&self) -> Option<TurnSummary> {
        let messages = self.messages.borrow();
        let Some(ix) = messages.iter().rposition(ChatMessage::is_assistant) else {
            // A settle with no assistant message at all (the run failed
            // before any text); still worth a quiet "turn finished" ping.
            return Some(TurnSummary::default());
        };
        if ix + 1 != messages.len() {
            return Some(TurnSummary::default());
        }
        let message = &messages[ix];
        if message.aborted {
            return Some(TurnSummary {
                aborted: true,
                ..TurnSummary::default()
            });
        }
        let text = message.text();
        Some(TurnSummary {
            body: if text.trim().is_empty() {
                message.error.clone().unwrap_or_default()
            } else {
                text
            },
            failed: message.error.is_some(),
            aborted: false,
        })
    }

    /// Pin the per-message copy feedback (green check) as if the footer
    /// button had been clicked.
    pub fn mark_copied(&self, ix: usize) {
        self.copied.borrow_mut().insert(ix, Instant::now());
    }

    /// Jump the scroller to the previous (`direction < 0`) or next user
    /// turn relative to the current viewport — the keyboard mirror of the
    /// navigation rail's ticks. Returns `false` when there is nowhere to go.
    pub fn jump_turn(&self, direction: i32) -> bool {
        let target = {
            let messages = self.messages.borrow();
            let user_turns: Vec<usize> = messages
                .iter()
                .enumerate()
                .filter(|(_, message)| message.user)
                .map(|(ix, _)| ix)
                .collect();
            if user_turns.is_empty() {
                return false;
            }
            // At the live edge the reader is on the newest turn; the
            // viewport hint only anchors a reader who scrolled away. Without
            // this, ⌘↑ from the tail compared against a stale row 0 and never
            // moved.
            let current = if self.scroller.is_following_tail() {
                self.scroller.item_count()
            } else {
                self.scroller.first_visible_index()
            };
            // The turn the viewport is reading: the newest user turn at or
            // above the first visible row (same rule the rail uses).
            let pos = user_turns
                .iter()
                .rposition(|&ix| ix <= current)
                .unwrap_or(0);
            let next_pos = if direction < 0 {
                pos.saturating_sub(1)
            } else {
                (pos + 1).min(user_turns.len() - 1)
            };
            if next_pos == pos {
                return false;
            }
            user_turns[next_pos]
        };
        self.scroller.scroll_to_item(target)
    }

    /// Show the just-sent prompt immediately — pi does not echo user
    /// messages in RPC mode. A later identical echo is deduped. `images`
    /// are the attachments that rode along with the prompt.
    pub fn append_user_message(&mut self, text: &str, images: Vec<Arc<Image>>) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }
        // A new task begins: the previous run's summary row steps aside.
        self.set_tail_row(false);
        let mut messages = self.messages.borrow_mut();
        if messages.last().map(|m| m.user && m.text() == trimmed) == Some(true) {
            return false;
        }
        messages.push(ChatMessage {
            user: true,
            summary: None,
            steps: vec![Step {
                text: trimmed.to_string(),
                ..Step::default()
            }],
            images,
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        });
        drop(messages);
        self.insert_row();
        true
    }

    /// Drop live streaming state so the working indicator and stop
    /// affordances clear even if a message end was missed.
    fn clear_live_state(&mut self) {
        self.streaming.set(None);
        self.step_mark.set(None);
        self.stream_started.set(None);
        self.turn_started.set(None);
        self.thinking_started.set(None);
        self.reset_generation();
        // A run that ends without ever streaming (an aborted first request, a
        // provider that answered with an error) stops waiting too.
        self.awaiting_reply.set(false);
        self.sync_tail_row();
    }

    /// Turn end (or abort): drop live streaming state, then — once the run
    /// has settled — pin the whole-task changed-files summary at the end of
    /// the transcript (when the task touched files).
    fn end_live_run(&mut self) {
        self.clear_live_state();
        let touched_files = !self.changed_files_summary().is_empty();
        self.set_tail_row(touched_files);
    }

    /// True while an assistant message is still streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming.get().is_some()
    }

    /// True from the moment a prompt is sent until its first step streams (or
    /// the turn is abandoned). A turn is in flight across that whole window, so
    /// the app's `is_running` — and every gate built on it — must see it.
    pub fn is_awaiting_reply(&self) -> bool {
        self.awaiting_reply.get()
    }

    /// Message indices whose text contains `needle` (already lowercased), in
    /// append order — drives the in-transcript find.
    pub fn find_messages(&self, needle: &str) -> Vec<usize> {
        self.messages
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, message)| message.text().to_lowercase().contains(needle))
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Number of messages currently held (the find uses it to notice new
    /// arrivals while the bar is open).
    pub fn message_count(&self) -> usize {
        self.messages.borrow().len()
    }

    /// Scroll the transcript so message `ix` sits at the top of the viewport.
    pub fn scroll_to_message(&self, ix: usize) -> bool {
        self.scroller.scroll_to_item(ix)
    }

    /// chars/4 heuristic over loaded messages — same estimate pi's /context
    /// view uses when the provider-serialized payload isn't available.
    pub fn estimated_tokens(&self) -> u64 {
        let chars: usize = self
            .messages
            .borrow()
            .iter()
            .map(|message| {
                message
                    .steps
                    .iter()
                    .map(|step| {
                        step.text.chars().count()
                            + step.thinking.chars().count()
                            + step
                                .tools
                                .iter()
                                .map(|tool| {
                                    tool.name.chars().count() + tool.summary.chars().count()
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>()
            })
            .sum();
        chars.div_ceil(4) as u64
    }

    /// Drop stale Copy feedback. Returns true while any entry still needs a tick.
    pub fn prune_copy_feedback(&self) -> bool {
        let mut copied = self.copied.borrow_mut();
        if copied.is_empty() {
            return false;
        }
        copied.retain(|_, at| at.elapsed() < Duration::from_secs(2));
        self.copied_sections
            .borrow_mut()
            .retain(|_, at| at.elapsed() < Duration::from_secs(2));
        true
    }

    /// Render into the chat panel — rail + centered 760px column. The
    /// workspace roots the changed-files Review git diff; the viewport
    /// height caps the rail, and the main-area width gates its visibility.
    /// `review_changes` wires the cards' Review buttons to the side pane.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        workspace: Option<&Path>,
        viewport_height: Pixels,
        main_width: Pixels,
        review_changes: Option<crate::transcript_view::ReviewOpener>,
        image_opener: Option<crate::transcript_view::ImageOpener>,
        search_hits: Option<HashSet<usize>>,
        search_active: Option<usize>,
        cx: &gpui::App,
    ) -> impl IntoElement + use<> {
        self.resync_list();
        // The tail slot exists only when a settled run left changed files.
        let summary_files = if self.tail_summary.get() {
            Some(self.changed_files_summary())
        } else {
            None
        };
        // Footer stamp: when the settled run's last message finished.
        let summary_finished_at = if summary_files.is_some() {
            self.messages.borrow().last().and_then(|m| m.finished_at)
        } else {
            None
        };
        // The run's whole usage rides the summary footer so the copy/time/
        // usage details show once, on the last element.
        let summary_usage = if summary_files.is_some() {
            self.messages.borrow().last().and_then(|m| m.usage())
        } else {
            None
        };
        transcript_view::render_transcript(
            TranscriptView {
                messages: self.messages.clone(),
                user_turns: self.cached_user_turns(),
                text_selection: self.text_selection.clone(),
                scroller: self.scroller.clone(),
                streaming: self.streaming.clone(),
                stream_started: self.stream_started.clone(),
                awaiting_reply: self.awaiting_reply.clone(),
                turn_started: self.turn_started.clone(),
                expanded_turns: self.expanded_turns.clone(),
                expanded_files: self.expanded_files.clone(),
                expanded_activities: self.expanded_activities.clone(),
                expanded_tools: self.expanded_tools.clone(),
                copied: self.copied.clone(),
                copied_sections: self.copied_sections.clone(),
                expanded_sections: self.expanded_sections.clone(),
                expanded_blocks: self.expanded_blocks.clone(),
                thinking_scrolls: self.thinking_scrolls.clone(),
                collapsed_thoughts: self.collapsed_thoughts.clone(),
                thinking_detached: self.thinking_detached.clone(),
                hovered_turn: self.hovered_turn.clone(),
                hovered_usage: self.hovered_usage.clone(),
                rail_hint_dismissed: self.rail_hint_dismissed.clone(),
                rail_hint_shown_at: self.rail_hint_shown_at.clone(),
                workspace: workspace.map(Path::to_path_buf),
                viewport_height,
                main_width,
                rail_scroll: self.rail_scroll.clone(),
                rail_autoscroll: self.rail_autoscroll.clone(),
                summary_files,
                summary_finished_at,
                summary_usage,
                review_changes,
                image_opener,
                search_hits: search_hits.map(|hits| Rc::new(RefCell::new(hits))),
                search_active: search_active.map(|ix| Rc::new(Cell::new(Some(ix)))),
            },
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Sending a prompt must put the live row up at once — the regression is a
    /// transcript that stays silent from Enter until the first token lands.
    #[test]
    fn sending_a_prompt_shows_the_live_row_immediately() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ship it", Vec::new());
        assert!(!transcript.awaiting_reply.get(), "nothing is in flight yet");
        assert_eq!(transcript.scroller.item_count(), 1);

        transcript.expect_reply();
        assert!(transcript.awaiting_reply.get(), "the tail row goes up now");
        assert!(
            transcript.stream_started.get().is_some(),
            "the Working clock starts at the send, not at the first token"
        );
        // The row is the list's tail slot: messages + 1.
        assert_eq!(transcript.scroller.item_count(), 2);
    }

    /// The first streamed step takes the pending row down and leaves exactly
    /// one row per message (no phantom slot between the live row and the
    /// assistant turn).
    #[test]
    fn the_first_streamed_step_retires_the_pending_row() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ship it", Vec::new());
        transcript.expect_reply();

        transcript.apply_event(&Event::MessageStart {
            value: json!({
                "type": "message_start",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "On it" }] },
            }),
        });

        assert!(!transcript.awaiting_reply.get());
        assert!(transcript.is_streaming());
        assert_eq!(transcript.message_count(), 2);
        assert_eq!(
            transcript.scroller.item_count(),
            transcript.message_count(),
            "the tail slot closed as the live row became a message row"
        );
    }

    /// A rejected prompt (or a run that ends without streaming) must stop
    /// waiting: otherwise the composer stays on Stop and the row spins forever.
    #[test]
    fn abandoning_a_prompt_closes_the_pending_row() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ship it", Vec::new());
        transcript.expect_reply();
        assert_eq!(transcript.scroller.item_count(), 2);

        transcript.abandon_reply();
        assert!(!transcript.awaiting_reply.get());
        assert!(transcript.stream_started.get().is_none());
        assert_eq!(transcript.scroller.item_count(), 1);
        // Idempotent: unwinding twice is not a second row operation.
        transcript.abandon_reply();
        assert_eq!(transcript.scroller.item_count(), 1);
    }

    /// The end-of-task summary and the pending row share the tail slot, so a
    /// settled task's card does not sit below an empty pending row.
    #[test]
    fn the_summary_and_the_pending_row_share_the_tail_slot() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ship it", Vec::new());
        transcript.expect_reply();
        assert_eq!(transcript.scroller.item_count(), 2);

        transcript.set_tail_row(true);
        assert_eq!(transcript.scroller.item_count(), 2, "no second tail row");
        transcript.retire_pending_row();
        assert_eq!(
            transcript.scroller.item_count(),
            2,
            "the card keeps the slot"
        );

        transcript.set_tail_row(false);
        assert_eq!(transcript.scroller.item_count(), 1);
    }

    #[test]
    fn assistant_error_message_surfaces_stop_reason_error() {
        // The exact shape pi emits when the provider rejects the request:
        // `stopReason: "error"` plus `errorMessage` (see agent-loop).
        let value = json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [],
                "stopReason": "error",
                "errorMessage": "Codex error: The 'gpt-5.3-codex-spark' model is not supported when using Codex with a ChatGPT account."
            }
        });
        let expected = "Codex error: The 'gpt-5.3-codex-spark' model is not supported when using Codex with a ChatGPT account.";
        assert_eq!(message_error(&value).as_deref(), Some(expected));
        let parsed = ChatMessage::from_value(&value).expect("assistant message");
        assert!(!parsed.user);
        assert_eq!(parsed.error.as_deref(), Some(expected));

        // A clean assistant message carries no error.
        let ok = json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}],
            "stopReason": "stop"
        });
        assert!(message_error(&ok).is_none());
        assert!(ChatMessage::from_value(&ok).unwrap().error.is_none());

        // User messages never surface a provider error.
        let user = json!({"role": "user", "content": "hi", "errorMessage": "ignored"});
        assert!(message_error(&user).is_none());
    }

    #[test]
    fn aborted_message_carries_no_agent_error() {
        // pi's `createAbortedMessage`: a user stop marks the message aborted
        // but still carries `errorMessage`. The transcript must show only its
        // "Stopped" marker, never an error card or the app banner.
        let value = json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "partial"}],
                "stopReason": "aborted",
                "errorMessage": "Request was aborted"
            }
        });
        assert!(message_error(&value).is_none());
        let parsed = ChatMessage::from_value(&value).expect("assistant message");
        assert!(parsed.aborted);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn real_session_payload_renders_changed_files_card() {
        // Ground-truth check against the live pi session when present:
        // `get_messages` snapshots must yield edit/write tools with paths and
        // line counts, otherwise the changed-files card never appears.
        // Regenerates the payload from the newest real session on disk.
        let mut data = None;
        if let Ok(sample) = std::fs::read_to_string("/tmp/orbit-sample.json") {
            data = serde_json::from_str(&sample).ok();
        }
        let data = data.unwrap_or_else(|| {
            let pattern = format!(
                "{}/.pi/agent/sessions/*/*.jsonl",
                std::env::var("HOME").unwrap_or_default()
            );
            let mut files: Vec<std::path::PathBuf> = glob_files(&pattern);
            files.sort_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok());
            for path in files.into_iter().rev() {
                let Ok(content) = std::fs::read_to_string(path) else {
                    continue;
                };
                let messages: Vec<Value> = content
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .filter_map(|entry: Value| {
                        let message = entry.get("message").cloned();
                        message.filter(|m| {
                            m.get("role")
                                .and_then(Value::as_str)
                                .is_some_and(|role| role == "user" || role == "assistant")
                        })
                    })
                    .collect();
                if messages.iter().any(|m| {
                    serde_json::to_string(m).is_ok_and(|s| s.contains("\"name\":\"edit\""))
                }) {
                    return serde_json::json!({ "messages": messages });
                }
            }
            eprintln!("skipping: no real session data");
            serde_json::json!({ "messages": [] })
        });
        let mut transcript = Transcript::new();
        transcript.load_from(&data);
        let messages = transcript.messages.borrow();
        if messages.is_empty() {
            eprintln!("skipping: empty session");
            return;
        }
        let cards: Vec<_> = messages
            .iter()
            .filter(|m| !changed_files(m).is_empty())
            .collect();
        assert!(
            !cards.is_empty(),
            "edit/write tool calls must produce changed-files cards"
        );
        assert!(
            cards
                .iter()
                .any(|m| m.tools().any(|t| t.added > 0 || t.removed > 0)),
            "edit tools must carry line counts"
        );
    }

    /// Sorted file list for a glob-ish pattern (no glob dep — simple scan).
    fn glob_files(pattern: &str) -> Vec<std::path::PathBuf> {
        let (dir, _rest) = pattern.split_once("/*/").unwrap_or((pattern, ""));
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if let Ok(sessions) = std::fs::read_dir(entry.path()) {
                        for session in sessions.flatten() {
                            if session.path().extension().is_some_and(|e| e == "jsonl") {
                                out.push(session.path());
                            }
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn tool_results_never_become_rows_and_attach_to_calls() {
        let payload = json!({
            "messages": [
                {"role": "user", "content": "fix it"},
                {
                    "role": "assistant",
                    "content": [{
                        "type": "toolCall",
                        "id": "call_1",
                        "name": "bash",
                        "arguments": {"command": "tsc"}
                    }]
                },
                {
                    "role": "toolResult",
                    "toolCallId": "call_1",
                    "toolName": "bash",
                    "isError": true,
                    "content": [{
                        "type": "text",
                        "text": "src/app.tsx(1,1): error TS1005: ')' expected."
                    }]
                }
            ]
        });
        let mut transcript = Transcript::new();
        transcript.load_from(&payload);
        let messages = transcript.messages.borrow();
        // Two rows: the prompt and the assistant call. The toolResult must
        // NOT add a prose row (that is what mangled code into paragraphs).
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].tools().count(), 1);
        let tool = messages[1].tools().next().unwrap();
        assert!(tool.failed);
        assert_eq!(
            tool.output
                .as_ref()
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("src/app.tsx(1,1): error TS1005: ')' expected.")
        );
    }

    #[test]
    fn live_nested_events_render_and_settle() {
        // pi wraps live message events: {"type": "message_end", "message": {...}}.
        // Unwrapped parsing previously dropped the echo, the streaming text
        // (for non-delta providers) and left the working state stuck.
        let mut t = Transcript::new();
        // Non-delta provider: whole content arrives via text_end.
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "assistant", "content": [], "stopReason": "pending"
            }}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::Other {
                kind: "text_end".into(),
                value: json!({"type": "text_end", "content": "hello", "contentIndex": 0}),
            }),
        });
        assert!(t.is_streaming());
        {
            let messages = t.messages.borrow();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text(), "hello");
        }
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "hello"}],
                "stopReason": "stop"
            }}),
        });
        assert!(!t.is_streaming(), "message end must clear the live state");
        let messages = t.messages.borrow();
        assert_eq!(messages[0].text(), "hello");
        assert!(messages[0].finished_at.is_some());
    }

    /// Deltas accrue a client-measured generation clock, and the settled
    /// replacement must carry it forward so the footer's tok/s does not
    /// vanish when the turn ends.
    #[test]
    fn live_deltas_accrue_generation_time_and_survive_settle() {
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "assistant", "content": [], "stopReason": "pending"
            }}),
        });
        for delta in ["Hel", "lo"] {
            t.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::TextDelta {
                    delta: delta.to_string(),
                }),
            });
        }
        {
            let messages = t.messages.borrow();
            assert_eq!(messages[0].text(), "Hello");
            assert!(
                messages[0].steps[0].generation_time.is_some(),
                "the live step must carry a generation clock"
            );
        }
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello"}],
                "stopReason": "stop"
            }}),
        });
        let messages = t.messages.borrow();
        assert!(
            messages[0].generation_time().is_some(),
            "generation time must survive the settled replacement"
        );
        assert!(!t.is_streaming());
    }

    /// pi's own `durationMs` rides the settled step and wins over any
    /// client-measured value — the client only ever saw pi's bursts, so its
    /// clock reads far too short (and the rate far too high).
    #[test]
    fn pi_duration_ms_wins_the_generation_clock() {
        let parsed = ChatMessage::from_value(&json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}],
                "stopReason": "stop",
                "durationMs": 5237,
                "usage": {"input": 10, "output": 4, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 14}
            }
        }))
        .expect("assistant message");
        assert_eq!(parsed.steps[0].duration, Some(Duration::from_millis(5237)));

        // When pi supplies a duration it wins; without it, the client
        // measurement is still used as the fallback.
        let mut message = ChatMessage {
            user: false,
            summary: None,
            steps: vec![Step {
                generation_time: Some(Duration::from_secs(3)),
                duration: Some(Duration::from_secs(9)),
                ..Step::default()
            }],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };
        assert_eq!(message.generation_time(), Some(Duration::from_secs(9)));
        message.steps[0].duration = None;
        assert_eq!(message.generation_time(), Some(Duration::from_secs(3)));
    }

    /// `turn_start` arms the live rate's clock — the same basis as pi's
    /// `durationMs` — so a bursty stream is not mistaken for the whole
    /// generation. Clearing live state disarms it for the next run.
    #[test]
    fn turn_start_arms_the_live_rate_clock() {
        let mut t = Transcript::new();
        assert!(t.turn_started.get().is_none());
        assert!(!t.apply_event(&Event::TurnStart));
        assert!(t.turn_started.get().is_some());
        t.clear_live_state();
        assert!(t.turn_started.get().is_none());
    }

    /// `generation_time()` sums the turn's per-step clocks (multi-step
    /// turns), and reports nothing when no step measured one.
    #[test]
    fn generation_time_sums_every_step() {
        let message = ChatMessage {
            user: false,
            summary: None,
            steps: vec![
                Step {
                    generation_time: Some(Duration::from_millis(400)),
                    ..Step::default()
                },
                Step {
                    generation_time: Some(Duration::from_millis(600)),
                    ..Step::default()
                },
            ],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };
        assert_eq!(message.generation_time(), Some(Duration::from_secs(1)));
        let bare = ChatMessage {
            user: false,
            summary: None,
            steps: vec![Step::default()],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };
        assert_eq!(bare.generation_time(), None);
    }

    #[test]
    fn nested_user_echo_dedupes_optimistic_message() {
        let mut t = Transcript::new();
        assert!(t.append_user_message("fix it", Vec::new()));
        // pi's echo (nested) must not duplicate the optimistic row.
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "user", "content": "fix it"
            }}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "user", "content": "fix it"
            }}),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].user);
    }

    /// An attached image that pi resized gets a dimension note appended to
    /// the echoed prompt text. The optimistic row carries the typed text only,
    /// so the echo must still dedupe instead of stacking a second bubble.
    #[test]
    fn image_hint_in_user_echo_does_not_duplicate_the_prompt() {
        let mut t = Transcript::new();
        let prompt = "can you check the attached og image";
        assert!(t.append_user_message(prompt, Vec::new()));
        let echo = format!(
            "{prompt}\n\n[Image: original 2400x1260, displayed at 2000x1050. \
             Multiply coordinates by 1.20 to map to original image.]"
        );
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "user", "content": [{"type": "text", "text": echo}]
            }}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "user", "content": [{"type": "text", "text": echo}]
            }}),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1, "the prompt must render once");
        assert_eq!(messages[0].text(), prompt);
    }

    /// Reloading from disk parses the persisted prompt with its image hint;
    /// the note must not render as user copy.
    #[test]
    fn reloaded_user_prompt_strips_image_hint() {
        let mut t = Transcript::new();
        t.load_from(&json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text":
                "see this\n\n[Image converted from image/heic to image/png.]\n[Image: original 800x600, displayed at 400x300. Multiply coordinates by 2.00 to map to original image.]"}]
            },
        ]}));
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text(), "see this");
    }

    /// pi emits a context-only `system` loadout/tool-change update right
    /// before the user echo (its session files show a system entry between
    /// the prompt's turn and the echoed user message). It must not become a
    /// row — and, critically, must not break the echo dedupe: a live event
    /// sequence of system start/end then user start/end shows the prompt
    /// twice when the system message lands in between.
    #[test]
    fn system_loadout_message_does_not_duplicate_the_user_echo() {
        let mut t = Transcript::new();
        assert!(t.append_user_message("hello", Vec::new()));
        let system = json!({"type": "message_start", "message": {
            "role": "system", "content": "",
            "sections": {"preamble": "You are an expert…"}
        }});
        t.apply_event(&Event::MessageStart {
            value: system.clone(),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "system", "content": "",
                "sections": {"preamble": "You are an expert…"}
            }}),
        });
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "user", "content": [{"type": "text", "text": "hello"}]
            }}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "user", "content": [{"type": "text", "text": "hello"}]
            }}),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1, "the prompt must render once");
        assert!(messages[0].user);
        assert_eq!(messages[0].text(), "hello");
    }

    /// The same system entry in a `get_messages`/disk snapshot must be
    /// skipped, not parsed as an empty assistant row between turns.
    #[test]
    fn system_messages_are_skipped_when_reloading() {
        let message = ChatMessage::from_value(&json!({
            "role": "system", "content": "",
            "sections": {"preamble": "You are an expert…"}
        }));
        assert!(message.is_none());

        let mut t = Transcript::new();
        t.load_from(&json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "first"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "answer"}]},
            {"role": "system", "content": "", "sections": {"preamble": "…"}},
            {"role": "user", "content": [{"type": "text", "text": "hello"}]},
        ]}));
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 3, "two user turns plus one assistant row");
        assert!(messages[2].user);
        assert_eq!(messages[2].text(), "hello");
    }

    #[test]
    fn injected_skill_blocks_are_reduced_to_the_command() {
        let body = "<skill name=\"impeccable\" location=\"/x/SKILL.md\">\nBody.\n</skill>\n\npolish the retail page";
        let (name, trailing) = injected_skill(body).expect("recognised");
        assert_eq!(name, "impeccable");
        assert_eq!(trailing, "polish the retail page");
        assert_eq!(
            compact_skill_prompt(&name, &trailing),
            "/skill:impeccable polish the retail page"
        );
        // A bare command keeps just the slash command.
        assert_eq!(compact_skill_prompt("impeccable", ""), "/skill:impeccable");
        // Prose that merely opens with the tag (no close) is left alone.
        assert!(injected_skill("<skill is a tag I am discussing").is_none());
    }

    #[test]
    fn injected_skill_restatement_is_dropped_after_the_optimistic_prompt() {
        let mut t = Transcript::new();
        assert!(t.append_user_message("/skill:impeccable polish the retail page", Vec::new()));
        // pi re-states the loaded skill as a second user message. Even though
        // its compact form differs from the typed prompt, it must not stack a
        // duplicate bubble on top of it.
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": {
                "role": "user",
                "content": [{"type": "text", "text":
                    "<skill name=\"impeccable\" location=\"/x/SKILL.md\">body</skill>\n\npolish the retail page thoroughly"}]
            }}),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].text(),
            "/skill:impeccable polish the retail page"
        );
    }

    /// pi emits both `message_start` and `message_end` for the injected skill
    /// document. The compacted form can differ from the typed prompt by
    /// whitespace (`/skill:a  do x` → `/skill:a do x`), which the text
    /// comparison in `on_message_end` misses — it still must not stack a
    /// second bubble.
    #[test]
    fn injected_skill_end_does_not_duplicate_when_compact_differs() {
        let mut t = Transcript::new();
        assert!(t.append_user_message("/skill:impeccable  polish the retail page", Vec::new()));
        let inner = json!({"role": "user", "content": [{"type": "text", "text":
            "<skill name=\"impeccable\" location=\"/x/SKILL.md\">body</skill>\n\n polish the retail page"}]});
        t.apply_event(&Event::MessageStart {
            value: json!({"type": "message_start", "message": inner.clone()}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": inner.clone()}),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 1, "the skill echo must render once");
        assert_eq!(
            messages[0].text(),
            "/skill:impeccable  polish the retail page"
        );
    }

    /// A `message_end` with no preceding `message_start` (a missed boundary)
    /// must dedupe the skill echo on its own.
    #[test]
    fn injected_skill_end_alone_dedupes_the_optimistic_prompt() {
        let mut t = Transcript::new();
        assert!(t.append_user_message("/skill:impeccable polish the retail page", Vec::new()));
        t.apply_event(&Event::MessageEnd {
            value: json!({"type": "message_end", "message": {
                "role": "user",
                "content": [{"type": "text", "text":
                    "<skill name=\"impeccable\" location=\"/x/SKILL.md\">body</skill>\n\npolish the retail page thoroughly"}]
            }}),
        });
        assert_eq!(t.messages.borrow().len(), 1);
    }

    #[test]
    fn reloaded_injected_skill_renders_as_a_readable_prompt() {
        // A session opened from disk holds only pi's injected block, so it is
        // rebuilt as the slash command plus the user's trailing prompt — the
        // skill body is never shown.
        let message = ChatMessage::from_value(&json!({
            "role": "user",
            "content": [{"type": "text", "text":
                "<skill name=\"impeccable\" location=\"/x/SKILL.md\">SKILL body\n</skill>\n\npolish the retail page"}]
        }))
        .expect("user message");
        assert!(message.user);
        assert_eq!(message.text(), "/skill:impeccable polish the retail page");
    }

    /// Perf harness (P6): a 10k-message transcript is the documented scale
    /// (README/PRODUCT). This guards against accidental O(n²) regressions in
    /// the model layer; the virtualized list itself is count-agnostic.
    /// The bound is deliberately generous so it never flakes on a cold CI box.
    #[test]
    fn ten_thousand_messages_stay_workable() {
        const N: usize = 10_000;
        let start = Instant::now();
        let mut t = Transcript::new();
        for ix in 0..N {
            if ix % 7 == 0 {
                t.append_user_message(&format!("needle {ix}"), Vec::new());
            } else {
                t.append_user_message(&format!("message {ix}"), Vec::new());
            }
        }
        let appended = start.elapsed();
        assert_eq!(t.message_count(), N);

        let start = Instant::now();
        let hits = t.find_messages("needle");
        let search = start.elapsed();
        assert_eq!(hits.len(), N.div_ceil(7));
        assert!(
            appended + search < Duration::from_secs(10),
            "10k-message model work took {:?} (append) + {:?} (search)",
            appended,
            search
        );
        eprintln!("10k messages: append {:?}, search {:?}", appended, search);
    }

    #[test]
    fn find_messages_matches_case_insensitively_in_order() {
        let mut t = Transcript::new();
        t.append_user_message("Fix the login bug", Vec::new());
        t.append_user_message("unrelated", Vec::new());
        t.append_user_message("another LOGIN issue", Vec::new());
        assert_eq!(t.message_count(), 3);
        assert_eq!(t.find_messages("login"), vec![0, 2]);
        assert!(t.find_messages("missing").is_empty());
    }

    #[test]
    fn summary_roles_parse_as_boundary_rows() {
        let compaction = json!({
            "role": "compactionSummary",
            "summary": "## Goal\nShip the fix",
            "tokensBefore": 150_700,
            "timestamp": 1_733_234_400_000i64
        });
        let parsed = ChatMessage::from_value(&compaction).expect("compaction row");
        assert!(parsed.is_summary());
        assert!(!parsed.is_assistant());
        assert!(!parsed.user);
        let row = parsed.summary.as_ref().expect("summary");
        assert!(!row.branch);
        assert_eq!(row.tokens_before, Some(150_700));
        assert_eq!(row.summary, "## Goal\nShip the fix");
        // The summary stays out of answer/navigation/search text paths.
        assert!(parsed.text().is_empty());

        let branch = json!({
            "role": "branchSummary",
            "summary": "Explored approach A",
            "fromId": "abc123",
            "timestamp": 1_733_234_500_000i64
        });
        let parsed = ChatMessage::from_value(&branch).expect("branch row");
        let row = parsed.summary.as_ref().expect("summary");
        assert!(row.branch);
        assert_eq!(row.tokens_before, None);
        assert!(parsed.text().is_empty());
    }

    #[test]
    fn summaries_stay_their_own_rows_on_load() {
        let payload = json!({"messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [{"type": "text", "text": "hi"}]},
            {"role": "compactionSummary", "summary": "earlier work", "tokensBefore": 90_000},
            {"role": "user", "content": "kept prompt"},
            {"role": "assistant", "content": [{"type": "text", "text": "kept answer"}]}
        ]});
        let mut t = Transcript::new();
        t.load_from(&payload);
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 5);
        // The summary is not merged into the assistant before it…
        assert!(messages[1].is_assistant());
        assert!(messages[2].is_summary());
        // …and the settled turn after it is a fresh row, not a summary step.
        assert!(messages[4].is_assistant());
        assert!(messages[4].summary.is_none());
        assert_eq!(messages[4].text(), "kept answer");
    }

    #[test]
    fn assistant_after_summary_starts_a_new_row() {
        let payload = json!({"messages": [
            {"role": "compactionSummary", "summary": "boundary", "tokensBefore": 1_000},
            {"role": "assistant", "content": [{"type": "text", "text": "resumed"}]}
        ]});
        let mut t = Transcript::new();
        t.load_from(&payload);
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 2);
        assert!(messages[1].is_assistant());
        assert_eq!(messages[1].text(), "resumed");
    }

    #[test]
    fn live_thinking_duration_rides_the_step() {
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ThinkingDelta {
                delta: "reason".into(),
            }),
        });
        // The reasoning clock starts with the first delta (a running value).
        {
            let messages = t.messages.borrow();
            assert!(messages[0].steps[0].thinking_duration.is_some());
        }
        std::thread::sleep(Duration::from_millis(5));
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "reason"}],
                "timestamp": 1_700_000_000_000i64
            }),
        });
        // The settled snapshot has no timing, so the measured value survives.
        let messages = t.messages.borrow();
        assert!(messages[0].steps[0].thinking_duration.is_some());
        assert_eq!(messages[0].steps[0].timestamp, Some(1_700_000_000_000));
    }

    #[test]
    fn live_assistant_steps_merge_into_one_row() {
        // One turn, many steps: each step is a separate assistant message
        // from pi, but the transcript renders ONE row with ONE fold.
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "step one".into(),
            }),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallStart {
                value: json!({"toolCallId": "c1", "toolName": "bash", "arguments": {"command": "ls"}}),
            }),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "step one"},
                    {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "ls"}}
                ]
            }),
        });
        t.apply_event(&Event::ToolExecutionStart {
            value: json!({"toolCallId": "c1", "toolName": "bash", "args": {"command": "ls"}}),
        });
        t.apply_event(&Event::ToolExecutionEnd {
            value: json!({"toolCallId": "c1", "result": "out", "isError": false}),
        });
        // Step 2 continues the same row.
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "step two".into(),
            }),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"role": "assistant", "content": "step two"}),
        });
        {
            let messages = t.messages.borrow();
            assert_eq!(messages.len(), 1, "steps must share one row");
            assert_eq!(messages[0].text(), "step one\n\nstep two");
            assert_eq!(messages[0].tools().count(), 1);
            assert_eq!(
                messages[0]
                    .tools()
                    .next()
                    .unwrap()
                    .output
                    .as_ref()
                    .and_then(|v: &serde_json::Value| v.as_str()),
                Some("out")
            );
            assert!(messages[0].elapsed.is_some(), "fold carries the run clock");
        }
        // A new prompt starts a fresh row.
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "user", "content": "next"}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"role": "user", "content": "next"}),
        });
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "second".into(),
            }),
        });
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].text(), "second");
        assert!(!messages[2].has_hidden_work());
    }

    #[test]
    fn seeded_message_start_does_not_double_the_first_think_chunk() {
        // Some providers buffer the first chunk and put it in `message_start`,
        // then re-send that same chunk as the first `thinking_delta`. The
        // transcript must drop the seed rather than render "TheThe".
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "The", "thinkingSignature": "reasoning"}],
            }),
        });
        for delta in ["The", " user", " asks"] {
            t.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::ThinkingDelta {
                    delta: delta.into(),
                }),
            });
        }
        assert_eq!(t.messages.borrow()[0].steps[0].thinking, "The user asks");

        // The settled message replaces the step with the authoritative text.
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "The user asks", "thinkingSignature": "reasoning"}],
            }),
        });
        assert_eq!(t.messages.borrow()[0].steps[0].thinking, "The user asks");
    }

    #[test]
    fn seeded_message_start_does_not_double_the_first_text_chunk() {
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": [{"type": "text", "text": "A"}]}),
        });
        for delta in ["A", " code"] {
            t.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::TextDelta {
                    delta: delta.into(),
                }),
            });
        }
        assert_eq!(t.messages.borrow()[0].steps[0].text, "A code");
    }

    #[test]
    fn repeated_identical_deltas_are_not_collapsed() {
        // A seeded start must not make the seed logic eat genuinely repeated
        // chunks ("aa" from two "a" deltas).
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        for _ in 0..2 {
            t.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::TextDelta { delta: "a".into() }),
            });
        }
        assert_eq!(t.messages.borrow()[0].steps[0].text, "aa");
    }

    #[test]
    fn snapshot_merges_consecutive_assistants() {
        let payload = json!({
            "messages": [
                {"role": "user", "content": "do it"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm"},
                    {"type": "toolCall", "id": "c1", "name": "read", "arguments": {"path": "a.rs"}},
                    {"type": "text", "text": "first part"}
                ]},
                {"role": "toolResult", "toolCallId": "c1", "toolName": "read",
                 "content": [{"type": "text", "text": "contents"}]},
                {"role": "assistant", "content": "second part"}
            ]
        });
        let mut t = Transcript::new();
        t.load_from(&payload);
        let messages = t.messages.borrow();
        assert_eq!(messages.len(), 2, "user + merged assistant turn");
        assert_eq!(messages[1].text(), "first part\n\nsecond part");
        assert_eq!(messages[1].steps[0].thinking, "hmm");
        assert_eq!(messages[1].tools().count(), 1);
        assert_eq!(
            messages[1]
                .tools()
                .next()
                .unwrap()
                .output
                .as_ref()
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("contents")
        );
        assert!(messages[1].has_hidden_work());
    }

    /// A transcript painted from a session file on disk (the cold-session
    /// preview) must parse to the same rows as pi's `get_messages` snapshot:
    /// user/assistant turns merge the same way and tool results attach by id.
    #[test]
    fn disk_session_payload_rebuilds_a_transcript() {
        let dir = std::env::temp_dir().join(format!("orbit-disk-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session","id":"s","cwd":"/tmp/ws"}
{"type":"model_change","provider":"ollama","modelId":"m"}
{"type":"message","message":{"role":"user","content":"do it"}}
{"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"a.rs"}},{"type":"text","text":"first part"}]}}
{"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"contents"}]}}
{"type":"message","message":{"role":"assistant","content":"second part"}}
"#,
        )
        .unwrap();

        let payload = crate::sessions::read_messages_payload(&path).expect("payload");
        let mut transcript = Transcript::new();
        transcript.load_from(&payload);
        let messages = transcript.messages.borrow();
        assert_eq!(messages.len(), 2, "user + merged assistant turn");
        assert_eq!(messages[1].text(), "first part\n\nsecond part");
        assert_eq!(messages[1].steps[0].thinking, "hmm");
        assert_eq!(messages[1].tools().count(), 1);
        assert_eq!(
            messages[1]
                .tools()
                .next()
                .unwrap()
                .output
                .as_ref()
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("contents")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_tool_result_message_attaches_without_a_row() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "working".into(),
            }),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "call_9",
                "toolName": "bash",
                "args": {"command": "ls"}
            }),
        });
        // pi settles a tool call with a `toolResult` message — it must not
        // replace the streaming assistant message with tool output.
        transcript.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "toolResult",
                "toolCallId": "call_9",
                "toolName": "bash",
                "isError": false,
                "content": [{"type": "text", "text": "file-a\nfile-b"}]
            }),
        });
        assert!(transcript.is_streaming());
        {
            let messages = transcript.messages.borrow();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text(), "working");
            assert_eq!(messages[0].tools().count(), 1);
        }
        assert!(transcript.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "working"},
                    {
                        "type": "toolCall",
                        "id": "call_9",
                        "name": "bash",
                        "arguments": {"command": "ls"}
                    }
                ]
            }),
        }));
        let messages = transcript.messages.borrow();
        assert_eq!(
            messages[0]
                .tools()
                .next()
                .unwrap()
                .output
                .as_ref()
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("file-a\nfile-b")
        );
    }

    #[test]
    fn hidden_work_is_thinking_or_tools() {
        let empty = ChatMessage::empty_assistant();
        assert!(!empty.has_hidden_work());
        let mut thinking = ChatMessage::empty_assistant();
        thinking.steps[0].thinking = "hmm".into();
        assert!(thinking.has_hidden_work());
    }

    #[test]
    fn edit_counts_old_and_new_text() {
        let args = json!({
            "path": "src/main.rs",
            "edits": [
                {"oldText": "a\nb", "newText": "a\nb\nc"}
            ]
        });
        assert_eq!(tool_line_stats("edit", &args), (3, 2));
    }

    #[test]
    fn write_counts_content_as_added() {
        let args = json!({ "path": "a.txt", "content": "one\ntwo\nthree" });
        assert_eq!(tool_line_stats("write", &args), (3, 0));
    }

    #[test]
    fn write_with_empty_content_counts_zero() {
        let args = json!({ "path": "a.txt", "content": "" });
        assert_eq!(tool_line_stats("write", &args), (0, 0));
    }

    #[test]
    fn other_tools_have_no_diff() {
        let args = json!({ "path": "a.txt" });
        assert_eq!(tool_line_stats("read", &args), (0, 0));
    }

    #[test]
    fn tool_call_from_value_keeps_path_and_stats() {
        let args = json!({
            "path": "lib.rs",
            "edits": [{"oldText": "x", "newText": "x\ny"}]
        });
        let tool = ToolCall::from_value("edit", Some(&args));
        assert_eq!(tool.path.as_deref(), Some("lib.rs"));
        assert_eq!((tool.added, tool.removed), (2, 1));
    }

    #[test]
    fn assistant_usage_parses_and_aggregates_across_steps() {
        let first = json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}],
            "usage": {
                "input": 100, "output": 50, "cacheRead": 20, "cacheWrite": 5,
                "cost": {"input": 0.0003, "output": 0.00075, "cacheRead": 0,
                    "cacheWrite": 0, "total": 0.00105}
            }
        });
        let mut message = ChatMessage::from_value(&first).expect("message");
        let usage = message.usage().expect("usage");
        assert_eq!(usage.input, 100);
        assert_eq!(usage.total, 175);
        assert_eq!(usage.cost, Some(0.00105));

        // A tool-loop turn merges another LLM call into the same row; the
        // turn-level usage is the sum of its steps.
        let second = json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "more"}],
            "usage": {"input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 12}
        });
        merge_step(
            &mut message,
            ChatMessage::from_value(&second).expect("message"),
        );
        let total = message.usage().expect("usage");
        assert_eq!(total.input, 110);
        assert_eq!(total.output, 52);
        assert_eq!(total.total, 187);
        assert_eq!(total.cost, Some(0.00105));
    }

    #[test]
    fn usage_is_absent_without_provider_report() {
        let user = json!({"role": "user", "content": "hi"});
        assert!(ChatMessage::from_value(&user)
            .expect("message")
            .usage()
            .is_none());

        let no_usage = json!({"role": "assistant", "content": [{"type": "text", "text": "hi"}]});
        assert!(ChatMessage::from_value(&no_usage)
            .expect("message")
            .usage()
            .is_none());
    }

    #[test]
    fn diff_from_message_sums_edit_and_write() {
        let message = json!({
            "role": "assistant",
            "content": [
                {
                    "type": "toolCall",
                    "name": "edit",
                    "arguments": {
                        "path": "a.rs",
                        "edits": [{"oldText": "old", "newText": "new\nline"}]
                    }
                },
                {
                    "type": "toolCall",
                    "name": "write",
                    "arguments": { "path": "b.rs", "content": "a\nb" }
                },
                {
                    "type": "toolCall",
                    "name": "read",
                    "arguments": { "path": "c.rs" }
                }
            ]
        });
        assert_eq!(diff_from_message(&message), (4, 1));
    }

    #[test]
    fn diff_from_message_unwraps_nested_message() {
        let value = json!({
            "message": {
                "content": [{
                    "type": "tool_call",
                    "name": "write",
                    "arguments": { "content": "hi" }
                }]
            }
        });
        assert_eq!(diff_from_message(&value), (1, 0));
    }

    #[test]
    fn changed_files_merges_same_path() {
        let tools = vec![
            ToolCall {
                name: "edit".into(),
                summary: String::new(),
                path: Some("a.rs".into()),
                added: 2,
                removed: 1,
                ..ToolCall::from_value("edit", None)
            },
            ToolCall {
                name: "edit".into(),
                summary: String::new(),
                path: Some("a.rs".into()),
                added: 3,
                removed: 0,
                ..ToolCall::from_value("edit", None)
            },
            ToolCall {
                name: "write".into(),
                summary: String::new(),
                path: Some("b.rs".into()),
                added: 4,
                removed: 0,
                ..ToolCall::from_value("write", None)
            },
            ToolCall {
                name: "read".into(),
                summary: String::new(),
                path: Some("c.rs".into()),
                added: 0,
                removed: 0,
                ..ToolCall::from_value("read", None)
            },
        ];
        let message = ChatMessage {
            user: false,
            summary: None,
            steps: vec![Step {
                tools,
                ..Step::default()
            }],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };
        assert_eq!(
            changed_files(&message),
            vec![("a.rs".into(), 5, 1), ("b.rs".into(), 4, 0)]
        );
    }

    #[test]
    fn settled_run_pins_changed_files_summary() {
        let edit_args = json!({
            "path": "a.rs",
            "edits": [{ "oldText": "x", "newText": "y\nz" }]
        });
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "user", "content": "build it"}),
        });
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallStart {
                value: json!({
                    "toolCallId": "c1",
                    "toolName": "edit",
                    "arguments": edit_args
                }),
            }),
        });
        // Mid-run: no summary row yet.
        assert_eq!(t.scroller.item_count(), 2);
        assert!(!t.tail_summary.get());
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [
                    {"type": "toolCall", "id": "c1", "name": "edit", "arguments": edit_args}
                ]
            }),
        });
        assert_eq!(t.scroller.item_count(), 2);
        // Settle: the whole-task summary pins itself after the last message.
        t.apply_event(&Event::AgentSettled);
        assert!(t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 3);
        assert_eq!(t.changed_files_summary(), vec![("a.rs".into(), 2, 1)]);
        // A new turn removes the summary slot again (prompt takes its place).
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "user", "content": "more"}),
        });
        assert!(!t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 3);
        // Settling again re-pins the summary — it aggregates the whole
        // task, which still includes the first turn's edit.
        t.apply_event(&Event::AgentSettled);
        assert!(t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 4);
    }

    /// Screenshot regression: `agent_end` fires between loop iterations
    /// while the run continues (steering/follow-up queued) — the changed-files
    /// card must stay hidden until the run truly settles.
    #[test]
    fn agent_end_mid_run_does_not_pin_summary() {
        let edit_args = json!({
            "path": "a.rs",
            "edits": [{ "oldText": "x", "newText": "y\nz" }]
        });
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "user", "content": "build it"}),
        });
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [
                    {"type": "toolCall", "id": "c1", "name": "edit", "arguments": edit_args}
                ]
            }),
        });
        // First loop iteration ends, but the run keeps working (steering
        // queued) — edits exist yet no summary card may appear.
        t.apply_event(&Event::AgentEnd { will_retry: false });
        assert!(!t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 2);
        // Run continues: a follow-up assistant message streams in.
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": "continuing"}),
        });
        assert!(t.is_streaming());
        // Only the real settle pins the summary.
        t.apply_event(&Event::AgentSettled);
        assert!(t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 3);
        assert_eq!(t.changed_files_summary(), vec![("a.rs".into(), 2, 1)]);
    }

    #[test]
    fn loaded_session_shows_changed_files_summary() {
        let payload = json!({
            "messages": [
                {"role": "user", "content": "do it"},
                {"role": "assistant", "content": [
                    {"type": "toolCall", "id": "c1", "name": "write",
                     "arguments": {"path": "n.rs", "content": "a\nb\nc"}}
                ]}
            ]
        });
        let mut t = Transcript::new();
        t.load_from(&payload);
        assert!(t.tail_summary.get());
        assert_eq!(t.scroller.item_count(), 3, "2 messages + summary row");
        assert_eq!(t.changed_files_summary(), vec![("n.rs".into(), 3, 0)]);
    }

    #[test]
    fn rail_hint_state_dismisses_once() {
        let dismissed = Cell::new(false);
        let shown_at = Cell::new(None);
        assert!(dismiss_rail_hint_state(&dismissed, &shown_at));
        assert!(dismissed.get());
        assert!(shown_at.get().is_none());
        // Second dismissal is a no-op (already persisted).
        assert!(!dismiss_rail_hint_state(&dismissed, &shown_at));
    }

    /// `hints.json` is one JSON object shared by every affordance hint:
    /// persisting one must preserve the others, and a missing file reads as
    /// "not seen".
    #[test]
    fn hints_round_trip_without_clobbering_siblings() {
        let dir = std::env::temp_dir().join(format!(
            "orbit-hints-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("hints.json");
        assert!(!hint_seen_at(&path, RAIL_HINT_KEY));
        persist_hint_at(&path, RAIL_HINT_KEY);
        assert!(hint_seen_at(&path, RAIL_HINT_KEY));
        persist_hint_at(&path, "another_hint");
        assert!(hint_seen_at(&path, "another_hint"));
        assert!(
            hint_seen_at(&path, RAIL_HINT_KEY),
            "persisting a hint must not wipe its siblings"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jump_turn_walks_and_refuses_edges() {
        let payload = json!({
            "messages": [
                {"role": "user", "content": "one"},
                {"role": "assistant", "content": "a"},
                {"role": "user", "content": "two"}
            ]
        });
        let mut t = Transcript::new();
        t.load_from(&payload);
        // The transcript opens at the live edge, so the reader is on the
        // newest turn: previous jumps back to the first user turn.
        assert!(t.jump_turn(-1));
        assert_eq!(t.scroller.first_visible_index(), 0);
        // Already on the first user turn: previous has nowhere to go.
        assert!(!t.jump_turn(-1));
        // Next walks forward to the second user turn.
        assert!(t.jump_turn(1));
        assert_eq!(t.scroller.first_visible_index(), 2);
        // Past the last user turn: next has nowhere to go.
        assert!(!t.jump_turn(1));
        // Empty transcripts have nothing to jump to.
        let t = Transcript::new();
        assert!(!t.jump_turn(1));
        assert!(!t.jump_turn(-1));
    }

    #[test]
    fn chat_message_parses_tool_stats() {
        let value = json!({
            "role": "assistant",
            "content": [{
                "type": "toolCall",
                "name": "write",
                "arguments": { "path": "n.rs", "content": "a\nb\nc" }
            }]
        });
        let message = ChatMessage::from_value(&value).unwrap();
        assert_eq!(message.tools().count(), 1);
        assert_eq!(message.tools().next().unwrap().added, 3);
        assert_eq!(
            message.tools().next().unwrap().path.as_deref(),
            Some("n.rs")
        );
    }

    #[test]
    fn stream_updates_keep_one_list_row() {
        let mut transcript = Transcript::new();
        assert!(transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        }));
        assert_eq!(transcript.scroller.item_count(), 1);
        for _ in 0..8 {
            assert!(transcript.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::TextDelta { delta: "x".into() }),
            }));
        }
        assert_eq!(transcript.scroller.item_count(), 1);
        assert_eq!(transcript.messages.borrow()[0].text(), "xxxxxxxx");
        assert!(transcript.apply_event(&Event::MessageEnd {
            value: json!({"role": "assistant", "content": "xxxxxxxx"}),
        }));
        assert_eq!(transcript.scroller.item_count(), 1);
    }

    #[test]
    fn mcp_tool_execution_keeps_the_wire_name_and_attaches_the_result() {
        // Pi names MCP tools `mcp__<server>__<tool>`; the transcript must
        // preserve the exact wire name (the card parses it) and still attach
        // the result by call id like any other tool.
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "m1",
                "toolName": "mcp__github__search_repositories",
                "args": {"query": "rust gpui"}
            }),
        });
        transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({
                "toolCallId": "m1",
                "toolName": "mcp__github__search_repositories",
                "result": {"content": [{"type": "text", "text": "3 results"}]},
                "isError": false
            }),
        });
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert_eq!(tool.name, "mcp__github__search_repositories");
        assert!(!tool.failed);
        assert!(tool.output.is_some());
        assert_eq!(
            crate::transcript_view::mcp_tool_parts(&tool.name),
            Some(("github", "search_repositories"))
        );
        drop(messages);

        // A failed MCP call keeps the same wire name and marks the row.
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "m2",
                "toolName": "mcp__sentry__create_issue",
                "args": {"title": "boom"}
            }),
        });
        transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({
                "toolCallId": "m2",
                "toolName": "mcp__sentry__create_issue",
                "result": {"content": [{"type": "text", "text": "401 Unauthorized"}]},
                "isError": true
            }),
        });
        let messages = transcript.messages.borrow();
        let failed = messages[0]
            .tools()
            .find(|tool| tool.name == "mcp__sentry__create_issue")
            .unwrap();
        assert!(failed.failed);
        assert_eq!(
            crate::transcript_view::mcp_tool_parts(&failed.name),
            Some(("sentry", "create_issue"))
        );
    }

    #[test]
    fn tool_execution_events_attach_results_by_id() {
        let mut transcript = Transcript::new();
        assert!(transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        }));
        assert!(transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "t1",
                "toolName": "read",
                "args": {"path": "lib.rs"}
            })
        }));
        assert!(transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({
                "toolCallId": "t1",
                "result": {"content": "contents"},
                "isError": false
            })
        }));
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert_eq!(tool.name, "read");
        assert_eq!(tool.path.as_deref(), Some("lib.rs"));
        assert_eq!(
            tool.output
                .as_ref()
                .and_then(|v| v.get("content"))
                .and_then(Value::as_str),
            Some("contents")
        );
        assert!(!tool.failed);
    }

    #[test]
    fn tool_execution_update_streams_accumulated_output() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "t1",
                "toolName": "bash",
                "args": {"command": "cargo test"}
            }),
        });
        // The accumulated partialResult replaces the display each update.
        transcript.apply_event(&Event::ToolExecutionUpdate {
            value: json!({
                "toolCallId": "t1",
                "toolName": "bash",
                "partialResult": {
                    "content": [{"type": "text", "text": "running 1 test\n"}],
                    "details": {"truncation": null}
                }
            }),
        });
        transcript.apply_event(&Event::ToolExecutionUpdate {
            value: json!({
                "toolCallId": "t1",
                "toolName": "bash",
                "partialResult": {
                    "content": [{"type": "text", "text": "running 1 test\nrunning 2 tests\n"}]
                }
            }),
        });
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        // The envelope is normalized away — the output is the joined text.
        assert_eq!(
            tool.output.as_ref().and_then(Value::as_str),
            Some("running 1 test\nrunning 2 tests\n")
        );
    }

    #[test]
    fn tool_execution_end_normalizes_the_result_envelope() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "t1",
                "toolName": "bash",
                "args": {"command": "ls"}
            }),
        });
        transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({
                "toolCallId": "t1",
                "result": {
                    "content": [{"type": "text", "text": "a.rs\nb.rs"}],
                    "details": {}
                },
                "isError": false
            }),
        });
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert_eq!(
            tool.output.as_ref().and_then(Value::as_str),
            Some("a.rs\nb.rs")
        );
    }

    #[test]
    fn tool_facts_read_pi_details() {
        // read/bash attach a `truncation` object only when the output was cut.
        let facts = ToolFacts::from_result(&json!({
            "content": [{"type": "text", "text": "x"}],
            "details": {"truncation": {"truncated": true, "outputLines": 200, "totalLines": 1303}}
        }));
        assert!(facts.truncated);
        assert_eq!(facts.output_lines, Some(200));
        assert_eq!(facts.total_lines, Some(1303));
        // grep / ls cap the result without a truncation object.
        assert!(ToolFacts::from_result(&json!({"details": {"matchLimitReached": true}})).truncated);
        assert!(ToolFacts::from_result(&json!({"details": {"entryLimitReached": true}})).truncated);
        // No details is no facts — never invented.
        assert_eq!(
            ToolFacts::from_result(&json!("plain")),
            ToolFacts::default()
        );
        assert_eq!(
            ToolFacts::from_result(&json!({"content": [], "details": {}})),
            ToolFacts::default()
        );
    }

    #[test]
    fn live_tool_result_captures_truncation_facts() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({"toolCallId": "r1", "toolName": "read", "args": {"path": "big.rs"}}),
        });
        transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({
                "toolCallId": "r1",
                "result": {
                    "content": [{"type": "text", "text": "…"}],
                    "details": {"truncation": {"truncated": true, "outputLines": 200, "totalLines": 5000}}
                },
                "isError": false
            }),
        });
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert!(tool.facts.truncated);
        assert_eq!(tool.facts.output_lines, Some(200));
        assert_eq!(tool.facts.total_lines, Some(5000));
    }

    #[test]
    fn loaded_tool_results_keep_truncation_facts() {
        let mut transcript = Transcript::new();
        transcript.load_from(&json!({"messages": [
            {"role": "assistant", "content": [
                {"type": "toolCall", "id": "g1", "name": "grep", "arguments": {"pattern": "foo"}}
            ]},
            {"role": "toolResult", "toolCallId": "g1", "toolName": "grep",
             "content": [{"type": "text", "text": "many lines"}],
             "details": {"matchLimitReached": true}, "isError": false}
        ]}));
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert!(tool.facts.truncated);
        assert_eq!(tool.facts.output_lines, None);
    }

    /// Ground-truth capture from pi 0.85.1 (`pi --mode rpc`, docs:
    /// pi.dev/docs/latest/rpc): `toolcall_start` carries the call id as `id`
    /// (not `toolCallId`) and no arguments; arguments stream as raw JSON text
    /// in `toolcall_delta`; `toolcall_end` nests the completed call under
    /// `toolCall`. One call must produce ONE correctly-named row whose
    /// arguments and result land on it — never a duplicate or a bare "tool".
    #[test]
    fn real_toolcall_wire_shape_produces_one_named_tool_row() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallStart {
                value: json!({"type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "bash"}),
            }),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallDelta {
                value: json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "{\"command\":\"echo hello-world\"}"}),
            }),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallEnd {
                value: json!({"type": "toolcall_end", "contentIndex": 0, "toolCall": {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "echo hello-world"}}}),
            }),
        });
        transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({"type": "tool_execution_start", "toolCallId": "call_1", "toolName": "bash", "args": {"command": "echo hello-world"}}),
        });
        transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({"type": "tool_execution_end", "toolCallId": "call_1", "toolName": "bash", "result": {"content": [{"type": "text", "text": "hello-world\n"}]}, "isError": false}),
        });
        let messages = transcript.messages.borrow();
        let tools: Vec<_> = messages[0].tools().collect();
        assert_eq!(tools.len(), 1, "one call must not duplicate the tool row");
        let tool = tools[0];
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.id.as_deref(), Some("call_1"));
        assert_eq!(
            tool.args
                .as_ref()
                .and_then(|args| args.get("command"))
                .and_then(Value::as_str),
            Some("echo hello-world")
        );
        assert_eq!(
            tool.output.as_ref().and_then(Value::as_str),
            Some("hello-world\n")
        );
        assert!(!tool.failed);
    }

    /// Two parallel calls in one step: each `toolcall_end` must replace its
    /// own placeholder (matched by id), never clobber the sibling's row.
    #[test]
    fn parallel_toolcalls_keep_their_own_rows() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        for (ix, id, path) in [(0u8, "call_a", "src/a.rs"), (1u8, "call_b", "src/b.rs")] {
            transcript.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::ToolcallStart {
                    value: json!({"type": "toolcall_start", "contentIndex": ix, "id": id, "toolName": "read"}),
                }),
            });
            transcript.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::ToolcallEnd {
                    value: json!({"type": "toolcall_end", "contentIndex": ix, "toolCall": {"type": "toolCall", "id": id, "name": "read", "arguments": {"path": path}}}),
                }),
            });
        }
        let messages = transcript.messages.borrow();
        let tools: Vec<_> = messages[0].tools().collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(
            tools[0]
                .args
                .as_ref()
                .and_then(|a| a.get("path"))
                .and_then(Value::as_str),
            Some("src/a.rs")
        );
        assert_eq!(
            tools[1]
                .args
                .as_ref()
                .and_then(|a| a.get("path"))
                .and_then(Value::as_str),
            Some("src/b.rs")
        );
    }

    /// Arguments that stream in several chunks apply once the accumulated
    /// buffer parses — the row shows the command before the call ends.
    #[test]
    fn toolcall_delta_applies_arguments_when_the_buffer_parses() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::ToolcallStart {
                value: json!({"type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "bash"}),
            }),
        });
        for chunk in ["{\"comm", "and\":\"cargo", " test\"}"] {
            transcript.apply_event(&Event::MessageUpdate {
                usage: None,
                assistant: Some(AssistantMessageEvent::ToolcallDelta {
                    value: json!({"type": "toolcall_delta", "contentIndex": 0, "delta": chunk}),
                }),
            });
        }
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert_eq!(
            tool.args
                .as_ref()
                .and_then(|args| args.get("command"))
                .and_then(Value::as_str),
            Some("cargo test")
        );
    }

    #[test]
    fn structured_results_keep_their_json_form() {
        // A tool whose result has no text blocks (structured data) keeps the
        // payload so the detail card highlights it as JSON.
        let value = normalize_tool_result(&json!({"rows": [1, 2, 3]}));
        assert_eq!(value, json!({"rows": [1, 2, 3]}));
        let text = normalize_tool_result(&json!("plain"));
        assert_eq!(text, json!("plain"));
    }

    #[test]
    fn empty_result_envelope_never_leaks_protocol_json() {
        // pi streams `partialResult: {"content": []}` before the first chunk
        // (real 0.85.1 capture). Normalizing must yield empty output — the
        // detail card hides empty sections — never render the raw envelope.
        let value = normalize_tool_result(&json!({"content": [], "details": {}}));
        assert_eq!(value, json!(""));
        // An image-only envelope (no text blocks) reads the same way.
        let value = normalize_tool_result(
            &json!({"content": [{"type": "image", "data": "…"}], "details": {}}),
        );
        assert_eq!(value, json!(""));
        // Structured payloads without pi's `content` envelope are untouched.
        let value = normalize_tool_result(&json!({"rows": []}));
        assert_eq!(value, json!({"rows": []}));
    }

    #[test]
    fn turn_summary_prefers_the_answer_and_skips_aborted() {
        let mut t = Transcript::new();
        // No assistant turn yet: an empty summary, not a stale one.
        assert_eq!(t.latest_turn_summary(), Some(TurnSummary::default()));

        t.apply_event(&Event::MessageStart {
            value: json!({"role": "user", "content": "fix it"}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({"role": "user", "content": "fix it"}),
        });
        // A user turn with no answer yet must not report the previous turn.
        assert_eq!(t.latest_turn_summary(), Some(TurnSummary::default()));

        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "done".into(),
            }),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "stopReason": "stop"
            }),
        });
        let summary = t.latest_turn_summary().expect("summary");
        assert_eq!(summary.body, "done");
        assert!(!summary.failed && !summary.aborted);

        // A user aborted the turn: nothing to announce.
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "half"}],
                "stopReason": "aborted"
            }),
        });
        assert!(t.latest_turn_summary().expect("summary").aborted);
    }

    #[test]
    fn turn_summary_reports_the_agent_error() {
        let mut t = Transcript::new();
        t.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        t.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [],
                "stopReason": "error",
                "errorMessage": "rate limited"
            }),
        });
        let summary = t.latest_turn_summary().expect("summary");
        assert_eq!(summary.body, "rate limited");
        assert!(summary.failed);
        assert!(!summary.aborted);
    }

    #[test]
    fn aborted_stop_reason_marks_the_turn() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::MessageUpdate {
            usage: None,
            assistant: Some(AssistantMessageEvent::TextDelta {
                delta: "partial answer".into(),
            }),
        });
        transcript.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "partial answer"}],
                "stopReason": "aborted"
            }),
        });
        let messages = transcript.messages.borrow();
        assert!(messages[0].aborted);
        assert_eq!(messages[0].text(), "partial answer");
        assert!(messages[0].error.is_none());
        // A completed turn is not marked.
        let settled = ChatMessage::from_value(&json!({
            "role": "assistant", "content": "done", "stopReason": "stop"
        }))
        .unwrap();
        assert!(!settled.aborted);
    }

    #[test]
    fn message_end_carries_live_results_into_final_message() {
        let mut transcript = Transcript::new();
        assert!(transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        }));
        assert!(transcript.apply_event(&Event::ToolExecutionStart {
            value: json!({
                "toolCallId": "t1",
                "toolName": "bash",
                "args": {"command": "ls"}
            })
        }));
        assert!(transcript.apply_event(&Event::ToolExecutionEnd {
            value: json!({"toolCallId": "t1", "result": "done", "isError": true})
        }));
        assert!(transcript.apply_event(&Event::MessageEnd {
            value: json!({
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "t1",
                    "name": "bash",
                    "arguments": {"command": "ls"}
                }]
            })
        }));
        let messages = transcript.messages.borrow();
        let tool = messages[0].tools().next().unwrap();
        assert_eq!(tool.output.as_ref(), Some(&Value::String("done".into())));
        assert!(tool.failed);
        assert!(messages[0].finished_at.is_some());
    }

    #[test]
    fn snapshot_timestamps_parse_rfc3339_and_epochs() {
        let message = json!({
            "role": "assistant",
            "content": "hi",
            "timestamp": "2026-08-01T10:30:00Z"
        });
        let parsed = ChatMessage::from_value(&message).unwrap();
        assert!(parsed.finished_at.is_some());

        let seconds = ChatMessage::from_value(&json!({
            "role": "assistant", "content": "hi", "timestamp": 1_800_000_000u64
        }))
        .unwrap();
        let millis = ChatMessage::from_value(&json!({
            "role": "assistant", "content": "hi", "timestamp": 1_800_000_000_000u64
        }))
        .unwrap();
        assert_eq!(seconds.finished_at, millis.finished_at);
    }

    #[test]
    fn live_assistant_end_stamps_wall_time() {
        let mut transcript = Transcript::new();
        transcript.apply_event(&Event::MessageStart {
            value: json!({"role": "assistant", "content": ""}),
        });
        transcript.apply_event(&Event::MessageEnd {
            value: json!({"role": "assistant", "content": "answer"}),
        });
        assert!(transcript.messages.borrow()[0].finished_at.is_some());
    }

    #[test]
    fn resync_drops_extra_list_slots() {
        let transcript = Transcript::new();
        transcript.scroller.splice(0..0, 5);
        *transcript.messages.borrow_mut() = vec![ChatMessage::empty_assistant()];
        transcript.resync_list();
        assert_eq!(transcript.scroller.item_count(), 1);
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    #[ignore] // manual: cargo test -p orbit-pi debug_load -- --ignored --nocapture
    fn debug_load() {
        let raw = std::fs::read_to_string("/tmp/get_messages_payload.json").unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut transcript = Transcript::new();
        transcript.load_from(&value["data"]);
        let messages = transcript.messages.borrow();
        let mut user_turns = 0;
        let mut tool_calls = 0;
        let mut text_chars = 0usize;
        for m in messages.iter() {
            if m.user {
                user_turns += 1;
            }
            for step in &m.steps {
                tool_calls += step.tools.len();
                text_chars += step.text.chars().count();
            }
        }
        println!(
            "raw=197 parsed turns={} user_turns={} tool_calls={} text_chars={}",
            messages.len(),
            user_turns,
            tool_calls,
            text_chars
        );
        for (ix, m) in messages.iter().enumerate().take(6) {
            let text: String = m.text().chars().take(50).collect();
            println!(
                "  [{ix}] user={} '{}' tools={}",
                m.user,
                text.replace('\n', " "),
                m.steps.iter().map(|s| s.tools.len()).sum::<usize>()
            );
        }
        assert!(
            messages.len() > 1,
            "load_from collapsed the trail to one message!"
        );
    }
}
