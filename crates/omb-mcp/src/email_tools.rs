use crate::ssh_tools::Reply;
use crate::{Shared, TurnContext};
use omb_core::approvals::Decision;
use serde_json::{Value, json};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(60);

pub use omb_email::definitions;

pub fn owns(name: &str) -> bool {
    name.starts_with("email_")
}

fn failure(text: impl Into<String>) -> Reply {
    Reply { text: text.into(), is_error: true }
}

pub async fn run(shared: &Shared, turn: &TurnContext, name: &str, args: &Value) -> Reply {
    if !definitions().iter().any(|tool| tool["name"] == name) {
        return failure("unknown tool");
    }
    if name != "email_draft" {
        turn.mail_seen.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let draft = if name == "email_draft" {
        match omb_email::parse_draft(args) {
            Ok(draft) => Some(draft),
            Err(message) => return failure(message),
        }
    } else {
        None
    };
    if let Some(draft) = &draft {
        let shown = json!({ "to": draft.to, "cc": draft.cc, "bcc": draft.bcc, "subject": draft.subject, "text": draft.text });
        if shared.approvals.ask(&turn.events, "Email draft", &shown).await != Decision::Allow {
            return failure("The person declined to save this draft. Ask what they would like instead.");
        }
    }

    let (open, name, args) = (shared.mailbox.clone(), name.to_owned(), args.clone());
    let work = tokio::task::spawn_blocking(move || {
        let (mut mailbox, from) = open()?;
        match &draft {
            Some(draft) => omb_email::create_draft(mailbox.as_mut(), &from, draft),
            None => omb_email::call(mailbox.as_mut(), &name, &args),
        }
    });
    match tokio::time::timeout(LIMIT, work).await {
        Ok(Ok(Ok(value))) => Reply { text: serde_json::to_string_pretty(&value).unwrap_or_default(), is_error: false },
        Ok(Ok(Err(message))) => failure(message),
        Ok(Err(_)) => failure("The mailbox request failed"),
        Err(_) => failure("The mail server did not answer within 60 seconds"),
    }
}
