//! Text field component — labeled text input.

use crate::dom;

pub struct TextField;

impl TextField {
    /// Create a labeled text input. Returns the wrapper element.
    pub fn create(id: &str, label: &str, value: &str, placeholder: &str) -> web_sys::Element {
        Self::build(id, label, value, placeholder, "text")
    }

    /// Create a labeled, masked input for secrets (passwords, keys).
    pub fn password(id: &str, label: &str, value: &str, placeholder: &str) -> web_sys::Element {
        Self::build(id, label, value, placeholder, "password")
    }

    fn build(
        id: &str,
        label: &str,
        value: &str,
        placeholder: &str,
        input_type: &str,
    ) -> web_sys::Element {
        let wrap = dom::create_div();
        dom::set_class(&wrap, "field");
        let lbl = dom::el("label", "", Some(label));
        dom::append(&wrap, &lbl);
        let input = dom::create_el("input");
        dom::set_attr(&input, "type", input_type);
        dom::set_attr(&input, "value", value);
        if input_type == "password" {
            dom::set_attr(&input, "autocomplete", "off");
        }
        if !placeholder.is_empty() {
            dom::set_attr(&input, "placeholder", placeholder);
        }
        input.set_id(id);
        dom::append(&wrap, &input);
        wrap
    }
}
