use leptos::prelude::*;

#[component]
fn App() -> impl IntoView {
    view! { <h1>"OpenNoxBot"</h1> }
}

fn main() {
    mount_to_body(App);
}
