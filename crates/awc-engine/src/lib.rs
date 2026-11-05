//! Real-time studio routing matrix and multi-client session coordinator
//! for AudioWireCraft.

pub use awc_alsa::{
    apply_runtime_quantum_tuning, asoundrc_path, discover_alsa_devices, is_asoundrc_registered,
    list_card_mixer_controls, register_engine_in_asoundrc, unregister_engine_from_asoundrc,
    AlsaRealtimeEngine, LiveEngineTelemetry,
};
pub use awc_core::{
    AudioDeviceDescriptor, ChannelStrip, EngineConfig, HardwareStreamStatus, RoutingMode,
};

/// 4-Input to 2-Output studio monitor routing matrix (designed for Scarlett 2i2 4th Gen:
/// Inputs 1-2 Mic/Hi-Z + Inputs 3-4 Loopback -> Outputs 1-2 Monitor L/R).
#[derive(Debug, Clone)]
pub struct RoutingMatrix4x2 {
    /// Linear gain coefficients `[input_idx][output_idx]`.
    pub gains: [[f32; 2]; 4],
}

impl Default for RoutingMatrix4x2 {
    fn default() -> Self {
        Self {
            gains: [
                [1.0, 0.0], // Input 1 -> Left
                [0.0, 1.0], // Input 2 -> Right
                [0.5, 0.0], // Loopback L -> Left
                [0.0, 0.5], // Loopback R -> Right
            ],
        }
    }
}

impl RoutingMatrix4x2 {
    /// Mixes 4 input samples into a stereo output pair `(left, right)`.
    #[inline(always)]
    pub fn mix_frame(&self, inputs: [f32; 4]) -> (f32, f32) {
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for (i, &sample) in inputs.iter().enumerate() {
            left += sample * self.gains[i][0];
            right += sample * self.gains[i][1];
        }
        (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_stereo_routing() {
        let matrix = RoutingMatrix4x2::default();
        let (l, r) = matrix.mix_frame([0.5, -0.5, 0.0, 0.0]);
        assert!((l - 0.5).abs() < 1e-6);
        assert!((r + 0.5).abs() < 1e-6);
    }
}
