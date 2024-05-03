use std::{
    array,
    collections::{BTreeMap, VecDeque},
    f64::NAN,
    fs::{create_dir_all, read_to_string},
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use arboard::Clipboard;
use downloader::{Download, Downloader};
use egui::{Context, FontData, FontDefinitions, FontFamily, FontId, TextEdit, Vec2, Window};
use egui_dropdown::DropDownBox;
use egui_extras::{Column, TableBuilder};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use glob::glob;
use indexmap::{indexmap, IndexMap};
use mio::{Events, Interest, Poll, Token};
use mio_serial::{DataBits, SerialPort, SerialPortInfo};
use mio_serial::{SerialPortBuilderExt, SerialStream};
use regex::Regex;
use std::io;
use subprocess::Exec;
use tempdir::TempDir;
use tokio::spawn;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const SERIAL_TOKEN: Token = Token(0);

const MEM_DEPTH_DEFAULT: usize = 500;

enum ScpiMode {
    IDN,
    CONF,
    SYST,
    MEAS,
}

#[derive(PartialEq)]
enum MeterMode {
    VDC,
    VAC,
    ADC,
    AAC,
    RES,
    CAP,
    FREQ,
    PER,
    DIOD,
    CONT,
    TEMP,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct MyApp {
    serial_port: String,
    baud_rate: u32,
    bits: u32,
    stop_bits: u32,
    parity: bool,
    mem_depth: usize,
    connect_on_startup: bool,
    #[serde(skip)]
    metermode: MeterMode,
    #[serde(skip)]
    scpimode: ScpiMode,
    #[serde(skip)]
    confstring: String,
    #[serde(skip)]
    curr_meas: f64,
    #[serde(skip)]
    curr_unit: String,
    #[serde(skip)]
    issue_new_write: bool,
    #[serde(skip)]
    readbuf: [u8; 1024],
    #[serde(skip)]
    portlist: VecDeque<String>,
    #[serde(skip)]
    values: VecDeque<f64>,
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
            mem_depth: MEM_DEPTH_DEFAULT,
            connect_on_startup: false,
            metermode: MeterMode::VDC,
            scpimode: ScpiMode::IDN,
            confstring: "".to_owned(),
            curr_meas: NAN,
            curr_unit: "VDC".to_owned(),
            issue_new_write: false,
            readbuf: [0u8; 1024],
            portlist: VecDeque::with_capacity(11),
            values: VecDeque::with_capacity(MEM_DEPTH_DEFAULT + 1),
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

        let mut fonts = FontDefinitions::default();

        fonts.font_data.insert(
            "B612Mono-Bold".to_owned(),
            FontData::from_static(include_bytes!("../assets/fonts/B612Mono-Bold.ttf")),
        );

        let mut newfam = BTreeMap::new();
        newfam.insert(
            FontFamily::Name("B612Mono-Bold".into()),
            vec!["B612Mono-Bold".to_owned()],
        );
        fonts.families.append(&mut newfam);

        cc.egui_ctx.set_fonts(fonts);

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }

        Default::default()
    }

    fn dispatch_serial_comms(ctx: Context) {
        println!("Hi in dispatch fn! Serial port!");
        tokio::spawn(async move {
            loop {
                // TODO this is the simple stupid approach
                // we should only request repaint if the last value has changed
                // from the previous one
                tokio::time::sleep(Duration::from_millis(10)).await;
                ctx.request_repaint();
            }
        });
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
            let _ = self
                .poll
                .poll(&mut self.events, Some(Duration::from_millis(1)));
            //println!("Poll: {:?}", res);

            if self.issue_new_write {
                let sendstring;
                match self.scpimode {
                    ScpiMode::IDN => sendstring = "*IDN?\n",
                    ScpiMode::SYST => sendstring = "SYST:REM\n",
                    ScpiMode::CONF => sendstring = &self.confstring, // TODO update UI only when sent successfully
                    ScpiMode::MEAS => sendstring = "MEAS?\n",
                }
                let res = serial.write_all(&sendstring.as_bytes());
                if res.is_ok() {
                    match self.scpimode {
                        ScpiMode::SYST => {
                            self.scpimode = ScpiMode::MEAS;
                            // write only command with no return data
                            // go straight to next write
                            self.issue_new_write = true;
                        }
                        ScpiMode::CONF => {
                            self.scpimode = ScpiMode::MEAS;
                            self.confstring = "".to_owned();
                            // write only command with no return data
                            // go straight to next write
                            self.issue_new_write = true;
                        }
                        _ => {
                            // await read data first
                            self.issue_new_write = false;
                        }
                    }
                }
            }

            // Process each event.
            for event in self.events.iter() {
                // Validate the token we registered our socket with,
                // in this example it will only ever be one but we
                // make sure it's valid none the less.
                match event.token() {
                    SERIAL_TOKEN => loop {
                        // In this loop we receive all packets queued for the socket.
                        match serial.read(&mut self.readbuf) {
                            Ok(count) => {
                                //println!("Count read: {:?}", count);
                                let content = String::from_utf8_lossy(&self.readbuf[..count]);
                                println!("{:?}", content);
                                // do not send a new request until we have the result of the old one
                                // OWON terminates everything with \r\n
                                if content.ends_with("\r\n") {
                                    self.issue_new_write = true;
                                    match self.scpimode {
                                        ScpiMode::IDN => {
                                            // Device ID string received, save it for UI
                                            // and move on to SYST mode
                                            self.device = content.trim_end().to_owned();
                                            self.scpimode = ScpiMode::SYST;
                                        }
                                        ScpiMode::SYST => {
                                            // no read data, SYST commands await no response
                                            // if anything came we just ignore it
                                            // change to measurement mode right after
                                            self.scpimode = ScpiMode::MEAS;
                                        }
                                        ScpiMode::CONF => {
                                            // see SYST, but if we have an outstanding conf
                                            // string we stay in that state for write to handle it
                                            if !self.confstring.is_empty() {
                                                self.scpimode = ScpiMode::MEAS;
                                            }
                                        }
                                        ScpiMode::MEAS => {
                                            // measurement value mode, store if we got something new
                                            self.curr_meas =
                                                content.trim_end().parse::<f64>().unwrap_or(NAN);
                                            self.values.push_back(self.curr_meas);
                                            if self.values.len() > self.mem_depth {
                                                self.values.pop_front();
                                            }
                                        }
                                    }
                                }
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                // println!("WouldBlock escape");
                                break;
                            }
                            Err(e) => {
                                println!("Quitting due to read error: {}", e);
                                // return Err(e);
                                break; // TODO display this
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
                        .desired_width(150.0)
                        .select_on_focus(true)
                        .filter_by_input(false),
                    );

                    if ui.button("Connect").clicked() {
                        self.serial = mio_serial::new(&self.serial_port, self.baud_rate)
                            .open_native_async()
                            .ok();
                        if let Some(serial) = &mut self.serial {
                            let _ = self.poll.registry().register(
                                serial,
                                SERIAL_TOKEN,
                                Interest::READABLE | Interest::WRITABLE,
                            );

                            // configure serial session
                            // TODO this might need to be generalized
                            let _ = serial.set_data_bits(DataBits::Eight);
                            let _ = serial.set_stop_bits(mio_serial::StopBits::One);
                            let _ = serial.set_parity(mio_serial::Parity::None);

                            //kick off first write
                            self.issue_new_write = true;

                            // TODO currently this does not handle the serial comms
                            // but just request a repaint every 10ms, the serial comms
                            // happen directly in this UI update function above
                            Self::dispatch_serial_comms(ctx.clone());
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

            ui.horizontal(|ui| {
                let meter_frame = egui::Frame {
                    inner_margin: 12.0.into(),
                    outer_margin: 24.0.into(),
                    rounding: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8.0, 12.0].into(),
                        blur: 16.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 255),
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                meter_frame.show(ui, |ui| {
                    ui.style_mut().wrap = Some(false);
                    ui.allocate_ui_with_layout(
                        // TODO this is bad as we actually want the size based on the minimal the fonts need
                        Vec2 { x: 400.0, y: 300.0 },
                        egui::Layout::top_down(egui::Align::RIGHT).with_cross_justify(false),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!("{:>10.4}", self.curr_meas))
                                    .color(egui::Color32::YELLOW)
                                    .font(FontId {
                                        size: 60.0,
                                        family: FontFamily::Name("B612Mono-Bold".into()),
                                    }),
                            );
                            ui.label(
                                egui::RichText::new(format!("{:>10}", self.curr_unit))
                                    .color(egui::Color32::YELLOW)
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
                    rounding: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8.0, 12.0].into(),
                        blur: 16.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 255),
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                control_frame.show(ui, |ui| {
                    ui.vertical(|ui| {
                        let btn_size = Vec2 { x: 70.0, y: 20.0 };
                        ui.horizontal(|ui| {
                            let vdc_btn = egui::Button::new("VDC")
                                .selected(self.metermode == MeterMode::VDC)
                                .min_size(btn_size);
                            if ui.add(vdc_btn).clicked() {
                                self.metermode = MeterMode::VDC;
                                self.curr_unit = "VDC".to_owned();
                                self.confstring = "CONF:VOLT:DC AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let vac_btn = egui::Button::new("VAC")
                                .selected(self.metermode == MeterMode::VAC)
                                .min_size(btn_size);
                            if ui.add(vac_btn).clicked() {
                                self.metermode = MeterMode::VAC;
                                self.curr_unit = "VAC".to_owned();
                                self.confstring = "CONF:VOLT:AC AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let adc_btn = egui::Button::new("ADC")
                                .selected(self.metermode == MeterMode::ADC)
                                .min_size(btn_size);
                            if ui.add(adc_btn).clicked() {
                                self.metermode = MeterMode::ADC;
                                self.curr_unit = "ADC".to_owned();
                                self.confstring = "CONF:CURR:DC AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let aac_btn = egui::Button::new("AAC")
                                .selected(self.metermode == MeterMode::AAC)
                                .min_size(btn_size);
                            if ui.add(aac_btn).clicked() {
                                self.metermode = MeterMode::AAC;
                                self.curr_unit = "AAC".to_owned();
                                self.confstring = "CONF:CURR:AC AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                        });
                        ui.horizontal(|ui| {
                            let res_btn = egui::Button::new("Ohm")
                                .selected(self.metermode == MeterMode::RES)
                                .min_size(btn_size);
                            if ui.add(res_btn).clicked() {
                                self.metermode = MeterMode::RES;
                                self.curr_unit = "Ohm".to_owned();
                                self.confstring = "CONF:RES AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let cap_btn = egui::Button::new("C")
                                .selected(self.metermode == MeterMode::CAP)
                                .min_size(btn_size);
                            if ui.add(cap_btn).clicked() {
                                self.metermode = MeterMode::CAP;
                                self.curr_unit = "F".to_owned();
                                self.confstring = "CONF:CAP AUTO\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let freq_btn = egui::Button::new("Freq")
                                .selected(self.metermode == MeterMode::FREQ)
                                .min_size(btn_size);
                            if ui.add(freq_btn).clicked() {
                                self.metermode = MeterMode::FREQ;
                                self.curr_unit = "Hz".to_owned();
                                self.confstring = "CONF:FREQ\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let per_btn = egui::Button::new("Period")
                                .selected(self.metermode == MeterMode::PER)
                                .min_size(btn_size);
                            if ui.add(per_btn).clicked() {
                                self.metermode = MeterMode::PER;
                                self.curr_unit = "s".to_owned();
                                self.confstring = "CONF:PER\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                        });
                        ui.horizontal(|ui| {
                            let diod_btn = egui::Button::new("Diode")
                                .selected(self.metermode == MeterMode::DIOD)
                                .min_size(btn_size);
                            if ui.add(diod_btn).clicked() {
                                self.metermode = MeterMode::DIOD;
                                self.curr_unit = "V".to_owned();
                                self.confstring = "CONF:DIOD\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let cont_btn = egui::Button::new("Cont")
                                .selected(self.metermode == MeterMode::CONT)
                                .min_size(btn_size);
                            if ui.add(cont_btn).clicked() {
                                self.metermode = MeterMode::CONT;
                                self.curr_unit = "Ohm".to_owned();
                                self.confstring = "CONF:DIOD\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                            let cont_btn = egui::Button::new("Temp")
                                .selected(self.metermode == MeterMode::TEMP)
                                .min_size(btn_size);
                            if ui.add(cont_btn).clicked() {
                                self.metermode = MeterMode::TEMP;
                                self.curr_unit = "°C".to_owned();
                                // TODO temp mode needs more selections like sensor typ, unit etc.
                                self.confstring = "CONF:TEMP:RTD 2\n".to_owned();
                                self.scpimode = ScpiMode::CONF;
                                self.issue_new_write = true;
                                self.values = VecDeque::with_capacity(self.mem_depth);
                            }
                        });
                    });
                });
            });

            ui.separator();

            ui.vertical(|ui| {
                let line = Line::new(PlotPoints::from_ys_f64(&self.values.make_contiguous()));
                let mut plot = Plot::new("graph")
                    .legend(Legend::default())
                    .y_axis_width(4)
                    .show_axes(true)
                    .show_grid(true)
                    .height(400.0)
                    .include_x(self.mem_depth as f64);
                plot.show(ui, |plot_ui| {
                    plot_ui.line(line);
                })
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
        ui.label(", ");
        ui.hyperlink_to("B612 Font", "https://b612-font.com/");
        ui.label(" and ");
        ui.hyperlink_to(
            "TheHWCave",
            "https://github.com/TheHWcave/OWON-XDM1041/tree/main",
        );
        ui.label(".");
    });
}
