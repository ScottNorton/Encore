//! Home Assistant MQTT bridge subsystem.
//!
//! Connects to an MQTT broker, publishes HA Discovery entities,
//! subscribes to control topics, and publishes device state.

use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use encore_common::protocol::{ClientMsg, LedAnimation, SpotifyAction, SubsystemState};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};

const DEFAULT_PORT: u16 = 1883;
const KEEPALIVE: Duration = Duration::from_secs(60);
const STATE_INTERVAL: Duration = Duration::from_secs(10);
const PREFIX: &str = "encore";
const DISCOVERY_PREFIX: &str = "homeassistant";

/// HA MQTT bridge subsystem.
pub struct HomeAssistantSubsystem {
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_user: Option<String>,
    mqtt_password: Option<String>,
    client_tx: Option<mpsc::Sender<ClientMsg>>,
    /// WebSocket broadcast subscriber for caching group status.
    ws_rx: Option<tokio::sync::broadcast::Receiver<String>>,
    /// Optional acoustic-sense update receiver (published under `encore/<slug>/sense/*`).
    sense_rx: Option<mpsc::Receiver<crate::sense::SenseUpdate>>,
    /// Per-device topic slug for sense topics (config device name, sanitized).
    device_slug: String,
}

impl HomeAssistantSubsystem {
    pub fn new(
        host: String,
        port: Option<u16>,
        user: Option<String>,
        password: Option<String>,
        client_tx: Option<mpsc::Sender<ClientMsg>>,
    ) -> Self {
        Self {
            mqtt_host: host,
            mqtt_port: port.unwrap_or(DEFAULT_PORT),
            mqtt_user: user,
            mqtt_password: password,
            client_tx,
            ws_rx: None,
            sense_rx: None,
            device_slug: "invoke".to_string(),
        }
    }

    /// Set WebSocket broadcast receiver for caching group state.
    pub fn set_ws_rx(&mut self, rx: tokio::sync::broadcast::Receiver<String>) {
        self.ws_rx = Some(rx);
    }

    /// Provide the acoustic-sense update receiver and the device name used to
    /// build the per-device topic prefix (`encore/<slug>/sense/*`). The slug is
    /// the device name lowercased with non-alphanumeric chars replaced by `_`;
    /// an empty result falls back to `invoke`.
    pub fn set_sense_rx(
        &mut self,
        rx: mpsc::Receiver<crate::sense::SenseUpdate>,
        device_name: &str,
    ) {
        let slug: String = device_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let slug = slug.trim_matches('_').to_string();
        self.device_slug = if slug.is_empty() {
            "invoke".to_string()
        } else {
            slug
        };
        self.sense_rx = Some(rx);
    }

    fn handle_command(&self, topic: &str, payload: &[u8]) {
        let Some(ref tx) = self.client_tx else { return };
        let payload_str = String::from_utf8_lossy(payload);

        if topic.ends_with("/light/ring/set") {
            debug!("HA: LED command: {}", payload_str);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                let state = json.get("state").and_then(|v| v.as_str()).unwrap_or("OFF");
                if state == "OFF" {
                    let _ = tx.try_send(ClientMsg::SetLed(LedAnimation::Off));
                } else {
                    let r = json
                        .get("color")
                        .and_then(|c| c.get("r"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(255) as u8;
                    let g = json
                        .get("color")
                        .and_then(|c| c.get("g"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(255) as u8;
                    let b = json
                        .get("color")
                        .and_then(|c| c.get("b"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(255) as u8;
                    let brightness = json
                        .get("brightness")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(255) as f32
                        / 255.0;
                    let r = (r as f32 * brightness) as u8;
                    let g = (g as f32 * brightness) as u8;
                    let b = (b as f32 * brightness) as u8;

                    if let Some(effect) = json.get("effect").and_then(|v| v.as_str()) {
                        let anim = match effect {
                            "Breathing" => LedAnimation::Breathe {
                                r,
                                g,
                                b,
                                period_ms: 2000,
                            },
                            "Spinning" => LedAnimation::Spin { r, g, b, speed: 3 },
                            "Pulse" => LedAnimation::Pulse { r, g, b },
                            _ => LedAnimation::Solid { r, g, b },
                        };
                        let _ = tx.try_send(ClientMsg::SetLed(anim));
                    } else {
                        let _ = tx.try_send(ClientMsg::SetLed(LedAnimation::Solid { r, g, b }));
                    }
                }
            }
        } else if topic.ends_with("/number/volume/set") {
            if let Ok(level) = payload_str.trim().parse::<u8>() {
                debug!("HA: volume command: {}", level);
                let _ = tx.try_send(ClientMsg::SetMasterVolume(level));
            }
        } else if topic.ends_with("/select/effect/set") {
            debug!("HA: effect command: {}", payload_str);
            let anim = match payload_str.trim() {
                "None" => LedAnimation::Off,
                "Breathing" => LedAnimation::Breathe {
                    r: 0,
                    g: 120,
                    b: 255,
                    period_ms: 2000,
                },
                "Spinning" => LedAnimation::Spin {
                    r: 0,
                    g: 120,
                    b: 255,
                    speed: 3,
                },
                "Pulse" => LedAnimation::Pulse {
                    r: 0,
                    g: 120,
                    b: 255,
                },
                "Volume Arc" => LedAnimation::VolumeArc { level: 50 },
                _ => return,
            };
            let _ = tx.try_send(ClientMsg::SetLed(anim));
        } else if topic.ends_with("/switch/group/set") {
            let on = payload_str.trim().eq_ignore_ascii_case("ON");
            debug!("HA: group enabled = {}", on);
            let _ = tx.try_send(ClientMsg::SetGroupEnabled(on));
        } else if topic.ends_with("/number/group_volume/set") {
            if let Ok(level) = payload_str.trim().parse::<u8>() {
                debug!("HA: group volume = {}", level);
                let _ = tx.try_send(ClientMsg::SetGroupVolume(level));
            }
        } else if topic.ends_with("/media/command") {
            debug!("HA: media command: {}", payload_str);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                match json.get("command").and_then(|v| v.as_str()).unwrap_or("") {
                    "play" => {
                        let _ = tx.try_send(ClientMsg::SpotifyControl(SpotifyAction::Play));
                    }
                    "pause" => {
                        let _ = tx.try_send(ClientMsg::SpotifyControl(SpotifyAction::Pause));
                    }
                    "next" => {
                        let _ = tx.try_send(ClientMsg::SpotifyControl(SpotifyAction::Next));
                    }
                    "previous" => {
                        let _ = tx.try_send(ClientMsg::SpotifyControl(SpotifyAction::Previous));
                    }
                    _ => {}
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Subsystem for HomeAssistantSubsystem {
    fn name(&self) -> &'static str {
        "homeassistant"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        let client_id = format!("encore_{}", std::process::id());
        let mut opts = MqttOptions::new(&client_id, &self.mqtt_host, self.mqtt_port);
        opts.set_keep_alive(KEEPALIVE);

        if let (Some(user), Some(pass)) = (&self.mqtt_user, &self.mqtt_password) {
            opts.set_credentials(user, pass);
        }

        // Last will: mark device offline
        opts.set_last_will(rumqttc::LastWill::new(
            format!("{}/status", PREFIX),
            "offline",
            QoS::AtLeastOnce,
            true,
        ));

        let (client, mut eventloop) = AsyncClient::new(opts, 64);

        info!(
            "HA: connecting to MQTT broker {}:{}",
            self.mqtt_host, self.mqtt_port
        );

        // Publish online status and discovery on first connect
        let mut discovered = false;
        let mut ticker = interval(STATE_INTERVAL);
        let mut ws_rx = self.ws_rx.take();
        let mut sense_rx = self.sense_rx.take();
        let sense_base = format!("{}/{}/sense", PREFIX, self.device_slug);

        // Cached group state for publishing
        let mut group_enabled = false;
        let mut group_role = "standalone".to_string();
        let mut group_peer_count: usize = 0;

        loop {
            tokio::select! {
                event = eventloop.poll() => {
                    match event {
                        Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                            info!("HA: MQTT connected");

                            // Publish online
                            client.publish(
                                &format!("{}/status", PREFIX),
                                QoS::AtLeastOnce,
                                true,
                                "online",
                            ).await.ok();

                            if !discovered {
                                publish_discovery(&client).await;
                                subscribe_commands(&client).await;
                                discovered = true;
                            }
                        }
                        Ok(Event::Incoming(Incoming::Publish(msg))) => {
                            self.handle_command(&msg.topic, &msg.payload);
                            ctx.health.inc_msg();
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!("HA: MQTT error: {}", e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
                // Cache group status from WebSocket broadcast
                ws_msg = async {
                    if let Some(ref mut rx) = ws_rx {
                        rx.recv().await.ok()
                    } else {
                        std::future::pending::<Option<String>>().await
                    }
                } => {
                    if let Some(msg) = ws_msg {
                        if msg.contains("\"GroupStatus\"") {
                            if let Ok(encore_common::protocol::ServerMsg::GroupStatus(status)) =
                                serde_json::from_str::<encore_common::protocol::ServerMsg>(&msg)
                            {
                                group_enabled = status.enabled;
                                group_role = status.role;
                                group_peer_count = status.peers.len();
                            }
                        }
                    }
                }
                // Acoustic-sense updates → MQTT (per-device prefix)
                Some(u) = async {
                    match sense_rx.as_mut() {
                        Some(r) => r.recv().await,
                        None => std::future::pending::<Option<crate::sense::SenseUpdate>>().await,
                    }
                } => {
                    match u {
                        crate::sense::SenseUpdate::State { state, level } => {
                            let s = match state {
                                crate::sense::SenseState::Quiet => "quiet",
                                crate::sense::SenseState::Activity => "activity",
                            };
                            client.publish(format!("{sense_base}/state"), QoS::AtLeastOnce, true, s).await.ok();
                            client.publish(format!("{sense_base}/level"), QoS::AtLeastOnce, true, level.to_string()).await.ok();
                        }
                        crate::sense::SenseUpdate::Loud { level } => {
                            let payload = serde_json::json!({ "event": "loud", "level": level }).to_string();
                            client.publish(format!("{sense_base}/event"), QoS::AtLeastOnce, false, payload).await.ok();
                        }
                    }
                    ctx.health.inc_msg();
                }
                _ = ticker.tick() => {
                    if discovered {
                        publish_state(&client, group_enabled, &group_role, group_peer_count).await;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        ctx.health.beat(now);
                    }
                }
                _ = ctx.shutdown.recv() => {
                    info!("HA: shutdown");
                    client.publish(
                        &format!("{}/status", PREFIX),
                        QoS::AtLeastOnce,
                        true,
                        "offline",
                    ).await.ok();
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Device block shared by all entities.
fn device_json() -> serde_json::Value {
    json!({
        "identifiers": ["encore"],
        "name": "Encore",
        "model": "Invoke",
        "manufacturer": "Harman Kardon",
        "sw_version": encore_common::VERSION
    })
}

/// Publish HA MQTT Discovery messages for all entities.
async fn publish_discovery(client: &AsyncClient) {
    let device = device_json();
    let avail = format!("{}/status", PREFIX);

    // Light: LED Ring
    let led_config = json!({
        "name": "LED Ring",
        "unique_id": "encore_led_ring",
        "command_topic": format!("{}/light/ring/set", PREFIX),
        "state_topic": format!("{}/light/ring/state", PREFIX),
        "schema": "json",
        "brightness": true,
        "supported_color_modes": ["rgb"],
        "effect": true,
        "effect_list": ["None", "Breathing", "Spinning", "Pulse", "Volume Arc"],
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/light/encore_led_ring/config", DISCOVERY_PREFIX),
        &led_config,
    )
    .await;

    // Number: Volume
    let vol_config = json!({
        "name": "Volume",
        "unique_id": "encore_volume",
        "command_topic": format!("{}/number/volume/set", PREFIX),
        "state_topic": format!("{}/number/volume/state", PREFIX),
        "min": 0,
        "max": 100,
        "step": 1,
        "icon": "mdi:volume-high",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/number/encore_volume/config", DISCOVERY_PREFIX),
        &vol_config,
    )
    .await;

    // Sensor: CPU
    let cpu_config = json!({
        "name": "CPU Usage",
        "unique_id": "encore_cpu",
        "state_topic": format!("{}/sensor/cpu/state", PREFIX),
        "unit_of_measurement": "%",
        "icon": "mdi:cpu-32-bit",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/sensor/encore_cpu/config", DISCOVERY_PREFIX),
        &cpu_config,
    )
    .await;

    // Sensor: Memory
    let mem_config = json!({
        "name": "Memory Usage",
        "unique_id": "encore_memory",
        "state_topic": format!("{}/sensor/memory/state", PREFIX),
        "unit_of_measurement": "%",
        "icon": "mdi:memory",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/sensor/encore_memory/config", DISCOVERY_PREFIX),
        &mem_config,
    )
    .await;

    // Sensor: WiFi Signal
    let wifi_config = json!({
        "name": "WiFi Signal",
        "unique_id": "encore_wifi_signal",
        "state_topic": format!("{}/sensor/wifi/state", PREFIX),
        "unit_of_measurement": "dBm",
        "device_class": "signal_strength",
        "icon": "mdi:wifi",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/sensor/encore_wifi_signal/config", DISCOVERY_PREFIX),
        &wifi_config,
    )
    .await;

    // Switch: Group Mode
    let group_switch_config = json!({
        "name": "Group Mode",
        "unique_id": "encore_group",
        "command_topic": format!("{}/switch/group/set", PREFIX),
        "state_topic": format!("{}/switch/group/state", PREFIX),
        "icon": "mdi:speaker-multiple",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/switch/encore_group/config", DISCOVERY_PREFIX),
        &group_switch_config,
    )
    .await;

    // Sensor: Group Role
    let group_role_config = json!({
        "name": "Group Role",
        "unique_id": "encore_group_role",
        "state_topic": format!("{}/sensor/group_role/state", PREFIX),
        "icon": "mdi:account-group",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/sensor/encore_group_role/config", DISCOVERY_PREFIX),
        &group_role_config,
    )
    .await;

    // Sensor: Group Peers
    let group_peers_config = json!({
        "name": "Group Peers",
        "unique_id": "encore_group_peers",
        "state_topic": format!("{}/sensor/group_peers/state", PREFIX),
        "icon": "mdi:lan-connect",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/sensor/encore_group_peers/config", DISCOVERY_PREFIX),
        &group_peers_config,
    )
    .await;

    // Number: Group Volume
    let group_vol_config = json!({
        "name": "Group Volume",
        "unique_id": "encore_group_volume",
        "command_topic": format!("{}/number/group_volume/set", PREFIX),
        "state_topic": format!("{}/number/group_volume/state", PREFIX),
        "min": 0,
        "max": 100,
        "step": 1,
        "icon": "mdi:volume-high",
        "device": device,
        "availability_topic": avail,
    });
    publish_retained(
        client,
        &format!("{}/number/encore_group_volume/config", DISCOVERY_PREFIX),
        &group_vol_config,
    )
    .await;

    info!("HA: published discovery for 9 entities");
}

/// Subscribe to command topics.
async fn subscribe_commands(client: &AsyncClient) {
    let topics = [
        format!("{}/light/ring/set", PREFIX),
        format!("{}/number/volume/set", PREFIX),
        format!("{}/select/effect/set", PREFIX),
        format!("{}/media/command", PREFIX),
        format!("{}/switch/group/set", PREFIX),
        format!("{}/number/group_volume/set", PREFIX),
    ];

    for topic in &topics {
        client.subscribe(topic, QoS::AtMostOnce).await.ok();
    }

    debug!("HA: subscribed to {} command topics", topics.len());
}

/// Publish current device state to state topics.
async fn publish_state(
    client: &AsyncClient,
    group_enabled: bool,
    group_role: &str,
    group_peers: usize,
) {
    // CPU usage from /proc/stat
    if let Ok(cpu) = read_cpu_percent() {
        client
            .publish(
                &format!("{}/sensor/cpu/state", PREFIX),
                QoS::AtMostOnce,
                false,
                cpu.to_string(),
            )
            .await
            .ok();
    }

    // Memory usage from /proc/meminfo
    if let Ok(mem) = read_mem_percent() {
        client
            .publish(
                &format!("{}/sensor/memory/state", PREFIX),
                QoS::AtMostOnce,
                false,
                mem.to_string(),
            )
            .await
            .ok();
    }

    // Group state
    client
        .publish(
            &format!("{}/switch/group/state", PREFIX),
            QoS::AtMostOnce,
            false,
            if group_enabled { "ON" } else { "OFF" },
        )
        .await
        .ok();

    client
        .publish(
            &format!("{}/sensor/group_role/state", PREFIX),
            QoS::AtMostOnce,
            false,
            group_role,
        )
        .await
        .ok();

    client
        .publish(
            &format!("{}/sensor/group_peers/state", PREFIX),
            QoS::AtMostOnce,
            false,
            group_peers.to_string(),
        )
        .await
        .ok();
}

/// Read CPU usage percentage from /proc/stat.
fn read_cpu_percent() -> Result<u8> {
    let stat = std::fs::read_to_string("/proc/stat").context("read /proc/stat")?;
    let line = stat.lines().next().context("empty /proc/stat")?;
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(7)
        .filter_map(|s| s.parse().ok())
        .collect();

    if vals.len() < 7 {
        anyhow::bail!("unexpected /proc/stat format");
    }

    let busy = vals[0] + vals[1] + vals[2] + vals[5] + vals[6]; // user+nice+system+irq+softirq
    let idle = vals[3] + vals[4]; // idle+iowait
    let total = busy + idle;

    if total == 0 {
        return Ok(0);
    }

    Ok(((busy * 100) / total) as u8)
}

/// Read memory usage percentage from /proc/meminfo.
fn read_mem_percent() -> Result<u8> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").context("read /proc/meminfo")?;

    let mut total = 0u64;
    let mut available = 0u64;

    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = parse_kb(rest);
        }
    }

    if total == 0 {
        return Ok(0);
    }

    let used = total.saturating_sub(available);
    Ok(((used * 100) / total) as u8)
}

fn parse_kb(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Publish a retained JSON message.
async fn publish_retained(client: &AsyncClient, topic: &str, value: &serde_json::Value) {
    let payload = serde_json::to_string(value).unwrap_or_default();
    client
        .publish(topic, QoS::AtLeastOnce, true, payload)
        .await
        .ok();
}
