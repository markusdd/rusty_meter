//! In-process agent bridge. The UI thread publishes a snapshot and applies
//! typed commands. A localhost socket serves that snapshot to `rusty-meter-mcp`.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock, mpsc};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::agent_proto::{self, LineRequest, LineResponse};
use crate::helpers::is_meter_overload;
use crate::multimeter::{GenScpi, MeterMode};

use super::{ConnectionState, ConnectionType};

const HISTORY_CAP: usize = 4000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const TRACE_NOTE: &str = "t_ms is unix time in milliseconds. DMM points are appended on the graph interval. PSU points are appended on each measurement, and the three channels stay aligned. trace matches the on-screen graph and clears when the mode changes or the graph is reset. history keeps samples across mode changes and clears on disconnect.";

#[derive(Clone, Debug, Serialize)]
struct AgentChannel {
    name: String,
    unit: String,
    value: Option<f64>,
    overload: bool,
}

#[derive(Clone, Debug, Serialize)]
struct AgentStatus {
    name: String,
    value: String,
}

#[derive(Clone, Debug, Serialize)]
struct AgentSample {
    t_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<u64>,
    channels: Vec<AgentChannel>,
    status: Vec<AgentStatus>,
}

#[derive(Clone, Debug, Serialize)]
struct PsuInfo {
    model: String,
    v_max: f32,
    i_max: f32,
    p_max: f32,
    ovp_max: f32,
    ocp_max: f32,
    set_v: f32,
    set_i: f32,
    ovp: f32,
    ocp: f32,
    output_on: bool,
    run: String,
    ovp_fault: bool,
    ocp_fault: bool,
    otp_fault: bool,
    editing_voltage: bool,
    editing_current: bool,
    editing_ovp: bool,
    editing_ocp: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
struct SessionInfo {
    connected: bool,
    connection: String,
    read_only: bool,
    control_enabled: bool,
    listening: bool,
    endpoint: String,
    idn: String,
    meter: String,
    is_psu: bool,
    mode: String,
    mode_label: String,
    unit: String,
    rate: String,
    rate_scpi: String,
    range: String,
    auto_range: bool,
    recording: bool,
    recording_samples: usize,
    graph_interval_ms: u64,
    poll_interval_ms: u64,
    trace_samples: usize,
    history_samples: usize,
    last_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bind_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    psu: Option<PsuInfo>,
    note: String,
}

#[derive(Default)]
pub(super) struct Published {
    session: SessionInfo,
    reading: Option<AgentSample>,
    trace: VecDeque<AgentSample>,
    history: VecDeque<AgentSample>,
    trace_spacing: String,
    trace_interval_ms: u64,
    ctx: Option<egui::Context>,
}

enum Command {
    GetRecording { limit: usize },
    SetMode { mode: String },
    SetRange { range: String },
    SetRate { rate: String },
    SetPsuVoltage { volts: f32 },
    SetPsuCurrent { amps: f32 },
    SetPsuLimits { ovp: Option<f32>, ocp: Option<f32> },
    SetOutput { on: bool },
    RunMacro { name: String },
}

pub(super) struct Pending {
    command: Command,
    reply: oneshot::Sender<Result<Value, String>>,
}

/// Sends the shutdown signal when dropped, including when the window closes.
pub(super) struct ShutdownOnDrop {
    tx: Option<oneshot::Sender<()>>,
}

impl ShutdownOnDrop {
    fn new(tx: oneshot::Sender<()>) -> Self {
        Self { tx: Some(tx) }
    }

    fn signal(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.signal();
    }
}

#[derive(Default)]
pub(super) enum AgentServer {
    #[default]
    Idle,
    Starting {
        port: u16,
        ready: oneshot::Receiver<Result<(), String>>,
        shutdown: ShutdownOnDrop,
    },
    Running {
        port: u16,
        _shutdown: ShutdownOnDrop,
    },
}

impl super::MyApp {
    pub(super) fn ensure_agent_io(&mut self) {
        if self.agent_cmd_tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.agent_cmd_tx = Some(tx);
        self.agent_cmd_rx = Some(rx);
        self.agent_published = Some(Arc::new(RwLock::new(Published::default())));
    }

    pub(super) fn poll_agent(&mut self, ctx: &egui::Context) {
        self.ensure_agent_io();
        self.publish_agent(Some(ctx));
        self.sync_agent_server();
        self.drain_agent_commands();
        self.publish_agent(Some(ctx));
    }

    pub(super) fn stop_agent_server(&mut self) {
        // Dropping the server signals the task, including when the window closes.
        self.agent_server = AgentServer::Idle;
    }

    /// Append one timestamped sample for the current live reading.
    /// Called from the same sites that push the on-screen graph.
    pub(super) fn observe_live_sample(&mut self) {
        let Some(sample) = self.live_sample() else {
            return;
        };
        let Some(published) = &self.agent_published else {
            return;
        };
        let mut guard = published.write().unwrap();
        guard.trace.push_back(sample.clone());
        while guard.trace.len() > self.mem_depth {
            guard.trace.pop_front();
        }
        guard.history.push_back(sample);
        while guard.history.len() > HISTORY_CAP {
            guard.history.pop_front();
        }
    }

    pub(super) fn clear_agent_trace(&mut self) {
        let Some(published) = &self.agent_published else {
            return;
        };
        let mut guard = published.write().unwrap();
        guard.trace.clear();
        guard.reading = None;
    }

    pub(super) fn clear_agent_history(&mut self) {
        let Some(published) = &self.agent_published else {
            return;
        };
        published.write().unwrap().history.clear();
    }

    pub(super) fn show_agent_banner(&self, ui: &mut egui::Ui) {
        if !self.agent_listen {
            return;
        }
        ui.separator();
        let (text, tone) = if let Some(err) = &self.agent_bind_error {
            (format!("Agent: {err}"), BannerTone::Error)
        } else if let AgentServer::Running { port, .. } = &self.agent_server {
            let mode = if self.agent_control {
                "control on"
            } else {
                "read only"
            };
            let text = if self.agent_last_action.is_empty() {
                format!("Agent 127.0.0.1:{port} ({mode})")
            } else {
                format!(
                    "Agent 127.0.0.1:{port} ({mode}) — {}",
                    shorten(&self.agent_last_action, 80)
                )
            };
            let tone = if self.agent_control {
                BannerTone::Control
            } else {
                BannerTone::Idle
            };
            (text, tone)
        } else {
            ("Agent starting…".to_owned(), BannerTone::Idle)
        };
        let color = match tone {
            BannerTone::Control => egui::Color32::from_rgb(255, 176, 46),
            BannerTone::Error => egui::Color32::from_rgb(220, 80, 80),
            BannerTone::Idle => egui::Color32::GRAY,
        };
        ui.colored_label(color, text)
            .on_hover_text("The agent uses the same controls as this window.");
    }

    pub(super) fn show_agent_window(&mut self, ctx: &egui::Context) {
        if !self.agent_open {
            return;
        }
        egui::Window::new("Agent")
            .auto_sized()
            .resizable(false)
            .show(ctx, |ui| {
                self.agent_window_contents(ui);
                if ui.button("Close").clicked() {
                    self.agent_open = false;
                }
            });
    }

    fn agent_window_contents(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.agent_listen, "Listen for an agent on 127.0.0.1")
            .on_hover_text("Serves live readings and traces. Nothing listens until this is on.");
        ui.horizontal(|ui| {
            ui.label("Port:");
            ui.add(egui::DragValue::new(&mut self.agent_port).range(1024..=65535));
        });
        ui.checkbox(
            &mut self.agent_control,
            "Allow agent control (mode, range, PSU, macros)",
        )
        .on_hover_text("Writes use the same commands as the buttons and sliders. Output and voltage changes are real.");
        ui.label(format!(
            "MCP process: rusty-meter-mcp. Endpoint: 127.0.0.1:{}. Override with RUSTY_METER_AGENT.",
            self.agent_port
        ));
        ui.label("Reads work while listening. Control is required before an agent can change the instrument.");
        if let Some(err) = &self.agent_bind_error {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
        }
    }

    fn drain_agent_commands(&mut self) {
        let Some(rx) = self.agent_cmd_rx.as_ref() else {
            return;
        };
        let pending: Vec<Pending> = rx.try_iter().collect();
        for pending in pending {
            if pending.reply.is_closed() {
                continue;
            }
            let result = self.apply_agent_command(pending.command);
            self.publish_agent(None);
            let _ = pending.reply.send(result);
        }
    }

    fn apply_agent_command(&mut self, command: Command) -> Result<Value, String> {
        match command {
            Command::GetRecording { limit } => self.agent_recording(limit),
            Command::SetMode { mode } => self.agent_set_mode(&mode),
            Command::SetRange { range } => self.agent_set_range(&range),
            Command::SetRate { rate } => self.agent_set_rate(&rate),
            Command::SetPsuVoltage { volts } => self.agent_set_psu_voltage(volts),
            Command::SetPsuCurrent { amps } => self.agent_set_psu_current(amps),
            Command::SetPsuLimits { ovp, ocp } => self.agent_set_psu_limits(ovp, ocp),
            Command::SetOutput { on } => self.agent_set_output(on),
            Command::RunMacro { name } => self.agent_run_macro(&name),
        }
    }

    fn agent_recording(&self, limit: usize) -> Result<Value, String> {
        let samples: Vec<AgentSample> = self
            .recording_data
            .iter()
            .rev()
            .take(limit)
            .map(|record| AgentSample {
                t_ms: record.timestamp.timestamp_millis(),
                index: Some(record.index as u64),
                channels: record
                    .channels
                    .iter()
                    .map(|ch| {
                        let overload =
                            !self.scpi_is_psu && is_meter_overload(ch.value, self.metermode);
                        finite_channel(&ch.name, &ch.unit, ch.value, overload)
                    })
                    .collect(),
                status: record
                    .status
                    .iter()
                    .map(|st| AgentStatus {
                        name: st.name.clone(),
                        value: st.value.clone(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(json!({
            "active": self.recording_active,
            "samples": samples,
            "note": "Rows come from the recorder. DMM overload uses the mode selected now, so a row captured in another mode can be mislabeled. history keeps the flag from the moment the sample was taken.",
        }))
    }

    fn agent_set_mode(&mut self, mode_name: &str) -> Result<Value, String> {
        self.require_meter_write()?;
        let mode = MeterMode::from_agent_name(mode_name)
            .ok_or_else(|| format!("unknown mode '{mode_name}'"))?;
        if !self.mode_visible_in_ui(mode) {
            return Err(format!(
                "{} is not available on this instrument",
                mode.agent_name()
            ));
        }
        let changed = mode != self.metermode;
        self.set_mode(mode);
        self.agent_last_action = format!("set mode {}", mode.agent_name());
        self.request_ui_refresh();
        Ok(json!({
            "mode": mode.agent_name(),
            "trace_cleared": changed,
        }))
    }

    fn agent_set_range(&mut self, range: &str) -> Result<Value, String> {
        self.require_meter_write()?;
        let idx = self
            .rangecmd
            .as_ref()
            .ok_or_else(|| "this mode has no range selection".to_owned())
            .and_then(|rangecmd| {
                rangecmd
                    .index_of_param(range)
                    .ok_or_else(|| format!("unknown range '{range}'"))
            })?;
        self.curr_range = idx;
        self.meter_auto_range = idx == 0;
        let cmd = self
            .rangecmd
            .as_ref()
            .map(|rangecmd| rangecmd.gen_scpi(rangecmd.get_opt(idx).0))
            .unwrap_or_default();
        self.confstring.clone_from(&cmd);
        self.queue_scpi(cmd, true);
        let label = self.range_label();
        self.agent_last_action = format!("set range {label}");
        self.request_ui_refresh();
        Ok(json!({
            "range": label,
            "auto_range": self.meter_auto_range,
        }))
    }

    fn agent_set_rate(&mut self, rate: &str) -> Result<Value, String> {
        self.require_meter_write()?;
        let wanted = rate.trim();
        let idx = self.ratecmd.index_of_scpi(wanted).or_else(|| {
            (0..self.ratecmd.len())
                .find(|&i| self.ratecmd.get_opt(i).0.eq_ignore_ascii_case(wanted))
        });
        let Some(idx) = idx else {
            return Err(format!("unknown rate '{rate}'"));
        };
        self.curr_rate = idx;
        let (label, code) = self.ratecmd.get_opt(idx);
        let cmd = self.ratecmd.gen_scpi(label);
        self.confstring.clone_from(&cmd);
        self.queue_scpi(cmd, true);
        self.agent_last_action = format!("set rate {label}");
        self.request_ui_refresh();
        Ok(json!({
            "rate": label,
            "rate_scpi": code,
        }))
    }

    fn agent_set_psu_voltage(&mut self, volts: f32) -> Result<Value, String> {
        self.require_psu_write()?;
        if !volts.is_finite() {
            return Err("voltage must be a finite number".into());
        }
        if self.psu_edit.set_v {
            return Err("voltage is being edited in the window".into());
        }
        let applied = volts.clamp(0.0, self.psu_model.v_max);
        self.psu.set_v = applied;
        self.psu_plot_v = applied;
        self.queue_scpi(crate::psu::set_volt(applied), true);
        self.agent_last_action = format!("set voltage {applied:.3} V");
        self.request_ui_refresh();
        Ok(json!({
            "requested": volts,
            "applied": applied,
            "v_max": self.psu_model.v_max,
        }))
    }

    fn agent_set_psu_current(&mut self, amps: f32) -> Result<Value, String> {
        self.require_psu_write()?;
        if !amps.is_finite() {
            return Err("current must be a finite number".into());
        }
        if self.psu_edit.set_i {
            return Err("current is being edited in the window".into());
        }
        let applied = amps.clamp(0.0, self.psu_model.i_max);
        self.psu.set_i = applied;
        self.psu_plot_i = applied;
        self.queue_scpi(crate::psu::set_curr(applied), true);
        self.agent_last_action = format!("set current {applied:.3} A");
        self.request_ui_refresh();
        Ok(json!({
            "requested": amps,
            "applied": applied,
            "i_max": self.psu_model.i_max,
        }))
    }

    fn agent_set_psu_limits(
        &mut self,
        ovp: Option<f32>,
        ocp: Option<f32>,
    ) -> Result<Value, String> {
        self.require_psu_write()?;
        if ovp.is_none() && ocp.is_none() {
            return Err("pass ovp, ocp, or both".into());
        }
        if ovp.is_some_and(|v| !v.is_finite()) || ocp.is_some_and(|v| !v.is_finite()) {
            return Err("limits must be finite numbers".into());
        }
        if ovp.is_some() && self.psu_edit.ovp {
            return Err("OVP is being edited in the window".into());
        }
        if ocp.is_some() && self.psu_edit.ocp {
            return Err("OCP is being edited in the window".into());
        }
        let mut applied_ovp = None;
        let mut applied_ocp = None;
        if let Some(volts) = ovp {
            let applied = volts.clamp(0.0, self.psu_model.ovp_max());
            self.psu.ovp = applied;
            self.queue_scpi(crate::psu::set_ovp(applied), true);
            applied_ovp = Some(applied);
        }
        if let Some(amps) = ocp {
            let applied = amps.clamp(0.0, self.psu_model.ocp_max());
            self.psu.ocp = applied;
            self.queue_scpi(crate::psu::set_ocp(applied), true);
            applied_ocp = Some(applied);
        }
        self.agent_last_action = match (applied_ovp, applied_ocp) {
            (Some(v), Some(i)) => format!("set limits OVP {v:.3} V, OCP {i:.3} A"),
            (Some(v), None) => format!("set OVP {v:.3} V"),
            (None, Some(i)) => format!("set OCP {i:.3} A"),
            (None, None) => "set limits".to_owned(),
        };
        self.request_ui_refresh();
        Ok(json!({
            "ovp": applied_ovp,
            "ocp": applied_ocp,
        }))
    }

    fn agent_set_output(&mut self, on: bool) -> Result<Value, String> {
        self.require_psu_write()?;
        self.psu.output_on = on;
        self.queue_scpi(crate::psu::set_output(on), true);
        self.agent_last_action = format!("output {}", if on { "ON" } else { "OFF" });
        self.request_ui_refresh();
        Ok(json!({ "output_on": on }))
    }

    fn agent_run_macro(&mut self, name: &str) -> Result<Value, String> {
        self.require_write()?;
        if self.is_read_only() {
            return Err("this connection is read-only".into());
        }
        let wanted = name.trim();
        if wanted.is_empty() {
            return Err("macro name is empty".into());
        }
        let idn = self.device.lock().unwrap().clone();
        let found = self.scpi_macros.iter().find(|macro_| {
            macro_.name.eq_ignore_ascii_case(wanted) && macro_.applies_to.matches(&idn)
        });
        let Some(found) = found else {
            return Err(format!(
                "no macro named '{wanted}' applies to the connected instrument"
            ));
        };
        let body = found.body.clone();
        let ran = found.name.clone();
        self.run_macro_body(&body, true);
        self.agent_last_action = format!("run macro {ran}");
        Ok(json!({ "name": ran }))
    }

    fn require_write(&self) -> Result<(), String> {
        if !self.agent_control {
            return Err("agent control is off. Enable Menu → Agent → Allow agent control.".into());
        }
        if self.connection_state != ConnectionState::Connected {
            return Err("not connected to an instrument".into());
        }
        Ok(())
    }

    fn require_meter_write(&self) -> Result<(), String> {
        self.require_write()?;
        if self.is_read_only() {
            return Err("this connection is read-only".into());
        }
        if self.scpi_is_psu {
            return Err(
                "connected instrument is a power supply. Use the set_psu and set_output tools."
                    .into(),
            );
        }
        Ok(())
    }

    fn require_psu_write(&self) -> Result<(), String> {
        self.require_write()?;
        if !self.scpi_is_psu {
            return Err("connected instrument is not a power supply".into());
        }
        Ok(())
    }

    fn publish_agent(&mut self, ctx: Option<&egui::Context>) {
        let mut session = self.session_info();
        let reading = self.live_sample();
        let plot_len = self.values.len();
        let spacing = if self.scpi_is_psu {
            "measurement"
        } else {
            "graph_interval"
        };
        let interval_ms = if self.scpi_is_psu {
            self.poll_interval_ms
        } else {
            self.graph_update_interval_ms
        };
        let listening = matches!(self.agent_server, AgentServer::Running { .. });
        let Some(published) = &self.agent_published else {
            return;
        };
        let mut guard = published.write().unwrap();
        while guard.trace.len() > plot_len {
            guard.trace.pop_front();
        }
        session.listening = listening;
        session.trace_samples = guard.trace.len();
        session.history_samples = guard.history.len();
        guard.session = session;
        guard.reading = reading;
        guard.trace_spacing = spacing.to_owned();
        guard.trace_interval_ms = interval_ms;
        if let Some(ctx) = ctx {
            guard.ctx = Some(ctx.clone());
        }
    }

    fn session_info(&self) -> SessionInfo {
        let (rate, rate_scpi) = if self.curr_rate < self.ratecmd.len() {
            let (label, code) = self.ratecmd.get_opt(self.curr_rate);
            (label.to_owned(), code.to_owned())
        } else {
            (String::new(), String::new())
        };
        let psu = self.scpi_is_psu.then(|| PsuInfo {
            model: self.psu_model.display_name(),
            v_max: self.psu_model.v_max,
            i_max: self.psu_model.i_max,
            p_max: self.psu_model.p_max,
            ovp_max: self.psu_model.ovp_max(),
            ocp_max: self.psu_model.ocp_max(),
            set_v: self.psu.set_v,
            set_i: self.psu.set_i,
            ovp: self.psu.ovp,
            ocp: self.psu.ocp,
            output_on: self.psu.output_on,
            run: self.psu.run.label().to_owned(),
            ovp_fault: self.psu.ovp_fault,
            ocp_fault: self.psu.ocp_fault,
            otp_fault: self.psu.otp_fault,
            editing_voltage: self.psu_edit.set_v,
            editing_current: self.psu_edit.set_i,
            editing_ovp: self.psu_edit.ovp,
            editing_ocp: self.psu_edit.ocp,
        });
        SessionInfo {
            connected: self.connection_state == ConnectionState::Connected,
            connection: self.connection_kind().to_owned(),
            read_only: self.is_read_only(),
            control_enabled: self.agent_control,
            listening: false,
            endpoint: format!("127.0.0.1:{}", self.agent_port),
            idn: self.device.lock().unwrap().clone(),
            meter: self.curr_meter.clone(),
            is_psu: self.scpi_is_psu,
            mode: self.metermode.agent_name().to_owned(),
            mode_label: self.metermode.button_label().to_owned(),
            unit: self.curr_unit.clone(),
            rate,
            rate_scpi,
            range: self.range_label(),
            auto_range: self.meter_auto_range,
            recording: self.recording_active,
            recording_samples: self.recording_data.len(),
            graph_interval_ms: self.graph_update_interval_ms,
            poll_interval_ms: self.poll_interval_ms,
            trace_samples: 0,
            history_samples: 0,
            last_action: self.agent_last_action.clone(),
            bind_error: self.agent_bind_error.clone(),
            psu,
            note: TRACE_NOTE.to_owned(),
        }
    }

    fn connection_kind(&self) -> &'static str {
        match self.connection_type {
            ConnectionType::ScpiSerial => "scpi_serial",
            #[cfg(not(target_arch = "wasm32"))]
            ConnectionType::VictorHid => "victor_hid",
            #[cfg(not(target_arch = "wasm32"))]
            ConnectionType::Victor86bcdSerial => "victor_86bcd_serial",
            #[cfg(not(target_arch = "wasm32"))]
            ConnectionType::Victor86eSerial => "victor_86e_serial",
        }
    }

    fn range_label(&self) -> String {
        if let Some(rangecmd) = &self.rangecmd
            && self.curr_range < rangecmd.len()
        {
            return rangecmd.get_opt(self.curr_range).0.to_owned();
        }
        if self.meter_auto_range {
            "auto".to_owned()
        } else {
            String::new()
        }
    }

    fn live_sample(&self) -> Option<AgentSample> {
        if self.connection_state != ConnectionState::Connected {
            return None;
        }
        if self.scpi_is_psu {
            psu_sample(self)
        } else {
            dmm_sample(self)
        }
    }

    fn sync_agent_server(&mut self) {
        self.poll_agent_bind();
        if !self.agent_listen {
            self.agent_bind_failed_port = None;
            self.agent_bind_error = None;
        }
        let current_port = match &self.agent_server {
            AgentServer::Running { port, .. } | AgentServer::Starting { port, .. } => Some(*port),
            AgentServer::Idle => None,
        };
        let want = self.agent_listen.then_some(self.agent_port);
        if current_port == want {
            return;
        }
        if want.is_some() && self.agent_bind_failed_port == want {
            return;
        }
        self.stop_agent_server();
        if let Some(port) = want {
            self.start_agent_server(port);
        }
    }

    fn poll_agent_bind(&mut self) {
        let state = std::mem::replace(&mut self.agent_server, AgentServer::Idle);
        self.agent_server = match state {
            AgentServer::Starting {
                port,
                mut ready,
                shutdown,
            } => match ready.try_recv() {
                Ok(Ok(())) => {
                    log::info!("agent endpoint listening on 127.0.0.1:{port}");
                    self.agent_bind_error = None;
                    self.agent_bind_failed_port = None;
                    AgentServer::Running {
                        port,
                        _shutdown: shutdown,
                    }
                }
                Ok(Err(err)) => {
                    self.agent_bind_error = Some(err);
                    self.agent_bind_failed_port = Some(port);
                    AgentServer::Idle
                }
                Err(oneshot::error::TryRecvError::Empty) => AgentServer::Starting {
                    port,
                    ready,
                    shutdown,
                },
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.agent_bind_error = Some("agent server stopped before it bound".into());
                    self.agent_bind_failed_port = Some(port);
                    AgentServer::Idle
                }
            },
            other => other,
        };
    }

    fn start_agent_server(&mut self, port: u16) {
        let Some(published) = self.agent_published.clone() else {
            return;
        };
        let Some(cmd_tx) = self.agent_cmd_tx.clone() else {
            return;
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(async move {
            let listener = match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => listener,
                Err(err) => {
                    let _ = ready_tx.send(Err(format!("could not bind 127.0.0.1:{port}: {err}")));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            serve(listener, shutdown_rx, published, cmd_tx).await;
        });
        self.agent_server = AgentServer::Starting {
            port,
            ready: ready_rx,
            shutdown: ShutdownOnDrop::new(shutdown_tx),
        };
        self.agent_bind_error = None;
    }
}

enum BannerTone {
    Idle,
    Control,
    Error,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn finite_channel(name: &str, unit: &str, value: f64, overload: bool) -> AgentChannel {
    AgentChannel {
        name: name.to_owned(),
        unit: unit.to_owned(),
        value: value.is_finite().then_some(value),
        overload,
    }
}

fn status(name: &str, value: &str) -> AgentStatus {
    AgentStatus {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn dmm_sample(app: &super::MyApp) -> Option<AgentSample> {
    if app.curr_meas.is_nan() {
        return None;
    }
    let overload = is_meter_overload(app.curr_meas, app.metermode);
    Some(AgentSample {
        t_ms: now_ms(),
        index: None,
        channels: vec![finite_channel(
            "value",
            &app.curr_unit,
            app.curr_meas,
            overload,
        )],
        status: vec![
            status("mode", app.metermode.agent_name()),
            status("range", &app.range_label()),
            status(
                "auto_range",
                if app.meter_auto_range {
                    "true"
                } else {
                    "false"
                },
            ),
        ],
    })
}

fn psu_sample(app: &super::MyApp) -> Option<AgentSample> {
    let volt = app.psu.meas_v;
    let curr = app.psu.meas_i;
    let power = app.psu.meas_p;
    if !volt.is_finite() && !curr.is_finite() && !power.is_finite() {
        return None;
    }
    let bit = |on: bool| if on { "1" } else { "0" };
    Some(AgentSample {
        t_ms: now_ms(),
        index: None,
        channels: vec![
            finite_channel("V", "V", volt, false),
            finite_channel("I", "A", curr, false),
            finite_channel("P", "W", power, false),
        ],
        status: vec![
            status("output", if app.psu.output_on { "ON" } else { "OFF" }),
            status("mode", app.psu.run.label()),
            status("ovp", bit(app.psu.ovp_fault)),
            status("ocp", bit(app.psu.ocp_fault)),
            status("otp", bit(app.psu.otp_fault)),
        ],
    })
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn clamp_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, HISTORY_CAP)
}

fn tail(samples: &VecDeque<AgentSample>, limit: usize) -> Vec<AgentSample> {
    let skip = samples.len().saturating_sub(limit);
    samples.iter().skip(skip).cloned().collect()
}

async fn serve(
    listener: TcpListener,
    mut shutdown: oneshot::Receiver<()>,
    published: Arc<RwLock<Published>>,
    cmd_tx: mpsc::Sender<Pending>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { break; };
                let published = published.clone();
                let cmd_tx = cmd_tx.clone();
                tokio::spawn(async move {
                    let _ = handle_socket(socket, published, cmd_tx).await;
                });
            }
        }
    }
}

async fn handle_socket(
    socket: tokio::net::TcpStream,
    published: Arc<RwLock<Published>>,
    cmd_tx: mpsc::Sender<Pending>,
) -> std::io::Result<()> {
    let _ = socket.set_nodelay(true);
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let read = tokio::time::timeout(COMMAND_TIMEOUT, reader.read_line(&mut line)).await;
    let response = match read {
        Ok(Ok(0)) | Err(_) => LineResponse::failure(0, "empty agent request").to_line(),
        Ok(Ok(_)) if line.len() > 65_536 => {
            LineResponse::failure(0, "agent request is too large").to_line()
        }
        Ok(Ok(_)) => match agent_proto::parse_request(&line) {
            Ok(request) => handle_line(request, &published, &cmd_tx).await.to_line(),
            Err(err) => LineResponse::failure(0, err).to_line(),
        },
        Ok(Err(err)) => LineResponse::failure(0, err.to_string()).to_line(),
    };
    let _ = tokio::time::timeout(COMMAND_TIMEOUT, write_half.write_all(response.as_bytes())).await;
    Ok(())
}

async fn handle_line(
    request: LineRequest,
    published: &Arc<RwLock<Published>>,
    cmd_tx: &mpsc::Sender<Pending>,
) -> LineResponse {
    let limit = clamp_limit(request.limit, default_limit(&request.op));
    let read_result = match request.op.as_str() {
        "get_session" => Some(read_session(published)),
        "get_reading" => Some(read_reading(published)),
        "get_trace" => Some(read_series(published, limit, true)),
        "get_history" => Some(read_series(published, limit, false)),
        _ => None,
    };
    if let Some(result) = read_result {
        return match result {
            Ok(value) => LineResponse::success(request.id, value),
            Err(err) => LineResponse::failure(request.id, err),
        };
    }
    let command = match request.op.as_str() {
        "get_recording" => Command::GetRecording { limit },
        "set_mode" => Command::SetMode {
            mode: request.mode.unwrap_or_default(),
        },
        "set_range" => Command::SetRange {
            range: request.range.unwrap_or_default(),
        },
        "set_rate" => Command::SetRate {
            rate: request.rate.unwrap_or_default(),
        },
        "set_psu_voltage" => match request.volts {
            Some(volts) => Command::SetPsuVoltage { volts },
            None => return LineResponse::failure(request.id, "volts is required"),
        },
        "set_psu_current" => match request.amps {
            Some(amps) => Command::SetPsuCurrent { amps },
            None => return LineResponse::failure(request.id, "amps is required"),
        },
        "set_psu_limits" => Command::SetPsuLimits {
            ovp: request.ovp,
            ocp: request.ocp,
        },
        "set_output" => match request.on {
            Some(on) => Command::SetOutput { on },
            None => return LineResponse::failure(request.id, "on is required"),
        },
        "run_macro" => Command::RunMacro {
            name: request.name.unwrap_or_default(),
        },
        other => {
            return LineResponse::failure(request.id, format!("unknown op '{other}'"));
        }
    };
    match submit_command(published, cmd_tx, command).await {
        Ok(value) => LineResponse::success(request.id, value),
        Err(err) => LineResponse::failure(request.id, err),
    }
}

fn default_limit(op: &str) -> usize {
    if op == "get_recording" { 50 } else { 200 }
}

fn read_session(published: &RwLock<Published>) -> Result<Value, String> {
    let guard = published.read().unwrap();
    serde_json::to_value(&guard.session).map_err(|err| err.to_string())
}

fn read_reading(published: &RwLock<Published>) -> Result<Value, String> {
    let guard = published.read().unwrap();
    Ok(json!({ "reading": guard.reading }))
}

fn read_series(published: &RwLock<Published>, limit: usize, trace: bool) -> Result<Value, String> {
    let guard = published.read().unwrap();
    let samples = if trace {
        tail(&guard.trace, limit)
    } else {
        tail(&guard.history, limit)
    };
    Ok(json!({
        "spacing": guard.trace_spacing,
        "interval_ms": guard.trace_interval_ms,
        "cleared_on_mode_change": trace,
        "samples": samples,
        "note": TRACE_NOTE,
    }))
}

async fn submit_command(
    published: &Arc<RwLock<Published>>,
    cmd_tx: &mpsc::Sender<Pending>,
    command: Command,
) -> Result<Value, String> {
    let (reply_tx, reply_rx) = oneshot::channel();
    cmd_tx
        .send(Pending {
            command,
            reply: reply_tx,
        })
        .map_err(|_| "RustyMeter is not running".to_owned())?;
    if let Some(ctx) = published.read().unwrap().ctx.clone() {
        ctx.request_repaint();
    }
    match tokio::time::timeout(COMMAND_TIMEOUT, reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the window dropped the command".into()),
        Err(_) => Err("the window did not answer within 2s".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::METER_OVERLOAD_VALUE;
    use crate::psu::PsuModel;

    fn connected_app() -> super::super::MyApp {
        let mut app = super::super::MyApp::default();
        app.ensure_agent_io();
        app.connection_state = ConnectionState::Connected;
        app.agent_control = true;
        app.curr_meas = 1.25;
        app.curr_unit = "VDC".to_owned();
        app
    }

    #[test]
    fn mode_change_clears_trace_and_keeps_history() {
        let mut app = connected_app();
        app.values.push_back(1.25);
        app.observe_live_sample();
        let changed = app.agent_set_mode("Res").unwrap();
        app.publish_agent(None);
        assert_eq!(changed["mode"], "Res");
        assert_eq!(changed["trace_cleared"], true);
        assert!(app.values.is_empty());
        let published = app.agent_published.as_ref().unwrap().read().unwrap();
        assert!(published.trace.is_empty());
        assert_eq!(published.history.len(), 1);
        assert_eq!(published.history[0].status[0].value, "Vdc");
        assert!(!published.history[0].channels[0].overload);
    }

    #[test]
    fn ohms_sentinel_is_overload_and_volts_are_not() {
        let mut app = connected_app();
        app.metermode = MeterMode::Res;
        app.curr_meas = METER_OVERLOAD_VALUE;
        app.curr_unit = "Ohm".to_owned();
        app.values.push_back(app.curr_meas);
        app.observe_live_sample();
        {
            let published = app.agent_published.as_ref().unwrap().read().unwrap();
            assert!(published.history[0].channels[0].overload);
        }
        app.metermode = MeterMode::Freq;
        app.curr_unit = "Hz".to_owned();
        app.values.push_back(METER_OVERLOAD_VALUE);
        app.observe_live_sample();
        let published = app.agent_published.as_ref().unwrap().read().unwrap();
        assert!(!published.history[1].channels[0].overload);
    }

    #[test]
    fn control_off_rejects_mode_change() {
        let mut app = connected_app();
        app.agent_control = false;
        let err = app.agent_set_mode("Vac").unwrap_err();
        assert!(err.contains("agent control is off"));
        assert_eq!(app.metermode, MeterMode::Vdc);
    }

    #[test]
    fn read_only_connection_rejects_writes() {
        let mut app = connected_app();
        app.connection_type = super::ConnectionType::VictorHid;
        let err = app.agent_set_mode("Vac").unwrap_err();
        assert!(err.contains("read-only"));
    }

    #[test]
    fn voltage_clamps_and_edit_mask_blocks() {
        let mut app = connected_app();
        app.scpi_is_psu = true;
        app.psu_model = PsuModel {
            v_max: 60.0,
            i_max: 5.0,
            ..PsuModel::UNKNOWN
        };
        let applied = app.agent_set_psu_voltage(100.0).unwrap();
        assert_eq!(applied["applied"], serde_json::json!(60.0));
        assert_eq!(app.psu.set_v, 60.0);
        app.psu_edit.set_v = true;
        let err = app.agent_set_psu_voltage(10.0).unwrap_err();
        assert!(err.contains("being edited"));
        assert_eq!(app.psu.set_v, 60.0);
    }

    #[test]
    fn output_is_rejected_on_a_meter() {
        let mut app = connected_app();
        let err = app.agent_set_output(true).unwrap_err();
        assert!(err.contains("not a power supply"));
        assert!(!app.psu.output_on);
    }

    #[test]
    fn same_mode_does_not_clear_history_marker() {
        let mut app = connected_app();
        app.values.push_back(1.25);
        app.observe_live_sample();
        let changed = app.agent_set_mode("Vdc").unwrap();
        assert_eq!(changed["trace_cleared"], false);
        assert_eq!(app.values.len(), 1);
    }
}
