use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    sync::{Arc, Mutex},
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
use tokio::sync::{mpsc, oneshot};

mod helpers;
use helpers::format_measurement;
use helpers::powered_by;

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

#[derive(PartialEq, Clone, Copy, Debug)]
enum ScpiMode {
    Idn,
    Meas,
}

#[derive(PartialEq, Clone, Copy)]
pub enum MeterMode {
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
    graph_update_interval_ms: u64, // Persistent, adjustable via slider in main GUI
    graph_update_interval_max: u64, // Persistent, maximum for graph update interval slider
    beeper_enabled: bool,          // New field for beeper state, persistent
    cont_threshold: u32,           // Persistent continuity threshold (0-1000 ohms)
    diod_threshold: f32,           // Persistent diode threshold (0-3.0 volts)
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
    device: Arc<Mutex<String>>, // Changed to shared ownership
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
    serial_rx: Option<mpsc::Receiver<Option<f64>>>, // handle measurements
    #[serde(skip)]
    serial_tx: Option<mpsc::Sender<String>>, // channel for sending commands to serial task
    #[serde(skip)]
    shutdown_tx: Option<oneshot::Sender<()>>, // Signal to shutdown serial task
    #[serde(skip)]
    value_debug_shared: Arc<Mutex<bool>>, // Shared debug flag for live updates
    #[serde(skip)]
    poll_interval_shared: Arc<Mutex<u64>>, // Shared poll interval for live updates
    #[serde(skip)]
    graph_update_interval_shared: Arc<Mutex<u64>>, // Shared graph update interval
    #[serde(skip)]
    last_graph_update: f64, // Track last graph update time
    #[serde(skip)]
    connection_state: ConnectionState, // New field to track connection status
    #[serde(skip)]
    connection_error: Option<String>, // New field to store connection error message
}

// Enum to track connection state
#[derive(PartialEq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
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
            device: Arc::new(Mutex::new("".to_owned())), // Initialize as shared
            ports: vec![],
            tempdir: Builder::new().prefix("rustymeter").tempdir().ok(),
            settings_open: false,
            is_init: false,
            ratecmd: RateCmd::default(),
            curr_rate: 0,
            rangecmd: Some(RangeCmd::default()),
            curr_range: 0,
            serial_rx: None,
            serial_tx: None,
            shutdown_tx: None, // Initially no shutdown signal
            poll_interval_ms: 20,
            graph_update_interval_ms: 20, // Default to 20ms for ~50 FPS
            graph_update_interval_max: 1000, // Default maximum of 1000ms
            beeper_enabled: true,         // Default to on, per meter spec
            cont_threshold: 50,           // Default continuity threshold: 50 ohms
            diod_threshold: 2.0,          // Default diode threshold: 2.0 volts (mid-range)
            value_debug_shared: Arc::new(Mutex::new(false)),
            poll_interval_shared: Arc::new(Mutex::new(20)),
            graph_update_interval_shared: Arc::new(Mutex::new(20)), // Default shared value to 20ms
            last_graph_update: 0.0,                                 // Initialize to 0
            connection_state: ConnectionState::Disconnected,        // Initially disconnected
            connection_error: None,                                 // No error initially
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
            let app: MyApp = eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
            *app.value_debug_shared.lock().unwrap() = app.value_debug;
            *app.poll_interval_shared.lock().unwrap() = app.poll_interval_ms;
            *app.graph_update_interval_shared.lock().unwrap() = app.graph_update_interval_ms;
            return app;
        }

        let app = Self::default();
        *app.value_debug_shared.lock().unwrap() = app.value_debug;
        *app.poll_interval_shared.lock().unwrap() = app.poll_interval_ms;
        *app.graph_update_interval_shared.lock().unwrap() = app.graph_update_interval_ms;
        app
    }

    fn spawn_serial_task(&mut self) {
        if self.serial.is_none() {
            return;
        }

        let (tx_data, rx_data) = mpsc::channel::<Option<f64>>(100); // Channel for measurements only
        let (tx_cmd, mut rx_cmd) = mpsc::channel::<String>(100); // Channel for commands
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>(); // Shutdown signal
        self.serial_rx = Some(rx_data);
        self.serial_tx = Some(tx_cmd.clone());
        self.shutdown_tx = Some(shutdown_tx);

        let mut serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let poll_interval_shared = self.poll_interval_shared.clone();
        let device_shared = self.device.clone(); // Clone Arc for task

        tokio::spawn(async move {
            let mut poll = Poll::new().unwrap();
            let mut events = Events::with_capacity(1);
            let mut readbuf = [0u8; 1024];
            let mut scpimode = ScpiMode::Idn;
            let mut command_queue: VecDeque<String> = VecDeque::new();
            let mut shutting_down = false;
            let mut drop_serial = false; // Flag to indicate when to drop serial

            // Register serial port for readable and writable events
            poll.registry()
                .register(
                    &mut serial,
                    SERIAL_TOKEN,
                    Interest::READABLE | Interest::WRITABLE,
                )
                .unwrap();
            if *value_debug_shared.lock().unwrap() {
                println!("Serial port registered for READABLE and WRITABLE events");
            }

            // Initial command to identify device
            command_queue.push_back("*IDN?\n".to_string());

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx, if !shutting_down => {
                        // Shutdown signal received, stop queuing MEAS? but keep processing UI commands
                        if *value_debug_shared.lock().unwrap() {
                            println!("Shutdown signal received, processing remaining queue: {:?}", command_queue);
                        }
                        shutting_down = true;
                    }
                    _ = async {
                        let debug = *value_debug_shared.lock().unwrap();
                        let interval = *poll_interval_shared.lock().unwrap();

                        if debug {
                            println!("Starting poll loop, queue: {:?}", command_queue);
                        }

                        // Queue new commands from UI (always, even during shutdown)
                        while let Ok(cmd) = rx_cmd.try_recv() {
                            if debug {
                                println!("Queuing command from UI: {:?}", cmd);
                            }
                            command_queue.push_back(cmd);
                        }

                        // Poll for readable or writable events
                        match poll.poll(&mut events, Some(Duration::from_millis(interval))) {
                            Ok(()) => {
                                if debug {
                                    println!(
                                        "Poll returned events: {:?}",
                                        events.iter().collect::<Vec<_>>()
                                    );
                                }

                                for event in events.iter() {
                                    // Handle writes
                                    if event.is_writable() && !command_queue.is_empty() {
                                        if debug {
                                            println!("Writable event detected, queue: {:?}", command_queue);
                                        }
                                        if let Some(cmd) = command_queue.front() {
                                            if debug {
                                                println!("Sending: {:?}", cmd);
                                            }
                                            match serial.write_all(cmd.as_bytes()) {
                                                Ok(()) => {
                                                    let cmd = command_queue.pop_front().unwrap();
                                                    if debug {
                                                        println!("Command sent: {:?}", cmd);
                                                    }
                                                    // Queue SYST:REM and MEAS? after sending *IDN?
                                                    if cmd == "*IDN?\n" && !shutting_down {
                                                        command_queue.push_back("SYST:REM\n".to_string());
                                                        command_queue.push_back("MEAS?\n".to_string());
                                                        if debug {
                                                            println!(
                                                                "Queued SYST:REM and MEAS? after sending *IDN?, queue: {:?}",
                                                                command_queue
                                                            );
                                                        }
                                                    }
                                                    // Set flag to drop serial after *RST is sent during shutdown
                                                    if shutting_down && cmd == "*RST\n" {
                                                        if debug {
                                                            println!("*RST sent, marking serial for shutdown");
                                                        }
                                                        drop_serial = true;
                                                    }
                                                }
                                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                                    if debug {
                                                        println!(
                                                            "Serial write would block for {:?}, waiting",
                                                            cmd
                                                        );
                                                    }
                                                    break;
                                                }
                                                Err(e) => {
                                                    if debug {
                                                        println!("Failed to send command {:?}: {}", cmd, e);
                                                    }
                                                    command_queue.pop_front();
                                                    break;
                                                }
                                            }
                                        }
                                    }

                                    // Handle reads
                                    if event.is_readable() {
                                        if debug {
                                            println!("Readable event detected");
                                        }
                                        loop {
                                            match serial.read(&mut readbuf) {
                                                Ok(count) => {
                                                    let content =
                                                        String::from_utf8_lossy(&readbuf[..count]);
                                                    if debug {
                                                        println!("Received: {:?}", content);
                                                    }
                                                    if content.ends_with("\r\n") {
                                                        if scpimode == ScpiMode::Idn {
                                                            let mut device = device_shared.lock().unwrap();
                                                            *device = content.trim_end().to_owned();
                                                            scpimode = ScpiMode::Meas;
                                                            if debug {
                                                                println!(
                                                                    "Updated device string: {}",
                                                                    *device
                                                                );
                                                            }
                                                        } else if scpimode == ScpiMode::Meas {
                                                            if let Ok(meas) =
                                                                content.trim_end().parse::<f64>()
                                                            {
                                                                let _ = tx_data.send(Some(meas)).await;
                                                                if debug {
                                                                    println!("Sent measurement: {}", meas);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                                    if debug {
                                                        println!("Read would block, exiting read loop");
                                                    }
                                                    break;
                                                }
                                                Err(e) => {
                                                    if debug {
                                                        println!("Serial read error: {}", e);
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if debug {
                                    println!("Poll error: {}", e);
                                }
                            }
                        }

                        // Queue MEAS? for continuous polling in Meas mode if queue is empty, only if not shutting down
                        if !shutting_down && scpimode == ScpiMode::Meas && command_queue.is_empty() {
                            command_queue.push_back("MEAS?\n".to_string());
                            if debug {
                                println!("Queued MEAS? for polling, queue: {:?}", command_queue);
                            }
                        }

                        tokio::time::sleep(Duration::from_millis(interval)).await;
                    } => {}
                }

                // Exit the loop if we're shutting down and serial should be dropped
                if shutting_down && drop_serial {
                    break;
                }
            }

            // Cleanup after exiting the loop
            if *value_debug_shared.lock().unwrap() {
                println!("Cleaning up serial task");
            }
            let _ = poll.registry().deregister(&mut serial);
            drop(serial); // Explicitly drop the serial port
        });
    }

    fn spawn_graph_update_task(&mut self, ctx: Context) {
        let graph_update_interval_shared = self.graph_update_interval_shared.clone();
        let ctx = ctx.clone();

        tokio::spawn(async move {
            loop {
                let interval = *graph_update_interval_shared.lock().unwrap();
                ctx.request_repaint(); // Trigger a repaint to update the graph
                tokio::time::sleep(Duration::from_millis(interval)).await;
            }
        });
    }

    fn set_mode(
        &mut self,
        mode: MeterMode,
        unit: &str,
        cmd: &str,
        range_type: Option<&str>,
        beeper_enabled: Option<bool>,
    ) {
        self.metermode = mode;
        self.curr_unit = unit.to_owned();
        self.confstring = cmd.to_owned();
        if let Some(tx) = self.serial_tx.clone() {
            let mode_cmd = self.confstring.clone();
            let value_debug = self.value_debug;
            let cont_threshold = self.cont_threshold;
            let diod_threshold = self.diod_threshold;
            if let Some(beep) = beeper_enabled {
                let beeper_cmd = if beep {
                    "SYST:BEEP:STATe ON\n".to_string()
                } else {
                    "SYST:BEEP:STATe OFF\n".to_string()
                };
                let threshold_cmd = if mode == MeterMode::Cont {
                    format!("CONT:THREshold {}\n", cont_threshold)
                } else {
                    format!("DIOD:THREshold {}\n", diod_threshold)
                };
                tokio::spawn(async move {
                    // Queue commands without delays
                    if let Err(e) = tx.send(mode_cmd.clone()).await {
                        if value_debug {
                            println!("Failed to queue mode command: {}", e);
                        }
                    } else if value_debug {
                        println!("Mode command queued: {}", mode_cmd);
                    }
                    if let Err(e) = tx.send(beeper_cmd.clone()).await {
                        if value_debug {
                            println!("Failed to queue beeper command: {}", e);
                        }
                    } else if value_debug {
                        println!("Beeper command queued: {}", beeper_cmd);
                    }
                    if let Err(e) = tx.send(threshold_cmd.clone()).await {
                        if value_debug {
                            println!("Failed to queue threshold command: {}", e);
                        }
                    } else if value_debug {
                        println!("Threshold command queued: {}", threshold_cmd);
                    }
                });
            } else {
                tokio::spawn(async move {
                    if let Err(e) = tx.send(mode_cmd.clone()).await {
                        if value_debug {
                            println!("Failed to queue command: {}", e);
                        }
                    } else if value_debug {
                        println!("Command queued: {}", mode_cmd);
                    }
                });
            }
        }
        self.values = VecDeque::with_capacity(self.mem_depth);
        self.rangecmd = range_type.and_then(|rt| RangeCmd::new(&self.curr_meter, rt));
        self.curr_range = 0;
    }

    // Method to handle disconnection
    fn disconnect(&mut self) {
        if let Some(ref tx) = self.serial_tx {
            // Queue SYST:LOC to exit remote mode
            if let Err(e) = tx.try_send("SYST:LOC\n".to_string()) {
                if self.value_debug {
                    println!("Failed to queue SYST:LOC: {}", e);
                }
            } else if self.value_debug {
                println!("SYST:LOC queued");
            }
            // Queue *RST to reset the meter
            if let Err(e) = tx.try_send("*RST\n".to_string()) {
                if self.value_debug {
                    println!("Failed to queue *RST: {}", e);
                }
            } else if self.value_debug {
                println!("*RST queued");
            }
        }
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(()); // Signal the serial task to shut down
        }
        self.serial_tx = None; // Drop sender to stop sending commands
        self.serial_rx = None; // Drop receiver to stop receiving measurements
        self.serial = None; // Clear serial port
        self.connection_state = ConnectionState::Disconnected;
        self.connection_error = None; // Clear any previous error
        let mut device = self.device.lock().unwrap();
        *device = "".to_owned(); // Clear device string
        self.curr_meas = f64::NAN; // Reset measurement
        self.values.clear(); // Clear graph data
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

        // Process all available measurements (no longer handling device ID here)
        if let Some(ref mut rx) = self.serial_rx {
            while let Ok(meas_opt) = rx.try_recv() {
                if let Some(meas) = meas_opt {
                    self.curr_meas = meas; // Update curr_meas with new data
                }
            }
        }

        // Handle graph updates based on the configured interval
        let current_time = ctx.input(|i| i.time); // Get current time in seconds
        let graph_interval = *self.graph_update_interval_shared.lock().unwrap() as f64 / 1000.0; // Convert ms to seconds
        if current_time - self.last_graph_update >= graph_interval {
            if !self.curr_meas.is_nan() {
                self.values.push_back(self.curr_meas);
                while self.values.len() > self.mem_depth {
                    self.values.pop_front();
                }
            }
            self.last_graph_update = current_time;
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    if !is_web && ui.button("Quit").clicked() {
                        self.disconnect(); // Use disconnect method instead of partial cleanup
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.add_space(16.0);
                egui::widgets::global_theme_preference_buttons(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
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

                    match self.connection_state {
                        ConnectionState::Disconnected => {
                            if ui.button("Connect").clicked() {
                                self.connection_state = ConnectionState::Connecting;
                                self.connection_error = None;
                                match mio_serial::new(&self.serial_port, self.baud_rate)
                                    .open_native_async()
                                {
                                    Ok(serial) => {
                                        self.serial = Some(serial);
                                        if let Some(ref mut serial) = self.serial {
                                            let _ = serial.set_data_bits(DataBits::Eight);
                                            let _ = serial.set_stop_bits(mio_serial::StopBits::One);
                                            let _ = serial.set_parity(mio_serial::Parity::None);
                                            self.connection_state = ConnectionState::Connected;
                                            self.spawn_serial_task();
                                            self.spawn_graph_update_task(ctx.clone());
                                        }
                                    }
                                    Err(e) => {
                                        self.connection_state = ConnectionState::Disconnected;
                                        self.connection_error =
                                            Some(format!("Failed to connect: {}", e));
                                    }
                                }
                            }
                        }
                        ConnectionState::Connecting => {
                            ui.label("Connecting...");
                        }
                        ConnectionState::Connected => {
                            if ui.button("Disconnect").clicked() {
                                self.disconnect();
                            }
                        }
                    }
                });

                ui.horizontal(|ui| {
                    let device = self.device.lock().unwrap();
                    match self.connection_state {
                        ConnectionState::Disconnected => {
                            if let Some(ref error) = self.connection_error {
                                ui.label(egui::RichText::new(error).color(egui::Color32::RED));
                            } else {
                                ui.label("Not connected.");
                            }
                        }
                        ConnectionState::Connecting => {
                            ui.label("Attempting to connect...");
                        }
                        ConnectionState::Connected => {
                            if !device.is_empty() {
                                ui.label("Connected to: ");
                                ui.label(&*device);
                            } else {
                                ui.label("Connected, awaiting device ID...");
                            }
                        }
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
                                self.set_mode(
                                    MeterMode::Vdc,
                                    "VDC",
                                    "CONF:VOLT:DC AUTO\n",
                                    Some("VDC"),
                                    None,
                                );
                            }
                            let vac_btn = egui::Button::new("VAC")
                                .selected(self.metermode == MeterMode::Vac)
                                .min_size(btn_size);
                            if ui.add(vac_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Vac,
                                    "VAC",
                                    "CONF:VOLT:AC AUTO\n",
                                    Some("VAC"),
                                    None,
                                );
                            }
                            let adc_btn = egui::Button::new("ADC")
                                .selected(self.metermode == MeterMode::Adc)
                                .min_size(btn_size);
                            if ui.add(adc_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Adc,
                                    "ADC",
                                    "CONF:CURR:DC AUTO\n",
                                    Some("ADC"),
                                    None,
                                );
                            }
                            let aac_btn = egui::Button::new("AAC")
                                .selected(self.metermode == MeterMode::Aac)
                                .min_size(btn_size);
                            if ui.add(aac_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Aac,
                                    "AAC",
                                    "CONF:CURR:AC AUTO\n",
                                    Some("AAC"),
                                    None,
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            let res_btn = egui::Button::new("Ohm")
                                .selected(self.metermode == MeterMode::Res)
                                .min_size(btn_size);
                            if ui.add(res_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Res,
                                    "Ohm",
                                    "CONF:RES AUTO\n",
                                    Some("RES"),
                                    None,
                                );
                            }
                            let cap_btn = egui::Button::new("C")
                                .selected(self.metermode == MeterMode::Cap)
                                .min_size(btn_size);
                            if ui.add(cap_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Cap,
                                    "F",
                                    "CONF:CAP AUTO\n",
                                    Some("CAP"),
                                    None,
                                );
                            }
                            let freq_btn = egui::Button::new("Freq")
                                .selected(self.metermode == MeterMode::Freq)
                                .min_size(btn_size);
                            if ui.add(freq_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Freq,
                                    "Hz",
                                    "CONF:FREQ\n",
                                    Some("FREQ"),
                                    None,
                                );
                            }
                            let per_btn = egui::Button::new("Period")
                                .selected(self.metermode == MeterMode::Per)
                                .min_size(btn_size);
                            if ui.add(per_btn).clicked() {
                                self.set_mode(MeterMode::Per, "s", "CONF:PER\n", Some("PER"), None);
                            }
                        });
                        ui.horizontal(|ui| {
                            let diod_btn = egui::Button::new("Diode")
                                .selected(self.metermode == MeterMode::Diod)
                                .min_size(btn_size);
                            if ui.add(diod_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Diod,
                                    "V",
                                    "CONF:DIOD\n",
                                    Some("DIOD"),
                                    Some(self.beeper_enabled),
                                );
                            }
                            let cont_btn = egui::Button::new("Cont")
                                .selected(self.metermode == MeterMode::Cont)
                                .min_size(btn_size);
                            if ui.add(cont_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Cont,
                                    "Ohm",
                                    "CONF:CONT\n",
                                    Some("CONT"),
                                    Some(self.beeper_enabled),
                                );
                            }
                            let temp_btn = egui::Button::new("Temp")
                                .selected(self.metermode == MeterMode::Temp)
                                .min_size(btn_size);
                            if ui.add(temp_btn).clicked() {
                                self.set_mode(
                                    MeterMode::Temp,
                                    "°C",
                                    "CONF:TEMP:RTD PT100\n",
                                    Some("TEMP"),
                                    None,
                                );
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
                            if let Some(tx) = self.serial_tx.clone() {
                                let cmd = self.confstring.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = tx.send(cmd).await {
                                        println!("Failed to queue command: {}", e);
                                    }
                                });
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
                                if let Some(tx) = self.serial_tx.clone() {
                                    let cmd = self.confstring.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = tx.send(cmd).await {
                                            println!("Failed to queue command: {}", e);
                                        }
                                    });
                                }
                                if self.value_debug {
                                    println!("Selected Range changed: {}", self.confstring);
                                }
                            }
                        }
                        // Add beeper and threshold controls for CONT and DIOD modes
                        if self.metermode == MeterMode::Cont || self.metermode == MeterMode::Diod {
                            let mut beeper = self.beeper_enabled;
                            if ui.checkbox(&mut beeper, "Beeper").changed() {
                                self.beeper_enabled = beeper;
                                if let Some(tx) = self.serial_tx.clone() {
                                    let cmd = if beeper {
                                        "SYST:BEEP:STATe ON\n".to_string()
                                    } else {
                                        "SYST:BEEP:STATe OFF\n".to_string()
                                    };
                                    let value_debug = self.value_debug;
                                    tokio::spawn(async move {
                                        if let Err(e) = tx.send(cmd).await {
                                            if value_debug {
                                                println!("Failed to queue beeper command: {}", e);
                                            }
                                        }
                                    });
                                }
                            }

                            if self.metermode == MeterMode::Cont {
                                let threshold_slider = ui.add(
                                    egui::Slider::new(&mut self.cont_threshold, 0..=1000)
                                        .text("Threshold (Ω)")
                                        .step_by(1.0)
                                        .clamping(SliderClamping::Always),
                                );
                                if threshold_slider.drag_stopped() || threshold_slider.lost_focus()
                                {
                                    if let Some(tx) = self.serial_tx.clone() {
                                        let cmd =
                                            format!("CONT:THREshold {}\n", self.cont_threshold);
                                        let value_debug = self.value_debug;
                                        tokio::spawn(async move {
                                            if let Err(e) = tx.send(cmd).await {
                                                if value_debug {
                                                    println!(
                                                        "Failed to queue threshold command: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        });
                                    }
                                }
                            } else if self.metermode == MeterMode::Diod {
                                let threshold_slider = ui.add(
                                    egui::Slider::new(&mut self.diod_threshold, 0.0..=3.0)
                                        .text("Threshold (V)")
                                        .step_by(0.1)
                                        .clamping(SliderClamping::Always),
                                );
                                if threshold_slider.drag_stopped() || threshold_slider.lost_focus()
                                {
                                    if let Some(tx) = self.serial_tx.clone() {
                                        let cmd =
                                            format!("DIOD:THREshold {}\n", self.diod_threshold);
                                        let value_debug = self.value_debug;
                                        tokio::spawn(async move {
                                            if let Err(e) = tx.send(cmd).await {
                                                if value_debug {
                                                    println!(
                                                        "Failed to queue threshold command: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        });
                                    }
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

            // Graph adjustments section with sliders side by side
            ui.vertical(|ui| {
                ui.label("Graph Adjustments");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut self.mem_depth, 10..=self.mem_depth_max)
                            .text("Memory Depth")
                            .step_by(10.0) // Step by 10 for smoother control
                            .clamping(SliderClamping::Always),
                    );
                    // Graph update interval slider to the right
                    let graph_interval_slider = ui.add(
                        egui::Slider::new(
                            &mut self.graph_update_interval_ms,
                            10..=self.graph_update_interval_max,
                        )
                        .text("Update Interval (ms)")
                        .step_by(10.0) // Step by 10 for smoother control
                        .clamping(SliderClamping::Always),
                    );
                    if graph_interval_slider.changed() {
                        // Update the shared value when the slider changes
                        *self.graph_update_interval_shared.lock().unwrap() =
                            self.graph_update_interval_ms;
                    }
                });
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
                            // Add maximum graph update interval setting
                            ui.label("Maximum graph update interval (ms):");
                            let mut max_graph_interval_str =
                                self.graph_update_interval_max.to_string();
                            if ui
                                .add(
                                    TextEdit::singleline(&mut max_graph_interval_str)
                                        .desired_width(800.0)
                                        .hint_text("Enter maximum graph update interval in ms"),
                                )
                                .changed()
                            {
                                if let Ok(new_max_interval) = max_graph_interval_str.parse::<u64>()
                                {
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
                            if ui.button("Close").clicked() {
                                self.settings_open = false;
                            }
                        });
                    });
            }
        });
    }
}
