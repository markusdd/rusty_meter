use std::fs::File;
use std::path::{Path, PathBuf};

use csv::WriterBuilder;
use egui::{FontId, RichText, TextEdit, ViewportBuilder, ViewportId};
use egui_extras::{Column, TableBuilder};
use rfd::FileDialog;
use xlsxwriter::Workbook;

impl super::MyApp {
    pub fn show_recording_window(&mut self, ui: &mut egui::Ui) {
        if self.recording_open {
            let viewport_id = ViewportId::from_hash_of("recording_viewport");

            ui.ctx().show_viewport_immediate(
                viewport_id,
                ViewportBuilder::default()
                    .with_title("Data Recording")
                    .with_inner_size([600.0, 400.0])
                    .with_resizable(true),
                |ui, class| {
                    assert!(
                        class == egui::ViewportClass::Immediate,
                        "This example is only intended to run as an immediate viewport"
                    );

                    egui::CentralPanel::default().show(ui, |ui| {
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
                                    let new_extension = match self.recording_format {
                                        super::RecordingFormat::Csv => "csv",
                                        super::RecordingFormat::Json => "json",
                                        super::RecordingFormat::Xlsx => "xlsx",
                                    };
                                    // Preserve the parent path and use platform-specific separators
                                    let new_path = if let Some(parent) = path.parent() {
                                        let mut new_path = PathBuf::from(parent);
                                        new_path.push(format!("{}.{}", stem, new_extension));
                                        new_path
                                    } else {
                                        PathBuf::from(format!("{}.{}", stem, new_extension))
                                    };
                                    self.recording_file_path =
                                        new_path.to_string_lossy().into_owned();
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
                            let (ch_names, st_names) = recording_column_names(&self.recording_data);
                            let legacy = is_legacy_layout(&self.recording_data);
                            let extra = if legacy {
                                2
                            } else {
                                ch_names.len() + st_names.len()
                            };
                            let mut table = TableBuilder::new(ui)
                                .striped(true)
                                .resizable(true)
                                .vscroll(true)
                                .stick_to_bottom(true)
                                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                .column(Column::initial(80.0).at_least(50.0))
                                .column(Column::initial(200.0).at_least(100.0));
                            for _ in 0..extra {
                                table = table.column(Column::initial(90.0).at_least(50.0));
                            }
                            let headers = preview_headers(legacy, &ch_names, &st_names);
                            table
                                .header(20.0, |mut header| {
                                    for title in &headers {
                                        header.col(|ui| {
                                            ui.label(
                                                RichText::new(title.as_str())
                                                    .font(FontId::proportional(16.0)),
                                            );
                                        });
                                    }
                                })
                                .body(|mut body| {
                                    for record in self.recording_data.iter() {
                                        let ts = match self.recording_timestamp_format {
                                            super::TimestampFormat::Rfc3339 => {
                                                record.timestamp.to_rfc3339()
                                            }
                                            super::TimestampFormat::Unix => {
                                                record.timestamp.timestamp().to_string()
                                            }
                                        };
                                        let cells = preview_cells(
                                            record, legacy, &ch_names, &st_names, &ts,
                                        );
                                        body.row(20.0, |mut row| {
                                            for cell in &cells {
                                                row.col(|ui| {
                                                    ui.label(cell.as_str());
                                                });
                                            }
                                        });
                                    }
                                });
                        });
                    });

                    // Handle close request (e.g., window close button)
                    if ui.ctx().input(|i| i.viewport().close_requested()) {
                        if self.recording_active {
                            self.recording_active = false;
                            self.save_recording_data();
                        }
                        self.recording_open = false;
                    }

                    // Request repaint to keep the viewport alive
                    ui.ctx().request_repaint();
                },
            );
        }
    }

    pub fn record_measurement(&mut self) {
        let Some((channels, status)) = self.recording_snapshot() else {
            return;
        };
        let index = self.recording_data.len();
        self.recording_data.push(super::Record {
            index,
            timestamp: chrono::Utc::now(),
            channels,
            status,
        });
    }

    fn recording_snapshot(&self) -> Option<(Vec<super::RecordChannel>, Vec<super::RecordStatus>)> {
        if self.scpi_is_psu {
            let v = self.psu.meas_v;
            let i = self.psu.meas_i;
            let p = self.psu.meas_p;
            if !v.is_finite() && !i.is_finite() && !p.is_finite() {
                return None;
            }
            let bit = |on: bool| if on { "1" } else { "0" };
            Some((
                vec![
                    record_channel("V", "V", v),
                    record_channel("I", "A", i),
                    record_channel("P", "W", p),
                ],
                vec![
                    record_status("output", if self.psu.output_on { "ON" } else { "OFF" }),
                    record_status("mode", self.psu.run.label()),
                    record_status("ovp", bit(self.psu.ovp_fault)),
                    record_status("ocp", bit(self.psu.ocp_fault)),
                    record_status("otp", bit(self.psu.otp_fault)),
                ],
            ))
        } else {
            if !self.curr_meas.is_finite() {
                return None;
            }
            Some((
                vec![record_channel("value", &self.curr_unit, self.curr_meas)],
                Vec::new(),
            ))
        }
    }

    pub fn save_recording_data(&self) {
        if self.recording_data.is_empty() || self.recording_file_path.is_empty() {
            return;
        }

        let (ch_names, st_names) = recording_column_names(&self.recording_data);
        let legacy = is_legacy_layout(&self.recording_data);
        let headers = export_headers(legacy, &ch_names, &st_names);

        match self.recording_format {
            super::RecordingFormat::Csv => {
                let file =
                    File::create(&self.recording_file_path).expect("Failed to create CSV file");
                let mut writer = WriterBuilder::new().from_writer(file);
                writer
                    .write_record(&headers)
                    .expect("Failed to write CSV header");
                for record in &self.recording_data {
                    let ts = self.format_record_timestamp(record);
                    let cells = export_cells(record, legacy, &ch_names, &st_names, &ts);
                    writer
                        .write_record(&cells)
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
                        record_json(record, timestamp_value, legacy)
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
                for (col, title) in headers.iter().enumerate() {
                    sheet
                        .write_string(0, col as u16, title, None)
                        .expect("Failed to write XLSX header");
                }
                for (i, record) in self.recording_data.iter().enumerate() {
                    let ts = self.format_record_timestamp(record);
                    let cells = export_cells(record, legacy, &ch_names, &st_names, &ts);
                    let row = (i + 1) as u32;
                    for (col, cell) in cells.iter().enumerate() {
                        let col = col as u16;
                        if col == 0 {
                            sheet
                                .write_number(row, col, record.index as f64, None)
                                .expect("Failed to write XLSX record");
                        } else if let Ok(n) = cell.parse::<f64>()
                            && n.is_finite()
                        {
                            sheet
                                .write_number(row, col, n, None)
                                .expect("Failed to write XLSX record");
                        } else {
                            sheet
                                .write_string(row, col, cell, None)
                                .expect("Failed to write XLSX record");
                        }
                    }
                }
                workbook.close().expect("Failed to close XLSX workbook");
            }
        }
    }

    fn format_record_timestamp(&self, record: &super::Record) -> String {
        match self.recording_timestamp_format {
            super::TimestampFormat::Rfc3339 => record.timestamp.to_rfc3339(),
            super::TimestampFormat::Unix => record.timestamp.timestamp().to_string(),
        }
    }
}

fn record_channel(name: &str, unit: &str, value: f64) -> super::RecordChannel {
    super::RecordChannel {
        name: name.to_owned(),
        unit: unit.to_owned(),
        value,
    }
}

fn record_status(name: &str, value: &str) -> super::RecordStatus {
    super::RecordStatus {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn is_legacy_layout(data: &[super::Record]) -> bool {
    !data.is_empty()
        && data
            .iter()
            .all(|r| r.channels.len() == 1 && r.status.is_empty())
}

fn recording_column_names(data: &[super::Record]) -> (Vec<String>, Vec<String>) {
    let mut channels = Vec::new();
    let mut status = Vec::new();
    for record in data {
        for ch in &record.channels {
            if !channels.iter().any(|n| n == &ch.name) {
                channels.push(ch.name.clone());
            }
        }
        for st in &record.status {
            if !status.iter().any(|n| n == &st.name) {
                status.push(st.name.clone());
            }
        }
    }
    (channels, status)
}

fn export_headers(legacy: bool, channels: &[String], status: &[String]) -> Vec<String> {
    let mut headers = vec!["Index".to_owned(), "Timestamp".to_owned()];
    if legacy {
        headers.push("Unit".to_owned());
        headers.push("Value".to_owned());
        return headers;
    }
    for name in channels {
        headers.push(name.clone());
        headers.push(format!("{name}_unit"));
    }
    headers.extend(status.iter().cloned());
    headers
}

fn preview_headers(legacy: bool, channels: &[String], status: &[String]) -> Vec<String> {
    let mut headers = vec!["Index".to_owned(), "Timestamp".to_owned()];
    if legacy {
        headers.push("Unit".to_owned());
        headers.push("Value".to_owned());
        return headers;
    }
    headers.extend(channels.iter().cloned());
    headers.extend(status.iter().cloned());
    headers
}

fn format_num(v: f64) -> String {
    if v.is_finite() {
        v.to_string()
    } else {
        String::new()
    }
}

fn channel_by_name<'a>(record: &'a super::Record, name: &str) -> Option<&'a super::RecordChannel> {
    record.channels.iter().find(|c| c.name == name)
}

fn status_by_name<'a>(record: &'a super::Record, name: &str) -> Option<&'a super::RecordStatus> {
    record.status.iter().find(|s| s.name == name)
}

fn export_cells(
    record: &super::Record,
    legacy: bool,
    channels: &[String],
    status: &[String],
    timestamp: &str,
) -> Vec<String> {
    let mut cells = vec![record.index.to_string(), timestamp.to_owned()];
    if legacy {
        let ch = &record.channels[0];
        cells.push(ch.unit.clone());
        cells.push(format_num(ch.value));
        return cells;
    }
    for name in channels {
        match channel_by_name(record, name) {
            Some(ch) => {
                cells.push(format_num(ch.value));
                cells.push(ch.unit.clone());
            }
            None => {
                cells.push(String::new());
                cells.push(String::new());
            }
        }
    }
    for name in status {
        cells.push(
            status_by_name(record, name)
                .map(|s| s.value.clone())
                .unwrap_or_default(),
        );
    }
    cells
}

fn preview_cells(
    record: &super::Record,
    legacy: bool,
    channels: &[String],
    status: &[String],
    timestamp: &str,
) -> Vec<String> {
    let mut cells = vec![record.index.to_string(), timestamp.to_owned()];
    if legacy {
        let ch = &record.channels[0];
        cells.push(ch.unit.clone());
        cells.push(format!("{:.4}", ch.value));
        return cells;
    }
    for name in channels {
        cells.push(
            channel_by_name(record, name)
                .map(|ch| {
                    if ch.value.is_finite() {
                        format!("{:.4}", ch.value)
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default(),
        );
    }
    for name in status {
        cells.push(
            status_by_name(record, name)
                .map(|s| s.value.clone())
                .unwrap_or_default(),
        );
    }
    cells
}

fn json_number(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

fn record_json(
    record: &super::Record,
    timestamp: serde_json::Value,
    legacy: bool,
) -> serde_json::Value {
    if legacy {
        let ch = &record.channels[0];
        return serde_json::json!({
            "index": record.index,
            "timestamp": timestamp,
            "unit": ch.unit,
            "value": json_number(ch.value),
        });
    }
    let channels: Vec<serde_json::Value> = record
        .channels
        .iter()
        .map(|ch| {
            serde_json::json!({
                "name": ch.name,
                "unit": ch.unit,
                "value": json_number(ch.value),
            })
        })
        .collect();
    let status: Vec<serde_json::Value> = record
        .status
        .iter()
        .map(|st| {
            serde_json::json!({
                "name": st.name,
                "value": st.value,
            })
        })
        .collect();
    serde_json::json!({
        "index": record.index,
        "timestamp": timestamp,
        "channels": channels,
        "status": status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_ts() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()
    }

    fn dmm_record() -> super::super::Record {
        super::super::Record {
            index: 0,
            timestamp: sample_ts(),
            channels: vec![record_channel("value", "VDC", 5.02681)],
            status: vec![],
        }
    }

    fn psu_record() -> super::super::Record {
        super::super::Record {
            index: 1,
            timestamp: sample_ts(),
            channels: vec![
                record_channel("V", "V", 12.0),
                record_channel("I", "A", 0.118),
                record_channel("P", "W", 1.419),
            ],
            status: vec![
                record_status("output", "ON"),
                record_status("mode", "CV"),
                record_status("ovp", "0"),
                record_status("ocp", "0"),
                record_status("otp", "0"),
            ],
        }
    }

    #[test]
    fn dmm_export_keeps_unit_value_columns() {
        let data = vec![dmm_record()];
        let (ch, st) = recording_column_names(&data);
        assert!(is_legacy_layout(&data));
        assert_eq!(
            export_headers(true, &ch, &st),
            ["Index", "Timestamp", "Unit", "Value"]
        );
        let cells = export_cells(&data[0], true, &ch, &st, "TS");
        assert_eq!(cells, ["0", "TS", "VDC", "5.02681"]);
    }

    #[test]
    fn psu_export_has_channels_and_status() {
        let data = vec![psu_record()];
        let (ch, st) = recording_column_names(&data);
        assert!(!is_legacy_layout(&data));
        assert_eq!(ch, ["V", "I", "P"]);
        assert_eq!(st, ["output", "mode", "ovp", "ocp", "otp"]);
        let headers = export_headers(false, &ch, &st);
        assert_eq!(
            headers,
            [
                "Index",
                "Timestamp",
                "V",
                "V_unit",
                "I",
                "I_unit",
                "P",
                "P_unit",
                "output",
                "mode",
                "ovp",
                "ocp",
                "otp",
            ]
        );
        let cells = export_cells(&data[0], false, &ch, &st, "TS");
        assert_eq!(
            cells,
            [
                "1", "TS", "12", "V", "0.118", "A", "1.419", "W", "ON", "CV", "0", "0", "0"
            ]
        );
    }

    #[test]
    fn dmm_json_stays_flat() {
        let v = record_json(&dmm_record(), serde_json::json!("TS"), true);
        assert_eq!(v["unit"], "VDC");
        assert!(v.get("channels").is_none());
    }

    #[test]
    fn psu_json_lists_channels_and_status() {
        let v = record_json(&psu_record(), serde_json::json!("TS"), false);
        assert_eq!(v["channels"].as_array().unwrap().len(), 3);
        assert_eq!(v["status"].as_array().unwrap().len(), 5);
        assert_eq!(v["channels"][1]["name"], "I");
        assert_eq!(v["status"][1]["value"], "CV");
    }
}
