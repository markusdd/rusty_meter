use egui::{color_picker::color_picker_color32, Context, TextEdit, Window};

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
                        ui.label("Maximum histogram memory depth:");
                        let mut hist_max_depth_str = self.hist_mem_depth_max.to_string();
                        if ui
                            .add(
                                TextEdit::singleline(&mut hist_max_depth_str)
                                    .desired_width(800.0)
                                    .hint_text("Enter maximum number of values for histogram"),
                            )
                            .changed()
                        {
                            if let Ok(new_max_depth) = hist_max_depth_str.parse::<usize>() {
                                if new_max_depth >= 100 {
                                    // Ensure minimum is at least 100
                                    self.hist_mem_depth_max = new_max_depth;
                                    // Clamp hist_mem_depth to new max if necessary
                                    if self.hist_mem_depth > self.hist_mem_depth_max {
                                        self.hist_mem_depth = self.hist_mem_depth_max;
                                        while self.hist_values.len() > self.hist_mem_depth {
                                            self.hist_values.pop_front();
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
                                ui.label("Histogram bar color:");
                                color_picker_color32(
                                    ui,
                                    &mut self.hist_bar_color,
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
}