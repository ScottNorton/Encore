//! Ephemeral toast notifications — success, error, info.
//!
//! `show(kind, message)` mounts a toast into a bottom-center container, fades
//! it in, and removes it after a kind-dependent duration. This is the single
//! feedback primitive every Save / restart / scan in the dashboard calls.

use crate::dom;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

fn variant_class(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::Success => "toast-success",
        ToastKind::Error => "toast-error",
        ToastKind::Info => "toast-info",
    }
}

fn default_duration_ms(kind: ToastKind) -> i32 {
    match kind {
        ToastKind::Success => 2500,
        ToastKind::Error => 5000,
        ToastKind::Info => 3000,
    }
}

/// Show a success toast.
pub fn success(message: &str) {
    show(ToastKind::Success, message);
}

/// Show an error toast.
pub fn error(message: &str) {
    show(ToastKind::Error, message);
}

/// Show an info toast.
pub fn info(message: &str) {
    show(ToastKind::Info, message);
}

/// Mount a toast, fade it in, and remove it after the kind's duration.
pub fn show(kind: ToastKind, message: &str) {
    let container = match dom::get_el("toast-container") {
        Some(el) => el,
        None => {
            let el = dom::create_div();
            el.set_id("toast-container");
            dom::set_class(&el, "toast-container");
            dom::body().append_child(&el).ok();
            el
        }
    };

    let toast = dom::el(
        "div",
        &format!("toast {}", variant_class(kind)),
        Some(message),
    );
    dom::append(&container, &toast);

    // Fade in on the next tick so the transition runs.
    let toast_in = toast.clone();
    dom::set_timeout(move || dom::add_class(&toast_in, "show"), 10);

    // Fade out, then remove.
    let toast_out = toast.clone();
    dom::set_timeout(
        move || {
            dom::remove_class(&toast_out, "show");
            let toast_gone = toast_out.clone();
            dom::set_timeout(move || toast_gone.remove(), 300);
        },
        default_duration_ms(kind),
    );
}

/// Toast CSS, concatenated into the stylesheet by `style::inject()`.
pub fn css() -> &'static str {
    r#"
.toast-container {
    position: fixed;
    left: 50%;
    bottom: calc(16px + env(safe-area-inset-bottom, 0px));
    transform: translateX(-50%);
    z-index: 300;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: center;
    pointer-events: none;
}
.toast {
    min-width: 200px;
    max-width: 90vw;
    padding: 12px 18px;
    border-radius: var(--radius-sm);
    background: var(--bg-card);
    color: var(--text);
    box-shadow: var(--shadow);
    border-left: 3px solid var(--text-secondary);
    font-size: 14px;
    opacity: 0;
    transform: translateY(8px);
    transition: opacity 0.25s ease, transform 0.25s ease;
    pointer-events: auto;
}
.toast.show { opacity: 1; transform: translateY(0); }
.toast-success { border-left-color: var(--green); }
.toast-error { border-left-color: var(--red); }
.toast-info { border-left-color: var(--accent); }
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_class_maps_each_kind() {
        assert_eq!(variant_class(ToastKind::Success), "toast-success");
        assert_eq!(variant_class(ToastKind::Error), "toast-error");
        assert_eq!(variant_class(ToastKind::Info), "toast-info");
    }

    #[test]
    fn error_lingers_longer_than_success() {
        assert!(default_duration_ms(ToastKind::Error) > default_duration_ms(ToastKind::Success));
    }

    #[test]
    fn css_defines_container_and_variants() {
        let c = css();
        assert!(c.contains(".toast-container"), "container selector missing");
        assert!(c.contains(".toast-success"), "success variant missing");
        assert!(c.contains(".toast-error"), "error variant missing");
        assert!(c.contains(".toast-info"), "info variant missing");
    }
}
