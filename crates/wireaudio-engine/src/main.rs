//! CLI and daemon runner for `wireaudio-engine`.

use std::env;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use wireaudio_engine::{
    apply_runtime_quantum_tuning, asoundrc_path, config_file_path, discover_alsa_devices,
    is_asoundrc_registered, load_saved_config, register_engine_in_asoundrc,
    reset_runtime_quantum_tuning, unregister_engine_from_asoundrc, AlsaRealtimeEngine,
    EngineConfig,
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("status");

    match cmd {
        "status" => cmd_status(),
        "register" => cmd_register(&args[1..]),
        "unregister" => cmd_unregister(),
        "reset-quantum" => {
            if let Ok(msg) = reset_runtime_quantum_tuning() {
                println!("{}", msg);
            }
        }
        "daemon" => cmd_daemon(),
        "run" => cmd_run(&args[1..]),
        _ => {
            eprintln!("Usage: wireaudio-engine <status|register|unregister|daemon|reset-quantum|run> [--period 128] [--rate 48000] [--card Gen] [--out-card Generic]");
            std::process::exit(1);
        }
    }
}

fn parse_config_flags(args: &[String]) -> EngineConfig {
    let mut cfg = load_saved_config();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--period" if i + 1 < args.len() => {
                if let Ok(p) = args[i + 1].parse::<u32>() {
                    cfg.period_size = p;
                }
                i += 2;
            }
            "--rate" if i + 1 < args.len() => {
                if let Ok(r) = args[i + 1].parse::<u32>() {
                    cfg.sample_rate = r;
                }
                i += 2;
            }
            "--card" if i + 1 < args.len() => {
                cfg.selected_card_id = args[i + 1].clone();
                i += 2;
            }
            "--out-card" if i + 1 < args.len() => {
                cfg.output_card_id = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }
    cfg
}

fn cmd_status() {
    let devices = discover_alsa_devices();
    println!("=== WireAudio ALSA Hardware Status ===");
    println!(
        "Managed ~/.asoundrc ({}): {}",
        asoundrc_path().display(),
        if is_asoundrc_registered() {
            "REGISTERED (pcm.wireaudio & pcm.wireaudio_direct active)"
        } else {
            "NOT REGISTERED"
        }
    );
    println!();

    for dev in &devices {
        println!(
            "[Card {} | hw:{}] {} ({} in / {} out)",
            dev.card_index, dev.id, dev.long_name, dev.capture_channels, dev.playback_channels
        );
        if dev.playback_status.is_running {
            println!(
                "  Playback: RUNNING | {} | {} Hz | period={} frames ({:.2} ms) | buffer={} frames ({:.2} ms)",
                dev.playback_status.format,
                dev.playback_status.rate,
                dev.playback_status.period_size,
                dev.current_period_latency_ms(),
                dev.playback_status.buffer_size,
                dev.current_buffer_latency_ms()
            );
        } else {
            println!("  Playback: IDLE");
        }
    }
}

fn cmd_register(args: &[String]) {
    let cfg = parse_config_flags(args);
    let devices = discover_alsa_devices();
    let Some(card) = devices
        .iter()
        .find(|d| d.id == cfg.selected_card_id)
        .or_else(|| devices.first())
    else {
        eprintln!("No ALSA soundcards found.");
        std::process::exit(1);
    };

    if let Ok(msg) = apply_runtime_quantum_tuning(cfg.period_size, cfg.sample_rate) {
        println!("{}", msg);
    }

    match register_engine_in_asoundrc(&cfg, card) {
        Ok(msg) => println!("{}", msg),
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    }
}

fn cmd_unregister() {
    let _ = reset_runtime_quantum_tuning();
    match unregister_engine_from_asoundrc() {
        Ok(msg) => println!("{}", msg),
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    }
}

fn cmd_daemon() {
    println!(
        "Starting WireAudio ALSA Daemon (config: {})...",
        config_file_path().display()
    );

    let mut last_card_present = false;
    loop {
        let cfg = load_saved_config();
        let devices = discover_alsa_devices();
        if let Some(card) = devices
            .iter()
            .find(|d| d.id == cfg.selected_card_id)
            .or_else(|| devices.first())
        {
            // Register only on card (re)appearance so a manual `unregister` sticks.
            if !last_card_present {
                match register_engine_in_asoundrc(&cfg, card) {
                    Ok(msg) => println!("[WireAudio Daemon] {}", msg),
                    Err(err) => eprintln!("[WireAudio Daemon] Error: {}", err),
                }
                last_card_present = true;
            }
        } else {
            last_card_present = false;
        }
        thread::sleep(Duration::from_secs(3));
    }
}

fn cmd_run(args: &[String]) {
    let cfg = parse_config_flags(args);
    let mut engine = AlsaRealtimeEngine::new();
    match engine.start(cfg.clone()) {
        Ok(msg) => println!("{}", msg),
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    }

    println!("WireAudio RT engine running (Ctrl+C to stop)...");
    loop {
        thread::sleep(Duration::from_secs(1));
        if !engine.is_running() {
            eprintln!("RT engine loop exited.");
            break;
        }
        let rate = engine.telemetry.achieved_rate.load(Ordering::Relaxed);
        let period = engine
            .telemetry
            .achieved_period_size
            .load(Ordering::Relaxed);
        let xruns = engine.telemetry.xrun_count.load(Ordering::Relaxed);
        let cycles = engine.telemetry.callback_cycles.load(Ordering::Relaxed);
        println!(
            "  [Telemetry] rate={} Hz | period={} frames | cycles={} | xruns={}",
            rate, period, cycles, xruns
        );
    }
}
