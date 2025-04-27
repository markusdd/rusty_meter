use std::fs::File;

use csv::WriterBuilder;
use egui::{
    Context, FontId, RichText, ScrollArea, TextEdit, Ui, ViewportBuilder, ViewportCommand,
    ViewportId,
};
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
                            {
                                if ui.button("Record Now").clicked() {
                                    self.record_measurement();
                                }
                            }

                            // Data table
                            ui.separator();
                            ui.label("Recorded Data:");
                            ScrollArea::vertical()
                                .max_height(ui.available_height() - 50.0)
                                .show(ui, |ui| {
                                    self.render_data_table(ui);
                                });

                            // Spacer and buttons
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
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

    fn render_data_table(&mut self, ui: &mut Ui) {
        // Define table with fixed column widths
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(100.0).at_least(50.0))
            .column(Column::initial(200.0).at_least(100.0))
            .column(Column::initial(100.0).at_least(50.0))
            .column(Column::initial(100.0).at_least(50.0));

        table
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.label(RichText::new("Index").font(FontId::proportional(16.0)));
                });
                header.col(|ui| {
                    ui.label(RichText::new("Timestamp").font(FontId::proportional(16.0)));
                });
                header.col(|ui| {
                    ui.label(RichText::new("Unit").font(FontId::proportional(16.0)));
                });
                header.col(|ui| {
                    ui.label(RichText::new("Value").font(FontId::proportional(16.0)));
                });
            })
            .body(|mut body| {
                for (_row_index, record) in self.recording_data.iter().enumerate() {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            ui.label(format!("{}", record.index));
                        });
                        row.col(|ui| {
                            ui.label(record.timestamp.to_rfc3339());
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
                    .write_record(&["Index", "Timestamp", "Unit", "Value"])
                    .expect("Failed to write CSV header");
                for record in &self.recording_data {
                    writer
                        .write_record(&[
                            record.index.to_string(),
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
                    .write_string(0, 0, "Index", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 1, "Timestamp", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 2, "Unit", None)
                    .expect("Failed to write XLSX header");
                sheet
                    .write_string(0, 3, "Value", None)
                    .expect("Failed to write XLSX header");
                for (i, record) in self.recording_data.iter().enumerate() {
                    sheet
                        .write_number((i + 1) as u32, 0, record.index as f64, None)
                        .expect("Failed to write XLSX record");
                    sheet
                        .write_string((i + 1) as u32, 1, &record.timestamp.to_rfc3339(), None)
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
