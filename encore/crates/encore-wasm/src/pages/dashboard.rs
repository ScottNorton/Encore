//! Dashboard page — system overview with CPU, memory+storage, network, system cards.
//!
//! All DOM updates are surgical: elements are created once in render() with stable
//! IDs, then update() modifies text/styles in-place. Sparkline canvases are redrawn
//! without recreating the DOM element, preserving scroll state and avoiding flicker.

use crate::components::stat_row_with_id;
use crate::dom;
use crate::state;
use wasm_bindgen::JsCast;

pub fn render(container: &web_sys::Element) {
    // Safe mode banner (hidden by default, shown when in safe mode)
    let banner = dom::create_div();
    banner.set_id("safe-mode-banner");
    dom::set_class(&banner, "card");
    dom::set_style(&banner, "border-left", "4px solid var(--warning)");
    dom::set_style(&banner, "background", "var(--warning-soft)");
    dom::set_style(&banner, "display", "none");
    dom::set_style(&banner, "margin-bottom", "16px");

    let banner_title = dom::el("div", "card-title", Some("Safe Mode"));
    dom::set_style(&banner_title, "color", "var(--warning)");
    dom::append(&banner, &banner_title);

    let banner_body = dom::el(
        "div",
        "",
        Some(
            "A firmware update failed to start. Running last stable firmware. \
         Upload a new version via Update, or restart to clear safe mode.",
        ),
    );
    dom::set_style(&banner_body, "line-height", "1.5");
    dom::append(&banner, &banner_body);

    let banner_btns = dom::create_div();
    dom::set_style(&banner_btns, "margin-top", "12px");
    dom::set_style(&banner_btns, "display", "flex");
    dom::set_style(&banner_btns, "gap", "12px");

    let restart_btn = dom::el("button", "btn btn-primary", Some("Exit Safe Mode"));
    dom::on_click(&restart_btn, || {
        wasm_bindgen_futures::spawn_local(async {
            let window = dom::window();
            let origin = dom::api_origin();
            let url = format!("{}/api/reboot", origin);
            let opts = web_sys::RequestInit::new();
            opts.set_method("POST");
            if let Ok(req) = web_sys::Request::new_with_str_and_init(&url, &opts) {
                let _ = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&req)).await;
            }
            if let Some(el) = dom::get_el("safe-mode-banner") {
                dom::clear(&el);
                let msg = dom::el("div", "card-title", Some("Restarting..."));
                dom::set_style(&msg, "color", "var(--warning)");
                dom::append(&el, &msg);
            }
        });
    });
    dom::append(&banner_btns, &restart_btn);
    dom::append(&banner, &banner_btns);

    dom::append(container, &banner);

    // Card grid
    let grid = dom::create_div();
    grid.set_id("dash-grid");
    dom::set_class(&grid, "card-grid");

    // CPU Card — pre-build skeleton with IDs
    let cpu = dom::create_div();
    cpu.set_id("cpu-card");
    dom::set_class(&cpu, "card");
    let cpu_title = dom::el("div", "card-title", Some("CPU"));
    dom::append(&cpu, &cpu_title);
    let cpu_body = dom::create_div();
    cpu_body.set_id("cpu-body");
    render_cpu_skeleton(&cpu_body);
    dom::append(&cpu, &cpu_body);
    dom::append(&grid, &cpu);

    // Memory & Storage Card
    let mem = dom::create_div();
    mem.set_id("mem-card");
    dom::set_class(&mem, "card");
    let mem_title = dom::el("div", "card-title", Some("Memory & Storage"));
    dom::append(&mem, &mem_title);
    let mem_body = dom::create_div();
    mem_body.set_id("mem-body");
    render_mem_skeleton(&mem_body);
    dom::append(&mem, &mem_body);
    dom::append(&grid, &mem);

    // Network Card
    let net = dom::create_div();
    net.set_id("net-card");
    dom::set_class(&net, "card");
    let net_title = dom::el("div", "card-title", Some("Network"));
    dom::append(&net, &net_title);
    let net_body = dom::create_div();
    net_body.set_id("net-body");
    render_net_skeleton(&net_body);
    dom::append(&net, &net_body);
    dom::append(&grid, &net);

    // System Card
    let sys = dom::create_div();
    sys.set_id("sys-card");
    dom::set_class(&sys, "card");
    let sys_title = dom::el("div", "card-title", Some("System"));
    dom::append(&sys, &sys_title);
    let sys_body = dom::create_div();
    sys_body.set_id("sys-body");
    render_sys_skeleton(&sys_body);
    dom::append(&sys, &sys_body);
    dom::append(&grid, &sys);

    dom::append(container, &grid);

    // Initial update
    update();
}

// ── Skeleton builders — create elements with stable IDs for surgical updates ──

fn render_cpu_skeleton(parent: &web_sys::Element) {
    // Sparkline canvas (created once, redrawn in update)
    let (spark_canvas, _spark_ctx) = crate::graphics::create_canvas(280, 48);
    let spark_el: web_sys::Element = spark_canvas.into();
    spark_el.set_id("dash-cpu-spark");
    dom::set_style(&spark_el, "width", "100%");
    dom::set_style(&spark_el, "margin-bottom", "8px");
    dom::append(parent, &spark_el);

    // Overall row
    let overall = dom::el("div", "stat-row", None);
    let lbl = dom::el("span", "stat-label", Some("Overall"));
    let val = dom::el("span", "stat-value", Some("--"));
    val.set_id("dash-cpu-overall");
    dom::append(&overall, &lbl);
    dom::append(&overall, &val);
    dom::append(parent, &overall);

    // Per-core rows (2 cores on BG2CDP)
    for i in 0..2 {
        let row = dom::create_div();
        row.set_id(&format!("dash-cpu-core-{}", i));
        dom::set_class(&row, "stat-row");

        let label_wrap = dom::create_div();
        dom::set_style(&label_wrap, "min-width", "48px");
        let lbl = dom::el("div", "stat-label", Some(&format!("CPU{}", i)));
        dom::append(&label_wrap, &lbl);
        let freq_label = dom::el("div", "text-muted", Some(""));
        freq_label.set_id(&format!("dash-cpu-freq-{}", i));
        dom::set_style(&freq_label, "font-size", "10px");
        dom::set_style(&freq_label, "line-height", "1");
        dom::append(&label_wrap, &freq_label);
        dom::append(&row, &label_wrap);

        let bar_wrap = dom::create_div();
        dom::set_class(&bar_wrap, "flex items-center gap-8");
        dom::set_style(&bar_wrap, "flex", "1");
        dom::set_style(&bar_wrap, "margin-left", "8px");

        let track = dom::create_div();
        dom::set_class(&track, "bar-track");
        dom::set_style(&track, "flex", "1");
        let fill = dom::create_div();
        fill.set_id(&format!("dash-cpu-fill-{}", i));
        dom::set_class(&fill, "bar-fill");
        dom::set_style(&fill, "background", "var(--green)");
        dom::set_style(&fill, "width", "0%");
        dom::append(&track, &fill);
        dom::append(&bar_wrap, &track);

        let pct_label = dom::el("span", "stat-value text-sm", Some("0%"));
        pct_label.set_id(&format!("dash-cpu-pct-{}", i));
        dom::set_style(&pct_label, "min-width", "32px");
        dom::set_style(&pct_label, "text-align", "right");
        dom::append(&bar_wrap, &pct_label);

        dom::append(&row, &bar_wrap);
        dom::append(parent, &row);
    }

    // Load averages
    let load = dom::el("div", "stat-row mt-8", None);
    let lbl = dom::el("span", "stat-label", Some("Load"));
    dom::append(
        &lbl,
        &dom::hint("Processes waiting to run, averaged over 1, 5, and 15 minutes."),
    );
    let val = dom::el("span", "stat-value text-sm", Some("--"));
    val.set_id("dash-cpu-load");
    dom::append(&load, &lbl);
    dom::append(&load, &val);
    dom::append(parent, &load);
}

fn render_mem_skeleton(parent: &web_sys::Element) {
    // ── RAM section ──
    let mem_label = dom::el("div", "stat-label mb-8", Some("RAM"));
    dom::append(parent, &mem_label);

    // Stacked bar
    let track = dom::create_div();
    track.set_id("dash-mem-track");
    dom::set_class(&track, "bar-track");
    dom::set_style(&track, "height", "10px");
    dom::set_style(&track, "display", "flex");
    dom::set_style(&track, "overflow", "hidden");

    let used_bar = dom::create_div();
    used_bar.set_id("dash-mem-used-bar");
    dom::set_style(&used_bar, "background", "var(--accent)");
    dom::set_style(&used_bar, "width", "0%");
    dom::append(&track, &used_bar);

    let buf_bar = dom::create_div();
    buf_bar.set_id("dash-mem-buf-bar");
    dom::set_style(&buf_bar, "background", "var(--cyan)");
    dom::set_style(&buf_bar, "width", "0%");
    dom::append(&track, &buf_bar);

    let cache_bar = dom::create_div();
    cache_bar.set_id("dash-mem-cache-bar");
    dom::set_style(&cache_bar, "background", "var(--purple)");
    dom::set_style(&cache_bar, "width", "0%");
    dom::append(&track, &cache_bar);

    dom::append(parent, &track);

    // Legend
    let legend = dom::create_div();
    dom::set_class(&legend, "badge-grid mt-8");
    dom::set_style(&legend, "gap", "4px 12px");
    for (label, color, id) in [
        ("Used", "var(--accent)", "dash-mem-legend-used"),
        ("Buf", "var(--cyan)", "dash-mem-legend-buf"),
        ("Cache", "var(--purple)", "dash-mem-legend-cache"),
        ("Free", "var(--text-muted)", "dash-mem-legend-free"),
    ] {
        let item = dom::create_div();
        dom::set_class(&item, "flex items-center gap-4");
        let dot = dom::create_div();
        dom::set_style(&dot, "width", "6px");
        dom::set_style(&dot, "height", "6px");
        dom::set_style(&dot, "border-radius", "50%");
        dom::set_style(&dot, "background", color);
        dom::set_style(&dot, "flex-shrink", "0");
        dom::append(&item, &dot);
        let text = dom::el("span", "text-muted", Some(&format!("{} --", label)));
        text.set_id(id);
        dom::set_style(&text, "font-size", "11px");
        dom::append(&item, &text);
        dom::append(&legend, &item);
    }
    dom::append(parent, &legend);

    // ── Storage section (rebuilt dynamically since disk list can vary) ──
    let storage_area = dom::create_div();
    storage_area.set_id("dash-storage-area");
    dom::append(parent, &storage_area);
}

fn render_net_skeleton(parent: &web_sys::Element) {
    // Sparkline canvas (created once, redrawn in update)
    let (spark_canvas, _spark_ctx) = crate::graphics::create_canvas(280, 40);
    let spark_el: web_sys::Element = spark_canvas.into();
    spark_el.set_id("dash-net-spark");
    dom::set_style(&spark_el, "width", "100%");
    dom::set_style(&spark_el, "margin-bottom", "8px");
    dom::append(parent, &spark_el);

    // AP mode row (hidden by default)
    let ap_row = dom::create_div();
    ap_row.set_id("dash-net-ap");
    dom::set_class(&ap_row, "stat-row");
    dom::set_style(&ap_row, "display", "none");
    let lbl = dom::el("span", "stat-label", Some("AP Mode"));
    let val = dom::el("span", "stat-value text-sm", Some(""));
    val.set_id("dash-net-ap-val");
    dom::append(&ap_row, &lbl);
    dom::append(&ap_row, &val);
    dom::append(parent, &ap_row);

    // Interface throughput rows (rebuilt dynamically since interface list varies)
    let iface_area = dom::create_div();
    iface_area.set_id("dash-net-ifaces");
    dom::append(parent, &iface_area);
}

fn render_sys_skeleton(parent: &web_sys::Element) {
    stat_row_with_id(parent, "Uptime", "--", "dash-sys-uptime");
    stat_row_with_id(parent, "Temperature", "--", "dash-sys-temp");
    stat_row_with_id(parent, "Processes", "--", "dash-sys-procs");
    stat_row_with_id(
        parent,
        "Firmware",
        &format!("v{}", encore_common::VERSION),
        "dash-sys-fw",
    );
}

// ── Surgical update functions ──

pub fn update() {
    state::with(|s| {
        // Toggle safe mode banner visibility
        if let Some(el) = dom::get_el("safe-mode-banner") {
            dom::set_style(&el, "display", if s.safe_mode { "block" } else { "none" });
        }

        if let Some(ref sys) = s.system {
            update_cpu(sys);
            update_memory_storage(sys);
            update_network(sys);
            update_system(sys);
        }
    });
}

fn set_text_if_changed(id: &str, text: &str) {
    if let Some(el) = dom::get_el(id) {
        if el.text_content().as_deref() != Some(text) {
            dom::set_text(&el, text);
        }
    }
}

fn update_cpu(sys: &encore_common::protocol::SystemSnapshot) {
    // Overall percentage
    let overall_pct = state::with(|s| {
        if let Some(last) = s.cpu_history.back() {
            *last
        } else {
            sys.cpu_percent
        }
    });
    set_text_if_changed("dash-cpu-overall", &format!("{}%", overall_pct));

    // Redraw sparkline on existing canvas
    let history: Vec<u8> = state::with(|s| s.cpu_history.iter().copied().collect());
    if !history.is_empty() {
        redraw_sparkline("dash-cpu-spark", &history, 280.0, 48.0);
    }

    // Per-core bars
    state::with(|s| {
        if s.prev_cores.len() == sys.cores.len() && !sys.cores.is_empty() {
            for (i, (cur, prev)) in sys.cores.iter().zip(s.prev_cores.iter()).enumerate() {
                let pct = compute_core_pct(prev, cur);
                let color = if pct > 80 {
                    "var(--red)"
                } else if pct > 50 {
                    "var(--orange)"
                } else {
                    "var(--green)"
                };

                if let Some(fill) = dom::get_el(&format!("dash-cpu-fill-{}", i)) {
                    dom::set_style(&fill, "width", &format!("{}%", pct));
                    dom::set_style(&fill, "background", color);
                }
                set_text_if_changed(&format!("dash-cpu-pct-{}", i), &format!("{}%", pct));

                if cur.freq_khz > 0 {
                    let freq_mhz = cur.freq_khz / 1000;
                    set_text_if_changed(
                        &format!("dash-cpu-freq-{}", i),
                        &format!("{} MHz", freq_mhz),
                    );
                }
            }
        }
    });

    // Load averages
    set_text_if_changed(
        "dash-cpu-load",
        &format!(
            "{:.2} / {:.2} / {:.2}",
            sys.load_avg[0], sys.load_avg[1], sys.load_avg[2]
        ),
    );
}

fn update_memory_storage(sys: &encore_common::protocol::SystemSnapshot) {
    let total = sys.ram_total_kb as f64 / 1024.0;
    let used = (sys.ram_total_kb - sys.ram_free_kb - sys.ram_buffers_kb - sys.ram_cached_kb) as f64
        / 1024.0;
    let buffers = sys.ram_buffers_kb as f64 / 1024.0;
    let cached = sys.ram_cached_kb as f64 / 1024.0;
    let free = sys.ram_free_kb as f64 / 1024.0;

    let pct = |v: f64| format!("{}%", (v / total * 100.0) as u32);

    // Update stacked bar widths
    if let Some(el) = dom::get_el("dash-mem-used-bar") {
        dom::set_style(&el, "width", &pct(used));
    }
    if let Some(el) = dom::get_el("dash-mem-buf-bar") {
        dom::set_style(&el, "width", &pct(buffers));
    }
    if let Some(el) = dom::get_el("dash-mem-cache-bar") {
        dom::set_style(&el, "width", &pct(cached));
    }

    // Update legend text
    set_text_if_changed("dash-mem-legend-used", &format!("Used {:.0}M", used));
    set_text_if_changed("dash-mem-legend-buf", &format!("Buf {:.0}M", buffers));
    set_text_if_changed("dash-mem-legend-cache", &format!("Cache {:.0}M", cached));
    set_text_if_changed("dash-mem-legend-free", &format!("Free {:.0}M", free));

    // Storage section (dynamic disk list — rebuild only this subsection)
    if let Some(area) = dom::get_el("dash-storage-area") {
        let writable_disks: Vec<_> = sys.disks.iter().filter(|d| d.mount != "/").collect();
        if writable_disks.is_empty() {
            // No writable disks — hide storage section
            if area.child_element_count() > 0 {
                dom::clear(&area);
            }
        } else {
            // Rebuild storage rows (small, infrequent changes)
            dom::clear(&area);

            let sep = dom::create_div();
            dom::set_style(&sep, "border-top", "1px solid var(--border)");
            dom::set_style(&sep, "margin", "12px 0");
            dom::append(&area, &sep);

            let storage_label = dom::el("div", "stat-label mb-8", Some("Storage"));
            dom::append(&area, &storage_label);

            for disk in &writable_disks {
                let row = dom::create_div();
                dom::set_class(&row, "mb-4");

                let header = dom::create_div();
                dom::set_class(&header, "flex justify-between mb-4");
                let mount = dom::el("span", "text-muted text-sm", Some(&disk.mount));
                let free_kb = disk.total_kb.saturating_sub(disk.used_kb);
                let usage = dom::el(
                    "span",
                    "text-muted text-sm",
                    Some(&format!("{:.1} MB free", free_kb as f64 / 1024.0,)),
                );
                dom::append(&header, &mount);
                dom::append(&header, &usage);
                dom::append(&row, &header);

                let track = dom::create_div();
                dom::set_class(&track, "bar-track");
                dom::set_style(&track, "height", "6px");
                let fill = dom::create_div();
                dom::set_class(&fill, "bar-fill");
                let pct = if disk.total_kb > 0 {
                    disk.used_kb * 100 / disk.total_kb
                } else {
                    0
                };
                dom::set_style(&fill, "width", &format!("{}%", pct));
                let color = if pct > 90 {
                    "var(--red)"
                } else if pct > 70 {
                    "var(--orange)"
                } else {
                    "var(--accent)"
                };
                dom::set_style(&fill, "background", color);
                dom::append(&track, &fill);
                dom::append(&row, &track);

                dom::append(&area, &row);
            }
        }
    }
}

fn update_network(sys: &encore_common::protocol::SystemSnapshot) {
    // Redraw sparkline on existing canvas
    let history: Vec<u64> = state::with(|s| s.net_rx_history.iter().copied().collect());
    if history.len() > 1 {
        let max_val = history.iter().copied().max().unwrap_or(1).max(1);
        let scaled: Vec<u8> = history
            .iter()
            .map(|&v| (v * 100 / max_val).min(100) as u8)
            .collect();
        redraw_sparkline("dash-net-spark", &scaled, 280.0, 40.0);
    }

    // AP mode info
    state::with(|s| {
        if let Some(ref net) = s.network {
            if let encore_common::protocol::NetworkState::ApMode { ssid, clients, .. } = net {
                if let Some(row) = dom::get_el("dash-net-ap") {
                    dom::set_style(&row, "display", "flex");
                }
                set_text_if_changed(
                    "dash-net-ap-val",
                    &format!(
                        "{} ({} client{})",
                        ssid,
                        clients,
                        if *clients != 1 { "s" } else { "" }
                    ),
                );
            } else if let Some(row) = dom::get_el("dash-net-ap") {
                dom::set_style(&row, "display", "none");
            }
        }
    });

    // Per-interface throughput (dynamic list — rebuild subsection)
    if let Some(area) = dom::get_el("dash-net-ifaces") {
        dom::clear(&area);
        state::with(|s| {
            for iface in &sys.net_interfaces {
                let row = dom::create_div();
                dom::set_class(&row, "stat-row");
                let name = dom::el("span", "stat-label", Some(&iface.name));
                dom::append(&row, &name);

                let rates = dom::create_div();
                dom::set_class(&rates, "flex gap-12 text-sm");

                let (rx_rate, tx_rate) = s
                    .prev_net
                    .get(&iface.name)
                    .map(|(prev_rx, prev_tx)| {
                        let rx = iface.rx_bytes.saturating_sub(*prev_rx);
                        let tx = iface.tx_bytes.saturating_sub(*prev_tx);
                        (format_bytes_rate(rx), format_bytes_rate(tx))
                    })
                    .unwrap_or_else(|| ("--".into(), "--".into()));

                let rx = dom::el("span", "text-muted", Some(&format!("\u{2193} {}", rx_rate)));
                let tx = dom::el("span", "text-muted", Some(&format!("\u{2191} {}", tx_rate)));
                dom::append(&rates, &rx);
                dom::append(&rates, &tx);
                dom::append(&row, &rates);

                dom::append(&area, &row);
            }
        });
    }
}

fn update_system(sys: &encore_common::protocol::SystemSnapshot) {
    set_text_if_changed("dash-sys-uptime", &format_uptime(sys.uptime_secs));
    set_text_if_changed(
        "dash-sys-temp",
        &match sys.temperature_mc {
            Some(mc) => format!("{:.1}\u{00B0}C", mc as f64 / 1000.0),
            None => "N/A".into(),
        },
    );
    set_text_if_changed("dash-sys-procs", &format!("{}", sys.process_count));
}

/// Redraw a sparkline on an existing canvas element (found by ID).
///
/// The context retains its DPI scale transform from create_canvas(), so we
/// save/restore and clear in logical coordinates — sparkline::draw works
/// correctly since it operates in the pre-scaled coordinate space.
fn redraw_sparkline(canvas_id: &str, data: &[u8], w: f64, h: f64) {
    if let Some(el) = dom::get_el(canvas_id) {
        // Clone element ref before moving into dyn_into
        let el_clone = el.clone();
        if let Ok(canvas) = el_clone.dyn_into::<web_sys::HtmlCanvasElement>() {
            if let Ok(Some(ctx)) = canvas.get_context("2d") {
                let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                // save/restore preserves the DPI transform applied by create_canvas
                ctx.save();
                crate::graphics::sparkline::draw(&ctx, data, w, h);
                ctx.restore();
            }
        }
    }
}

fn compute_core_pct(
    prev: &encore_common::protocol::CpuCoreSnapshot,
    cur: &encore_common::protocol::CpuCoreSnapshot,
) -> u8 {
    let prev_total =
        prev.user + prev.nice + prev.system + prev.idle + prev.iowait + prev.irq + prev.softirq;
    let cur_total =
        cur.user + cur.nice + cur.system + cur.idle + cur.iowait + cur.irq + cur.softirq;
    let total_delta = cur_total.saturating_sub(prev_total);
    let idle_delta = cur.idle.saturating_sub(prev.idle);
    if total_delta == 0 {
        return 0;
    }
    ((total_delta - idle_delta) * 100 / total_delta) as u8
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

fn format_bytes_rate(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB/s", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB/s", bytes as f64 / 1024.0)
    } else {
        format!("{} B/s", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encore_common::protocol::CpuCoreSnapshot;

    /// Build a core snapshot from user/idle jiffies (other fields zero).
    fn core(user: u64, idle: u64) -> CpuCoreSnapshot {
        CpuCoreSnapshot {
            user,
            nice: 0,
            system: 0,
            idle,
            iowait: 0,
            irq: 0,
            softirq: 0,
            freq_khz: 0,
        }
    }

    #[test]
    fn format_uptime_minutes_only() {
        // Below one hour: just "<m>m". 0s and 59s both floor to 0 minutes.
        assert_eq!(format_uptime(0), "0m");
        assert_eq!(format_uptime(59), "0m");
        // 60s is exactly one minute.
        assert_eq!(format_uptime(60), "1m");
        // One second short of an hour is still 59 minutes.
        assert_eq!(format_uptime(3599), "59m");
    }

    #[test]
    fn format_uptime_hours_boundary() {
        // Exactly one hour switches to the "<h>h <m>m" form.
        assert_eq!(format_uptime(3600), "1h 0m");
        // One second short of a day: 23h 59m.
        assert_eq!(format_uptime(86399), "23h 59m");
    }

    #[test]
    fn format_uptime_days_boundary() {
        // Exactly one day switches to the "<d>d <h>h <m>m" form.
        assert_eq!(format_uptime(86400), "1d 0h 0m");
        // 1 day + 1 hour + 1 minute + 1 second.
        assert_eq!(format_uptime(90061), "1d 1h 1m");
    }

    #[test]
    fn format_bytes_rate_bytes_branch() {
        // Below 1 KiB shows raw bytes per second.
        assert_eq!(format_bytes_rate(0), "0 B/s");
        assert_eq!(format_bytes_rate(1023), "1023 B/s");
    }

    #[test]
    fn format_bytes_rate_kilobytes_branch() {
        // 1024 B is exactly 1.0 KB/s.
        assert_eq!(format_bytes_rate(1024), "1.0 KB/s");
        // 1536 B = 1.5 KiB.
        assert_eq!(format_bytes_rate(1536), "1.5 KB/s");
        // One byte short of a MiB stays in the KB branch (rounds to 1024.0).
        assert_eq!(format_bytes_rate(1_048_575), "1024.0 KB/s");
    }

    #[test]
    fn format_bytes_rate_megabytes_branch() {
        // Exactly 1 MiB is 1.0 MB/s.
        assert_eq!(format_bytes_rate(1_048_576), "1.0 MB/s");
        // 1.5 MiB.
        assert_eq!(format_bytes_rate(1_572_864), "1.5 MB/s");
    }

    #[test]
    fn compute_core_pct_half_busy() {
        // 50 user + 50 idle jiffies of delta => 50% busy.
        let prev = core(0, 0);
        let cur = core(50, 50);
        assert_eq!(compute_core_pct(&prev, &cur), 50);
    }

    #[test]
    fn compute_core_pct_fully_busy() {
        // All delta in user, none in idle => 100%.
        let prev = core(0, 0);
        let cur = core(100, 0);
        assert_eq!(compute_core_pct(&prev, &cur), 100);
    }

    #[test]
    fn compute_core_pct_fully_idle() {
        // All delta in idle => 0%.
        let prev = core(0, 0);
        let cur = core(0, 100);
        assert_eq!(compute_core_pct(&prev, &cur), 0);
    }

    #[test]
    fn compute_core_pct_no_delta_returns_zero() {
        // Identical snapshots => total_delta == 0 => guarded to 0.
        let snap = core(10, 20);
        assert_eq!(compute_core_pct(&snap, &snap), 0);
    }
}
