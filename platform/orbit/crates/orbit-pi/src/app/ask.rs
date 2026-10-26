use super::*;
use crate::ask::{self, AskAnswer, AskQuestion};

/// Replays a committed questionnaire back to the extension.
///
/// The whole questionnaire is answered locally (that is what lets the user go
/// back and change an earlier answer); on commit the buffered answers are fed
/// to the extension one blocking `select` / `input` request at a time, exactly
/// as if the user had answered each dialog in turn. The response sequence
/// itself lives in [`ask::AskReplayState`], so it is unit-testable.
pub(super) struct AskReplay {
    questions: Vec<AskQuestion>,
    answers: Vec<AskAnswer>,
    state: ask::AskReplayState,
    /// RPC id of the request being answered right now, so a departing session
    /// can still be declined out of its parked block.
    pub(super) current_id: Option<String>,
}

impl OrbitApp {
    /// Record a running `ask_user_question` call and parse its questionnaire.
    /// Called from the event pump before the transcript applies the same
    /// event, so the panel is ready before the extension's first `select`.
    pub(super) fn on_ask_tool_start(&mut self, value: &Value, cx: &mut Context<Self>) {
        let tool = value.get("toolName").and_then(Value::as_str).unwrap_or("");
        if !ask::is_ask_tool(tool) {
            return;
        }
        self.ask_tool_id = value
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.ask_questions = ask::questions_from_args(value.get("args"));
        self.ask_replay = None;
        cx.notify();
    }

    /// The running ask tool finished — its panel (if any) can never be
    /// answered, so drop it. Matching by tool call id keeps a concurrent
    /// tool's end from clearing the wrong questionnaire.
    pub(super) fn on_ask_tool_end(&mut self, value: &Value, cx: &mut Context<Self>) {
        let Some(id) = value.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        if self.ask_tool_id.as_deref() != Some(id) {
            return;
        }
        self.ask_tool_id = None;
        self.ask_questions.clear();
        self.ask = None;
        self.ask_replay = None;
        self.ask_focus_pending = false;
        cx.notify();
    }

    /// True when a questionnaire is live and may claim a `select` / `input`.
    pub(super) fn ask_is_live(&self) -> bool {
        self.ask_tool_id.is_some() && !self.ask_questions.is_empty()
    }

    /// Open the inline questionnaire panel for the running tool's first
    /// blocking request. Every question is known up front (parsed from the
    /// tool arguments), so the panel drives the whole questionnaire locally
    /// and only talks back to the extension on commit.
    pub(super) fn open_ask(&mut self, id: String, cx: &mut Context<Self>) {
        // pi blocks one request at a time; a modal that is somehow already up
        // owns the answer, and a live panel already owns the questionnaire.
        if self.dialog.is_some() || self.approval.is_some() || self.ask.is_some() {
            self.respond_to_dialog(&id, &DialogResponse::Cancelled);
            return;
        }
        if self.ask_questions.is_empty() {
            self.respond_to_dialog(&id, &DialogResponse::Cancelled);
            return;
        }
        let questions = self.ask_questions.clone();
        let count = questions.len();
        let checked = questions
            .iter()
            .map(|question| vec![false; question.options.len()])
            .collect();
        let input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_element_id("ask-input")
                .with_placeholder_key("ask.type_your_answer")
                .with_max_lines(3)
                .with_key_context("Composer AskInput")
        });
        self.ask = Some(AskPrompt {
            id,
            questions,
            cursor: 0,
            highlighted: vec![0; count],
            checked,
            custom: vec![String::new(); count],
            input,
            submitted: false,
        });
        self.ask_focus_pending = true;
        cx.notify();
    }

    /// Snapshot the live text field into the current question's buffered
    /// custom answer, so navigating away (or committing) never loses it.
    fn ask_sync_custom(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        let text = prompt.input.read(cx).text();
        let cursor = prompt.cursor;
        if let Some(slot) = prompt.custom.get_mut(cursor) {
            *slot = text;
        }
    }

    /// Restore the text field from the buffered custom answer for the cursor.
    fn ask_restore_custom(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        let cursor = prompt.cursor;
        let text = prompt.custom.get(cursor).cloned().unwrap_or_default();
        prompt
            .input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Move the question cursor (Back / Next within the panel), carrying the
    /// current question's custom text along.
    fn ask_goto(&mut self, cursor: usize, cx: &mut Context<Self>) {
        self.ask_sync_custom(cx);
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        if cursor >= prompt.questions.len() || cursor == prompt.cursor {
            return;
        }
        prompt.cursor = cursor;
        self.ask_restore_custom(cx);
        self.ask_focus_pending = true;
        cx.notify();
    }

    /// Move the highlight within the current question (arrow keys). Select
    /// questions include the trailing "Type something." row; multi questions
    /// list only their options.
    pub(super) fn ask_move(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        if prompt.submitted {
            return;
        }
        let count = prompt.row_count();
        if count == 0 {
            return;
        }
        let cursor = prompt.cursor;
        let current = prompt.highlighted();
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        if let Some(slot) = prompt.highlighted.get_mut(cursor) {
            *slot = next;
        }
        cx.notify();
    }

    /// Click / keyboard action for one option row. A single-select row only
    /// records the choice (Next advances); a multi-select row toggles.
    pub(super) fn ask_choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        if prompt.submitted {
            return;
        }
        let cursor = prompt.cursor;
        if prompt.is_multi() {
            if let Some(slot) = prompt
                .checked
                .get_mut(cursor)
                .and_then(|row| row.get_mut(ix))
            {
                *slot = !*slot;
            }
        } else if ix >= prompt.row_count() {
            return;
        }
        if let Some(slot) = prompt.highlighted.get_mut(cursor) {
            *slot = ix;
        }
        cx.notify();
    }

    /// Advance to the next question, or commit the questionnaire when the
    /// current one is the last. Called by the Next / Submit button and by
    /// Enter on a single-select question.
    pub(super) fn ask_next_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_ref() else {
            return;
        };
        if prompt.submitted {
            return;
        }
        if prompt.cursor + 1 < prompt.questions.len() {
            let cursor = prompt.cursor + 1;
            self.ask_goto(cursor, cx);
        } else {
            self.ask_commit(window, cx);
        }
    }

    /// Step back to the previous question so its answer can be changed.
    pub(super) fn ask_prev_question(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_ref() else {
            return;
        };
        if prompt.submitted || prompt.cursor == 0 {
            return;
        }
        let cursor = prompt.cursor - 1;
        self.ask_goto(cursor, cx);
    }

    /// Commit the whole questionnaire: freeze the panel and replay every
    /// buffered answer to the extension, starting with the request that is
    /// currently blocking pi.
    fn ask_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_sync_custom(cx);
        let Some(prompt) = self.ask.as_mut() else {
            return;
        };
        if prompt.submitted {
            return;
        }
        let answers: Vec<AskAnswer> = prompt
            .questions
            .iter()
            .enumerate()
            .map(|(ix, question)| {
                if question.multi_select {
                    AskAnswer::Multi {
                        checked: prompt
                            .checked
                            .get(ix)
                            .cloned()
                            .unwrap_or_else(|| vec![false; question.options.len()]),
                        custom: prompt.custom.get(ix).cloned().unwrap_or_default(),
                    }
                } else {
                    let highlighted = prompt.highlighted.get(ix).copied().unwrap_or(0);
                    if highlighted >= question.options.len() {
                        AskAnswer::Custom(prompt.custom.get(ix).cloned().unwrap_or_default())
                    } else {
                        AskAnswer::Option(highlighted)
                    }
                }
            })
            .collect();
        prompt.submitted = true;
        let id = prompt.id.clone();
        let questions = prompt.questions.clone();
        let first_is_multi = questions
            .first()
            .is_some_and(|question| question.multi_select);
        self.ask_replay = Some(AskReplay {
            questions,
            answers,
            state: ask::AskReplayState::default(),
            current_id: Some(id.clone()),
        });
        // Hand focus back to the composer: the panel is now read-only.
        self.input.read(cx).focus(window);
        cx.notify();
        self.ask_replay_reply(&id, if first_is_multi { "input" } else { "select" });
    }

    /// Answer the extension's current questionnaire request from the buffered
    /// answers, advancing through the questionnaire. Runs once per request
    /// until every answer has been replayed.
    pub(super) fn ask_replay_reply(&mut self, id: &str, method: &str) {
        let reply = {
            let Some(replay) = self.ask_replay.as_mut() else {
                return;
            };
            replay.current_id = Some(id.to_string());
            replay
                .state
                .reply(method, &replay.questions, &replay.answers)
        };

        match reply {
            ask::AskReplayReply::Value(value) => {
                self.respond_to_dialog(id, &DialogResponse::Value(value))
            }
            ask::AskReplayReply::Cancel => {
                self.ask_replay = None;
                self.respond_to_dialog(id, &DialogResponse::Cancelled);
            }
        }
        if self
            .ask_replay
            .as_ref()
            .is_some_and(|replay| replay.state.ix >= replay.questions.len())
        {
            self.ask_replay = None;
        }
    }

    /// Cancel the questionnaire (Esc / session replaced): pi sees a decline
    /// and the run continues without an answer.
    pub(super) fn ask_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.ask.as_ref() else {
            return;
        };
        // A committed questionnaire is already replaying; there is nothing
        // left to decline.
        if prompt.submitted {
            return;
        }
        let id = prompt.id.clone();
        self.ask = None;
        self.ask_replay = None;
        self.ask_focus_pending = false;
        self.respond_to_dialog(&id, &DialogResponse::Cancelled);
        self.input.read(cx).focus(window);
        cx.notify();
    }

    // ── key handlers (the `AskPanel` / `AskInput` contexts) ──────────────

    pub(super) fn on_ask_next(
        &mut self,
        _: &crate::AskNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask_move(true, cx);
    }

    pub(super) fn on_ask_prev(
        &mut self,
        _: &crate::AskPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask_move(false, cx);
    }

    pub(super) fn on_ask_confirm(
        &mut self,
        _: &crate::AskConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // On a multi-select question Enter/Space toggles the highlighted row;
        // the Next button commits. Every other question advances on Enter.
        if self.ask.as_ref().is_some_and(AskPrompt::is_multi) {
            let ix = self.ask.as_ref().map(AskPrompt::highlighted).unwrap_or(0);
            self.ask_choose(ix, cx);
        } else {
            self.ask_next_question(window, cx);
        }
    }

    /// Enter inside the panel's text field advances (submits on the last
    /// question), matching the Next button.
    pub(super) fn on_ask_submit(
        &mut self,
        _: &crate::AskSubmit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Enter may be confirming an IME candidate in the custom-answer
        // field; that must not advance or commit the questionnaire.
        if self
            .ask
            .as_ref()
            .is_some_and(|prompt| prompt.input.read(cx).is_composing())
        {
            return;
        }
        self.ask_next_question(window, cx);
    }

    pub(super) fn on_ask_close(
        &mut self,
        _: &crate::AskClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask_cancel(window, cx);
    }
}
