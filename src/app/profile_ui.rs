//! Profile combo on the connection row, and Save as….

use crate::profile::{self, StartupView};

impl super::MyApp {
    pub(super) fn apply_profile_startup(&mut self, ctx: &egui::Context) {
        let Some(startup) = profile::startup_profile() else {
            return;
        };
        match startup {
            StartupView::Picker { notice } => {
                self.profile_name.clear();
                self.connect_on_startup = false;
                let mut used_ports = profile::used_agent_ports();
                used_ports.push(self.agent_port);
                self.agent_port = profile::next_agent_port(&used_ports);
                self.profile_notice = notice;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("RustyMeter".to_owned()));
            }
            StartupView::Profile { display } => {
                self.profile_name = display.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "RustyMeter — {display}"
                )));
            }
        }
    }

    pub(super) fn show_profile_combo(&mut self, ui: &mut egui::Ui) {
        ui.label("Profile:");
        let connected = self.connection_state != super::ConnectionState::Disconnected;
        let current = self.profile_name.clone();
        let profiles = profile::list_profiles();
        let combo = ui.add_enabled_ui(!connected, |ui| {
            let selected = if current.is_empty() {
                "Select a profile".to_owned()
            } else {
                current.clone()
            };
            egui::ComboBox::from_id_salt("profile_picker")
                .selected_text(selected)
                .width(160.0)
                .show_ui(ui, |ui| {
                    for profile in &profiles {
                        let label = if profile.locked && profile.name != current {
                            format!("{} (open)", profile.name)
                        } else {
                            profile.name.clone()
                        };
                        if ui
                            .selectable_label(profile.name == current, label)
                            .clicked()
                            && profile.name != current
                        {
                            if profile.locked {
                                self.profile_notice =
                                    Some(format!("{} is open in another window.", profile.name));
                            } else {
                                self.request_profile_switch(profile.name.clone(), ui.ctx());
                            }
                        }
                    }
                });
        });
        combo.response.on_hover_text(if connected {
            "Disconnect to load another profile."
        } else {
            "Load a saved profile. Connect after it has loaded."
        });
        if ui
            .button("Save as…")
            .on_hover_text(
                "Save these settings under a new name. Nothing is sent to the instrument.",
            )
            .clicked()
        {
            self.save_as_open = true;
            self.save_as_error = None;
        }
        if ui
            .button("Manage…")
            .on_hover_text("Delete a profile that is not open. Default stays.")
            .clicked()
        {
            self.manage_open = true;
            self.manage_error = None;
            self.delete_confirm = None;
        }
    }

    pub(super) fn show_profile_notice(&self, ui: &mut egui::Ui) {
        if let Some(notice) = &self.profile_notice {
            ui.colored_label(egui::Color32::from_rgb(220, 160, 60), notice);
        }
    }

    pub(super) fn show_profile_manage(&mut self, ctx: &egui::Context) {
        if !self.manage_open {
            return;
        }
        let current = self.profile_name.clone();
        let profiles = profile::list_profiles();
        let confirm = self.delete_confirm.clone();
        egui::Window::new("Manage profiles")
            .auto_sized()
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Default is kept. An open profile cannot be deleted.");
                if let Some(err) = &self.manage_error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                }
                if profiles.is_empty() {
                    ui.label("No profiles yet.");
                }
                for profile in &profiles {
                    ui.horizontal(|ui| {
                        ui.label(&profile.name);
                        let is_default = profile.name.eq_ignore_ascii_case(profile::DEFAULT_NAME);
                        let is_current = profile.name == current;
                        let blocked = is_default || is_current || profile.locked;
                        if blocked {
                            let why = if is_default {
                                "Other windows copy Default when they open."
                            } else if is_current {
                                "This window is using it."
                            } else {
                                "It is open in another window."
                            };
                            ui.add_enabled(false, egui::Button::new("Delete"))
                                .on_hover_text(why);
                        } else if confirm.as_deref() == Some(profile.name.as_str()) {
                            if ui.button("Delete").clicked() {
                                match profile::delete_profile(&profile.name) {
                                    Ok(()) => {
                                        self.delete_confirm = None;
                                        self.manage_error = None;
                                        self.profile_notice =
                                            Some(format!("Deleted {}.", profile.name));
                                    }
                                    Err(err) => self.manage_error = Some(err),
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                self.delete_confirm = None;
                            }
                        } else if ui.button("Delete").clicked() {
                            self.delete_confirm = Some(profile.name.clone());
                            self.manage_error = None;
                        }
                    });
                }
                if ui.button("Close").clicked() {
                    self.manage_open = false;
                    self.delete_confirm = None;
                    self.manage_error = None;
                }
            });
    }

    pub(super) fn show_profile_save_as(&mut self, ctx: &egui::Context) {
        if !self.save_as_open {
            return;
        }
        let connected = self.connection_state != super::ConnectionState::Disconnected;
        egui::Window::new("Save profile")
            .auto_sized()
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Save the current settings as a new profile.");
                if connected {
                    ui.label("This window stays on its current profile until you disconnect.");
                }
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.save_as_name);
                });
                if let Some(err) = &self.save_as_error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.commit_save_as(ui.ctx());
                    }
                    if ui.button("Cancel").clicked() {
                        self.save_as_open = false;
                        self.save_as_error = None;
                    }
                });
            });
    }

    fn commit_save_as(&mut self, ctx: &egui::Context) {
        let name = self.save_as_name.trim().to_owned();
        let mut used_ports = profile::used_agent_ports();
        used_ports.push(self.agent_port);
        let port = profile::next_agent_port(&used_ports);
        let app_ron = match self.profile_export_ron(&name, port) {
            Ok(app_ron) => app_ron,
            Err(err) => {
                self.save_as_error = Some(err);
                return;
            }
        };
        if let Err(err) = profile::write_new_profile(&name, &app_ron) {
            self.save_as_error = Some(err);
            return;
        }
        self.save_as_open = false;
        self.save_as_name.clear();
        if self.connection_state == super::ConnectionState::Disconnected {
            self.request_profile_switch(name, ctx);
        } else {
            self.profile_notice = Some(format!("Saved as {name}. Disconnect to switch to it."));
        }
    }

    fn request_profile_switch(&mut self, name: String, ctx: &egui::Context) {
        if !profile::can_relaunch() {
            self.profile_notice = Some("Could not find the RustyMeter program to restart.".into());
            return;
        }
        self.relaunch_profile = Some(name);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Serialize this window as `name`, without keeping that change in memory.
    fn profile_export_ron(&mut self, name: &str, port: u16) -> Result<String, String> {
        if name.trim().is_empty() {
            return Err("Enter a profile name.".into());
        }
        if self.scpi_is_psu {
            self.remember_psu_setpoints();
        }
        let old_name = std::mem::replace(&mut self.profile_name, name.trim().to_owned());
        let old_port = self.agent_port;
        self.agent_port = port;
        let exported = ron::ser::to_string(self).map_err(|err| err.to_string());
        self.profile_name = old_name;
        self.agent_port = old_port;
        exported
    }

    pub(super) fn relaunch_profile_on_exit(&self) {
        if let Some(name) = &self.relaunch_profile
            && let Err(err) = profile::relaunch(name)
        {
            log::error!("could not open profile {name}: {err}");
        }
    }
}
