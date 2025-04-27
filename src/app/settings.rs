use std::fs::File;

use csv::WriterBuilder;
use egui::{color_picker::color_picker_color32, Context, TextEdit, Window};
use rfd::FileDialog;
use xlsxwriter::Workbook;

impl super::MyApp {
    pub fn show_settings(&mut self, ctx: &Context) {
        if self.settings_open {
            Window::new("Settings")
                .auto_sized()
                .interactable(true)
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.heading("Settings");
                        ui.checkbox(&mut self.connect_on_startup, "Connect on startup");
                        ui.checkbox(&mut self.lock_remote, "Lock meter in remote mode");
                        ui.checkbox(
                            &mut self.parity,
                            "Use parity bit (ignored right now, always None)",
                        );
                        let mut value_debug = *self.value_debug_shared.lock().unwrap();
                        if ui
                            .checkbox(&mut value_debug, "Value debug (print to CLI)")
                            .changed()
                        {
                            self.value_debug = value_debug;
                            *self.value_debug_shared.lock().unwrap() = value_debug;
                        }
                        ui.label("Baud rate:");
                        ui.add(
                            TextEdit::singleline(&mut self.baud_rate.to_string())
                                .desired_width(800.0),
                        );
                        ui.label("Data Bits (ignored right now, always 8):");
                        ui.add(
                            TextEdit::singleline(&mut self.bits.to_string()).desired_width(800.0),
                        );
                        ui.label("Stop bits (ignored right now, always 1):");
                        ui.add(
                            TextEdit::singleline(&mut self.stop_bits.to_string())
                                .desired_width(800.0),
                        );
                        ui.label("Serial poll interval (ms):");
                        let mut interval_str = self.poll_interval_ms.to_string();
                        if ui
                            .add(
                                TextEdit::singleline(&mut interval_str)
                                    .desired_width(800.0)
                                    .hint_text("Enter polling interval in ms"),
                            )
                            .changed()
                        {
                            if let Ok(new_interval) = interval_str.parse::<u64>() {
                                if new_interval > 0 {
                                    self.poll_interval_ms = new_interval;
                                    *self.poll_interval_shared.lock().unwrap() = new_interval;
                                }
                            }
                        }
                        ui.label("Maximum graph memory depth:");
                        let mut max_depth_str = self.mem_depth_max.to_string();
                        if ui
                            .add(
                                TextEdit::singleline(&mut max_depth_str)
                                    .desired_width(800.0)
                                    .hint_text("Enter maximum number of values for graph"),
                            )
                            .changed()
                        {
                            if let Ok(new_max_depth) = max_depth_str.parse::<usize>() {
                                if new_max_depth >= 10 {
                                    // Ensure minimum is at least 10
                                    self.mem_depth_max = new_max_depth;
                                    // Clamp mem_depth to new max if necessary
                                    if self.mem_depth > self.mem_depth_max {
                                        self.mem_depth = self.mem_depth_max;
                                        while self.values.len() > self.mem_depth {
                                            self.values.pop_front();
                                        }
                                    }
                                }
                            }
                        }
                        ui.label("Maximum graph update interval (ms):");
                        let mut max_graph_interval_str = self.graph_update_interval_max.to_string();
                        if ui
                            .add(
                                TextEdit::singleline(&mut max_graph_interval_str)
                                    .desired_width(800.0)
                                    .hint_text("Enter maximum graph update interval in ms"),
                            )
                            .changed()
                        {
                            if let Ok(new_max_interval) = max_graph_interval_str.parse::<u64>() {
                                if new_max_interval >= 10 {
                                    self.graph_update_interval_max = new_max_interval;
                                    // Clamp graph_update_interval_ms to new max if necessary
                                    if self.graph_update_interval_ms
                                        > self.graph_update_interval_max
                                    {
                                        self.graph_update_interval_ms =
                                            self.graph_update_interval_max;
                                        *self.graph_update_interval_shared.lock().unwrap() =
                                            self.graph_update_interval_max;
                                    }
                                }
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label("Graph line color:");
                                color_picker_color32(
                                    ui,
                                    &mut self.graph_line_color,
                                    egui::color_picker::Alpha::Opaque,
                                );
                            });
                            ui.vertical(|ui| {
                                ui.label("Measurement font color:");
                                color_picker_color32(
                                    ui,
                                    &mut self.measurement_font_color,
                                    egui::color_picker::Alpha::Opaque,
                                );
                            });
                            ui.vertical(|ui| {
                                ui.label("Box background color:");
                                color_picker_color32(
                                    ui,
                                    &mut self.box_background_color,
                                    egui::color_picker::Alpha::Opaque,
                                );
                            });
                        });
                        if ui.button("Close").clicked() {
                            self.settings_open = false;
                        }
                    });
                });
        }
    }

    pub fn show_recording_window(&mut self, ctx: &Context) {
        if self.recording_open {
            Window::new("Data Recording")
                .auto_sized()
                .interactable(true)
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.heading("Data Recording");

                        // Format selection
                        ui.horizontal(|ui| {
                            ui.label("Output format: ");
                            egui::ComboBox::from_label("")
                                .selected_text(match self.recording_format {
                                    super::RecordingFormat::Csv => "CSV",
                                    super::RecordingFormat::Json => "JSON",
                                    super::RecordingFormat::Xlsx => "XLSX",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.recording_format,
                                        super::RecordingFormat::Csv,
                                        "CSV",
                                    );
                                    ui.selectable_value(
                                        &mut self.recording_format,
                                        super::RecordingFormat::Json,
                                        "JSON",
                                    );
                                    ui.selectable_value(
                                        &mut self.recording_format,
                                        super::RecordingFormat::Xlsx,
                                        "XLSX",
                                    );
                                });
                        });

                        // File path selection
                        ui.horizontal(|ui| {
                            ui.label("File path: ");
                            ui.add(
                                TextEdit::singleline(&mut self.recording_file_path)
                                    .desired_width(300.0)
                                    .hint_text("Select or enter file path"),
                            );
                            if ui.button("Browse").clicked() {
                                if let Some(path) = FileDialog::new()
                                    .add_filter(
                                        "Data Files",
                                        match self.recording_format {
                                            super::RecordingFormat::Csv => &["csv"],
                                            super::RecordingFormat::Json => &["json"],
                                            super::RecordingFormat::Xlsx => &["xlsx"],
                                        },
                                    )
                                    .save_file()
                                {
                                    self.recording_file_path = path.to_string_lossy().into_owned();
                                }
                            }
                        });

                        // Recording mode
                        ui.horizontal(|ui| {
                            ui.label("Recording mode: ");
                            ui.radio_value(
                                &mut self.recording_mode,
                                super::RecordingMode::FixedInterval,
                                "Fixed Interval",
                            );
                            ui.radio_value(
                                &mut self.recording_mode,
                                super::RecordingMode::Manual,
                                "Manual",
                            );
                        });

                        // Interval for fixed interval mode
                        if matches!(self.recording_mode, super::RecordingMode::FixedInterval) {
                            ui.horizontal(|ui| {
                                ui.label("Interval (ms): ");
                                ui.add(
                                    TextEdit::singleline(
                                        &mut self.recording_interval_ms.to_string(),
                                    )
                                    .desired_width(100.0)
                                    .hint_text("Enter interval in ms"),
                                );
                            });
                        }

                        // Start/Stop recording
                        if ui
                            .button(if self.recording_active {
                                "Stop Recording"
                            } else {
                                "Start Recording"
                            })
                            .clicked()
                        {
                            if self.recording_active {
                                self.recording_active = false;
                                // Save data to file
                                self.save_recording_data();
                            } else if !self.recording_file_path.is_empty() {
                                self.recording_active = true;
                            }
                        }

                        // Manual record button
                        if matches!(self.recording_mode, super::RecordingMode::Manual)
                            && self.recording_active
                        {
                            if ui.button("Record Now").clicked() {
                                self.record_measurement();
                            }
                        }

                        // Data table
                        ui.separator();
                        ui.label("Recorded Data:");
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("recording_table")
                                .striped(true)
                                .show(ui, |ui| {
                                    ui.label("Timestamp");
                                    ui.label("Unit");
                                    ui.label("Value");
                                    ui.end_row();

                                    for record in &self.recording_data {
                                        ui.label(record.timestamp.to_rfc3339());
                                        ui.label(&record.unit);
                                        ui.label(format!("{:.4}", record.value));
                                        ui.end_row();
                                    }
                                });
                        });

                        if ui.button("Clear Data").clicked() {
                            self.recording_data.clear();
                        }

                        if ui.button("Close").clicked() {
                            if self.recording_active {
                                self.recording_active = false;
                                self.save_recording_data();
                            }
                            self.recording_open = false;
                        }
                    });
                });
        }
    }

    pub fn record_measurement(&mut self) {
        if !self.curr_meas.is_nan() {
            self.recording_data.push(super::Record {
                timestamp: chrono::Utc::now(),
                unit: self.curr_unit.clone(),
                value: self.curr_meas,
            });
        }
    }

    pub fn save_recording_data(&self) {
        if self.recording_data.is_empty() || self.recording_file_path.is_empty() {
            return;
        }

        match self.recording_format {
            super::RecordingFormat::Csv => {
                let file =
                    File::create(&self.recording_file_path).expect("Failed to create CSV file");
                let mut writer = WriterBuilder::new().from_writer(file);
                writer
                    .write_record(&["Timestamp", "Unit", "Value"])
                    .expect("Failed to write CSV header");
                for record in &self.recording_data {
                    writer
                        .write_record(&[
                            record.timestamp.to_rfc3339(),
                            record.unit.clone(),
                            record.value.to_string(),
                        ])
                        .expect("Failed to write CSV record");
                }
                writer.flush().expect("Failed to flush CSV writer");
            }
            super::RecordingFormat::Json => {
                let file =
                    File::create(&self.recording_file_path).expect("Failed to create JSON file");
                serde_json::to_writer(file, &self.recording_data)
                    .expect("Failed to write JSON data");
            }
            super::RecordingFormat::Xlsx => {
                let workbook =
                    Workbook::new(&self.recording_file_path).expect("Failed to create XLSX file");
                let mut sheet = workbook
                    .add_worksheet(None)
                    .expect("Failed to add worksheet");
                sheet
                    .write_string(0, 0, "Timestamp", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 1, "Unit", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 2, "Value", None)
                    .expect("Failed to write XLSX header");
                for (i, record) in self.recording_data.iter().enumerate() {
                    sheet
                        .write_string((i + 1) as u32, 0, &record.timestamp.to_rfc3339(), None)
                        .expect("Failed to write XLSX record");
                    sheet
                        .write_string((i + 1) as u32, 1, &record.unit, None)
                        .expect("Failed to write XLSX record");
                    sheet
                        .write_number((i + 1) as u32, 2, record.value, None)
                        .expect("Failed to write XLSX record");
                }
                workbook.close().expect("Failed to close XLSX workbook");
            }
        }
    }
}
