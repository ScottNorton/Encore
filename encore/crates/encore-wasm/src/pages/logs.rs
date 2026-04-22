//! Logs page — real-time log viewer with level filtering.
//!
//! On first render, fetches log history from `GET /api/logs` so the user
//! sees all entries since boot, not just those arriving after page load.

use crate::components::{SegmentedControl, SegmentedMode};
use crate::dom;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    static LOG_BUFFER: RefCell<VecDeque<LogLine>> = RefCell::new(VecDeque::new());
    static HISTORY_LOADED: Cell<bool> = Cell::new(false);
}

struct LogLine {
    level: String,
    target: String,
    message: String,
    timestamp_ms: u64,
}

const MAX_LINES: usize = 500;

pub fn render(container: &web_sys::Element) {
    // Filter bar
    let filter_bar = dom::create_div();
    dom::set_class(&filter_bar, "flex gap-8 items-center mb-12");
    dom::set_style(&filter_bar, "flex-wrap", "wrap");

    // Level filter buttons (multi-select segmented control)
    let seg = SegmentedControl::create(
        "log-filters",
        &[
            ("error", "ERROR"),
            ("warn", "WARN"),
            ("info", "INFO"),
            ("debug", "DEBUG"),
            ("trace", "TRACE"),
        ],
        &["error", "warn", "info"],
        SegmentedMode::Multi(Box::new(|_level, _active| {
            render_log_output();
        })),
    );
    dom::append(&filter_bar, &seg);

    // Spacer
    let spacer = dom::create_div();
    dom::set_style(&spacer, "flex", "1");
    dom::append(&filter_bar, &spacer);

    // Clear button
    let clear_btn = dom::el("button", "btn btn-sm", Some("Clear"));
    dom::on_click(&clear_btn, || {
        LOG_BUFFER.with(|b| b.borrow_mut().clear());
        if let Some(el) = dom::get_el("log-output") {
            dom::clear(&el);
        }
    });
    dom::append(&filter_bar, &clear_btn);

    dom::append(container, &filter_bar);

    // Log output area
    let output = dom::create_div();
    output.set_id("log-output");
    dom::set_class(&output, "card log-output");

    dom::append(container, &output);

    // Stats bar
    let stats = dom::create_div();
    stats.set_id("log-stats");
    dom::set_class(&stats, "flex gap-12 mt-8 text-sm text-muted");
    dom::append(container, &stats);

    // Render existing buffer
    render_log_output();

    // Fetch log history from server on first render
    HISTORY_LOADED.with(|loaded| {
        if !loaded.get() {
            loaded.set(true);
            fetch_log_history();
        }
    });
}

fn is_level_enabled(level: &str) -> bool {
    let id = format!("log-filters-{}", level.to_lowercase());
    dom::get_el(&id)
        .map(|el| el.class_name().contains("active"))
        .unwrap_or(true)
}

fn level_class(level: &str) -> &'static str {
    match level {
        "ERROR" => "log-line-error",
        "WARN" => "log-line-warn",
        "INFO" => "log-line-info",
        "DEBUG" => "log-line-debug",
        "TRACE" => "log-line-trace",
        _ => "",
    }
}

fn render_log_output() {
    if let Some(el) = dom::get_el("log-output") {
        dom::clear(&el);
        LOG_BUFFER.with(|b| {
            let buf = b.borrow();
            if buf.is_empty() {
                let placeholder =
                    dom::el("span", "text-muted", Some("Waiting for log entries..."));
                dom::append(&el, &placeholder);
                return;
            }
            for line in buf.iter() {
                if !is_level_enabled(&line.level) {
                    continue;
                }
                let entry = dom::create_div();
                let cls = level_class(&line.level);
                if !cls.is_empty() {
                    dom::set_class(&entry, cls);
                }
                let ts = format_timestamp(line.timestamp_ms);
                let text = format!(
                    "{} {:>5} {} \u{2014} {}",
                    ts, line.level, line.target, line.message
                );
                dom::set_text(&entry, &text);
                dom::append(&el, &entry);
            }
        });
        // Auto-scroll to bottom
        let scroll_height = el.scroll_height();
        el.set_scroll_top(scroll_height);
    }
}

/// Format epoch millis as local time HH:MM:SS.
fn format_timestamp(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:02}:{:02}:{:02}",
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds(),
    )
}

/// Called from ws.rs when LogEntries arrive.
pub fn append_entries(entries: &[encore_common::protocol::LogEntry]) {
    LOG_BUFFER.with(|b| {
        let mut buf = b.borrow_mut();
        for entry in entries {
            buf.push_back(LogLine {
                level: entry.level.clone(),
                target: entry.target.clone(),
                message: entry.message.clone(),
                timestamp_ms: entry.timestamp_ms,
            });
            while buf.len() > MAX_LINES {
                buf.pop_front();
            }
        }
    });
    render_log_output();
}

/// Fetch log history from the server and prepend to the buffer.
fn fetch_log_history() {
    wasm_bindgen_futures::spawn_local(async {
        let window = dom::window();
        let origin = dom::api_origin();
        let url = format!("{}/api/logs", origin);

        let resp_val = match JsFuture::from(window.fetch_with_str(&url)).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let resp: web_sys::Response = resp_val.unchecked_into();
        let text_val = match resp.text() {
            Ok(promise) => match JsFuture::from(promise).await {
                Ok(v) => v,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let text = match text_val.as_string() {
            Some(s) => s,
            None => return,
        };

        if let Ok(entries) = serde_json::from_str::<Vec<encore_common::protocol::LogEntry>>(&text) {
            if entries.is_empty() {
                return;
            }
            LOG_BUFFER.with(|b| {
                let mut buf = b.borrow_mut();
                // Only prepend history entries older than what WS already delivered
                let oldest_existing = buf.front().map(|l| l.timestamp_ms);
                let history: Vec<LogLine> = entries
                    .iter()
                    .filter(|e| oldest_existing.map_or(true, |t| e.timestamp_ms < t))
                    .map(|e| LogLine {
                        level: e.level.clone(),
                        target: e.target.clone(),
                        message: e.message.clone(),
                        timestamp_ms: e.timestamp_ms,
                    })
                    .collect();
                // Prepend history before existing WS entries
                for line in history.into_iter().rev() {
                    buf.push_front(line);
                }
                while buf.len() > MAX_LINES {
                    buf.pop_front();
                }
            });
            render_log_output();
        }
    });
}
