use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    time::Duration,
};

use mio::{Events, Interest, Poll, Token};
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::{MeterMode, RateCmd, ScpiMode};

const SERIAL_TOKEN: Token = Token(0);

impl super::MyApp {
    pub fn spawn_serial_task(&mut self) {
        if self.serial.is_none() {
            return;
        }

        let (tx_data, rx_data) = mpsc::channel::<Option<f64>>(100); // Channel for measurements
        let (tx_cmd, mut rx_cmd) = mpsc::channel::<String>(100); // Channel for commands
        let (tx_mode, rx_mode) = mpsc::channel::<MeterMode>(10); // Channel for mode updates
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>(); // Shutdown signal
        self.serial_rx = Some(rx_data);
        self.serial_tx = Some(tx_cmd.clone());
        self.mode_rx = Some(rx_mode);
        self.shutdown_tx = Some(shutdown_tx);

        let mut serial = self.serial.take().unwrap();
        let value_debug_shared = self.value_debug_shared.clone();
        let poll_interval_shared = self.poll_interval_shared.clone();
        let device_shared = self.device.clone();
        let lock_remote = self.lock_remote;
        let beeper_enabled = self.beeper_enabled;
        let cont_threshold = self.cont_threshold;
        let diod_threshold = self.diod_threshold;
        let curr_rate = self.curr_rate;
        let curr_mode = self.metermode;

        tokio::spawn(async move {
            let mut poll = Poll::new().unwrap();
            let mut events = Events::with_capacity(1);
            let mut readbuf = [0u8; 1024];
            let mut scpimode = ScpiMode::Idn;
            let mut command_queue: VecDeque<String> = VecDeque::new();
            let mut shutting_down = false;
            let mut drop_serial = false; // Flag to indicate when to drop serial
            let mut meas_count = 0; // Counter for measurement cycles
            let mut last_mode = curr_mode;
            let mut swap_diod_cont = false; // Default to no swap

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

            // Initial commands
            command_queue.push_back("*IDN?\n".to_string());
            // Queue initial configuration commands
            command_queue.push_back(format!(
                "RATE {}\n",
                RateCmd::default().get_opt(curr_rate).1
            ));
            if beeper_enabled {
                command_queue.push_back("SYST:BEEP:STATe ON\n".to_string());
            } else {
                command_queue.push_back("SYST:BEEP:STATe OFF\n".to_string());
            }
            command_queue.push_back(format!("CONT:THREshold {}\n", cont_threshold));
            command_queue.push_back(format!("DIOD:THREshold {}\n", diod_threshold));

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx, if !shutting_down => {
                        // Shutdown signal received, queue shutdown commands and stop MEAS? polling
                        if *value_debug_shared.lock().unwrap() {
                            println!("Shutdown signal received, processing remaining queue: {:?}", command_queue);
                        }
                        shutting_down = true;
                        command_queue.push_back("SYST:LOC\n".to_string());
                        command_queue.push_back("*RST\n".to_string());
                        if *value_debug_shared.lock().unwrap() {
                            println!("Queued SYST:LOC and *RST for shutdown, queue: {:?}", command_queue);
                        }
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
                                                    // Queue SYST:REM (if enabled) and MEAS? after sending *IDN?
                                                    if cmd == "*IDN?\n" && !shutting_down {
                                                        if lock_remote {
                                                            command_queue.push_back("SYST:REM\n".to_string());
                                                            if debug {
                                                                println!("Queued SYST:REM after *IDN?");
                                                            }
                                                        }
                                                        command_queue.push_back("MEAS?\n".to_string());
                                                        if debug {
                                                            println!(
                                                                "Queued MEAS? after sending *IDN?, queue: {:?}",
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
                                                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
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
                                                        let trimmed = content.trim_end();
                                                        if scpimode == ScpiMode::Idn {
                                                            let mut device = device_shared.lock().unwrap();
                                                            *device = trimmed.to_owned();
                                                            scpimode = ScpiMode::Meas;
                                                            if debug {
                                                                println!(
                                                                    "Updated device string: {}",
                                                                    *device
                                                                );
                                                            }
                                                            // Parse *IDN? response to determine DIOD/CONT swap
                                                            // this is to circumvent a bug on OWON XDM 1041/1241 meters
                                                            let parts: Vec<&str> = trimmed.split(',').collect();
                                                            if parts.len() >= 4 && parts[0] == "OWON" && (parts[1] == "XDM1041" || parts[1] == "XDM1241") {
                                                                let fw_version = parts[3].trim_start_matches('V');
                                                                let version_parts: Vec<&str> = fw_version.split('.').collect();
                                                                if version_parts.len() >= 3 {
                                                                    if let Ok(major) = version_parts[0].parse::<u32>() {
                                                                        if let Ok(minor) = version_parts[1].parse::<u32>() {
                                                                            // Swap DIOD/CONT for firmware < 4.3.0
                                                                            swap_diod_cont = major < 4 || (major == 4 && minor < 3);
                                                                            if debug {
                                                                                println!(
                                                                                    "Firmware detected: V{}.{}.{}, swap_diod_cont: {}",
                                                                                    major, minor, version_parts[2], swap_diod_cont
                                                                                );
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        } else if scpimode == ScpiMode::Meas {
                                                            // Handle quoted function responses
                                                            let unquoted = trimmed.trim_matches('"');
                                                            if unquoted.starts_with("VOLT") || unquoted.starts_with("CURR") ||
                                                               unquoted == "FREQ" || unquoted == "PER" ||
                                                               unquoted == "CAP" || unquoted == "CONT" ||
                                                               unquoted == "DIOD" || unquoted == "RES" ||
                                                               unquoted == "TEMP"
                                                            {
                                                                let mode = match unquoted {
                                                                    "VOLT" => MeterMode::Vdc,
                                                                    "VOLT AC" => MeterMode::Vac,
                                                                    "CURR" => MeterMode::Adc,
                                                                    "CURR AC" => MeterMode::Aac,
                                                                    "RES" => MeterMode::Res,
                                                                    "CAP" => MeterMode::Cap,
                                                                    "FREQ" => MeterMode::Freq,
                                                                    "PER" => MeterMode::Per,
                                                                    "TEMP" => MeterMode::Temp,
                                                                    // Handle DIOD/CONT based on firmware version
                                                                    "DIOD" => if swap_diod_cont { MeterMode::Cont } else { MeterMode::Diod },
                                                                    "CONT" => if swap_diod_cont { MeterMode::Diod } else { MeterMode::Cont },
                                                                    _ => continue,
                                                                };
                                                                if mode != last_mode {
                                                                    last_mode = mode;
                                                                    let _ = tx_mode.send(mode).await;
                                                                    if mode == MeterMode::Cont {
                                                                        if beeper_enabled {
                                                                            command_queue.push_back("SYST:BEEP:STATe ON\n".to_string());
                                                                        } else {
                                                                            command_queue.push_back("SYST:BEEP:STATe OFF\n".to_string());
                                                                        }
                                                                        command_queue.push_back(format!("CONT:THREshold {}\n", cont_threshold));
                                                                    } else if mode == MeterMode::Diod {
                                                                        if beeper_enabled {
                                                                            command_queue.push_back("SYST:BEEP:STATe ON\n".to_string());
                                                                        } else {
                                                                            command_queue.push_back("SYST:BEEP:STATe OFF\n".to_string());
                                                                        }
                                                                        command_queue.push_back(format!("DIOD:THREshold {}\n", diod_threshold));
                                                                    }
                                                                    if debug {
                                                                        println!("Sent mode update: {:?}", mode);
                                                                    }
                                                                }
                                                            } else if let Ok(meas) = trimmed.parse::<f64>() {
                                                                let _ = tx_data.send(Some(meas)).await;
                                                                if debug {
                                                                    println!("Sent measurement: {}", meas);
                                                                }
                                                                meas_count += 1;
                                                            }
                                                        }
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

                        // Queue MEAS? or FUNC? for continuous polling in Meas mode if queue is empty, only if not shutting down
                        if !shutting_down && scpimode == ScpiMode::Meas && command_queue.is_empty() {
                            if meas_count >= 10 {
                                command_queue.push_back("FUNC?\n".to_string());
                                meas_count = 0;
                                if debug {
                                    println!("Queued FUNC? for polling, queue: {:?}", command_queue);
                                }
                            } else {
                                command_queue.push_back("MEAS?\n".to_string());
                                if debug {
                                    println!("Queued MEAS? for polling, queue: {:?}", command_queue);
                                }
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
}