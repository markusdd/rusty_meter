use crate::scpi_macro::{MacroTarget, ScpiMacro, idn_model};

use egui::{Context, FontId, TextEdit, Window};

impl super::MyApp {
    pub fn show_macros(&mut self, ctx: &Context) {
        if !self.macros_open {
            return;
        }

        let live_idn = self.device.lock().unwrap().clone();
        let live_model = idn_model(&live_idn);
        let connected_scpi = self.connection_state == super::ConnectionState::Connected
            && self.connection_type == super::ConnectionType::ScpiSerial
            && self.serial_tx.is_some();

        Window::new("SCPI macros")
            .default_size([780.0, 460.0])
            .min_size([560.0, 280.0])
            .resizable(true)
            .vscroll(false)
            .show(ctx, |ui| {
                egui::Panel::bottom("scpi_macro_footer").show(ui, |ui| {
                    if ui.button("Close").clicked() {
                        self.macros_open = false;
                    }
                });

                egui::Panel::top("scpi_macro_toolbar").show(ui, |ui| {
                    ui.label(
                        "Sequences of SCPI commands. A connect macro runs after the built-in \
                         settings bootstrap for the detected meter. Button macros appear below \
                         the mode buttons on the main window.",
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Add").clicked() {
                            let mut m = ScpiMacro::new("New macro");
                            m.applies_to = self.default_macro_target();
                            self.selected_macro_id = Some(m.id.clone());
                            self.scpi_macros.push(m);
                        }
                        let can_dup = self.selected_macro_id.is_some();
                        if ui
                            .add_enabled(can_dup, egui::Button::new("Duplicate"))
                            .clicked()
                            && let Some(id) = self.selected_macro_id.clone()
                        {
                            if let Some(src) = self.scpi_macros.iter().find(|m| m.id == id).cloned()
                            {
                                let mut copy = src;
                                copy.id = crate::scpi_macro::new_macro_id();
                                copy.name = format!("{} copy", copy.name);
                                self.selected_macro_id = Some(copy.id.clone());
                                self.scpi_macros.push(copy);
                            }
                        }
                        if ui
                            .add_enabled(can_dup, egui::Button::new("Delete"))
                            .clicked()
                            && let Some(id) = self.selected_macro_id.clone()
                        {
                            self.scpi_macros.retain(|m| m.id != id);
                            self.selected_macro_id = self.scpi_macros.first().map(|m| m.id.clone());
                        }
                        ui.separator();
                        if ui
                            .add_enabled(can_dup, egui::Button::new("Move up"))
                            .clicked()
                        {
                            self.move_selected_macro(-1);
                        }
                        if ui
                            .add_enabled(can_dup, egui::Button::new("Move down"))
                            .clicked()
                        {
                            self.move_selected_macro(1);
                        }
                    });
                });

                egui::Panel::left("scpi_macro_list")
                    .exact_size(240.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        ui.label("Macros");
                        egui::ScrollArea::vertical()
                            .id_salt("macro_list")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut pick: Option<String> = None;
                                for m in &self.scpi_macros {
                                    let mut label = m.name.clone();
                                    if m.run_on_connect {
                                        label.push_str("  [connect]");
                                    }
                                    if m.show_as_button {
                                        label.push_str("  [button]");
                                    }
                                    let selected =
                                        self.selected_macro_id.as_deref() == Some(m.id.as_str());
                                    if ui.selectable_label(selected, label).clicked() {
                                        pick = Some(m.id.clone());
                                    }
                                }
                                if let Some(id) = pick {
                                    self.selected_macro_id = Some(id);
                                }
                            });
                    });

                egui::CentralPanel::default().show(ui, |ui| {
                    let Some(sel_id) = self.selected_macro_id.clone() else {
                        ui.label("Add a macro, or select one from the list.");
                        return;
                    };
                    let Some(idx) = self.scpi_macros.iter().position(|m| m.id == sel_id) else {
                        return;
                    };

                    egui::ScrollArea::vertical()
                        .id_salt("macro_editor")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            {
                                let m = &mut self.scpi_macros[idx];

                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    ui.add(TextEdit::singleline(&mut m.name).desired_width(280.0));
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Applies to:");
                                    let current_label = m.applies_to.label();
                                    egui::ComboBox::from_id_salt("macro_target")
                                        .selected_text(current_label)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    matches!(m.applies_to, MacroTarget::AllScpi),
                                                    "All SCPI meters",
                                                )
                                                .clicked()
                                            {
                                                m.applies_to = MacroTarget::AllScpi;
                                            }
                                            if ui
                                                .selectable_label(
                                                    matches!(m.applies_to, MacroTarget::OwonMeas),
                                                    "Owon XDM (MEAS-era)",
                                                )
                                                .clicked()
                                            {
                                                m.applies_to = MacroTarget::OwonMeas;
                                            }
                                            if ui
                                                .selectable_label(
                                                    matches!(
                                                        m.applies_to,
                                                        MacroTarget::OwonXdm6000
                                                    ),
                                                    "Owon XDM 6000",
                                                )
                                                .clicked()
                                            {
                                                m.applies_to = MacroTarget::OwonXdm6000;
                                            }
                                            let this_label = if live_model.is_empty() {
                                                "This meter (connect to set model)".to_owned()
                                            } else {
                                                format!("This meter ({live_model})")
                                            };
                                            if ui
                                                .add_enabled(
                                                    !live_model.is_empty(),
                                                    egui::Button::selectable(
                                                        matches!(
                                                            m.applies_to,
                                                            MacroTarget::Model(_)
                                                        ),
                                                        this_label,
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                m.applies_to =
                                                    MacroTarget::Model(live_model.clone());
                                            }
                                            if ui
                                                .selectable_label(
                                                    matches!(
                                                        m.applies_to,
                                                        MacroTarget::IdnContains(_)
                                                    ),
                                                    "Custom IDN substring",
                                                )
                                                .clicked()
                                                && !matches!(
                                                    m.applies_to,
                                                    MacroTarget::IdnContains(_)
                                                )
                                            {
                                                m.applies_to =
                                                    MacroTarget::IdnContains(String::new());
                                            }
                                        });
                                });
                                if let MacroTarget::IdnContains(ref mut needle) = m.applies_to {
                                    ui.horizontal(|ui| {
                                        ui.label("IDN contains:");
                                        ui.add(TextEdit::singleline(needle).desired_width(280.0));
                                    });
                                }

                                ui.checkbox(
                                    &mut m.run_on_connect,
                                    "Run on connect (after settings bootstrap)",
                                );
                                ui.checkbox(
                                    &mut m.show_as_button,
                                    "Show as button on main window",
                                );

                                ui.label(
                                    "SCPI (one command per line; ';' also splits). '#' or '//' comments. Queries are ignored.",
                                );
                                ui.add(
                                    TextEdit::multiline(&mut m.body)
                                        .font(FontId::monospace(13.0))
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(12),
                                );
                            }

                            ui.horizontal(|ui| {
                                if ui.button("Insert current setup").clicked() {
                                    let snippet = self.current_setup_scpi();
                                    if let Some(m) =
                                        self.scpi_macros.iter_mut().find(|m| m.id == sel_id)
                                    {
                                        if !m.body.is_empty() && !m.body.ends_with('\n') {
                                            m.body.push('\n');
                                        }
                                        m.body.push_str(&snippet);
                                    }
                                }
                                if connected_scpi && ui.button("Run now").clicked() {
                                    if let Some(body) = self
                                        .scpi_macros
                                        .iter()
                                        .find(|m| m.id == sel_id)
                                        .map(|m| m.body.clone())
                                    {
                                        self.run_macro_body(&body, false);
                                    }
                                }
                            });
                        });
                });
            });
    }

    fn move_selected_macro(&mut self, delta: isize) {
        let Some(id) = self.selected_macro_id.as_ref() else {
            return;
        };
        let Some(idx) = self.scpi_macros.iter().position(|m| m.id == *id) else {
            return;
        };
        let new_idx = idx as isize + delta;
        if new_idx < 0 || new_idx >= self.scpi_macros.len() as isize {
            return;
        }
        self.scpi_macros.swap(idx, new_idx as usize);
    }
}
