//! Range slider component with label and value display.

use crate::dom;

pub struct Slider;

impl Slider {
    /// Create a labeled range slider. Returns the wrapper element.
    pub fn create(
        id: &str,
        label: &str,
        min: u32,
        max: u32,
        value: u32,
        suffix: &str,
        on_change: impl Fn(u32) + 'static,
    ) -> web_sys::Element {
        let wrap = dom::create_div();
        dom::set_class(&wrap, "mb-12");

        let header = dom::create_div();
        dom::set_class(&header, "flex justify-between mb-8");
        let lbl = dom::el("label", "", Some(label));
        let val = dom::el(
            "span",
            "stat-value text-sm",
            Some(&format!("{}{}", value, suffix)),
        );
        val.set_id(&format!("{}-val", id));
        dom::append(&header, &lbl);
        dom::append(&header, &val);
        dom::append(&wrap, &header);

        let input = dom::create_el("input");
        dom::set_attr(&input, "type", "range");
        dom::set_attr(&input, "min", &min.to_string());
        dom::set_attr(&input, "max", &max.to_string());
        dom::set_attr(&input, "value", &value.to_string());
        input.set_id(id);

        let id_str = id.to_string();
        let suffix_str = suffix.to_string();
        dom::on_input(&input, move |val_str| {
            if let Ok(v) = val_str.parse::<u32>() {
                if let Some(label) = dom::get_el(&format!("{}-val", id_str)) {
                    dom::set_text(&label, &format!("{}{}", v, suffix_str));
                }
                on_change(v);
            }
        });

        dom::append(&wrap, &input);
        wrap
    }
}
