use std::{
    collections::VecDeque,
    fs::{create_dir_all, read_to_string},
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use arboard::Clipboard;
use downloader::{Download, Downloader};
use egui::{TextEdit, Vec2, Window};
use egui_dropdown::DropDownBox;
use egui_extras::{Column, TableBuilder};
use glob::glob;
use indexmap::{indexmap, IndexMap};
use mio::{Events, Interest, Poll, Token};
use mio_serial::{DataBits, SerialPort, SerialPortInfo};
use mio_serial::{SerialPortBuilderExt, SerialStream};
use regex::Regex;
use std::io;
use subprocess::Exec;
use tempdir::TempDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const SERIAL_TOKEN: Token = Token(0);

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct MyApp {
    serial_port: String,
    baud_rate: u32,
    bits: u32,
    stop_bits: u32,
    parity: bool,
    connect_on_startup: bool,
    portlist: VecDeque<String>,
    #[serde(skip)]
    poll: Poll,
    #[serde(skip)]
    events: Events,
    #[serde(skip)]
    serial: Option<SerialStream>,
    #[serde(skip)]
    device: String,
    #[serde(skip)]
    ports: Vec<SerialPortInfo>,
    #[serde(skip)]
    tempdir: Option<TempDir>,
    #[serde(skip)]
    settings_open: bool,
    #[serde(skip)]
    is_init: bool,
}

impl Default for MyApp {
    fn default() -> Self {
        Self {
            serial_port: "".to_owned(),
            baud_rate: 115200,
            bits: 8,
            stop_bits: 1,
            parity: false,
            connect_on_startup: false,
            portlist: VecDeque::with_capacity(11),
            poll: Poll::new().unwrap(), // if this does not work there's no point in running anyway
            events: Events::with_capacity(1),
            serial: None,
            device: "".to_owned(),
            ports: vec![],
            tempdir: TempDir::new("rustymeter").ok(),
            settings_open: false,
            is_init: false,
        }
    }
}

impl MyApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }

        Default::default()
    }
}

impl eframe::App for MyApp {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Put your widgets into a `SidePanel`, `TopBottomPanel`, `CentralPanel`, `Window` or `Area`.
        // For inspiration and more examples, go to https://emilk.github.io/egui
        let is_web = cfg!(target_arch = "wasm32");

        // on startup handle certain items once
        if !self.is_init {
            if let Ok(ports) = mio_serial::available_ports() {
                for p in ports {
                    self.portlist.push_front(p.port_name);
                }
            }
            self.is_init = true
        }

        if let Some(serial) = &mut self.serial {
            // Poll to check if we have serial events waiting for us.
            let res = self.poll.poll(&mut self.events, None);
            println!("Poll: {:?}", res);

            // buffer for read data
            let mut buf = vec![];

            // Process each event.
            for event in self.events.iter() {
                // Validate the token we registered our socket with,
                // in this example it will only ever be one but we
                // make sure it's valid none the less.
                match event.token() {
                    SERIAL_TOKEN => loop {
                        // In this loop we receive all packets queued for the socket.
                        match serial.read(&mut buf) {
                            Ok(count) => {
                                println!("{:?}", String::from_utf8_lossy(&buf[..count]));
                                self.device = String::from_utf8_lossy(&buf).to_string();
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                println!("WouldBlock escape");
                                break;
                            }
                            Err(e) => {
                                println!("Quitting due to read error: {}", e);
                                // return Err(e);
                            }
                        }
                    },
                    _ => {
                        // This should never happen as there is only one port open
                    }
                }
            }
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:

            egui::menu::bar(ui, |ui| {
                // NOTE: no File->Quit on web pages!
                ui.menu_button("File", |ui| {
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    if !is_web {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
                ui.add_space(16.0);

                egui::widgets::global_dark_light_mode_buttons(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // The central panel the region left after adding TopPanel's and SidePanel's
            if is_web {
                ui.heading("RustyMeter");
            }

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Serial port: ");
                    // ui.add(TextEdit::singleline(&mut self.part).desired_width(800.0));

                    ui.add(
                        DropDownBox::from_iter(
                            &self.portlist,
                            "portlistbox",
                            &mut self.serial_port,
                            |ui, text| ui.selectable_label(false, text),
                        )
                        .desired_width(800.0)
                        .select_on_focus(true)
                        .filter_by_input(false),
                    );

                    if ui.button("Connect").clicked() {
                        self.serial = mio_serial::new(&self.serial_port, self.baud_rate)
                            .open_native_async()
                            .ok();
                        if let Some(serial) = &mut self.serial {
                            let res = self.poll.registry().register(
                                serial,
                                SERIAL_TOKEN,
                                Interest::READABLE | Interest::WRITABLE,
                            );
                            println!("Registry: {:?}", res);
                            serial.set_data_bits(DataBits::Eight);
                            serial.set_stop_bits(mio_serial::StopBits::One);
                            serial.set_parity(mio_serial::Parity::None);
                            let res = serial.write("SYST:REM\n".as_bytes());
                            // println!("{:?}", res);
                            // let res = serial.flush();
                            // println!("{:?}", res);
                            //let res = serial.write("*IDN?\n".as_bytes());
                            // println!("*IDN?: {:?}", res);
                            let res = serial.write("MEAS?\n".as_bytes());
                            println!("MEAS?: {:?}", res);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if !self.device.is_empty() {
                        ui.label("Connected to: ");
                        ui.label(&self.device);
                    } else {
                        ui.label("Not connected.");
                    }
                });
            });

            ui.separator();

            ui.vertical(|ui| {
                ui.separator();

                ui.horizontal(|ui| {});
            });

            ui.separator();

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                powered_by(ui);
                ui.hyperlink_to(
                    format!("Version: v{VERSION}"),
                    format!("https://github.com/markusdd/RustyMeter/releases/tag/v{VERSION}"),
                );
                egui::warn_if_debug_build(ui);
            });

            //settings window
            if self.settings_open {
                Window::new("Settings")
                    .auto_sized()
                    .interactable(true)
                    .show(ctx, |ui| {
                        ui.vertical(|ui| {
                            ui.heading("Settings");
                            ui.checkbox(&mut self.connect_on_startup, "Connect on startup");
                            ui.checkbox(
                                &mut self.parity,
                                "Use parity bit (ignored right now, always None)",
                            );
                            ui.label("Baud rate:");
                            ui.add(
                                TextEdit::singleline(&mut self.baud_rate.to_string())
                                    .desired_width(800.0),
                            );
                            ui.label("Data Bits (ignored right now, always 8):");
                            ui.add(
                                TextEdit::singleline(&mut self.bits.to_string())
                                    .desired_width(800.0),
                            );
                            ui.label("Stop bits (ignored right now, always 1):");
                            ui.add(
                                TextEdit::singleline(&mut self.stop_bits.to_string())
                                    .desired_width(800.0),
                            );
                            if ui.button("Close").clicked() {
                                self.settings_open = false;
                            }
                        });
                    });
            }
        });
    }
}

fn powered_by(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to("egui", "https://github.com/emilk/egui");
        ui.label(", ");
        ui.hyperlink_to(
            "eframe",
            "https://github.com/emilk/egui/tree/master/crates/eframe",
        );
        ui.label(" and ");
        ui.hyperlink_to(
            "TheHWCave",
            "https://github.com/TheHWcave/OWON-XDM1041/tree/main",
        );
        ui.label(".");
    });
}
