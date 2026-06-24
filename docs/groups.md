# Multi-Speaker Groups

Groups let multiple Invokes running Encore play audio in sync across rooms. One speaker
acts as leader and streams audio to the others over the local network or P2P AP.

---

## How It Works

### Discovery

Speakers find each other using mDNS. Each device with groups enabled advertises
a `_encore-group._tcp` service on the local network. The advertisement includes
TXT records with:

- `id` -- the speaker's unique peer ID (UUID v4, auto-generated on first boot)
- `group` -- the group name (speakers only sync with others in the same group)
- `channel` -- the channel assignment (`stereo`, `left`, or `right`)

Discovered peers expire after 120 seconds of inactivity.

For speakers on different subnets, you can list static peer IPs in the
`peers` bootstrap list (see configuration below). Bootstrap peers are
reconnected every 60 seconds if not already connected.

### Peer Connections

Speakers connect to each other over TCP on port 48200. To avoid duplicate
connections, the speaker with the lexicographically smaller peer ID always
initiates the connection. Once connected, peers exchange a PeerInfo handshake
(name, peer ID, channel assignment), then communicate using a binary wire
protocol (`HG` magic, little-endian, 12-byte header + payload).

On connect, each peer sends a gossip message listing all other known peers.
This lets new speakers discover the full group without waiting for the next
mDNS cycle.

### Leader Election

When a speaker starts producing audio (Spotify, Bluetooth, etc.) and has at
least one connected peer, it triggers a leader election. Elections also
happen when the current leader disconnects or times out.

Each speaker computes an election score based on:

1. **Average RTT** to all connected peers (lower is better -- the most central
   speaker wins)
2. **Uptime** (longer uptime earns a stability bonus of up to 1,000,000 points)
3. **Peer ID hash** (deterministic tiebreaker via FNV-1a)

The lowest score wins. All peers cast votes, and the election resolves when
either all votes are collected or a 1.5-second timeout expires. A 5-second
cooldown prevents rapid re-elections. Re-election is suppressed when the
current leader's score is within 20% of the best candidate.

### Roles

| Role | Behavior |
|------|----------|
| **Standalone** | Normal operation |
| **Leader** | Taps the audio mixer, sends 10ms audio chunks (480 stereo frames at 48 kHz) to all followers with a `play_at` timestamp. |
| **Follower** | Receives audio chunks, buffers them in a jitter buffer, and feeds them to the local mixer. Local sources (Spotify, Bluetooth) continue playing alongside group audio -- they are not suspended. |

The leader sets each chunk's `play_at` timestamp by adding a playout lead to
the current clock. This lead gives followers time to receive, buffer, and
schedule playback. The lead is owned internally by the buffer controller, not a
user-configurable value.

When all local audio sources stop on the leader, it releases leadership and
broadcasts a `LeaderRelease` to all peers. All speakers return to standalone or to their original groups if [Party Mode](groups.md#party-mode) was in use and is disabled.

### Clock Synchronization

Followers synchronize their clocks with the leader using an NTP-like protocol:

1. Follower sends `ClockSyncReq` with its local timestamp (T1)
2. Leader records receive time (T2), responds with T2 and transmit time (T3)
3. Follower records receive time (T4), computes offset and RTT:
   - **Offset** = ((T2 - T1) + (T3 - T4)) / 2
   - **RTT** = (T4 - T1) - (T3 - T2)

Offset and RTT are smoothed with an exponential moving average (alpha = 0.2).
Sync requests are sent every 2 seconds during convergence (first 16 samples),
then every 10 seconds in steady state. The clock is considered converged after
4 samples.

### Jitter Buffer

Each follower maintains a jitter buffer that holds incoming audio chunks and
releases them at the correct local time (converted from the leader's clock
using the computed offset). Chunks are inserted in sorted order by play time;
out-of-order packets are handled by binary-search insertion.

The buffer keeps the beat aligned without altering playback speed:

- On packet loss, the follower inserts exact-length silence so the timeline
  never slips. The next real chunk lands at the same play time it would have
  without the loss.
- Duplicate chunks are dropped and stale ones are rejected by sequence number,
  so a late or repeated packet never plays twice or out of order.
- There is no time-stretching or compression. Audio is never sped up or slowed
  down to chase buffer fullness, so there is no audible wobble.

Buffer health (0-100%) is reported to the leader via periodic health pings
every 5 seconds.

### Relay Topology

The wire protocol carries a relay tree so that, for larger groups,
well-connected speakers could rebroadcast audio to poorly-connected peers
(leader at the root, maximum hop count 3). This is inactive in v1: the leader
does not currently compute or send relay assignments, so all followers receive
audio directly from the leader. The receive-side handler and wire codec exist,
but nothing produces a relay tree yet.

### Volume Sync

Volume changes (from the volume ring or web UI) are broadcast to all peers
with an originator ID to prevent feedback loops. Every speaker in the group
stays at the same volume.

This is the only configuration synced across speakers in a group, and syncs across all groups in party mode. Returns to local group defaults when party mode it turned off.

Equalizer / custom audio settings are not synchronized.

---

## Configuration

Add a `[group]` section to `/lsync/encore/config.toml`:

```toml
[group]
enabled = true
group_name = "Home"
channel = "stereo"
# peer_id is auto-generated on first boot -- do not edit
# peers = ["192.168.1.50", "10.0.0.5"]
# party_mode = false
```

### Fields

| Field        | Type     | Default    | Description |
|--------------|----------|------------|-------------|
| `enabled`    | bool     | `false`    | Enable multi-speaker group sync. |
| `group_name` | string   | `"Home"`   | Group name. Speakers with the same name sync together. |
| `channel`    | string   | `"stereo"` | Channel assignment: `"stereo"`, `"left"`, or `"right"`. |
| `peer_id`    | string   | *auto*     | UUID v4 generated on first boot. Do not edit. |
| `peers`      | string[] | `[]`       | Bootstrap peer IP addresses for cross-subnet discovery. |
| `party_mode` | bool     | `false`    | Accept audio streams from leaders in any group, not just your own. |

Group changes require a reboot.

---

## Channel Assignment

Each speaker can be assigned a channel:

| Channel    | Behavior |
|------------|----------|
| `"stereo"` | Plays the full stereo mix (default). Both channels pass through unmodified. |
| `"left"`   | Extracts the left channel from the incoming stream and plays it on both drivers. |
| `"right"`  | Extracts the right channel from the incoming stream and plays it on both drivers. |

To create a stereo pair, assign one speaker as `"left"` and another as
`"right"`. Both speakers play the assigned channel through both of their
drivers (mono output from that channel).

Channel assignment can be changed at runtime from the web dashboard without
rebooting.

---

## Examples

### Multi-Room (All Stereo)

Three speakers playing the same audio in every room:

```toml
# Speaker 1: Living Room
[group]
enabled = true
group_name = "Home"

# Speaker 2: Kitchen
[group]
enabled = true
group_name = "Home"

# Speaker 3: Bedroom
[group]
enabled = true
group_name = "Home"
```

### Stereo Pair

Two speakers acting as left and right channels:

```toml
# Speaker 1: Left
[group]
enabled = true
group_name = "Living Room"
channel = "left"

# Speaker 2: Right
[group]
enabled = true
group_name = "Living Room"
channel = "right"
```

### Cross-Subnet

Speakers on different VLANs (mDNS won't cross subnets, so use bootstrap peers):

```toml
# Speaker on 192.168.1.x
[group]
enabled = true
group_name = "Home"
peers = ["192.168.2.50"]

# Speaker on 192.168.2.x
[group]
enabled = true
group_name = "Home"
peers = ["192.168.1.100"]
```

---

## Troubleshooting

**Speakers don't discover each other**
- All speakers must be on the same subnet for mDNS discovery. Speakers on
  different subnets or VLANs need bootstrap `peers` configured.
- Verify that `group_name` matches exactly (case-sensitive) on all speakers.
- Make sure `enabled = true` on all speakers.
- Check that TCP port 48200 is not blocked by your router or AP isolation
  settings. Many consumer routers enable "client isolation" on guest networks,
  which blocks device-to-device traffic.

**Audio stutters or glitches on followers**
- Check buffer health in the web dashboard (Speakers page). Healthy buffers
  stay at 80-100%. Below 50% indicates network issues. On packet loss the
  follower fills the gap with silence rather than slipping the beat, so a low
  buffer health reading points at the network rather than at a tuning value.
- 5 GHz WiFi is preferred over 2.4 GHz for lower jitter. Note that running
  the AP on 2.4 GHz alongside a 5 GHz WiFi connection can cause interference
  on the single-radio chip.

**Wrong speaker becomes leader**
- The speaker with the lowest average RTT to all peers wins the election.
  This is by design -- the most central speaker minimizes total streaming
  latency.
- Uptime also factors in: a speaker that has been running longer gets a
  stability bonus. Restarting a speaker resets its uptime score.

**Leader disconnects and audio stops**
- When the leader disconnects or times out (15 seconds with no heartbeat,
  confirmed after 2 missed health checks), followers detect it and trigger a
  re-election. If no other speaker has active audio sources, all speakers
  return to standalone mode.

**Volume is different on some speakers**
- Volume sync broadcasts to all peers in the group. If a speaker was
  temporarily disconnected when the volume changed, it may be out of sync.
  Adjust the volume again from any speaker in the group to re-sync.

---

## Network Requirements

- All speakers on the same LAN subnet (or use bootstrap `peers` for cross-subnet)
- TCP port 48200 open between speakers
- mDNS (UDP port 5353) for automatic discovery
- No AP isolation / client isolation between speakers
- Recommended: 5 GHz WiFi for lower jitter

---

## Related Documentation

- [Configuration Reference](config-reference.md) -- complete config.toml reference
- [Architecture](architecture.md) -- subsystem overview and boot process
- [Troubleshooting](troubleshooting.md) -- general recovery procedures
