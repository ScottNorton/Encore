//! Audio pipeline — mixer, resampler, PCM output, ALSA control, and subsystem.
//!
//! The mixer combines multiple audio sources (Spotify, Bluetooth, Wyoming TTS)
//! into a single stereo S32 stream at 48kHz. The PCM module writes to ALSA
//! hardware (hw:1) via direct ioctl for minimal latency.

#[cfg(target_os = "linux")]
pub mod alsa_ctl;
#[cfg(target_os = "linux")]
pub mod capture;
pub mod mixer;
#[cfg(target_os = "linux")]
pub mod pcm;
pub mod resample;
#[cfg(target_os = "linux")]
pub mod subsystem;
