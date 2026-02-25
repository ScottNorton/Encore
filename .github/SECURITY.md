# Security

## Default Credentials

The following default credentials are **intentional** and documented:

- **SSH**: root / `ridiculous`
- **WiFi AP**: `Invoke-XXXX` / `ridiculous` (SSID is unique per device, derived from MAC)

The password is the same for all devices running Encore. The SSID varies per device. The device is designed for
use on trusted local networks. If you expose your Invoke to the internet,
change the root password via SSH (`passwd`).

## Reporting Vulnerabilities

If you find a security issue beyond the intentional defaults above, please
**do not open a public issue**. Instead, use GitHub's private vulnerability
reporting:

1. Go to the Security tab on this repository
2. Click "Report a vulnerability"
3. Describe the issue

You'll get a response within a few days.
