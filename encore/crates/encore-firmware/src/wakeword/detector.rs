//! Wake word detector implementations.
//!
//! `StubDetector` is the default — never triggers, placeholder for a real
//! ML engine (tract + openWakeWord ONNX model).

use super::{Detection, WakeWordDetector};

/// Stub detector — never triggers. The full capture pipeline works end-to-end,
/// but wake word detection is deferred until a real engine is added.
pub struct StubDetector;

impl WakeWordDetector for StubDetector {
    fn feed(&mut self, _samples: &[i16]) -> Option<Detection> {
        None
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_detects() {
        let mut det = StubDetector;
        let samples = vec![0i16; 320]; // 20ms at 16kHz
        assert!(det.feed(&samples).is_none());
        assert!(det.feed(&samples).is_none());
    }

    #[test]
    fn stub_reset_is_noop() {
        let mut det = StubDetector;
        det.reset(); // should not panic
    }
}
