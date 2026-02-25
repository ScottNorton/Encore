//! Audio pipeline — mixer, resampler, PCM output, ALSA control, and subsystem.
//!
//! The mixer combines multiple audio sources (Spotify, Bluetooth, Wyoming TTS)
//! into a single stereo S32 stream at 48kHz. The PCM module writes to ALSA
//! hardware (hw:1) via direct ioctl for minimal latency.

pub mod mixer;
pub mod resample;
#[cfg(target_os = "linux")]
pub mod alsa_ctl;
#[cfg(target_os = "linux")]
pub mod capture;
#[cfg(target_os = "linux")]
pub mod pcm;
#[cfg(target_os = "linux")]
pub mod subsystem;
