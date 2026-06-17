//! Lights page — LED ring control, color presets, animation buttons,
//! and an interactive animation designer.

use std::cell::RefCell;
use wasm_bindgen::JsCast;

use crate::dom;
use encore_common::protocol::{ClientMsg, LedAnimation, LedFrame};

/// Color presets: (name, r, g, b)
const PRESETS: &[(&str, u8, u8, u8)] = &[
    ("Red", 255, 0, 0),
    ("Orange", 255, 120, 0),
    ("Yellow", 255, 220, 0),
    ("Green", 0, 255, 0),
    ("Cyan", 0, 255, 200),
    ("Blue", 0, 80, 255),
    ("Purple", 128, 0, 255),
    ("Pink", 255, 0, 128),
    ("White", 255, 255, 255),
    ("Warm", 255, 180, 80),
    ("Cool", 150, 200, 255),
    ("Gold", 255, 200, 50),
];

// ── Animation Designer State ──

thread_local! {
    static DESIGNER: RefCell<DesignerData> = RefCell::new(DesignerData::default());
}

#[derive(Clone)]
struct DesignerFrame {
    colors: [(u8, u8, u8); 13],
    duration_ms: u16,
}

struct DesignerData {
    frames: Vec<DesignerFrame>,
    current: usize,
    selected: [bool; 13],
    expanded: bool,
}

impl Default for DesignerData {
    fn default() -> Self {
        Self {
            frames: vec![DesignerFrame {
                colors: [(100, 100, 255); 13],
                duration_ms: 100,
            }],
            current: 0,
            selected: [true; 13],
            expanded: false,
        }
    }
}

pub fn render(container: &web_sys::Element) {
    // LED Ring preview
    let ring_card = dom::create_div();
    dom::set_class(&ring_card, "card text-center");

    let ring_title = dom::el("div", "card-title", Some("LED Ring"));
    dom::append(&ring_card, &ring_title);

    let (ring_canvas, ring_ctx) = crate::graphics::create_canvas(200, 200);
    ring_canvas.set_id("led-ring-canvas");
    let ring_el: web_sys::Element = ring_canvas.into();
    dom::append(&ring_card, &ring_el);

    // Draw initial state — start animation if animated, otherwise static
    let led_anim = crate::state::with(|s| s.led.clone());
    match &led_anim {
        Some(
            anim @ (LedAnimation::Breathe { .. }
            | LedAnimation::Spin { .. }
            | LedAnimation::Pulse { .. }
            | LedAnimation::BootSurge
            | LedAnimation::Custom { .. }),
        ) => {
            // Draw one frame immediately, then start animation loop
            let color = match anim {
                LedAnimation::Breathe { r, g, b, .. } => Some((*r, *g, *b)),
                LedAnimation::Spin { r, g, b, .. } => Some((*r, *g, *b)),
                LedAnimation::Pulse { r, g, b } => Some((*r, *g, *b)),
                LedAnimation::BootSurge => Some((255, 180, 30)),
                LedAnimation::Custom { .. } => Some((100, 100, 255)),
                _ => None,
            };
            crate::graphics::led_ring::draw(&ring_ctx, 200.0, color);
            crate::graphics::led_ring::start_animation(anim.clone());
        }
        Some(anim) => {
            let color = match anim {
                LedAnimation::Solid { r, g, b } => Some((*r, *g, *b)),
                LedAnimation::VolumeArc { .. } => Some((255, 255, 255)),
                _ => None,
            };
            crate::graphics::led_ring::draw(&ring_ctx, 200.0, color);
        }
        None => {
            crate::graphics::led_ring::draw(&ring_ctx, 200.0, None);
        }
    }

    dom::append(container, &ring_card);

    // Color wheel
    let wheel_card = dom::create_div();
    dom::set_class(&wheel_card, "card");
    let wheel_title = dom::el("div", "card-title", Some("Color Wheel"));
    dom::append(&wheel_card, &wheel_title);

    let wheel_wrap = dom::create_div();
    dom::set_class(&wheel_wrap, "color-wheel-wrap");
    let wheel_size = crate::graphics::color_wheel::size();
    let (wheel_canvas, wheel_ctx) = crate::graphics::create_canvas(wheel_size, wheel_size);
    crate::graphics::color_wheel::draw(&wheel_ctx);
    crate::graphics::color_wheel::make_interactive(&wheel_canvas, |r, g, b| {
        crate::ws::send_msg(&ClientMsg::SetLed(LedAnimation::Solid { r, g, b }));
    });
    let wheel_el: web_sys::Element = wheel_canvas.into();
    dom::append(&wheel_wrap, &wheel_el);
    dom::append(&wheel_card, &wheel_wrap);
    dom::append(container, &wheel_card);

    // Color presets
    let presets_card = dom::create_div();
    dom::set_class(&presets_card, "card");

    let presets_title = dom::el("div", "card-title", Some("Color Presets"));
    dom::append(&presets_card, &presets_title);

    let grid = dom::create_div();
    dom::set_style(&grid, "display", "grid");
    dom::set_style(&grid, "grid-template-columns", "repeat(6, 1fr)");
    dom::set_style(&grid, "gap", "8px");

    for &(_name, r, g, b) in PRESETS {
        let swatch = dom::create_div();
        dom::set_style(&swatch, "width", "100%");
        dom::set_style(&swatch, "aspect-ratio", "1");
        dom::set_style(&swatch, "border-radius", "8px");
        dom::set_style(&swatch, "cursor", "pointer");
        dom::set_style(&swatch, "background", &crate::graphics::rgb_str(r, g, b));
        dom::set_style(&swatch, "border", "2px solid transparent");
        dom::set_style(&swatch, "transition", "border-color .2s, transform .1s");
        dom::on_click(&swatch, move || {
            crate::ws::send_msg(&ClientMsg::SetLed(LedAnimation::Solid { r, g, b }));
        });
        dom::append(&grid, &swatch);
    }

    // Off button
    let off_btn = dom::el("button", "btn btn-danger w-full mt-12", Some("Off"));
    dom::on_click(&off_btn, || {
        crate::ws::send_msg(&ClientMsg::SetLed(LedAnimation::Off));
    });

    dom::append(&presets_card, &grid);
    dom::append(&presets_card, &off_btn);
    dom::append(container, &presets_card);

    // Animation buttons
    let anim_card = dom::create_div();
    dom::set_class(&anim_card, "card");

    let anim_title = dom::el("div", "card-title", Some("Animations"));
    dom::append(&anim_card, &anim_title);

    let anim_grid = dom::create_div();
    dom::set_class(&anim_grid, "flex gap-8");

    for (label, anim) in [
        (
            "Breathe",
            LedAnimation::Breathe {
                r: 255,
                g: 200,
                b: 50,
                period_ms: 3000,
            },
        ),
        (
            "Spin",
            LedAnimation::Spin {
                r: 0,
                g: 150,
                b: 255,
                speed: 3,
            },
        ),
        (
            "Pulse",
            LedAnimation::Pulse {
                r: 255,
                g: 100,
                b: 0,
            },
        ),
    ] {
        let btn = dom::el("button", "btn", Some(label));
        dom::set_style(&btn, "flex", "1");
        dom::on_click(&btn, move || {
            crate::ws::send_msg(&ClientMsg::SetLed(anim.clone()));
        });
        dom::append(&anim_grid, &btn);
    }

    dom::append(&anim_card, &anim_grid);
    dom::append(container, &anim_card);

    // Animation designer
    render_designer(container);
}

pub fn update() {
    let led_anim = crate::state::with(|s| s.led.clone());

    match &led_anim {
        Some(
            anim @ (LedAnimation::Breathe { .. }
            | LedAnimation::Spin { .. }
            | LedAnimation::Pulse { .. }
            | LedAnimation::BootSurge
            | LedAnimation::Custom { .. }),
        ) => {
            // Restart animation with new parameters
            crate::graphics::led_ring::start_animation(anim.clone());
        }
        Some(anim) => {
            // Static — stop any running animation, draw once
            crate::graphics::led_ring::stop_animation();
            let color = match anim {
                LedAnimation::Solid { r, g, b } => Some((*r, *g, *b)),
                LedAnimation::VolumeArc { .. } => Some((255, 255, 255)),
                LedAnimation::Off => None,
                _ => None,
            };
            if let Some(canvas) = dom::get_el("led-ring-canvas") {
                if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                    if let Ok(Some(ctx)) = canvas.get_context("2d") {
                        let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                        crate::graphics::led_ring::draw(&ctx, 200.0, color);
                    }
                }
            }
        }
        None => {
            crate::graphics::led_ring::stop_animation();
            if let Some(canvas) = dom::get_el("led-ring-canvas") {
                if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                    if let Ok(Some(ctx)) = canvas.get_context("2d") {
                        let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                        crate::graphics::led_ring::draw(&ctx, 200.0, None);
                    }
                }
            }
        }
    }
}

// ── Animation Designer ──

fn render_designer(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    // Expandable header
    let header = dom::create_div();
    dom::set_class(&header, "card-title");
    dom::set_style(&header, "cursor", "pointer");
    dom::set_style(&header, "user-select", "none");
    let expanded = DESIGNER.with(|d| d.borrow().expanded);
    dom::set_text(
        &header,
        &format!(
            "{} Animation Designer",
            if expanded { "\u{25BC}" } else { "\u{25B6}" }
        ),
    );

    let body = dom::create_div();
    body.set_id("designer-body");
    dom::set_style(&body, "display", if expanded { "block" } else { "none" });

    // Toggle expand/collapse
    {
        let header_c = header.clone();
        let body_c = body.clone();
        dom::on_click(&header, move || {
            let exp = DESIGNER.with(|d| {
                let mut d = d.borrow_mut();
                d.expanded = !d.expanded;
                d.expanded
            });
            dom::set_text(
                &header_c,
                &format!(
                    "{} Animation Designer",
                    if exp { "\u{25BC}" } else { "\u{25B6}" }
                ),
            );
            dom::set_style(&body_c, "display", if exp { "block" } else { "none" });
        });
    }

    dom::append(&card, &header);

    // ── LED grid ──
    let led_label = dom::el(
        "div",
        "text-sm text-muted",
        Some("Click LEDs to select, then adjust color:"),
    );
    dom::append(&body, &led_label);

    // Ring LEDs: 6x2 grid
    let ring_grid = dom::create_div();
    dom::set_style(&ring_grid, "display", "grid");
    dom::set_style(&ring_grid, "grid-template-columns", "repeat(6, 28px)");
    dom::set_style(&ring_grid, "gap", "4px");
    dom::set_style(&ring_grid, "justify-content", "center");
    dom::set_style(&ring_grid, "margin", "8px 0 4px");

    for i in 0..12 {
        let sq = dom::create_div();
        let sq_id = format!("dsn-led-{}", i);
        sq.set_id(&sq_id);
        dom::set_style(&sq, "width", "28px");
        dom::set_style(&sq, "height", "28px");
        dom::set_style(&sq, "border-radius", "50%");
        dom::set_style(&sq, "cursor", "pointer");
        dom::set_style(&sq, "transition", "border-color .15s");

        DESIGNER.with(|d| {
            let d = d.borrow();
            let (r, g, b) = d.frames[d.current].colors[i];
            dom::set_style(&sq, "background", &crate::graphics::rgb_str(r, g, b));
            let border = if d.selected[i] {
                "2px solid #fff"
            } else {
                "2px solid #333"
            };
            dom::set_style(&sq, "border", border);
        });

        {
            let id = sq_id.clone();
            dom::on_click(&sq, move || {
                let sel = DESIGNER.with(|d| {
                    let mut d = d.borrow_mut();
                    d.selected[i] = !d.selected[i];
                    d.selected[i]
                });
                if let Some(el) = dom::get_el(&id) {
                    dom::set_style(
                        &el,
                        "border",
                        if sel {
                            "2px solid #fff"
                        } else {
                            "2px solid #333"
                        },
                    );
                }
            });
        }

        dom::append(&ring_grid, &sq);
    }
    dom::append(&body, &ring_grid);

    // Center LED
    let center_wrap = dom::create_div();
    dom::set_style(&center_wrap, "display", "flex");
    dom::set_style(&center_wrap, "justify-content", "center");
    dom::set_style(&center_wrap, "margin-bottom", "8px");

    let csq = dom::create_div();
    csq.set_id("dsn-led-12");
    dom::set_style(&csq, "width", "32px");
    dom::set_style(&csq, "height", "32px");
    dom::set_style(&csq, "border-radius", "50%");
    dom::set_style(&csq, "cursor", "pointer");
    dom::set_style(&csq, "transition", "border-color .15s");

    DESIGNER.with(|d| {
        let d = d.borrow();
        let (r, g, b) = d.frames[d.current].colors[12];
        dom::set_style(&csq, "background", &crate::graphics::rgb_str(r, g, b));
        let border = if d.selected[12] {
            "2px solid #fff"
        } else {
            "2px solid #333"
        };
        dom::set_style(&csq, "border", border);
    });

    dom::on_click(&csq, || {
        let sel = DESIGNER.with(|d| {
            let mut d = d.borrow_mut();
            d.selected[12] = !d.selected[12];
            d.selected[12]
        });
        if let Some(el) = dom::get_el("dsn-led-12") {
            dom::set_style(
                &el,
                "border",
                if sel {
                    "2px solid #fff"
                } else {
                    "2px solid #333"
                },
            );
        }
    });

    dom::append(&center_wrap, &csq);
    dom::append(&body, &center_wrap);

    // Select All / Ring / None
    let sel_row = dom::create_div();
    dom::set_class(&sel_row, "flex gap-8");
    dom::set_style(&sel_row, "justify-content", "center");
    dom::set_style(&sel_row, "margin-bottom", "12px");

    let sel_all = dom::el("button", "btn", Some("All"));
    dom::set_style(&sel_all, "font-size", "12px");
    dom::set_style(&sel_all, "padding", "4px 10px");
    dom::on_click(&sel_all, || {
        DESIGNER.with(|d| d.borrow_mut().selected = [true; 13]);
        update_designer_led_borders();
    });

    let sel_ring = dom::el("button", "btn", Some("Ring"));
    dom::set_style(&sel_ring, "font-size", "12px");
    dom::set_style(&sel_ring, "padding", "4px 10px");
    dom::on_click(&sel_ring, || {
        DESIGNER.with(|d| {
            let mut d = d.borrow_mut();
            d.selected = [true; 13];
            d.selected[12] = false;
        });
        update_designer_led_borders();
    });

    let sel_none = dom::el("button", "btn", Some("None"));
    dom::set_style(&sel_none, "font-size", "12px");
    dom::set_style(&sel_none, "padding", "4px 10px");
    dom::on_click(&sel_none, || {
        DESIGNER.with(|d| d.borrow_mut().selected = [false; 13]);
        update_designer_led_borders();
    });

    dom::append(&sel_row, &sel_all);
    dom::append(&sel_row, &sel_ring);
    dom::append(&sel_row, &sel_none);
    dom::append(&body, &sel_row);

    // ── Color sliders (R / G / B) ──
    let color_section = dom::create_div();
    for (label, channel) in [("R", 0u8), ("G", 1), ("B", 2)] {
        let row = dom::create_div();
        dom::set_class(&row, "flex gap-8");
        dom::set_style(&row, "align-items", "center");
        dom::set_style(&row, "margin", "4px 0");

        let lbl = dom::el("span", "text-sm", Some(label));
        dom::set_style(&lbl, "width", "16px");
        dom::append(&row, &lbl);

        let slider = dom::create_el("input");
        dom::set_attr(&slider, "type", "range");
        dom::set_attr(&slider, "min", "0");
        dom::set_attr(&slider, "max", "255");
        dom::set_attr(&slider, "value", "128");
        dom::set_style(&slider, "flex", "1");

        let val_id = format!("dsn-{}-val", label.to_lowercase());
        let val_span = dom::el("span", "text-sm", Some("128"));
        val_span.set_id(&val_id);
        dom::set_style(&val_span, "width", "32px");
        dom::set_style(&val_span, "text-align", "right");

        {
            let val_id = val_id.clone();
            dom::on_input(&slider, move |v| {
                let val: u8 = v.parse().unwrap_or(0);
                if let Some(el) = dom::get_el(&val_id) {
                    dom::set_text(&el, &v);
                }
                DESIGNER.with(|d| {
                    let mut d = d.borrow_mut();
                    let cur = d.current;
                    for i in 0..13 {
                        if d.selected[i] {
                            let c = &mut d.frames[cur].colors[i];
                            match channel {
                                0 => c.0 = val,
                                1 => c.1 = val,
                                2 => c.2 = val,
                                _ => {}
                            }
                            let id = format!("dsn-led-{}", i);
                            if let Some(el) = dom::get_el(&id) {
                                dom::set_style(
                                    &el,
                                    "background",
                                    &crate::graphics::rgb_str(c.0, c.1, c.2),
                                );
                            }
                        }
                    }
                });
            });
        }

        dom::append(&row, &slider);
        dom::append(&row, &val_span);
        dom::append(&color_section, &row);
    }
    dom::append(&body, &color_section);

    // ── Frame timeline ──
    let tl_section = dom::create_div();
    dom::set_style(&tl_section, "margin-top", "12px");

    let tl_label = dom::el("div", "text-sm text-muted", Some("Frames:"));
    dom::append(&tl_section, &tl_label);

    let tl_strip = dom::create_div();
    tl_strip.set_id("dsn-timeline");
    dom::set_class(&tl_strip, "designer-timeline");
    refresh_timeline_strip(&tl_strip);
    dom::append(&tl_section, &tl_strip);

    // Frame control row
    let frame_row = dom::create_div();
    dom::set_class(&frame_row, "flex gap-8 mt-8");
    dom::set_style(&frame_row, "flex-wrap", "wrap");
    dom::set_style(&frame_row, "align-items", "center");

    let add_btn = dom::el("button", "btn", Some("Add"));
    dom::set_style(&add_btn, "font-size", "12px");
    dom::set_style(&add_btn, "padding", "4px 10px");
    dom::on_click(&add_btn, || {
        DESIGNER.with(|d| {
            let mut d = d.borrow_mut();
            d.frames.push(DesignerFrame {
                colors: [(100, 100, 255); 13],
                duration_ms: 100,
            });
            d.current = d.frames.len() - 1;
        });
        if let Some(strip) = dom::get_el("dsn-timeline") {
            refresh_timeline_strip(&strip);
        }
        refresh_designer_leds();
    });

    let dup_btn = dom::el("button", "btn", Some("Dup"));
    dom::set_style(&dup_btn, "font-size", "12px");
    dom::set_style(&dup_btn, "padding", "4px 10px");
    dom::on_click(&dup_btn, || {
        DESIGNER.with(|d| {
            let mut d = d.borrow_mut();
            let cur = d.current;
            let clone = d.frames[cur].clone();
            d.frames.insert(cur + 1, clone);
            d.current = cur + 1;
        });
        if let Some(strip) = dom::get_el("dsn-timeline") {
            refresh_timeline_strip(&strip);
        }
        refresh_designer_leds();
    });

    let del_btn = dom::el("button", "btn btn-danger", Some("Del"));
    dom::set_style(&del_btn, "font-size", "12px");
    dom::set_style(&del_btn, "padding", "4px 10px");
    dom::on_click(&del_btn, || {
        DESIGNER.with(|d| {
            let mut d = d.borrow_mut();
            if d.frames.len() > 1 {
                let cur = d.current;
                d.frames.remove(cur);
                if d.current >= d.frames.len() {
                    d.current = d.frames.len() - 1;
                }
            }
        });
        if let Some(strip) = dom::get_el("dsn-timeline") {
            refresh_timeline_strip(&strip);
        }
        refresh_designer_leds();
    });

    // Duration input
    let dur_label = dom::el("span", "text-sm", Some("ms:"));
    let dur_input = dom::create_el("input");
    dur_input.set_id("dsn-duration");
    dom::set_attr(&dur_input, "type", "number");
    dom::set_attr(&dur_input, "min", "33");
    dom::set_attr(&dur_input, "max", "1000");
    let cur_dur = DESIGNER.with(|d| {
        let d = d.borrow();
        d.frames[d.current].duration_ms.to_string()
    });
    dom::set_attr(&dur_input, "value", &cur_dur);
    dom::set_style(&dur_input, "width", "56px");
    dom::set_style(&dur_input, "background", "var(--surface)");
    dom::set_style(&dur_input, "color", "var(--text)");
    dom::set_style(&dur_input, "border", "1px solid var(--border)");
    dom::set_style(&dur_input, "border-radius", "4px");
    dom::set_style(&dur_input, "padding", "2px 4px");

    dom::on_input(&dur_input, |v| {
        if let Ok(ms) = v.parse::<u16>() {
            DESIGNER.with(|d| {
                let mut d = d.borrow_mut();
                let cur = d.current;
                d.frames[cur].duration_ms = ms.clamp(33, 1000);
            });
        }
    });

    dom::append(&frame_row, &add_btn);
    dom::append(&frame_row, &dup_btn);
    dom::append(&frame_row, &del_btn);
    dom::append(&frame_row, &dur_label);
    dom::append(&frame_row, &dur_input);
    dom::append(&tl_section, &frame_row);

    dom::append(&body, &tl_section);

    // ── Action buttons ──
    let action_row = dom::create_div();
    dom::set_class(&action_row, "flex gap-8 mt-12");

    let preview_btn = dom::el("button", "btn", Some("Preview"));
    dom::on_click(&preview_btn, || {
        crate::graphics::led_ring::start_animation(build_custom_animation());
    });

    let stop_btn = dom::el("button", "btn", Some("Stop"));
    dom::on_click(&stop_btn, || {
        crate::graphics::led_ring::stop_animation();
    });

    let send_btn = dom::el("button", "btn btn-primary", Some("Send to Device"));
    dom::on_click(&send_btn, || {
        let frames = DESIGNER.with(|d| {
            d.borrow()
                .frames
                .iter()
                .map(|f| LedFrame {
                    colors: f.colors,
                    duration_ms: f.duration_ms,
                })
                .collect::<Vec<_>>()
        });
        crate::ws::send_msg(&ClientMsg::SetCustomAnimation { frames });
    });

    dom::append(&action_row, &preview_btn);
    dom::append(&action_row, &stop_btn);
    dom::append(&action_row, &send_btn);
    dom::append(&body, &action_row);

    dom::append(&card, &body);
    dom::append(container, &card);
}

fn update_designer_led_borders() {
    DESIGNER.with(|d| {
        let d = d.borrow();
        for i in 0..13 {
            let id = format!("dsn-led-{}", i);
            if let Some(el) = dom::get_el(&id) {
                dom::set_style(
                    &el,
                    "border",
                    if d.selected[i] {
                        "2px solid #fff"
                    } else {
                        "2px solid #333"
                    },
                );
            }
        }
    });
}

fn refresh_designer_leds() {
    DESIGNER.with(|d| {
        let d = d.borrow();
        let frame = &d.frames[d.current];
        for i in 0..13 {
            let id = format!("dsn-led-{}", i);
            if let Some(el) = dom::get_el(&id) {
                let (r, g, b) = frame.colors[i];
                dom::set_style(&el, "background", &crate::graphics::rgb_str(r, g, b));
                dom::set_style(
                    &el,
                    "border",
                    if d.selected[i] {
                        "2px solid #fff"
                    } else {
                        "2px solid #333"
                    },
                );
            }
        }
    });
    // Update duration input
    if let Some(el) = dom::get_el("dsn-duration") {
        let dur = DESIGNER.with(|d| {
            let d = d.borrow();
            d.frames[d.current].duration_ms.to_string()
        });
        dom::set_attr(&el, "value", &dur);
    }
}

fn refresh_timeline_strip(strip: &web_sys::Element) {
    dom::clear(strip);
    DESIGNER.with(|d| {
        let d = d.borrow();
        for (idx, frame) in d.frames.iter().enumerate() {
            let thumb = dom::create_div();
            dom::set_class(&thumb, "designer-frame");

            let (avg_r, avg_g, avg_b) = avg_frame_color(&frame.colors);
            dom::set_style(
                &thumb,
                "background",
                &crate::graphics::rgb_str(avg_r, avg_g, avg_b),
            );

            if idx == d.current {
                dom::set_style(&thumb, "border-color", "#58a6ff");
            }

            dom::set_text(&thumb, &format!("{}", idx + 1));
            dom::set_style(&thumb, "font-size", "10px");
            dom::set_style(&thumb, "display", "flex");
            dom::set_style(&thumb, "align-items", "center");
            dom::set_style(&thumb, "justify-content", "center");
            dom::set_style(&thumb, "color", "#fff");

            dom::on_click(&thumb, move || {
                DESIGNER.with(|d| d.borrow_mut().current = idx);
                if let Some(strip) = dom::get_el("dsn-timeline") {
                    refresh_timeline_strip(&strip);
                }
                refresh_designer_leds();
            });

            dom::append(strip, &thumb);
        }
    });
}

fn avg_frame_color(colors: &[(u8, u8, u8); 13]) -> (u8, u8, u8) {
    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for &(cr, cg, cb) in colors.iter() {
        r += cr as u32;
        g += cg as u32;
        b += cb as u32;
    }
    ((r / 13) as u8, (g / 13) as u8, (b / 13) as u8)
}

fn build_custom_animation() -> LedAnimation {
    DESIGNER.with(|d| {
        let d = d.borrow();
        let frames: Vec<LedFrame> = d
            .frames
            .iter()
            .map(|f| LedFrame {
                colors: f.colors,
                duration_ms: f.duration_ms,
            })
            .collect();
        LedAnimation::Custom { frames }
    })
}
