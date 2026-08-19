use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use mio::{Events, Interest, Poll, Token};
use mio_serial::SerialStream;
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::{MeterMode, ScpiMode};
use crate::scpi_macro::{self, MeterStatus, ReplyClass};

const SERIAL_TOKEN: Token = Token(0);

/// How often to start a new FUNC/RATE/BEEP/AUTO(/RANGE) GUI sync.
/// MEAS? keeps running on the poll interval and is never paused for this.
const UI_SYNC_INTERVAL: Duration = Duration::from_millis(500);
/// If a status query (e.g. `RANGE?` in CONT/DIOD) never replies, skip that
/// step. MEAS? is independent and must not stall.
const STATUS_TIMEOUT: Duration = Duration::from_millis(1000);
const MEAS_TIMEOUT: Duration = Duration::from_secs(2);

/// One GUI status query. Replies never look like `MEAS?` (no scientific
/// notation), so they can share the wire with measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusStep {
    Func,
    Rate,
    Beep,
    Auto,
    Range,
}

impl StatusStep {
    fn cmd(self) -> &'static str {
        match self {
            Self::Func => "FUNC?\n",
            Self::Rate => "RATE?\n",
            Self::Beep => "SYST:BEEP:STATe?\n",
            Self::Auto => "AUTO?\n",
            Self::Range => "RANGE?\n",
        }
    }
}

struct Session {
    scpimode: ScpiMode,
    awaiting_idn: bool,
    idn_since: Option<Instant>,
    retry_idn: bool,
    idn_tries_left: u8,
    awaiting_meas: bool,
    meas_since: Option<Instant>,
    /// `Some` = a GUI sync is in progress (status queries only).
    in_status_cycle: bool,
    status: Option<StatusStep>,
    status_since: Option<Instant>,
    next_status: Option<StatusStep>,
    snap: MeterStatus,
    skip_rate: bool,
    skip_beep: bool,
    skip_auto: bool,
    skip_range: bool,
    last_status_done: Instant,
    last_mode: MeterMode,
    swap_diod_cont: bool,
}

impl Session {
    fn new(mode: MeterMode) -> Self {
        Self {
            scpimode: ScpiMode::Idn,
            awaiting_idn: false,
            idn_since: None,
            retry_idn: true,
            idn_tries_left: 5,
            awaiting_meas: false,
            meas_since: None,
            in_status_cycle: false,
            status: None,
            status_since: None,
            next_status: None,
            snap: MeterStatus::default(),
            skip_rate: false,
            skip_beep: false,
            skip_auto: false,
            skip_range: false,
            last_status_done: Instant::now(),
            last_mode: mode,
            swap_diod_cont: false,
        }
    }

    fn ask_status(&mut self, step: StatusStep) {
        self.next_status = Some(step);
        self.status = Some(step);
        self.status_since = None;
    }

    fn start_status_cycle(&mut self) {
        self.in_status_cycle = true;
        self.snap = MeterStatus::default();
        self.skip_rate = false;
        self.skip_beep = false;
        self.skip_auto = false;
        self.skip_range = false;
        self.ask_status(StatusStep::Func);
    }

    fn end_status_cycle(&mut self) {
        self.in_status_cycle = false;
        self.status = None;
        self.status_since = None;
        self.next_status = None;
        self.last_status_done = Instant::now();
    }

    /// Next missing GUI field, or `None` if the snapshot is complete.
    fn next_missing_status(&self) -> Option<StatusStep> {
        if self.snap.rate.is_none() && !self.skip_rate {
            return Some(StatusStep::Rate);
        }
        if self.snap.beep.is_none() && !self.skip_beep {
            return Some(StatusStep::Beep);
        }
        if self.snap.auto.is_none() && !self.skip_auto {
            return Some(StatusStep::Auto);
        }
        if self.snap.auto == Some(false)
            && self.last_mode.has_manual_range()
            && self.snap.range.is_none()
            && !self.skip_range
        {
            return Some(StatusStep::Range);
        }
        None
    }

    fn continue_status(&mut self, debug: bool) {
        if !self.in_status_cycle {
            return;
        }
        match self.next_missing_status() {
            Some(step) => {
                if self.status != Some(step) {
                    self.ask_status(step);
                }
            }
            None => {
                if debug && self.snap.auto == Some(true) {
                    println!("AUTO=1, skipping RANGE? (live window is not a manual range)");
                }
                self.next_status = None;
                self.status = None;
                self.status_since = None;
            }
        }
    }
}

/// mio is edge-triggered: a single WRITABLE after open is easy to miss once
/// `*IDN?` has been sent and later bootstrap commands arrive with the socket
/// still writable. Drain whenever the queue is non-empty; WouldBlock waits.
fn drain_sets(serial: &mut SerialStream, command_queue: &mut VecDeque<String>, debug: bool) {
    while let Some(cmd) = command_queue.front() {
        if scpi_macro::is_query(cmd) {
            break;
        }
        if !write_cmd(serial, cmd, debug) {
            break;
        }
        command_queue.pop_front();
    }
}

/// UI-queued `FUNC?`/`RANGE?` from an older connect path: do not send them.
/// Fold into one sequenced GUI sync instead of pipelining five queries.
fn coalesce_ui_queries(
    command_queue: &mut VecDeque<String>,
    refresh_requested: &std::sync::atomic::AtomicBool,
    debug: bool,
) {
    while let Some(cmd) = command_queue.front() {
        if !scpi_macro::is_query(cmd) {
            break;
        }
        if debug {
            println!("Coalescing UI query into status refresh: {:?}", cmd);
        }
        refresh_requested.store(true, Ordering::SeqCst);
        command_queue.pop_front();
    }
}

fn write_cmd(serial: &mut SerialStream, cmd: &str, debug: bool) -> bool {
    if debug {
        println!("Sending: {:?}", cmd);
    }
    match serial.write_all(cmd.as_bytes()) {
        Ok(()) => {
            if debug {
                println!("Command sent: {:?}", cmd);
            }
            true
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            if debug {
                println!("Serial write would block for {:?}, waiting", cmd);
            }
            false
        }
        Err(e) => {
            if debug {
                println!("Failed to send command {:?}: {}", cmd, e);
            }
            false
        }
    }
}

/// Pull one SCPI reply (`…\n`, optional CR) out of the accumulation buffer.
fn take_scpi_line(buf: &mut String) -> Option<String> {
    let pos = buf.find('\n')?;
    let mut line = buf[..pos].to_owned();
    if line.ends_with('\r') {
        line.pop();
    }
    buf.drain(..=pos);
    Some(line)
}

fn discard_pending_input(serial: &mut SerialStream) {
    let mut buf = [0u8; 1024];
    loop {
        match serial.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

impl super::MyApp {
    pub fn spawn_serial_task(&mut self) {
        if self.serial.is_none() {
            return;
        }

        let (tx_data, rx_data) = mpsc::channel::<Option<f64>>(100); // Channel for measurements
        let (tx_cmd, mut rx_cmd) = mpsc::channel::<String>(100); // Channel for commands
        let (tx_mode, rx_mode) = mpsc::channel::<(MeterMode, String)>(10);
        let (tx_status, rx_status) = mpsc::channel::<MeterStatus>(16);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>(); // Shutdown signal
        self.serial_rx = Some(rx_data);
        self.serial_tx = Some(tx_cmd.clone());
        self.mode_rx = Some(rx_mode);
        self.status_rx = Some(rx_status);
        self.shutdown_tx = Some(shutdown_tx);

        let mut serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let poll_interval_shared = self.poll_interval_shared.clone();
        let device_shared = self.device.clone();
        let poll_ready = self.poll_ready.clone();
        let refresh_requested = self.refresh_requested.clone();
        let rst_on_disconnect = self.rst_on_disconnect;
        let curr_mode = self.metermode;

        tokio::spawn(async move {
            let mut poll = Poll::new().unwrap();
            let mut events = Events::with_capacity(1);
            let mut readbuf = [0u8; 1024];
            let mut line_buf = String::new();
            let mut command_queue: VecDeque<String> = VecDeque::new();
            let mut shutting_down = false;
            let mut session = Session::new(curr_mode);

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

            // Drop leftover MEAS? replies from a previous session sitting in the UART.
            discard_pending_input(&mut serial);

            // Identify first. Dialect bootstrap + user connect macros are queued
            // from the UI after the IDN reply is parsed.

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx, if !shutting_down => {
                        if *value_debug_shared.lock().unwrap() {
                            println!("Shutdown signal received, processing remaining queue: {:?}", command_queue);
                        }
                        shutting_down = true;
                        session.awaiting_meas = false;
                        session.in_status_cycle = false;
                        session.status = None;
                        session.next_status = None;
                        session.retry_idn = false;
                        command_queue.push_back("SYST:LOC\n".to_string());
                        if rst_on_disconnect {
                            command_queue.push_back("*RST\n".to_string());
                        }
                        if *value_debug_shared.lock().unwrap() {
                            println!("Queued SYST:LOC and *RST (if set) for shutdown, queue: {:?}", command_queue);
                        }
                    }
                    _ = async {
                        let debug = *value_debug_shared.lock().unwrap();
                        let interval = *poll_interval_shared.lock().unwrap();

                        if debug {
                            println!("Starting poll loop, queue: {:?}", command_queue);
                        }

                        while let Ok(cmd) = rx_cmd.try_recv() {
                            if debug {
                                println!("Queuing command from UI: {:?}", cmd);
                            }
                            command_queue.push_back(cmd);
                        }

                        match poll.poll(&mut events, Some(Duration::from_millis(interval))) {
                            Ok(()) => {
                                if debug {
                                    println!(
                                        "Poll returned events: {:?}",
                                        events.iter().collect::<Vec<_>>()
                                    );
                                }

                                for event in events.iter() {
                                    if event.is_readable() {
                                        if debug {
                                            println!("Readable event detected");
                                        }
                                        loop {
                                            match serial.read(&mut readbuf) {
                                                Ok(count) => {
                                                    let chunk = String::from_utf8_lossy(
                                                        &readbuf[..count],
                                                    );
                                                    if debug {
                                                        println!("Received: {:?}", chunk);
                                                    }
                                                    line_buf.push_str(&chunk);
                                                    while let Some(line) = take_scpi_line(&mut line_buf) {
                                                        let trimmed = line.trim();
                                                        if trimmed.is_empty() {
                                                            continue;
                                                        }
                                                        handle_line(
                                                            &mut session,
                                                            trimmed,
                                                            &device_shared,
                                                            &tx_mode,
                                                            &tx_status,
                                                            &tx_data,
                                                            debug,
                                                        ).await;
                                                    }
                                                }
                                                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
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

                        on_timeouts(&mut session, &tx_status, debug).await;

                        drain_sets(&mut serial, &mut command_queue, debug);
                        if !shutting_down {
                            coalesce_ui_queries(
                                &mut command_queue,
                                &refresh_requested,
                                debug,
                            );
                        }

                        if !shutting_down && (session.scpimode == ScpiMode::Idn
                            || session.awaiting_idn
                            || session.retry_idn)
                        {
                            if session.retry_idn
                                && !session.awaiting_idn
                                && write_cmd(&mut serial, "*IDN?\n", debug)
                            {
                                session.retry_idn = false;
                                session.awaiting_idn = true;
                                session.idn_since = Some(Instant::now());
                            }
                        } else if !shutting_down
                            && command_queue.is_empty()
                            && poll_ready.load(Ordering::SeqCst)
                        {
                            // MEAS? first, always, on this loop's roster. Status never
                            // occupies the measurement slot.
                            if !session.awaiting_meas && write_cmd(&mut serial, "MEAS?\n", debug)
                            {
                                session.awaiting_meas = true;
                                session.meas_since = Some(Instant::now());
                            }

                            let want_cycle = refresh_requested.load(Ordering::SeqCst)
                                || session.last_status_done.elapsed() >= UI_SYNC_INTERVAL;
                            if !session.in_status_cycle && want_cycle {
                                refresh_requested.store(false, Ordering::SeqCst);
                                session.start_status_cycle();
                            }

                            if let Some(step) = session.next_status {
                                if write_cmd(&mut serial, step.cmd(), debug) {
                                    session.next_status = None;
                                    session.status = Some(step);
                                    session.status_since = Some(Instant::now());
                                }
                            }
                        }

                        tokio::time::sleep(Duration::from_millis(interval)).await;
                    } => {}
                }

                if shutting_down {
                    let debug = *value_debug_shared.lock().unwrap();
                    while let Ok(cmd) = rx_cmd.try_recv() {
                        command_queue.push_back(cmd);
                    }
                    drain_sets(&mut serial, &mut command_queue, debug);
                    if debug {
                        println!("Shutdown flush done, leftover queue: {:?}", command_queue);
                    }
                    break;
                }
            }

            if *value_debug_shared.lock().unwrap() {
                println!("Cleaning up serial task");
            }
            let _ = poll.registry().deregister(&mut serial);
            drop(serial);
        });
    }
}

async fn handle_line(
    session: &mut Session,
    trimmed: &str,
    device_shared: &std::sync::Arc<std::sync::Mutex<String>>,
    tx_mode: &mpsc::Sender<(MeterMode, String)>,
    tx_status: &mpsc::Sender<MeterStatus>,
    tx_data: &mpsc::Sender<Option<f64>>,
    debug: bool,
) {
    let unquoted = trimmed.trim_matches('"');
    let class = scpi_macro::classify_reply(unquoted);

    if session.scpimode == ScpiMode::Idn || session.awaiting_idn {
        if !scpi_macro::looks_like_idn(trimmed) {
            if debug {
                println!("Ignoring non-IDN while waiting for *IDN?: {trimmed:?}");
            }
            if session.idn_tries_left > 0 {
                session.idn_tries_left -= 1;
                session.retry_idn = true;
                session.awaiting_idn = false;
                session.idn_since = None;
            }
            return;
        }
        let mut device = device_shared.lock().unwrap();
        *device = trimmed.to_owned();
        session.scpimode = ScpiMode::Meas;
        session.awaiting_idn = false;
        session.idn_since = None;
        session.retry_idn = false;
        if debug {
            println!("Updated device string: {}", *device);
        }
        let parts: Vec<&str> = trimmed.split(',').collect();
        if parts.len() >= 4
            && parts[0] == "OWON"
            && (parts[1] == "XDM1041" || parts[1] == "XDM1241")
        {
            let fw_version = parts[3].trim_start_matches('V');
            let version_parts: Vec<&str> = fw_version.split('.').collect();
            if version_parts.len() >= 3 {
                if let Ok(major) = version_parts[0].parse::<u32>() {
                    if let Ok(minor) = version_parts[1].parse::<u32>() {
                        session.swap_diod_cont = major < 4 || (major == 4 && minor < 3);
                        if debug {
                            println!(
                                "Firmware detected: V{}.{}.{}, swap_diod_cont: {}",
                                major, minor, version_parts[2], session.swap_diod_cont
                            );
                        }
                    }
                }
            }
        }
        return;
    }

    match class {
        ReplyClass::Meas => {
            if let Ok(meas) = trimmed.parse::<f64>() {
                let _ = tx_data.send(Some(meas)).await;
                session.awaiting_meas = false;
                session.meas_since = None;
                if debug {
                    println!("Sent measurement: {}", meas);
                }
            }
        }
        ReplyClass::Func => {
            apply_func(session, unquoted, tx_mode, debug).await;
            if session.in_status_cycle {
                session.continue_status(debug);
                maybe_flush_status(session, tx_status, debug).await;
            }
        }
        ReplyClass::Rate => {
            if session.in_status_cycle {
                session.snap.rate = Some(unquoted.to_owned());
                session.continue_status(debug);
                maybe_flush_status(session, tx_status, debug).await;
            }
        }
        ReplyClass::Beep => {
            if session.in_status_cycle {
                if let Some(on) = scpi_macro::parse_beep_reply(unquoted) {
                    session.snap.beep = Some(on);
                }
                session.continue_status(debug);
                maybe_flush_status(session, tx_status, debug).await;
            }
        }
        ReplyClass::Auto => {
            if session.in_status_cycle {
                session.snap.auto = Some(unquoted == "1" || unquoted.eq_ignore_ascii_case("ON"));
                session.continue_status(debug);
                maybe_flush_status(session, tx_status, debug).await;
            }
        }
        ReplyClass::Range => {
            if session.in_status_cycle {
                if let Some(raw) = scpi_macro::parse_range_reply(unquoted) {
                    session.snap.range = Some(raw);
                }
                session.continue_status(debug);
                maybe_flush_status(session, tx_status, debug).await;
            }
        }
        ReplyClass::Unknown => {
            if debug {
                println!("Ignored SCPI reply: {unquoted:?}");
            }
        }
    }
}

async fn apply_func(
    session: &mut Session,
    unquoted: &str,
    tx_mode: &mpsc::Sender<(MeterMode, String)>,
    debug: bool,
) {
    let Some(mut mode) = MeterMode::from_func_reply(unquoted) else {
        return;
    };
    if session.swap_diod_cont {
        mode = match mode {
            MeterMode::Diod => MeterMode::Cont,
            MeterMode::Cont => MeterMode::Diod,
            other => other,
        };
    }
    if mode != session.last_mode {
        session.last_mode = mode;
        let unit = mode.default_unit().to_owned();
        let _ = tx_mode.send((mode, unit)).await;
        if debug {
            println!("Sent mode update: {:?}", mode);
        }
    }
}

async fn maybe_flush_status(
    session: &mut Session,
    tx_status: &mpsc::Sender<MeterStatus>,
    debug: bool,
) {
    if !session.in_status_cycle || session.next_missing_status().is_some() {
        return;
    }
    if debug {
        println!(
            "Status snapshot: rate={:?} beep={:?} auto={:?} range={:?}",
            session.snap.rate, session.snap.beep, session.snap.auto, session.snap.range
        );
    }
    let _ = tx_status.send(std::mem::take(&mut session.snap)).await;
    session.end_status_cycle();
}

async fn on_timeouts(session: &mut Session, tx_status: &mpsc::Sender<MeterStatus>, debug: bool) {
    if session.awaiting_idn
        && session
            .idn_since
            .is_some_and(|t| t.elapsed() >= STATUS_TIMEOUT)
    {
        if debug {
            println!("SCPI timeout waiting for Idn");
        }
        if session.idn_tries_left > 0 {
            session.idn_tries_left -= 1;
            session.retry_idn = true;
        }
        session.awaiting_idn = false;
        session.idn_since = None;
    }

    if session.awaiting_meas
        && session
            .meas_since
            .is_some_and(|t| t.elapsed() >= MEAS_TIMEOUT)
    {
        if debug {
            println!("SCPI timeout waiting for Meas");
        }
        session.awaiting_meas = false;
        session.meas_since = None;
    }

    if let Some(step) = session.status {
        if session
            .status_since
            .is_some_and(|t| t.elapsed() >= STATUS_TIMEOUT)
        {
            if debug {
                println!("SCPI timeout waiting for {step:?}");
            }
            match step {
                StatusStep::Func => session.ask_status(StatusStep::Rate),
                StatusStep::Rate => {
                    session.skip_rate = true;
                    session.continue_status(debug);
                }
                StatusStep::Beep => {
                    session.skip_beep = true;
                    session.continue_status(debug);
                }
                StatusStep::Auto => {
                    session.skip_auto = true;
                    session.continue_status(debug);
                }
                StatusStep::Range => {
                    session.skip_range = true;
                    session.continue_status(debug);
                }
            }
            maybe_flush_status(session, tx_status, debug).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::take_scpi_line;

    #[test]
    fn splits_batched_status_replies() {
        let mut buf = String::from("\"VOLT AC\"\r\nF\r\nOFF\r\n1\r\n5.072094E-01\r\n");
        let mut lines = Vec::new();
        while let Some(line) = take_scpi_line(&mut buf) {
            lines.push(line);
        }
        assert_eq!(lines, ["\"VOLT AC\"", "F", "OFF", "1", "5.072094E-01",]);
        assert!(buf.is_empty());
    }

    #[test]
    fn keeps_partial_line() {
        let mut buf = String::from("VOLT");
        assert!(take_scpi_line(&mut buf).is_none());
        buf.push_str(" AC\r\nF\r\n");
        assert_eq!(take_scpi_line(&mut buf).as_deref(), Some("VOLT AC"));
        assert_eq!(take_scpi_line(&mut buf).as_deref(), Some("F"));
        assert!(buf.is_empty());
    }
}
