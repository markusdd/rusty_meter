use egui::{FontFamily, FontId, SliderClamping, Vec2};
use egui_dock::{DockArea, DockState, Style, TabViewer};
use egui_dropdown::DropDownBox;
use mio_serial::{DataBits, SerialPort, SerialPortBuilderExt};
use std::collections::VecDeque;

use crate::helpers::{format_measurement, powered_by};
use crate::multimeter::{GenScpi, MeterMode, RangeCmd};

// Enum to represent tab types
#[derive(Clone, PartialEq)]
pub enum PlotTab {
    Graph,
    Histogram,
}

// Tab viewer implementation for PlotTab
struct PlotTabViewer<'a> {
    values: &'a VecDeque<f64>,
    hist_values: &'a mut VecDeque<f64>,
    reverse_graph: &'a mut bool,
    graph_line_color: egui::Color32,
    hist_bar_color: egui::Color32,
    mem_depth: &'a mut usize,
    curr_meas: f64,
    metermode: MeterMode,
    graph_config: &'a mut super::graph::GraphConfig,
    hist_collect_active: &'a mut bool,
    hist_collect_interval_ms: &'a mut u64,
    hist_mem_depth: &'a mut usize,
    mem_depth_max: usize,
    graph_update_interval_ms: &'a mut u64,
    graph_update_interval_max: u64,
    hist_mem_depth_max: usize,
    curr_unit: &'a str,
}

impl TabViewer for PlotTabViewer<'_> {
    type Tab = PlotTab;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            PlotTab::Graph => "Graph".into(),
            PlotTab::Histogram => "Histogram".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            PlotTab::Graph => super::graph::show_line_graph(
                ui,
                self.values,
                *self.reverse_graph,
                self.graph_line_color,
                self.mem_depth,
                self.graph_update_interval_ms,
                self.reverse_graph,
                self.mem_depth_max,
                self.graph_update_interval_max,
                self.curr_unit,
            ),
            PlotTab::Histogram => super::graph::show_histogram(
                ui,
                self.hist_values,
                self.curr_meas,
                self.metermode,
                self.graph_config,
                self.hist_bar_color,
                self.hist_collect_active,
                self.hist_collect_interval_ms,
                self.hist_mem_depth,
                self.hist_mem_depth_max,
            ),
        }
    }
}

impl super::MyApp {
    /// Called by the framework to save state before shutdown.
    pub fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Save recording data if recording is active
        if self.recording_active {
            self.save_recording_data();
        }
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    pub fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let is_web = cfg!(target_arch = "wasm32");

        // On startup, handle certain items once
        if !self.is_init {
            if let Ok(ports) = mio_serial::available_ports() {
                for p in ports {
                    self.portlist.push_front(p.port_name);
                }
            }
            // Apply initial sampling rate
            self.confstring = self
                .ratecmd
                .gen_scpi(self.ratecmd.get_opt(self.curr_rate).0);
            if let Some(tx) = self.serial_tx.clone() {
                let cmd = self.confstring.clone();
                let value_debug = self.value_debug;
                tokio::spawn(async move {
                    if let Err(e) = tx.send(cmd).await {
                        if value_debug {
                            println!("Failed to queue initial rate command: {}", e);
                        }
                    }
                });
            }
            // Initialize dock state
            let tabs = vec![PlotTab::Graph, PlotTab::Histogram];
            self.plot_dock_state = DockState::new(tabs);
            self.is_init = true;
        }

        // Process all available measurements
        if let Some(ref mut rx) = self.serial_rx {
            while let Ok(meas_opt) = rx.try_recv() {
                if let Some(meas) = meas_opt {
                    self.curr_meas = meas; // Update curr_meas with new data
                }
            }
        }

        // Process all available mode updates
        if let Some(ref mut rx) = self.mode_rx {
            while let Ok(mode) = rx.try_recv() {
                if mode != self.metermode {
                    self.metermode = mode;
                    self.values = VecDeque::with_capacity(self.mem_depth);
                    self.hist_values = VecDeque::with_capacity(self.hist_mem_depth); // Reset histogram buffer
                    match mode {
                        MeterMode::Vdc => {
                            self.curr_unit = "VDC".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "VDC");
                        }
                        MeterMode::Vac => {
                            self.curr_unit = "VAC".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "VAC");
                        }
                        MeterMode::Adc => {
                            self.curr_unit = "ADC".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "ADC");
                        }
                        MeterMode::Aac => {
                            self.curr_unit = "AAC".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "AAC");
                        }
                        MeterMode::Res => {
                            self.curr_unit = "Ohm".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "RES");
                        }
                        MeterMode::Cap => {
                            self.curr_unit = "F".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "CAP");
                        }
                        MeterMode::Freq => {
                            self.curr_unit = "Hz".to_owned();
                            self.rangecmd = None;
                        }
                        MeterMode::Per => {
                            self.curr_unit = "s".to_owned();
                            self.rangecmd = None;
                        }
                        MeterMode::Diod => {
                            self.curr_unit = "V".to_owned();
                            self.rangecmd = None;
                        }
                        MeterMode::Cont => {
                            self.curr_unit = "Ohm".to_owned();
                            self.rangecmd = None;
                        }
                        MeterMode::Temp => {
                            self.curr_unit = "°C".to_owned();
                            self.rangecmd = RangeCmd::new(&self.curr_meter, "TEMP");
                        }
                    }
                    self.curr_range = 0;
                    if self.value_debug {
                        println!("Updated metermode to: {:?}", mode);
                    }
                }
            }
        }

        // Handle graph and histogram updates and recording based on the configured interval
        let current_time = ctx.input(|i| i.time); // Get current time in seconds
        let graph_interval = *self.graph_update_interval_shared.lock().unwrap() as f64 / 1000.0; // Convert ms to seconds
        if current_time - self.last_graph_update >= graph_interval {
            if !self.curr_meas.is_nan() {
                self.values.push_back(self.curr_meas);
                self.update_histogram(self.curr_meas); // Update histogram with new measurement
                while self.values.len() > self.mem_depth {
                    self.values.pop_front();
                }
                // Record measurement for fixed interval mode
                if self.recording_active
                    && matches!(self.recording_mode, super::RecordingMode::FixedInterval)
                    && current_time - self.last_record_time
                        >= self.recording_interval_ms as f64 / 1000.0
                {
                    self.record_measurement();
                    self.last_record_time = current_time;
                }
            }
            self.last_graph_update = current_time;
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    if !is_web && ui.button("Quit").clicked() {
                        self.disconnect(); // Use disconnect method instead of partial cleanup
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.add_space(16.0);
                egui::widgets::global_theme_preference_buttons(ui);
            });
        });

        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                powered_by(ui);
                ui.hyperlink_to(
                    format!("Version: v{}", super::VERSION),
                    format!(
                        "https://github.com/markusdd/RustyMeter/releases/tag/v{}",
                        super::VERSION
                    ),
                );
                egui::warn_if_debug_build(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if is_web {
                ui.heading("RustyMeter");
            }

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Serial port: ");
                    ui.add(
                        DropDownBox::from_iter(
                            &self.portlist,
                            "portlistbox",
                            &mut self.serial_port,
                            |ui, text| ui.selectable_label(false, text),
                        )
                        .desired_width(150.0)
                        .select_on_focus(true)
                        .filter_by_input(false),
                    );

                    match self.connection_state {
                        super::ConnectionState::Disconnected => {
                            if ui.button("Connect").clicked() {
                                self.connection_state = super::ConnectionState::Connecting;
                                self.connection_error = None;
                                match mio_serial::new(&self.serial_port, self.baud_rate)
                                    .open_native_async()
                                {
                                    Ok(serial) => {
                                        self.serial = Some(serial);
                                        if let Some(ref mut serial) = self.serial {
                                            let _ = serial.set_data_bits(DataBits::Eight);
                                            let _ = serial.set_stop_bits(mio_serial::StopBits::One);
                                            let _ = serial.set_parity(mio_serial::Parity::None);
                                            self.connection_state =
                                                super::ConnectionState::Connected;
                                            self.spawn_serial_task();
                                            self.spawn_graph_update_task(ctx.clone());
                                        }
                                    }
                                    Err(e) => {
                                        self.connection_state =
                                            super::ConnectionState::Disconnected;
                                        self.connection_error =
                                            Some(format!("Failed to connect: {}", e));
                                    }
                                }
                            }
                        }
                        super::ConnectionState::Connecting => {
                            ui.label("Connecting...");
                        }
                        super::ConnectionState::Connected => {
                            if ui.button("Disconnect").clicked() {
                                self.disconnect();
                            }
                        }
                    }

                    // Recording button
                    if ui.button("Start Recording").clicked() {
                        self.recording_open = true;
                    }
                });

                ui.horizontal(|ui| {
                    let device = self.device.lock().unwrap();
                    match self.connection_state {
                        super::ConnectionState::Disconnected => {
                            if let Some(ref error) = self.connection_error {
                                ui.label(egui::RichText::new(error).color(egui::Color32::RED));
                            } else {
                                ui.label("Not connected.");
                            }
                        }
                        super::ConnectionState::Connecting => {
                            ui.label("Attempting to connect...");
                        }
                        super::ConnectionState::Connected => {
                            if !device.is_empty() {
                                ui.label("Connected to: ");
                                ui.label(&*device);
                            } else {
                                ui.label("Connected, awaiting device ID...");
                            }
                        }
                    }
                });
            });

            ui.separator();

            ui.horizontal(|ui| {
                // Determine if the background and shadow should be dark red based on mode and threshold
                let is_below_threshold = match self.metermode {
                    MeterMode::Cont => self
                        .values
                        .back()
                        .is_some_and(|&val| val <= self.cont_threshold as f64),
                    MeterMode::Diod => self
                        .values
                        .back()
                        .is_some_and(|&val| val <= self.diod_threshold as f64),
                    _ => false,
                };
                let background_color = if is_below_threshold {
                    egui::Color32::from_rgb(139, 0, 0) // Dark red for threshold condition
                } else {
                    self.box_background_color // Use custom background color
                };
                let shadow_color = if is_below_threshold {
                    // don't do this for now egui::Color32::from_rgba_unmultiplied(139, 0, 0, 180) // Dark red shadow with alpha
                    egui::Color32::from_black_alpha(180) // Default black shadow
                } else {
                    egui::Color32::from_black_alpha(180) // Default black shadow
                };

                let meter_frame = egui::Frame {
                    inner_margin: 12.0.into(),
                    outer_margin: 24.0.into(),
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
                        color: shadow_color,
                    },
                    fill: background_color,
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                meter_frame.show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.allocate_ui_with_layout(
                        Vec2 { x: 400.0, y: 300.0 },
                        egui::Layout::top_down(egui::Align::RIGHT).with_cross_justify(false),
                        |ui| {
                            let (formatted_value, display_unit) = format_measurement(
                                self.curr_meas,
                                10,
                                1_000_000.0,
                                0.0001,
                                &self.metermode,
                            );
                            ui.label(
                                egui::RichText::new(formatted_value)
                                    .color(self.measurement_font_color)
                                    .font(FontId {
                                        size: 60.0,
                                        family: FontFamily::Name("B612Mono-Bold".into()),
                                    }),
                            );
                            ui.label(
                                egui::RichText::new(format!("{:>10}", display_unit))
                                    .color(self.measurement_font_color)
                                    .font(FontId {
                                        size: 20.0,
                                        family: FontFamily::Name("B612Mono-Bold".into()),
                                    }),
                            );
                        },
                    );
                });

                let control_frame = egui::Frame {
                    inner_margin: 12.0.into(),
                    outer_margin: 24.0.into(),
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: self.box_background_color,
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                control_frame.show(ui, |ui| {
                    ui.vertical(|ui| {
                        let btn_size = Vec2 { x: 70.0, y: 20.0 };
                        ui.horizontal(|ui| {
                            let vdc_btn = egui::Button::new("VDC")
                                .selected(self.metermode == MeterMode::Vdc)
                                .min_size(btn_size);
                            if ui.add(vdc_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Vdc,
                                    "VDC",
                                    "CONF:VOLT:DC AUTO\n",
                                    Some("VDC"),
                                    None,
                                );
                            }
                            let vac_btn = egui::Button::new("VAC")
                                .selected(self.metermode == MeterMode::Vac)
                                .min_size(btn_size);
                            if ui.add(vac_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Vac,
                                    "VAC",
                                    "CONF:VOLT:AC AUTO\n",
                                    Some("VAC"),
                                    None,
                                );
                            }
                            let adc_btn = egui::Button::new("ADC")
                                .selected(self.metermode == MeterMode::Adc)
                                .min_size(btn_size);
                            if ui.add(adc_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Adc,
                                    "ADC",
                                    "CONF:CURR:DC AUTO\n",
                                    Some("ADC"),
                                    None,
                                );
                            }
                            let aac_btn = egui::Button::new("AAC")
                                .selected(self.metermode == MeterMode::Aac)
                                .min_size(btn_size);
                            if ui.add(aac_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Aac,
                                    "AAC",
                                    "CONF:CURR:AC AUTO\n",
                                    Some("AAC"),
                                    None,
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            let res_btn = egui::Button::new("Ohm")
                                .selected(self.metermode == MeterMode::Res)
                                .min_size(btn_size);
                            if ui.add(res_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Res,
                                    "Ohm",
                                    "CONF:RES AUTO\n",
                                    Some("RES"),
                                    None,
                                );
                            }
                            let cap_btn = egui::Button::new("C")
                                .selected(self.metermode == MeterMode::Cap)
                                .min_size(btn_size);
                            if ui.add(cap_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Cap,
                                    "F",
                                    "CONF:CAP AUTO\n",
                                    Some("CAP"),
                                    None,
                                );
                            }
                            let freq_btn = egui::Button::new("Freq")
                                .selected(self.metermode == MeterMode::Freq)
                                .min_size(btn_size);
                            if ui.add(freq_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Freq,
                                    "Hz",
                                    "CONF:FREQ\n",
                                    Some("FREQ"),
                                    None,
                                );
                            }
                            let per_btn = egui::Button::new("Period")
                                .selected(self.metermode == MeterMode::Per)
                                .min_size(btn_size);
                            if ui.add(per_btn).clicked() {
                                self.set_mode(MeterMode::Per, "s", "CONF:PER\n", Some("PER"), None);
                            }
                        });
                        ui.horizontal(|ui| {
                            let diod_btn = egui::Button::new("Diode")
                                .selected(self.metermode == MeterMode::Diod)
                                .min_size(btn_size);
                            if ui.add(diod_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Diod,
                                    "V",
                                    "CONF:DIOD\n",
                                    Some("DIOD"),
                                    Some(self.beeper_enabled),
                                );
                            }
                            let cont_btn = egui::Button::new("Cont")
                                .selected(self.metermode == MeterMode::Cont)
                                .min_size(btn_size);
                            if ui.add(cont_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Cont,
                                    "Ohm",
                                    "CONF:CONT\n",
                                    Some("CONT"),
                                    Some(self.beeper_enabled),
                                );
                            }
                            let temp_btn = egui::Button::new("Temp")
                                .selected(self.metermode == MeterMode::Temp)
                                .min_size(btn_size);
                            if ui.add(temp_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Temp,
                                    "°C",
                                    "CONF:TEMP:RTD PT100\n",
                                    Some("TEMP"),
                                    None,
                                );
                            }
                        });
                    });
                });

                let options_frame = egui::Frame {
                    inner_margin: 12.0.into(),
                    outer_margin: 24.0.into(),
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: self.box_background_color,
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                options_frame.show(ui, |ui| {
                    ui.vertical(|ui| {
                        let ratebox = egui::ComboBox::from_label("Sampling Rate").show_index(
                            ui,
                            &mut self.curr_rate,
                            self.ratecmd.len(),
                            |i| self.ratecmd.get_opt(i).0,
                        );
                        if ratebox.changed() {
                            self.confstring = self
                                .ratecmd
                                .gen_scpi(self.ratecmd.get_opt(self.curr_rate).0);
                            if let Some(tx) = self.serial_tx.clone() {
                                let cmd = self.confstring.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = tx.send(cmd).await {
                                        println!("Failed to queue command: {}", e);
                                    }
                                });
                            }
                            if self.value_debug {
                                println!("Selected Rate changed: {}", self.confstring);
                            }
                        }
                        if let Some(rangecmd) = &self.rangecmd {
                            let rangebox = egui::ComboBox::from_label("Range").show_index(
                                ui,
                                &mut self.curr_range,
                                rangecmd.len(),
                                |i| rangecmd.get_opt(i).0,
                            );
                            if rangebox.changed() {
                                self.confstring =
                                    rangecmd.gen_scpi(rangecmd.get_opt(self.curr_range).0);
                                if let Some(tx) = self.serial_tx.clone() {
                                    let cmd = self.confstring.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = tx.send(cmd).await {
                                            println!("Failed to queue command: {}", e);
                                        }
                                    });
                                }
                                if self.value_debug {
                                    println!("Selected Range changed: {}", self.confstring);
                                }
                            }
                        }
                        // Add beeper and threshold controls for CONT and DIOD modes
                        if self.metermode == MeterMode::Cont || self.metermode == MeterMode::Diod {
                            let mut beeper = self.beeper_enabled;
                            if ui.checkbox(&mut beeper, "Beeper").changed() {
                                self.beeper_enabled = beeper;
                                if let Some(tx) = self.serial_tx.clone() {
                                    let cmd = if beeper {
                                        "SYST:BEEP:STATe ON\n".to_string()
                                    } else {
                                        "SYST:BEEP:STATe OFF\n".to_string()
                                    };
                                    let value_debug = self.value_debug;
                                    tokio::spawn(async move {
                                        if let Err(e) = tx.send(cmd).await {
                                            if value_debug {
                                                println!("Failed to queue beeper command: {}", e);
                                            }
                                        }
                                    });
                                }
                            }

                            if self.metermode == MeterMode::Cont {
                                let threshold_slider = ui.add(
                                    egui::Slider::new(&mut self.cont_threshold, 0..=1000)
                                        .text("Threshold (Ω)")
                                        .step_by(1.0)
                                        .clamping(SliderClamping::Always),
                                );
                                if threshold_slider.drag_stopped() || threshold_slider.lost_focus()
                                {
                                    if let Some(tx) = self.serial_tx.clone() {
                                        let cmd =
                                            format!("CONT:THREshold {}\n", self.cont_threshold);
                                        let value_debug = self.value_debug;
                                        tokio::spawn(async move {
                                            if let Err(e) = tx.send(cmd).await {
                                                if value_debug {
                                                    println!(
                                                        "Failed to queue threshold command: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        });
                                    }
                                }
                            } else if self.metermode == MeterMode::Diod {
                                let threshold_slider = ui.add(
                                    egui::Slider::new(&mut self.diod_threshold, 0.0..=3.0)
                                        .text("Threshold (V)")
                                        .step_by(0.1)
                                        .clamping(SliderClamping::Always),
                                );
                                if threshold_slider.drag_stopped() || threshold_slider.lost_focus()
                                {
                                    if let Some(tx) = self.serial_tx.clone() {
                                        let cmd =
                                            format!("DIOD:THREshold {}\n", self.diod_threshold);
                                        let value_debug = self.value_debug;
                                        tokio::spawn(async move {
                                            if let Err(e) = tx.send(cmd).await {
                                                if value_debug {
                                                    println!(
                                                        "Failed to queue threshold command: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    });
                });
            });

            ui.separator();

            // Dock area for graph and histogram
            {
                // Scope to limit the mutable borrow of plot_dock_state
                let dock_state = &mut self.plot_dock_state;
                let mut viewer = PlotTabViewer {
                    values: &self.values,
                    hist_values: &mut self.hist_values,
                    reverse_graph: &mut self.reverse_graph,
                    graph_line_color: self.graph_line_color,
                    hist_bar_color: self.hist_bar_color,
                    mem_depth: &mut self.mem_depth,
                    curr_meas: self.curr_meas,
                    metermode: self.metermode,
                    graph_config: &mut self.graph_config,
                    hist_collect_active: &mut self.hist_collect_active,
                    hist_collect_interval_ms: &mut self.hist_collect_interval_ms,
                    hist_mem_depth: &mut self.hist_mem_depth,
                    mem_depth_max: self.mem_depth_max,
                    graph_update_interval_ms: &mut self.graph_update_interval_ms,
                    graph_update_interval_max: self.graph_update_interval_max,
                    hist_mem_depth_max: self.hist_mem_depth_max,
                    curr_unit: &self.curr_unit,
                };
                DockArea::new(dock_state)
                    .style(Style::from_egui(ui.style()))
                    .show_close_buttons(false)
                    .show_inside(ui, &mut viewer);
            }

            // Show settings and recording windows
            self.show_settings(ctx);
            self.show_recording_window(ctx);
        });
    }
}

impl eframe::App for super::MyApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.save(storage);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.update(ctx, frame);
    }
}
