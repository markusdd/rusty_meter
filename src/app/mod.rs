use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily};
use egui_dock::DockState;
use mio::{Events, Poll};
use mio_serial::{SerialPortInfo, SerialStream};
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::{MeterMode, RangeCmd, RateCmd, ScpiMode};

// Submodules for split impl blocks
mod graph;
mod recording;
mod serial;
mod settings;
mod ui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const MEM_DEPTH_DEFAULT: usize = 100; // Default slider value
const MEM_DEPTH_MAX_DEFAULT: usize = 2000; // Default maximum

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RecordingFormat {
    Csv,
    Json,
    Xlsx,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RecordingMode {
    FixedInterval,
    Manual,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TimestampFormat {
    Rfc3339,
    Unix,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Record {
    pub index: usize, // New field for measurement index
    #[serde(with = "chrono::serde::ts_seconds")]
    pub timestamp: DateTime<chrono::Utc>,
    pub unit: String,
    pub value: f64,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(Serialize, Deserialize)]
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
    lock_remote: bool,             // Persistent, whether to lock meter in remote mode
    curr_rate: usize,              // Persistent, current sampling rate index
    reverse_graph: bool,           // Persistent, whether to reverse graph direction
    graph_line_color: Color32,     // Persistent, color for graph line
    measurement_font_color: Color32, // Persistent, color for measurement box font
    box_background_color: Color32, // Persistent, background color for measurement, mode, and option boxes
    #[serde(skip)]
    recording_open: bool, // Do not persist, whether recording viewport is open
    recording_format: RecordingFormat, // Persistent, selected recording format
    recording_file_path: String,   // Persistent, target file path
    recording_mode: RecordingMode, // Persistent, recording mode
    recording_interval_ms: u64,    // Persistent, fixed interval duration
    recording_active: bool,        // Persistent, whether recording is active
    recording_timestamp_format: TimestampFormat, // Persistent, timestamp format
    #[serde(skip)]
    recording_data: Vec<Record>, // Do not persist recording data
    #[serde(skip)]
    recording_data_len: usize, // Do not persist, tracks length of recording_data for auto-scroll
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
    hist_values: VecDeque<f64>, // Buffer for histogram data
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
    tempdir: Option<tempfile::TempDir>,
    #[serde(skip)]
    settings_open: bool,
    #[serde(skip)]
    is_init: bool,
    #[serde(skip)]
    ratecmd: RateCmd,
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
    mode_rx: Option<mpsc::Receiver<MeterMode>>, // Channel for mode updates
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
    #[serde(skip)]
    meas_count: u32, // Track measurement cycles for periodic FUNC? polling
    #[serde(skip)]
    last_record_time: f64, // Track last recording time for fixed interval
    graph_config: graph::GraphConfig, // Graph configuration
    #[serde(skip)]
    plot_dock_state: DockState<ui::PlotTab>, // Dock state for plot tabs
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
            hist_values: VecDeque::with_capacity(MEM_DEPTH_DEFAULT + 1), // Initialize histogram buffer
            poll: Poll::new().unwrap(),
            events: Events::with_capacity(1),
            serial: None,
            device: Arc::new(Mutex::new("".to_owned())), // Initialize as shared
            ports: vec![],
            tempdir: tempfile::Builder::new().prefix("rustymeter").tempdir().ok(),
            settings_open: false,
            is_init: false,
            ratecmd: RateCmd::default(),
            curr_rate: 0,
            rangecmd: Some(RangeCmd::default()),
            curr_range: 0,
            reverse_graph: false, // Default to right-to-left (most recent on right)
            graph_line_color: Color32::from_rgb(0, 255, 255), // Default to cyan (#00FFFF)
            measurement_font_color: Color32::from_rgb(0, 255, 255), // Default to cyan (#00FFFF)
            box_background_color: Color32::from_rgba_unmultiplied(0, 0, 0, 255), // Default to black
            recording_open: false, // Always start closed
            recording_format: RecordingFormat::Csv,
            recording_file_path: "".to_owned(),
            recording_mode: RecordingMode::FixedInterval,
            recording_interval_ms: 1000, // Default to 1 second
            recording_active: false,
            recording_timestamp_format: TimestampFormat::Rfc3339, // Default to RFC3339
            recording_data: vec![],                               // Initialize empty, not persisted
            recording_data_len: 0, // Initialize to 0, tracks length of recording_data
            serial_rx: None,
            serial_tx: None,
            shutdown_tx: None, // Initially no shutdown signal
            mode_rx: None,     // Initially no mode update channel
            poll_interval_ms: 20,
            graph_update_interval_ms: 20, // Default to 20ms for ~50 FPS
            graph_update_interval_max: 1000, // Default maximum of 1000ms
            beeper_enabled: true,         // Default to on, per meter spec
            cont_threshold: 50,           // Default continuity threshold: 50 ohms
            diod_threshold: 2.0,          // Default diode threshold: 2.0 volts (mid-range)
            lock_remote: true,            // Default to locking remote mode
            value_debug_shared: Arc::new(Mutex::new(false)),
            poll_interval_shared: Arc::new(Mutex::new(20)),
            graph_update_interval_shared: Arc::new(Mutex::new(20)), // Default shared value to 20ms
            last_graph_update: 0.0,                                 // Initialize to 0
            connection_state: ConnectionState::Disconnected,        // Initially disconnected
            connection_error: None,                                 // No error initially
            meas_count: 0,         // Initialize measurement counter
            last_record_time: 0.0, // Initialize last recording time
            graph_config: graph::GraphConfig::default(), // Default graph config
            plot_dock_state: DockState::new(vec![]), // Initialize empty, populated in update
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
                "../../assets/fonts/B612Mono-Bold.ttf"
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
        self.hist_values = VecDeque::with_capacity(self.mem_depth); // Reset histogram buffer
        self.rangecmd = range_type.and_then(|rt| RangeCmd::new(&self.curr_meter, rt));
        self.curr_range = 0;
    }

    // Method to handle disconnection
    fn disconnect(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(()); // Signal the serial task to shut down
        }
        self.serial_tx = None; // Drop sender to stop sending commands
        self.serial_rx = None; // Drop receiver to stop receiving measurements
        self.mode_rx = None; // Drop mode receiver
        self.serial = None; // Clear serial port
        self.connection_state = ConnectionState::Disconnected;
        self.connection_error = None; // Clear any previous error
        let mut device = self.device.lock().unwrap();
        *device = "".to_owned(); // Clear device string
        self.curr_meas = f64::NAN; // Reset measurement
        self.values.clear(); // Clear graph data
        self.hist_values.clear(); // Clear histogram data
        self.meas_count = 0; // Reset measurement counter
    }
}