use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use egui::{Color32, FontData, FontDefinitions, FontFamily};
use egui_dock::DockState;
use mio::{Events, Poll};
use mio_serial::{SerialPortInfo, SerialStream};
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::{GenScpi, MeterMode, RangeCmd, RateCmd, ScpiMode};
use crate::scpi_macro::{
    BootstrapSettings, MacroTarget, MeterStatus, ScpiMacro, ScpiUiHint, bootstrap_commands,
    classify_idn, ensure_newline, idn_model, is_recordable_scpi, looks_like_idn, parse_macro_body,
    range_table_meter, ui_hint_from_command, ui_refresh_queries,
};

// Submodules for split impl blocks
mod graph;
#[cfg(not(target_arch = "wasm32"))]
mod hid;
mod macros;
mod recording;
mod serial;
mod settings;
mod ui;
#[cfg(not(target_arch = "wasm32"))]
mod victor_86bcd_capture_ui;
#[cfg(not(target_arch = "wasm32"))]
mod victor_readonly_serial;

/// How rusty_meter talks to the multimeter.
///
/// - `ScpiSerial` — SCPI over UART (OWON XDM series, remote control)
/// - `VictorHid` — **legacy** Victor 86B/C/D via USB HID + FS9922 cable (discontinued)
/// - `Victor86bcdSerial` — **newer** Victor (e.g. 86D): DM1107, opto-isolated CP2102 serial
/// - `Victor86eSerial` — Victor 86E via CP2102 UART + ES51932 ASCII frames (read only)
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnectionType {
    #[default]
    ScpiSerial,
    /// Legacy Victor 86B/C/D: USB HID, Fortune FS9922-DMM4. See `victor_fs9922` / sigrok wiki.
    #[cfg(not(target_arch = "wasm32"))]
    VictorHid,
    /// Newer Victor (e.g. 86D): DM1107, 9600 8N1 serial over opto-isolated USB. See `victor_dm1107`.
    #[cfg(not(target_arch = "wasm32"))]
    Victor86bcdSerial,
    /// Victor 86E: CP2102 serial 19200 7o1, Cyrustek ES51932. See `victor_es519xx` module.
    #[cfg(not(target_arch = "wasm32"))]
    Victor86eSerial,
}

/// Victor 86D / DM1107: 9600 baud, 8 data bits, no parity, 1 stop (8N1).
/// Line settings must be set on the builder before open — post-open `set_*` is unreliable.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_victor_8n1_serial(
    path: &str,
    baud: u32,
) -> Result<SerialStream, mio_serial::Error> {
    use mio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};

    mio_serial::new(path, baud)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .open_native_async()
}

/// Victor 86E / ES51932: 19200 baud, 7 data bits, odd parity, 1 stop (7o1).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_victor_7o1_serial(
    path: &str,
    baud: u32,
) -> Result<SerialStream, mio_serial::Error> {
    use mio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};

    mio_serial::new(path, baud)
        .data_bits(DataBits::Seven)
        .parity(Parity::Odd)
        .stop_bits(StopBits::One)
        .open_native_async()
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

const MEM_DEPTH_DEFAULT: usize = 100; // Default slider value
const MEM_DEPTH_MAX_DEFAULT: usize = 2000; // Default maximum
const HIST_MEM_DEPTH_DEFAULT: usize = 1000; // Default histogram memory depth
const HIST_MEM_DEPTH_MAX_DEFAULT: usize = 10000; // Default maximum histogram memory depth

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ModeDisplaySettings {
    /// Prefer mV / kΩ / µF etc. from magnitude (default on, same as SCPI).
    pub auto_scale_units: bool,
}

impl Default for ModeDisplaySettings {
    fn default() -> Self {
        Self {
            auto_scale_units: true,
        }
    }
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(Serialize, Deserialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct MyApp {
    connection_type: ConnectionType,
    serial_port: String,
    #[cfg(not(target_arch = "wasm32"))]
    hid_device_path: String,
    baud_rate: u32,
    bits: u32,
    stop_bits: u32,
    parity: bool,
    mem_depth: usize,              // Persistent, adjustable via slider
    mem_depth_max: usize,          // Persistent, maximum for slider
    hist_mem_depth: usize,         // Persistent, histogram memory depth
    hist_mem_depth_max: usize,     // Persistent, maximum for histogram memory depth
    hist_collect_interval_ms: u64, // Persistent, histogram collection interval
    hist_collect_active: bool,     // Persistent, whether histogram collection is active
    connect_on_startup: bool,
    value_debug: bool,
    poll_interval_ms: u64,
    graph_update_interval_ms: u64, // Persistent, adjustable via slider in main GUI
    graph_update_interval_max: u64, // Persistent, maximum for graph update interval slider
    beeper_enabled: bool,          // Persistent, beeper state
    rst_on_disconnect: bool,       // Persistent, whther to send RST SCPI cmd on disconnect
    cont_threshold: u32,           // Persistent continuity threshold (0-1000 ohms)
    diod_threshold: f32,           // Persistent diode threshold (0-3.0 volts)
    lock_remote: bool,             // Persistent, whether to lock meter in remote mode
    curr_rate: usize,              // Persistent, current sampling rate index
    reverse_graph: bool,           // Persistent, whether to reverse graph direction
    graph_line_color: Color32,     // Persistent, color for graph line
    hist_bar_color: Color32,       // Persistent, color for histogram bars
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
    mode_display_settings: HashMap<MeterMode, ModeDisplaySettings>,
    #[serde(default)]
    scpi_macros: Vec<ScpiMacro>,
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
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    hid_devicelist: VecDeque<(String, String)>,
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
    macros_open: bool,
    #[serde(skip)]
    selected_macro_id: Option<String>,
    #[serde(skip)]
    macro_recording: bool,
    #[serde(skip)]
    macro_record_buffer: String,
    #[serde(skip)]
    applied_idn: Option<String>,
    #[serde(skip)]
    poll_ready: Arc<AtomicBool>,
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
    mode_rx: Option<mpsc::Receiver<(MeterMode, String)>>, // Channel for mode + unit updates
    #[serde(skip)]
    status_rx: Option<mpsc::Receiver<MeterStatus>>,
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    victor_86bcd_rx: Option<mpsc::Receiver<crate::victor_dm1107::Dm1107LiveUpdate>>,
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    victor_lcd_display: String,
    #[serde(skip)]
    value_debug_shared: Arc<Mutex<bool>>, // Shared debug flag for live updates
    #[serde(skip)]
    poll_interval_shared: Arc<Mutex<u64>>, // Shared poll interval for live updates
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    victor_86bcd_capture_function: crate::victor_86bcd_capture::Victor86bcdCaptureFunction,
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_unit: crate::victor_86bcd_capture::Victor86bcdCaptureUnit,
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_dp_mode: crate::victor_86bcd_capture::Victor86bcdCaptureDpMode,
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_digits: [crate::victor_86bcd_capture::LcdDigit; 4],
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_dp_after: Option<u8>,
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_notes: String,
    #[cfg(not(target_arch = "wasm32"))]
    victor_86bcd_capture_duration_ms: u64,
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    victor_86bcd_capture_tx:
        Option<mpsc::Sender<crate::victor_86bcd_capture::Victor86bcdCaptureJob>>,
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    victor_86bcd_capture_status_shared:
        Arc<Mutex<crate::victor_86bcd_capture::Victor86bcdCaptureStatus>>,
    #[serde(skip)]
    last_graph_update: f64, // Track last graph update time
    #[serde(skip)]
    last_hist_collect_time: f64, // Track last histogram collection time
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
            connection_type: ConnectionType::default(),
            serial_port: "".to_owned(),
            #[cfg(not(target_arch = "wasm32"))]
            hid_device_path: "".to_owned(),
            baud_rate: 115200,
            bits: 8,
            stop_bits: 1,
            parity: false,
            mem_depth: MEM_DEPTH_DEFAULT, // Default slider value: 100
            mem_depth_max: MEM_DEPTH_MAX_DEFAULT, // Default max: 2000
            hist_mem_depth: HIST_MEM_DEPTH_DEFAULT, // Default histogram memory depth: 1000
            hist_mem_depth_max: HIST_MEM_DEPTH_MAX_DEFAULT, // Default max: 10000
            hist_collect_interval_ms: 100, // Default to 100ms
            hist_collect_active: false,   // Default to stopped
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
            #[cfg(not(target_arch = "wasm32"))]
            hid_devicelist: VecDeque::with_capacity(4),
            values: VecDeque::with_capacity(MEM_DEPTH_DEFAULT + 1),
            hist_values: VecDeque::with_capacity(MEM_DEPTH_DEFAULT + 1), // Initialize histogram buffer
            poll: Poll::new().unwrap(),
            events: Events::with_capacity(1),
            serial: None,
            device: Arc::new(Mutex::new("".to_owned())), // Initialize as shared
            ports: vec![],
            tempdir: tempfile::Builder::new().prefix("rustymeter").tempdir().ok(),
            settings_open: false,
            macros_open: false,
            selected_macro_id: None,
            macro_recording: false,
            macro_record_buffer: String::new(),
            applied_idn: None,
            poll_ready: Arc::new(AtomicBool::new(false)),
            scpi_macros: vec![],
            is_init: false,
            ratecmd: RateCmd::default(),
            curr_rate: 0,
            rangecmd: Some(RangeCmd::default()),
            curr_range: 0,
            reverse_graph: false, // Default to right-to-left (most recent on right)
            graph_line_color: Color32::from_rgb(0, 255, 255), // Default to cyan (#00FFFF)
            hist_bar_color: Color32::from_rgb(0, 255, 255), // Default to cyan (#00FFFF)
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
            status_rx: None,
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_rx: None,
            #[cfg(not(target_arch = "wasm32"))]
            victor_lcd_display: String::new(),
            poll_interval_ms: 20,
            graph_update_interval_ms: 20, // Default to 20ms for ~50 FPS
            graph_update_interval_max: 1000, // Default maximum of 1000ms
            beeper_enabled: true,         // Default to on, per meter spec
            rst_on_disconnect: true,      // Default to on, can be disabled in settings
            cont_threshold: 50,           // Default continuity threshold: 50 ohms
            diod_threshold: 2.0,          // Default diode threshold: 2.0 volts (mid-range)
            lock_remote: true,            // Default to locking remote mode
            value_debug_shared: Arc::new(Mutex::new(false)),
            poll_interval_shared: Arc::new(Mutex::new(20)),
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_function:
                crate::victor_86bcd_capture::Victor86bcdCaptureFunction::default(),
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_unit: crate::victor_86bcd_capture::Victor86bcdCaptureUnit::default(
            ),
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_dp_mode:
                crate::victor_86bcd_capture::Victor86bcdCaptureDpMode::default(),
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_digits: [crate::victor_86bcd_capture::LcdDigit::Off; 4],
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_dp_after: None,
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_notes: String::new(),
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_duration_ms: 1000,
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_tx: None,
            #[cfg(not(target_arch = "wasm32"))]
            victor_86bcd_capture_status_shared: Arc::new(Mutex::new(
                crate::victor_86bcd_capture::Victor86bcdCaptureStatus::default(),
            )),
            last_graph_update: 0.0,                          // Initialize to 0
            last_hist_collect_time: 0.0,                     // Initialize to 0
            connection_state: ConnectionState::Disconnected, // Initially disconnected
            connection_error: None,                          // No error initially
            meas_count: 0,                                   // Initialize measurement counter
            last_record_time: 0.0,                           // Initialize last recording time
            graph_config: graph::GraphConfig::default(),     // Default graph config
            plot_dock_state: DockState::new(vec![]), // Initialize empty, populated in update
            mode_display_settings: HashMap::default(),
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
            app.poll_ready.store(false, Ordering::SeqCst);
            return app;
        }

        let app = Self::default();
        *app.value_debug_shared.lock().unwrap() = app.value_debug;
        *app.poll_interval_shared.lock().unwrap() = app.poll_interval_ms;
        app
    }

    fn queue_scpi(&mut self, cmd: impl Into<String>, record: bool) {
        let cmd = ensure_newline(&cmd.into());
        if cmd.trim().is_empty() {
            return;
        }
        if record && self.macro_recording && is_recordable_scpi(&cmd) {
            self.macro_record_buffer.push_str(cmd.trim_end());
            self.macro_record_buffer.push('\n');
        }
        let Some(tx) = self.serial_tx.as_ref() else {
            return;
        };
        let value_debug = self.value_debug;
        match tx.try_send(cmd.clone()) {
            Ok(()) => {
                if value_debug {
                    println!("Command queued: {}", cmd.trim_end());
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(pending)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = tx.send(pending).await {
                        if value_debug {
                            println!("Failed to queue command: {}", e);
                        }
                    }
                });
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                if value_debug {
                    println!("Failed to queue command: serial task closed");
                }
            }
        }
    }

    fn run_macro_body(&mut self, body: &str, record: bool) {
        let parsed = parse_macro_body(body);
        if self.value_debug && !parsed.skipped_queries.is_empty() {
            println!("SCPI macro skipped queries: {:?}", parsed.skipped_queries);
        }
        for cmd in &parsed.commands {
            self.queue_scpi(cmd.clone(), record);
        }
        self.apply_scpi_hints(&parsed.commands);
        self.queue_ui_refresh();
    }

    fn apply_scpi_hints(&mut self, cmds: &[String]) {
        for cmd in cmds {
            if let Some(hint) = ui_hint_from_command(cmd) {
                self.apply_ui_hint(hint);
            }
        }
    }

    fn apply_ui_hint(&mut self, hint: ScpiUiHint) {
        match hint {
            ScpiUiHint::Mode { mode, range_param } => {
                self.adopt_mode(mode, None);
                if let Some(param) = range_param {
                    if let Some(idx) = self
                        .rangecmd
                        .as_ref()
                        .and_then(|r| r.index_of_param(&param))
                    {
                        self.curr_range = idx;
                    }
                }
            }
            ScpiUiHint::Rate(code) => {
                if let Some(idx) = self.ratecmd.index_of_scpi(&code) {
                    self.curr_rate = idx;
                }
            }
            ScpiUiHint::Beep(on) => self.beeper_enabled = on,
            ScpiUiHint::ContThreshold(v) => self.cont_threshold = v,
            ScpiUiHint::DiodThreshold(v) => self.diod_threshold = v,
        }
    }

    fn queue_ui_refresh(&mut self) {
        for q in ui_refresh_queries() {
            self.queue_scpi(q, false);
        }
    }

    fn apply_meter_status(&mut self, status: MeterStatus) {
        match status {
            MeterStatus::Rate(code) => {
                if let Some(idx) = self.ratecmd.index_of_scpi(&code) {
                    self.curr_rate = idx;
                }
            }
            MeterStatus::Beep(on) => self.beeper_enabled = on,
            MeterStatus::AutoRange(auto) => {
                if auto {
                    self.curr_range = 0;
                }
            }
        }
    }

    fn bootstrap_settings(&self) -> BootstrapSettings {
        BootstrapSettings {
            rate_opt: self.ratecmd.get_opt(self.curr_rate).1.to_owned(),
            beeper_enabled: self.beeper_enabled,
            cont_threshold: self.cont_threshold,
            diod_threshold: self.diod_threshold,
            lock_remote: self.lock_remote,
        }
    }

    fn apply_connect_sequence(&mut self, idn: &str) {
        if !looks_like_idn(idn) {
            return;
        }
        let family = classify_idn(idn);
        self.curr_meter = range_table_meter(idn);
        let bootstrap = bootstrap_commands(family, &self.bootstrap_settings());
        if self.value_debug {
            println!("IDN {idn:?} -> {family:?}, bootstrap: {bootstrap:?}");
        }
        for cmd in bootstrap {
            self.queue_scpi(cmd, false);
        }
        let matching: Vec<String> = self
            .scpi_macros
            .iter()
            .filter(|m| m.run_on_connect && m.applies_to.matches(idn))
            .map(|m| m.body.clone())
            .collect();
        for body in matching {
            let parsed = parse_macro_body(&body);
            for cmd in &parsed.commands {
                self.queue_scpi(cmd.clone(), false);
            }
            self.apply_scpi_hints(&parsed.commands);
        }
        self.queue_ui_refresh();
        self.poll_ready.store(true, Ordering::SeqCst);
    }

    fn current_setup_scpi(&self) -> String {
        let mut lines = Vec::new();
        let conf = if let Some(rangecmd) = &self.rangecmd {
            rangecmd.gen_scpi(rangecmd.get_opt(self.curr_range).0)
        } else {
            self.metermode.default_conf().to_owned()
        };
        if !conf.trim().is_empty() {
            lines.push(conf);
        }
        lines.push(
            self.ratecmd
                .gen_scpi(self.ratecmd.get_opt(self.curr_rate).0),
        );
        if self.metermode == MeterMode::Cont || self.metermode == MeterMode::Diod {
            lines.push(if self.beeper_enabled {
                "SYST:BEEP:STATe ON\n".to_owned()
            } else {
                "SYST:BEEP:STATe OFF\n".to_owned()
            });
            if self.metermode == MeterMode::Cont {
                lines.push(format!("CONT:THREshold {}\n", self.cont_threshold));
            } else {
                lines.push(format!("DIOD:THREshold {}\n", self.diod_threshold));
            }
        }
        lines.concat()
    }

    fn finish_macro_recording(&mut self) {
        self.macro_recording = false;
        let body = self.macro_record_buffer.trim().to_owned();
        self.macro_record_buffer.clear();
        if body.is_empty() {
            return;
        }
        let mut recorded = ScpiMacro::new("Recorded");
        recorded.body = if body.ends_with('\n') {
            body
        } else {
            format!("{body}\n")
        };
        recorded.applies_to = self.default_macro_target();
        recorded.show_as_button = true;
        recorded.run_on_connect = false;
        self.selected_macro_id = Some(recorded.id.clone());
        self.scpi_macros.push(recorded);
        self.macros_open = true;
    }

    fn default_macro_target(&self) -> MacroTarget {
        let idn = self.device.lock().unwrap().clone();
        let model = idn_model(&idn);
        if !model.is_empty() {
            MacroTarget::Model(model)
        } else {
            MacroTarget::OwonMeas
        }
    }

    fn adopt_mode(&mut self, mode: MeterMode, unit: Option<&str>) {
        if mode == self.metermode {
            if let Some(unit) = unit
                && unit != self.curr_unit
            {
                self.curr_unit = unit.to_owned();
            }
            return;
        }
        self.metermode = mode;
        self.curr_unit = unit.unwrap_or(mode.default_unit()).to_owned();
        self.values = VecDeque::with_capacity(self.mem_depth);
        self.hist_values = VecDeque::with_capacity(self.hist_mem_depth);
        self.rangecmd = if self.is_read_only() {
            None
        } else {
            RangeCmd::new(&self.curr_meter, mode)
        };
        self.curr_range = 0;
    }

    fn set_mode(&mut self, mode: MeterMode) {
        self.adopt_mode(mode, None);
        let cmd = mode.default_conf();
        self.confstring = cmd.to_owned();
        if !cmd.is_empty() {
            self.queue_scpi(cmd, true);
        }
        if mode.with_beeper_threshold() {
            self.queue_scpi(
                if self.beeper_enabled {
                    "SYST:BEEP:STATe ON\n"
                } else {
                    "SYST:BEEP:STATe OFF\n"
                },
                true,
            );
            let threshold_cmd = if mode == MeterMode::Cont {
                format!("CONT:THREshold {}\n", self.cont_threshold)
            } else {
                format!("DIOD:THREshold {}\n", self.diod_threshold)
            };
            self.queue_scpi(threshold_cmd, true);
        }
    }

    // Method to handle disconnection
    fn disconnect(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(()); // Signal the serial task to shut down
        }
        self.serial_tx = None; // Drop sender to stop sending commands
        self.serial_rx = None; // Drop receiver to stop receiving measurements
        self.mode_rx = None; // Drop mode receiver
        self.status_rx = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.victor_86bcd_rx = None;
            self.victor_lcd_display.clear();
            self.victor_86bcd_capture_tx = None;
        }
        self.serial = None; // Clear serial port
        self.connection_state = ConnectionState::Disconnected;
        self.connection_error = None; // Clear any previous error
        let mut device = self.device.lock().unwrap();
        *device = "".to_owned(); // Clear device string
        drop(device);
        self.applied_idn = None;
        self.poll_ready.store(false, Ordering::SeqCst);
        self.macro_recording = false;
        self.curr_meas = f64::NAN; // Reset measurement
        self.values.clear(); // Clear graph data
        self.hist_values.clear(); // Clear histogram data
        self.meas_count = 0; // Reset measurement counter
    }

    pub fn auto_scale_units(&self, mode: &MeterMode) -> bool {
        self.mode_display_settings
            .get(mode)
            .is_none_or(|s| s.auto_scale_units) // default = enabled
    }

    pub fn set_auto_scale_units(&mut self, mode: MeterMode, enabled: bool) {
        self.mode_display_settings
            .entry(mode)
            .or_default()
            .auto_scale_units = enabled;
        // Optional: self.save_settings() if you have an immediate-save helper
    }

    pub fn is_read_only(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            matches!(
                self.connection_type,
                ConnectionType::VictorHid
                    | ConnectionType::Victor86bcdSerial
                    | ConnectionType::Victor86eSerial
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn is_victor_connection(&self) -> bool {
        matches!(
            self.connection_type,
            ConnectionType::VictorHid
                | ConnectionType::Victor86bcdSerial
                | ConnectionType::Victor86eSerial
        )
    }

    /// Whether a mode button should appear in the control panel for the current connection.
    pub fn mode_visible_in_ui(&self, mode: MeterMode) -> bool {
        match mode {
            MeterMode::Duty => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.is_victor_connection()
                }
                #[cfg(target_arch = "wasm32")]
                {
                    false
                }
            }
            MeterMode::Per => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.connection_type == ConnectionType::ScpiSerial
                }
                #[cfg(target_arch = "wasm32")]
                {
                    true
                }
            }
            _ => true,
        }
    }
}
