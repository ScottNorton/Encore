//! Firmware update panel — GitHub auto-update + manual drag-and-drop upload.

use crate::components::modal::Modal;
use crate::dom;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/ScottNorton/Encore/releases?per_page=5";
const MAX_ENCORE_SIZE: f64 = 16.0 * 1024.0 * 1024.0;
const MAX_ROOTFS_SIZE: f64 = 84_824_064.0;

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub fn render(container: &web_sys::Element) {
    // Current version
    let ver = dom::el(
        "div",
        "text-center mb-16",
        Some(&format!("Current firmware: v{}", encore_common::VERSION)),
    );
    dom::append(container, &ver);

    // GitHub update section (populated async)
    let update_section = dom::create_div();
    update_section.set_id("github-update");
    dom::append(container, &update_section);

    // Separator between GitHub update and manual upload (hidden until update found)
    let sep = dom::create_div();
    sep.set_id("upload-separator");
    dom::set_class(&sep, "text-center text-muted text-sm");
    dom::set_style(&sep, "margin", "20px 0");
    dom::set_style(&sep, "display", "none");
    dom::set_text(&sep, "\u{2014} or upload manually \u{2014}");
    dom::append(container, &sep);

    // Upload area (click or drag-and-drop)
    let upload = dom::create_div();
    upload.set_id("upload-area");
    dom::set_class(&upload, "card text-center");
    dom::set_style(&upload, "border", "2px dashed var(--border)");
    dom::set_style(&upload, "padding", "32px");
    dom::set_style(&upload, "cursor", "pointer");
    dom::set_style(&upload, "transition", "border-color 0.2s, background 0.2s");

    let icon = dom::el("div", "mb-8", None);
    dom::set_style(&icon, "font-size", "32px");
    dom::set_style(&icon, "opacity", "0.5");
    icon.set_inner_html("&#128190;"); // floppy disk
    dom::append(&upload, &icon);

    dom::append(
        &upload,
        &dom::el(
            "div",
            "",
            Some("Drop firmware file here, or click to browse"),
        ),
    );
    dom::append(
        &upload,
        &dom::el(
            "div",
            "text-sm text-muted mt-8",
            Some("SquashFS rootfs (.squashfs) or Encore binary (ELF)"),
        ),
    );

    // Hidden file input
    let file_input = dom::create_el("input");
    dom::set_attr(&file_input, "type", "file");
    dom::set_style(&file_input, "display", "none");
    file_input.set_id("fw-file-input");
    dom::append(&upload, &file_input);

    // Click upload area -> trigger file picker
    dom::on_click(&upload, || {
        if let Some(input) = dom::get_el("fw-file-input") {
            if let Some(input) = input.dyn_ref::<web_sys::HtmlInputElement>() {
                input.click();
            }
        }
    });

    // File selection handler
    let onchange = Closure::wrap(Box::new(|_: web_sys::Event| {
        if let Some(input) = dom::get_el("fw-file-input") {
            if let Some(input) = input.dyn_ref::<web_sys::HtmlInputElement>() {
                if let Some(files) = input.files() {
                    if let Some(file) = files.get(0) {
                        handle_file(file);
                    }
                }
                input.set_value(""); // allow re-selecting same file
            }
        }
    }) as Box<dyn FnMut(_)>);
    if let Some(input) = dom::get_el("fw-file-input") {
        input
            .add_event_listener_with_callback("change", onchange.as_ref().unchecked_ref())
            .ok();
    }
    onchange.forget();

    // Drag-and-drop handlers
    {
        let ondragover = Closure::wrap(Box::new(|e: web_sys::DragEvent| {
            e.prevent_default();
        }) as Box<dyn FnMut(_)>);
        upload
            .add_event_listener_with_callback("dragover", ondragover.as_ref().unchecked_ref())
            .ok();
        ondragover.forget();

        let ondragenter = Closure::wrap(Box::new(|e: web_sys::DragEvent| {
            e.prevent_default();
            if let Some(area) = dom::get_el("upload-area") {
                dom::set_style(&area, "border-color", "var(--accent)");
                dom::set_style(&area, "background", "var(--accent-soft)");
            }
        }) as Box<dyn FnMut(_)>);
        upload
            .add_event_listener_with_callback("dragenter", ondragenter.as_ref().unchecked_ref())
            .ok();
        ondragenter.forget();

        let ondragleave = Closure::wrap(Box::new(|e: web_sys::DragEvent| {
            e.prevent_default();
            if let Some(area) = dom::get_el("upload-area") {
                dom::set_style(&area, "border-color", "var(--border)");
                dom::set_style(&area, "background", "");
            }
        }) as Box<dyn FnMut(_)>);
        upload
            .add_event_listener_with_callback("dragleave", ondragleave.as_ref().unchecked_ref())
            .ok();
        ondragleave.forget();

        let ondrop = Closure::wrap(Box::new(|e: web_sys::DragEvent| {
            e.prevent_default();
            if let Some(area) = dom::get_el("upload-area") {
                dom::set_style(&area, "border-color", "var(--border)");
                dom::set_style(&area, "background", "");
            }
            if let Some(dt) = e.data_transfer() {
                if let Some(files) = dt.files() {
                    if let Some(file) = files.get(0) {
                        handle_file(file);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        upload
            .add_event_listener_with_callback("drop", ondrop.as_ref().unchecked_ref())
            .ok();
        ondrop.forget();
    }

    dom::append(container, &upload);

    // Progress area (hidden initially)
    let progress = dom::create_div();
    progress.set_id("upload-progress");
    dom::set_style(&progress, "display", "none");
    dom::set_class(&progress, "mt-16");

    let bar_track = dom::create_div();
    dom::set_class(&bar_track, "bar-track");
    dom::set_style(&bar_track, "height", "12px");
    let bar_fill = dom::create_div();
    bar_fill.set_id("upload-fill");
    dom::set_class(&bar_fill, "bar-fill");
    dom::set_style(&bar_fill, "background", "var(--accent)");
    dom::set_style(&bar_fill, "width", "0%");
    dom::set_style(&bar_fill, "transition", "width 0.3s ease");
    dom::append(&bar_track, &bar_fill);
    dom::append(&progress, &bar_track);

    let status = dom::create_div();
    status.set_id("upload-status");
    dom::set_class(&status, "text-center mt-8 text-sm");
    dom::append(&progress, &status);

    // Try Again button (shown on error)
    let retry_wrap = dom::create_div();
    retry_wrap.set_id("upload-retry");
    dom::set_style(&retry_wrap, "display", "none");
    dom::set_style(&retry_wrap, "text-align", "center");
    dom::set_class(&retry_wrap, "mt-12");
    let retry_btn = dom::el("button", "btn", Some("Try Again"));
    dom::on_click(&retry_btn, reset_upload_ui);
    dom::append(&retry_wrap, &retry_btn);
    dom::append(&progress, &retry_wrap);

    dom::append(container, &progress);

    // Check for GitHub updates
    check_for_updates();
}

// ── GitHub auto-update ──────────────────────────────────────────────────────

fn check_for_updates() {
    wasm_bindgen_futures::spawn_local(async {
        let result: Result<Vec<GithubRelease>, &str> = async {
            let window = dom::window();
            let resp_val = JsFuture::from(window.fetch_with_str(GITHUB_RELEASES_URL))
                .await
                .map_err(|_| "fetch failed")?;
            let resp: web_sys::Response = resp_val.unchecked_into();
            if !resp.ok() {
                return Err("GitHub API error");
            }
            let text_val = JsFuture::from(resp.text().map_err(|_| "text() failed")?)
                .await
                .map_err(|_| "text await failed")?;
            let text = text_val.as_string().ok_or("not a string")?;
            serde_json::from_str(&text).map_err(|_| "JSON parse failed")
        }
        .await;

        let releases = match result {
            Ok(r) => r,
            Err(_) => return, // silently fail — user can still upload manually
        };

        let current = encore_common::VERSION;

        // Find newest non-draft, non-prerelease with a higher version and assets
        let latest = releases
            .iter()
            .filter(|r| !r.draft && !r.prerelease && !r.assets.is_empty())
            .find(|r| version_newer(&r.tag_name, current));

        if let Some(release) = latest {
            let asset = release
                .assets
                .iter()
                .find(|a| a.name == "encore")
                .or_else(|| release.assets.first());

            if let Some(asset) = asset {
                show_update_available(release, asset);
            }
        }
    });
}

fn version_newer(remote_tag: &str, local: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };
    let r = parse(remote_tag);
    let l = parse(local);
    r > l
}

fn show_update_available(release: &GithubRelease, asset: &GithubAsset) {
    let section = match dom::get_el("github-update") {
        Some(s) => s,
        None => return,
    };

    // Show the separator between GitHub update and manual upload
    if let Some(sep) = dom::get_el("upload-separator") {
        dom::set_style(&sep, "display", "block");
    }

    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::set_style(&card, "border-left", "3px solid var(--green)");

    let title = dom::el(
        "div",
        "card-title",
        Some(&format!("Update available: {}", release.tag_name)),
    );
    dom::set_style(&title, "color", "var(--green)");
    dom::append(&card, &title);

    if !release.body.is_empty() {
        // Show first ~300 chars of release notes
        let notes: String = release.body.chars().take(300).collect();
        let notes = if release.body.chars().count() > 300 {
            format!("{}...", notes)
        } else {
            notes
        };
        let notes_el = dom::el("div", "text-sm text-secondary", Some(&notes));
        dom::set_style(&notes_el, "margin", "8px 0 16px");
        dom::set_style(&notes_el, "line-height", "1.5");
        dom::set_style(&notes_el, "white-space", "pre-wrap");
        dom::append(&card, &notes_el);
    }

    let size_mb = asset.size as f64 / (1024.0 * 1024.0);
    let size_text = dom::el(
        "div",
        "text-sm text-muted",
        Some(&format!("{:.1} MB", size_mb)),
    );
    dom::append(&card, &size_text);

    let btn = dom::el("button", "btn btn-primary mt-12", Some("Install Update"));
    let url = asset.browser_download_url.clone();
    let version = release.tag_name.clone();
    dom::on_click(&btn, move || {
        install_github_update(&url, &version);
    });
    dom::append(&card, &btn);

    dom::append(&section, &card);
}

fn install_github_update(url: &str, version: &str) {
    let msg = format!(
        "Download and install {}?\n\nThe device will reboot after installing.",
        version
    );
    let url = url.to_string();
    let version = version.to_string();

    Modal::confirm(
        &msg,
        "Install",
        move || do_github_install(url.clone(), version.clone()),
        || {},
    );
}

fn do_github_install(url: String, version: String) {
    show_progress_ui();
    set_upload_status(&format!("Downloading {}...", version));

    wasm_bindgen_futures::spawn_local(async move {
        // Download binary from GitHub
        let window = dom::window();
        let resp_val = match JsFuture::from(window.fetch_with_str(&url)).await {
            Ok(v) => v,
            Err(_) => {
                show_upload_error("Download failed: network error");
                return;
            }
        };
        let resp: web_sys::Response = resp_val.unchecked_into();
        if !resp.ok() {
            show_upload_error(&format!("Download failed: HTTP {}", resp.status()));
            return;
        }

        set_progress(30);

        let buffer = match resp.array_buffer() {
            Ok(promise) => match JsFuture::from(promise).await {
                Ok(buf) => buf,
                Err(_) => {
                    show_upload_error("Failed to read download");
                    return;
                }
            },
            Err(_) => {
                show_upload_error("Failed to read download");
                return;
            }
        };

        // Verify ELF magic
        let preview = js_sys::Uint8Array::new(&buffer);
        if preview.length() < 4 {
            show_upload_error("Downloaded file too small");
            return;
        }
        let magic = [
            preview.get_index(0),
            preview.get_index(1),
            preview.get_index(2),
            preview.get_index(3),
        ];
        if magic != [0x7f, 0x45, 0x4c, 0x46] {
            show_upload_error("Downloaded file is not a valid ELF binary");
            return;
        }

        set_progress(50);
        set_upload_status("Uploading to device...");

        // Upload to device via XHR with progress
        let origin = crate::dom::api_origin();
        let upload_url = format!("{}/api/update", origin);

        let xhr = match web_sys::XmlHttpRequest::new() {
            Ok(x) => x,
            Err(_) => {
                show_upload_error("Failed to create upload request");
                return;
            }
        };
        if xhr.open_with_async("POST", &upload_url, true).is_err() {
            show_upload_error("Failed to open connection");
            return;
        }

        // Upload progress (50-99% of total bar)
        if let Ok(upload_target) = xhr.upload() {
            let ver = version.clone();
            let onprogress = Closure::wrap(Box::new(move |e: web_sys::ProgressEvent| {
                if e.length_computable() && e.total() > 0.0 {
                    let upload_pct = (e.loaded() / e.total() * 49.0) as u32;
                    set_progress(50 + upload_pct);
                    set_upload_status(&format!(
                        "Uploading {} to device ({:.1} / {:.1} MB)...",
                        ver,
                        e.loaded() / (1024.0 * 1024.0),
                        e.total() / (1024.0 * 1024.0),
                    ));
                }
            }) as Box<dyn FnMut(_)>);
            upload_target
                .add_event_listener_with_callback("progress", onprogress.as_ref().unchecked_ref())
                .ok();
            onprogress.forget();
        }

        let xhr2 = xhr.clone();
        let ver_done = version.clone();
        let onload = Closure::wrap(Box::new(move |_: web_sys::ProgressEvent| {
            let status = xhr2.status().unwrap_or(0);
            if (200..300).contains(&status) {
                show_upload_success(&format!("{} installed", ver_done));
            } else {
                let text = xhr2.response_text().ok().flatten().unwrap_or_default();
                let msg = if text.is_empty() {
                    format!("Upload failed: HTTP {}", status)
                } else {
                    format!("Upload failed: {}", text)
                };
                show_upload_error(&msg);
            }
        }) as Box<dyn FnMut(_)>);
        xhr.add_event_listener_with_callback("load", onload.as_ref().unchecked_ref())
            .ok();
        onload.forget();

        let onerror = Closure::wrap(Box::new(|_: web_sys::ProgressEvent| {
            show_upload_error("Upload to device failed: network error");
        }) as Box<dyn FnMut(_)>);
        xhr.add_event_listener_with_callback("error", onerror.as_ref().unchecked_ref())
            .ok();
        onerror.forget();

        // Send the ArrayBuffer
        let buf_obj: &js_sys::Object = buffer.unchecked_ref();
        if xhr.send_with_opt_buffer_source(Some(buf_obj)).is_err() {
            show_upload_error("Failed to start upload");
        }
    });
}

// ── Manual file upload ──────────────────────────────────────────────────────

/// Validate file header and size, then confirm before upload.
fn handle_file(file: web_sys::File) {
    let size = file.size();
    if size < 1024.0 {
        show_upload_error("File too small (< 1 KB)");
        return;
    }

    wasm_bindgen_futures::spawn_local(async move {
        // Read first 4 bytes for magic detection (don't buffer the whole file)
        let slice = match file.slice_with_f64_and_f64(0.0, 4.0) {
            Ok(s) => s,
            Err(_) => {
                show_upload_error("Failed to read file");
                return;
            }
        };
        let header_buf = match JsFuture::from(slice.array_buffer()).await {
            Ok(buf) => buf,
            Err(_) => {
                show_upload_error("Failed to read file");
                return;
            }
        };
        let header = js_sys::Uint8Array::new(&header_buf);
        if header.length() < 4 {
            show_upload_error("File too small");
            return;
        }
        let magic = [
            header.get_index(0),
            header.get_index(1),
            header.get_index(2),
            header.get_index(3),
        ];

        let (endpoint, label, max_size) = if magic == [0x68, 0x73, 0x71, 0x73] {
            ("/api/firmware/flash", "SquashFS rootfs", MAX_ROOTFS_SIZE)
        } else if magic == [0x7f, 0x45, 0x4c, 0x46] {
            ("/api/update", "Encore binary", MAX_ENCORE_SIZE)
        } else {
            show_upload_error("Unrecognized file type (expected SquashFS or ELF)");
            return;
        };

        if size > max_size {
            let max_mb = max_size / (1024.0 * 1024.0);
            show_upload_error(&format!("{} too large (max {:.0} MB)", label, max_mb));
            return;
        }

        let size_mb = size / (1024.0 * 1024.0);
        let warning = if endpoint == "/api/firmware/flash" {
            "This will flash the root filesystem and reboot."
        } else {
            "The binary will be staged and activated on reboot."
        };
        let msg = format!("Upload {} ({:.1} MB)?\n\n{}", label, size_mb, warning);
        let confirm_label = if endpoint == "/api/firmware/flash" {
            "Flash & Reboot"
        } else {
            "Upload & Reboot"
        };

        let endpoint = endpoint.to_string();
        let label = label.to_string();

        Modal::confirm(
            &msg,
            confirm_label,
            move || start_upload(&file, &endpoint, &label),
            || {},
        );
    });
}

/// Upload a local file to the device via XHR with real-time progress.
fn start_upload(file: &web_sys::File, endpoint: &str, label: &str) {
    show_progress_ui();

    let size_mb = file.size() / (1024.0 * 1024.0);
    set_upload_status(&format!("Uploading {} ({:.1} MB)...", label, size_mb));

    let origin = crate::dom::api_origin();
    let url = format!("{}{}", origin, endpoint);

    let xhr = match web_sys::XmlHttpRequest::new() {
        Ok(x) => x,
        Err(_) => {
            show_upload_error("Failed to create upload request");
            return;
        }
    };
    if xhr.open_with_async("POST", &url, true).is_err() {
        show_upload_error("Failed to open connection");
        return;
    }

    // Upload progress
    if let Ok(upload_target) = xhr.upload() {
        let label_p = label.to_string();
        let onprogress = Closure::wrap(Box::new(move |e: web_sys::ProgressEvent| {
            if e.length_computable() && e.total() > 0.0 {
                let pct = (e.loaded() / e.total() * 100.0) as u32;
                set_progress(pct.min(99)); // cap at 99% until server responds
                set_upload_status(&format!(
                    "Uploading {} ({:.1} / {:.1} MB)...",
                    label_p,
                    e.loaded() / (1024.0 * 1024.0),
                    e.total() / (1024.0 * 1024.0),
                ));
            }
        }) as Box<dyn FnMut(_)>);
        upload_target
            .add_event_listener_with_callback("progress", onprogress.as_ref().unchecked_ref())
            .ok();
        onprogress.forget();
    }

    // Load complete
    let xhr2 = xhr.clone();
    let label_done = label.to_string();
    let onload = Closure::wrap(Box::new(move |_: web_sys::ProgressEvent| {
        let status = xhr2.status().unwrap_or(0);
        if (200..300).contains(&status) {
            show_upload_success(&label_done);
        } else {
            let text = xhr2.response_text().ok().flatten().unwrap_or_default();
            let msg = if text.is_empty() {
                format!("Upload failed: HTTP {}", status)
            } else {
                format!("Upload failed: {}", text)
            };
            show_upload_error(&msg);
        }
    }) as Box<dyn FnMut(_)>);
    xhr.add_event_listener_with_callback("load", onload.as_ref().unchecked_ref())
        .ok();
    onload.forget();

    // Network error
    let onerror = Closure::wrap(Box::new(|_: web_sys::ProgressEvent| {
        show_upload_error("Upload failed: network error");
    }) as Box<dyn FnMut(_)>);
    xhr.add_event_listener_with_callback("error", onerror.as_ref().unchecked_ref())
        .ok();
    onerror.forget();

    // Send file directly (browser streams from disk — no JS-side buffering)
    let blob: &web_sys::Blob = file;
    if xhr.send_with_opt_blob(Some(blob)).is_err() {
        show_upload_error("Failed to start upload");
    }
}

// ── UI helpers ──────────────────────────────────────────────────────────────

/// Transition to progress display, hiding everything else.
fn show_progress_ui() {
    if let Some(area) = dom::get_el("upload-area") {
        dom::set_style(&area, "display", "none");
    }
    if let Some(sep) = dom::get_el("upload-separator") {
        dom::set_style(&sep, "display", "none");
    }
    if let Some(gh) = dom::get_el("github-update") {
        dom::set_style(&gh, "display", "none");
    }
    if let Some(prog) = dom::get_el("upload-progress") {
        dom::set_style(&prog, "display", "block");
    }
    if let Some(retry) = dom::get_el("upload-retry") {
        dom::set_style(&retry, "display", "none");
    }
    set_progress(0);
    if let Some(fill) = dom::get_el("upload-fill") {
        dom::set_style(&fill, "background", "var(--accent)");
    }
}

fn set_progress(pct: u32) {
    if let Some(fill) = dom::get_el("upload-fill") {
        dom::set_style(&fill, "width", &format!("{}%", pct));
    }
}

fn set_upload_status(msg: &str) {
    if let Some(el) = dom::get_el("upload-status") {
        dom::set_text(&el, msg);
        dom::set_style(&el, "color", "");
    }
}

fn show_upload_success(label: &str) {
    set_progress(100);
    if let Some(fill) = dom::get_el("upload-fill") {
        dom::set_style(&fill, "background", "var(--green)");
    }

    let l = label.to_string();
    set_upload_status(&format!("{} \u{2014} rebooting in 3s...", l));

    let l2 = l.clone();
    dom::set_timeout(
        move || {
            set_upload_status(&format!("{} \u{2014} rebooting in 2s...", l2));
            let l3 = l.clone();
            dom::set_timeout(
                move || {
                    set_upload_status(&format!("{} \u{2014} rebooting in 1s...", l3));
                    dom::set_timeout(
                        || {
                            set_upload_status("Rebooting now...");
                        },
                        1000,
                    );
                },
                1000,
            );
        },
        1000,
    );
}

fn show_upload_error(msg: &str) {
    if let Some(prog) = dom::get_el("upload-progress") {
        dom::set_style(&prog, "display", "block");
    }
    if let Some(retry) = dom::get_el("upload-retry") {
        dom::set_style(&retry, "display", "block");
    }
    if let Some(el) = dom::get_el("upload-status") {
        dom::set_text(&el, msg);
        dom::set_style(&el, "color", "var(--red)");
    }
    set_progress(0);
    if let Some(fill) = dom::get_el("upload-fill") {
        dom::set_style(&fill, "background", "var(--accent)");
    }
}

fn reset_upload_ui() {
    if let Some(area) = dom::get_el("upload-area") {
        dom::set_style(&area, "display", "block");
    }
    // Restore GitHub update section if it had content
    if let Some(gh) = dom::get_el("github-update") {
        if gh.child_element_count() > 0 {
            dom::set_style(&gh, "display", "block");
            if let Some(sep) = dom::get_el("upload-separator") {
                dom::set_style(&sep, "display", "block");
            }
        }
    }
    if let Some(prog) = dom::get_el("upload-progress") {
        dom::set_style(&prog, "display", "none");
    }
    set_progress(0);
    if let Some(fill) = dom::get_el("upload-fill") {
        dom::set_style(&fill, "background", "var(--accent)");
    }
}
