use crate::api::call;
use leptos::prelude::*;
use omb_wire::{HostPolicy, HostStatus, NewHost, SshLogEntry, SshOutcome};
use serde::Serialize;
use wasm_bindgen::JsValue;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostId {
    host_id: i64,
}

#[derive(Serialize)]
struct AddArgs<'a> {
    host: &'a NewHost,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyArgs {
    host_id: i64,
    policy: HostPolicy,
}

#[derive(Serialize)]
struct Limit {
    limit: u32,
}

const POLICIES: [(HostPolicy, &str, &str); 3] = [
    (HostPolicy::ReadOnly, "read-only", "Read-only"),
    (HostPolicy::Ask, "ask", "Ask"),
    (HostPolicy::Full, "full", "Full access"),
];

fn policy_of(key: &str) -> HostPolicy {
    POLICIES.iter().find(|(_, k, _)| *k == key).map_or(HostPolicy::Ask, |(policy, _, _)| *policy)
}

fn destination(status: &HostStatus) -> String {
    let host = &status.host;
    let user = host.user.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
    let port = host.port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("{user}{}{port}", host.target)
}

fn when(at: i64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(at as f64 * 1000.0));
    date.to_locale_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default()
}

fn outcome(entry: &SshLogEntry) -> &'static str {
    match entry.outcome {
        SshOutcome::Ran => "Ran",
        SshOutcome::Declined => "Declined",
        SshOutcome::Refused => "Refused",
        SshOutcome::Failed => "Failed",
    }
}

#[component]
pub fn Hosts() -> impl IntoView {
    let hosts = RwSignal::new(Vec::<HostStatus>::new());
    let activity = RwSignal::new(Vec::<SshLogEntry>::new());
    let failure = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);
    let scan = RwSignal::new(None::<(i64, String, Vec<String>)>);
    let (name, target, user, port) = (RwSignal::new(String::new()), RwSignal::new(String::new()), RwSignal::new(String::new()), RwSignal::new(String::new()));
    let policy = RwSignal::new("ask".to_owned());

    let refresh = move || {
        leptos::task::spawn_local(async move {
            match call::<Vec<HostStatus>>("list_hosts", ()).await {
                Ok(found) => hosts.set(found),
                Err(message) => failure.set(Some(message)),
            }
            if let Ok(found) = call::<Vec<SshLogEntry>>("ssh_activity", Limit { limit: 20 }).await {
                activity.set(found);
            }
        });
    };
    refresh();

    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let port_text = port.get_untracked();
        let port_number = match port_text.trim() {
            "" => None,
            text => match text.parse::<u16>() {
                Ok(number) => Some(number),
                Err(_) => return failure.set(Some("Enter a port from 1 to 65535".into())),
            },
        };
        let user_text = user.get_untracked();
        let new = NewHost {
            name: name.get_untracked().trim().to_owned(),
            target: target.get_untracked().trim().to_owned(),
            user: Some(user_text.trim().to_owned()).filter(|u| !u.is_empty()),
            port: port_number,
            policy: policy_of(&policy.get_untracked()),
        };
        leptos::task::spawn_local(async move {
            match call::<omb_wire::SshHost>("add_host", AddArgs { host: &new }).await {
                Ok(_) => {
                    for field in [name, target, user, port] {
                        field.set(String::new());
                    }
                    failure.set(None);
                    refresh();
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let change_policy = move |host_id: i64, key: String| {
        leptos::task::spawn_local(async move {
            match call::<()>("set_host_policy", PolicyArgs { host_id, policy: policy_of(&key) }).await {
                Ok(()) => refresh(),
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let remove = move |host_id: i64| {
        leptos::task::spawn_local(async move {
            match call::<()>("remove_host", HostId { host_id }).await {
                Ok(()) => refresh(),
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let start_trust = move |host_id: i64, host_name: String| {
        leptos::task::spawn_local(async move {
            notice.set(None);
            match call::<Vec<String>>("scan_host", HostId { host_id }).await {
                Ok(prints) => scan.set(Some((host_id, host_name, prints))),
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let confirm_trust = move |_| {
        let Some((host_id, _, _)) = scan.get_untracked() else { return };
        leptos::task::spawn_local(async move {
            match call::<()>("trust_host", HostId { host_id }).await {
                Ok(()) => {
                    scan.set(None);
                    failure.set(None);
                    refresh();
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let test = move |host_id: i64, host_name: String| {
        leptos::task::spawn_local(async move {
            notice.set(Some(format!("Testing {host_name}…")));
            match call::<String>("test_host", HostId { host_id }).await {
                Ok(said) => notice.set(Some(format!("{host_name}: {said}"))),
                Err(message) => notice.set(Some(format!("{host_name}: {message}"))),
            }
            refresh();
        });
    };

    view! {
        <h1>"Hosts"</h1>
        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
        <ul class="hosts">
            <For
                each=move || hosts.get()
                key=|status| (status.host.id, status.trusted, format!("{:?}", status.host.policy))
                children=move |status| {
                    let (id, host_name, trusted, current) = (status.host.id, status.host.name.clone(), status.trusted, status.host.policy);
                    let (for_scan, for_test) = (host_name.clone(), host_name.clone());
                    view! {
                        <li>
                            <div class="host-main">
                                <strong>{host_name.clone()}</strong>
                                <small>{destination(&status)}</small>
                            </div>
                            <select aria-label="Policy" on:change=move |ev| change_policy(id, event_target_value(&ev))>
                                {POLICIES.iter().map(|(mode, key, label)| view! {
                                    <option value=*key selected={*mode == current}>{*label}</option>
                                }).collect_view()}
                            </select>
                            <span class="trust" data-trusted=trusted.to_string()>{if trusted { "Trusted" } else { "Not trusted" }}</span>
                            <div class="host-actions">
                                {if trusted {
                                    view! { <button class="secondary" on:click=move |_| test(id, for_test.clone())>"Test"</button> }.into_any()
                                } else {
                                    view! { <button class="secondary" on:click=move |_| start_trust(id, for_scan.clone())>"Trust key"</button> }.into_any()
                                }}
                                <button class="secondary" on:click=move |_| remove(id)>"Remove"</button>
                            </div>
                        </li>
                    }
                }
            />
        </ul>
        {move || scan.get().map(|(_, host_name, prints)| view! {
            <div class="scan">
                <p>{format!("{host_name} offered these keys. Check them against the server before trusting.")}</p>
                <ul>{prints.into_iter().map(|print| view! { <li><code>{print}</code></li> }).collect_view()}</ul>
                <div class="host-actions">
                    <button class="allow" on:click=confirm_trust>"Trust these keys"</button>
                    <button class="secondary" on:click=move |_| scan.set(None)>"Cancel"</button>
                </div>
            </div>
        })}
        {move || notice.get().map(|message| view! { <p class="notice">{message}</p> })}

        <h2>"Add a host"</h2>
        <form class="add-host" on:submit=add>
            <input placeholder="Name" autocomplete="off" prop:value=name on:input=move |ev| name.set(event_target_value(&ev)) />
            <input placeholder="Hostname, address or ssh alias" autocomplete="off" prop:value=target on:input=move |ev| target.set(event_target_value(&ev)) />
            <input placeholder="User" autocomplete="off" prop:value=user on:input=move |ev| user.set(event_target_value(&ev)) />
            <input placeholder="Port" inputmode="numeric" autocomplete="off" prop:value=port on:input=move |ev| port.set(event_target_value(&ev)) />
            <select aria-label="Policy" on:change=move |ev| policy.set(event_target_value(&ev))>
                {POLICIES.iter().map(|(_, key, label)| view! { <option value=*key selected={*key == "ask"}>{*label}</option> }).collect_view()}
            </select>
            <button type="submit" disabled=move || name.with(|n| n.trim().is_empty()) || target.with(|t| t.trim().is_empty())>"Add"</button>
        </form>

        <h2>"Activity"</h2>
        <ul class="activity">
            <For each=move || activity.get() key=|entry| entry.id children=|entry| view! {
                <li data-outcome=outcome(&entry)>
                    <span>{when(entry.at)}</span>
                    <strong>{entry.host.clone()}</strong>
                    <code>{entry.command.clone()}</code>
                    <em>{outcome(&entry)}</em>
                </li>
            } />
        </ul>
    }
}
