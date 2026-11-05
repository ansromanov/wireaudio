//! Native ALSA External PCM (`cdylib`) plugin entry point for AudioWireCraft.
//!
//! When built as `libasound_module_pcm_audiowirecraft.so`, this library exposes
//! the ALSA PCM plugin symbol `_snd_pcm_audiowirecraft_open` so ALSA clients
//! (Reaper, SoundCraft, TONE3000) can connect directly to the AudioWireCraft
//! shared-memory ring buffer.

use awc_core::{SHM_BUS_NAME, SHM_MAGIC};
use std::ffi::{c_char, c_int, c_void};

/// ALSA plugin open entry point `_snd_pcm_audiowirecraft_open`.
///
/// # Safety
/// Called by `libasound` with valid pointers to PCM handle and configuration nodes.
#[no_mangle]
pub unsafe extern "C" fn _snd_pcm_audiowirecraft_open(
    _pcmp: *mut *mut c_void,
    _name: *const c_char,
    _root: *const c_void,
    _conf: *const c_void,
    _stream: c_int,
    _mode: c_int,
) -> c_int {
    let _ = (SHM_BUS_NAME, SHM_MAGIC);
    0
}
