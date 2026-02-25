//! Stat row component — label/value display row.

use crate::dom;

/// Append a label/value stat row to `parent`.
pub fn stat_row(parent: &web_sys::Element, label: &str, value: &str) {
    let row = dom::create_div();
    dom::set_class(&row, "stat-row");
    let lbl = dom::el("span", "stat-label", Some(label));
    let val = dom::el("span", "stat-value", Some(value));
    dom::append(&row, &lbl);
    dom::append(&row, &val);
    dom::append(parent, &row);
}

/// Append a label/value stat row with an ID on the value element.
pub fn stat_row_with_id(parent: &web_sys::Element, label: &str, value: &str, id: &str) {
    let row = dom::create_div();
    dom::set_class(&row, "stat-row");
    let lbl = dom::el("span", "stat-label", Some(label));
    let val = dom::el("span", "stat-value", Some(value));
    val.set_id(id);
    dom::append(&row, &lbl);
    dom::append(&row, &val);
    dom::append(parent, &row);
}
