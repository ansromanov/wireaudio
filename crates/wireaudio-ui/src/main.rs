//! WireAudio Desktop UI — macOS "Audio MIDI Setup" style studio control center
//! for Linux ALSA.

use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use wireaudio_alsa::{
    apply_runtime_quantum_tuning, asoundrc_path, discover_alsa_devices, is_asoundrc_registered,
    list_card_mixer_controls, load_saved_config, register_engine_in_asoundrc,
    unregister_engine_from_asoundrc, AlsaRealtimeEngine,
};
use wireaudio_core::{
    AudioDeviceDescriptor, EngineConfig, RoutingMode, SUPPORTED_BUFFER_SIZES,
    SUPPORTED_SAMPLE_RATES,
};

const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 220, 160);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 180, 80);

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title.to_uppercase()).small().weak());
    ui.separator();
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WireAudio")
            .with_inner_size([1080.0, 680.0])
            .with_min_inner_size([860.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "WireAudio",
        options,
        Box::new(|cc| Ok(Box::new(WireAudioApp::new(cc)))),
    )
}

struct WireAudioApp {
    devices: Vec<AudioDeviceDescriptor>,
    selected_idx: usize,
    config: EngineConfig,
    engine: AlsaRealtimeEngine,
    mixer_controls: Vec<String>,
    asoundrc_registered: bool,
    status_banner: String,
    last_refresh: Instant,
}

impl WireAudioApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 4.0);
        });
        let devices = discover_alsa_devices();
        let mut config = load_saved_config();
        let selected_idx = devices
            .iter()
            .position(|d| d.id == config.selected_card_id)
            .or_else(|| {
                devices
                    .iter()
                    .position(|d| d.name.to_lowercase().contains("scarlett"))
            })
            .unwrap_or(0);

        if let Some(dev) = devices.get(selected_idx) {
            config.selected_card_index = dev.card_index;
            config.selected_card_id = dev.id.clone();
        }

        let mixer_controls = devices
            .get(selected_idx)
            .map(|d| list_card_mixer_controls(d.card_index))
            .unwrap_or_default();

        let asoundrc_registered = is_asoundrc_registered();

        Self {
            devices,
            selected_idx,
            config,
            engine: AlsaRealtimeEngine::new(),
            mixer_controls,
            asoundrc_registered,
            status_banner: "Ready.".to_string(),
            last_refresh: Instant::now(),
        }
    }

    fn refresh_devices(&mut self) {
        self.devices = discover_alsa_devices();
        if self.selected_idx >= self.devices.len() && !self.devices.is_empty() {
            self.selected_idx = 0;
        }
        if let Some(dev) = self.devices.get(self.selected_idx) {
            self.config.selected_card_index = dev.card_index;
            self.config.selected_card_id = dev.id.clone();
            self.mixer_controls = list_card_mixer_controls(dev.card_index);
        }
        self.asoundrc_registered = is_asoundrc_registered();
        self.last_refresh = Instant::now();
    }
}

impl eframe::App for WireAudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() >= Duration::from_secs(2) {
            self.refresh_devices();
        }
        ui.ctx().request_repaint_after(Duration::from_millis(50));

        egui::Panel::top("top_header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("WireAudio").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⟳ Refresh").clicked() {
                        self.refresh_devices();
                    }
                    let badge = if self.asoundrc_registered {
                        egui::RichText::new("● ALSA registered").color(OK_COLOR)
                    } else {
                        egui::RichText::new("○ ALSA not registered").color(WARN_COLOR)
                    };
                    ui.label(badge);
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("bottom_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status_banner).monospace());
            });
        });

        egui::Panel::left("device_sidebar")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("DEVICES").small().weak());
                ui.add_space(2.0);

                let mut new_selection = self.selected_idx;
                for (idx, dev) in self.devices.iter().enumerate() {
                    let selected = idx == self.selected_idx;
                    let bus = if dev.is_usb { "USB" } else { "PCI" };
                    let state = if dev.playback_status.is_running {
                        format!(
                            "{} Hz · {}f",
                            dev.playback_status.rate, dev.playback_status.period_size
                        )
                    } else {
                        "idle".to_string()
                    };
                    let mut job = egui::text::LayoutJob::default();
                    let style = ui.style();
                    egui::RichText::new(format!("{}\n", dev.name))
                        .strong()
                        .append_to(
                            &mut job,
                            style,
                            egui::FontSelection::Default,
                            egui::Align::LEFT,
                        );
                    egui::RichText::new(format!(
                        "{} · {} in / {} out · {}",
                        bus, dev.capture_channels, dev.playback_channels, state
                    ))
                    .small()
                    .weak()
                    .append_to(
                        &mut job,
                        style,
                        egui::FontSelection::Default,
                        egui::Align::LEFT,
                    );
                    let resp = ui.add_sized(
                        [ui.available_width(), 0.0],
                        egui::Button::selectable(selected, job),
                    );
                    if resp.clicked() {
                        new_selection = idx;
                    }
                }

                if new_selection != self.selected_idx {
                    self.selected_idx = new_selection;
                    if let Some(dev) = self.devices.get(self.selected_idx) {
                        self.config.selected_card_index = dev.card_index;
                        self.config.selected_card_id = dev.id.clone();
                        self.mixer_controls = list_card_mixer_controls(dev.card_index);
                    }
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(dev) = self.devices.get(self.selected_idx).cloned() else {
                ui.label("No ALSA soundcards detected.");
                return;
            };

            ui.add_space(4.0);
            ui.heading(&dev.long_name);
            let mut info = format!("hw:{},0 · card {}", dev.id, dev.card_index);
            if let Some(us) = dev.playback_status.usb_packet_interval_us {
                info += &format!(" · USB {} µs", us);
            }
            if dev.playback_status.is_running {
                info += &format!(
                    " · active {}f ({:.2} ms)",
                    dev.playback_status.period_size,
                    dev.current_period_latency_ms()
                );
            }
            ui.label(egui::RichText::new(info).weak());
            ui.add_space(8.0);

            section(ui, "Configuration");
            egui::Grid::new("clock_buffer_grid")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Routing");
                    egui::ComboBox::from_id_salt("routing_mode_combo")
                        .selected_text(self.config.routing_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in [
                                RoutingMode::SharedLowLatencyBus,
                                RoutingMode::SystemSharedTuned,
                                RoutingMode::ExclusiveDirectHw,
                            ] {
                                ui.selectable_value(
                                    &mut self.config.routing_mode,
                                    mode,
                                    mode.label(),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Output");
                    let current_out_label = self
                        .config
                        .output_card_id
                        .as_ref()
                        .and_then(|id| self.devices.iter().find(|d| &d.id == id))
                        .map(|d| format!("{} (hw:{})", d.name, d.id))
                        .unwrap_or_else(|| format!("Same as Input ({} / hw:{})", dev.name, dev.id));

                    egui::ComboBox::from_id_salt("output_device_combo")
                        .selected_text(current_out_label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    self.config.output_card_id.is_none(),
                                    format!("Same as Input ({} / hw:{})", dev.name, dev.id),
                                )
                                .clicked()
                            {
                                self.config.output_card_id = None;
                            }
                            for out_dev in &self.devices {
                                if out_dev.playback_channels > 0 {
                                    let is_sel = self.config.output_card_id.as_deref()
                                        == Some(out_dev.id.as_str());
                                    if ui
                                        .selectable_label(
                                            is_sel,
                                            format!("{} (hw:{})", out_dev.name, out_dev.id),
                                        )
                                        .clicked()
                                    {
                                        self.config.output_card_id = Some(out_dev.id.clone());
                                    }
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Sample rate");
                    ui.horizontal(|ui| {
                        for &rate in SUPPORTED_SAMPLE_RATES {
                            let supported = dev.supported_rates.contains(&rate);
                            ui.add_enabled_ui(supported, |ui| {
                                if ui
                                    .selectable_label(
                                        self.config.sample_rate == rate,
                                        format!("{}k", rate as f32 / 1000.0),
                                    )
                                    .clicked()
                                {
                                    self.config.sample_rate = rate;
                                }
                            });
                        }
                    });
                    ui.end_row();

                    ui.label("Buffer");
                    ui.horizontal(|ui| {
                        for &frames in SUPPORTED_BUFFER_SIZES {
                            if ui
                                .selectable_label(
                                    self.config.period_size == frames,
                                    frames.to_string(),
                                )
                                .clicked()
                            {
                                self.config.period_size = frames;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Latency");
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.2} ms one-way · {:.2} ms round-trip ({} periods)",
                            self.config.target_period_latency_ms(),
                            self.config.target_roundtrip_latency_ms(),
                            self.config.periods
                        ))
                        .color(OK_COLOR)
                        .strong(),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("Apply").strong()))
                    .on_hover_text("Write ~/.asoundrc profile for Reaper / SoundCraft / TONE3000")
                    .clicked()
                {
                    let _ = apply_runtime_quantum_tuning(
                        self.config.period_size,
                        self.config.sample_rate,
                    );
                    match register_engine_in_asoundrc(&self.config, &dev) {
                        Ok(msg) => {
                            self.status_banner = msg;
                            self.asoundrc_registered = true;
                        }
                        Err(err) => self.status_banner = err,
                    }
                    self.refresh_devices();
                }

                if self.asoundrc_registered && ui.button("Unregister").clicked() {
                    match unregister_engine_from_asoundrc() {
                        Ok(msg) => {
                            self.status_banner = msg;
                            self.asoundrc_registered = false;
                        }
                        Err(err) => self.status_banner = err,
                    }
                }
            });

            ui.add_space(12.0);
            section(ui, "Monitor");
            ui.horizontal(|ui| {
                let rt_running = self.engine.is_running();
                let btn_label = if rt_running { "■ Stop" } else { "▶ Start" };
                if ui.button(btn_label).clicked() {
                    if rt_running {
                        self.engine.stop();
                        self.status_banner = "Stopped RT monitor thread.".to_string();
                    } else {
                        match self.engine.start(self.config.clone()) {
                            Ok(msg) => self.status_banner = msg,
                            Err(err) => self.status_banner = err,
                        }
                    }
                }

                let mut tone = self
                    .engine
                    .telemetry
                    .test_tone_enabled
                    .load(Ordering::Relaxed);
                if ui.checkbox(&mut tone, "Test tone 440 Hz").changed() {
                    self.engine
                        .telemetry
                        .test_tone_enabled
                        .store(tone, Ordering::Relaxed);
                }

                let mut mon = self
                    .engine
                    .telemetry
                    .direct_monitor_enabled
                    .load(Ordering::Relaxed);
                if ui.checkbox(&mut mon, "Input monitor").changed() {
                    self.engine
                        .telemetry
                        .direct_monitor_enabled
                        .store(mon, Ordering::Relaxed);
                }

                if rt_running {
                    let xruns = self.engine.telemetry.xrun_count.load(Ordering::Relaxed);
                    let color = if xruns == 0 { OK_COLOR } else { WARN_COLOR };
                    ui.label(egui::RichText::new(format!("XRUNs: {}", xruns)).color(color));
                }
            });

            ui.add_space(8.0);
            ui.columns(2, |cols| {
                cols[0].label(egui::RichText::new("Inputs").weak());
                for ch in &dev.inputs {
                    let peak_bits = self
                        .engine
                        .telemetry
                        .input_peaks_bits
                        .get(ch.index)
                        .map(|a| a.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    let peak = f32::from_bits(peak_bits).clamp(0.0, 1.0);
                    cols[0].horizontal(|ui| {
                        ui.label(&ch.name);
                        ui.add(
                            egui::ProgressBar::new(peak)
                                .desired_width(120.0)
                                .text(format!("{:.0}%", peak * 100.0)),
                        );
                    });
                }

                cols[1].label(egui::RichText::new("Outputs").weak());
                for ch in &dev.outputs {
                    let peak_bits = self
                        .engine
                        .telemetry
                        .output_peaks_bits
                        .get(ch.index)
                        .map(|a| a.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    let peak = f32::from_bits(peak_bits).clamp(0.0, 1.0);
                    cols[1].horizontal(|ui| {
                        ui.label(&ch.name);
                        ui.add(
                            egui::ProgressBar::new(peak)
                                .desired_width(120.0)
                                .text(format!("{:.0}%", peak * 100.0)),
                        );
                    });
                }
            });

            ui.add_space(12.0);
            ui.collapsing(
                format!("Hardware controls ({})", self.mixer_controls.len()),
                |ui| {
                    ui.label(
                        egui::RichText::new(format!("Config: {}", asoundrc_path().display()))
                            .weak(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for ctl in &self.mixer_controls {
                            ui.code(ctl);
                        }
                    });
                },
            );
        });
    }
}
