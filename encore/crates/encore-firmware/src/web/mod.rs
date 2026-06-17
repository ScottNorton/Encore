//! Web server subsystem.
//!
//! HTTPS server on port 443 with private CA-signed TLS cert (runtime generated).
//! Falls back to embedded self-signed cert if runtime generation fails.
//! HTTP on port 80 serves `/ca.crt` for trust installation, redirects rest to HTTPS.
//! JSON WebSocket at `/ws` for real-time ServerMsg/ClientMsg.
//! 1Hz system telemetry broadcast. Captive portal redirect when in AP mode.

pub mod api;
pub mod tls;

pub mod log_layer;
pub mod ws;

use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use axum::http::{header, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use encore_common::protocol::{ServerMsg, SubsystemSnapshot, SubsystemState, SystemSnapshot};
use rust_embed::Embed;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

const DEFAULT_HTTPS_PORT: u16 = 443;
const HTTP_REDIRECT_PORT: u16 = 80;

/// Embedded TLS certificate and private key (self-signed fallback).
const CERT_PEM: &[u8] = include_bytes!("../../../../tls/cert.pem");
const KEY_PEM: &[u8] = include_bytes!("../../../../tls/key.pem");

const CONFIG_PATH: &str = "/lsync/encore/config.toml";

/// Embedded dashboard files (HTML/JS/CSS).
#[derive(Embed)]
#[folder = "../../web/"]
struct Assets;

/// Embedded branding assets (PNGs) from repo root branding/ directory.
/// Served under /branding/* URL path via the prefix.
#[derive(Embed)]
#[folder = "../../../branding/"]
#[prefix = "branding/"]
struct BrandingAssets;

/// Shared state for axum handlers.
pub struct AppState {
    /// Broadcast JSON-serialized ServerMsg strings to all WebSocket clients.
    pub ws_tx: broadcast::Sender<String>,
    /// Receive deserialized ClientMsg from WebSocket clients.
    pub client_tx: mpsc::Sender<encore_common::protocol::ClientMsg>,
    /// Whether the AP is currently active (for captive portal behavior).
    pub ap_active: Arc<std::sync::atomic::AtomicBool>,
    /// Hardware debug tools (GPIO, UART) — REST API state.
    pub debug_state: Option<Arc<crate::debug::DebugState>>,
    /// True when running fallback binary after OTA crash.
    pub safe_mode: bool,
    /// Boot source: "next", "lsync", "rootfs", "dev", or "unknown".
    pub boot_source: String,
    /// Cached WiFi connect result for reconnecting clients (set by network subsystem).
    pub wifi_result_cache: crate::network::WifiResultCache,
    /// Cached network state for reconnecting clients (set by network subsystem).
    pub network_state_cache: crate::network::NetworkStateCache,
}

/// Web server subsystem.
pub struct WebSubsystem {
    port: u16,
    /// Receiver end — manager takes this to route ClientMsg to subsystems.
    client_rx: Option<mpsc::Receiver<encore_common::protocol::ClientMsg>>,
    /// Sender end — cloned into AppState for WebSocket handlers.
    client_tx: mpsc::Sender<encore_common::protocol::ClientMsg>,
    /// Subsystem status receiver — taken by run() for broadcasting.
    status_rx: Option<mpsc::Receiver<SubsystemSnapshot>>,
    /// Pre-created broadcast channel (shared with main for config routing).
    ws_tx_pre: Option<broadcast::Sender<String>>,
    /// Whether the AP is currently active (shared with network subsystem).
    ap_active: Arc<std::sync::atomic::AtomicBool>,
    /// Hardware debug tools state (GPIO, UART).
    debug_state: Option<Arc<crate::debug::DebugState>>,
    /// True when running fallback binary after OTA crash.
    safe_mode: bool,
    /// Boot source: "next", "lsync", "rootfs", "dev", or "unknown".
    boot_source: String,
    /// Cached WiFi connect result (shared with network subsystem).
    wifi_result_cache: crate::network::WifiResultCache,
    /// Cached network state (shared with network subsystem).
    network_state_cache: crate::network::NetworkStateCache,
}

impl WebSubsystem {
    pub fn new(port: Option<u16>) -> Self {
        let (client_tx, client_rx) = mpsc::channel(256);
        Self {
            port: port.unwrap_or(DEFAULT_HTTPS_PORT),
            client_rx: Some(client_rx),
            client_tx,
            status_rx: None,
            ws_tx_pre: None,
            ap_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            debug_state: None,
            safe_mode: false,
            boot_source: "unknown".to_string(),
            wifi_result_cache: Arc::new(std::sync::Mutex::new(None)),
            network_state_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Set the shared AP active flag (from network subsystem).
    pub fn set_ap_active(&mut self, ap_active: Arc<std::sync::atomic::AtomicBool>) {
        self.ap_active = ap_active;
    }

    /// Set the debug tools state (GPIO, UART).
    pub fn set_debug_state(&mut self, state: Arc<crate::debug::DebugState>) {
        self.debug_state = Some(state);
    }

    /// Set a pre-created broadcast sender so main.rs can share it
    /// for sending ConfigLoaded responses.
    pub fn set_ws_tx(&mut self, tx: broadcast::Sender<String>) {
        self.ws_tx_pre = Some(tx);
    }

    /// Take the ClientMsg receiver. The SubsystemManager calls this
    /// to route dashboard commands to subsystem channels.
    pub fn take_client_rx(&mut self) -> Option<mpsc::Receiver<encore_common::protocol::ClientMsg>> {
        self.client_rx.take()
    }

    /// Set safe mode flag (OTA binary crashed, running fallback).
    pub fn set_safe_mode(&mut self, safe_mode: bool) {
        self.safe_mode = safe_mode;
    }

    /// Set boot source ("next", "lsync", "rootfs", "dev", "unknown").
    pub fn set_boot_source(&mut self, source: String) {
        self.boot_source = source;
    }

    /// Give the subsystem status receiver for broadcasting to dashboard.
    pub fn set_status_rx(&mut self, rx: mpsc::Receiver<SubsystemSnapshot>) {
        self.status_rx = Some(rx);
    }

    /// Set the shared WiFi result cache (from network subsystem).
    pub fn set_wifi_result_cache(&mut self, cache: crate::network::WifiResultCache) {
        self.wifi_result_cache = cache;
    }

    /// Set the shared network state cache (from network subsystem).
    pub fn set_network_state_cache(&mut self, cache: crate::network::NetworkStateCache) {
        self.network_state_cache = cache;
    }
}

#[async_trait::async_trait]
impl Subsystem for WebSubsystem {
    fn name(&self) -> &'static str {
        "web"
    }

    async fn run(&mut self, ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        let ws_tx = self
            .ws_tx_pre
            .take()
            .unwrap_or_else(|| broadcast::channel::<String>(256).0);
        let debug_state = self.debug_state.take();
        let state = Arc::new(AppState {
            ws_tx: ws_tx.clone(),
            client_tx: self.client_tx.clone(),
            ap_active: self.ap_active.clone(),
            debug_state: debug_state.clone(),
            safe_mode: self.safe_mode,
            boot_source: self.boot_source.clone(),
            wifi_result_cache: self.wifi_result_cache.clone(),
            network_state_cache: self.network_state_cache.clone(),
        });

        // Spawn 1Hz system status broadcaster
        let ws_tx_status = ws_tx.clone();
        let mut shutdown_status = ctx.shutdown.resubscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let uptime = api::read_uptime() as u64;
                        let cpu = api::compute_cpu_percent();
                        let (used, total) = api::read_mem_used_kb();
                        let (mem_total, mem_free, _available, mem_buffers, mem_cached) =
                            api::read_detailed_memory();
                        let snap = SystemSnapshot {
                            uptime_secs: uptime,
                            cpu_percent: cpu,
                            ram_used_kb: used,
                            ram_total_kb: total,
                            cores: api::read_cpu_cores(),
                            load_avg: api::read_load_avg(),
                            ram_free_kb: mem_free,
                            ram_buffers_kb: mem_buffers,
                            ram_cached_kb: mem_cached,
                            temperature_mc: api::read_temperature_mc(),
                            disks: api::read_disk_usage(),
                            net_interfaces: api::read_net_interfaces(),
                            process_count: api::read_process_count(),
                        };
                        let _ = mem_total; // used via read_mem_used_kb already
                        let msg = ServerMsg::SystemStatus(snap);
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = ws_tx_status.send(json);
                        }
                    }
                    _ = shutdown_status.recv() => break,
                }
            }
        });

        // Spawn subsystem status forwarder (if we have a receiver)
        if let Some(mut status_rx) = self.status_rx.take() {
            let ws_tx_sub = ws_tx.clone();
            let mut shutdown_sub = ctx.shutdown.resubscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        snap = status_rx.recv() => {
                            match snap {
                                Some(snap) => {
                                    let msg = ServerMsg::SubsystemStatus(snap);
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        let _ = ws_tx_sub.send(json);
                                    }
                                }
                                None => break,
                            }
                        }
                        _ = shutdown_sub.recv() => break,
                    }
                }
            });
        }

        let mut app = Router::new()
            .route("/api/system", get(api::system_handler))
            .route(
                "/api/config",
                get(api::config_handler).post(api::config_save_handler),
            )
            .route("/api/setup", get(api::setup_handler))
            .route("/api/setup/complete", post(api::setup_complete_handler))
            .route("/api/logs", get(api::logs_handler))
            .route("/api/crashes", get(api::crashes_handler))
            .route(
                "/api/crashes/{subsystem}",
                get(api::crashes_subsystem_handler),
            )
            .route(
                "/api/update",
                post(api::update_handler)
                    .layer(axum::extract::DefaultBodyLimit::max(api::MAX_UPDATE_SIZE)),
            )
            .route(
                "/api/firmware/flash",
                post(api::firmware_flash_handler).layer(axum::extract::DefaultBodyLimit::disable()),
            )
            .route("/api/wifi/scan", get(api::wifi_scan_handler))
            .route("/api/reboot", post(api::reboot_handler))
            .route("/ca.crt", get(tls::ca_cert_handler))
            .route("/ws", get(ws::ws_handler))
            .fallback(static_handler)
            .layer(CorsLayer::permissive())
            .with_state(state.clone());

        // Mount debug routes if debug state is available (Linux only)
        if let Some(ds) = debug_state {
            app = app.nest("/api/debug", crate::debug::routes::router().with_state(ds));
        }

        // Read device name from config for TLS cert generation
        let device_name = std::fs::read_to_string(CONFIG_PATH)
            .ok()
            .and_then(|s| toml::from_str::<encore_common::config::EncoreConfigFile>(&s).ok())
            .map(|c| c.device.name)
            .unwrap_or_else(|| "Encore".into());

        // Build TLS config — try runtime CA-signed cert, fall back to embedded
        let tls_acceptor = build_tls_acceptor(&device_name).context("build TLS acceptor")?;

        // Bind HTTPS on configured port (default 443)
        let socket = tokio::net::TcpSocket::new_v4().context("create HTTPS socket")?;
        socket.set_reuseaddr(true).ok();
        socket
            .bind(std::net::SocketAddr::from(([0, 0, 0, 0], self.port)))
            .context("bind HTTPS server")?;
        let listener = socket.listen(128).context("listen HTTPS server")?;

        info!("Web: HTTPS listening on port {}", self.port);

        // Spawn HTTP portal server on port 80 (if HTTPS port isn't 80)
        if self.port != HTTP_REDIRECT_PORT {
            let https_port = self.port;
            let shutdown_redir = ctx.shutdown.resubscribe();
            let ap_active = self.ap_active.clone();
            let http_state = state;
            tokio::spawn(async move {
                if let Err(e) =
                    run_http_portal(https_port, shutdown_redir, ap_active, http_state).await
                {
                    warn!("HTTP portal server failed: {}", e);
                }
            });
        }

        let tls_listener = TlsListener {
            tcp: listener,
            acceptor: tls_acceptor,
        };

        let mut shutdown = ctx.shutdown;
        axum::serve(tls_listener, app)
            .with_graceful_shutdown(async move {
                shutdown.recv().await.ok();
                info!("Web: shutdown signal received");
            })
            .await
            .context("HTTPS server")?;

        Ok(())
    }
}

/// Serve embedded static files. Falls back to index.html for SPA routing.
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // Check dashboard assets first, then branding assets
    let file = Assets::get(path).or_else(|| BrandingAssets::get(path));

    if let Some(file) = file {
        let mime = mime_for_path(path);
        let cache = cache_control_for(path);
        (
            [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
            file.data,
        )
            .into_response()
    } else if let Some(file) = Assets::get("index.html") {
        // SPA fallback — serve index.html for unmatched routes
        (
            [
                (header::CONTENT_TYPE, "text/html"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            file.data,
        )
            .into_response()
    } else {
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}

/// Map file extension to MIME type. Covers all types the dashboard uses.
fn mime_for_path(path: &str) -> &'static str {
    if path == "manifest.json" {
        return "application/manifest+json";
    }
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Cache control policy per file type.
/// SW and manifest must never be cached (browser needs latest for PWA updates).
/// Immutable assets (WASM, JS with hashed names) can be cached aggressively.
fn cache_control_for(path: &str) -> &'static str {
    match path {
        "sw.js" | "manifest.json" => "no-cache, no-store, must-revalidate",
        "index.html" => "no-cache",
        _ => match path.rsplit('.').next() {
            Some("wasm") | Some("js") => "public, max-age=86400, immutable",
            Some("svg") | Some("png") | Some("ico") => "public, max-age=604800",
            _ => "no-cache",
        },
    }
}

// ── TLS ─────────────────────────────────────────────────────────────────

/// Build a TLS acceptor. Tries runtime CA-signed cert first, falls back to embedded.
fn build_tls_acceptor(device_name: &str) -> Result<tokio_rustls::TlsAcceptor> {
    match tls::ensure_certs(device_name) {
        Ok((_ca_pem, cert_pem, key_pem)) => {
            info!("TLS: using runtime CA-signed certificate");
            build_acceptor_from_pem(&cert_pem, &key_pem)
        }
        Err(e) => {
            warn!("TLS: runtime cert failed ({}), using embedded fallback", e);
            build_acceptor_from_pem(CERT_PEM, KEY_PEM)
        }
    }
}

/// Build a TLS acceptor from PEM-encoded cert and key bytes.
fn build_acceptor_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<tokio_rustls::TlsAcceptor> {
    use std::io::BufReader;
    use tokio_rustls::rustls::ServerConfig;

    let cert_chain: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_pem))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse TLS certificate")?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_pem))
        .context("parse TLS private key")?
        .ok_or_else(|| anyhow::anyhow!("no private key in PEM"))?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key.into())
        .context("TLS single cert")?;

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// TLS-wrapping listener that implements axum's Listener trait.
struct TlsListener {
    tcp: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

#[allow(refining_impl_trait)]
impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send + '_ {
        async {
            loop {
                let (stream, addr) = match self.tcp.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::error!("TCP accept error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                match self.acceptor.accept(stream).await {
                    Ok(tls) => return (tls, addr),
                    Err(e) => {
                        tracing::debug!("TLS handshake failed from {}: {}", addr, e);
                        continue;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// Run HTTP server on port 80.
///
/// When AP is active: intercept captive portal detection URLs (redirect to
/// our dashboard), and serve the full WASM dashboard over HTTP (no HTTPS
/// redirect). This allows captive portal browsers (which can't handle
/// self-signed certs) to access the setup wizard.
///
/// When AP is inactive: redirect all HTTP to HTTPS (original behavior).
async fn run_http_portal(
    https_port: u16,
    mut shutdown: broadcast::Receiver<()>,
    ap_active: Arc<std::sync::atomic::AtomicBool>,
    app_state: Arc<AppState>,
) -> Result<()> {
    use std::sync::atomic::Ordering;

    let ap_flag = ap_active.clone();

    // Build the full app router (same as HTTPS but over HTTP)
    let http_app = Router::new()
        .route("/api/system", get(api::system_handler))
        .route(
            "/api/config",
            get(api::config_handler).post(api::config_save_handler),
        )
        .route("/api/setup", get(api::setup_handler))
        .route("/api/setup/complete", post(api::setup_complete_handler))
        .route("/api/logs", get(api::logs_handler))
        .route("/api/crashes", get(api::crashes_handler))
        .route(
            "/api/crashes/{subsystem}",
            get(api::crashes_subsystem_handler),
        )
        .route(
            "/api/update",
            post(api::update_handler)
                .layer(axum::extract::DefaultBodyLimit::max(api::MAX_UPDATE_SIZE)),
        )
        .route(
            "/api/firmware/flash",
            post(api::firmware_flash_handler).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/wifi/scan", get(api::wifi_scan_handler))
        .route("/api/reboot", post(api::reboot_handler))
        .route("/ca.crt", get(tls::ca_cert_handler))
        .route("/ws", get(ws::ws_handler))
        .fallback(move |req: axum::extract::Request| {
            let ap_on = ap_flag.load(Ordering::Relaxed);
            async move {
                // Serve the dashboard over plain HTTP (no HTTPS upgrade) when the
                // request targets one of the device's own link IPs: the AP
                // (192.168.43.1) or the USB RNDIS admin link (10.55.55.1). Those
                // links have no trusted-cert path, so a 308 -> HTTPS strands a
                // browser on the self-signed cert. This does NOT rely on the
                // ap_active flag, which is false when the AP was started by the
                // boot script rather than by Encore. WiFi/LAN still upgrade to TLS.
                let host = req
                    .headers()
                    .get("host")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("encore.local");
                let host = host.split(':').next().unwrap_or(host);
                let on_local_link = host == "192.168.43.1" || host == "10.55.55.1";
                if ap_on || on_local_link {
                    // Captive-portal probes only matter while the AP is up; bounce
                    // those to the dashboard. Everything else gets the dashboard.
                    if ap_on && is_captive_portal_probe(req.uri().path()) {
                        return axum::response::Redirect::temporary("http://192.168.43.1/")
                            .into_response();
                    }
                    // Serve the embedded dashboard over HTTP
                    return static_handler(req.uri().clone()).await;
                }
                // Not a device link and AP inactive — redirect to HTTPS
                let path = req
                    .uri()
                    .path_and_query()
                    .map(|pq| pq.as_str())
                    .unwrap_or("/");
                let url = if https_port == 443 {
                    format!("https://{}{}", host, path)
                } else {
                    format!("https://{}:{}{}", host, https_port, path)
                };
                axum::response::Redirect::permanent(&url).into_response()
            }
        })
        .layer(CorsLayer::permissive())
        .with_state(app_state.clone());

    // Mount debug routes on HTTP portal too
    let mut http_app = http_app;
    if let Some(ds) = &app_state.debug_state {
        http_app = http_app.nest(
            "/api/debug",
            crate::debug::routes::router().with_state(ds.clone()),
        );
    }

    let socket = tokio::net::TcpSocket::new_v4().context("create HTTP portal socket")?;
    socket.set_reuseaddr(true).ok();
    socket
        .bind(std::net::SocketAddr::from((
            [0, 0, 0, 0],
            HTTP_REDIRECT_PORT,
        )))
        .context("bind HTTP portal")?;
    let listener = socket.listen(128).context("listen HTTP portal")?;

    info!("Web: HTTP portal on port {}", HTTP_REDIRECT_PORT);

    axum::serve(listener, http_app)
        .with_graceful_shutdown(async move {
            shutdown.recv().await.ok();
        })
        .await
        .context("HTTP portal server")?;

    Ok(())
}

/// Check if a request path is a known captive portal detection probe.
fn is_captive_portal_probe(path: &str) -> bool {
    matches!(
        path,
        // Android
        "/generate_204" | "/gen_204" |
        // Apple
        "/hotspot-detect.html" |
        // Windows
        "/ncsi.txt" | "/connecttest.txt" |
        // Firefox
        "/success.txt"
    )
}
