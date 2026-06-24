# librespot-audio (vendored + patched)

`librespot-audio` v0.8.0 from [librespot-org/librespot](https://github.com/librespot-org/librespot)
(MIT), vendored so we can apply **upstream PR #1722** ahead of a librespot release.

## Why
librespot 0.8.0 tries only the *first* CDN URL Spotify returns for a track. When that
node answers with a transient non-206 status — commonly a Cloudflare **530** ("origin
unreachable"), which Spotify's CDN has thrown intermittently since late 2025 — librespot
gives up instead of trying the next URL. Result: every track load fails with
`StatusCode(530)` and playback never starts.

## The patch
`src/fetch/mod.rs` → `AudioFileStreaming::open`: the CDN-URL loop now breaks only on
`206 Partial Content` and falls back to the next URL on any other status, matching
[PR #1722](https://github.com/librespot-org/librespot/pull/1722). Only this crate is
patched; the other librespot crates stay on crates.io 0.8.0. Wired via
`[patch.crates-io]` in `encore/Cargo.toml`.

## Remove when
Once PR #1722 (or an equivalent fix) ships in a published librespot release: delete this
directory, drop the `[patch.crates-io]` entry and the `.gitignore` exception, and bump
the librespot version in `encore/Cargo.toml`.

License: MIT — see <https://github.com/librespot-org/librespot/blob/master/LICENSE>.
