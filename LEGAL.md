# Legal Notice

## Disclaimer

This software is provided "as is", without warranty of any kind, express or implied.
Installing custom firmware on your Harman Kardon Invoke carries inherent risks, including
but not limited to:

- Voiding any remaining manufacturer warranty
- Rendering the device temporarily or permanently inoperable
- Loss of original Cortana functionality (already discontinued by Microsoft)

The authors and contributors are not responsible for any damage to your hardware, data
loss, or other consequences of using this software. You use this firmware at your own risk.

## Trademarks

"Harman Kardon" and "Invoke" are registered trademarks of Harman International Industries,
Incorporated, a subsidiary of Samsung Electronics Co., Ltd.

"Cortana" and "Microsoft" are registered trademarks of Microsoft Corporation.

"Spotify" is a registered trademark of Spotify AB.

"Bluetooth" is a registered trademark of Bluetooth SIG, Inc.

"Home Assistant" is a trademark of Nabu Casa, Inc.

"aptX" and "aptX HD" are trademarks of Qualcomm Technologies, Inc.

"WireGuard" is a registered trademark of Jason A. Donenfeld.

This project is not affiliated with, endorsed by, sponsored by, or associated with any of
these companies. All trademarks are the property of their respective owners and are used
here solely for identification and interoperability purposes.

## Reverse Engineering

The hardware documentation, communication protocols, and technical specifications in this
repository were produced by the contributors through reverse engineering of devices they
legally own. The work included bus monitoring, firmware image analysis, and disassembly and
decompilation of the stock binaries. Its sole purpose is interoperability: making
independently created software run on the contributors' own hardware.

The legal basis for this work:

- **17 U.S.C. § 1201(f)** permits circumvention and reverse engineering for the purpose of
  achieving interoperability with independently created computer programs. This provision
  is permanent and does not expire.
- The U.S. Copyright Office's triennial DMCA exemptions (37 C.F.R. § 201.40) include an
  exemption for jailbreaking voice assistant devices, renewed in the 2024 rulemaking.
- U.S. case law (*Sega v. Accolade*, *Sony v. Connectix*) holds that disassembly of a
  program to study its unprotected functional elements for interoperability is fair use.
- Other jurisdictions have equivalent interoperability provisions (for example,
  EU Directive 2009/24/EC, Article 6).

No stock source code or binary code has been copied into Encore. Encore is original Rust
code that reimplements the documented interfaces. Protocols, register maps, and interface
facts are not subject to copyright. The stock firmware, the DSP firmware, the MCU firmware,
and the decompilation outputs used during research are **not** distributed in this
repository.

## Owner Rights

- You own your hardware. What software runs on it after purchase is your decision.
- The Invoke shipped with the Linux kernel under GPL v2, and Harman published the
  corresponding source code. The GPL prohibits adding restrictions on recipients' rights to
  modify and run that software; an EULA term cannot override it.
- Cortana service ended in January 2021. This firmware does not circumvent any active
  service, subscription, or business relationship.
- U.S. right-to-repair legislation and the FTC's 2021
  [Policy Statement on Repair Restrictions](https://www.ftc.gov/legal-library/browse/policy-statement-federal-trade-commission-repair-restrictions-imposed-manufacturers-sellers)
  treat software locks that prevent owners from restoring their own devices as an
  enforcement concern, and several state statutes void contract terms that conflict with
  repair rights.

## Project License and Contributions

Encore is released under the [GNU General Public License v3.0](LICENSE). Documentation in
`docs/` is licensed under
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

- Contributors retain copyright over their contributions and license them under GPL-3.0 by
  submitting them. Ownership is not transferred.
- Derivative works must remain open source under GPL-3.0. The license, once granted, is
  irrevocable.
- No contributor is liable for damages arising from use of this software (see
  [Disclaimer](#disclaimer)).
- Contributions containing proprietary code, code obtained under NDA, or code under
  GPL-incompatible licenses are not accepted.
- "Encore" as used by this firmware project is an unregistered trademark of its
  contributors. Forks are welcome under the GPL, but should not use the name in a way that
  implies endorsement or affiliation.

## Vendor Software

This project does not distribute proprietary firmware images. The build process requires a
stock firmware image that users must obtain independently. The vendor's Linux kernel source
was published under the GNU General Public License v2.

The Marvell WLAN driver configuration files in `rootfs/usr/share/factory/misc_config/` are
unmodified functional configuration data from the stock device; see the
[README in that directory](rootfs/usr/share/factory/misc_config/README.md) for provenance
and contact information.

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
| libfreeaptx | LGPL-2.1-or-later | aptX / aptX HD Bluetooth codec decoder (vendored, statically linked) |
| libsbc | Apache-2.0 | SBC Bluetooth codec decoder (vendored, statically linked) |

The complete Rust dependency list with pinned versions is in `encore/Cargo.toml`. Vendored
C sources for the Bluetooth codecs are in `encore/crates/encore-firmware/csrc/` with their
upstream license files preserved alongside the code.

## Contact

If you are a rights holder and believe this project infringes on your intellectual
property, please open an issue on the repository. We take these matters seriously and will
respond promptly. For security vulnerabilities, use GitHub's private vulnerability
reporting (see [SECURITY.md](.github/SECURITY.md)).
