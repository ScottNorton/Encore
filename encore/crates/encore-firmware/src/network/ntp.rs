//! NTP / time sync detection.
//!
//! The Harman Kardon Invoke has no RTC — it boots to Unix epoch (Jan 1 1970).
//! After WiFi connects, NTP syncs the clock. We detect this by checking
//! if system time is after a known threshold (Jan 1 2025).

/// Check if the system clock has been synced (i.e. not still at epoch).
/// Returns true if current time > Jan 1 2025 00:00:00 UTC.
pub fn is_time_synced() -> bool {
    // Jan 1 2025 00:00:00 UTC = 1735689600
    const THRESHOLD: u64 = 1_735_689_600;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now > THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_synced_on_modern_system() {
        // On any dev machine, time should be well past 2025
        assert!(is_time_synced());
    }
}
