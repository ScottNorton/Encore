# Legal Notice
![US Flag](https://flagcdn.com/w20/us.png) *Encore, an American open-source project, built on the principle that you own what you buy.*

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

"aptX" and "aptX HD" are trademarks of Qualcomm Technologies, Inc.

"WireGuard" is a registered trademark of Jason A. Donenfeld.

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

## Your Rights

You own your hardware. Encore exists because of that simple fact.

When you purchased a Harman Kardon Invoke, you acquired a physical device that
belongs to you. The decision to retire Cortana was made unilaterally by Microsoft
without meaningful consent from device owners. Encore restores functionality to
hardware you already paid for.

Your right to install and use this firmware is grounded in established law and
a growing international consensus that owners — not manufacturers — decide what
runs on hardware they've purchased.

### Ownership & Right to Repair

No manufacturer or software provider can dictate how you use your own property
after purchase. A growing body of legislation affirms your right to maintain,
repair, and restore functionality to products you own — including through
third-party or community-developed software.

**United States:** Five states have enacted Right to Repair laws for consumer
electronics — New York, Minnesota, California, Colorado, and Oregon — with all
fifty states having considered similar legislation. These laws require
manufacturers to make repair tools, parts, and documentation available to
consumers and independent repair providers. Several explicitly void any
contractual provision (including EULAs) that conflicts with these rights.

The Federal Trade Commission (FTC) unanimously adopted a
[Policy Statement on Repair Restrictions](https://www.ftc.gov/legal-library/browse/policy-statement-federal-trade-commission-repair-restrictions-imposed-manufacturers-sellers)
in July 2021, identifying software locks, DRM, and overbroad assertions of
patent and trademark rights as enforcement priorities. The FTC has sent warning
letters to manufacturers whose terms exceed what intellectual property law
actually protects.

### DMCA Exemptions

The U.S. Copyright Office grants exemptions to the DMCA's anti-circumvention
provisions every three years. The voice assistant jailbreaking exemption
(37 CFR 201.40(b)(11)) — covering devices like the Amazon Echo and Google
Home — directly applies to the Invoke. These exemptions have been renewed
through October 2027. The interoperability exception at 17 U.S.C. § 1201(f)
is permanent and does not expire.

### No Active Service Agreement

The Invoke has no active service contract, subscription, or cloud dependency
that this firmware circumvents. Cortana was retired by Microsoft in January 2021.
The speaker was abandoned. There is nothing left to violate.

You are not breaking anything. You are fixing what was broken for you.

### Manufacturer EULA & Terms

Harman Kardon's Invoke EULA contains broad prohibitions on modifying,
reverse engineering, or redistributing the device's software, and claims
ownership of all intellectual property in the software and its derivatives.

These terms do not prevent you from using Encore:

- **The Invoke runs GPL-licensed software.** Harman shipped the Invoke with a
  Linux kernel licensed under GPL v2 and published the source code to fulfill
  their obligations. The GPL explicitly prohibits distributors from imposing
  further restrictions on recipients' rights. Any EULA term that restricts
  modification of GPL-covered components is void under the license itself.
  Courts have consistently upheld this principle.

- **Encore does not modify Harman's proprietary software.** Encore is original
  Rust code written from scratch. It replaces Harman's discontinued proprietary
  user-space entirely — no Harman binaries are copied, modified, or
  redistributed.

- **EULA restrictions are contractual, not copyright conditions.** Courts have
  held that EULA "no modify" clauses are contractual covenants, not copyright
  conditions. Violating them is a breach of contract — not copyright
  infringement — with typically minimal damages.

- **Post-sale exhaustion limits manufacturer control.** The U.S. Supreme Court
  has held that selling a product exhausts the seller's patent rights in that
  product, regardless of any post-sale restrictions.

- **Right to Repair laws void conflicting EULA terms.** Multiple state statutes
  explicitly provide that contractual provisions attempting to limit the
  manufacturer's repair obligations are void and unenforceable.

- **The warranty is already void.** The Invoke is long past its warranty period,
  the warranty was non-transferable, and the original product functionality no
  longer exists.

**A note to the Invoke's creators:** 
>This project exists because you built something worth
saving. The Invoke's acoustic engineering, premium design, and component
quality are why a community formed around it years after discontinuation. We
are not adversaries — we are customers who liked what you made. Encore is GPL-3.0. 
You are welcome to fork it, contribute to it, or use it as a reference. 
The door is open. It would be great to see you here!

### Learn More

For more information on the Right to Repair movement and your rights as a
device owner:

- [The Repair Association](https://www.repair.org/) (repair.org) — U.S. Right
  to Repair coalition tracking legislation across all 50 states
- [Fight to Repair](https://www.fighttorepair.org/) (fighttorepair.org) —
  Newsletter and advocacy covering global repair legislation and enforcement
- [iFixit Right to Repair](https://www.ifixit.com/Right-to-Repair) — Repair
  advocacy, guides, and policy tracking
- [Electronic Frontier Foundation](https://www.eff.org/issues/right-to-repair)
  (eff.org) — DMCA exemption petitions, digital rights advocacy

## Project & Contributor Protections

Encore is released under the [GNU General Public License v3.0](LICENSE). This
license was chosen deliberately to protect the project, its contributors, and
every user who benefits from it.

**What GPL-3.0 guarantees:**

- **Freedom to use** — anyone can run Encore for any purpose
- **Freedom to study** — the full source code is always available
- **Freedom to share** — anyone can redistribute Encore
- **Freedom to improve** — anyone can modify and share their modifications
- **Copyleft** — derivative works must also be open source under GPL-3.0. No
  one can take this community's work, close the source, and sell it back to you.

**Contributor protections:**

- Contributors retain copyright over their individual contributions. By
  submitting code, you grant a license under GPL-3.0 but do not transfer
  ownership.
- No contributor is liable for damages arising from the use of this software.
  See [Disclaimer](#disclaimer).
- Contributors are protected by the same reverse engineering provisions that
  protect end users. See [Reverse Engineering](#reverse-engineering).

**Project integrity:**

- "Encore" as used in the context of this firmware project is an unregistered
  trademark of its contributors. Forks and derivatives are welcome under
  GPL-3.0, but should not use the Encore name in a way that implies official
  endorsement or affiliation.
- This project does not accept contributions that contain proprietary code,
  code obtained through NDA violations, or code copied from sources
  incompatible with GPL-3.0.
- No entity — corporate, governmental, or individual — may claim exclusive
  rights over this project or its community-developed codebase. The GPL-3.0
  license is irrevocable once granted.

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
| libfreeaptx | LGPL-2.1-or-later | aptX / aptX HD Bluetooth codec decoder (vendored, statically linked) |
| libsbc | Apache-2.0 | SBC Bluetooth codec decoder (vendored, statically linked) |

The complete Rust dependency list with pinned versions is specified in
`encore/Cargo.toml`. Vendored C sources for the Bluetooth codecs are in
`encore/crates/encore-firmware/csrc/` with their upstream license files
preserved alongside the code.

## Contact

If you are a rights holder and believe this project infringes on your
intellectual property, please open an issue on the repository. We take these
matters seriously and will respond promptly.
