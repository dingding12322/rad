//! `ask_user_question` — the structured questionnaire tool.
//!
//! The tool is an extension (`@juicesharp/rpiv-ask-user-question`). A terminal
//! renders it as a tabbed overlay; in RPC mode (Orbit) it cannot, so it walks
//! the questionnaire through pi's `select` / `input` dialog primitives
//! instead. Orbit answers those on an inline panel above the composer — no
//! scrim modal — and renders the finished tool call as a question card in the
//! transcript.
//!
//! This module is pure data: parsing the tool arguments (`questions`) and
//! reading the answer envelope back. The panel state lives in
//! [`crate::app::ask`]; rendering lives in `app/view.rs` and
//! `transcript_view.rs`.

use gpui::Entity;
use serde_json::Value;

use crate::composer::ComposerInput;

/// One author-defined option of a question.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AskOption {
    pub label: String,
    pub description: String,
    /// Optional markdown preview (mockups, code, diagrams) shown for the
    /// focused option.
    pub preview: Option<String>,
}

/// One question in an `ask_user_question` call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AskQuestion {
    /// Short chip/tag shown next to the question (≤16 chars).
    pub header: String,
    /// The full question text.
    pub question: String,
    /// Whether more than one option may be selected.
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

/// A locally-buffered answer to one questionnaire question.
///
/// Nothing is sent to the extension until the whole questionnaire is
/// committed. Keeping every answer local is what lets the user step back to
/// an earlier question and change it; replay then hands the extension the
/// same `select` / `input` responses it would have seen one at a time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AskAnswer {
    /// No choice recorded yet.
    #[default]
    Unanswered,
    /// Single-select: index into the question's options.
    Option(usize),
    /// Single-select "Type something." (or a multi custom answer): free text.
    Custom(String),
    /// Multi-select: per-option toggles plus an optional custom answer. An
    /// empty selection is a deliberate commit, exactly as the extension's RPC
    /// fallback treats a blank input.
    Multi { checked: Vec<bool>, custom: String },
}

/// The live questionnaire panel rendered above the composer. It answers the
/// extension's `select` / `input` requests in place of the scrim modal.
///
/// The panel is whole-questionnaire: it renders one question at a time with
/// back / next, buffering every answer locally so the user can revisit and
/// change earlier choices. It only talks to the extension once, when the last
/// answer is committed — see `crate::app::ask` for the replay.
pub struct AskPrompt {
    /// RPC id of the questionnaire's first (currently blocking) request.
    pub id: String,
    /// Every question in the running `ask_user_question` call.
    pub questions: Vec<AskQuestion>,
    /// The question on screen.
    pub cursor: usize,
    /// Highlighted row per question (parallel to `questions`).
    pub highlighted: Vec<usize>,
    /// Multi-select toggles per question (parallel to `questions`; empty for
    /// single-select questions).
    pub checked: Vec<Vec<bool>>,
    /// Custom-answer text per question (parallel to `questions`).
    pub custom: Vec<String>,
    /// Shared text field for the current question's custom answer.
    pub input: Entity<ComposerInput>,
    /// The questionnaire was committed; buffered answers are replaying to the
    /// extension, so the panel is read-only.
    pub submitted: bool,
}

impl AskPrompt {
    /// The question currently on screen.
    pub fn question(&self) -> Option<&AskQuestion> {
        self.questions.get(self.cursor)
    }

    pub fn total(&self) -> usize {
        self.questions.len()
    }

    pub fn is_multi(&self) -> bool {
        self.question()
            .is_some_and(|question| question.multi_select)
    }

    /// The highlighted row for the current question.
    pub fn highlighted(&self) -> usize {
        self.highlighted.get(self.cursor).copied().unwrap_or(0)
    }

    /// Whether the current question offers a trailing "Type something." row.
    pub fn has_custom_row(&self) -> bool {
        self.question()
            .is_some_and(|question| !question.multi_select)
    }

    /// Selectable rows for the current question: its options plus the trailing
    /// "Type something." row on single-select questions.
    pub fn row_count(&self) -> usize {
        let Some(question) = self.question() else {
            return 0;
        };
        question.options.len() + usize::from(!question.multi_select)
    }

    /// The custom-answer field is meaningful on multi questions (always) and
    /// on single-select questions when the trailing row is highlighted.
    pub fn custom_active(&self) -> bool {
        match self.question() {
            Some(question) if question.multi_select => true,
            _ => self.has_custom_row() && self.highlighted() >= self.row_count().saturating_sub(1),
        }
    }
}

/// The cursor over a questionnaire being replayed to the extension, plus the
/// text a single-select "Type something." choice owes the `input` request that
/// follows its `select`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AskReplayState {
    /// Index of the next question to answer.
    pub ix: usize,
    /// Text for the pending custom follow-up `input`, if any.
    pub pending_custom: Option<String>,
}

/// What to send the extension for its current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskReplayReply {
    /// Send this value as `extension_ui_response`.
    Value(String),
    /// The buffered answers cannot answer this request — decline.
    Cancel,
}

impl AskReplayState {
    /// Answer one `select` / `input` request from the buffered answers,
    /// advancing when a question is fully consumed. A `select` that chooses
    /// "Type something." records the owed text and leaves `ix` put so the
    /// `input` that follows completes the same question.
    pub fn reply(
        &mut self,
        method: &str,
        questions: &[AskQuestion],
        answers: &[AskAnswer],
    ) -> AskReplayReply {
        if method == "select" {
            let answer = answers.get(self.ix).cloned().unwrap_or_default();
            let options_len = questions
                .get(self.ix)
                .map(|question| question.options.len())
                .unwrap_or(0);
            let label = match (&answer, questions.get(self.ix)) {
                (AskAnswer::Option(ix), Some(question)) => question
                    .options
                    .get(*ix)
                    .map(|option| option.label.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            match answer {
                AskAnswer::Option(ix) => {
                    self.ix += 1;
                    AskReplayReply::Value(format!("{}. {}", ix + 1, label))
                }
                AskAnswer::Custom(text) => {
                    self.pending_custom = Some(text);
                    AskReplayReply::Value(format!(
                        "{}. {}",
                        options_len + 1,
                        tr!("view.type_something")
                    ))
                }
                _ => AskReplayReply::Cancel,
            }
        } else if let Some(text) = self.pending_custom.take() {
            self.ix += 1;
            AskReplayReply::Value(text)
        } else {
            match answers.get(self.ix).cloned() {
                Some(AskAnswer::Multi { checked, custom }) => {
                    let custom = custom.trim();
                    let value = if custom.is_empty() {
                        checked
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .map(|(ix, _)| (ix + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    } else {
                        custom.to_string()
                    };
                    self.ix += 1;
                    AskReplayReply::Value(value)
                }
                _ => AskReplayReply::Cancel,
            }
        }
    }
}

/// Tool names that carry the structured questionnaire. `ask_user_question` is
/// the extension's name; the shorter aliases cover older/renamed registrations
/// and the analytics classifier's vocabulary.
pub fn is_ask_tool(name: &str) -> bool {
    matches!(name, "ask_user_question" | "ask_question" | "ask_user")
}

/// Parse the `questions` array out of an `ask_user_question` tool's arguments.
/// Tolerant by design: a missing or malformed payload yields an empty list
/// (the caller then leaves the request to the generic dialog path), never an
/// error.
pub fn questions_from_args(args: Option<&Value>) -> Vec<AskQuestion> {
    let Some(list) = args
        .and_then(|args| args.get("questions"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    list.iter().filter_map(question_from_value).collect()
}

fn question_from_value(value: &Value) -> Option<AskQuestion> {
    // A question without text is not renderable; skip it rather than show a
    // blank card.
    let question = value.get("question").and_then(Value::as_str)?.to_string();
    let header = value
        .get("header")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let multi_select = value
        .get("multiSelect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(option_from_value)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(AskQuestion {
        header,
        question,
        multi_select,
        options,
    })
}

fn option_from_value(value: &Value) -> Option<AskOption> {
    let label = value.get("label").and_then(Value::as_str)?.to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let preview = value
        .get("preview")
        .and_then(Value::as_str)
        .filter(|preview| !preview.is_empty())
        .map(str::to_string);
    Some(AskOption {
        label,
        description,
        preview,
    })
}

/// Whether the tool result is the extension's "declined" envelope. The
/// extension uses one canonical string for every way a questionnaire can end
/// without an answer.
pub fn is_declined(output: Option<&Value>) -> bool {
    matches!(output, Some(Value::String(text)) if text.contains("declined to answer"))
}

/// The answer envelope's raw text, when the tool completed with answers.
pub fn answer_envelope(output: Option<&Value>) -> Option<&str> {
    match output {
        Some(Value::String(text)) if text.starts_with("User has answered your questions:") => {
            Some(text.as_str())
        }
        _ => None,
    }
}

/// The comma-separated labels (or typed text) inside one answer.
fn answer_parts(answer: &str) -> impl Iterator<Item = &str> {
    answer
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

/// Whether one of the answer's parts is `label`. Multi-select answers arrive
/// comma-joined (`"A, B"`), so a whole-string comparison would miss them.
pub fn answer_includes_label(answer: &str, label: &str) -> bool {
    answer_parts(answer).any(|part| part == label)
}

/// Whether every part of the answer is one of the question's option labels
/// (i.e. the user picked options rather than typing a custom answer).
pub fn answer_is_option_selection(question: &AskQuestion, answer: &str) -> bool {
    let mut any = false;
    for part in answer_parts(answer) {
        any = true;
        if !question.options.iter().any(|option| option.label == part) {
            return false;
        }
    }
    any
}

/// The answer the envelope records for one question, if any.
///
/// The envelope is `User has answered your questions: "Q"="A". "Q2"="A2". …`,
/// where `A` is an option label (multi answers comma-joined) or the typed
/// custom text. Anchoring on the quoted question text reads exactly that
/// answer back, so the transcript can show what was chosen instead of asking
/// the reader to infer it from the options.
pub fn envelope_answer<'a>(envelope: &'a str, question: &str) -> Option<&'a str> {
    let needle = format!("\"{question}\"=\"");
    let start = envelope.find(&needle)? + needle.len();
    let rest = &envelope[start..];
    // The answer runs to the closing quote that ends the segment — `". `
    // before the next segment, or `".` at the very end.
    let end = rest
        .find("\". ")
        .or_else(|| rest.find("\"."))
        .unwrap_or(rest.len());
    let answer = rest[..end].trim();
    (!answer.is_empty()).then_some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_questions_and_options() {
        let questions = questions_from_args(Some(&json!({
            "questions": [{
                "header": "Auth method",
                "question": "Which auth should we use?",
                "multiSelect": true,
                "options": [
                    {"label": "OAuth", "description": "Delegated login."},
                    {"label": "API key", "description": "Static token.", "preview": "KEY=…"},
                ],
            }],
        })));
        assert_eq!(questions.len(), 1);
        let question = &questions[0];
        assert_eq!(question.header, "Auth method");
        assert_eq!(question.question, "Which auth should we use?");
        assert!(question.multi_select);
        assert_eq!(question.options.len(), 2);
        assert_eq!(question.options[1].preview.as_deref(), Some("KEY=…"));
    }

    #[test]
    fn malformed_payloads_degrade_to_empty() {
        assert!(questions_from_args(None).is_empty());
        assert!(questions_from_args(Some(&json!({"questions": "nope"}))).is_empty());
        // A question with no text is skipped, not rendered blank.
        assert!(questions_from_args(Some(&json!({
            "questions": [{"header": "X", "options": []}]
        })))
        .is_empty());
    }

    #[test]
    fn reads_the_answer_envelope() {
        let answered = json!("User has answered your questions: \"Q\"=\"OAuth\". You can now continue with the user's answers in mind.");
        let envelope = answer_envelope(Some(&answered)).unwrap();
        assert_eq!(envelope_answer(envelope, "Q"), Some("OAuth"));
        assert_eq!(envelope_answer(envelope, "Other"), None);
        assert!(answer_includes_label("OAuth", "OAuth"));
        assert!(answer_includes_label("A, B", "B"));
        assert!(!answer_includes_label("A, B", "C"));
        assert!(is_declined(Some(&json!(
            "User declined to answer questions"
        ))));
        assert!(!is_declined(Some(&answered)));
    }

    /// Shorthand for a question with the given options.
    fn ask_question(multi: bool, labels: &[&str]) -> AskQuestion {
        AskQuestion {
            multi_select: multi,
            options: labels
                .iter()
                .map(|label| AskOption {
                    label: (*label).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn replay_walks_options_then_multi() {
        let questions = vec![
            ask_question(false, &["A", "B"]),
            ask_question(true, &["X", "Y"]),
        ];
        let answers = vec![
            AskAnswer::Option(1),
            AskAnswer::Multi {
                checked: vec![false, true],
                custom: String::new(),
            },
        ];
        let mut state = AskReplayState::default();
        assert_eq!(
            state.reply("select", &questions, &answers),
            AskReplayReply::Value("2. B".into())
        );
        assert_eq!(state.ix, 1);
        assert_eq!(
            state.reply("input", &questions, &answers),
            AskReplayReply::Value("2".into())
        );
        assert_eq!(state.ix, 2);
    }

    #[test]
    fn replay_answers_a_custom_select_follow_up() {
        let questions = vec![ask_question(false, &["A", "B"])];
        let answers = vec![AskAnswer::Custom("hello".into())];
        let mut state = AskReplayState::default();
        // The select picks the trailing row and waits for the input.
        assert_eq!(
            state.reply("select", &questions, &answers),
            AskReplayReply::Value("3. Type something.".into())
        );
        assert_eq!(state.ix, 0);
        assert_eq!(
            state.reply("input", &questions, &answers),
            AskReplayReply::Value("hello".into())
        );
        assert_eq!(state.ix, 1);
    }

    #[test]
    fn replay_prefers_multi_custom_over_toggles() {
        let questions = vec![ask_question(true, &["X", "Y"])];
        let answers = vec![AskAnswer::Multi {
            checked: vec![true, false],
            custom: "other".into(),
        }];
        let mut state = AskReplayState::default();
        assert_eq!(
            state.reply("input", &questions, &answers),
            AskReplayReply::Value("other".into())
        );
    }

    #[test]
    fn replay_declines_unanswerable_requests() {
        let questions = vec![ask_question(false, &["A"])];
        let answers = vec![AskAnswer::Unanswered];
        let mut state = AskReplayState::default();
        assert_eq!(
            state.reply("select", &questions, &answers),
            AskReplayReply::Cancel
        );
    }

    #[test]
    fn tells_option_selections_from_custom_answers() {
        let question = AskQuestion {
            options: vec![
                AskOption {
                    label: "A".into(),
                    ..Default::default()
                },
                AskOption {
                    label: "B".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(answer_is_option_selection(&question, "A"));
        assert!(answer_is_option_selection(&question, "A, B"));
        assert!(!answer_is_option_selection(&question, "something else"));
        assert!(!answer_is_option_selection(&question, "A, something else"));
    }
}
