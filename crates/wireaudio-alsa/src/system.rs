//! Persistent engine config and PipeWire card hand-off helpers.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use wireaudio_core::EngineConfig;

/// Returns the path to `~/.config/wireaudio/config.json`.
pub fn config_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/andrei".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("wireaudio")
        .join("config.json")
}

/// Loads the persistent `EngineConfig` from `~/.config/wireaudio/config.json`,
/// falling back to `EngineConfig::default()`.
pub fn load_saved_config() -> EngineConfig {
    let path = config_file_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<EngineConfig>(&s).ok())
        .unwrap_or_default()
}

/// Saves the `EngineConfig` to `~/.config/wireaudio/config.json`.
pub fn save_config(cfg: &EngineConfig) -> Result<(), String> {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config dir {}: {}", parent.display(), e))?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

/// Automatically releases the target ALSA soundcard from PipeWire/WirePlumber (`wpctl set-profile <id> off`)
/// so that ALSA `dmix`/`dsnoop` (`pcm.wireaudio`) and `pcm.wireaudio_direct` never hit `EBUSY`.
pub fn release_card_from_pipewire(card_name: &str) {
    if let Some(dev_id) = find_wpctl_device_id(card_name) {
        let _ = std::process::Command::new("wpctl")
            .args(["set-profile", &dev_id.to_string(), "off"])
            .output();
        thread::sleep(Duration::from_millis(120));
    }
}

/// Restores the soundcard back to PipeWire (`pro-audio` profile) when unregistering WireAudio.
pub fn restore_card_to_pipewire() {
    if let Some(dev_id) = find_wpctl_device_id("Scarlett") {
        let _ = std::process::Command::new("wpctl")
            .args(["set-profile", &dev_id.to_string(), "pro-audio"])
            .output();
    }
}

fn find_wpctl_device_id(card_name: &str) -> Option<u32> {
    let out = std::process::Command::new("wpctl")
        .arg("status")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = card_name.to_lowercase();
    let mut in_devices = false;
    for line in text.lines() {
        if line.contains("├─ Devices:") {
            in_devices = true;
            continue;
        }
        if in_devices && (line.contains("├─ Sinks:") || line.contains("└─")) {
            break;
        }
        if in_devices && line.to_lowercase().contains(&needle) {
            for token in line.split_whitespace() {
                if let Some(num_str) = token.strip_suffix('.') {
                    if let Ok(id) = num_str.parse::<u32>() {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}
