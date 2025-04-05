use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    sync::Arc,
    time::Duration,
};

use egui::{
    Context, FontData, FontDefinitions, FontFamily, FontId, SliderClamping, TextEdit, Vec2, Window,
};
use egui_dropdown::DropDownBox;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use mio::{Events, Interest, Poll, Token};
use mio_serial::{DataBits, SerialPort, SerialPortInfo};
use mio_serial::{SerialPortBuilderExt, SerialStream};
use phf::{phf_ordered_map, OrderedMap};
use std::io;
use tempfile::{Builder, TempDir};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};

mod helpers;
use helpers::format_measurement;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const SERIAL_TOKEN: Token = Token(0);

const MEM_DEPTH_DEFAULT: usize = 100; // Default slider value
const MEM_DEPTH_MAX_DEFAULT: usize = 2000; // Default maximum

/// A trait that must be implemented for all SCPI command structs.
/// Gets passed the struct instance itself and the selected option name
/// and must return a complete SCPI command string (including newline)
/// that can be sent via serial or LXI to the target device.
pub trait GenScpi {
    fn gen_scpi(&self, opt_name: &str) -> String;
}

enum ScpiMode {
    Idn,
    Conf,
    Syst,
    Meas,
}

#[derive(PartialEq)]
enum MeterMode {
    Vdc,
    Vac,
    Adc,
    Aac,
    Res,
    Cap,
    Freq,
    Per,
    Diod,
    Cont,
    Temp,
}

pub struct RateCmd {
    scpi: &'static str,
    opts: OrderedMap<&'static str, &'static str>,
}

impl Default for RateCmd {
    // this corresponds to OWON XDM1041
    fn default() -> Self {
        Self {
            scpi: "RATE ",
            opts: phf_ordered_map! {
                "Slow" => "S",
                "Medium" => "M",
                "Fast" => "F",
            },
        }
    }
}

impl GenScpi for RateCmd {
    fn gen_scpi(&self, opt_name: &str) -> String {
        format!("{}{}\n", self.scpi, self.opts[opt_name])
    }
}

pub struct RangeCmd {
    scpi: &'static str,
    opts: OrderedMap<&'static str, &'static str>,
}

impl Default for RangeCmd {
    // this corresponds to OWON XDM1041 VDC ranges
    fn default() -> Self {
        Self {
            scpi: "CONF:VOLT:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "50mV" => "50E-3",
                "500mV" => "500E-3",
                "5V" => "5",
                "50V" => "50",
                "500V" => "500",
                "1000V" => "1000",
            },
        }
    }
}

impl GenScpi for RangeCmd {
    fn gen_scpi(&self, opt_name: &str) -> String {
        format!("{}{}\n", self.scpi, self.opts[opt_name])
    }
}

impl RangeCmd {
    fn new(meter: &str, mode: &str) -> Option<Self> {
        match (meter, mode) {
            ("OWON XDM1041", "VDC") => Some(Self::default()),
            ("OWON XDM1041", "VAC") => Some(Self::owon_xdm1041_vac()),
            ("OWON XDM1041", "ADC") => Some(Self::owon_xdm1041_adc()),
            ("OWON XDM1041", "AAC") => Some(Self::owon_xdm1041_aac()),
            ("OWON XDM1041", "RES") => Some(Self::owon_xdm1041_res()),
            ("OWON XDM1041", "CAP") => Some(Self::owon_xdm1041_cap()),
            ("OWON XDM1041", "TEMP") => Some(Self::owon_xdm1041_temp()),
            _ => None,
        }
    }

    fn owon_xdm1041_vac() -> Self {
        Self {
            scpi: "CONF:VOLT:AC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500mV" => "500E-3",
                "5V" => "5",
                "50V" => "50",
                "500V" => "500",
                "750V" => "750",
            },
        }
    }

    fn owon_xdm1041_adc() -> Self {
        Self {
            scpi: "CONF:CURR:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500uA" => "500E-6",
                "5mA" => "5E-3",
                "50mA" => "50E-3",
                "500mA" => "500E-3",
                "5A" => "5",
                "10A" => "10",
            },
        }
    }

    fn owon_xdm1041_aac() -> Self {
        Self {
            scpi: "CONF:CURR:AC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500uA" => "500E-6",
                "5mA" => "5E-3",
                "50mA" => "50E-3",
                "500mA" => "500E-3",
                "5A" => "5",
                "10A" => "10",
            },
        }
    }

    fn owon_xdm1041_res() -> Self {
        Self {
            scpi: "CONF:RES ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500Ohm" => "500",
                "5kOhm" => "5E3",
                "50kOhm" => "50E3",
                "500kOhm" => "500E3",
                "5MOhm" => "5E6",
                "50MOhm" => "50E6",
            },
        }
    }

    fn owon_xdm1041_cap() -> Self {
        Self {
            scpi: "CONF:CAP ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "50nF" => "50E-9",
                "500nF" => "500E-9",
                "5uF" => "5E-6",
                "50uF" => "50E-6",
                "500uF" => "500E-6",
                "5mF" => "5E-3",
                "50mF" => "50E-3",
            },
        }
    }

    fn owon_xdm1041_temp() -> Self {
        Self {
            scpi: "CONF:TEMP_RTD ",
            opts: phf_ordered_map! {
                "PT100" => "PT100",
                "K-type (KITS90)" => "KITS90",
            },
        }
    }
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
    mem_depth: usize,     // Persistent, adjustable via slider
    mem_depth_max: usize, // Persistent, maximum for slider
    connect_on_startup: bool,
    value_debug: bool,
    poll_interval_ms: u64,
    #[serde(skip)]
    curr_meter: String,
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
    #[serde(skip)]
    ratecmd: RateCmd,
    #[serde(skip)]
    curr_rate: usize,
    #[serde(skip)]
    rangecmd: Option<RangeCmd>,
    #[serde(skip)]
    curr_range: usize,
    #[serde(skip)]
    serial_rx: Option<Receiver<(Option<String>, Option<f64>)>>, // Updated to handle device ID and measurements
    #[serde(skip)]
    serial_tx: Option<Sender<String>>, // New channel for sending commands to serial task
}

impl Default for MyApp {
    fn default() -> Self {
        Self {
            serial_port: "".to_owned(),
            baud_rate: 115200,
            bits: 8,
            stop_bits: 1,
            parity: false,
            mem_depth: MEM_DEPTH_DEFAULT, // Default slider value: 100
            mem_depth_max: MEM_DEPTH_MAX_DEFAULT, // Default max: 2000
            connect_on_startup: false,
            value_debug: false,
            curr_meter: "OWON XDM1041".to_owned(),
            metermode: MeterMode::Vdc,
            scpimode: ScpiMode::Idn,
            confstring: "".to_owned(),
            curr_meas: f64::NAN,
            curr_unit: "VDC".to_owned(),
            issue_new_write: false,
            readbuf: [0u8; 1024],
            portlist: VecDeque::with_capacity(11),
            values: VecDeque::with_capacity(MEM_DEPTH_DEFAULT + 1),
            poll: Poll::new().unwrap(),
            events: Events::with_capacity(1),
            serial: None,
            device: "".to_owned(),
            ports: vec![],
            tempdir: Builder::new().prefix("rustymeter").tempdir().ok(),
            settings_open: false,
            is_init: false,
            ratecmd: RateCmd::default(),
            curr_rate: 0,
            rangecmd: Some(RangeCmd::default()),
            curr_range: 0,
            serial_rx: None,
            serial_tx: None, // Initialize as None
            poll_interval_ms: 20,
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
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/B612Mono-Bold.ttf"
            ))),
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

    fn spawn_serial_task(&mut self, ctx: Context) {
        if self.serial.is_none() {
            return;
        }

        let (tx_data, rx_data) = mpsc::channel::<(Option<String>, Option<f64>)>(100); // Channel for data (device ID, measurements)
        let (tx_cmd, mut rx_cmd) = mpsc::channel::<String>(100); // New channel for commands
        self.serial_rx = Some(rx_data);
        self.serial_tx = Some(tx_cmd.clone()); // Store the sender for UI use

        let mut serial = self.serial.take().unwrap();
        let interval_ms = self.poll_interval_ms;
        let value_debug = self.value_debug;

        // Clone the context for waking the UI
        let ctx_clone = ctx.clone();

        tokio::spawn(async move {
            let mut poll = Poll::new().unwrap();
            let mut events = Events::with_capacity(1);
            let mut readbuf = [0u8; 1024];
            let mut scpimode = ScpiMode::Idn;
            let mut issue_new_write = true;

            poll.registry()
                .register(
                    &mut serial,
                    SERIAL_TOKEN,
                    Interest::READABLE | Interest::WRITABLE,
                )
                .unwrap();

            loop {
                // Check for UI commands first
                if let Ok(cmd) = rx_cmd.try_recv() {
                    if let Ok(()) = serial.write_all(cmd.as_bytes()) {
                        // If it's a configuration command, switch to Meas mode after sending
                        if cmd.starts_with("CONF:") {
                            scpimode = ScpiMode::Meas;
                            issue_new_write = true; // Trigger MEAS? next
                        }
                    }
                    continue; // Prioritize handling commands
                }

                // Handle predefined startup sequence or MEAS? if no UI command
                if issue_new_write {
                    let sendstring = match scpimode {
                        ScpiMode::Idn => "*IDN?\n",
                        ScpiMode::Syst => "SYST:REM\n",
                        ScpiMode::Conf => unreachable!("CONF handled via rx_cmd"),
                        ScpiMode::Meas => "MEAS?\n",
                    };
                    if let Ok(()) = serial.write_all(sendstring.as_bytes()) {
                        match scpimode {
                            ScpiMode::Syst => {
                                scpimode = ScpiMode::Meas;
                                issue_new_write = true;
                            }
                            _ => issue_new_write = false,
                        }
                    }
                }

                if let Ok(()) = poll.poll(&mut events, Some(Duration::from_millis(interval_ms))) {
                    for event in events.iter() {
                        if event.token() == SERIAL_TOKEN {
                            loop {
                                match serial.read(&mut readbuf) {
                                    Ok(count) => {
                                        let content = String::from_utf8_lossy(&readbuf[..count]);
                                        if value_debug {
                                            println!("{:?}", content);
                                        }
                                        if content.ends_with("\r\n") {
                                            issue_new_write = true;
                                            match scpimode {
                                                ScpiMode::Idn => {
                                                    let device = content.trim_end().to_owned();
                                                    let _ =
                                                        tx_data.send((Some(device), None)).await;
                                                    scpimode = ScpiMode::Syst;
                                                }
                                                ScpiMode::Syst => {
                                                    scpimode = ScpiMode::Meas;
                                                }
                                                ScpiMode::Conf => {
                                                    scpimode = ScpiMode::Meas;
                                                }
                                                ScpiMode::Meas => {
                                                    if let Ok(meas) =
                                                        content.trim_end().parse::<f64>()
                                                    {
                                                        let _ =
                                                            tx_data.send((None, Some(meas))).await;
                                                        // Wake the UI when data is sent
                                                        ctx_clone.request_repaint();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                                    Err(e) => {
                                        println!("Serial read error: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        });

        ctx.request_repaint();
    }
}

impl eframe::App for MyApp {
    /// Called by the framework to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let is_web = cfg!(target_arch = "wasm32");

        // On startup, handle certain items once
        if !self.is_init {
            if let Ok(ports) = mio_serial::available_ports() {
                for p in ports {
                    self.portlist.push_front(p.port_name);
                }
            }
            self.is_init = true;
        }

        // Process all available messages (device ID or measurements)
        if let Some(ref mut rx) = self.serial_rx {
            while let Ok((device_opt, meas_opt)) = rx.try_recv() {
                if let Some(device) = device_opt {
                    self.device = device; // Update self.device when IDN response arrives
                }
                if let Some(meas) = meas_opt {
                    self.curr_meas = meas;
                    self.values.push_back(meas);
                    // Trim values if exceeding mem_depth, keeping most recent
                    while self.values.len() > self.mem_depth {
                        self.values.pop_front();
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
                    if !is_web && ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.add_space(16.0);

                egui::widgets::global_theme_preference_buttons(ui);
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
                        if self.serial.is_some() {
                            let _ = self.serial.as_mut().unwrap().set_data_bits(DataBits::Eight);
                            let _ = self
                                .serial
                                .as_mut()
                                .unwrap()
                                .set_stop_bits(mio_serial::StopBits::One);
                            let _ = self
                                .serial
                                .as_mut()
                                .unwrap()
                                .set_parity(mio_serial::Parity::None);
                            self.spawn_serial_task(ctx.clone());
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
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 255),
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                meter_frame.show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.allocate_ui_with_layout(
                        Vec2 { x: 400.0, y: 300.0 },
                        egui::Layout::top_down(egui::Align::RIGHT).with_cross_justify(false),
                        |ui| {
                            let (formatted_value, display_unit) = format_measurement(
                                self.curr_meas,
                                10,
                                1_000_000.0,
                                0.0001,
                                &self.metermode,
                            );
                            ui.label(
                                egui::RichText::new(formatted_value)
                                    .color(egui::Color32::YELLOW)
                                    .font(FontId {
                                        size: 60.0,
                                        family: FontFamily::Name("B612Mono-Bold".into()),
                                    }),
                            );
                            ui.label(
                                egui::RichText::new(format!("{:>10}", display_unit))
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
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
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
                                .selected(self.metermode == MeterMode::Vdc)
                                .min_size(btn_size);
                            if ui.add(vdc_btn).clicked() {
                                self.metermode = MeterMode::Vdc;
                                self.curr_unit = "VDC".to_owned();
                                self.confstring = "CONF:VOLT:DC AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "VDC");
                                self.curr_range = 0;
                            }
                            let vac_btn = egui::Button::new("VAC")
                                .selected(self.metermode == MeterMode::Vac)
                                .min_size(btn_size);
                            if ui.add(vac_btn).clicked() {
                                self.metermode = MeterMode::Vac;
                                self.curr_unit = "VAC".to_owned();
                                self.confstring = "CONF:VOLT:AC AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "VAC");
                                self.curr_range = 0;
                            }
                            let adc_btn = egui::Button::new("ADC")
                                .selected(self.metermode == MeterMode::Adc)
                                .min_size(btn_size);
                            if ui.add(adc_btn).clicked() {
                                self.metermode = MeterMode::Adc;
                                self.curr_unit = "ADC".to_owned();
                                self.confstring = "CONF:CURR:DC AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "ADC");
                                self.curr_range = 0;
                            }
                            let aac_btn = egui::Button::new("AAC")
                                .selected(self.metermode == MeterMode::Aac)
                                .min_size(btn_size);
                            if ui.add(aac_btn).clicked() {
                                self.metermode = MeterMode::Aac;
                                self.curr_unit = "AAC".to_owned();
                                self.confstring = "CONF:CURR:AC AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "AAC");
                                self.curr_range = 0;
                            }
                        });
                        ui.horizontal(|ui| {
                            let res_btn = egui::Button::new("Ohm")
                                .selected(self.metermode == MeterMode::Res)
                                .min_size(btn_size);
                            if ui.add(res_btn).clicked() {
                                self.metermode = MeterMode::Res;
                                self.curr_unit = "Ohm".to_owned();
                                self.confstring = "CONF:RES AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "RES");
                                self.curr_range = 0;
                            }
                            let cap_btn = egui::Button::new("C")
                                .selected(self.metermode == MeterMode::Cap)
                                .min_size(btn_size);
                            if ui.add(cap_btn).clicked() {
                                self.metermode = MeterMode::Cap;
                                self.curr_unit = "F".to_owned();
                                self.confstring = "CONF:CAP AUTO\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "CAP");
                                self.curr_range = 0;
                            }
                            let freq_btn = egui::Button::new("Freq")
                                .selected(self.metermode == MeterMode::Freq)
                                .min_size(btn_size);
                            if ui.add(freq_btn).clicked() {
                                self.metermode = MeterMode::Freq;
                                self.curr_unit = "Hz".to_owned();
                                self.confstring = "CONF:FREQ\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "FREQ");
                                self.curr_range = 0;
                            }
                            let per_btn = egui::Button::new("Period")
                                .selected(self.metermode == MeterMode::Per)
                                .min_size(btn_size);
                            if ui.add(per_btn).clicked() {
                                self.metermode = MeterMode::Per;
                                self.curr_unit = "s".to_owned();
                                self.confstring = "CONF:PER\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "PER");
                                self.curr_range = 0;
                            }
                        });
                        ui.horizontal(|ui| {
                            let diod_btn = egui::Button::new("Diode")
                                .selected(self.metermode == MeterMode::Diod)
                                .min_size(btn_size);
                            if ui.add(diod_btn).clicked() {
                                self.metermode = MeterMode::Diod;
                                self.curr_unit = "V".to_owned();
                                self.confstring = "CONF:DIOD\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "DIOD");
                                self.curr_range = 0;
                            }
                            let cont_btn = egui::Button::new("Cont")
                                .selected(self.metermode == MeterMode::Cont)
                                .min_size(btn_size);
                            if ui.add(cont_btn).clicked() {
                                self.metermode = MeterMode::Cont;
                                self.curr_unit = "Ohm".to_owned();
                                self.confstring = "CONF:CONT\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "CONT");
                                self.curr_range = 0;
                            }
                            let temp_btn = egui::Button::new("Temp")
                                .selected(self.metermode == MeterMode::Temp)
                                .min_size(btn_size);
                            if ui.add(temp_btn).clicked() {
                                self.metermode = MeterMode::Temp;
                                self.curr_unit = "°C".to_owned();
                                // TODO temp mode needs more selections like sensor typ, unit etc.
                                self.confstring = "CONF:TEMP:RTD PT100\n".to_owned();
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                self.values = VecDeque::with_capacity(self.mem_depth);
                                self.rangecmd = RangeCmd::new(&self.curr_meter, "TEMP");
                                self.curr_range = 0;
                            }
                        });
                    });
                });
                let options_frame = egui::Frame {
                    inner_margin: 12.0.into(),
                    outer_margin: 24.0.into(),
                    corner_radius: 5.0.into(),
                    shadow: epaint::Shadow {
                        offset: [8, 12],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(180),
                    },
                    fill: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 255),
                    stroke: egui::Stroke::new(1.0, egui::Color32::GRAY),
                };
                options_frame.show(ui, |ui| {
                    ui.vertical(|ui| {
                        let ratebox = egui::ComboBox::from_label("Sampling Rate").show_index(
                            ui,
                            &mut self.curr_rate,
                            self.ratecmd.opts.len(),
                            |i| *self.ratecmd.opts.index(i).unwrap().0,
                        );
                        if ratebox.changed() {
                            self.confstring = self
                                .ratecmd
                                .gen_scpi(self.ratecmd.opts.index(self.curr_rate).unwrap().0);
                            if let Some(ref tx) = self.serial_tx {
                                let _ = tx.try_send(self.confstring.clone());
                            }
                            if self.value_debug {
                                println!("Selected Rate changed: {}", self.confstring);
                            }
                        }
                        if let Some(rangecmd) = &self.rangecmd {
                            let rangebox = egui::ComboBox::from_label("Range").show_index(
                                ui,
                                &mut self.curr_range,
                                rangecmd.opts.len(),
                                |i| *rangecmd.opts.index(i).unwrap().0,
                            );
                            if rangebox.changed() {
                                self.confstring = rangecmd
                                    .gen_scpi(rangecmd.opts.index(self.curr_range).unwrap().0);
                                if let Some(ref tx) = self.serial_tx {
                                    let _ = tx.try_send(self.confstring.clone());
                                }
                                if self.value_debug {
                                    println!("Selected Range changed: {}", self.confstring);
                                }
                            }
                        }
                    });
                });
            });

            ui.separator();

            ui.vertical(|ui| {
                let line = Line::new(PlotPoints::from_ys_f64(self.values.make_contiguous()));
                let plot = Plot::new("graph")
                    .legend(Legend::default())
                    .y_axis_min_width(4.0)
                    .show_axes(true)
                    .show_grid(true)
                    .height(400.0)
                    .include_x(self.mem_depth as f64); // Use dynamic mem_depth
                plot.show(ui, |plot_ui| {
                    plot_ui.line(line);
                });
            });

            ui.separator();

            // New graph adjustments section
            ui.vertical(|ui| {
                ui.label("Graph Adjustments");
                ui.add(
                    egui::Slider::new(&mut self.mem_depth, 10..=self.mem_depth_max)
                        .text("Memory Depth")
                        .step_by(10.0) // Step by 10 for smoother control
                        .clamping(SliderClamping::Always), // Updated from clamp_to_range
                );
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

            // Settings window with polling rate and memory depth max adjustment
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
                            ui.checkbox(&mut self.value_debug, "Value debug (print to CLI)");
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
                                    }
                                }
                            }
                            ui.label("Note: Reconnect required to apply new poll interval");
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
