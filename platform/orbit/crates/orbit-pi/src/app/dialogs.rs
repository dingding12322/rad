use super::*;

impl OrbitApp {
    /// Route an `extension_ui_request` from pi.
    ///
    /// Dialog methods (`select` / `confirm` / `input` / `editor`) open the
    /// modal [`Dialog`] and block the run until the user answers. The
    /// remaining methods are fire-and-forget chrome; the ones with a desktop
    /// equivalent are surfaced, the rest are ignored (never faked).
    pub(super) fn handle_extension_ui_request(
        &mut self,
        id: String,
        method: &str,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        match method {
            "select" | "confirm" | "input" | "editor" => {
                // A committed questionnaire replays its buffered answers in
                // place; while it does, its requests are answered directly
                // rather than opening another surface.
                if self.ask_replay.is_some() && matches!(method, "select" | "input") {
                    self.ask_replay_reply(&id, method);
                    return;
                }
                // The access guard's `select` is marked so it renders as the
                // inline approval bar instead of the blocking modal.
                if let Some(request) = ApprovalRequest::from_event(id.clone(), method, value) {
                    self.open_approval(request, cx);
                    return;
                }
                // A running `ask_user_question` owns its `select` / `input`
                // primitives: answer them on the inline panel above the
                // composer instead of the scrim modal.
                if self.ask_is_live() && matches!(method, "select" | "input") {
                    self.open_ask(id, cx);
                    return;
                }
                if let Some(request) = DialogRequest::from_event(id, method, value) {
                    self.open_dialog(request, cx);
                }
            }
            // `notify` is a user-facing toast — render it as one. `setStatus`
            // stays on the quiet transient status line.
            "notify" => {
                if let Some(message) = value.get("message").and_then(Value::as_str) {
                    self.toast_info(message.to_owned());
                    cx.notify();
                }
            }
            "setStatus" => {
                if let Some(text) = value.get("statusText").and_then(Value::as_str) {
                    self.set_status(text.to_owned());
                }
            }
            // Seed the composer with the extension's text (pi's `setEditorText`).
            "set_editor_text" => {
                if let Some(text) = value.get("text").and_then(Value::as_str) {
                    self.input.update(cx, |input, cx| input.set_text(text, cx));
                }
            }
            // `custom` streams a component's rendered frames; see
            // `crate::custom_ui`.
            "custom" => self.apply_custom_frame(value, cx),
            // `setWidget` is a small text block the extension wants above or
            // below the editor; render it natively.
            "setWidget" => self.apply_extension_widget(value, cx),
            // `setTitle` is terminal chrome with no desktop home.
            _ => {}
        }
    }

    /// Apply an extension `setWidget` request: replace — or, when the payload
    /// carries no lines, clear — one keyed text block. RPC delivers only the
    /// string-array form; component factories never reach Orbit.
    fn apply_extension_widget(&mut self, value: &Value, cx: &mut Context<Self>) {
        let Some(key) = value.get("widgetKey").and_then(Value::as_str) else {
            return;
        };
        let placement = match value.get("widgetPlacement").and_then(Value::as_str) {
            Some("belowEditor") => WidgetPlacement::BelowEditor,
            _ => WidgetPlacement::AboveEditor,
        };
        let lines: Vec<String> = value
            .get("widgetLines")
            .and_then(Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| line.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(ix) = self.extension_widgets.iter().position(|w| w.key == key) {
            if lines.is_empty() {
                self.extension_widgets.remove(ix);
            } else {
                self.extension_widgets[ix].lines = lines;
                self.extension_widgets[ix].placement = placement;
            }
        } else if !lines.is_empty() {
            self.extension_widgets.push(ExtensionWidget {
                key: key.to_string(),
                placement,
                lines,
            });
        }
        cx.notify();
    }

    /// Apply a `custom` extension-UI frame: open a surface, update it, or
    /// close it. The server has already composited the component into final
    /// lines, so each frame is just text plus a cursor.
    fn apply_custom_frame(&mut self, value: &Value, cx: &mut Context<Self>) {
        let Some(frame) = CustomFrame::from_event(value) else {
            return;
        };
        match frame {
            CustomFrame::Open(surface) => {
                // A repeat `open` for a live id (e.g. a re-render) updates in
                // place rather than stacking a duplicate surface.
                if let Some(existing) = self.custom_ui.find(&surface.id) {
                    existing.update(cx, |ui, cx| {
                        ui.apply(surface);
                        cx.notify();
                    });
                    return;
                }
                let id = surface.id.clone();
                let this = cx.weak_entity();
                let input_id = id.clone();
                let on_input: CustomInput = Box::new(move |data, _window, cx| {
                    let payload = serde_json::json!({
                        "type": "extension_ui_input",
                        "id": input_id,
                        "data": data,
                    });
                    let _ = this.update(cx, |app, cx| {
                        app.send(CommandBody::Raw(payload), "extension_ui_input");
                        cx.notify();
                    });
                });
                let this = cx.weak_entity();
                let cancel_id = id.clone();
                let on_cancel: CustomCancel = Box::new(move |_window, cx| {
                    let _ = this.update(cx, |app, cx| app.close_custom_ui(&cancel_id, cx));
                });
                let ui = cx.new(|cx| CustomUi::new(surface, on_input, on_cancel, cx));
                self.custom_ui.push(id.clone(), ui);
                self.custom_ui_focus_pending = true;
                self.send_custom_resize(&id);
            }
            CustomFrame::Render(surface) => {
                let id = surface.id.clone();
                if let Some(ui) = self.custom_ui.find(&id) {
                    ui.update(cx, |ui, cx| {
                        ui.apply(surface);
                        cx.notify();
                    });
                }
            }
            CustomFrame::Close { id, .. } => {
                self.custom_ui.remove(&id);
                // Focus the new top surface, or the composer if none is left.
                self.custom_ui_focus_pending = true;
            }
        }
        cx.notify();
    }

    /// Report the host's viewport to a custom component so it re-renders at
    /// the width the card will actually paint at.
    fn send_custom_resize(&mut self, id: &str) {
        let payload = serde_json::json!({
            "type": "extension_ui_resize",
            "id": id,
            "width": crate::custom_ui::CUSTOM_UI_COLUMNS,
            "height": crate::custom_ui::CUSTOM_UI_ROWS,
        });
        self.send(CommandBody::Raw(payload), "extension_ui_resize");
    }

    /// Cancel a custom surface and tell pi, so the extension's pending promise
    /// resolves instead of waiting forever. Used by the scrim and on session
    /// switches.
    pub(super) fn close_custom_ui(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.custom_ui.contains(id) {
            return;
        }
        let payload = serde_json::json!({
            "type": "extension_ui_response",
            "id": id,
            "cancelled": true,
        });
        self.send(CommandBody::Raw(payload), "extension_ui_response");
        self.custom_ui.remove(id);
        // Focus the new top surface, or the composer if none is left.
        self.custom_ui_focus_pending = true;
        cx.notify();
    }

    /// Open the modal dialog for a dialog request. pi blocks one request at a
    /// time, so if a dialog is already up the newcomer is cancelled rather
    /// than replacing (and stranding) the first.
    pub(super) fn open_dialog(&mut self, request: DialogRequest, cx: &mut Context<Self>) {
        if self.dialog.is_some() || self.approval.is_some() {
            self.respond_to_dialog(&request.id, &DialogResponse::Cancelled);
            return;
        }
        let id = request.id.clone();
        // The entity talks back through a weak handle, the same shape the
        // command palette uses; the app owns the client the answer is sent on.
        let this = cx.weak_entity();
        let on_respond = Box::new(
            move |response: DialogResponse, window: &mut Window, cx: &mut App| {
                let id = id.clone();
                this.update(cx, |app, cx| {
                    app.dialog = None;
                    app.dialog_focus_pending = false;
                    app.respond_to_dialog(&id, &response);
                    // The dialog's focus handle dies with it; hand focus back
                    // to the composer so typing continues.
                    app.input.read(cx).focus(window);
                    cx.notify();
                })
                .ok();
            },
        ) as Box<dyn Fn(DialogResponse, &mut Window, &mut App)>;
        let dialog = cx.new(|cx| Dialog::new(request, on_respond, cx));
        self.dialog = Some(dialog);
        self.dialog_focus_pending = true;
        cx.notify();
    }

    /// Send the answer for a dialog id over RPC.
    pub(super) fn respond_to_dialog(&mut self, id: &str, response: &DialogResponse) {
        let payload = match response {
            DialogResponse::Value(value) => serde_json::json!({
                "type": "extension_ui_response",
                "id": id,
                "value": value
            }),
            DialogResponse::Confirmed(confirmed) => serde_json::json!({
                "type": "extension_ui_response",
                "id": id,
                "confirmed": confirmed
            }),
            DialogResponse::Cancelled => serde_json::json!({
                "type": "extension_ui_response",
                "id": id,
                "cancelled": true
            }),
        };
        self.send(CommandBody::Raw(payload), "extension_ui_response");
    }

    /// Open the inline approval bar for a guard request. pi blocks one request
    /// at a time, so if a modal or another approval is already up the newcomer
    /// is cancelled rather than replacing (and stranding) the first.
    pub(super) fn open_approval(&mut self, request: ApprovalRequest, cx: &mut Context<Self>) {
        if self.dialog.is_some() || self.approval.is_some() {
            self.respond_to_dialog(&request.id, &DialogResponse::Cancelled);
            return;
        }
        self.approval_highlight = 0;
        self.approval = Some(request);
        self.approval_focus_pending = true;
        cx.notify();
    }

    /// Answer the open approval with the chosen option (`None` = cancelled /
    /// dismissed, which the extension treats as a denial). Hands focus back to
    /// the composer so typing continues.
    pub(super) fn respond_to_approval(
        &mut self,
        option: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.approval.take() else {
            return;
        };
        self.approval_focus_pending = false;
        self.approval_highlight = 0;
        let response = match option {
            Some(value) => DialogResponse::Value(value),
            None => DialogResponse::Cancelled,
        };
        self.respond_to_dialog(&request.id, &response);
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// Cancel and close the open approval (session replaced, pi exited). The
    /// departing run would otherwise stay blocked forever on a prompt no one
    /// can see.
    pub(super) fn cancel_open_approval(&mut self) {
        if let Some(request) = self.approval.take() {
            self.approval_focus_pending = false;
            self.approval_highlight = 0;
            self.respond_to_dialog(&request.id, &DialogResponse::Cancelled);
        }
    }

    /// Cancel and close the open dialog and/or approval. Called when the active
    /// session is replaced: the departing (now parked) run would otherwise stay
    /// blocked forever on a question no one can see.
    pub(super) fn cancel_open_dialog(&mut self, cx: &mut Context<Self>) {
        self.cancel_open_approval();
        // The questionnaire belongs to the departing session too — decline it
        // so its parked run can settle. A committed questionnaire is mid-replay
        // instead, so decline whatever request the replay is currently on.
        if let Some(prompt) = self.ask.take() {
            if prompt.submitted {
                if let Some(id) = self
                    .ask_replay
                    .as_ref()
                    .and_then(|replay| replay.current_id.clone())
                {
                    self.respond_to_dialog(&id, &DialogResponse::Cancelled);
                }
            } else {
                self.respond_to_dialog(&prompt.id, &DialogResponse::Cancelled);
            }
        }
        self.ask_tool_id = None;
        self.ask_questions.clear();
        self.ask_replay = None;
        self.ask_focus_pending = false;
        // Custom-UI surfaces belong to the departing session too; cancel them
        // so their parked runs can settle.
        let custom_ids: Vec<String> = self.custom_ui.ids().map(str::to_string).collect();
        for id in custom_ids {
            self.close_custom_ui(&id, cx);
        }
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        self.dialog_focus_pending = false;
        let id = dialog.read(cx).request_id().to_string();
        self.respond_to_dialog(&id, &DialogResponse::Cancelled);
        cx.notify();
    }

    // ── approval bar keyboard control (the `Approval` key context) ──────

    pub(super) fn on_approval_next(
        &mut self,
        _: &crate::ApprovalNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_approval_highlight(true, cx);
    }

    pub(super) fn on_approval_prev(
        &mut self,
        _: &crate::ApprovalPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_approval_highlight(false, cx);
    }

    pub(super) fn on_approval_confirm(
        &mut self,
        _: &crate::ApprovalConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = self.approval_highlight;
        self.approval_choose(ix, window, cx);
    }

    pub(super) fn on_approval_close(
        &mut self,
        _: &crate::ApprovalClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.respond_to_approval(None, window, cx);
    }

    fn move_approval_highlight(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.approval.as_ref().map(|r| r.options.len()).unwrap_or(0);
        if count == 0 {
            return;
        }
        self.approval_highlight = if forward {
            (self.approval_highlight + 1) % count
        } else {
            (self.approval_highlight + count - 1) % count
        };
        cx.notify();
    }

    /// Answer the open approval with option `ix` (its label string is what the
    /// extension maps back).
    pub(super) fn approval_choose(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let option = self
            .approval
            .as_ref()
            .and_then(|request| request.options.get(ix).cloned());
        match option {
            Some(option) => self.respond_to_approval(Some(option), window, cx),
            None => self.respond_to_approval(None, window, cx),
        }
    }
}
