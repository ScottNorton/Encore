# Legal Notice

## Disclaimer

This software is provided "as is", without warranty of any kind, express or
implied. Installing custom firmware on your Harman Kardon Invoke carries
inherent risks, including but not limited to:

- Voiding any remaining manufacturer warranty
- Rendering the device temporarily or permanently inoperable
- Loss of original Cortana functionality (already discontinued by Microsoft)

The authors and contributors are not responsible for any damage to your
hardware, data loss, or other consequences of using this software. You use
this firmware at your own risk.

## Trademarks

"Harman Kardon" and "Invoke" are registered trademarks of Harman International
Industries, Incorporated, a subsidiary of Samsung Electronics Co., Ltd.

"Cortana" and "Microsoft" are registered trademarks of Microsoft Corporation.

"Spotify" is a registered trademark of Spotify AB.

"Bluetooth" is a registered trademark of Bluetooth SIG, Inc.

"Home Assistant" is a trademark of Nabu Casa, Inc.

This project is not affiliated with, endorsed by, sponsored by, or associated
with any of these companies. All trademarks are the property of their
respective owners and are used here solely for identification and
interoperability purposes.

## Reverse Engineering

Hardware documentation, communication protocols, and technical specifications
in this repository were obtained through clean-room reverse engineering of
devices legally owned and purchased by the contributors. This work was
performed for the sole purpose of achieving interoperability with the
contributors' own hardware, as permitted under:

- **United States**: 17 U.S.C. § 1201(f) — Reverse Engineering Exception
  (interoperability with independently created programs)
- **European Union**: Directive 2009/24/EC, Article 6 — Decompilation for
  Interoperability
- **Other jurisdictions**: Equivalent interoperability provisions under
  applicable local law

No proprietary source code was copied or redistributed. All firmware and
application code in this repository is original work by the contributors or
derived from open-source components under their respective licenses.

DSP reverse engineering tools (disassembler, assembler) and any custom DSP
firmware in this repository are original works created through clean-room
analysis of the instruction set architecture and publicly available
documentation. The stock DSP firmware binary is not distributed; only
independently-authored tools and research documentation are included.

## Vendor Software

This project **does not distribute** proprietary firmware images. The build
process currently requires a stock firmware image that users must obtain independently.

The vendor's Linux kernel source code was made available under the terms of the
GNU General Public License v2.

## Third-Party Software

Encore incorporates or depends on the following open-source projects:

| Project | License | Purpose |
|---------|---------|---------|
| librespot | MIT | Spotify Connect protocol implementation |
| Tokio | MIT | Asynchronous runtime |
| Axum | MIT | HTTP/WebSocket server |
| nix | MIT | Unix system call bindings (ALSA PCM, BT sockets) |
| boringtun | BSD-3-Clause | WireGuard VPN implementation |
| rumqttc | Apache-2.0 | MQTT client |
| rkyv | MIT | Zero-copy serialization |
| rust-embed | MIT | Static asset embedding |
| wasm-bindgen | MIT / Apache-2.0 | WebAssembly bindings |
| Zig | MIT | Cross-compilation toolchain |
| tracing | MIT | Structured logging |

The complete dependency list with pinned versions is specified in
`encore/Cargo.toml`.

## Contact

If you are a rights holder and believe this project infringes on your
intellectual property, please open an issue on the repository. We take these
matters seriously and will respond promptly.
