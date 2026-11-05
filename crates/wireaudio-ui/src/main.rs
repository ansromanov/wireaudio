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

const BG: egui::Color32 = egui::Color32::from_rgb(0x14, 0x16, 0x1b);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1d, 0x23);
const CARD: egui::Color32 = egui::Color32::from_rgb(0x21, 0x25, 0x2d);
const CONTROL: egui::Color32 = egui::Color32::from_rgb(0x2b, 0x30, 0x3a);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x33, 0x3d);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe6, 0xe8, 0xec);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x91, 0x9e);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4c, 0x8d, 0xff);
const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(0x4c, 0xd3, 0x8a);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xb8, 0x4b);
const CLIP_COLOR: egui::Color32 = egui::Color32::from_rgb(0xef, 0x5b, 0x5b);

fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|s| {
        use egui::{FontId, TextStyle};
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(22.0));
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.0));
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;

        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = BG;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = egui::CornerRadius::same(6);
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = CONTROL;
        v.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(0x3a, 0x40, 0x4c));
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x36, 0x3c, 0x48);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER);
    });
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(add);
        });
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .small()
            .strong()
            .color(TEXT_DIM),
    );
}

fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(CONTROL)
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(TEXT_DIM));
        });
}

fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Segmented pill control; returns the clicked value, if any.
fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    current: T,
    items: impl IntoIterator<Item = (T, String, bool)>,
) -> Option<T> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (value, label, enabled) in items {
            let selected = value == current;
            let text = egui::RichText::new(label).color(if selected {
                egui::Color32::WHITE
            } else {
                TEXT
            });
            let btn = egui::Button::new(text)
                .fill(if selected { ACCENT } else { CONTROL })
                .min_size(egui::vec2(52.0, 30.0));
            if ui.add_enabled(enabled, btn).clicked() {
                clicked = Some(value);
            }
        }
    });
    clicked
}

/// Vertical LED-style peak meter (-60..0 dBFS) with a short label underneath.
fn led_meter(ui: &mut egui::Ui, peak: f32, label: &str, tooltip: &str) {
    const SEGMENTS: usize = 24;
    let size = egui::vec2(34.0, 150.0);
    ui.vertical(|ui| {
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, BG);
        let db = 20.0 * peak.max(1e-6).log10();
        let lit = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * SEGMENTS as f32).round() as usize;
        let seg_h = (rect.height() - 6.0) / SEGMENTS as f32;
        for i in 0..SEGMENTS {
            let frac = i as f32 / SEGMENTS as f32;
            let base = if frac >= 0.95 {
                CLIP_COLOR
            } else if frac >= 0.8 {
                WARN_COLOR
            } else {
                OK_COLOR
            };
            let color = if i < lit {
                base
            } else {
                base.gamma_multiply(0.12)
            };
            let y = rect.bottom() - 3.0 - (i as f32 + 1.0) * seg_h;
            let seg = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 5.0, y + 1.0),
                egui::vec2(rect.width() - 10.0, seg_h - 2.0),
            );
            painter.rect_filled(seg, 1.0, color);
        }
        resp.on_hover_text(format!("{}\n{:.1} dBFS", tooltip, db));
        ui.add_sized(
            egui::vec2(size.x, 14.0),
            egui::Label::new(egui::RichText::new(label).small().color(TEXT_DIM)),
        );
    });
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WireAudio")
            .with_inner_size([1180.0, 760.0])
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
        apply_style(&cc.egui_ctx);
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

impl WireAudioApp {
    fn select_device(&mut self, idx: usize) {
        self.selected_idx = idx;
        if let Some(dev) = self.devices.get(idx) {
            self.config.selected_card_index = dev.card_index;
            self.config.selected_card_id = dev.id.clone();
            self.mixer_controls = list_card_mixer_controls(dev.card_index);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("WireAudio").size(18.0).strong());
            ui.label(egui::RichText::new("ALSA studio engine").color(TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⟳  Refresh").clicked() {
                    self.refresh_devices();
                }
                let (color, text) = if self.asoundrc_registered {
                    (OK_COLOR, "ALSA profile active")
                } else {
                    (WARN_COLOR, "ALSA profile not registered")
                };
                ui.label(egui::RichText::new(text).color(color));
                status_dot(ui, color);
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        caption(ui, "Devices");
        ui.add_space(4.0);
        let mut clicked = None;
        for (idx, dev) in self.devices.iter().enumerate() {
            let selected = idx == self.selected_idx;
            let (rect, resp) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
            let painter = ui.painter();
            if selected {
                painter.rect_filled(rect, 8.0, ACCENT.gamma_multiply(0.22));
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 10.0),
                        egui::vec2(3.0, 34.0),
                    ),
                    2.0,
                    ACCENT,
                );
            } else if resp.hovered() {
                painter.rect_filled(rect, 8.0, CONTROL.gamma_multiply(0.6));
            }
            let running = dev.playback_status.is_running;
            painter.circle_filled(
                egui::pos2(rect.left() + 18.0, rect.center().y),
                4.0,
                if running {
                    OK_COLOR
                } else {
                    TEXT_DIM.gamma_multiply(0.5)
                },
            );
            painter.text(
                egui::pos2(rect.left() + 32.0, rect.top() + 17.0),
                egui::Align2::LEFT_CENTER,
                &dev.name,
                egui::FontId::proportional(14.0),
                TEXT,
            );
            let sub = format!(
                "{} · {} in / {} out{}",
                if dev.is_usb { "USB" } else { "PCI" },
                dev.capture_channels,
                dev.playback_channels,
                if running {
                    format!(" · {} kHz", dev.playback_status.rate as f32 / 1000.0)
                } else {
                    String::new()
                }
            );
            painter.text(
                egui::pos2(rect.left() + 32.0, rect.top() + 37.0),
                egui::Align2::LEFT_CENTER,
                sub,
                egui::FontId::proportional(11.5),
                TEXT_DIM,
            );
            if resp.clicked() {
                clicked = Some(idx);
            }
        }
        if let Some(idx) = clicked {
            self.select_device(idx);
        }
    }

    fn config_card(&mut self, ui: &mut egui::Ui, dev: &AudioDeviceDescriptor) {
        card(ui, |ui| {
            caption(ui, "Configuration");
            ui.add_space(6.0);
            egui::Grid::new("config_grid")
                .num_columns(2)
                .spacing([20.0, 14.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Routing").color(TEXT_DIM));
                    egui::ComboBox::from_id_salt("routing_mode_combo")
                        .width(320.0)
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

                    ui.label(egui::RichText::new("Output").color(TEXT_DIM));
                    let same = format!("Same as input ({})", dev.name);
                    let current = self
                        .config
                        .output_card_id
                        .as_ref()
                        .and_then(|id| self.devices.iter().find(|d| &d.id == id))
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| same.clone());
                    egui::ComboBox::from_id_salt("output_device_combo")
                        .width(320.0)
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.config.output_card_id.is_none(), &same)
                                .clicked()
                            {
                                self.config.output_card_id = None;
                            }
                            for out_dev in self.devices.iter().filter(|d| d.playback_channels > 0) {
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
                        });
                    ui.end_row();

                    ui.label(egui::RichText::new("Sample rate").color(TEXT_DIM));
                    let rates = SUPPORTED_SAMPLE_RATES.iter().map(|&r| {
                        (
                            r,
                            format!("{}k", r as f32 / 1000.0),
                            dev.supported_rates.contains(&r),
                        )
                    });
                    if let Some(r) = segmented(ui, self.config.sample_rate, rates) {
                        self.config.sample_rate = r;
                    }
                    ui.end_row();

                    ui.label(egui::RichText::new("Buffer").color(TEXT_DIM));
                    let sizes = SUPPORTED_BUFFER_SIZES
                        .iter()
                        .map(|&f| (f, f.to_string(), true));
                    if let Some(f) = segmented(ui, self.config.period_size, sizes) {
                        self.config.period_size = f;
                    }
                    ui.end_row();
                });
        });
    }

    fn latency_card(&mut self, ui: &mut egui::Ui, dev: &AudioDeviceDescriptor) {
        card(ui, |ui| {
            caption(ui, "Latency");
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("{:.2} ms", self.config.target_period_latency_ms()))
                    .size(34.0)
                    .strong()
                    .color(OK_COLOR),
            );
            ui.label(
                egui::RichText::new(format!(
                    "one-way · {} frames @ {} kHz",
                    self.config.period_size,
                    self.config.sample_rate as f32 / 1000.0
                ))
                .color(TEXT_DIM),
            );
            ui.add_space(6.0);
            ui.label(format!(
                "{:.2} ms round-trip · {} periods",
                self.config.target_roundtrip_latency_ms(),
                self.config.periods
            ));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let apply = egui::Button::new(
                    egui::RichText::new("Apply")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(ACCENT)
                .min_size(egui::vec2(96.0, 32.0));
                if ui
                    .add(apply)
                    .on_hover_text(
                        "Write the ~/.asoundrc profile for Reaper / SoundCraft / TONE3000",
                    )
                    .clicked()
                {
                    let _ = apply_runtime_quantum_tuning(
                        self.config.period_size,
                        self.config.sample_rate,
                    );
                    match register_engine_in_asoundrc(&self.config, dev) {
                        Ok(msg) => {
                            self.status_banner = msg;
                            self.asoundrc_registered = true;
                        }
                        Err(err) => self.status_banner = err,
                    }
                    self.refresh_devices();
                }
                if self.asoundrc_registered
                    && ui
                        .add(egui::Button::new("Unregister").min_size(egui::vec2(96.0, 32.0)))
                        .clicked()
                {
                    match unregister_engine_from_asoundrc() {
                        Ok(msg) => {
                            self.status_banner = msg;
                            self.asoundrc_registered = false;
                        }
                        Err(err) => self.status_banner = err,
                    }
                }
            });
        });
    }

    fn monitor_card(&mut self, ui: &mut egui::Ui, dev: &AudioDeviceDescriptor) {
        let telemetry = self.engine.telemetry.clone();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                caption(ui, "Monitor");
                ui.add_space(12.0);
                let running = self.engine.is_running();
                let (label, fill) = if running {
                    ("■  Stop", CLIP_COLOR.gamma_multiply(0.8))
                } else {
                    ("▶  Start", CONTROL)
                };
                if ui.add(egui::Button::new(label).fill(fill)).clicked() {
                    if running {
                        self.engine.stop();
                        self.status_banner = "Stopped RT monitor thread.".to_string();
                    } else {
                        match self.engine.start(self.config.clone()) {
                            Ok(msg) => self.status_banner = msg,
                            Err(err) => self.status_banner = err,
                        }
                    }
                }
                let mut tone = telemetry.test_tone_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut tone, "Test tone 440 Hz").changed() {
                    telemetry.test_tone_enabled.store(tone, Ordering::Relaxed);
                }
                let mut mon = telemetry.direct_monitor_enabled.load(Ordering::Relaxed);
                if ui.checkbox(&mut mon, "Input monitor").changed() {
                    telemetry
                        .direct_monitor_enabled
                        .store(mon, Ordering::Relaxed);
                }
                if running {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let xruns = telemetry.xrun_count.load(Ordering::Relaxed);
                        let color = if xruns == 0 { OK_COLOR } else { WARN_COLOR };
                        ui.label(egui::RichText::new(format!("{} XRUNs", xruns)).color(color));
                        status_dot(ui, color);
                    });
                }
            });
            ui.add_space(10.0);

            let peak = |bits: &[std::sync::atomic::AtomicU32], i: usize| {
                bits.get(i)
                    .map(|a| f32::from_bits(a.load(Ordering::Relaxed)).clamp(0.0, 1.0))
                    .unwrap_or(0.0)
            };
            let short = |name: &str| name.split(" — ").next().unwrap_or(name).to_string();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Inputs").color(TEXT_DIM));
                    ui.horizontal_top(|ui| {
                        for ch in &dev.inputs {
                            led_meter(
                                ui,
                                peak(&telemetry.input_peaks_bits, ch.index),
                                &short(&ch.name),
                                &ch.name,
                            );
                        }
                    });
                });
                ui.add_space(24.0);
                ui.separator();
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Outputs").color(TEXT_DIM));
                    ui.horizontal_top(|ui| {
                        for ch in &dev.outputs {
                            led_meter(
                                ui,
                                peak(&telemetry.output_peaks_bits, ch.index),
                                &short(&ch.name),
                                &ch.name,
                            );
                        }
                    });
                });
            });
        });
    }
}

impl eframe::App for WireAudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() >= Duration::from_secs(2) {
            self.refresh_devices();
        }
        ui.ctx().request_repaint_after(Duration::from_millis(50));

        let bar = |fill| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(16, 10))
        };

        egui::Panel::top("top_bar")
            .frame(bar(SIDEBAR).stroke(egui::Stroke::new(1.0, BORDER)))
            .show(ui, |ui| self.top_bar(ui));

        egui::Panel::bottom("status_bar")
            .frame(bar(SIDEBAR).inner_margin(egui::Margin::symmetric(16, 6)))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&self.status_banner)
                        .small()
                        .color(TEXT_DIM),
                );
            });

        egui::Panel::left("device_sidebar")
            .resizable(true)
            .default_size(270.0)
            .frame(bar(SIDEBAR).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(bar(BG).inner_margin(egui::Margin::same(20)))
            .show(ui, |ui| {
                let Some(dev) = self.devices.get(self.selected_idx).cloned() else {
                    ui.label("No ALSA soundcards detected.");
                    return;
                };
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(egui::RichText::new(&dev.name).strong());
                    ui.horizontal(|ui| {
                        chip(ui, &format!("hw:{},0", dev.id));
                        chip(ui, &format!("card {}", dev.card_index));
                        chip(ui, if dev.is_usb { "USB" } else { "PCI" });
                        if let Some(us) = dev.playback_status.usb_packet_interval_us {
                            chip(ui, &format!("{} µs interval", us));
                        }
                        chip(
                            ui,
                            &format!(
                                "{} in / {} out",
                                dev.capture_channels, dev.playback_channels
                            ),
                        );
                    });
                    ui.add_space(14.0);

                    let gap = 14.0;
                    let total = ui.available_width();
                    let left_w = (total * 0.64).max(420.0).min(total - 260.0 - gap);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.allocate_ui(egui::vec2(left_w, 0.0), |ui| self.config_card(ui, &dev));
                        ui.allocate_ui(egui::vec2(ui.available_width(), 0.0), |ui| {
                            self.latency_card(ui, &dev)
                        });
                    });
                    ui.add_space(gap);
                    self.monitor_card(ui, &dev);
                    ui.add_space(gap);

                    card(ui, |ui| {
                        egui::CollapsingHeader::new(
                            egui::RichText::new(format!(
                                "Hardware controls ({})",
                                self.mixer_controls.len()
                            ))
                            .color(TEXT_DIM),
                        )
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Config: {}",
                                    asoundrc_path().display()
                                ))
                                .small()
                                .color(TEXT_DIM),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for ctl in &self.mixer_controls {
                                    chip(ui, ctl);
                                }
                            });
                        });
                    });
                });
            });
    }
}
