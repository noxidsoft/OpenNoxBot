use crate::api::call;
use leptos::prelude::*;
use omb_wire::{SpendLimits, UsagePeriod, UsageReport, UsageRow};
use serde::Serialize;
use wasm_bindgen::JsValue;

#[derive(Serialize)]
struct Period {
    period: UsagePeriod,
}

#[derive(Serialize)]
struct Limits {
    limits: SpendLimits,
}

fn amount(text: &str) -> Result<Option<f64>, String> {
    match text.trim().trim_start_matches('$') {
        "" => Ok(None),
        number => number.parse::<f64>().map(Some).map_err(|_| "Enter an amount like 5 or 2.50".to_owned()),
    }
}

const PERIODS: [(UsagePeriod, &str, &str); 4] = [
    (UsagePeriod::Day, "day", "Last 24 hours"),
    (UsagePeriod::Week, "week", "Last 7 days"),
    (UsagePeriod::Month, "month", "Last 30 days"),
    (UsagePeriod::All, "all", "All time"),
];

fn money(amount: f64) -> String {
    if amount == 0.0 {
        "$0".into()
    } else if amount < 0.001 {
        "<$0.001".into()
    } else if amount < 0.1 {
        format!("${amount:.3}")
    } else {
        format!("${amount:.2}")
    }
}

fn duration(millis: u64) -> String {
    let seconds = millis as f64 / 1000.0;
    if seconds < 60.0 { format!("{seconds:.1}s") } else { format!("{}m {}s", millis / 60_000, millis / 1000 % 60) }
}

fn count(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}K", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

fn when(at: i64) -> String {
    js_sys::Date::new(&JsValue::from_f64(at as f64 * 1000.0)).to_locale_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default()
}

fn bars(title: &'static str, rows: Vec<UsageRow>) -> impl IntoView {
    let top = rows.iter().map(|r| r.cost_usd).fold(0.0, f64::max);
    view! {
        <div class="bars">
            <h3>{title}</h3>
            {rows.into_iter().map(|row| {
                let width = if top > 0.0 { row.cost_usd / top * 100.0 } else { 0.0 };
                view! {
                    <div class="bar-row">
                        <span>{row.label}</span>
                        <i style=format!("width:{width:.0}%")></i>
                        <b>{money(row.cost_usd)}</b>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
pub fn Usage() -> impl IntoView {
    let report = RwSignal::new(UsageReport::default());
    let failure = RwSignal::new(None::<String>);
    let period = RwSignal::new(UsagePeriod::Week);

    Effect::new(move |_| {
        let chosen = period.get();
        leptos::task::spawn_local(async move {
            match call::<UsageReport>("usage_report", Period { period: chosen }).await {
                Ok(found) => {
                    failure.set(None);
                    report.set(found);
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    });

    let (day_cap, month_cap) = (RwSignal::new(String::new()), RwSignal::new(String::new()));
    let saved = RwSignal::new(false);
    leptos::task::spawn_local(async move {
        if let Ok(found) = call::<SpendLimits>("spend_limits", ()).await {
            day_cap.set(found.day.map(|d| d.to_string()).unwrap_or_default());
            month_cap.set(found.month.map(|m| m.to_string()).unwrap_or_default());
        }
    });
    let save_limits = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let parsed = amount(&day_cap.get_untracked()).and_then(|day| Ok(SpendLimits { day, month: amount(&month_cap.get_untracked())? }));
        leptos::task::spawn_local(async move {
            let result = match parsed {
                Ok(limits) => call::<()>("set_spend_limits", Limits { limits }).await,
                Err(message) => Err(message),
            };
            match result {
                Ok(()) => {
                    failure.set(None);
                    saved.set(true);
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let change = move |ev: leptos::ev::Event| {
        let key = event_target_value(&ev);
        if let Some((chosen, _, _)) = PERIODS.iter().find(|(_, k, _)| *k == key) {
            period.set(*chosen);
        }
    };

    view! {
        <div class="usage-head">
            <h1>"Usage"</h1>
            <select aria-label="Period" on:change=change>
                {PERIODS.iter().map(|(p, key, label)| view! { <option value=*key selected={*p == UsagePeriod::Week}>{*label}</option> }).collect_view()}
            </select>
        </div>
        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
        {move || {
            let totals = report.get().totals;
            view! {
                <div class="tiles">
                    <div><small>"TIME"</small><strong>{duration(totals.millis)}</strong></div>
                    <div><small>"COST"</small><strong>{money(totals.cost_usd)}</strong></div>
                    <div><small>"TOKENS"</small><strong>{count(totals.tokens)}</strong></div>
                    <div><small>"AI STEPS"</small><strong>{count(totals.steps)}</strong></div>
                    <div><small>"TOOL USES"</small><strong>{count(totals.tools)}</strong></div>
                    <div><small>"ERRORS"</small><strong>{count(totals.errors)}</strong></div>
                </div>
            }
        }}
        {move || {
            let current = report.get();
            view! { <div class="where">{bars("Bot", current.by_bot)}{bars("Model", current.by_model)}</div> }
        }}
        <h2>"Spend limits"</h2>
        <form class="limits" on:submit=save_limits>
            <input aria-label="24-hour limit" placeholder="24 hours: no limit" inputmode="decimal" autocomplete="off" prop:value=day_cap on:input=move |ev| { saved.set(false); day_cap.set(event_target_value(&ev)) } />
            <input aria-label="30-day limit" placeholder="30 days: no limit" inputmode="decimal" autocomplete="off" prop:value=month_cap on:input=move |ev| { saved.set(false); month_cap.set(event_target_value(&ev)) } />
            <button type="submit">{move || if saved.get() { "Saved" } else { "Save" }}</button>
        </form>
        <h2>"Runs"</h2>
        <ul class="runs">
            <For
                each=move || report.get().runs
                key=|run| run.id
                children=|run| view! {
                    <li data-error=run.error.to_string()>
                        <div><strong>{run.title.clone()}</strong><small>{format!("{} · {}", when(run.at), run.bot)}</small></div>
                        <span>{duration(run.millis)}</span>
                        <b>{money(run.cost_usd)}</b>
                    </li>
                }
            />
        </ul>
    }
}
