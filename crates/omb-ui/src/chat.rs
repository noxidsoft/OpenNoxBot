use crate::api::{call, on_event};
use leptos::prelude::*;
use omb_wire::{ApprovalMode, ApprovalRequest, ApprovalState, Bot, BotEvent, ChatMessage, MessageRole, RuntimeEvent, ToolCall};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BotArg {
    bot_id: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs<'a> {
    bot_id: i64,
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnswerArgs<'a> {
    request_id: &'a str,
    allow: bool,
}

#[derive(Serialize)]
struct NameArg<'a> {
    name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalArg {
    bot_id: i64,
    approval: ApprovalMode,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelArg<'a> {
    bot_id: i64,
    model: Option<&'a str>,
}

const MODELS: [(&str, &str); 4] = [("", "Default"), ("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")];

const APPROVALS: [(ApprovalMode, &str, &str); 4] = [
    (ApprovalMode::Ask, "ask", "Ask for approval"),
    (ApprovalMode::Edits, "edits", "Auto-accept edits"),
    (ApprovalMode::Auto, "auto", "Approve for me"),
    (ApprovalMode::Full, "full", "Full access"),
];

fn load_messages(bot_id: i64, messages: RwSignal<Vec<ChatMessage>>, failure: RwSignal<Option<String>>) {
    leptos::task::spawn_local(async move {
        match call::<Vec<ChatMessage>>("list_messages", BotArg { bot_id }).await {
            Ok(found) => messages.set(found),
            Err(message) => failure.set(Some(message)),
        }
    });
}

fn summary(input: &Value) -> String {
    if let (Some(host), Some(command)) = (input["host"].as_str(), input["command"].as_str()) {
        return format!("{host} $ {command}").chars().take(120).collect();
    }
    let text = ["command", "file_path", "path", "pattern", "url", "query", "description"]
        .iter()
        .find_map(|key| input[*key].as_str().map(str::to_owned))
        .unwrap_or_else(|| input.to_string());
    text.chars().take(120).collect()
}

fn detail(input: &Value) -> String {
    if let (Some(host), Some(command)) = (input["host"].as_str(), input["command"].as_str()) {
        return format!("{host} $ {command}");
    }
    let text = input["command"].as_str().map(str::to_owned).unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default());
    text.chars().take(2000).collect()
}

#[component]
fn ApprovalCard(request: ApprovalRequest) -> impl IntoView {
    let failure = RwSignal::new(None::<String>);
    let id = request.request_id.clone();
    let answer = Callback::new(move |allow: bool| {
        let id = id.clone();
        leptos::task::spawn_local(async move {
            if let Err(message) = call::<()>("answer_request", AnswerArgs { request_id: &id, allow }).await {
                failure.set(Some(message));
            }
        });
    });
    let (state, label) = match request.state {
        ApprovalState::Pending => ("pending", ""),
        ApprovalState::Allowed => ("allowed", "Allowed"),
        ApprovalState::Denied => ("denied", "Denied"),
    };
    let (headline, body) = (summary(&request.input), detail(&request.input));
    let repeats = headline == body;
    view! {
        <div class="msg approval" data-state=state>
            <div class="ask">
                <strong>{request.tool.clone()}</strong>
                {(!repeats).then(|| view! { <code>{headline.clone()}</code> })}
            </div>
            <pre>{body}</pre>
            {(request.state == ApprovalState::Pending).then(|| view! {
                <div class="actions">
                    <button class="allow" on:click=move |_| answer.run(true)>"Allow"</button>
                    <button class="secondary" on:click=move |_| answer.run(false)>"Deny"</button>
                </div>
            })}
            {(!label.is_empty()).then(|| view! { <span class="status">{label}</span> })}
            {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
        </div>
    }
}

#[component]
fn MessageView(message: ChatMessage) -> impl IntoView {
    match message.role {
        MessageRole::User => view! { <div class="msg user">{message.body}</div> }.into_any(),
        MessageRole::Assistant => view! { <div class="msg assistant">{message.body}</div> }.into_any(),
        MessageRole::Error => view! { <div class="msg error">{message.body}</div> }.into_any(),
        MessageRole::Approval => match serde_json::from_str::<ApprovalRequest>(&message.body) {
            Ok(request) => view! { <ApprovalCard request=request /> }.into_any(),
            Err(_) => view! { <div class="msg error">"Unreadable approval request"</div> }.into_any(),
        },
        MessageRole::Tool => {
            let call = serde_json::from_str::<ToolCall>(&message.body).unwrap_or(ToolCall {
                name: "Tool".into(),
                input: Value::Null,
                ok: None,
                output: String::new(),
            });
            let (state, label) = match call.ok {
                None => ("running", "Running"),
                Some(true) => ("ok", "Done"),
                Some(false) => ("failed", "Failed"),
            };
            let output: String = call.output.chars().take(4000).collect();
            view! {
                <details class="msg tool" data-state=state>
                    <summary>
                        <strong>{call.name.clone()}</strong>
                        <code>{summary(&call.input)}</code>
                        <span>{label}</span>
                    </summary>
                    <pre>{output}</pre>
                </details>
            }
            .into_any()
        }
    }
}

#[component]
pub fn Chat(#[prop(into)] open_settings: Callback<()>) -> impl IntoView {
    let bots = RwSignal::new(Vec::<Bot>::new());
    let selected = RwSignal::new(None::<i64>);
    let messages = RwSignal::new(Vec::<ChatMessage>::new());
    let live = RwSignal::new(String::new());
    let running = RwSignal::new(HashSet::<i64>::new());
    let failure = RwSignal::new(None::<String>);
    let draft = RwSignal::new(String::new());
    let creating = RwSignal::new(false);
    let new_name = RwSignal::new(String::new());
    let confirm_delete = RwSignal::new(false);
    let scroller = NodeRef::<leptos::html::Div>::new();

    let current = Memo::new(move |_| selected.get().and_then(|id| bots.with(|all| all.iter().find(|b| b.id == id).cloned())));
    let waiting = Memo::new(move |_| {
        messages.with(|all| {
            all.last().is_some_and(|m| {
                m.role == MessageRole::Approval
                    && serde_json::from_str::<ApprovalRequest>(&m.body).is_ok_and(|r| r.state == ApprovalState::Pending)
            })
        })
    });
    let busy = Memo::new(move |_| selected.get().is_some_and(|id| running.with(|r| r.contains(&id))));

    let select = move |id: i64| {
        selected.set(Some(id));
        live.set(String::new());
        confirm_delete.set(false);
        failure.set(None);
        load_messages(id, messages, failure);
    };

    leptos::task::spawn_local(async move {
        match call::<Vec<Bot>>("list_bots", ()).await {
            Ok(found) => {
                let first = found.first().map(|b| b.id);
                bots.set(found);
                if let Some(id) = first {
                    select(id);
                }
            }
            Err(message) => failure.set(Some(message)),
        }
        if let Ok(ids) = call::<Vec<i64>>("running_bots", ()).await {
            running.set(ids.into_iter().collect());
        }
    });

    on_event::<BotEvent>(
        "runtime",
        move |incoming| {
        let is_selected = selected.get_untracked() == Some(incoming.bot_id);
        match &incoming.event {
            RuntimeEvent::ContentDelta { text } => {
                if is_selected {
                    live.update(|buffer| buffer.push_str(text));
                }
            }
            other => {
                if is_selected {
                    live.set(String::new());
                    load_messages(incoming.bot_id, messages, failure);
                }
                if matches!(other, RuntimeEvent::TurnCompleted { .. } | RuntimeEvent::RuntimeError { .. }) {
                    running.update(|ids| {
                        ids.remove(&incoming.bot_id);
                    });
                }
            }
        }
        },
        move |message| failure.set(Some(message)),
    );

    Effect::new(move |_| {
        messages.track();
        live.track();
        request_animation_frame(move || {
            if let Some(list) = scroller.get() {
                list.set_scroll_top(list.scroll_height());
            }
        });
    });

    let send = move || {
        let Some(bot_id) = selected.get_untracked() else { return };
        let text = draft.get_untracked();
        if text.trim().is_empty() || running.with_untracked(|r| r.contains(&bot_id)) {
            return;
        }
        draft.set(String::new());
        failure.set(None);
        running.update(|ids| {
            ids.insert(bot_id);
        });
        leptos::task::spawn_local(async move {
            match call::<ChatMessage>("send_message", SendArgs { bot_id, text: &text }).await {
                Ok(sent) => messages.update(|all| all.push(sent)),
                Err(message) => {
                    running.update(|ids| {
                        ids.remove(&bot_id);
                    });
                    failure.set(Some(message));
                    draft.set(text);
                }
            }
        });
    };

    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name = new_name.get_untracked();
        leptos::task::spawn_local(async move {
            match call::<Bot>("create_bot", NameArg { name: &name }).await {
                Ok(bot) => {
                    let id = bot.id;
                    new_name.set(String::new());
                    creating.set(false);
                    bots.update(|all| all.push(bot));
                    select(id);
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let remove = move |_| {
        let Some(bot_id) = selected.get_untracked() else { return };
        if !confirm_delete.get_untracked() {
            confirm_delete.set(true);
            return;
        }
        leptos::task::spawn_local(async move {
            match call::<()>("delete_bot", BotArg { bot_id }).await {
                Ok(()) => {
                    bots.update(|all| all.retain(|b| b.id != bot_id));
                    confirm_delete.set(false);
                    match bots.with_untracked(|all| all.first().map(|b| b.id)) {
                        Some(id) => select(id),
                        None => {
                            selected.set(None);
                            messages.set(Vec::new());
                        }
                    }
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let on_approval = move |ev: leptos::ev::Event| {
        let Some(bot_id) = selected.get_untracked() else { return };
        let value = event_target_value(&ev);
        let Some(approval) = APPROVALS.iter().find(|(_, key, _)| *key == value).map(|(mode, _, _)| *mode) else { return };
        leptos::task::spawn_local(async move {
            match call::<()>("set_bot_approval", ApprovalArg { bot_id, approval }).await {
                Ok(()) => bots.update(|all| {
                    if let Some(bot) = all.iter_mut().find(|b| b.id == bot_id) {
                        bot.approval = approval;
                    }
                }),
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let on_model = move |ev: leptos::ev::Event| {
        let Some(bot_id) = selected.get_untracked() else { return };
        let value = event_target_value(&ev);
        let model = (!value.is_empty()).then_some(value);
        leptos::task::spawn_local(async move {
            match call::<()>("set_bot_model", ModelArg { bot_id, model: model.as_deref() }).await {
                Ok(()) => bots.update(|all| {
                    if let Some(bot) = all.iter_mut().find(|b| b.id == bot_id) {
                        bot.model = model.clone();
                    }
                }),
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    view! {
        <div class="chat">
            <aside>
                <div class="side-head">
                    <h2>"Bots"</h2>
                    <button class="icon" aria-label="New bot" on:click=move |_| creating.update(|c| *c = !*c)>"+"</button>
                </div>
                <Show when=move || creating.get() || bots.with(|all| all.is_empty())>
                    <form class="new-bot" on:submit=create>
                        <input
                            placeholder="Bot name"
                            autocomplete="off"
                            prop:value=new_name
                            on:input=move |ev| new_name.set(event_target_value(&ev))
                        />
                    </form>
                </Show>
                <ul class="bot-list">
                    <For each=move || bots.get() key=|b| b.id children=move |bot| {
                        let id = bot.id;
                        view! {
                            <li>
                                <button class:active=move || selected.get() == Some(id) on:click=move |_| select(id)>
                                    <span>{bot.name.clone()}</span>
                                    <i class="dot" class:on=move || running.with(|r| r.contains(&id))></i>
                                </button>
                            </li>
                        }
                    } />
                </ul>
                <button class="settings-link" on:click=move |_| open_settings.run(())>"Settings"</button>
            </aside>
            <section class="conversation">
                {move || current.get().map(|bot| {
                    let model = bot.model.clone().unwrap_or_default();
                    let approval = bot.approval;
                    view! {
                        <header>
                            <h1>{bot.name.clone()}</h1>
                            <select aria-label="Model" on:change=on_model>
                                {MODELS.iter().map(|(value, label)| view! {
                                    <option value=*value selected={*value == model}>{*label}</option>
                                }).collect_view()}
                            </select>
                            <button class="secondary" class:danger=move || confirm_delete.get() on:click=remove>
                                {move || if confirm_delete.get() { "Confirm delete" } else { "Delete" }}
                            </button>
                        </header>
                        <div class="messages" node_ref=scroller>
                            <For each=move || messages.get() key=|m| (m.id, m.body.clone()) children=|m| view! { <MessageView message=m /> } />
                            {move || {
                                let text = live.get();
                                (!text.is_empty()).then(|| view! { <div class="msg assistant live">{text}</div> })
                            }}
                            {move || (busy.get() && !waiting.get() && live.with(|l| l.is_empty())).then(|| view! { <div class="thinking">"…"</div> })}
                        </div>
                        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
                        <form class="composer" on:submit=move |ev| { ev.prevent_default(); send(); }>
                            <select class="approval" aria-label="Approval" on:change=on_approval>
                                {APPROVALS.iter().map(|(mode, key, label)| view! {
                                    <option value=*key selected={*mode == approval}>{*label}</option>
                                }).collect_view()}
                            </select>
                            <textarea
                                rows="1"
                                placeholder=format!("Message {}", bot.name)
                                prop:value=draft
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                                        ev.prevent_default();
                                        send();
                                    }
                                }
                            ></textarea>
                            <button type="submit" aria-label="Send" disabled=move || busy.get() || draft.with(|d| d.trim().is_empty())>"↑"</button>
                        </form>
                    }
                })}
            </section>
        </div>
    }
}
