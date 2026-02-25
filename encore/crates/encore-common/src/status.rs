//! Subsystem vitality classification.
//!
//! Vital subsystems (watchdog, network, web, audio) trigger firmware
//! restart if they crash beyond MAX_RESTARTS. Non-vital subsystems
//! (Spotify, Bluetooth) degrade gracefully.

/// Vital subsystems that must never stop
pub const VITAL_SUBSYSTEMS: &[&str] = &["watchdog", "network", "web", "audio"];

/// Check if a subsystem name is vital
pub fn is_vital(name: &str) -> bool {
    VITAL_SUBSYSTEMS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vital_subsystems_are_vital() {
        assert!(is_vital("watchdog"));
        assert!(is_vital("network"));
        assert!(is_vital("web"));
        assert!(is_vital("audio"));
    }

    #[test]
    fn non_vital_subsystems_are_not_vital() {
        assert!(!is_vital("spotify"));
        assert!(!is_vital("bluetooth"));
        assert!(!is_vital("wyoming"));
        assert!(!is_vital("homeassistant"));
        assert!(!is_vital("led"));
        assert!(!is_vital("mcu"));
        assert!(!is_vital(""));
        assert!(!is_vital("nonexistent"));
    }
}
