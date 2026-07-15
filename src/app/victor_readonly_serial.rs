//! Read-only Victor serial task (86E ES51932 and 86B/C/D DM1107).
//!
//! Both meters stream measurement frames over CP2102 USB-UART; neither accepts
//! remote commands. Serial line settings are applied before spawn (see
//! [`super::open_victor_8n1_serial`] / [`super::open_victor_7o1_serial`]).
//!
//! I/O uses [`mio::Poll`] on a blocking thread. The poll wakes when the port is
//! readable; the idle timeout exists only so shutdown can be observed.

use std::{
    io::{self, Read},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use mio::{Events, Interest, Poll, Token};
use mio_serial::{ClearBuffer, SerialPort};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep_until, Instant};

use crate::multimeter::MeterMode;
use crate::victor_86bcd_capture::{self, Victor86bcdCaptureJob};
use crate::victor_dm1107::{self, Dm1107LiveUpdate};
use crate::victor_es519xx;

const SERIAL_TOKEN: Token = Token(2);
const IDLE_POLL_MS: u64 = 250;

/// Which Victor read-only wire protocol to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VictorReadonlyProtocol {
    /// Victor 86E — Cyrustek ES51932, 19200 7o1, 14-byte ASCII frames.
    Es519xx,
    /// Victor 86B/C/D — DM1107, 9600 8N1, 20-byte `a5 12` binary frames.
    Dm1107,
}

impl VictorReadonlyProtocol {
    fn device_label(self) -> &'static str {
        match self {
            Self::Es519xx => "Victor 86E (read only)",
            Self::Dm1107 => "Victor 86B/C/D serial (DM1107)",
        }
    }

    fn startup_log(self) -> &'static str {
        match self {
            Self::Es519xx => "Victor 86E serial task started (ES51932, 19200 7o1)",
            Self::Dm1107 => "Victor 86B/C/D serial task started (DM1107, 9600 8N1)",
        }
    }

    fn assert_dtr(self) -> bool {
        matches!(self, Self::Dm1107)
    }
}

struct ActiveCapture {
    job: Victor86bcdCaptureJob,
    buf: Vec<u8>,
    ends_at: Instant,
}

struct CaptureSide {
    rx: mpsc::Receiver<Victor86bcdCaptureJob>,
    status: Arc<std::sync::Mutex<victor_86bcd_capture::Victor86bcdCaptureStatus>>,
}

enum TaskDispatch {
    Es519xx {
        tx_value: mpsc::Sender<Option<f64>>,
        tx_mode: mpsc::Sender<(MeterMode, String)>,
    },
    Dm1107 {
        tx_live: mpsc::Sender<Dm1107LiveUpdate>,
    },
}

enum Decoder {
    Es519xx {
        packet_buf: Vec<u8>,
        last_mode: Option<MeterMode>,
    },
    Dm1107 {
        stream: victor_dm1107::Dm1107Stream,
    },
}

impl Decoder {
    fn new(protocol: VictorReadonlyProtocol) -> Self {
        match protocol {
            VictorReadonlyProtocol::Es519xx => Self::Es519xx {
                packet_buf: Vec::with_capacity(512),
                last_mode: None,
            },
            VictorReadonlyProtocol::Dm1107 => Self::Dm1107 {
                stream: victor_dm1107::Dm1107Stream::new(),
            },
        }
    }

    async fn feed_and_dispatch(
        &mut self,
        protocol: VictorReadonlyProtocol,
        chunk: &[u8],
        capture: &mut Option<ActiveCapture>,
        dispatch: &TaskDispatch,
        debug: bool,
    ) {
        if debug {
            match protocol {
                VictorReadonlyProtocol::Es519xx => {
                    println!("Victor 86E +{} bytes", chunk.len());
                }
                VictorReadonlyProtocol::Dm1107 => {
                    println!(
                        "Victor serial +{} bytes: {}",
                        chunk.len(),
                        victor_dm1107::hex_encode(&chunk[..chunk.len().min(32)]),
                    );
                }
            }
        }

        if let Some(cap) = capture {
            cap.buf.extend_from_slice(chunk);
        }

        match (self, dispatch) {
            (Self::Es519xx { packet_buf, last_mode }, TaskDispatch::Es519xx { tx_value, tx_mode }) => {
                for reading in victor_es519xx::feed_bytes(packet_buf, chunk) {
                    if debug {
                        println!(
                            "Victor 86E reading: {} {:?}",
                            reading.value, reading.mode
                        );
                    }
                    let _ = tx_value.send(Some(reading.value)).await;
                    if *last_mode != Some(reading.mode) {
                        *last_mode = Some(reading.mode);
                        let _ = tx_mode.send((reading.mode, reading.unit)).await;
                    }
                }
            }
            (Self::Dm1107 { stream }, TaskDispatch::Dm1107 { tx_live }) => {
                for update in victor_dm1107::feed_bytes(stream, chunk) {
                    if debug {
                        println!(
                            "Victor 86B/C/D: {} {} {:?}",
                            update.display, update.unit, update.mode
                        );
                    }
                    let _ = tx_live.send(update).await;
                }
            }
            _ => {}
        }
    }
}

fn write_capture_status(
    status: &Arc<std::sync::Mutex<victor_86bcd_capture::Victor86bcdCaptureStatus>>,
    msg: String,
    bytes: usize,
) {
    let mut st = status.lock().unwrap();
    st.message = msg;
    st.bytes_written = bytes;
}

fn finish_labeled_capture(cap: ActiveCapture, debug: bool, status: &CaptureSide) {
    let path: PathBuf = victor_86bcd_capture::default_samples_path();
    let job = cap.job;
    match victor_86bcd_capture::append_labeled_capture(
        &path,
        &job.context,
        job.duration_ms,
        &cap.buf,
    ) {
        Ok(written) => {
            let msg = format!(
                "Saved {written} bytes ({} ms) → {}",
                job.duration_ms,
                path.display()
            );
            if debug {
                println!("{msg}");
                if written > 0 {
                    let preview = victor_dm1107::hex_encode(&cap.buf[..written.min(48)]);
                    println!("  start: {preview}…");
                }
            }
            write_capture_status(&status.status, msg, written);
        }
        Err(e) => {
            let msg = format!("Capture write failed: {e}");
            eprintln!("{msg}");
            write_capture_status(&status.status, msg, 0);
        }
    }
}

fn drain_readable_serial(
    serial: &mut mio_serial::SerialStream,
    readbuf: &mut [u8],
) -> io::Result<Vec<u8>> {
    let mut chunk = Vec::new();
    loop {
        match serial.read(readbuf) {
            Ok(0) => break,
            Ok(count) => chunk.extend_from_slice(&readbuf[..count]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    Ok(chunk)
}

struct SerialReader {
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    reader_handle: tokio::task::JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl SerialReader {
    fn spawn(mut serial: mio_serial::SerialStream) -> Self {
        let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>(32);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = stop.clone();

        let reader_handle = tokio::task::spawn_blocking(move || {
            let mut poll = match Poll::new() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Victor serial Poll::new failed: {e}");
                    return;
                }
            };
            let mut events = Events::with_capacity(1);
            if let Err(e) = poll
                .registry()
                .register(&mut serial, SERIAL_TOKEN, Interest::READABLE)
            {
                eprintln!("Victor serial register failed: {e}");
                return;
            }

            let mut readbuf = [0u8; 256];
            loop {
                if stop_bg.load(Ordering::Acquire) {
                    break;
                }

                match poll.poll(&mut events, Some(Duration::from_millis(IDLE_POLL_MS))) {
                    Ok(()) => {
                        for event in events.iter() {
                            if !event.is_readable() {
                                continue;
                            }
                            match drain_readable_serial(&mut serial, &mut readbuf) {
                                Ok(chunk) if chunk.is_empty() => {}
                                Ok(chunk) => {
                                    if bytes_tx.blocking_send(chunk).is_err() {
                                        let _ = poll.registry().deregister(&mut serial);
                                        return;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Victor serial read error: {e}");
                                    let _ = poll.registry().deregister(&mut serial);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Victor serial poll error: {e}");
                        break;
                    }
                }
            }

            let _ = poll.registry().deregister(&mut serial);
        });

        Self {
            bytes_rx,
            reader_handle,
            stop,
        }
    }

    async fn next_chunk(&mut self) -> Option<Vec<u8>> {
        self.bytes_rx.recv().await
    }

    async fn shutdown(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.reader_handle.await;
    }
}

async fn run_serial_loop(
    mut serial: mio_serial::SerialStream,
    protocol: VictorReadonlyProtocol,
    mut shutdown_rx: oneshot::Receiver<()>,
    dispatch: TaskDispatch,
    mut capture: Option<CaptureSide>,
    value_debug_shared: Arc<std::sync::Mutex<bool>>,
    device_shared: Arc<std::sync::Mutex<String>>,
) {
    let _ = serial.clear(ClearBuffer::Input);
    if protocol.assert_dtr() {
        let _ = serial.write_data_terminal_ready(true);
    }

    if *value_debug_shared.lock().unwrap() {
        println!("{}", protocol.startup_log());
        if let Some(name) = serial.name() {
            println!("Victor serial port: {name}");
        }
    }

    {
        let mut dev = device_shared.lock().unwrap();
        *dev = protocol.device_label().to_owned();
    }

    let mut reader = SerialReader::spawn(serial);
    let mut decoder = Decoder::new(protocol);
    let mut active_capture: Option<ActiveCapture> = None;
    let mut shutting_down = false;

    loop {
        let capture_deadline = active_capture.as_ref().map(|c| c.ends_at);

        tokio::select! {
            _ = &mut shutdown_rx, if !shutting_down => {
                shutting_down = true;
            }
            job = async {
                match capture.as_mut() {
                    Some(side) => side.rx.recv().await,
                    None => std::future::pending().await,
                }
            }, if active_capture.is_none() && capture.is_some() => {
                if let Some(job) = job {
                    if let Some(side) = &capture {
                        write_capture_status(
                            &side.status,
                            format!(
                                "Recording {} ms for {}…",
                                job.duration_ms,
                                job.context.summary(),
                            ),
                            0,
                        );
                    }
                    let duration = Duration::from_millis(job.duration_ms);
                    active_capture = Some(ActiveCapture {
                        job,
                        buf: Vec::new(),
                        ends_at: Instant::now() + duration,
                    });
                }
            }
            _ = async {
                if let Some(deadline) = capture_deadline {
                    sleep_until(deadline).await;
                }
            }, if capture_deadline.is_some() => {
                if let (Some(cap), Some(side)) = (active_capture.take(), &capture) {
                    let debug = *value_debug_shared.lock().unwrap();
                    finish_labeled_capture(cap, debug, side);
                }
            }
            chunk = reader.next_chunk(), if !shutting_down => {
                let Some(chunk) = chunk else { break; };
                let debug = *value_debug_shared.lock().unwrap();
                decoder
                    .feed_and_dispatch(protocol, &chunk, &mut active_capture, &dispatch, debug)
                    .await;
            }
        }

        if shutting_down {
            break;
        }
    }

    reader.shutdown().await;

    if *value_debug_shared.lock().unwrap() {
        println!("Victor serial task shutting down ({protocol:?})");
    }
}

impl super::MyApp {
    pub fn spawn_victor_readonly_serial_task(&mut self, protocol: VictorReadonlyProtocol) {
        if self.serial.is_none() {
            return;
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);
        self.serial_tx = None;

        let serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let device_shared = self.device.clone();

        let (dispatch, capture) = match protocol {
            VictorReadonlyProtocol::Es519xx => {
                let (tx_value, rx_value) = mpsc::channel::<Option<f64>>(100);
                let (tx_mode, rx_mode) = mpsc::channel::<(MeterMode, String)>(10);
                self.serial_rx = Some(rx_value);
                self.mode_rx = Some(rx_mode);
                self.victor_86bcd_rx = None;
                (
                    TaskDispatch::Es519xx {
                        tx_value,
                        tx_mode,
                    },
                    None,
                )
            }
            VictorReadonlyProtocol::Dm1107 => {
                let (capture_tx, capture_rx) = mpsc::channel::<Victor86bcdCaptureJob>(8);
                let (tx_live, rx_live) = mpsc::channel::<Dm1107LiveUpdate>(100);
                self.victor_86bcd_capture_tx = Some(capture_tx);
                self.victor_86bcd_rx = Some(rx_live);
                self.serial_rx = None;
                self.mode_rx = None;
                (
                    TaskDispatch::Dm1107 { tx_live },
                    Some(CaptureSide {
                        rx: capture_rx,
                        status: self.victor_86bcd_capture_status_shared.clone(),
                    }),
                )
            }
        };

        tokio::spawn(async move {
            run_serial_loop(
                serial,
                protocol,
                shutdown_rx,
                dispatch,
                capture,
                value_debug_shared,
                device_shared,
            )
            .await;
        });
    }

    pub fn spawn_victor_86e_serial_task(&mut self) {
        self.spawn_victor_readonly_serial_task(VictorReadonlyProtocol::Es519xx);
    }

    pub fn spawn_victor_86bcd_serial_task(&mut self) {
        self.spawn_victor_readonly_serial_task(VictorReadonlyProtocol::Dm1107);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_labels_differ() {
        assert_ne!(
            VictorReadonlyProtocol::Es519xx.device_label(),
            VictorReadonlyProtocol::Dm1107.device_label(),
        );
    }
}