# USB Access Guide

How to reach the speaker over a USB cable, with no WiFi involved. The speaker exposes a USB
network gadget: plug it into a computer and it appears as a USB network adapter at the
address `10.55.55.1`. From there you get the web dashboard, SSH, and the same API the
desktop and mobile apps use.

This works independently of WiFi and comes up automatically at boot, so it also doubles as a
recovery channel when WiFi or the access point is unavailable.

## What You Need

- A USB cable that fits the speaker's USB **Mini-B** port. Mini-B is the older, wider
  connector, not the Micro-B common on old phones.
- A computer with a USB port. On Windows the built-in RNDIS driver binds automatically, so
  there is nothing to install.

## Steps

1. Plug the USB cable into the speaker's Mini-B port and into your computer.
2. Wait a few seconds. The speaker shows up as a USB network adapter, and its built-in DHCP
   server hands your computer an address automatically (from the pool `10.55.55.10` to
   `10.55.55.50`). The speaker itself is at `10.55.55.1`.
3. Open the dashboard: browse to `http://10.55.55.1/`. This link is plain HTTP, so there is
   no certificate prompt.
4. For a shell, SSH in as `root`:

   ```bash
   ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa root@10.55.55.1
   ```

   The password is `ridiculous`. The extra `-o` flags are needed because the speaker runs
   Dropbear, an older SSH server that needs the legacy RSA host-key algorithms enabled.

   If you are on an untrusted network, change the password with `passwd` after logging in.
   It is the same on every device by default.

WiFi access still works as before: join the `Invoke-XXXX` network (password `ridiculous`),
let the captive portal open the dashboard, or browse to `http://192.168.43.1`. SSH reaches
the speaker at its LAN or AP address the same way.

## How It Works

- The speaker's DHCP server on the USB link deliberately advertises no gateway and no DNS.
  Plugging in therefore never disturbs your computer's normal internet connection, for
  example a phone tether you are already using.
- The gadget starts automatically at boot, and a small monitor restarts it if the link
  drops, so the USB channel is available without any action on the speaker.
- Because it does not depend on WiFi, it works as a recovery channel when the WiFi network
  or the access point is down.

## Limitations

The USB device controller (Marvell `mv_udc`) stalls multi-packet bulk transfers, so the
gadget MTU is fixed at 400 bytes. That keeps every network frame inside a single USB packet,
which avoids the stall and keeps the link reliable. The cost is throughput: measured around
7 MB/s, which is fine for the dashboard, SSH, and OTA uploads, but slower than a normal USB
link. Raising the MTU brings the stall back. A controller-level fix is future work; the
fixed MTU is the workaround that ships.

## Building and Maintaining

- The gadget kernel modules are baked into the rootfs at `/usr/lib/usbgadget/` and brought
  up by `rootfs/sbin/usb_gadget.sh` (self-healed by `usb_gadget_monitor.sh`).
- The patched `g_ether`/RNDIS modules are rebuilt with
  `scripts/device/build_usb_gadget_modules.sh`, using the period Linaro 4.9.4
  cross-compiler. A modern gcc builds modules that load but then fault the 3.8.13 kernel.
- The kernel patches are tracked as diffs in `scripts/device/usb-gadget-patches/`. The
  modules are GPL-2.0 code, and [LEGAL.md](../LEGAL.md#gpl-components-and-source-availability)
  says where their source is.
