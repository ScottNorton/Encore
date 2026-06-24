//! Dashboard pages — 8 tabs + connect screen.

pub mod assistant;
pub mod audio;
pub mod bluetooth;
pub mod connect;
pub mod dashboard;
pub mod home;
pub mod lights;
pub mod logs;
pub mod network;
pub mod settings;
pub mod setup;
pub mod speakers;
pub mod spotify;

/// Render a page by route ID into the given container.
pub fn render(page: &str, container: &web_sys::Element) {
    match page {
        "dashboard" => dashboard::render(container),
        "assistant" => assistant::render(container),
        // Standalone Spotify route folds into Stage (the rings live on Stage now).
        "spotify" => home::render(container),
        "audio" => audio::render(container),
        "lights" => lights::render(container),
        "bluetooth" => bluetooth::render(container),
        "network" => network::render(container),
        "speakers" => speakers::render(container),
        "setup" => setup::render(container),
        "connect" => connect::render(container),
        _ => {
            let msg = crate::dom::el("div", "text-muted text-center", Some("Page not found"));
            crate::dom::append(container, &msg);
        }
    }
}

/// Notify the active page to update its DOM.
pub fn update(page: &str) {
    match page {
        "dashboard" => dashboard::update(),
        "assistant" => assistant::update(),
        "spotify" => home::update(),
        "audio" => audio::update(),
        "lights" => lights::update(),
        "bluetooth" => bluetooth::update(),
        "network" => network::update(),
        "speakers" => speakers::update(),
        "setup" => setup::update(),
        "connect" => connect::update(),
        _ => {}
    }
}
