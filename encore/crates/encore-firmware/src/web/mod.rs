//! Web server subsystem.
//!
//! HTTPS server on port 443 with private CA-signed TLS cert (runtime generated).
//! If that fails, falls back to the cert/key pair the firmware build installed
//! (see `fallback.rs`), then to a throwaway self-signed cert kept in memory.
//! The runtime cert's dates are checked again once NTP has set the clock, and
//! daily after that (see `certstore.rs`); a renewed cert is swapped in live.
//! HTTP on port 80 serves `/ca.crt` for trust installation, redirects rest to HTTPS.
//! JSON WebSocket at `/ws` for real-time ServerMsg/ClientMsg.
//! 1Hz system telemetry broadcast. Captive portal redirect when in AP mode.

pub mod api;
mod certstore;
mod fallback;
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
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};

const DEFAULT_HTTPS_PORT: u16 = 443;
const HTTP_REDIRECT_PORT: u16 = 80;

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
            .route("/api/factory-reset", post(api::factory_reset_handler))
            .route("/ca.crt", get(tls::ca_cert_handler))
            .route("/ws", get(ws::ws_handler))
            .fallback(static_handler)
            .with_state(state.clone());

        // Mount debug routes if debug state is available (Linux only).
        // Nested BEFORE the layers below so the origin gate covers them too.
        if let Some(ds) = debug_state {
            app = app.nest("/api/debug", crate::debug::routes::router().with_state(ds));
        }

        // Origin gate + allowlisted CORS wrap every route above, including
        // the nested debug API and the WebSocket upgrade (which CORS alone
        // never protects).
        let app = app
            .layer(axum::middleware::from_fn(origin_gate))
            .layer(cors_layer());

        // Read device name from config for TLS cert generation
        let device_name = std::fs::read_to_string(CONFIG_PATH)
            .ok()
            .and_then(|s| toml::from_str::<encore_common::config::EncoreConfigFile>(&s).ok())
            .map(|c| c.device.name)
            .unwrap_or_else(|| "Encore".into());

        // Build TLS config: try the runtime CA-signed cert, then the build-provided
        // pair, then a throwaway in-memory cert
        let (tls_acceptor, served_cert) =
            build_tls_acceptor(&device_name).context("build TLS acceptor")?;
        let shared_tls = SharedAcceptor::new(tls_acceptor, served_cert);

        // A speaker has no battery-backed clock, so the certificate dates can only be
        // judged properly after NTP has run, which is after this point.
        tokio::spawn(renew_certs_when_due(
            shared_tls.clone(),
            device_name.clone(),
            ctx.shutdown.resubscribe(),
        ));

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
            let http_state = state;
            tokio::spawn(async move {
                if let Err(e) = run_http_portal(https_port, shutdown_redir, http_state).await {
                    warn!("HTTP portal server failed: {}", e);
                }
            });
        }

        let tls_listener = TlsListener {
            tcp: listener,
            acceptor: shared_tls,
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

/// Build a TLS acceptor. Tries the runtime CA-signed cert first, then the fallbacks.
/// Also returns the PEM of the certificate it presents, which is empty for a fallback,
/// so the renewal task can tell when the runtime certificate has changed.
fn build_tls_acceptor(device_name: &str) -> Result<(tokio_rustls::TlsAcceptor, Vec<u8>)> {
    let runtime = tls::ensure_certs(device_name).and_then(|(_ca_pem, cert_pem, key_pem)| {
        build_acceptor_from_pem(&cert_pem, &key_pem).map(|acceptor| (acceptor, cert_pem))
    });
    match runtime {
        Ok(served) => {
            info!("TLS: using runtime CA-signed certificate");
            Ok(served)
        }
        Err(e) => {
            warn!("TLS: runtime cert failed ({}), using a fallback", e);
            let hostname = crate::network::sanitize_hostname(device_name);
            let (acceptor, source) = fallback::choose(
                std::path::Path::new(fallback::BUILD_PAIR_DIR),
                &hostname,
                &tls::detect_local_ips(),
                build_acceptor_from_pem,
            )?;
            match source {
                fallback::FallbackSource::BuildPair => {
                    info!("TLS: using the build-provided certificate pair")
                }
                fallback::FallbackSource::Throwaway => {
                    info!("TLS: using a throwaway in-memory certificate")
                }
            }
            Ok((acceptor, Vec::new()))
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
        .with_single_cert(cert_chain, key)
        .context("TLS single cert")?;

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The acceptor new connections use, and the certificate it presents, behind a lock so
/// the renewal task can swap in a new one. Connections already open keep what they had.
#[derive(Clone)]
struct SharedAcceptor(Arc<std::sync::RwLock<ServedTls>>);

struct ServedTls {
    acceptor: tokio_rustls::TlsAcceptor,
    /// PEM of the certificate the acceptor presents. Empty for a fallback.
    cert_pem: Vec<u8>,
}

impl SharedAcceptor {
    fn new(acceptor: tokio_rustls::TlsAcceptor, cert_pem: Vec<u8>) -> Self {
        Self(Arc::new(std::sync::RwLock::new(ServedTls {
            acceptor,
            cert_pem,
        })))
    }

    fn current(&self) -> tokio_rustls::TlsAcceptor {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .acceptor
            .clone()
    }

    fn is_serving(&self, cert_pem: &[u8]) -> bool {
        self.0.read().unwrap_or_else(|e| e.into_inner()).cert_pem == cert_pem
    }

    fn replace(&self, acceptor: tokio_rustls::TlsAcceptor, cert_pem: Vec<u8>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = ServedTls { acceptor, cert_pem };
    }
}

/// How often the certificate dates are checked once the clock has been set.
const CERT_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Check the certificate dates again once NTP has set the clock, and then daily, and
/// switch to a renewed certificate without restarting. The check at startup can only
/// judge dates by the firmware's build time.
async fn renew_certs_when_due(
    shared: SharedAcceptor,
    device_name: String,
    mut shutdown: broadcast::Receiver<()>,
) {
    while !crate::network::ntp::is_time_synced() {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
            _ = shutdown.recv() => return,
        }
    }
    loop {
        let name = device_name.clone();
        match tokio::task::spawn_blocking(move || tls::renew_certs(&name)).await {
            Ok(Ok((_ca_pem, cert_pem, key_pem))) => {
                if !shared.is_serving(&cert_pem) {
                    match build_acceptor_from_pem(&cert_pem, &key_pem) {
                        Ok(acceptor) => {
                            shared.replace(acceptor, cert_pem);
                            info!("TLS: now serving a renewed certificate");
                        }
                        Err(e) => warn!("TLS: renewed certificate rejected: {:#}", e),
                    }
                }
            }
            Ok(Err(e)) => warn!("TLS: certificate renewal check failed: {:#}", e),
            Err(e) => warn!("TLS: certificate renewal check did not finish: {}", e),
        }
        tokio::select! {
            _ = tokio::time::sleep(CERT_CHECK_INTERVAL) => {}
            _ = shutdown.recv() => return,
        }
    }
}

/// TLS-wrapping listener that implements axum's Listener trait.
struct TlsListener {
    tcp: tokio::net::TcpListener,
    acceptor: SharedAcceptor,
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
                let acceptor = self.acceptor.current();
                match acceptor.accept(stream).await {
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
/// Plain HTTP is only for links that cannot do TLS: AP clients mid-setup
/// (192.168.43.0/24) and the USB RNDIS admin link (10.55.55.0/24), where
/// there is no trusted-cert path and a 308 to HTTPS would strand the browser
/// on the self-signed cert. Those peers get the full dashboard + API over
/// HTTP, exactly as before. Everyone else gets `/ca.crt` for trust bootstrap
/// and a 308 to HTTPS for everything else, so the API (config, OTA flash,
/// /ws) never runs cleartext on the LAN.
async fn run_http_portal(
    https_port: u16,
    mut shutdown: broadcast::Receiver<()>,
    app_state: Arc<AppState>,
) -> Result<()> {
    // Build the full app router (same as HTTPS but over HTTP). The
    // http_local_gate layer below keeps these routes off the LAN.
    let mut http_app = Router::new()
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
        .route("/api/factory-reset", post(api::factory_reset_handler))
        .route("/ca.crt", get(tls::ca_cert_handler))
        .route("/ws", get(ws::ws_handler))
        .fallback(portal_fallback)
        .with_state(app_state.clone());

    // Mount debug routes on HTTP portal too — before the layers below so the
    // local-link gate and origin gate cover them.
    if let Some(ds) = &app_state.debug_state {
        http_app = http_app.nest(
            "/api/debug",
            crate::debug::routes::router().with_state(ds.clone()),
        );
    }

    let http_app = http_app
        .layer(axum::middleware::from_fn(move |req, next| {
            http_local_gate(req, next, https_port)
        }))
        .layer(axum::middleware::from_fn(origin_gate))
        .layer(cors_layer());

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

    axum::serve(
        listener,
        http_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.recv().await.ok();
    })
    .await
    .context("HTTP portal server")?;

    Ok(())
}

/// Is this peer on a TLS-incapable device link (AP subnet, USB RNDIS, or
/// loopback)? Classified by the connection's peer address, NOT the Host
/// header (which any LAN client controls) and NOT the ap_active flag
/// (ap_keep_alive defaults to true, so the AP being up must not exempt the
/// whole LAN from the HTTPS upgrade).
fn on_local_link(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || matches!(v4.octets(), [192, 168, 43, _] | [10, 55, 55, _])
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Port-80 gate: local-link peers pass through (full dashboard + API over
/// HTTP), everyone else gets `/ca.crt` or a 308 to HTTPS.
async fn http_local_gate(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    https_port: u16,
) -> Response {
    let local = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        // No peer info — fail closed, upgrade to HTTPS.
        .is_some_and(|ci| on_local_link(ci.0.ip()));
    if local || req.uri().path() == "/ca.crt" {
        return next.run(req).await;
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("encore.local");
    let host = host.split(':').next().unwrap_or(host);
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

/// Port-80 fallback — only local-link traffic reaches this (the gate 308s
/// everything else). Bounce captive-portal probes from AP clients to the
/// dashboard; serve the embedded dashboard for the rest.
async fn portal_fallback(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
) -> Response {
    let from_ap = matches!(addr.ip(), std::net::IpAddr::V4(v4) if v4.octets()[..3] == [192, 168, 43]);
    if from_ap && is_captive_portal_probe(req.uri().path()) {
        return axum::response::Redirect::temporary("http://192.168.43.1/").into_response();
    }
    static_handler(req.uri().clone()).await
}

// ── Cross-origin policy ─────────────────────────────────────────────────
//
// The API has no auth — LAN reachability is the trust boundary — so the one
// control against a hostile web page riding the user's browser is the Origin
// header: attacker-visible but not forgeable from a page. Two pieces:
//
// - `origin_gate` rejects browser requests from unknown origins server-side.
//   This is what actually protects /ws (WebSocket upgrades ignore CORS) and
//   state-changing simple POSTs like /api/update (CORS never blocks the
//   request from being sent, only the response from being read).
// - `cors_layer` mirrors only allowlisted origins, replacing the old
//   `CorsLayer::permissive()` that let any page read API responses.

/// Host part of a "host[:port]" string.
fn host_only(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// Is this browser Origin allowed to use the API and WebSocket?
///
/// Allowed: the host this request was sent to (the dashboard served by this
/// device, over either scheme), the Tauri desktop/mobile app
/// (tauri.localhost / tauri://localhost), and localhost (the documented
/// `make wasm` browser dev loop). Everything else — including "null" — is
/// rejected.
fn origin_allowed(origin: &str, request_host: Option<&str>) -> bool {
    let authority = origin.split_once("://").map(|(_, a)| a).unwrap_or(origin);
    let ohost = host_only(authority);
    if ohost.is_empty() {
        return false;
    }
    if let Some(h) = request_host {
        if ohost.eq_ignore_ascii_case(host_only(h)) {
            return true;
        }
    }
    ohost.eq_ignore_ascii_case("tauri.localhost")
        || ohost.eq_ignore_ascii_case("localhost")
        || ohost == "127.0.0.1"
}

/// Reject browser requests from unknown origins. Requests without an Origin
/// header (curl, same-origin navigations, captive-portal probes) pass.
async fn origin_gate(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok());
        if !origin_allowed(origin, host) {
            warn!("Web: rejected cross-origin request from {}", origin);
            return axum::http::StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(req).await
}

/// CORS restricted to the same allowlist as `origin_gate`, so the Tauri app
/// and the localhost dev loop keep working while other pages get nothing.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, parts| {
            let host = parts
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok());
            origin.to_str().is_ok_and(|o| origin_allowed(o, host))
        }))
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([header::CONTENT_TYPE])
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

#[cfg(test)]
mod tests {
    use super::{on_local_link, origin_allowed};

    #[test]
    fn origin_allowed_same_host_any_scheme_or_port() {
        assert!(origin_allowed(
            "https://encore.local",
            Some("encore.local")
        ));
        assert!(origin_allowed(
            "http://192.168.1.50",
            Some("192.168.1.50:443")
        ));
        assert!(origin_allowed(
            "https://Encore.Local:8443",
            Some("encore.local")
        ));
    }

    #[test]
    fn origin_allowed_app_and_dev_loop() {
        assert!(origin_allowed("http://tauri.localhost", Some("192.168.1.50")));
        assert!(origin_allowed("tauri://localhost", Some("192.168.1.50")));
        assert!(origin_allowed("http://localhost:8765", Some("192.168.1.50")));
        assert!(origin_allowed("http://127.0.0.1:8765", Some("192.168.1.50")));
    }

    #[test]
    fn origin_allowed_rejects_hostile_pages() {
        assert!(!origin_allowed("https://evil.example", Some("192.168.1.50")));
        assert!(!origin_allowed("null", Some("192.168.1.50")));
        assert!(!origin_allowed("", Some("192.168.1.50")));
        // Attacker page named to look local must not match a real host
        assert!(!origin_allowed(
            "https://encore.local.evil.example",
            Some("encore.local")
        ));
        // Missing Host header: only the fixed allowlist passes
        assert!(!origin_allowed("https://encore.local", None));
        assert!(origin_allowed("http://localhost", None));
    }

    #[test]
    fn local_link_is_ap_rndis_loopback_only() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        assert!(on_local_link(IpAddr::V4(Ipv4Addr::new(192, 168, 43, 7))));
        assert!(on_local_link(IpAddr::V4(Ipv4Addr::new(10, 55, 55, 2))));
        assert!(on_local_link(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(on_local_link(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!on_local_link(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))));
        assert!(!on_local_link(IpAddr::V4(Ipv4Addr::new(10, 55, 56, 2))));
        assert!(!on_local_link(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }
}
