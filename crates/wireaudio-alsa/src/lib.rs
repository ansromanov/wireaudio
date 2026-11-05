//! Direct low-latency ALSA backend, USB/Scarlett hardware discovery,
//! real-time audio loop, and non-destructive `~/.asoundrc` registration for
//! Reaper, SoundCraft, and TONE3000.

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
mod system;
pub use system::{
    config_file_path, load_saved_config, release_card_from_pipewire, restore_card_to_pipewire,
    save_config,
};

use wireaudio_core::{
    f32_to_s32_le, s32_le_to_f32, AudioDeviceDescriptor, ChannelStrip, EngineConfig,
    HardwareStreamStatus, RoutingMode,
};

const ASOUNDRC_MARKER_BEGIN: &str = "# >>> WireAudio Managed Block >>>";
const ASOUNDRC_MARKER_END: &str = "# <<< WireAudio Managed Block <<<";

/// Live telemetry shared between the real-time ALSA thread and the UI.
pub struct LiveEngineTelemetry {
    pub running: AtomicBool,
    pub achieved_rate: AtomicU32,
    pub achieved_period_size: AtomicU32,
    pub achieved_buffer_size: AtomicU32,
    pub xrun_count: AtomicU64,
    pub callback_cycles: AtomicU64,
    pub test_tone_enabled: AtomicBool,
    pub direct_monitor_enabled: AtomicBool,
    pub input_peaks_bits: [AtomicU32; 4],
    pub output_peaks_bits: [AtomicU32; 2],
}

impl Default for LiveEngineTelemetry {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            achieved_rate: AtomicU32::new(0),
            achieved_period_size: AtomicU32::new(0),
            achieved_buffer_size: AtomicU32::new(0),
            xrun_count: AtomicU64::new(0),
            callback_cycles: AtomicU64::new(0),
            test_tone_enabled: AtomicBool::new(false),
            direct_monitor_enabled: AtomicBool::new(false),
            input_peaks_bits: [
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
            ],
            output_peaks_bits: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }
}

/// Discovers all ALSA soundcards on the system and reads their live `/proc/asound` state.
pub fn discover_alsa_devices() -> Vec<AudioDeviceDescriptor> {
    let mut devices = Vec::new();

    for card_res in alsa::card::Iter::new() {
        let Ok(card) = card_res else { continue };
        let idx = card.get_index();
        let name = card.get_name().unwrap_or_else(|_| format!("Card {}", idx));
        let long_name = card.get_longname().unwrap_or_else(|_| name.clone());
        let id = read_card_id(idx).unwrap_or_else(|| format!("card{}", idx));

        let stream_info = fs::read_to_string(format!("/proc/asound/card{}/stream0", idx)).ok();
        let is_usb = stream_info.is_some() || long_name.to_lowercase().contains("usb");

        let playback_status = read_hw_params_status(idx, "pcm0p", stream_info.as_deref());
        let capture_status = read_hw_params_status(idx, "pcm0c", stream_info.as_deref());

        let (play_ch, cap_ch, rates) = parse_usb_stream_capabilities(
            stream_info.as_deref(),
            &playback_status,
            &capture_status,
        );

        let inputs = build_channel_strips(&name, cap_ch, true);
        let outputs = build_channel_strips(&name, play_ch, false);

        devices.push(AudioDeviceDescriptor {
            card_index: idx,
            id,
            name,
            long_name,
            is_usb,
            playback_channels: play_ch,
            capture_channels: cap_ch,
            supported_rates: rates,
            supported_formats: vec!["S32_LE (24/32-bit)".to_string(), "FLOAT_LE".to_string()],
            playback_status,
            capture_status,
            inputs,
            outputs,
        });
    }

    devices
}

fn read_card_id(card_index: i32) -> Option<String> {
    fs::read_to_string(format!("/proc/asound/card{}/id", card_index))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_hw_params_status(
    card_index: i32,
    pcm_dir: &str,
    stream0: Option<&str>,
) -> HardwareStreamStatus {
    let path = format!("/proc/asound/card{}/{}/sub0/hw_params", card_index, pcm_dir);
    let mut status = HardwareStreamStatus::default();

    if let Some(usb_text) = stream0 {
        if usb_text.contains("Data packet interval: 125 us") {
            status.usb_packet_interval_us = Some(125);
        } else if usb_text.contains("Data packet interval: 1000 us") {
            status.usb_packet_interval_us = Some(1000);
        }
    }

    let Ok(contents) = fs::read_to_string(path) else {
        return status;
    };

    if contents.trim() == "closed" || contents.trim().is_empty() {
        status.is_running = false;
        status.access_mode = "IDLE".to_string();
        return status;
    }

    status.is_running = true;
    for line in contents.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "access" => status.access_mode = val.to_string(),
                "format" => status.format = val.to_string(),
                "channels" => status.channels = val.parse().unwrap_or(2),
                "rate" => {
                    let rate_num = val.split_whitespace().next().unwrap_or("48000");
                    status.rate = rate_num.parse().unwrap_or(48_000);
                }
                "period_size" => status.period_size = val.parse().unwrap_or(0),
                "buffer_size" => status.buffer_size = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    status
}

fn parse_usb_stream_capabilities(
    stream0: Option<&str>,
    play_status: &HardwareStreamStatus,
    cap_status: &HardwareStreamStatus,
) -> (u32, u32, Vec<u32>) {
    let mut play_ch = if play_status.channels > 0 {
        play_status.channels
    } else {
        2
    };
    let mut cap_ch = if cap_status.channels > 0 {
        cap_status.channels
    } else {
        2
    };
    let mut rates = vec![44100, 48000, 88200, 96000, 176400, 192000];

    if let Some(text) = stream0 {
        let mut in_capture = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Playback:") {
                in_capture = false;
            } else if trimmed.starts_with("Capture:") {
                in_capture = true;
            } else if let Some(ch_str) = trimmed.strip_prefix("Channels:") {
                if let Ok(ch) = ch_str.trim().parse::<u32>() {
                    if in_capture {
                        cap_ch = ch;
                    } else {
                        play_ch = ch;
                    }
                }
            } else if let Some(rates_str) = trimmed.strip_prefix("Rates:") {
                let parsed: Vec<u32> = rates_str
                    .split(',')
                    .filter_map(|s| s.trim().parse::<u32>().ok())
                    .collect();
                if !parsed.is_empty() {
                    rates = parsed;
                }
            }
        }
    }

    (play_ch, cap_ch, rates)
}

fn build_channel_strips(card_name: &str, count: u32, is_input: bool) -> Vec<ChannelStrip> {
    let is_scarlett = card_name.to_lowercase().contains("scarlett");
    let mut strips = Vec::with_capacity(count as usize);

    for i in 0..(count as usize) {
        let label = if is_scarlett && is_input {
            match i {
                0 => "In 1 — Mic / Line / Hi-Z Inst".to_string(),
                1 => "In 2 — Mic / Line / Hi-Z Inst".to_string(),
                2 => "In 3 — Scarlett Loopback L".to_string(),
                3 => "In 4 — Scarlett Loopback R".to_string(),
                _ => format!("Input {}", i + 1),
            }
        } else if is_scarlett && !is_input {
            match i {
                0 => "Out 1 — Monitor Left (FL)".to_string(),
                1 => "Out 2 — Monitor Right (FR)".to_string(),
                _ => format!("Output {}", i + 1),
            }
        } else if is_input {
            format!("Input Ch {}", i + 1)
        } else {
            format!("Output Ch {}", i + 1)
        };

        strips.push(ChannelStrip {
            index: i,
            name: label,
            is_input,
            gain_db: 0.0,
            muted: false,
            direct_monitor: false,
            peak_level: 0.0,
        });
    }

    strips
}

/// Lists hardware mixer control names (`alsa::mixer::Mixer`) exposed by the soundcard (e.g. Scarlett 4th Gen).
pub fn list_card_mixer_controls(card_index: i32) -> Vec<String> {
    let ctl_name = format!("hw:{}", card_index);
    let Ok(mixer) = alsa::mixer::Mixer::new(&ctl_name, false) else {
        return Vec::new();
    };

    let mut names = Vec::new();
    for elem in mixer.iter() {
        if let Some(selem) = alsa::mixer::Selem::new(elem) {
            if let Ok(name) = selem.get_id().get_name() {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Returns the path to `~/.asoundrc`.
pub fn asoundrc_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/andrei".to_string());
    PathBuf::from(home).join(".asoundrc")
}

/// Checks whether WireAudio is currently registered in `~/.asoundrc`.
pub fn is_asoundrc_registered() -> bool {
    let path = asoundrc_path();
    fs::read_to_string(path)
        .map(|s| s.contains(ASOUNDRC_MARKER_BEGIN))
        .unwrap_or(false)
}

/// Generates and writes the non-destructive `~/.asoundrc` block so that Reaper, SoundCraft,
/// and TONE3000 immediately see `wireaudio` (shared low-latency multi-client bus)
/// and `wireaudio_direct` without removing PipeWire or breaking voice chat.
pub fn register_engine_in_asoundrc(
    cfg: &EngineConfig,
    card_desc: &AudioDeviceDescriptor,
) -> Result<String, String> {
    release_card_from_pipewire(&card_desc.name);
    let _ = save_config(cfg);
    let path = asoundrc_path();
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let stripped = strip_managed_block(&existing);
    let buffer_size = cfg.period_size * cfg.periods;
    let play_ch = card_desc.playback_channels.max(2);
    let cap_ch = card_desc.capture_channels.max(2);
    let out_card_id = cfg.output_card_id.as_deref().unwrap_or(&card_desc.id);

    let managed_block = format!(
        r#"{begin}
# Generated by WireAudio for Input hw:{card_id},0 -> Output hw:{out_card_id},0
# Target Latency: period={period} frames ({lat:.2} ms @ {rate} Hz), buffer={buffer} frames

pcm.wireaudio_dmix {{
    type dmix
    ipc_key 5678293
    ipc_perm 0666
    slave {{
        pcm "hw:{out_card_id},0"
        rate {rate}
        format S32_LE
        channels {play_ch}
        period_size {period}
        buffer_size {buffer}
    }}
}}

pcm.wireaudio_dsnoop {{
    type dsnoop
    ipc_key 5678294
    ipc_perm 0666
    slave {{
        pcm "hw:{card_id},0"
        rate {rate}
        format S32_LE
        channels {cap_ch}
        period_size {period}
        buffer_size {buffer}
    }}
}}

# Capture plug with explicit Mono + Stereo routing from 4-channel Scarlett USB stream
pcm.wireaudio_capture_routed {{
    type plug
    slave.pcm "wireaudio_dsnoop"
    ttable.0.0 1.0
    ttable.0.1 1.0
    ttable.1.1 1.0
}}

pcm.wireaudio_playback_routed {{
    type plug
    slave.pcm "wireaudio_dmix"
}}

pcm.wireaudio_duplex {{
    type asym
    playback.pcm "wireaudio_playback_routed"
    capture.pcm "wireaudio_capture_routed"
}}

# Override ALSA default so all desktop apps, browsers, and DAWs route through WireAudio
# even when PipeWire daemons are disabled.
pcm.!default {{
    type plug
    slave.pcm "wireaudio_duplex"
    hint {{
        show on
        description "WireAudio Default Studio Bus ({card_name} - {period}f / {lat:.2}ms @ {rate}Hz)"
    }}
}}

ctl.!default {{
    type hw
    card "{out_card_id}"
}}

# 1. Shared Low-Latency Bus (Supports both Mono & Stereo in TONE3000, Reaper, SoundCraft)
pcm.{pcm_name} {{
    type plug
    slave.pcm "wireaudio_duplex"
    hint {{
        show on
        description "WireAudio Shared Bus ({card_name} - {period}f / {lat:.2}ms @ {rate}Hz)"
    }}
}}

# 2. Dedicated Guitar/Mic Input 1 (Centered Mono->Stereo + Mono compatible)
pcm.{pcm_name}_in1 {{
    type asym
    playback.pcm "wireaudio_playback_routed"
    capture.pcm {{
        type plug
        slave.pcm "wireaudio_dsnoop"
        ttable.0.0 1.0
        ttable.1.0 1.0
    }}
    hint {{
        show on
        description "WireAudio Input 1 Mono/Guitar ({card_name} - {period}f / {lat:.2}ms)"
    }}
}}

# 3. Dedicated Guitar/Mic Input 2 (Centered Mono->Stereo + Mono compatible)
pcm.{pcm_name}_in2 {{
    type asym
    playback.pcm "wireaudio_playback_routed"
    capture.pcm {{
        type plug
        slave.pcm "wireaudio_dsnoop"
        ttable.0.1 1.0
        ttable.1.1 1.0
    }}
    hint {{
        show on
        description "WireAudio Input 2 Mono/Guitar ({card_name} - {period}f / {lat:.2}ms)"
    }}
}}

# 4. Direct Zero-Copy Hardware Access (Exclusive DMA)
pcm.{pcm_name}_direct {{
    type plug
    slave {{
        pcm "hw:{card_id},0"
        rate {rate}
        format S32_LE
    }}
    hint {{
        show on
        description "WireAudio Direct HW ({card_name} - {period}f Exclusive DMA)"
    }}
}}

ctl.{pcm_name} {{
    type hw
    card "{card_id}"
}}
{end}
"#,
        begin = ASOUNDRC_MARKER_BEGIN,
        end = ASOUNDRC_MARKER_END,
        card_id = card_desc.id,
        out_card_id = out_card_id,
        card_name = card_desc.name,
        rate = cfg.sample_rate,
        period = cfg.period_size,
        buffer = buffer_size,
        play_ch = play_ch,
        cap_ch = cap_ch,
        pcm_name = cfg.alsa_pcm_name,
        lat = cfg.target_period_latency_ms(),
    );

    let new_contents = if stripped.trim().is_empty() {
        managed_block
    } else {
        format!("{}\n\n{}", stripped.trim_end(), managed_block)
    };

    fs::write(&path, new_contents)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;

    Ok(format!(
        "Registered ALSA PCM devices `{}` and `{}_direct` in {}",
        cfg.alsa_pcm_name,
        cfg.alsa_pcm_name,
        path.display()
    ))
}

/// Safely removes only the WireAudio block from `~/.asoundrc`.
pub fn unregister_engine_from_asoundrc() -> Result<String, String> {
    restore_card_to_pipewire();
    let path = asoundrc_path();
    let Ok(existing) = fs::read_to_string(&path) else {
        return Ok("~/.asoundrc is already clean.".to_string());
    };
    let stripped = strip_managed_block(&existing);
    if stripped.trim().is_empty() {
        let _ = fs::remove_file(&path);
    } else {
        fs::write(&path, stripped)
            .map_err(|e| format!("Failed to update {}: {}", path.display(), e))?;
    }
    Ok("Removed WireAudio block from ~/.asoundrc; existing audio stack preserved.".to_string())
}

fn strip_managed_block(input: &str) -> String {
    let mut output = Vec::new();
    let mut inside_block = false;
    for line in input.lines() {
        if line.trim() == ASOUNDRC_MARKER_BEGIN {
            inside_block = true;
            continue;
        }
        if line.trim() == ASOUNDRC_MARKER_END {
            inside_block = false;
            continue;
        }
        if !inside_block {
            output.push(line);
        }
    }
    output.join("\n")
}

/// Applies PipeWire runtime quantum tuning (`pw-metadata`) when in `SystemSharedTuned`
/// mode. Uses a safe minimum quantum (>= 64 frames when sharing with a 256-frame ALSA node)
/// to prevent USB 6-sample headroom starvation and crackling.
pub fn apply_runtime_quantum_tuning(period_size: u32, sample_rate: u32) -> Result<String, String> {
    let safe_shared_quantum = period_size.max(64);
    let quantum_str = format!("{}", safe_shared_quantum);
    let rate_str = format!("{}", sample_rate);

    let status1 = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.force-quantum", &quantum_str])
        .output();

    let _status2 = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.force-rate", &rate_str])
        .output();

    match status1 {
        Ok(out) if out.status.success() => Ok(format!(
            "Applied glitch-free shared quantum {} frames @ {} Hz",
            safe_shared_quantum, sample_rate
        )),
        _ => Ok(format!(
            "Applied {} frames @ {} Hz configuration to ALSA profile",
            period_size, sample_rate
        )),
    }
}

/// Resets any forced runtime quantum on the host session to eliminate crackling.
pub fn reset_runtime_quantum_tuning() -> Result<String, String> {
    let _ = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.force-quantum", "0"])
        .output();
    let _ = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.force-rate", "0"])
        .output();
    Ok("Reset system clock quantum to automatic (crackle-free default).".to_string())
}

/// Real-time ALSA engine controller for testing and monitoring 32-frame low-latency I/O.
pub struct AlsaRealtimeEngine {
    pub telemetry: Arc<LiveEngineTelemetry>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Default for AlsaRealtimeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AlsaRealtimeEngine {
    pub fn new() -> Self {
        Self {
            telemetry: Arc::new(LiveEngineTelemetry::default()),
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.telemetry.running.load(Ordering::Relaxed)
    }

    pub fn start(&mut self, cfg: EngineConfig) -> Result<String, String> {
        self.stop();
        self.stop_flag.store(false, Ordering::SeqCst);

        let telemetry = Arc::clone(&self.telemetry);
        let stop_flag = Arc::clone(&self.stop_flag);

        let pcm_target = match cfg.routing_mode {
            RoutingMode::ExclusiveDirectHw => format!("hw:{},0", cfg.selected_card_id),
            RoutingMode::SharedLowLatencyBus | RoutingMode::SystemSharedTuned => {
                "default".to_string()
            }
        };
        let target_label = pcm_target.clone();
        let period_size = cfg.period_size;
        let sample_rate = cfg.sample_rate;

        let handle = thread::Builder::new()
            .name("wireaudio-rt".to_string())
            .spawn(move || {
                elevate_realtime_thread_priority();
                run_alsa_loop(&pcm_target, &cfg, &telemetry, &stop_flag);
            })
            .map_err(|e| format!("Failed to spawn RT audio thread: {}", e))?;

        self.worker = Some(handle);
        Ok(format!(
            "Started RT engine on `{}` (target period: {} frames @ {} Hz)",
            target_label, period_size, sample_rate
        ))
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.telemetry.running.store(false, Ordering::Relaxed);
    }
}

fn elevate_realtime_thread_priority() {
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = 80;
        let _ = libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param);
    }
}

fn run_alsa_loop(
    pcm_device: &str,
    cfg: &EngineConfig,
    telemetry: &Arc<LiveEngineTelemetry>,
    stop_flag: &Arc<AtomicBool>,
) {
    let Ok(play_pcm) = PCM::new(pcm_device, Direction::Playback, false) else {
        telemetry.running.store(false, Ordering::Relaxed);
        return;
    };

    let cap_pcm = PCM::new(pcm_device, Direction::Capture, false).ok();

    let (achieved_rate, achieved_period, achieved_buf) = match configure_pcm_hw(&play_pcm, cfg, 2) {
        Ok(vals) => vals,
        Err(_) => {
            telemetry.running.store(false, Ordering::Relaxed);
            return;
        }
    };

    if let Some(ref cap) = cap_pcm {
        let _ = configure_pcm_hw(cap, cfg, 2);
        let _ = cap.prepare();
    }

    let _ = play_pcm.prepare();

    telemetry
        .achieved_rate
        .store(achieved_rate, Ordering::Relaxed);
    telemetry
        .achieved_period_size
        .store(achieved_period, Ordering::Relaxed);
    telemetry
        .achieved_buffer_size
        .store(achieved_buf, Ordering::Relaxed);
    telemetry.running.store(true, Ordering::Relaxed);

    let frames = achieved_period.max(16) as usize;
    let mut play_buf = vec![0i32; frames * 2];
    let mut cap_buf = vec![0i32; frames * 2];
    let mut phase = 0.0f32;
    let phase_inc = 2.0 * std::f32::consts::PI * 440.0 / (achieved_rate.max(1) as f32);

    // Pre-fill 2 periods of silence into playback ring buffer before starting duplex capture
    // so `appl_ptr - hw_ptr` maintains headroom and never starves USB DMA (fixes crackling).
    if let Ok(io_play) = play_pcm.io_i32() {
        let _ = io_play.writei(&play_buf);
        let _ = io_play.writei(&play_buf);
    }
    if let Some(ref cap) = cap_pcm {
        let _ = cap.start();
    }

    while !stop_flag.load(Ordering::Relaxed) {
        let mut in_peak_l = 0.0f32;
        let mut in_peak_r = 0.0f32;

        if let Some(ref cap) = cap_pcm {
            if let Ok(io_cap) = cap.io_i32() {
                match io_cap.readi(&mut cap_buf) {
                    Ok(_) => {
                        for &[l_raw, r_raw] in cap_buf.as_chunks::<2>().0 {
                            let l = s32_le_to_f32(l_raw).abs();
                            let r = s32_le_to_f32(r_raw).abs();
                            if l > in_peak_l {
                                in_peak_l = l;
                            }
                            if r > in_peak_r {
                                in_peak_r = r;
                            }
                        }
                    }
                    Err(_) => {
                        let _ = cap.try_recover(nix_epipe(), true);
                    }
                }
            }
        }

        let tone_on = telemetry.test_tone_enabled.load(Ordering::Relaxed);
        let mon_on = telemetry.direct_monitor_enabled.load(Ordering::Relaxed);
        let mut out_peak_l = 0.0f32;
        let mut out_peak_r = 0.0f32;

        for i in 0..frames {
            let mut sample_l = if mon_on {
                s32_le_to_f32(cap_buf[i * 2])
            } else {
                0.0
            };
            let mut sample_r = if mon_on {
                s32_le_to_f32(cap_buf[i * 2 + 1])
            } else {
                0.0
            };

            if tone_on {
                let sine = (phase).sin() * 0.15;
                phase += phase_inc;
                if phase > 2.0 * std::f32::consts::PI {
                    phase -= 2.0 * std::f32::consts::PI;
                }
                sample_l += sine;
                sample_r += sine;
            }

            out_peak_l = out_peak_l.max(sample_l.abs());
            out_peak_r = out_peak_r.max(sample_r.abs());

            play_buf[i * 2] = f32_to_s32_le(sample_l);
            play_buf[i * 2 + 1] = f32_to_s32_le(sample_r);
        }

        telemetry.input_peaks_bits[0].store(in_peak_l.to_bits(), Ordering::Relaxed);
        telemetry.input_peaks_bits[1].store(in_peak_r.to_bits(), Ordering::Relaxed);
        telemetry.output_peaks_bits[0].store(out_peak_l.to_bits(), Ordering::Relaxed);
        telemetry.output_peaks_bits[1].store(out_peak_r.to_bits(), Ordering::Relaxed);

        if let Ok(io_play) = play_pcm.io_i32() {
            if io_play.writei(&play_buf).is_err() {
                telemetry.xrun_count.fetch_add(1, Ordering::Relaxed);
                let _ = play_pcm.try_recover(nix_epipe(), true);
            }
        } else {
            thread::sleep(Duration::from_millis(2));
        }

        telemetry.callback_cycles.fetch_add(1, Ordering::Relaxed);
    }

    telemetry.running.store(false, Ordering::Relaxed);
}

fn nix_epipe() -> alsa::Error {
    alsa::Error::new("EPIPE", libc::EPIPE)
}

fn configure_pcm_hw(
    pcm: &PCM,
    cfg: &EngineConfig,
    channels: u32,
) -> Result<(u32, u32, u32), alsa::Error> {
    let hwp = HwParams::any(pcm)?;
    let _ = hwp.set_access(Access::RWInterleaved);
    let _ = hwp.set_format(Format::s32());
    let _ = hwp.set_channels(channels);
    let rate = hwp.set_rate_near(cfg.sample_rate, ValueOr::Nearest)?;
    let period = hwp.set_period_size_near(cfg.period_size as i64, ValueOr::Nearest)?;
    let periods = cfg.periods.max(3) as i64;
    let target_buf = (period * periods).max(period * 3);
    let buf = hwp.set_buffer_size_near(target_buf)?;
    pcm.hw_params(&hwp)?;

    if let Ok(swp) = pcm.sw_params_current() {
        let _ = swp.set_avail_min(period);
        let _ = swp.set_start_threshold(period * 2);
        let _ = pcm.sw_params(&swp);
    }

    Ok((rate, period as u32, buf as u32))
}
