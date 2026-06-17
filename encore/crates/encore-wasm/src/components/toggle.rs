//! Toggle switch component.

use crate::dom;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement};

pub struct Toggle {
    pub element: Element,
}

impl Toggle {
    /// Create a toggle switch and return just the wrapper element.
    /// `on_change` is called with the new checked state on each click.
    pub fn create(
        id: &str,
        label: &str,
        is_on: bool,
        on_change: impl Fn(bool) + 'static,
    ) -> Element {
        Self::new(id, label, is_on, on_change).element
    }

    /// Create a toggle switch. Returns a Toggle holding the wrapper element.
    pub fn new(id: &str, label: &str, is_on: bool, on_change: impl Fn(bool) + 'static) -> Self {
        let wrap = dom::create_div();
        dom::set_class(&wrap, "toggle-wrap");

        if !label.is_empty() {
            let lbl = dom::el("span", "", Some(label));
            dom::append(&wrap, &lbl);
        }

        let toggle = dom::create_div();
        dom::set_class(&toggle, "toggle");

        let input = dom::create_el("input");
        dom::set_attr(&input, "type", "checkbox");
        input.set_id(id);
        if is_on {
            dom::set_attr(&input, "checked", "");
        }

        let track = dom::create_div();
        dom::set_class(&track, "toggle-track");

        let thumb = dom::create_div();
        dom::set_class(&thumb, "toggle-thumb");

        dom::append(&toggle, &input);
        dom::append(&toggle, &track);
        dom::append(&toggle, &thumb);
        dom::append(&wrap, &toggle);

        // Click handler — toggle the checkbox and notify
        let id_str = id.to_string();
        let on_change = Box::new(on_change);
        dom::on_click(&toggle, move || {
            if let Some(el) = dom::get_el(&id_str) {
                if let Some(input) = el.dyn_ref::<HtmlInputElement>() {
                    let new_val = !input.checked();
                    input.set_checked(new_val);
                    on_change(new_val);
                }
            }
        });

        Toggle { element: wrap }
    }
}
