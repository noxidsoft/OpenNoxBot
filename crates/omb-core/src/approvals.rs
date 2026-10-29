use crate::ids::random_hex;
use omb_wire::RuntimeEvent;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// Requests waiting for the person, each answered once.
#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
}

impl Approvals {
    fn take(&self, request_id: &str) -> Option<oneshot::Sender<Decision>> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(request_id)
    }

    /// False when the request is unknown, already answered, or its asker is gone.
    pub fn answer(&self, request_id: &str, decision: Decision) -> bool {
        self.take(request_id).is_some_and(|sender| sender.send(decision).is_ok())
    }

    /// Opens a request on the turn's event stream and waits for the answer. A request whose asker goes away is denied.
    pub async fn ask(&self, events: &UnboundedSender<RuntimeEvent>, tool: &str, input: &Value) -> Decision {
        let request_id = random_hex(16);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(request_id.clone(), sender);
        let mut abandoned = Abandoned { approvals: self, request_id: &request_id, events, armed: true };
        let _ = events.send(RuntimeEvent::RequestOpened { request_id: request_id.clone(), tool: tool.to_owned(), input: input.clone() });
        let decision = receiver.await.unwrap_or(Decision::Deny);
        abandoned.armed = false;
        let _ = events.send(RuntimeEvent::RequestResolved { request_id: request_id.clone(), allowed: decision == Decision::Allow });
        decision
    }
}

struct Abandoned<'a> {
    approvals: &'a Approvals,
    request_id: &'a str,
    events: &'a UnboundedSender<RuntimeEvent>,
    armed: bool,
}

impl Drop for Abandoned<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.approvals.take(self.request_id);
            let _ = self.events.send(RuntimeEvent::RequestResolved { request_id: self.request_id.to_owned(), allowed: false });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc::unbounded_channel;

    async fn opened_id(rx: &mut tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>) -> String {
        match rx.recv().await {
            Some(RuntimeEvent::RequestOpened { request_id, tool, input }) => {
                assert_eq!((tool.as_str(), input), ("Bash", json!({"command": "ls"})));
                request_id
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_answer_reaches_the_asker_and_is_announced() {
        for (decision, allowed) in [(Decision::Allow, true), (Decision::Deny, false)] {
            let approvals = Arc::new(Approvals::default());
            let (tx, mut rx) = unbounded_channel();
            let asking = tokio::spawn({
                let approvals = approvals.clone();
                async move { approvals.ask(&tx, "Bash", &json!({"command": "ls"})).await }
            });
            let id = opened_id(&mut rx).await;
            assert!(approvals.answer(&id, decision));
            assert_eq!(asking.await.unwrap(), decision);
            assert_eq!(rx.recv().await, Some(RuntimeEvent::RequestResolved { request_id: id.clone(), allowed }));
            assert!(!approvals.answer(&id, decision), "answered once");
        }
    }

    #[tokio::test]
    async fn a_request_whose_asker_goes_away_is_denied_and_cleared() {
        let approvals = Arc::new(Approvals::default());
        let (tx, mut rx) = unbounded_channel();
        let asking = tokio::spawn({
            let approvals = approvals.clone();
            async move { approvals.ask(&tx, "Bash", &json!({"command": "ls"})).await }
        });
        let id = opened_id(&mut rx).await;
        asking.abort();
        let _ = asking.await;
        assert_eq!(rx.recv().await, Some(RuntimeEvent::RequestResolved { request_id: id.clone(), allowed: false }));
        assert!(!approvals.answer(&id, Decision::Allow));
    }

    #[test]
    fn an_unknown_request_cannot_be_answered() {
        assert!(!Approvals::default().answer("nope", Decision::Allow));
    }
}
