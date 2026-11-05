//! Core types, lock-free shared-memory multi-client audio bus, and DSP primitives
//! for AudioWireCraft.
//!
//! Designed so a single studio soundcard (e.g. Focusrite Scarlett 2i2 4th Gen)
//! can operate at 32-frame low latency while being simultaneously shared between
//! DAWs (Reaper, SoundCraft, TONE3000) and desktop/system audio — just like macOS CoreAudio.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Shared memory path in `/dev/shm` for zero-copy multi-client audio routing.
pub const SHM_BUS_NAME: &str = "/audiowirecraft_bus_v1";

/// Maximum number of simultaneous client streams on the shared bus.
pub const MAX_CLIENTS: usize = 8;

/// Maximum frames per period supported in shared memory ring buffers.
pub const MAX_RING_FRAMES: usize = 4096;

/// Maximum channels per stream.
pub const MAX_CHANNELS: usize = 8;

/// Supported studio buffer sizes (frames per period).
pub const SUPPORTED_BUFFER_SIZES: &[u32] = &[16, 32, 48, 64, 96, 128, 256, 512];

/// Supported studio sample rates (Hz).
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000];

/// Operating mode of an audio interface in AudioWireCraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingMode {
    /// CoreAudio-style Shared Low-Latency Bus:
    /// Engine drives hardware at 32–64 frames and mixes DAWs (Reaper, SoundCraft, TONE3000)
    /// together with system/browser audio without locking out desktop apps.
    SharedLowLatencyBus,
    /// PipeWire Pro-Audio Quantum Tuning:
    /// Keeps the device inside the existing session graph so voice chat and desktop apps
    /// continue untouched, while forcing a 32-frame quantum and registering a direct ALSA PCM alias.
    SystemSharedTuned,
    /// Exclusive Direct Hardware (`hw:` MMAP):
    /// Acquires `org.freedesktop.ReserveDevice1.Audio<N>` so the DAW talks directly to DMA.
    ExclusiveDirectHw,
}

impl RoutingMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SharedLowLatencyBus => "CoreAudio-Style Shared Bus (DAW + System, 32f)",
            Self::SystemSharedTuned => "Shared System + ALSA Bridge (Voice + DAW)",
            Self::ExclusiveDirectHw => "Exclusive Direct HW (DAW Only, Direct DMA)",
        }
    }
}

/// Real-time hardware stream status read from `/proc/asound/cardX/...`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareStreamStatus {
    pub is_running: bool,
    pub access_mode: String,
    pub format: String,
    pub rate: u32,
    pub channels: u32,
    pub period_size: u32,
    pub buffer_size: u32,
    pub usb_packet_interval_us: Option<u32>,
}

/// Channel descriptor for macOS Audio MIDI Setup style display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStrip {
    pub index: usize,
    pub name: String,
    pub is_input: bool,
    pub gain_db: f32,
    pub muted: bool,
    pub direct_monitor: bool,
    pub peak_level: f32,
}

/// Discovered ALSA soundcard descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceDescriptor {
    pub card_index: i32,
    pub id: String,
    pub name: String,
    pub long_name: String,
    pub is_usb: bool,
    pub playback_channels: u32,
    pub capture_channels: u32,
    pub supported_rates: Vec<u32>,
    pub supported_formats: Vec<String>,
    pub playback_status: HardwareStreamStatus,
    pub capture_status: HardwareStreamStatus,
    pub inputs: Vec<ChannelStrip>,
    pub outputs: Vec<ChannelStrip>,
}

impl AudioDeviceDescriptor {
    /// Calculates the current hardware single-period latency in milliseconds.
    pub fn current_period_latency_ms(&self) -> f32 {
        let rate = if self.playback_status.rate > 0 {
            self.playback_status.rate
        } else {
            48_000
        };
        let period = if self.playback_status.period_size > 0 {
            self.playback_status.period_size
        } else {
            256
        };
        (period as f32 * 1000.0) / (rate as f32)
    }

    /// Calculates the current total ring-buffer latency in milliseconds.
    pub fn current_buffer_latency_ms(&self) -> f32 {
        let rate = if self.playback_status.rate > 0 {
            self.playback_status.rate
        } else {
            48_000
        };
        let buf = if self.playback_status.buffer_size > 0 {
            self.playback_status.buffer_size
        } else {
            768
        };
        (buf as f32 * 1000.0) / (rate as f32)
    }
}

/// Persistent engine and routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub selected_card_index: i32,
    pub selected_card_id: String,
    pub sample_rate: u32,
    pub period_size: u32,
    pub periods: u32,
    pub routing_mode: RoutingMode,
    pub expose_alsa_plugin: bool,
    pub alsa_pcm_name: String,
    pub direct_monitor_enabled: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            selected_card_index: 0,
            selected_card_id: "Gen".to_string(),
            sample_rate: 48_000,
            period_size: 32,
            periods: 3,
            routing_mode: RoutingMode::SharedLowLatencyBus,
            expose_alsa_plugin: true,
            alsa_pcm_name: "audiowirecraft".to_string(),
            direct_monitor_enabled: false,
        }
    }
}

impl EngineConfig {
    /// Theoretical one-way period latency in milliseconds.
    pub fn target_period_latency_ms(&self) -> f32 {
        (self.period_size as f32 * 1000.0) / (self.sample_rate.max(1) as f32)
    }

    /// Theoretical round-trip DMA latency (input period + output buffer) in milliseconds.
    pub fn target_roundtrip_latency_ms(&self) -> f32 {
        let frames = self.period_size * (self.periods + 1);
        (frames as f32 * 1000.0) / (self.sample_rate.max(1) as f32)
    }
}

/// Lock-free client slot inside the shared memory audio bus.
#[repr(C)]
pub struct ShmClientSlot {
    pub active: AtomicU32,
    pub pid: AtomicU32,
    pub write_pos: AtomicU64,
    pub read_pos: AtomicU64,
    /// Interleaved f32 playback ring buffer (`MAX_RING_FRAMES * 2` stereo samples).
    pub playback_ring: [AtomicU32; MAX_RING_FRAMES * 2],
}

/// Lock-free shared memory header mapped at `/dev/shm/audiowirecraft_bus_v1`.
#[repr(C)]
pub struct ShmAudioBusHeader {
    pub magic: u32,
    pub version: u32,
    pub sample_rate: AtomicU32,
    pub period_size: AtomicU32,
    pub xrun_count: AtomicU64,
    pub engine_heartbeat: AtomicU64,
    /// Latest capture samples (`MAX_RING_FRAMES * 4` channels) published by the engine
    /// so multiple clients (Reaper, SoundCraft, TONE3000) can read hardware inputs simultaneously.
    pub capture_write_pos: AtomicU64,
    pub capture_ring: [AtomicU32; MAX_RING_FRAMES * 4],
    /// Peak levels in `f32::to_bits()` for 4 inputs and 2 outputs.
    pub input_peaks_bits: [AtomicU32; 4],
    pub output_peaks_bits: [AtomicU32; 2],
    pub clients: [ShmClientSlot; MAX_CLIENTS],
}

pub const SHM_MAGIC: u32 = 0x4157_4331; // "AWC1"

/// Fast lossless conversion from 32-bit signed PCM (`S32_LE`, used by Focusrite Scarlett 4th Gen)
/// to normalized `f32` in `[-1.0, 1.0]`.
#[inline(always)]
pub fn s32_le_to_f32(sample: i32) -> f32 {
    (sample as f64 / 2_147_483_648.0) as f32
}

/// Fast saturating conversion from normalized `f32` in `[-1.0, 1.0]` to `S32_LE`.
#[inline(always)]
pub fn f32_to_s32_le(sample: f32) -> i32 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped as f64 * 2_147_483_647.0) as i32
}

/// Helper to store an `f32` atomically as `u32` bits.
#[inline(always)]
pub fn store_atomic_f32(slot: &AtomicU32, val: f32) {
    slot.store(val.to_bits(), Ordering::Relaxed);
}

/// Helper to load an `f32` atomically from `u32` bits.
#[inline(always)]
pub fn load_atomic_f32(slot: &AtomicU32) -> f32 {
    f32::from_bits(slot.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_s32_f32_roundtrip() {
        let full_pos = f32_to_s32_le(1.0);
        assert_eq!(full_pos, i32::MAX);
        let back = s32_le_to_f32(full_pos);
        assert!((back - 1.0).abs() < 1e-6);

        let zero = f32_to_s32_le(0.0);
        assert_eq!(zero, 0);
    }

    #[test]
    fn test_latency_calculation() {
        let cfg = EngineConfig {
            sample_rate: 48_000,
            period_size: 32,
            periods: 2,
            ..Default::default()
        };
        assert!((cfg.target_period_latency_ms() - 0.6666667).abs() < 1e-4);
        assert!((cfg.target_roundtrip_latency_ms() - 2.0).abs() < 1e-4);
    }
}
