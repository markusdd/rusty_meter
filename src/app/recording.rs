use std::fs::File;
use std::path::Path;

use csv::WriterBuilder;
use egui::{Context, FontId, RichText, TextEdit, ViewportBuilder, ViewportId};
use egui_extras::{Column, TableBuilder};
use rfd::FileDialog;
use xlsxwriter::Workbook;

impl super::MyApp {
    pub fn show_recording_window(&mut self, ctx: &Context) {
        if self.recording_open {
            let viewport_id = ViewportId::from_hash_of("recording_viewport");

            ctx.show_viewport_immediate(
                viewport_id,
                ViewportBuilder::default()
                    .with_title("Data Recording")
                    .with_inner_size([600.0, 400.0])
                    .with_resizable(true),
                |ctx, class| {
                    assert!(
                        class == egui::ViewportClass::Immediate,
                        "This example is only intended to run as an immediate viewport"
                    );

                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.vertical(|ui| {
                            ui.heading("Data Recording");

                            // Format selection
                            ui.horizontal(|ui| {
                                ui.label("Output format: ");
                                let previous_format = self.recording_format.clone();
                                ui.push_id("output_format", |ui| {
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
                                // Update file extension if format changed and path exists
                                if self.recording_format != previous_format
                                    && !self.recording_file_path.is_empty()
                                {
                                    let path = Path::new(&self.recording_file_path);
                                    let stem = path
                                        .file_stem()
                                        .map(|s| s.to_string_lossy())
                                        .unwrap_or_default();
                                    let parent = path
                                        .parent()
                                        .map(|p| p.to_string_lossy())
                                        .unwrap_or_default();
                                    let new_extension = match self.recording_format {
                                        super::RecordingFormat::Csv => "csv",
                                        super::RecordingFormat::Json => "json",
                                        super::RecordingFormat::Xlsx => "xlsx",
                                    };
                                    self.recording_file_path = if parent.is_empty() {
                                        format!("{}.{}", stem, new_extension)
                                    } else {
                                        format!("{}/{}.{}", parent, stem, new_extension)
                                    };
                                }
                            });

                            // Timestamp format selection
                            ui.horizontal(|ui| {
                                ui.label("Timestamp format: ");
                                ui.push_id("timestamp_format", |ui| {
                                    egui::ComboBox::from_label("")
                                        .selected_text(match self.recording_timestamp_format {
                                            super::TimestampFormat::Rfc3339 => "RFC3339",
                                            super::TimestampFormat::Unix => "Unix",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.recording_timestamp_format,
                                                super::TimestampFormat::Rfc3339,
                                                "RFC3339",
                                            );
                                            ui.selectable_value(
                                                &mut self.recording_timestamp_format,
                                                super::TimestampFormat::Unix,
                                                "Unix",
                                            );
                                        });
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
                                        self.recording_file_path =
                                            path.to_string_lossy().into_owned();
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
                                    self.save_recording_data();
                                } else if !self.recording_file_path.is_empty() {
                                    self.recording_active = true;
                                }
                            }

                            // Manual record button
                            if matches!(self.recording_mode, super::RecordingMode::Manual)
                                && self.recording_active
                                && ui.button("Record Now").clicked()
                            {
                                self.record_measurement();
                            }

                            // Clear Data button
                            ui.add_space(10.0);
                            if ui.button("Clear Data").clicked() {
                                self.recording_data.clear();
                            }

                            // Data table
                            ui.separator();
                            TableBuilder::new(ui)
                                .striped(true)
                                .resizable(true)
                                .vscroll(true)
                                .stick_to_bottom(true)
                                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                .column(Column::initial(100.0).at_least(50.0))
                                .column(Column::initial(200.0).at_least(100.0))
                                .column(Column::initial(100.0).at_least(50.0))
                                .column(Column::initial(100.0).at_least(50.0))
                                .header(20.0, |mut header| {
                                    header.col(|ui| {
                                        ui.label(
                                            RichText::new("Index").font(FontId::proportional(16.0)),
                                        );
                                    });
                                    header.col(|ui| {
                                        ui.label(
                                            RichText::new("Timestamp")
                                                .font(FontId::proportional(16.0)),
                                        );
                                    });
                                    header.col(|ui| {
                                        ui.label(
                                            RichText::new("Unit").font(FontId::proportional(16.0)),
                                        );
                                    });
                                    header.col(|ui| {
                                        ui.label(
                                            RichText::new("Value").font(FontId::proportional(16.0)),
                                        );
                                    });
                                })
                                .body(|mut body| {
                                    for record in self.recording_data.iter() {
                                        body.row(20.0, |mut row| {
                                            row.col(|ui| {
                                                ui.label(format!("{}", record.index));
                                            });
                                            row.col(|ui| match self.recording_timestamp_format {
                                                super::TimestampFormat::Rfc3339 => {
                                                    ui.label(record.timestamp.to_rfc3339());
                                                }
                                                super::TimestampFormat::Unix => {
                                                    ui.label(format!(
                                                        "{}",
                                                        record.timestamp.timestamp()
                                                    ));
                                                }
                                            });
                                            row.col(|ui| {
                                                ui.label(&record.unit);
                                            });
                                            row.col(|ui| {
                                                ui.label(format!("{:.4}", record.value));
                                            });
                                        });
                                    }
                                });
                        });
                    });

                    // Handle close request (e.g., window close button)
                    if ctx.input(|i| i.viewport().close_requested()) {
                        if self.recording_active {
                            self.recording_active = false;
                            self.save_recording_data();
                        }
                        self.recording_open = false;
                    }

                    // Request repaint to keep the viewport alive
                    ctx.request_repaint();
                },
            );
        }
    }

    pub fn record_measurement(&mut self) {
        if !self.curr_meas.is_nan() {
            let index = self.recording_data.len(); // Assign index based on current length
            self.recording_data.push(super::Record {
                index,
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
                    .write_record(["Index", "Timestamp", "Unit", "Value"])
                    .expect("Failed to write CSV header");
                for record in &self.recording_data {
                    let timestamp_str = match self.recording_timestamp_format {
                        super::TimestampFormat::Rfc3339 => record.timestamp.to_rfc3339(),
                        super::TimestampFormat::Unix => record.timestamp.timestamp().to_string(),
                    };
                    writer
                        .write_record(&[
                            record.index.to_string(),
                            timestamp_str,
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
                let records: Vec<serde_json::Value> = self
                    .recording_data
                    .iter()
                    .map(|record| {
                        let timestamp_value = match self.recording_timestamp_format {
                            super::TimestampFormat::Rfc3339 => {
                                serde_json::Value::String(record.timestamp.to_rfc3339())
                            }
                            super::TimestampFormat::Unix => serde_json::Value::Number(
                                serde_json::Number::from(record.timestamp.timestamp()),
                            ),
                        };
                        serde_json::json!({
                            "index": record.index,
                            "timestamp": timestamp_value,
                            "unit": record.unit,
                            "value": record.value,
                        })
                    })
                    .collect();
                serde_json::to_writer(file, &records).expect("Failed to write JSON data");
            }
            super::RecordingFormat::Xlsx => {
                let workbook =
                    Workbook::new(&self.recording_file_path).expect("Failed to create XLSX file");
                let mut sheet = workbook
                    .add_worksheet(None)
                    .expect("Failed to add worksheet");
                sheet
                    .write_string(0, 0, "Index", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 1, "Timestamp", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 2, "Unit", None)
                    .expect("Fixed headers");
                sheet
                    .write_string(0, 3, "Value", None)
                    .expect("Failed to write XLSX header");
                for (i, record) in self.recording_data.iter().enumerate() {
                    sheet
                        .write_number((i + 1) as u32, 0, record.index as f64, None)
                        .expect("Failed to write XLSX record");
                    let timestamp_str = match self.recording_timestamp_format {
                        super::TimestampFormat::Rfc3339 => record.timestamp.to_rfc3339(),
                        super::TimestampFormat::Unix => record.timestamp.timestamp().to_string(),
                    };
                    sheet
                        .write_string((i + 1) as u32, 1, &timestamp_str, None)
                        .expect("Failed to write XLSX record");
                    sheet
                        .write_string((i + 1) as u32, 2, &record.unit, None)
                        .expect("Failed to write XLSX record");
                    sheet
                        .write_number((i + 1) as u32, 3, record.value, None)
                        .expect("Failed to write XLSX record");
                }
                workbook.close().expect("Failed to close XLSX workbook");
            }
        }
    }
}
