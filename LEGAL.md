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

Encore is released under the [GNU General Public License](LICENSE), version 3 or (at your
option) any later version. Documentation in `docs/` is licensed under
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

## GPL Components and Source Availability

Encore's own code is GPL-3.0-or-later, and its source is this repository. The repository
also carries one set of compiled files that are not Encore's own code:

| Files | License | What they are |
|-------|---------|---------------|
| `rootfs/usr/lib/usbgadget/udc-core.ko`, `mv_udc.ko`, `libcomposite.ko`, `g_ether.ko` | GPL-2.0 (Linux kernel) | The USB network gadget modules, built from the Invoke's vendor kernel with patches from this repository |

**Source for the USB gadget modules.**

1. The kernel tree: the Linux 3.8.13 source for the Invoke that Harman published under the
   GPL. The Internet Archive keeps a copy at <https://archive.org/details/invoke-kernel>
   (`Invoke-kernel.tar`, SHA-1 `0ebb18b574b83f5bf153b0a9355aad74c196ff0a`).
2. The changes made to it: the patch files in
   [`scripts/device/usb-gadget-patches/`](scripts/device/usb-gadget-patches/), which change
   `composite.c`, `ether.c`, `f_rndis.c`, `mv_udc_core.c`, and `u_ether.c`.
3. The build recipe:
   [`scripts/device/build_usb_gadget_modules.sh`](scripts/device/build_usb_gadget_modules.sh)
   sets the kernel options (the gadget drivers as modules) and builds with the Linaro GCC
   4.9.4 cross compiler. The
   [build guide](docs/build-guide.md#building-the-usb-gadget-kernel-modules) explains how to
   run it.

**Written offer.** If you received these modules from this repository and want the
corresponding source in another form, for example the patched tree on its own, open an
issue and it will be provided. This offer is valid for at least three years from the date
the modules were last published here. A short copy of this notice ships next to the modules
as `SOURCE.txt`, so it is also inside firmware images you build.

A firmware image you build yourself also contains the vendor's stock kernel and userland,
taken from your own copy of the vendor image. This project does not distribute those. If you
share a built image, you are distributing them yourself, including their GPL source
obligations.

## Third-Party Software

Encore incorporates or depends on the following open-source projects:

| Project | License | Purpose |
|---------|---------|---------|
| [librespot](https://github.com/librespot-org/librespot) | MIT | Spotify Connect protocol implementation |
| [Tokio](https://github.com/tokio-rs/tokio) | MIT | Asynchronous runtime |
| [Axum](https://github.com/tokio-rs/axum) | MIT | HTTP/WebSocket server |
| [nix](https://github.com/nix-rust/nix) | MIT | Unix system call bindings (ALSA PCM, BT sockets) |
| [boringtun](https://github.com/cloudflare/boringtun) | BSD-3-Clause | WireGuard VPN implementation |
| [rumqttc](https://github.com/bytebeamio/rumqtt) | Apache-2.0 | MQTT client |
| [rkyv](https://github.com/rkyv/rkyv) | MIT | Zero-copy serialization |
| [rust-embed](https://crates.io/crates/rust-embed) | MIT | Static asset embedding |
| [wasm-bindgen](https://github.com/wasm-bindgen/wasm-bindgen) | MIT / Apache-2.0 | WebAssembly bindings |
| [Zig](https://github.com/ziglang/zig) | MIT | Cross-compilation toolchain |
| [tracing](https://github.com/tokio-rs/tracing) | MIT | Structured logging |
| [libfreeaptx](https://github.com/regularhunter/libfreeaptx) | LGPL-2.1-or-later | aptX / aptX HD Bluetooth codec decoder (vendored, statically linked) |
| [libsbc](https://github.com/google/libsbc) | Apache-2.0 | SBC Bluetooth codec decoder (vendored, statically linked) |
| [Inter](https://github.com/rsms/inter) (Regular, Medium) | SIL OFL 1.1 | Dashboard font, embedded in the firmware and served with the dashboard. License text: `encore/web/fonts/OFL.txt` |

The complete Rust dependency list with pinned versions is in `encore/Cargo.toml`. Vendored
C sources for the Bluetooth codecs are in `encore/crates/encore-firmware/csrc/` with their
upstream license files preserved alongside the code. Two librespot crates are kept as local
copies with small patches, `encore/vendor/librespot-audio` and
`encore/crates/vendored/librespot-discovery`. They are MIT licensed, like the rest of
librespot, and each keeps librespot's license file next to the code.

## Contact

If you are a rights holder and believe this project infringes on your intellectual
property, please open an issue on the repository. We take these matters seriously and will
respond promptly. For security vulnerabilities, use GitHub's private vulnerability
reporting (see [SECURITY.md](.github/SECURITY.md)).
