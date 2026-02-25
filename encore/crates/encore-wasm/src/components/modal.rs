//! Confirmation dialog modal.

use crate::dom;

pub struct Modal;

impl Modal {
    /// Show a centered confirmation modal with message and confirm/cancel buttons.
    /// Returns immediately — callbacks handle user response.
    pub fn confirm(
        message: &str,
        confirm_label: &str,
        on_confirm: impl Fn() + 'static,
        on_cancel: impl Fn() + 'static,
    ) {
        let backdrop = dom::create_div();
        backdrop.set_id("modal-backdrop");
        dom::set_style(&backdrop, "position", "fixed");
        dom::set_style(&backdrop, "inset", "0");
        dom::set_style(&backdrop, "background", "rgba(0,0,0,0.6)");
        dom::set_style(&backdrop, "z-index", "200");
        dom::set_style(&backdrop, "display", "flex");
        dom::set_style(&backdrop, "align-items", "center");
        dom::set_style(&backdrop, "justify-content", "center");

        let dialog = dom::create_div();
        dom::set_class(&dialog, "card");
        dom::set_style(&dialog, "max-width", "360px");
        dom::set_style(&dialog, "width", "90%");

        let msg = dom::el("div", "text-center", Some(message));
        dom::set_style(&msg, "margin-bottom", "20px");
        dom::append(&dialog, &msg);

        let btns = dom::create_div();
        dom::set_class(&btns, "flex gap-12");
        dom::set_style(&btns, "justify-content", "center");

        let cancel_btn = dom::el("button", "btn", Some("Cancel"));
        dom::on_click(&cancel_btn, move || {
            on_cancel();
            Self::close();
        });

        let confirm_btn = dom::el("button", "btn btn-danger", Some(confirm_label));
        dom::on_click(&confirm_btn, move || {
            on_confirm();
            Self::close();
        });

        dom::append(&btns, &cancel_btn);
        dom::append(&btns, &confirm_btn);
        dom::append(&dialog, &btns);
        dom::append(&backdrop, &dialog);

        dom::body().append_child(&backdrop).unwrap();
    }

    /// Show a modal with a password input field.
    /// `on_submit` receives the input value when "Join" is clicked or Enter pressed.
    pub fn input(
        title: &str,
        input_label: &str,
        submit_label: &str,
        on_submit: impl Fn(String) + 'static,
        on_cancel: impl Fn() + 'static,
    ) {
        use wasm_bindgen::JsCast;

        let backdrop = dom::create_div();
        backdrop.set_id("modal-backdrop");
        dom::set_style(&backdrop, "position", "fixed");
        dom::set_style(&backdrop, "inset", "0");
        dom::set_style(&backdrop, "background", "rgba(0,0,0,0.6)");
        dom::set_style(&backdrop, "z-index", "200");
        dom::set_style(&backdrop, "display", "flex");
        dom::set_style(&backdrop, "align-items", "center");
        dom::set_style(&backdrop, "justify-content", "center");

        let dialog = dom::create_div();
        dom::set_class(&dialog, "card");
        dom::set_style(&dialog, "max-width", "360px");
        dom::set_style(&dialog, "width", "90%");

        let title_el = dom::el("div", "card-title", Some(title));
        dom::append(&dialog, &title_el);

        let label_el = dom::el("label", "", Some(input_label));
        dom::append(&dialog, &label_el);

        let input = dom::create_el("input");
        dom::set_attr(&input, "type", "password");
        dom::set_attr(&input, "autocomplete", "off");
        dom::set_style(&input, "margin-bottom", "16px");
        input.set_id("modal-input");
        dom::append(&dialog, &input);

        let btns = dom::create_div();
        dom::set_class(&btns, "flex gap-12");
        dom::set_style(&btns, "justify-content", "center");

        let cancel_btn = dom::el("button", "btn", Some("Cancel"));
        dom::on_click(&cancel_btn, move || {
            on_cancel();
            Self::close();
        });

        let on_submit = std::rc::Rc::new(on_submit);
        let on_submit_btn = on_submit.clone();
        let submit_btn = dom::el("button", "btn btn-primary", Some(submit_label));
        dom::on_click(&submit_btn, move || {
            let val = dom::get_el("modal-input")
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|i| i.value())
                .unwrap_or_default();
            on_submit_btn(val);
            Self::close();
        });

        // Enter key submits
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            if e.key() == "Enter" {
                let val = dom::get_el("modal-input")
                    .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
                    .map(|i| i.value())
                    .unwrap_or_default();
                on_submit(val);
                Self::close();
            }
        }) as Box<dyn FnMut(_)>);
        input.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref()).ok();
        cb.forget();

        dom::append(&btns, &cancel_btn);
        dom::append(&btns, &submit_btn);
        dom::append(&dialog, &btns);
        dom::append(&backdrop, &dialog);

        dom::body().append_child(&backdrop).unwrap();

        // Auto-focus the input
        if let Some(el) = dom::get_el("modal-input") {
            if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
                html.focus().ok();
            }
        }
    }

    /// Show a single-button informational alert modal.
    pub fn alert(title: &str, message: &str, button_label: &str) {
        let backdrop = dom::create_div();
        backdrop.set_id("modal-backdrop");
        dom::set_style(&backdrop, "position", "fixed");
        dom::set_style(&backdrop, "inset", "0");
        dom::set_style(&backdrop, "background", "rgba(0,0,0,0.6)");
        dom::set_style(&backdrop, "z-index", "200");
        dom::set_style(&backdrop, "display", "flex");
        dom::set_style(&backdrop, "align-items", "center");
        dom::set_style(&backdrop, "justify-content", "center");

        let dialog = dom::create_div();
        dom::set_class(&dialog, "card");
        dom::set_style(&dialog, "max-width", "360px");
        dom::set_style(&dialog, "width", "90%");

        let title_el = dom::el("div", "card-title", Some(title));
        dom::set_style(&title_el, "color", "#FFA030");
        dom::append(&dialog, &title_el);

        let msg = dom::el("div", "", Some(message));
        dom::set_style(&msg, "margin-bottom", "20px");
        dom::set_style(&msg, "line-height", "1.5");
        dom::append(&dialog, &msg);

        let btns = dom::create_div();
        dom::set_style(&btns, "text-align", "center");

        let ok_btn = dom::el("button", "btn btn-primary", Some(button_label));
        dom::on_click(&ok_btn, || {
            Self::close();
        });

        dom::append(&btns, &ok_btn);
        dom::append(&dialog, &btns);
        dom::append(&backdrop, &dialog);

        dom::body().append_child(&backdrop).unwrap();
    }

    fn close() {
        if let Some(el) = dom::get_el("modal-backdrop") {
            el.remove();
        }
    }
}
