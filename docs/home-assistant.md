# Home Assistant Integration

> **Experimental:** this integration is an early proof of concept, like much of Encore.
> The MQTT auto-discovery and entities have been tested against a live Home Assistant
> instance with a single speaker. The group-related entities (group switch, role and
> peer sensors, group volume) have not been tested, and the Wyoming voice satellite has
> not been validated end-to-end. Bug reports and testing feedback are welcome.

The Encore integrates with Home Assistant via MQTT auto-discovery and the Wyoming voice protocol.

## Prerequisites

- MQTT broker running (Mosquitto add-on or external)
- MQTT integration configured in Home Assistant
- Encore on the same network as your HA instance

## Setup

1. Open the Encore web dashboard (http://device-ip)
2. Go to Settings
3. Enter your MQTT broker IP address
4. Click Save — the HA bridge restarts automatically

## Auto-Discovered Entities

Encore's MQTT bridge registers entities via MQTT Discovery. These appear automatically in Home Assistant:

| Entity | Type | Description |
|--------|------|-------------|
| `light.encore_led_ring` | Light | RGB LED ring with color + effects |
| `number.encore_volume` | Number | Volume 0-100 |
| `sensor.encore_cpu_usage` | Sensor | CPU usage percentage |
| `sensor.encore_memory_usage` | Sensor | Memory usage percentage |
| `sensor.encore_wifi_signal` | Sensor | WiFi signal strength (dBm) |
| `switch.encore_group` | Switch | Toggle group mode on/off |
| `sensor.encore_group_role` | Sensor | Shows leader/follower/standalone state |
| `sensor.encore_group_peers` | Sensor | Connected peer count |
| `number.encore_group_volume` | Number | Volume control for group audio |

## Template Media Player

Since Home Assistant has no native MQTT media_player platform, you need a template media player. See [ha-config-example.yaml](ha-config-example.yaml) for the complete configuration including:

- Template media_player entity
- TTS and audio playback scripts
- Example automations (doorbell alert, volume ring, touch toggle, night light)

Copy the relevant sections into your HA `configuration.yaml`.

## Wyoming Voice Satellite

The Encore runs a Wyoming protocol satellite on port 10700, enabling voice assistant pipelines through Home Assistant.

### Setup

1. In Home Assistant, go to Settings → Devices & Services
2. Add integration: **Wyoming Protocol**
3. Host: your device IP
4. Port: 10700

### Features

- Wake word detection (if configured in HA pipeline)
- Speech-to-text via HA pipeline
- Text-to-speech playback through the speaker
- LED ring feedback during voice interaction

## MQTT Topics

For advanced use or custom automations:

| Topic | Direction | Description |
|-------|-----------|-------------|
| `encore/status` | → HA | Online/offline availability |
| `encore/light/ring/set` | ← HA | LED ring control (JSON: state, color, brightness, effect) |
| `encore/light/ring/state` | → HA | Current LED state |
| `encore/number/volume/set` | ← HA | Volume control (integer 0-100) |
| `encore/number/volume/state` | → HA | Current volume |
| `encore/select/effect/set` | ← HA | LED effect selection |
| `encore/media/command` | ← HA | Playback commands (JSON: play, pause, next, previous) |
| `encore/sensor/cpu/state` | → HA | CPU usage percentage |
| `encore/sensor/memory/state` | → HA | Memory usage percentage |
| `encore/sensor/wifi/state` | → HA | WiFi signal strength (dBm) |
| `encore/switch/group/set` | ← HA | Group mode on/off control |
| `encore/switch/group/state` | → HA | Group mode state |
| `encore/sensor/group_role/state` | → HA | Leader/follower/standalone role |
| `encore/sensor/group_peers/state` | → HA | Connected peer count |
| `encore/number/group_volume/set` | ← HA | Group volume control (0-100) |
| `encore/number/group_volume/state` | → HA | Current group volume |

## Example Automations

See [ha-config-example.yaml](ha-config-example.yaml) for ready-to-use automations:

- **Doorbell alert**: Flash LED blue + play sound
- **Volume ring**: Map physical ring to HA volume
- **Touch toggle**: Short press toggles LED on/off
- **Long press**: Announce the time via TTS
- **Night light**: LED ring on motion after sunset
- **High CPU alert**: Notification on sustained high usage
