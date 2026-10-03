//! Slide-over panels — gear menu items.

mod about;
mod config;
mod health;
mod theme;
mod update;

/// Render panel content into the panel body.
pub fn render(id: &str, container: &web_sys::Element) {
    match id {
        "config" => config::render(container),
        "health" => health::render(container),
        "crashes" => render_crashes(container),
        "logs" => crate::pages::logs::render(container),
        "update" => update::render(container),
        "reboot" => render_reboot(container),
        "about" => about::render(container),
        "theme" => theme::render(container),
        _ => {
            let msg = crate::dom::el("div", "text-muted", Some("Unknown panel"));
            crate::dom::append(container, &msg);
        }
    }
}

fn render_crashes(container: &web_sys::Element) {
    use crate::dom;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let list = dom::create_div();
    list.set_id("crash-list");
    dom::set_style(
        &list,
        "font-family",
        "'SF Mono', 'Cascadia Code', 'Consolas', monospace",
    );
    dom::set_style(&list, "font-size", "12px");
    dom::set_style(&list, "line-height", "1.6");
    dom::set_style(&list, "max-height", "60vh");
    dom::set_style(&list, "overflow-y", "auto");
    dom::set_style(&list, "padding", "12px");
    dom::set_style(&list, "background", "var(--bg-card)");
    dom::set_style(&list, "border-radius", "8px");

    let loading = dom::el("div", "text-muted", Some("$ fetching crash log..."));
    dom::append(&list, &loading);
    dom::append(container, &list);

    wasm_bindgen_futures::spawn_local(async {
        let window = dom::window();
        let origin = dom::api_origin();
        let url = format!("{}/api/crashes", origin);

        let result: Result<String, String> = async {
            let resp_val = JsFuture::from(window.fetch_with_str(&url))
                .await
                .map_err(|_| "could not reach the device".to_string())?;
            let resp: web_sys::Response = resp_val.unchecked_into();
            if !resp.ok() {
                return Err(format!("HTTP {}", resp.status()));
            }
            let text_promise = resp
                .text()
                .map_err(|_| "could not read the response".to_string())?;
            let text_val = JsFuture::from(text_promise)
                .await
                .map_err(|_| "could not read the response".to_string())?;
            text_val
                .as_string()
                .ok_or_else(|| "response not a string".into())
        }
        .await;

        if let Some(el) = dom::get_el("crash-list") {
            dom::clear(&el);
            match result {
                Err(err) => {
                    let msg = dom::el("div", "", Some(&format!("$ error: {}", err)));
                    dom::set_style(&msg, "color", "var(--red)");
                    dom::append(&el, &msg);
                }
                Ok(text) => {
                    match serde_json::from_str::<Vec<encore_common::protocol::CrashSummary>>(&text)
                    {
                        Ok(crashes) if crashes.is_empty() => {
                            let msg =
                                dom::el("div", "text-muted", Some("$ No crash reports recorded."));
                            dom::append(&el, &msg);
                            let msg2 = dom::el(
                                "div",
                                "text-muted",
                                Some("$ All subsystems running normally."),
                            );
                            dom::append(&el, &msg2);
                        }
                        Ok(crashes) => {
                            let header = dom::el(
                                "div",
                                "text-muted",
                                Some(&format!("$ {} crash report(s):", crashes.len())),
                            );
                            dom::append(&el, &header);
                            let sep = dom::el("div", "text-muted", Some("---"));
                            dom::append(&el, &sep);
                            for crash in &crashes {
                                let line = dom::create_div();
                                dom::set_style(&line, "margin-top", "8px");
                                let name =
                                    dom::el("span", "", Some(&format!("[{}] ", crash.subsystem)));
                                dom::set_style(&name, "color", "var(--red)");
                                dom::set_style(&name, "font-weight", "600");
                                dom::append(&line, &name);
                                let restarts = dom::el(
                                    "span",
                                    "text-muted",
                                    Some(&format!("(restart #{})", crash.restart_count)),
                                );
                                dom::append(&line, &restarts);
                                dom::append(&el, &line);

                                let msg = dom::el("div", "", Some(&format!("  {}", crash.message)));
                                dom::append(&el, &msg);

                                if !crash.backtrace.is_empty() {
                                    let bt = dom::el("pre", "text-muted", Some(&crash.backtrace));
                                    dom::set_style(&bt, "font-size", "10px");
                                    dom::set_style(&bt, "margin", "4px 0 0 16px");
                                    dom::set_style(&bt, "overflow-x", "auto");
                                    dom::append(&el, &bt);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = dom::el("div", "", Some(&format!("$ parse error: {}", e)));
                            dom::set_style(&msg, "color", "var(--red)");
                            dom::append(&el, &msg);
                            let raw = dom::el("pre", "text-muted", Some(&text));
                            dom::set_style(&raw, "font-size", "10px");
                            dom::set_style(&raw, "margin-top", "8px");
                            dom::append(&el, &raw);
                        }
                    }
                }
            }
        }
    });
}

fn render_reboot(container: &web_sys::Element) {
    use crate::dom;
    use wasm_bindgen_futures::JsFuture;

    let msg = dom::el(
        "div",
        "text-center mt-16",
        Some("Reboot the device? This will interrupt all audio."),
    );
    dom::set_style(&msg, "font-size", "16px");
    dom::append(container, &msg);

    let btns = dom::create_div();
    dom::set_class(&btns, "flex gap-12 mt-16");
    dom::set_style(&btns, "justify-content", "center");

    let cancel_btn = dom::el("button", "btn", Some("Cancel"));
    dom::on_click(&cancel_btn, || {
        dom::window().location().set_hash("settings/system").ok();
    });

    let confirm_btn = dom::el("button", "btn btn-danger", Some("Reboot Now"));
    dom::on_click(&confirm_btn, || {
        wasm_bindgen_futures::spawn_local(async {
            let window = dom::window();
            let origin = dom::api_origin();
            let url = format!("{}/api/reboot", origin);

            let opts = web_sys::RequestInit::new();
            opts.set_method("POST");
            if let Ok(req) = web_sys::Request::new_with_str_and_init(&url, &opts) {
                let _ = JsFuture::from(window.fetch_with_request(&req)).await;
            }

            if let Some(el) = dom::get_el("panel-body") {
                dom::clear(&el);
                let msg = dom::el(
                    "div",
                    "text-center mt-16",
                    Some("Rebooting... The device will be back shortly."),
                );
                dom::append(&el, &msg);
            }
        });
    });

    dom::append(&btns, &cancel_btn);
    dom::append(&btns, &confirm_btn);
    dom::append(container, &btns);

    render_factory_reset(container);
}

/// Factory-reset section, shown below the reboot controls. Erases the Encore
/// config (WiFi credentials + all settings) and reboots into setup mode. Guarded
/// by a confirm modal because, unlike a plain reboot, this loses the WiFi
/// connection and requires re-provisioning through the captive portal.
fn render_factory_reset(container: &web_sys::Element) {
    use crate::components::modal::Modal;
    use crate::dom;
    use wasm_bindgen_futures::JsFuture;

    let divider = dom::create_div();
    dom::set_style(&divider, "border-top", "1px solid var(--border)");
    dom::set_style(&divider, "margin", "28px 0 16px");
    dom::append(container, &divider);

    let heading = dom::el("div", "card-title text-center", Some("Factory Reset"));
    dom::append(container, &heading);

    let msg = dom::el(
        "div",
        "text-center text-muted",
        Some(
            "Erase WiFi credentials and all settings, then reboot into setup mode. \
             You will need to reconnect the speaker to WiFi afterwards.",
        ),
    );
    dom::set_style(&msg, "line-height", "1.5");
    dom::set_style(&msg, "margin-bottom", "16px");
    dom::append(container, &msg);

    let btn_row = dom::create_div();
    dom::set_class(&btn_row, "flex mt-16");
    dom::set_style(&btn_row, "justify-content", "center");

    let reset_btn = dom::el("button", "btn btn-danger", Some("Factory Reset\u{2026}"));
    dom::on_click(&reset_btn, || {
        Modal::confirm(
            "This erases WiFi and all settings and reboots into setup mode. Continue?",
            "Erase & Reset",
            || {
                wasm_bindgen_futures::spawn_local(async {
                    let window = dom::window();
                    let origin = dom::api_origin();
                    let url = format!("{}/api/factory-reset", origin);

                    let opts = web_sys::RequestInit::new();
                    opts.set_method("POST");
                    if let Ok(req) = web_sys::Request::new_with_str_and_init(&url, &opts) {
                        let _ = JsFuture::from(window.fetch_with_request(&req)).await;
                    }

                    if let Some(el) = dom::get_el("panel-body") {
                        dom::clear(&el);
                        let done = dom::el(
                            "div",
                            "text-center mt-16",
                            Some(
                                "Resetting\u{2026} The speaker will reboot into setup mode. \
                                 Reconnect to its WiFi network to set it up again.",
                            ),
                        );
                        dom::set_style(&done, "line-height", "1.5");
                        dom::append(&el, &done);
                    }
                });
            },
            || {},
        );
    });

    dom::append(&btn_row, &reset_btn);
    dom::append(container, &btn_row);
}
