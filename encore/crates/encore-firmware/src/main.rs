//! Encore firmware — main entry point for the Harman Kardon Invoke community firmware.
//!
//! Initializes hardware (MCU, DAC, DSP, IO expander), starts subsystems
//! (audio, Spotify, Bluetooth, Wyoming, web server, network), and runs
//! the WebSocket command dispatch loop.

// Works around rustc 1.93.1 ICE in check_mod_deathness on cross-compile.
#![allow(dead_code)]

mod audio;
#[cfg(target_os = "linux")]
mod bluetooth;
#[cfg(target_os = "linux")]
mod debug;
mod debugger;
mod group;
#[cfg(target_os = "linux")]
mod homeassistant;
#[cfg(target_os = "linux")]
mod led;
mod manager;
#[cfg(target_os = "linux")]
mod mcu;
#[cfg(target_os = "linux")]
mod network;
#[cfg(target_os = "linux")]
mod spotify;
mod subsystem;
#[cfg(target_os = "linux")]
mod vpn;
mod wakeword;
#[cfg(target_os = "linux")]
mod watchdog;
#[cfg(target_os = "linux")]
mod web;
mod wyoming;

use manager::SubsystemManager;
use tracing::info;
#[cfg(target_os = "linux")]
use tracing::warn;

/// Local-time formatter for tracing logs.
/// Uses libc::localtime_r (thread-safe) to read /etc/localtime.
#[cfg(target_os = "linux")]
struct LocalTime;

#[cfg(target_os = "linux")]
impl tracing_subscriber::fmt::time::FormatTime for LocalTime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        #[allow(deprecated)]
        let secs = now.as_secs() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        write!(
            w,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        )
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    // On Linux, create the WebSocket broadcast channel BEFORE tracing init
    // so the log layer can capture all events from the start.
    #[cfg(target_os = "linux")]
    let ws_tx = {
        use tracing_subscriber::filter::LevelFilter;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tracing_subscriber::Layer as _;

        let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel::<String>(256);

        let (log_layer, log_rx) = web::log_layer::WsBroadcastLayer::new();
        web::log_layer::spawn_log_broadcaster(log_rx, ws_tx.clone());

        // Persistent file log to /lsync/encore/encore.log (survives watchdog reboot).
        // Truncate on each start to avoid filling yaffs2 — the previous run's
        // logs are the ones we lose, but a crash at the END of a run is what
        // we need to capture, and this log captures it.
        let file_layer = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("/lsync/encore/encore.log")
        {
            Ok(file) => {
                let writer = std::sync::Mutex::new(file);
                Some(tracing_subscriber::fmt::layer()
                    .with_timer(LocalTime)
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(LevelFilter::INFO))
            }
            Err(e) => {
                eprintln!("Warning: could not open /lsync/encore/encore.log: {}", e);
                None
            }
        };

        // All layers filter to INFO+ so the registry sets the global max
        // to INFO — TRACE/DEBUG events are never even created.  This is
        // critical on the dual-core Cortex-A7: without this filter, tokio/
        // hyper/rustls TRACE events caused 100% CPU and a watchdog reboot.
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_timer(LocalTime).with_filter(LevelFilter::INFO))
            .with(log_layer.with_filter(LevelFilter::INFO))
            .with(file_layer)
            .init();

        ws_tx
    };

    #[cfg(not(target_os = "linux"))]
    tracing_subscriber::fmt::init();

    info!("Encore v{} starting", encore_common::VERSION);

    // Check if supervisor flagged safe mode (OTA binary crashed, running fallback)
    #[cfg(target_os = "linux")]
    let safe_mode = std::fs::read_to_string("/lsync/encore/safe_mode")
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    #[cfg(not(target_os = "linux"))]
    let safe_mode = false;

    // Detect boot source from /proc/self/exe symlink
    #[cfg(target_os = "linux")]
    let boot_source = std::fs::read_link("/proc/self/exe")
        .ok()
        .and_then(|p| p.to_str().map(|s| match s {
            "/lsync/encore/encore_next" => "next",
            "/lsync/encore/encore" => "lsync",
            "/usr/bin/encore" => "rootfs",
            _ => "unknown",
        }.to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    #[cfg(not(target_os = "linux"))]
    let boot_source = "dev".to_string();

    if safe_mode {
        info!("SAFE MODE: running fallback binary (OTA binary crashed)");
    }
    info!("Boot source: {}", boot_source);

    #[allow(unused_mut)]
    let mut mgr = SubsystemManager::new();
    #[cfg(target_os = "linux")]
    mgr.set_ws_tx(ws_tx.clone());
    let shutdown = mgr.shutdown_handle();
    let mut shutdown_rx = mgr.shutdown_handle().subscribe();

    // Signal handler — sends shutdown on first Ctrl+C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Received shutdown signal");
        let _ = shutdown.send(());
    });

    #[cfg(target_os = "linux")]
    {
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::mpsc;

        // ── MCU (I2C hardware driver, shared with LED subsystem) ──
        let mcu = match mcu::Mcu::open() {
            Ok(mut m) => {
                info!("MCU: opened I2C bus");
                if let Err(e) = m.init() {
                    warn!("MCU: init failed ({}), continuing anyway", e);
                }
                Some(Arc::new(std::sync::Mutex::new(m)))
            }
            Err(e) => {
                warn!("MCU: failed to open ({}), LED subsystem disabled", e);
                None
            }
        };

        // ── Prepare web dashboard channels ──
        // ws_tx was created above (before tracing init) so the log layer
        // can broadcast from the very first event.
        let ap_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut web = web::WebSubsystem::new(None);
        web.set_ws_tx(ws_tx.clone());
        web.set_ap_active(ap_active.clone());
        web.set_safe_mode(safe_mode);
        web.set_boot_source(boot_source.clone());
        let debug_state = Arc::new(debug::DebugState::new());
        web.set_debug_state(debug_state);
        let client_rx = web.take_client_rx();
        let status_rx = mgr.take_status_rx();
        web.set_status_rx(status_rx);

        // ── Audio subsystem (mixer thread + hardware init) ──
        let (audio_cmd_tx, audio_cmd_rx) = mpsc::channel::<audio::subsystem::AudioCmd>(32);
        let mut audio = audio::subsystem::AudioSubsystem::new(audio_cmd_rx);
        audio.set_ws_tx(ws_tx.clone());
        {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            audio.apply_power_config(&cfg.audio);
        }

        // Create MixerSlots for each audio consumer BEFORE starting audio subsystem
        let spotify_slot = audio.add_source().slot;
        let bt_slot = audio.add_source().slot;
        let wyoming_slot = audio.add_source().slot;
        let network_slot = audio.add_source().slot; // group follower writes here

        // Network tap: mixer thread writes post-DRC audio here when active (leader reads)
        let network_tap = crate::audio::mixer::MixerSlot::new();
        let tap_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        audio.set_network_tap(network_tap.clone(), tap_active.clone());

        // Create capture consumers for mic audio (16kHz mono S16 from DSP left channel)
        let wyoming_mic = audio.add_capture_consumer(audio::capture::CaptureChannel::Left);
        let wakeword_mic = audio.add_capture_consumer(audio::capture::CaptureChannel::Left);

        // Mic monitor consumers (one per beamformed channel for VU meters)
        let mic_monitor_l = audio.add_capture_consumer(audio::capture::CaptureChannel::Left);
        let mic_monitor_r = audio.add_capture_consumer(audio::capture::CaptureChannel::Right);
        let mic_test_active = audio.mic_test_flag();

        mgr.start(Box::new(audio));

        // ── Mic monitor task (computes levels from capture, sends to dashboard) ──
        {
            let ws_tx_mic = ws_tx.clone();
            let active = mic_test_active;
            let mut left_rx = mic_monitor_l.rx;
            let mut right_rx = mic_monitor_r.rx;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(l_chunk) = left_rx.recv() => {
                            if !active.load(std::sync::atomic::Ordering::Relaxed) { continue; }
                            let r_chunk = right_rx.try_recv().ok();
                            let (l_rms, l_peak) = mic_compute_levels(&l_chunk);
                            let (r_rms, r_peak) = r_chunk.map(|c| mic_compute_levels(&c)).unwrap_or((0.0, 0.0));
                            let msg = encore_common::protocol::ServerMsg::MicLevels {
                                left_rms: l_rms, right_rms: r_rms,
                                left_peak: l_peak, right_peak: r_peak,
                            };
                            let _ = ws_tx_mic.send(serde_json::to_string(&msg).unwrap_or_default());
                        }
                        Some(_) = right_rx.recv() => { /* drain R when L hasn't arrived */ }
                        else => break,
                    }
                }
            });
        }

        // ── Watchdog (feeds both Linux /dev/watchdog AND MCU I2C heartbeat) ──
        let mut wdt = watchdog::WatchdogSubsystem::new();
        if let Some(ref mcu) = mcu {
            wdt.set_mcu(mcu.clone());
        }
        mgr.start(Box::new(wdt));

        // ── Resolve group peer_id early (before mDNS starts advertising) ──
        let group_peer_id = {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            cfg.group.peer_id.clone().unwrap_or_else(|| {
                let id = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                    rand_u32(), rand_u16(), rand_u16(), rand_u16(), rand_u48());
                let mut save_cfg = cfg;
                save_cfg.group.peer_id = Some(id.clone());
                let _ = save_cfg.save(std::path::Path::new("/lsync/encore/config.toml"));
                id
            })
        };

        // ── Network ──
        let (network_cmd_tx, network_cmd_rx) = mpsc::channel::<network::NetworkCmd>(32);
        let mdns_services = {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            let group_txt = group::discovery::format_txt_records(
                &group_peer_id,
                &cfg.group.group_name,
                match cfg.group.channel.as_str() {
                    "left" => group::wire::ChannelAssignment::Left,
                    "right" => group::wire::ChannelAssignment::Right,
                    _ => group::wire::ChannelAssignment::Stereo,
                },
            );
            vec![
                network::mdns::MdnsService {
                    service_type: "_encore._tcp".into(),
                    instance_name: cfg.device.name.clone(),
                    port: 80,
                    txt: vec!["VERSION=1.0".into()],
                },
                network::mdns::MdnsService {
                    service_type: "_spotify-connect._tcp".into(),
                    instance_name: "Invoke".into(),
                    port: spotify::ZEROCONF_PORT,
                    txt: vec!["VERSION=1.0".into(), "CPath=/".into()],
                },
                network::mdns::MdnsService {
                    service_type: "_encore-group._tcp".into(),
                    instance_name: cfg.device.name.clone(),
                    port: group::peer::GROUP_PORT,
                    txt: group_txt,
                },
            ]
        };
        // Discovery channels: mDNS → forwarding task → group subsystem
        let (mdns_discovery_tx, mut mdns_discovery_rx) = mpsc::channel::<network::mdns::MdnsDiscovery>(32);
        let (group_discovery_tx, group_discovery_rx) = mpsc::channel::<group::discovery::DiscoveryEvent>(32);

        // Forward mDNS discoveries to group-compatible type
        tokio::spawn(async move {
            while let Some(disc) = mdns_discovery_rx.recv().await {
                let _ = group_discovery_tx.try_send(group::discovery::DiscoveryEvent {
                    peer_id: disc.peer_id,
                    name: disc.name,
                    address: disc.address,
                    port: disc.port,
                    group_name: disc.group_name,
                    channel: disc.channel,
                });
            }
        });

        let mut net_sub = network::NetworkSubsystem::new(Some(ws_tx.clone()), Some(network_cmd_rx), ap_active.clone(), mdns_services);
        web.set_wifi_result_cache(net_sub.wifi_result_cache());
        web.set_network_state_cache(net_sub.network_state_cache());
        net_sub.set_discovery(mdns_discovery_tx, group_peer_id.clone());
        mgr.start(Box::new(net_sub));

        // ── VPN (WireGuard tunnel) ──
        {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            )
            .unwrap_or_default();
            if cfg.vpn.enabled && cfg.vpn.private_key.is_some() {
                mgr.start(Box::new(vpn::VpnSubsystem::new(cfg.vpn)));
            } else {
                info!("VPN: disabled or not configured, skipping");
            }
        }

        // ── Web dashboard ──
        mgr.start(Box::new(web));

        // ── Spotify + BT command channels (created early for group wiring) ──
        let (spotify_cmd_tx, spotify_cmd_rx) = mpsc::channel(8);
        let (bt_cmd_tx, bt_cmd_rx) = mpsc::channel(8);

        // ── Group (multi-speaker sync) ──
        let (group_cmd_tx, group_cmd_rx) = mpsc::channel::<group::GroupCmd>(32);

        // Suspend channels: group → audio sources (true = suspend, false = resume)
        let (spotify_suspend_tx, spotify_suspend_rx) = mpsc::channel::<bool>(1);
        let (bt_suspend_tx, bt_suspend_rx) = mpsc::channel::<bool>(1);
        let (wyoming_suspend_tx, wyoming_suspend_rx) = mpsc::channel::<bool>(1);

        let mut group_vol_sync_rx: Option<mpsc::Receiver<u8>> = None;
        {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            )
            .unwrap_or_default();

            let channel = match cfg.group.channel.as_str() {
                "left" => group::wire::ChannelAssignment::Left,
                "right" => group::wire::ChannelAssignment::Right,
                _ => group::wire::ChannelAssignment::Stereo,
            };

            let mut group_sub = group::GroupSubsystem::new(
                group_peer_id.clone(),
                cfg.device.name.clone(),
                cfg.group.group_name.clone(),
                channel,
                cfg.group.buffer_ms,
                cfg.group.enabled,
                network_slot.clone(),
                network_tap.clone(),
                tap_active.clone(),
                group_cmd_rx,
                cfg.group.peers.clone(),
                cfg.group.party_mode,
            );
            group_sub.set_ws_tx(ws_tx.clone());
            group_sub.set_spotify_tx(spotify_cmd_tx.clone());
            group_sub.set_discovery_rx(group_discovery_rx);
            // Volume sync: group sends u8 → forward to audio subsystem (spawned after led_tx exists)
            let (vol_sync_tx, vol_sync_rx) = mpsc::channel::<u8>(8);
            group_sub.set_volume_tx(vol_sync_tx);
            group_vol_sync_rx = Some(vol_sync_rx);
            group_sub.add_suspend_tx(spotify_suspend_tx);
            group_sub.add_suspend_tx(bt_suspend_tx);
            group_sub.add_suspend_tx(wyoming_suspend_tx);
            mgr.start(Box::new(group_sub));
        }

        // ── Spotify Connect ──
        {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            if cfg.spotify.enabled {
                let mut spotify_sub = spotify::SpotifySubsystem::new(spotify_slot, Some(cfg.device.name.clone()), Some(spotify_cmd_rx), Some(ws_tx.clone()));
                spotify_sub.set_suspend_rx(spotify_suspend_rx);
                spotify_sub.set_group_tx(group_cmd_tx.clone());
                mgr.start(Box::new(spotify_sub));
            } else {
                info!("Spotify: disabled in config, skipping");
            }
        }

        // ── Bluetooth A2DP sink ──
        {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            if cfg.bluetooth.enabled {
                let mut bt_sub = bluetooth::BluetoothSubsystem::new(
                    bt_slot, Some(cfg.device.name.clone()), Some(bt_cmd_rx), Some(ws_tx.clone()),
                );
                bt_sub.set_suspend_rx(bt_suspend_rx);
                bt_sub.set_group_tx(group_cmd_tx.clone());
                mgr.start(Box::new(bt_sub));
            } else {
                info!("Bluetooth: disabled in config, skipping");
            }
        }

        // ── LED ring ──
        let vol_step = {
            let cfg = encore_common::config::EncoreConfigFile::load(
                std::path::Path::new("/lsync/encore/config.toml"),
            ).unwrap_or_default();
            cfg.audio.volume_ring_step
        };
        let (led_tx, button_rx) = if let Some(mcu) = mcu {
            let (led_tx, led_rx) = mpsc::channel(32);
            let (button_tx, button_rx) = mpsc::channel::<led::ButtonAction>(32);
            let mut led_sub = led::LedSubsystem::new(led_rx, audio_cmd_tx.clone(), button_tx, mcu, vol_step);
            led_sub.set_ws_tx(ws_tx.clone());
            led_sub.set_group_tx(group_cmd_tx.clone());
            mgr.start(Box::new(led_sub));
            (Some(led_tx), Some(button_rx))
        } else {
            (None, None)
        };

        // ── Group volume sync (deferred until led_tx exists) ──
        if let Some(mut vol_sync_rx) = group_vol_sync_rx {
            let led_tx_vol = led_tx.clone();
            let audio_tx_vol = audio_cmd_tx.clone();
            tokio::spawn(async move {
                while let Some(level) = vol_sync_rx.recv().await {
                    if let Some(ref tx) = led_tx_vol {
                        let _ = tx.try_send(led::LedCmd::SetVolume(level));
                    } else {
                        let _ = audio_tx_vol.try_send(audio::subsystem::AudioCmd::SetVolume(level));
                    }
                }
            });
        }

        // ── Wyoming voice satellite ──
        let wyoming_led_tx = led_tx.clone();
        let (wyoming_trigger_tx, wyoming_trigger_rx) = mpsc::channel::<()>(4);

        // Voice session channels (VoiceCmd feedback from Wyoming → voice session)
        let (voice_cmd_tx, voice_cmd_rx) = mpsc::channel::<wakeword::VoiceCmd>(8);
        let (voice_event_tx, _voice_event_rx) = mpsc::channel::<wakeword::VoiceEvent>(8);

        {
            let mut wyoming_sub = wyoming::WyomingSubsystem::new(
                wyoming_slot, wyoming_mic, Some(voice_cmd_tx),
                None, None, wyoming_led_tx, Some(wyoming_trigger_rx),
            );
            wyoming_sub.set_suspend_rx(wyoming_suspend_rx);
            mgr.start(Box::new(wyoming_sub));
        }

        // Voice session task — wake word detection + session state machine
        {
            let led_tx_voice = led_tx.clone();
            let detector = Box::new(wakeword::detector::StubDetector);
            tokio::spawn(async move {
                wakeword::voice_session_task(
                    wakeword_mic, voice_cmd_rx, voice_event_tx,
                    led_tx_voice, detector,
                ).await;
            });
        }

        // ── Home Assistant (optional — requires MQTT config) ──
        match load_ha_config() {
            Some((host, port, user, pass)) => {
                let (ha_tx, mut ha_rx) = mpsc::channel::<encore_common::protocol::ClientMsg>(32);
                let mut ha_sub = homeassistant::HomeAssistantSubsystem::new(
                    host, port, user, pass, Some(ha_tx),
                );
                ha_sub.set_ws_rx(ws_tx.subscribe());
                mgr.start(Box::new(ha_sub));
                // Forward HA commands into the main routing pipeline
                let audio_tx_ha = audio_cmd_tx.clone();
                let led_tx_ha = led_tx.clone();
                let spotify_tx_ha = spotify_cmd_tx.clone();
                tokio::spawn(async move {
                    while let Some(msg) = ha_rx.recv().await {
                        use encore_common::protocol::ClientMsg;
                        match msg {
                            ClientMsg::SetMasterVolume(level) => {
                                if let Some(ref tx) = led_tx_ha {
                                    let _ = tx.try_send(led::LedCmd::SetVolume(level));
                                } else {
                                    let _ = audio_tx_ha.try_send(audio::subsystem::AudioCmd::SetVolume(level));
                                }
                            }
                            ClientMsg::SetLed(anim) => {
                                if let Some(ref tx) = led_tx_ha {
                                    let _ = tx.try_send(led::LedCmd::Animate(anim));
                                }
                            }
                            ClientMsg::SpotifyControl(action) => {
                                let _ = spotify_tx_ha.try_send(action);
                            }
                            _ => {}
                        }
                    }
                });
            }
            None => {
                info!("HA: no MQTT config found, skipping HomeAssistant subsystem");
            }
        }

        // ── 1Hz subsystem status broadcaster ──
        mgr.start_status_broadcaster();

        // Signal boot complete to LED subsystem after a settling delay
        if let Some(ref led_tx) = led_tx {
            let boot_tx = led_tx.clone();
            let boot_cmd = if safe_mode {
                led::LedCmd::SafeBootComplete
            } else {
                led::LedCmd::BootComplete
            };
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = boot_tx.send(boot_cmd).await;
            });
        }

        // ── Button routing task ──
        if let Some(mut button_rx) = button_rx {
            let spotify_tx_btn = spotify_cmd_tx.clone();
            let audio_tx_btn = audio_cmd_tx.clone();
            let led_tx_btn = led_tx.clone();
            let network_cmd_tx_btn = network_cmd_tx.clone();
            let group_tx_btn = group_cmd_tx.clone();
            let mic_muted = Arc::new(std::sync::atomic::AtomicBool::new(false));
            tokio::spawn(async move {
                use encore_common::protocol::SpotifyAction;
                while let Some(action) = button_rx.recv().await {
                    match action {
                        led::ButtonAction::PlayPause => {
                            info!("Button: play/pause");
                            let _ = spotify_tx_btn.try_send(SpotifyAction::Play);
                            let _ = group_tx_btn.try_send(group::GroupCmd::PlayPause { action: "play".into() });
                        }
                        led::ButtonAction::VoiceTrigger => {
                            info!("Button: voice trigger (push-to-talk)");
                            if let Some(ref tx) = led_tx_btn {
                                let _ = tx.try_send(led::LedCmd::PlayBin {
                                    name: "L_101_c_listening".into(),
                                    repeat: true,
                                });
                            }
                            let _ = wyoming_trigger_tx.try_send(());
                        }
                        led::ButtonAction::MicToggle => {
                            let current = mic_muted.load(std::sync::atomic::Ordering::Relaxed);
                            let toggled = !current;
                            mic_muted.store(toggled, std::sync::atomic::Ordering::Relaxed);
                            info!("Button: mic mute = {}", toggled);
                            let _ = audio_tx_btn.try_send(audio::subsystem::AudioCmd::SetMicMute(toggled));
                        }
                        led::ButtonAction::WifiSetup => {
                            info!("Button: WiFi setup requested");
                            let _ = network_cmd_tx_btn.try_send(network::NetworkCmd::EnterApMode);
                        }
                        led::ButtonAction::BluetoothToggle => {
                            info!("Button: BT pairing not available on this hardware");
                        }
                    }
                }
            });
        }

        // ── ClientMsg routing task ──
        if let Some(mut client_rx) = client_rx {
            let audio_tx = audio_cmd_tx.clone();
            let led_tx_clone = led_tx.clone();
            let ws_tx_route = ws_tx.clone();
            let group_tx = group_cmd_tx.clone();
            let debug_modes = mgr.debug_modes();
            tokio::spawn(async move {
                use encore_common::protocol::{ClientMsg, EncoreConfig, ServerMsg};
                let config_path = std::path::Path::new("/lsync/encore/config.toml");

                // Helper: load config and broadcast ConfigLoaded
                let broadcast_config = |tx: &tokio::sync::broadcast::Sender<String>| {
                    let cfg_file = encore_common::config::EncoreConfigFile::load(config_path)
                        .unwrap_or_default();
                    let proto = EncoreConfig::from_file(&cfg_file);
                    let msg = ServerMsg::ConfigLoaded(Box::new(proto));
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = tx.send(json);
                    }
                };

                while let Some(msg) = client_rx.recv().await {
                    match msg {
                        ClientMsg::SetMasterVolume(level) => {
                            // Route through LED subsystem: updates volume, shows arc, forwards to audio
                            if let Some(ref tx) = led_tx_clone {
                                let _ = tx.try_send(led::LedCmd::SetVolume(level));
                            } else {
                                // No LED subsystem — send directly to audio
                                let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetVolume(level));
                            }
                            // Notify group for volume sync
                            let _ = group_tx.try_send(group::GroupCmd::SetVolume(level));
                        }
                        ClientMsg::SetVolume { source, level } => {
                            // Per-source volume — controls the source's internal volume,
                            // NOT the hardware DAC. Master volume (SetMasterVolume) controls DAC.
                            match source {
                                encore_common::protocol::SourceId::Spotify => {
                                    let _ = spotify_cmd_tx.try_send(
                                        encore_common::protocol::SpotifyAction::SetVolume { level }
                                    );
                                }
                                _ => {
                                    // BT/Wyoming/System: no per-source volume yet
                                    info!("ClientMsg: SetVolume({:?}, {}) — not implemented", source, level);
                                }
                            }
                        }
                        ClientMsg::SetLed(anim) => {
                            if let Some(ref tx) = led_tx_clone {
                                let _ = tx.try_send(led::LedCmd::Animate(anim));
                            }
                        }
                        ClientMsg::SaveConfig(config) => {
                            let vol_step = config.volume_ring_step;
                            let device_name = config.device_name.clone();
                            let spotify_enabled = config.spotify_enabled;
                            let existing = encore_common::config::EncoreConfigFile::load(config_path)
                                .unwrap_or_default();
                            let old_spotify_enabled = existing.spotify.enabled;
                            let cfg = config.to_file_merge(&existing);
                            if let Err(e) = cfg.save(config_path) {
                                warn!("ClientMsg: config save failed: {}", e);
                            } else {
                                info!("ClientMsg: config saved");
                                broadcast_config(&ws_tx_route);
                                if let Some(ref tx) = led_tx_clone {
                                    let _ = tx.try_send(led::LedCmd::SetVolStep(vol_step));
                                }
                                // Notify network subsystem if device name changed
                                let _ = network_cmd_tx.try_send(
                                    network::NetworkCmd::SetDeviceName(device_name),
                                );
                                // Notify Spotify subsystem if enabled state changed
                                if spotify_enabled != old_spotify_enabled {
                                    let _ = spotify_cmd_tx.try_send(
                                        encore_common::protocol::SpotifyAction::SetEnabled { enabled: spotify_enabled },
                                    );
                                }
                            }
                        }
                        ClientMsg::RequestConfig => {
                            broadcast_config(&ws_tx_route);
                        }
                        ClientMsg::SpotifyControl(action) => {
                            info!("ClientMsg: SpotifyControl({:?})", action);
                            let _ = spotify_cmd_tx.try_send(action);
                        }
                        ClientMsg::BluetoothControl(action) => {
                            info!("ClientMsg: BluetoothControl({:?})", action);
                            let _ = bt_cmd_tx.try_send(action);
                        }
                        ClientMsg::RestartSubsystem(name) => {
                            warn!("ClientMsg: RestartSubsystem({}) — requires subsystem restart API", name);
                        }
                        ClientMsg::SetWifi(creds) => {
                            info!("ClientMsg: SetWifi(ssid={})", creds.ssid);
                            if let Err(e) = network_cmd_tx.try_send(network::NetworkCmd::ConnectWifi {
                                ssid: creds.ssid,
                                password: creds.password,
                            }) {
                                warn!("ClientMsg: SetWifi failed to send to network subsystem: {}", e);
                            }
                        }
                        ClientMsg::RequestNetworkState => {
                            let _ = network_cmd_tx.try_send(network::NetworkCmd::RequestState);
                        }
                        ClientMsg::SetDebugMode { subsystem, mode } => {
                            info!("ClientMsg: SetDebugMode({}, {:?})", subsystem, mode);
                            debug_modes.set(&subsystem, mode);
                        }
                        // EQ controls → audio subsystem
                        ClientMsg::SetEqBand { band, config } => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetEqBand { band, config });
                        }
                        ClientMsg::SetEqPreset(preset) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetEqPreset(preset));
                        }
                        ClientMsg::SetEqEnabled(enabled) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetEqEnabled(enabled));
                        }
                        // DRC controls → audio subsystem
                        ClientMsg::SetDrc { band, config } => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetDrc { band, config });
                        }
                        ClientMsg::SetDrcCrossover { low_mid_hz, mid_high_hz } => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetDrcCrossover { low_mid_hz, mid_high_hz });
                        }
                        ClientMsg::SetDrcEnabled(enabled) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetDrcEnabled(enabled));
                        }
                        ClientMsg::SetDrcPreset(preset) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetDrcPreset(preset));
                        }
                        // DSP controls → audio subsystem
                        ClientMsg::SetDspVolume(level) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetDspVolume(level));
                        }
                        ClientMsg::SetMicMute(muted) => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::SetMicMute(muted));
                        }
                        // Hardware explorer: DAC register read (async with oneshot reply)
                        ClientMsg::DacRegRead { page, reg } => {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DacRegRead { page, reg, reply: reply_tx });
                            let ws = ws_tx_route.clone();
                            tokio::spawn(async move {
                                if let Ok(Ok(value)) = reply_rx.await {
                                    let msg = ServerMsg::DacRegValue { page, reg, value };
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        let _ = ws.send(json);
                                    }
                                }
                            });
                        }
                        ClientMsg::DacRegWrite { page, reg, value } => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DacRegWrite { page, reg, value });
                        }
                        ClientMsg::DspSpiSend { msg_type, data } => {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DspSpiSend { msg_type, data, reply: reply_tx });
                            let ws = ws_tx_route.clone();
                            tokio::spawn(async move {
                                if let Ok(Ok(data)) = reply_rx.await {
                                    let msg = ServerMsg::DspSpiResponse { data };
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        let _ = ws.send(json);
                                    }
                                }
                            });
                        }
                        ClientMsg::DspMemoryDump { start_page, num_pages } => {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DspMemoryDump { start_page, num_pages, reply: reply_tx });
                            let ws = ws_tx_route.clone();
                            tokio::spawn(async move {
                                match reply_rx.await {
                                    Ok(Ok(data)) => {
                                        let msg = ServerMsg::DspMemoryDump { start_page, data };
                                        if let Ok(json) = serde_json::to_string(&msg) {
                                            let _ = ws.send(json);
                                        }
                                    }
                                    Ok(Err(e)) => {
                                        let msg = ServerMsg::DspEvent { description: format!("Memory dump failed: {}", e) };
                                        if let Ok(json) = serde_json::to_string(&msg) {
                                            let _ = ws.send(json);
                                        }
                                    }
                                    _ => {}
                                }
                            });
                        }
                        ClientMsg::DspDumpToFile { path } => {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DspDumpToFile { path: path.clone(), reply: reply_tx });
                            let ws = ws_tx_route.clone();
                            tokio::spawn(async move {
                                match reply_rx.await {
                                    Ok(Ok(bytes)) => {
                                        let msg = ServerMsg::DspEvent { description: format!("Memory dump complete: {} bytes written to {}", bytes, path) };
                                        if let Ok(json) = serde_json::to_string(&msg) {
                                            let _ = ws.send(json);
                                        }
                                    }
                                    Ok(Err(e)) => {
                                        let msg = ServerMsg::DspEvent { description: format!("Memory dump failed: {}", e) };
                                        if let Ok(json) = serde_json::to_string(&msg) {
                                            let _ = ws.send(json);
                                        }
                                    }
                                    _ => {}
                                }
                            });
                        }
                        ClientMsg::SetCustomAnimation { frames } => {
                            if let Some(ref tx) = led_tx_clone {
                                let _ = tx.try_send(led::LedCmd::Animate(
                                    encore_common::protocol::LedAnimation::Custom { frames },
                                ));
                            }
                        }
                        ClientMsg::DspPollEvents => {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::DspPollEvents { reply: reply_tx });
                            let ws = ws_tx_route.clone();
                            tokio::spawn(async move {
                                if let Ok(Ok(events)) = reply_rx.await {
                                    for desc in events {
                                        let msg = ServerMsg::DspEvent { description: desc };
                                        if let Ok(json) = serde_json::to_string(&msg) {
                                            let _ = ws.send(json);
                                        }
                                    }
                                }
                            });
                        }
                        // Group controls
                        ClientMsg::SetGroupEnabled(enabled) => {
                            let _ = group_tx.try_send(group::GroupCmd::SetEnabled(enabled));
                        }
                        ClientMsg::SetGroupChannel(ch) => {
                            let channel = match ch.as_str() {
                                "left" => group::wire::ChannelAssignment::Left,
                                "right" => group::wire::ChannelAssignment::Right,
                                _ => group::wire::ChannelAssignment::Stereo,
                            };
                            let _ = group_tx.try_send(group::GroupCmd::SetChannel(channel));
                        }
                        ClientMsg::SetGroupBufferMs(ms) => {
                            let _ = group_tx.try_send(group::GroupCmd::SetBufferMs(ms));
                        }
                        ClientMsg::SetGroupName(name) => {
                            let _ = group_tx.try_send(group::GroupCmd::SetGroupName(name));
                        }
                        ClientMsg::SetGroupVolume(vol) => {
                            let _ = group_tx.try_send(group::GroupCmd::SetVolume(vol));
                        }
                        ClientMsg::SetPartyMode(enabled) => {
                            let _ = group_tx.try_send(group::GroupCmd::SetPartyMode(enabled));
                        }
                        ClientMsg::RequestGroupStatus => {
                            let _ = group_tx.try_send(group::GroupCmd::RequestStatus);
                        }
                        ClientMsg::StartMicTest => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::StartMicTest);
                        }
                        ClientMsg::StopMicTest => {
                            let _ = audio_tx.try_send(audio::subsystem::AudioCmd::StopMicTest);
                        }
                    }
                }
            });
        }

        info!("Encore running -- all subsystems started");

        // Keep channel senders alive + wait for shutdown
        // (audio_cmd_tx must stay alive or audio subsystem's cmd_rx closes)
        let _keep = (audio_cmd_tx, led_tx, group_cmd_tx);
        shutdown_rx.recv().await.ok();
        mgr.shutdown().await;

        info!("Encore stopped");
        return Ok(());
    }

    // Non-Linux: no subsystems to start
    info!("Encore running (no subsystems on this platform)");
    shutdown_rx.recv().await.ok();
    mgr.shutdown().await;
    info!("Encore stopped");
    Ok(())
}

/// Pseudo-random u32 using system time (no crate dependency).
#[cfg(target_os = "linux")]
fn rand_u32() -> u32 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (t.as_nanos() as u32).wrapping_mul(2654435761)
}

/// Pseudo-random u16.
#[cfg(target_os = "linux")]
fn rand_u16() -> u16 {
    rand_u32() as u16
}

/// Pseudo-random u48 (for UUID last segment).
#[cfg(target_os = "linux")]
fn rand_u48() -> u64 {
    let a = rand_u32() as u64;
    let b = rand_u16() as u64;
    (a << 16) | b
}

/// Compute RMS and peak levels from 16kHz mono S16 samples.
#[cfg(target_os = "linux")]
fn mic_compute_levels(samples: &[i16]) -> (f32, f32) {
    let n = samples.len().max(1) as f64;
    let scale = 1.0 / 32768.0;
    let (mut sum_sq, mut peak) = (0.0_f64, 0.0_f64);
    for &s in samples {
        let v = s as f64 * scale;
        sum_sq += v * v;
        peak = peak.max(v.abs());
    }
    ((sum_sq / n).sqrt() as f32, peak as f32)
}

/// Load Home Assistant MQTT config from /lsync/encore/config.toml.
/// Returns None if file missing or [mqtt] section absent.
#[cfg(target_os = "linux")]
fn load_ha_config() -> Option<(String, Option<u16>, Option<String>, Option<String>)> {
    let content = std::fs::read_to_string("/lsync/encore/config.toml").ok()?;
    let table: toml::Table = content.parse().ok()?;
    let mqtt = table.get("mqtt")?.as_table()?;
    let host = mqtt.get("host")?.as_str()?.to_string();
    let port = mqtt.get("port").and_then(|v| v.as_integer()).map(|v| v as u16);
    let user = mqtt.get("user").and_then(|v| v.as_str()).map(|s| s.to_string());
    let password = mqtt.get("password").and_then(|v| v.as_str()).map(|s| s.to_string());
    Some((host, port, user, password))
}
