//! Segmented control — horizontal pill group with connected look.
//!
//! Replaces preset-bar, viz-toggle, eq-filter-btns, and log level filters
//! with a single unified component.

use std::rc::Rc;

use crate::dom;

/// Selection mode for a segmented control.
pub enum SegmentedMode {
    /// Exclusive selection — clicking a button deactivates all siblings.
    Single(Box<dyn Fn(&str) + 'static>),
    /// Toggle individual items — clicking a button toggles its active state.
    Multi(Box<dyn Fn(&str, bool) + 'static>),
}

pub struct SegmentedControl;

impl SegmentedControl {
    /// Create a segmented control.
    ///
    /// - `id`: base ID for the control (buttons get `{id}-{value}`)
    /// - `options`: slice of `(value, label)` pairs
    /// - `active`: initially active values
    /// - `mode`: `Single` or `Multi` selection
    pub fn create(
        id: &str,
        options: &[(&str, &str)],
        active: &[&str],
        mode: SegmentedMode,
    ) -> web_sys::Element {
        let wrap = dom::create_div();
        dom::set_class(&wrap, "seg-control");
        wrap.set_id(id);

        let mode = Rc::new(mode);

        for &(value, label) in options {
            let btn = dom::create_el("button");
            let btn_id = format!("{}-{}", id, value);
            btn.set_id(&btn_id);
            let is_active = active.contains(&value);
            dom::set_class(
                &btn,
                if is_active {
                    "seg-btn active"
                } else {
                    "seg-btn"
                },
            );
            dom::set_text(&btn, label);

            let mode_rc = Rc::clone(&mode);
            let value_str = value.to_string();
            let parent_id = id.to_string();
            dom::on_click(&btn, move || {
                match mode_rc.as_ref() {
                    SegmentedMode::Single(cb) => {
                        // Deactivate all siblings
                        if let Some(parent) = dom::get_el(&parent_id) {
                            let children = parent.children();
                            for i in 0..children.length() {
                                if let Some(child) = children.item(i) {
                                    dom::set_class(&child, "seg-btn");
                                }
                            }
                        }
                        // Activate clicked
                        let btn_id = format!("{}-{}", parent_id, value_str);
                        if let Some(el) = dom::get_el(&btn_id) {
                            dom::set_class(&el, "seg-btn active");
                        }
                        cb(&value_str);
                    }
                    SegmentedMode::Multi(cb) => {
                        let btn_id = format!("{}-{}", parent_id, value_str);
                        if let Some(el) = dom::get_el(&btn_id) {
                            let classes = el.class_name();
                            let now_active = !classes.contains("active");
                            dom::set_class(
                                &el,
                                if now_active {
                                    "seg-btn active"
                                } else {
                                    "seg-btn"
                                },
                            );
                            cb(&value_str, now_active);
                        }
                    }
                }
            });

            dom::append(&wrap, &btn);
        }

        wrap
    }
}
