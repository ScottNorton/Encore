# Security

## Default Credentials

The following default credentials are **intentional** and documented:

- **SSH**: root / `ridiculous`
- **WiFi AP**: `Invoke-XXXX` / `ridiculous` (SSID is unique per device, derived from MAC)

The password is the same for all devices running Encore. The SSID varies per device. The device is designed for
use on trusted local networks. If you expose your Invoke to the internet,
change the root password via SSH (`passwd`).

## TLS Certificates

Each speaker makes its own HTTPS certificate at first boot, so no certificate or key is shared between devices. A spare certificate and key are created on your machine when you build the firmware (`build/tls/`, see the [build guide](../docs/build-guide.md#tls-fallback-pair)). The speaker uses them only if it cannot make its own. They are never committed or published. Browsers do not trust either one until you install the speaker's CA from `/ca.crt`.

## Reporting Vulnerabilities

If you find a security issue beyond the intentional defaults above, please
**do not open a public issue**. Instead, use GitHub's private vulnerability
reporting:

1. Go to the Security tab on this repository
2. Click "Report a vulnerability"
3. Describe the issue

You'll get a response within a few days.
