//! Read-only serial task for Victor 86E (ES51932 over CP2102).
//!
//! Unlike the SCPI `serial` task, this only listens for 14-byte frames and never
//! sends commands. Serial port must be opened at 19200 7o1 — see `victor_es519xx`.

use std::{
    io::{self, Read},
    time::Duration,
};

use mio::{Events, Interest, Poll, Token};
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::MeterMode;
use crate::victor_es519xx;

const SERIAL_TOKEN: Token = Token(1);

impl super::MyApp {
    pub fn spawn_victor_serial_task(&mut self) {
        if self.serial.is_none() {
            return;
        }

        let (tx_data, rx_data) = mpsc::channel::<Option<f64>>(100);
        let (tx_mode, rx_mode) = mpsc::channel::<(MeterMode, String)>(10);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.serial_rx = Some(rx_data);
        self.mode_rx = Some(rx_mode);
        self.shutdown_tx = Some(shutdown_tx);
        self.serial_tx = None;

        let mut serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let poll_interval_shared = self.poll_interval_shared.clone();
        let device_shared = self.device.clone();

        tokio::spawn(async move {
            let mut poll = Poll::new().unwrap();
            let mut events = Events::with_capacity(1);
            let mut readbuf = [0u8; 256];
            let mut packet_buf = Vec::with_capacity(512);
            let mut shutting_down = false;
            let mut last_mode = None::<MeterMode>;

            poll.registry()
                .register(&mut serial, SERIAL_TOKEN, Interest::READABLE)
                .unwrap();

            {
                let mut dev = device_shared.lock().unwrap();
                *dev = "Victor 86E (read only)".to_owned();
            }

            if *value_debug_shared.lock().unwrap() {
                println!("Victor 86E serial task started");
            }

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx, if !shutting_down => {
                        shutting_down = true;
                    }
                    _ = async {
                        let debug = *value_debug_shared.lock().unwrap();
                        let interval = *poll_interval_shared.lock().unwrap();

                        match poll.poll(&mut events, Some(Duration::from_millis(interval))) {
                            Ok(()) => {
                                for event in events.iter() {
                                    if !event.is_readable() {
                                        continue;
                                    }
                                    loop {
                                        match serial.read(&mut readbuf) {
                                            Ok(0) => break,
                                            Ok(count) => {
                                                if debug {
                                                    println!("Victor 86E received {} bytes", count);
                                                }
                                                let readings =
                                                    victor_es519xx::feed_bytes(
                                                        &mut packet_buf,
                                                        &readbuf[..count],
                                                    );
                                                for reading in readings {
                                                    if debug {
                                                        println!(
                                                            "Victor 86E reading: {} {:?}",
                                                            reading.value, reading.mode
                                                        );
                                                    }
                                                    let _ = tx_data.send(Some(reading.value)).await;
                                                    if last_mode != Some(reading.mode) {
                                                        last_mode = Some(reading.mode);
                                                        let _ = tx_mode
                                                            .send((reading.mode, reading.unit))
                                                            .await;
                                                    }
                                                }
                                            }
                                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                                            Err(e) => {
                                                if debug {
                                                    println!("Victor 86E read error: {}", e);
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if debug {
                                    println!("Victor 86E poll error: {}", e);
                                }
                            }
                        }

                        tokio::time::sleep(Duration::from_millis(interval)).await;
                    } => {}
                }

                if shutting_down {
                    break;
                }
            }

            let _ = poll.registry().deregister(&mut serial);
            drop(serial);
            if *value_debug_shared.lock().unwrap() {
                println!("Victor 86E serial task shutting down");
            }
        });
    }
}