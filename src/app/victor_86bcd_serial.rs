//! Victor DM1107 serial task (mio-serial, 9600 8N1).
//!
//! Newer Victor meters (e.g. 86D): binary stream from opto-isolated USB link.
//! Live display values use [`victor_dm1107::feed_bytes`] (20-byte `a5 12` frame decode).
//!
//! Serial I/O uses [`mio::Poll`] on a blocking thread (same integration `mio-serial`
//! expects). The poll wakes as soon as the port is readable; the timeout is only
//! checked when idle so shutdown can be observed.

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

use crate::victor_86bcd_capture::{self, Victor86bcdCaptureJob};
use crate::victor_dm1107::{self, Dm1107LiveUpdate};

const SERIAL_TOKEN: Token = Token(2);
/// Max wait when no bytes are arriving; lets the reader thread observe `stop`.
const IDLE_POLL_MS: u64 = 250;

struct ActiveCapture {
    job: Victor86bcdCaptureJob,
    buf: Vec<u8>,
    ends_at: Instant,
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

fn process_chunk(
    chunk: &[u8],
    stream: &mut victor_dm1107::Dm1107Stream,
    active_capture: &mut Option<ActiveCapture>,
    debug: bool,
) -> Vec<Dm1107LiveUpdate> {
    if debug {
        println!(
            "Victor serial +{} bytes: {}",
            chunk.len(),
            victor_dm1107::hex_encode(&chunk[..chunk.len().min(32)]),
        );
    }
    if let Some(cap) = active_capture {
        cap.buf.extend_from_slice(chunk);
    }
    victor_dm1107::feed_bytes(stream, chunk)
}

async fn push_live_updates(
    tx_live: &mpsc::Sender<Dm1107LiveUpdate>,
    updates: Vec<Dm1107LiveUpdate>,
    debug: bool,
) {
    for update in updates {
        if debug {
            println!(
                "Victor 86B/C/D: {} {} {:?}",
                update.display, update.unit, update.mode
            );
        }
        let _ = tx_live.send(update).await;
    }
}

/// Background reader: `mio::Poll` blocks until the OS reports readability.
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
    mut shutdown_rx: oneshot::Receiver<()>,
    mut capture_rx: mpsc::Receiver<Victor86bcdCaptureJob>,
    tx_live: mpsc::Sender<Dm1107LiveUpdate>,
    value_debug_shared: Arc<std::sync::Mutex<bool>>,
    capture_status_shared: Arc<std::sync::Mutex<crate::victor_86bcd_capture::Victor86bcdCaptureStatus>>,
    device_shared: Arc<std::sync::Mutex<String>>,
) {
    let _ = serial.clear(ClearBuffer::Input);
    let _ = serial.write_data_terminal_ready(true);

    if *value_debug_shared.lock().unwrap() {
        println!("Victor 86B/C/D serial task started (DM1107 decode, 9600 8N1)");
        if let Some(name) = serial.name() {
            println!("Victor 86B/C/D port: {name}");
        }
    }

    {
        let mut dev = device_shared.lock().unwrap();
        *dev = "Victor 86B/C/D serial (DM1107)".to_owned();
    }

    let mut reader = SerialReader::spawn(serial);
    let mut stream = victor_dm1107::Dm1107Stream::new();
    let mut active_capture: Option<ActiveCapture> = None;
    let mut shutting_down = false;

    let write_status = |msg: String, bytes: usize| {
        let mut st = capture_status_shared.lock().unwrap();
        st.message = msg;
        st.bytes_written = bytes;
    };

    let finish_capture = |cap: ActiveCapture, debug: bool, write_status: &dyn Fn(String, usize)| {
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
                write_status(msg, written);
            }
            Err(e) => {
                let msg = format!("Capture write failed: {e}");
                eprintln!("{msg}");
                write_status(msg, 0);
            }
        }
    };

    loop {
        let capture_deadline = active_capture.as_ref().map(|c| c.ends_at);

        tokio::select! {
            _ = &mut shutdown_rx, if !shutting_down => {
                shutting_down = true;
            }
            job = capture_rx.recv(), if active_capture.is_none() => {
                if let Some(job) = job {
                    let duration = Duration::from_millis(job.duration_ms);
                    write_status(
                        format!(
                            "Recording {} ms for {}…",
                            job.duration_ms,
                            job.context.summary(),
                        ),
                        0,
                    );
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
                if let Some(cap) = active_capture.take() {
                    let debug = *value_debug_shared.lock().unwrap();
                    finish_capture(cap, debug, &write_status);
                }
            }
            chunk = reader.next_chunk(), if !shutting_down => {
                let Some(chunk) = chunk else { break; };
                let debug = *value_debug_shared.lock().unwrap();
                let updates = process_chunk(&chunk, &mut stream, &mut active_capture, debug);
                push_live_updates(&tx_live, updates, debug).await;
            }
        }

        if shutting_down {
            break;
        }
    }

    reader.shutdown().await;

    if *value_debug_shared.lock().unwrap() {
        println!("Victor 86B/C/D serial task shutting down");
    }
}

impl super::MyApp {
    pub fn spawn_victor_86bcd_serial_task(&mut self) {
        if self.serial.is_none() {
            return;
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (capture_tx, capture_rx) = mpsc::channel::<Victor86bcdCaptureJob>(8);
        let (tx_live, rx_live) = mpsc::channel::<Dm1107LiveUpdate>(100);

        self.shutdown_tx = Some(shutdown_tx);
        self.victor_86bcd_capture_tx = Some(capture_tx);
        self.victor_86bcd_rx = Some(rx_live);
        self.serial_rx = None;
        self.serial_tx = None;
        self.mode_rx = None;

        let serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let capture_status_shared = self.victor_86bcd_capture_status_shared.clone();
        let device_shared = self.device.clone();

        tokio::spawn(async move {
            run_serial_loop(
                serial,
                shutdown_rx,
                capture_rx,
                tx_live,
                value_debug_shared,
                capture_status_shared,
                device_shared,
            )
            .await;
        });
    }
}