use crate::store::{NewRun, Store};
use omb_wire::{ApprovalRequest, ApprovalState, MessageRole, RuntimeEvent, ToolCall, TurnUsage};
use std::collections::HashMap;

/// Persists one turn's events: assistant text and each tool call become messages, in the order they happened.
pub struct TurnRecorder<'a> {
    store: &'a Store,
    bot_id: i64,
    text: String,
    tools: HashMap<String, (i64, ToolCall)>,
    requests: HashMap<String, (i64, ApprovalRequest)>,
    started: std::time::Instant,
    tool_uses: u32,
    usage: Option<TurnUsage>,
    failed: bool,
}

impl<'a> TurnRecorder<'a> {
    pub fn new(store: &'a Store, bot_id: i64) -> Self {
        Self { store, bot_id, text: String::new(), tools: HashMap::new(), requests: HashMap::new(), started: std::time::Instant::now(), tool_uses: 0, usage: None, failed: false }
    }

    pub fn apply(&mut self, event: &RuntimeEvent) -> Result<(), String> {
        match event {
            RuntimeEvent::TurnStarted { session_id, .. } if !session_id.is_empty() => self.store.set_session(self.bot_id, session_id),
            RuntimeEvent::TurnStarted { .. } => Ok(()),
            RuntimeEvent::ContentDelta { text } => {
                self.text.push_str(text);
                Ok(())
            }
            RuntimeEvent::ItemStarted { id, name, input } => {
                self.flush_text()?;
                self.tool_uses += 1;
                let call = ToolCall { name: name.clone(), input: input.clone(), ok: None, output: String::new() };
                let message = self.store.add_message(self.bot_id, MessageRole::Tool, &body_of(&call))?;
                self.tools.insert(id.clone(), (message.id, call));
                Ok(())
            }
            RuntimeEvent::ItemCompleted { id, ok, output } => match self.tools.get_mut(id) {
                Some((message_id, call)) => {
                    call.ok = Some(*ok);
                    call.output = output.clone();
                    self.store.set_message_body(*message_id, &body_of(call))
                }
                None => Ok(()),
            },
            RuntimeEvent::RequestOpened { request_id, tool, input } => {
                self.flush_text()?;
                let request = ApprovalRequest { request_id: request_id.clone(), tool: tool.clone(), input: input.clone(), state: ApprovalState::Pending };
                let message = self.store.add_message(self.bot_id, MessageRole::Approval, &body_of(&request))?;
                self.requests.insert(request_id.clone(), (message.id, request));
                Ok(())
            }
            RuntimeEvent::RequestResolved { request_id, allowed } => match self.requests.get_mut(request_id) {
                Some((message_id, request)) => {
                    request.state = if *allowed { ApprovalState::Allowed } else { ApprovalState::Denied };
                    self.store.set_message_body(*message_id, &body_of(request))
                }
                None => Ok(()),
            },
            RuntimeEvent::TurnCompleted { is_error, result, usage } => {
                self.flush_text()?;
                self.usage = usage.clone();
                self.failed |= *is_error;
                if *is_error {
                    let said = result.as_deref().filter(|r| !r.is_empty()).unwrap_or("The turn failed");
                    self.store.add_message(self.bot_id, MessageRole::Error, said)?;
                }
                Ok(())
            }
            RuntimeEvent::RuntimeError { message } => {
                self.flush_text()?;
                self.failed = true;
                self.store.add_message(self.bot_id, MessageRole::Error, message).map(|_| ())
            }
        }
    }

    /// Saves what the turn cost. Call once the turn is over; a turn that reported nothing and did not fail is not a run.
    pub fn finish(&self, prompt: &str) -> Result<(), String> {
        if self.usage.is_none() && !self.failed {
            return Ok(());
        }
        let bot_name = self.store.bot(self.bot_id)?.map(|bot| bot.name).unwrap_or_default();
        let title: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
        let usage = self.usage.clone();
        self.store.record_run(&NewRun {
            bot_name,
            title,
            millis: self.started.elapsed().as_millis() as u64,
            cost_usd: usage.as_ref().map_or(0.0, |u| u.cost_usd),
            tokens: usage.as_ref().map_or(0, |u| u.tokens),
            steps: usage.as_ref().map_or(0, |u| u.steps),
            tools: self.tool_uses,
            error: self.failed,
            models: usage.map(|u| u.models).unwrap_or_default(),
        })
    }

    fn flush_text(&mut self) -> Result<(), String> {
        if self.text.trim().is_empty() {
            self.text.clear();
            return Ok(());
        }
        let text = std::mem::take(&mut self.text);
        self.store.add_message(self.bot_id, MessageRole::Assistant, &text).map(|_| ())
    }
}

fn body_of(body: &impl serde::Serialize) -> String {
    serde_json::to_string(body).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use omb_wire::{ApprovalMode, ChatMessage, EngineId};
    use serde_json::json;

    fn record(events: &[RuntimeEvent]) -> (Store, i64, Vec<ChatMessage>) {
        let store = Store::in_memory().unwrap();
        let bot = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let mut recorder = TurnRecorder::new(&store, bot.id);
        for event in events {
            recorder.apply(event).unwrap();
        }
        let messages = store.messages(bot.id).unwrap();
        (store, bot.id, messages)
    }

    fn delta(text: &str) -> RuntimeEvent {
        RuntimeEvent::ContentDelta { text: text.into() }
    }

    fn done() -> RuntimeEvent {
        RuntimeEvent::TurnCompleted { is_error: false, result: None, usage: None }
    }

    #[test]
    fn deltas_become_one_assistant_message_and_the_session_is_kept() {
        let (store, bot, messages) = record(&[
            RuntimeEvent::TurnStarted { session_id: "s1".into(), model: None },
            delta("Hel"),
            delta("lo"),
            done(),
        ]);
        assert_eq!(messages.iter().map(|m| (m.role, m.body.as_str())).collect::<Vec<_>>(), [(MessageRole::Assistant, "Hello")]);
        assert_eq!(store.session(bot).unwrap().as_deref(), Some("s1"));
    }

    #[test]
    fn text_before_a_tool_call_is_saved_before_it_and_the_result_updates_the_call() {
        let (_, _, messages) = record(&[
            delta("Checking"),
            RuntimeEvent::ItemStarted { id: "t1".into(), name: "Bash".into(), input: json!({"command":"pwd"}) },
            RuntimeEvent::ItemCompleted { id: "t1".into(), ok: true, output: "/tmp".into() },
            delta("Done"),
            done(),
        ]);
        let roles: Vec<MessageRole> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [MessageRole::Assistant, MessageRole::Tool, MessageRole::Assistant]);
        let call: ToolCall = serde_json::from_str(&messages[1].body).unwrap();
        assert_eq!((call.name.as_str(), call.ok, call.output.as_str()), ("Bash", Some(true), "/tmp"));
    }

    #[test]
    fn failures_are_recorded_as_errors_after_any_partial_text() {
        let (_, _, messages) = record(&[delta("half"), RuntimeEvent::RuntimeError { message: "boom".into() }]);
        assert_eq!(messages.iter().map(|m| (m.role, m.body.as_str())).collect::<Vec<_>>(), [(MessageRole::Assistant, "half"), (MessageRole::Error, "boom")]);
        let (_, _, messages) = record(&[RuntimeEvent::TurnCompleted { is_error: true, result: Some("limit reached".into()), usage: None }]);
        assert_eq!((messages[0].role, messages[0].body.as_str()), (MessageRole::Error, "limit reached"));
    }

    #[test]
    fn a_request_is_saved_as_pending_and_then_marked_with_the_answer() {
        let opened = RuntimeEvent::RequestOpened { request_id: "r1".into(), tool: "Bash".into(), input: json!({"command":"ls"}) };
        let (store, bot, messages) = record(&[delta("Let me look"), opened.clone()]);
        assert_eq!(messages.iter().map(|m| m.role).collect::<Vec<_>>(), [MessageRole::Assistant, MessageRole::Approval]);
        let saved: ApprovalRequest = serde_json::from_str(&messages[1].body).unwrap();
        assert_eq!((saved.tool.as_str(), saved.state), ("Bash", ApprovalState::Pending));

        let mut recorder = TurnRecorder::new(&store, bot);
        recorder.apply(&opened).unwrap();
        recorder.apply(&RuntimeEvent::RequestResolved { request_id: "r1".into(), allowed: true }).unwrap();
        let last = store.messages(bot).unwrap().pop().unwrap();
        assert_eq!(serde_json::from_str::<ApprovalRequest>(&last.body).unwrap().state, ApprovalState::Allowed);
    }

    #[test]
    fn a_finished_turn_is_recorded_as_a_run_with_its_tool_count_and_failures() {
        let store = Store::in_memory().unwrap();
        let bot = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let usage = TurnUsage { cost_usd: 0.5, tokens: 10, steps: 3, models: vec![omb_wire::ModelCost { model: "m".into(), cost_usd: 0.5 }] };
        let mut recorder = TurnRecorder::new(&store, bot.id);
        recorder.apply(&RuntimeEvent::ItemStarted { id: "t".into(), name: "Bash".into(), input: json!({}) }).unwrap();
        recorder.apply(&RuntimeEvent::TurnCompleted { is_error: false, result: None, usage: Some(usage) }).unwrap();
        recorder.finish("  do   the\nthing ").unwrap();
        TurnRecorder::new(&store, bot.id).finish("ignored: nothing happened").unwrap();
        let mut failing = TurnRecorder::new(&store, bot.id);
        failing.apply(&RuntimeEvent::RuntimeError { message: "x".into() }).unwrap();
        failing.finish("broken").unwrap();
        let report = store.usage(omb_wire::UsagePeriod::All, 10).unwrap();
        assert_eq!(report.runs.len(), 2);
        assert_eq!((report.runs[1].bot.as_str(), report.runs[1].title.as_str(), report.runs[1].cost_usd, report.runs[1].error), ("A", "do the thing", 0.5, false));
        assert!(report.runs[0].error);
        assert_eq!((report.totals.tools, report.totals.steps, report.totals.errors), (1, 3, 1));
    }

    #[test]
    fn an_empty_turn_records_nothing() {
        let (_, _, messages) = record(&[done()]);
        assert!(messages.is_empty());
    }
}
