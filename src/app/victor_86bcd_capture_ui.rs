//! Victor 86B/C/D serial labeled capture panel (DM1107).

use egui::{DragValue, RichText};

use crate::victor_86bcd_capture::{
    LcdDigit, LcdDisplay, Victor86bcdCaptureContext, Victor86bcdCaptureDpMode,
    Victor86bcdCaptureFunction, Victor86bcdCaptureJob, Victor86bcdCaptureUnit,
};

use super::MyApp;

impl MyApp {
    pub fn show_victor_86bcd_capture_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Victor 86B/C/D serial capture");
        ui.label(
            "DM1107 USB-serial meters only (not 86E ES51932 or legacy HID). \
             Describe what the meter shows, then record a raw byte window (9600 8N1). \
             Set each LCD digit: off (_), 0–9, or L. Decimal-after picks where the point sits \
             between digits (manual range = fixed position).",
        );

        ui.horizontal(|ui| {
            ui.label("Function:");
            let prev_function = self.victor_86bcd_capture_function;
            egui::ComboBox::from_id_salt("victor_86bcd_capture_function")
                .selected_text(self.victor_86bcd_capture_function.label())
                .show_ui(ui, |ui| {
                    for function in Victor86bcdCaptureFunction::all() {
                        ui.selectable_value(
                            &mut self.victor_86bcd_capture_function,
                            *function,
                            function.label(),
                        );
                    }
                });
            if self.victor_86bcd_capture_function != prev_function {
                self.victor_86bcd_capture_unit = self
                    .victor_86bcd_capture_function
                    .default_unit();
            }

            ui.label("Unit:");
            egui::ComboBox::from_id_salt("victor_86bcd_capture_unit")
                .selected_text(self.victor_86bcd_capture_unit.label())
                .show_ui(ui, |ui| {
                    for unit in Victor86bcdCaptureUnit::all() {
                        ui.selectable_value(
                            &mut self.victor_86bcd_capture_unit,
                            *unit,
                            unit.label(),
                        );
                    }
                });

            ui.label("Decimal:");
            egui::ComboBox::from_id_salt("victor_86bcd_capture_dp_mode")
                .selected_text(self.victor_86bcd_capture_dp_mode.label())
                .show_ui(ui, |ui| {
                    for mode in Victor86bcdCaptureDpMode::all() {
                        ui.selectable_value(
                            &mut self.victor_86bcd_capture_dp_mode,
                            *mode,
                            mode.label(),
                        );
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("LCD:");
            for (i, digit) in self.victor_86bcd_capture_digits.iter_mut().enumerate() {
                ui.label(format!("d{i}"));
                egui::ComboBox::from_id_salt(format!("victor_86bcd_capture_d{i}"))
                    .selected_text(digit.ui_label())
                    .width(52.0)
                    .show_ui(ui, |ui| {
                        for option in LcdDigit::all() {
                            ui.selectable_value(digit, *option, option.ui_label());
                        }
                    });
            }

            ui.label("DP after:");
            let mut dp_choice = self
                .victor_86bcd_capture_dp_after
                .map(|n| n + 1)
                .unwrap_or(0);
            egui::ComboBox::from_id_salt("victor_86bcd_capture_dp_after")
                .selected_text(match self.victor_86bcd_capture_dp_after {
                    None => "none",
                    Some(0) => "d0",
                    Some(1) => "d1",
                    Some(2) => "d2",
                    Some(_) => "?",
                })
                .width(64.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dp_choice, 0, "none");
                    ui.selectable_value(&mut dp_choice, 1, "d0");
                    ui.selectable_value(&mut dp_choice, 2, "d1");
                    ui.selectable_value(&mut dp_choice, 3, "d2");
                });
            self.victor_86bcd_capture_dp_after = match dp_choice {
                0 => None,
                n => Some((n - 1) as u8),
            };

            let preview = LcdDisplay {
                digits: self.victor_86bcd_capture_digits,
                dp_after: self.victor_86bcd_capture_dp_after,
            };
            ui.label(RichText::new(format!("→ {}", preview.format())).monospace());
        });

        ui.horizontal(|ui| {
            ui.label("Notes:");
            ui.add(
                egui::TextEdit::singleline(&mut self.victor_86bcd_capture_notes)
                    .desired_width(280.0)
                    .hint_text("REL, MAX/MIN — optional"),
            );
        });

        ui.horizontal(|ui| {
            ui.label("Record duration:");
            ui.add(
                DragValue::new(&mut self.victor_86bcd_capture_duration_ms)
                    .range(50..=10_000)
                    .speed(50)
                    .suffix(" ms"),
            );
            ui.label("of raw serial (click to type)");
        });

        let display = LcdDisplay {
            digits: self.victor_86bcd_capture_digits,
            dp_after: self.victor_86bcd_capture_dp_after,
        };
        let can_capture = self.connection_state == super::ConnectionState::Connected
            && self.victor_86bcd_capture_tx.is_some()
            && !display.is_empty();

        if ui
            .add_enabled(can_capture, egui::Button::new("Capture sample"))
            .on_hover_text("Append one CSV row: d0–d3, dp_after, and raw bytes in the window")
            .clicked()
        {
            if let Some(tx) = &self.victor_86bcd_capture_tx {
                let job = Victor86bcdCaptureJob {
                    context: Victor86bcdCaptureContext {
                        function: self.victor_86bcd_capture_function,
                        unit: self.victor_86bcd_capture_unit,
                        dp_mode: self.victor_86bcd_capture_dp_mode,
                        display,
                        notes: self.victor_86bcd_capture_notes.clone(),
                    },
                    duration_ms: self.victor_86bcd_capture_duration_ms,
                };
                if tx.try_send(job).is_err() {
                    let mut st = self.victor_86bcd_capture_status_shared.lock().unwrap();
                    st.message = "Capture busy — wait for current recording".to_owned();
                }
            }
        }

        let status = self.victor_86bcd_capture_status_shared.lock().unwrap();
        if !status.message.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(&status.message).small());
        }
        ui.label(
            RichText::new(format!(
                "CSV: {}  |  explore: cargo run --bin victor-86bcd-explore",
                crate::victor_86bcd_capture::default_samples_path().display()
            ))
            .small()
            .weak(),
        );
    }
}